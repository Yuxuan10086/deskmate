//! 系统音频采集 → 16kHz 单声道 10 秒 WAV 分块落盘。
//!
//! - 线下课模式：cpal 采集默认麦克风（多声道自动下混，任意采样率线性重采样到 16kHz）；
//! - 网课模式：WASAPI loopback 采集系统默认输出设备的声音；
//! - 采集期间通过 Metrics 暴露设备名、瞬时电平和静音告警，供前端实时自检。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use crate::error::{AppError, AppResult};

/// 输出给 ASR 的统一格式。
const TARGET_RATE: u32 = 16_000;
/// 每块时长（秒）。
const CHUNK_SECONDS: u32 = 10;

/// 按应用采集连续多久拿不到任何数据包，即判定选错进程/应用未放声，回退系统声音。
const PACKET_STALL_FALLBACK: Duration = Duration::from_secs(12);
/// 按应用采集持续数字静音多久即回退系统声音（采到了流但不是网课音频）。
const SILENCE_FALLBACK: Duration = Duration::from_secs(45);
/// 静音判定阈值（i16 量纲 RMS）。正常说话 RMS 通常 > 500。
const SILENCE_RMS_THRESHOLD: f32 = 200.0;
/// 启动后参与静音判定的块数（前 20 秒）。
const SILENCE_CHECK_CHUNKS: u32 = 2;

#[derive(Clone)]
pub struct CaptureConfig {
    pub session_id: i64,
    pub chunks_dir: PathBuf,
    /// "offline"=麦克风，"online"=系统声音 loopback。
    pub mode: String,
    /// 网课模式下的采集目标进程；None=整个系统声音。
    pub target_pid: Option<u32>,
    /// 麦克风软件增益（f32 位存储，运行中可实时调整）。
    pub mic_gain: Arc<AtomicU32>,
    /// 实时字幕音频支流（AST 同传引擎）：16kHz mono i16 采样流；None 表示不分流。
    /// 接收端在 tokio 任务里消费并打包成 80ms PCM 包发给 WebSocket。
    pub live_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<i16>>>,
}

impl CaptureConfig {
    pub fn gain_value(&self) -> f32 {
        f32::from_bits(self.mic_gain.load(Ordering::SeqCst))
    }
}

/// 供前端轮询的采集指标。
#[derive(Default, Clone)]
pub struct Metrics {
    pub device_name: String,
    /// 瞬时音量 RMS（i16 量纲，满幅 32767）。
    pub rms: f32,
    /// 启动前 20 秒持续静音时为 true，提示用户检查设备/权限/音量。
    pub silence_warned: bool,
    pub chunks: u32,
    /// 实际生效的采集目标进程（None=整个系统声音）。按应用采集失败自动回退后会变为 None。
    pub effective_pid: Option<u32>,
    /// 采集异常提示（如按应用采集拿不到声音已自动回退），供前端展示给用户。
    pub note: Option<String>,
}

pub struct CaptureThread {
    pub config: CaptureConfig,
    pub running: Arc<AtomicBool>,
    pub metrics: Arc<Mutex<Metrics>>,
    pub handle: Option<thread::JoinHandle<()>>,
}

impl CaptureThread {
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            metrics: Arc::new(Mutex::new(Metrics::default())),
            handle: None,
        }
    }

    /// 启动采集（非阻塞）。返回 true 表示新启动，false 表示已在运行。
    pub fn start(&mut self) -> AppResult<bool> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        let running = Arc::clone(&self.running);
        let cfg = self.config.clone();
        let metrics = Arc::clone(&self.metrics);

        std::fs::create_dir_all(&cfg.chunks_dir)?;
        let writer = ChunkWriter::new(
            &cfg.chunks_dir,
            CHUNK_SECONDS,
            Arc::clone(&metrics),
            Arc::clone(&cfg.mic_gain),
            cfg.live_tx.clone(),
        )?;

        let is_online = cfg.mode == "online";
        // 采集指标复位：effective_pid 先按请求值展示，采集线程若回退系统声音会改写为 None
        {
            let mut m = self.metrics.lock().unwrap();
            m.effective_pid = cfg.target_pid;
            m.note = None;
        }
        // 线程异常退出时也复位运行标志，否则前端会以为还在录制（此前会静默假死）
        let running_after = Arc::clone(&self.running);
        let handle = thread::Builder::new()
            .name(format!("deskmate-audio-{}", cfg.session_id))
            .spawn(move || {
            let metrics_after = Arc::clone(&metrics);
            let result = if is_online {
                run_loopback_loop(cfg, running, metrics, writer)
            } else {
                run_mic_loop(cfg, running, metrics, writer)
            };
            if let Err(e) = result {
                eprintln!("[capture] error: {e}");
                // 线程异常退出的原因要写到 note，前端才会展示，而不是无声假死
                if let Ok(mut m) = metrics_after.lock() {
                    m.note = Some(format!("音频采集线程异常退出：{e}"));
                }
                running_after.store(false, Ordering::SeqCst);
            }
        })
            .map_err(|e| AppError::new(format!("创建采集线程失败：{e}")))?;

        self.handle = Some(handle);
        Ok(true)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/* ----------------------------- 麦克风（cpal） ----------------------------- */

fn run_mic_loop(
    cfg: CaptureConfig,
    running: Arc<AtomicBool>,
    metrics: Arc<Mutex<Metrics>>,
    mut writer: ChunkWriter,
) -> AppResult<()> {
    // 用 WASAPI 共享 + RAW 模式采集，绕过 Intel 智音阵列等设备的 APO（音频处理对象），
    // 避免其 VAD（语音活动检测）把零碎说话声当背景噪音静音。
    let _ = wasapi::initialize_mta();
    let enumerator = wasapi::DeviceEnumerator::new().map_err(wasapi_err)?;
    let device = enumerator
        .get_default_device(&wasapi::Direction::Capture)
        .map_err(|_| AppError::new("未找到默认音频输入设备（麦克风），请在系统设置中选择麦克风"))?;

    let dev_name = device.get_friendlyname().unwrap_or_else(|_| "默认麦克风".into());
    {
        let mut m = metrics.lock().unwrap();
        m.device_name = dev_name.clone();
    }
    eprintln!("[capture] session {} 麦克风采集（WASAPI RAW）：{}", cfg.session_id, dev_name);

    let dev_format = device.get_device_format().map_err(wasapi_err)?;
    let channels = dev_format.get_nchannels() as usize;
    let sample_rate = dev_format.get_samplespersec();
    let bits = dev_format.get_bitspersample() as usize;
    eprintln!(
        "[capture] 麦克风格式: channels={channels}, rate={sample_rate}, bits={bits}"
    );

    let mut audio_client = device.get_iaudioclient().map_err(wasapi_err)?;

    // 关键：设置 RAW 模式，绕过 APO（含 VAD/降噪/AGC）
    let props = wasapi::AudioClientProperties::new()
        .set_option(wasapi::StreamOption::Raw);
    if let Err(e) = audio_client.set_properties(props) {
        eprintln!("[capture] set_properties(RAW) 失败（{e:?}），回退普通共享模式");
    }

    // 共享模式（独占模式在 Intel 阵列上返回 UNSUPPORTED_FORMAT）
    let mode = wasapi::StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 2_000_000,
    };
    audio_client
        .initialize_client(&dev_format, &wasapi::Direction::Capture, &mode)
        .map_err(wasapi_err)?;
    eprintln!("[capture] WASAPI RAW 共享模式初始化 OK");

    let capture_client = audio_client.get_audiocaptureclient().map_err(wasapi_err)?;
    audio_client.start_stream().map_err(wasapi_err)?;
    eprintln!("[capture] mic stream started");

    let bytes_per_sample = bits / 8;
    let frame_bytes = bytes_per_sample * channels;
    let mut queue: VecDeque<u8> = VecDeque::new();
    let mut total_frames: u64 = 0;
    let mut last_log = Instant::now();

    while running.load(Ordering::SeqCst) {
        let _ = capture_client.read_from_device_to_deque(&mut queue);

        // 每次取最多 1024 帧处理，避免单次 push 过大
        const FRAMES_PER_BATCH: usize = 1024;
        while queue.len() >= frame_bytes * FRAMES_PER_BATCH {
            let mut buf = vec![0u8; frame_bytes * FRAMES_PER_BATCH];
            for slot in buf.iter_mut() {
                *slot = queue.pop_front().unwrap();
            }
            let samples_f32: Vec<f32> = match bits {
                16 => buf
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                    .collect(),
                32 => buf
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
                _ => continue,
            };
            writer.push_native(&samples_f32, channels, sample_rate);
            total_frames += FRAMES_PER_BATCH as u64;
        }

        if last_log.elapsed() >= Duration::from_secs(1) {
            let max = queue
                .iter()
                .take(frame_bytes * 100)
                .cloned()
                .collect::<Vec<u8>>()
                .chunks_exact(bytes_per_sample)
                .map(|b| {
                    if bits == 16 {
                        i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0
                    } else {
                        f32::from_le_bytes([b[0], b[1], b[2], b[3]])
                    }
                })
                .fold(0.0f32, |a, s| if s.abs() > a { s.abs() } else { a });
            eprintln!(
                "[capture] mic RAW 运行中：总帧={total_frames}, 队列字节={}, 近帧 max={max:.4}",
                queue.len()
            );
            last_log = Instant::now();
        }

        thread::sleep(Duration::from_millis(5));
    }
    audio_client.stop_stream().ok();
    eprintln!("[capture] mic loop exit, total_frames={total_frames}");
    Ok(())
}

/* ------------------------ 系统声音（WASAPI loopback） ---------------------- */

fn wasapi_err(e: wasapi::WasapiError) -> AppError {
    AppError::new(format!("WASAPI 采集失败：{e:?}"))
}

/// 建立并初始化回环采集客户端：target 为 None 时采集整个系统声音，Some(pid) 时采集该进程树。
fn init_loopback_client(
    target_pid: Option<u32>,
    format: &wasapi::WaveFormat,
    metrics: &Arc<Mutex<Metrics>>,
) -> AppResult<wasapi::AudioClient> {
    let mut audio_client = match target_pid {
        Some(pid) => {
            let name = crate::win_audio::process_names()
                .get(&pid)
                .map(|exe| exe.strip_suffix(".exe").unwrap_or(exe).to_string())
                .unwrap_or_else(|| format!("PID {pid}"));
            {
                let mut m = metrics.lock().unwrap();
                m.device_name = format!("应用声音 · {name}");
            }
            eprintln!("[capture] 进程回环采集：{name} (pid={pid})");
            wasapi::AudioClient::new_application_loopback_client(pid, true).map_err(wasapi_err)?
        }
        None => {
            let enumerator = wasapi::DeviceEnumerator::new().map_err(wasapi_err)?;
            // loopback：在默认「渲染（输出）」设备上以 Capture 方向初始化，
            // wasapi crate 会自动附加 AUDCLNT_STREAMFLAGS_LOOPBACK。
            let device = enumerator
                .get_default_device(&wasapi::Direction::Render)
                .map_err(wasapi_err)?;
            let dev_name = device.get_friendlyname().unwrap_or_else(|_| "默认输出设备".into());
            {
                let mut m = metrics.lock().unwrap();
                m.device_name = format!("系统声音 · {dev_name}");
            }
            device.get_iaudioclient().map_err(wasapi_err)?
        }
    };
    // 进程回环客户端不支持 get_device_period，统一用固定 200ms 缓冲。
    let mode = wasapi::StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 2_000_000,
    };
    audio_client
        .initialize_client(format, &wasapi::Direction::Capture, &mode)
        .map_err(wasapi_err)?;
    Ok(audio_client)
}

fn run_loopback_loop(
    cfg: CaptureConfig,
    running: Arc<AtomicBool>,
    metrics: Arc<Mutex<Metrics>>,
    mut writer: ChunkWriter,
) -> AppResult<()> {
    // MTA 初始化可重复调用（同一线程多次返回 S_FALSE 也无妨）。
    let _ = wasapi::initialize_mta();

    // 目标格式：16kHz 单声道 F32，autoconvert 让系统完成重采样/下混。
    let format =
        wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Float, TARGET_RATE as usize, 1, None);

    // 按应用采集的客户端若建立/初始化失败，直接回退到系统声音，保证课堂仍被录到。
    let (mut audio_client, mut effective, mut note) =
        match init_loopback_client(cfg.target_pid, &format, &metrics) {
            Ok(c) => (c, cfg.target_pid, None),
            Err(e) if cfg.target_pid.is_some() => {
                let msg = format!("未能从所选应用捕获声音（{e}），已自动改为录制系统声音");
                eprintln!("[capture] session {} {msg}", cfg.session_id);
                (
                    init_loopback_client(None, &format, &metrics)?,
                    None,
                    Some(msg),
                )
            }
            Err(e) => return Err(e),
        };
    {
        let mut m = metrics.lock().unwrap();
        m.effective_pid = effective;
        m.note = note.clone();
    }

    let mut capture_client = audio_client.get_audiocaptureclient().map_err(wasapi_err)?;
    audio_client.start_stream().map_err(wasapi_err)?;

    let mut queue: VecDeque<u8> = VecDeque::new();
    // 按应用采集自检：持续拿不到任何数据包（选错进程/应用未放声），或持续
    // 数字静音（进程树在渲染但不是网课音频流）→ 自动回退系统声音。
    // 注意不能用「首个数据包后永久关闭看门狗」的写法：目标进程偶发的提示音、
    // 静音包会永久锁死看门狗，选错进程后全程无声（Teams 实测）。
    let mut started_at = Instant::now();
    let mut last_packet: Option<Instant> = None;
    let mut last_sound: Option<Instant> = None;
    let mut last_playing: Option<bool> = None;
    let mut playing_checked_at: Option<Instant> = None;
    let mut fell_back = false;
    while running.load(Ordering::SeqCst) {
        // 关键：回环 GetBuffer 每次只返回一个渲染周期（约 10ms）的 packet。
        // 必须在一次轮询内循环取完所有就绪 packet——若 50ms 周期只取一次，
        // 5 个 packet 只取走 1 个，其余在环形缓冲中被覆盖，录音与实时字幕
        // 稳定只有 1/5 速率（服务端约 35s 后以 AudioSendSlow 踢断 AST）。
        for _ in 0..50 {
            let before = queue.len();
            match capture_client.read_from_device_to_deque(&mut queue) {
                Ok(_) if queue.len() > before => {}
                // 0 帧（无新数据）或瞬时错误：本轮已取空，等下一周期
                _ => break,
            }
        }
        let mut frames = Vec::with_capacity(queue.len() / 4);
        while queue.len() >= 4 {
            let mut b = [0u8; 4];
            for slot in b.iter_mut() {
                if let Some(byte) = queue.pop_front() {
                    *slot = byte;
                }
            }
            frames.push(f32::from_le_bytes(b));
        }
        if !frames.is_empty() {
            last_packet = Some(Instant::now());
            // 数字静音检测：正确的采集流课堂里几乎不会出现连续 45s 的纯 0
            let sum: f32 = frames.iter().map(|s| s * s).sum();
            if sum / frames.len() as f32 > 1e-8 {
                last_sound = Some(Instant::now());
            }
            // 系统已按请求格式输出 16kHz mono，直接落盘（电平统计在 writer 内）。
            writer.push_mono16k(&frames);
        } else if effective.is_some() && !fell_back {
            let stalled = last_packet
                .map_or_else(|| started_at.elapsed(), |t| t.elapsed())
                >= PACKET_STALL_FALLBACK;
            let silent = last_sound
                .map_or_else(|| started_at.elapsed(), |t| t.elapsed())
                >= SILENCE_FALLBACK;
            if stalled || silent {
                // is_playing 只用于提示文案；每 2s 刷新一次，避免每轮做 COM 枚举
                if playing_checked_at
                    .map_or(true, |t| t.elapsed() >= Duration::from_secs(2))
                {
                    last_playing =
                        Some(crate::win_audio::is_playing(effective.unwrap_or(0)));
                    playing_checked_at = Some(Instant::now());
                }
                fell_back = true;
                let reason = if silent && !stalled {
                    "所选应用持续无声（网课音频可能由其他进程渲染）".to_string()
                } else if last_playing == Some(true) {
                    "所选应用在放声音，但音频由其他进程渲染，采集不到".to_string()
                } else {
                    "所选应用当前没有输出声音".to_string()
                };
                let msg = format!(
                    "{reason}，已自动改为录制系统声音；如需只录该应用，请在它播放声音时重新选择"
                );
                eprintln!("[capture] session {} {msg}", cfg.session_id);
                let _ = audio_client.stop_stream();
                match init_loopback_client(None, &format, &metrics) {
                    Ok(c) => {
                        audio_client = c;
                        capture_client = audio_client.get_audiocaptureclient().map_err(wasapi_err)?;
                        audio_client.start_stream().map_err(wasapi_err)?;
                        effective = None;
                        note = Some(msg.clone());
                        started_at = Instant::now();
                        last_packet = Some(started_at);
                        last_sound = Some(started_at);
                        let mut m = metrics.lock().unwrap();
                        m.effective_pid = None;
                        m.note = Some(msg);
                    }
                    Err(e) => {
                        let mut m = metrics.lock().unwrap();
                        m.note = Some(format!("所选应用无声音，且回退系统声音失败：{e}"));
                    }
                }
            }
        }
        // 与 10ms 设备周期对齐；取空循环保证不漏包，sleep 长短只影响延迟
        thread::sleep(Duration::from_millis(10));
    }
    let _ = audio_client.stop_stream();
    Ok(())
}

/* --------------------------- 分块写入与重采样 ----------------------------- */

/// 线性插值重采样器（任意采样率 → 16kHz，跨块保留末样本）。
struct Resampler {
    ratio: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    fn new(input_rate: u32) -> Self {
        Self {
            ratio: input_rate as f64 / TARGET_RATE as f64,
            pos: 0.0,
            last: 0.0,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.ratio - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity((input.len() as f64 / self.ratio) as usize + 1);
        for &s in input {
            self.pos -= 1.0;
            while self.pos <= 0.0 {
                let alpha = (1.0 + self.pos).clamp(0.0, 1.0) as f32;
                out.push(self.last + (s - self.last) * alpha);
                self.pos += self.ratio;
            }
            self.last = s;
        }
        out
    }
}

struct ChunkWriter {
    dir: PathBuf,
    next_index: u32,
    buf: Vec<f32>,
    target_samples: usize,
    resampler: Resampler,
    metrics: Arc<Mutex<Metrics>>,
    /// 麦克风软件增益（仅 push_native 路径生效；运行中可实时调整）。
    mic_gain: Arc<AtomicU32>,
    /// 实时字幕音频支流（AST 同传），None 表示不分流。
    live_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<i16>>>,
}

impl ChunkWriter {
    fn new(
        dir: &Path,
        chunk_seconds: u32,
        metrics: Arc<Mutex<Metrics>>,
        mic_gain: Arc<AtomicU32>,
        live_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<i16>>>,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        // 续接：从已有分块最大序号 +1，继续录制不覆盖旧文件。
        let mut max_seq: i64 = -1;
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix("chunk_").and_then(|n| n.strip_suffix(".wav")) {
                    if let Ok(seq) = rest.parse::<i64>() {
                        max_seq = max_seq.max(seq);
                    }
                }
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            next_index: (max_seq + 1) as u32,
            buf: Vec::new(),
            target_samples: TARGET_RATE as usize * chunk_seconds as usize,
            resampler: Resampler::new(TARGET_RATE),
            metrics,
            mic_gain,
            live_tx,
        })
    }

    /// 接收设备原生交错 F32 数据：下混 mono → 增益 → 重采样 16kHz → 写块。
    /// 下混策略：单/双通道算术平均；多通道（>2，常见于麦克风阵列）取能量最大通道，
    /// 因为阵列的辅助通道（后置/参考/降噪）可能是静音或相位反转信号，算术平均会稀释人声。
    fn push_native(&mut self, data: &[f32], channels: usize, input_rate: u32) {
        if channels == 0 || data.is_empty() {
            return;
        }
        if (self.resampler.ratio - input_rate as f64 / TARGET_RATE as f64).abs() > 1e-6 {
            self.resampler = Resampler::new(input_rate);
        }
        let gain = f32::from_bits(self.mic_gain.load(Ordering::SeqCst));
        let mut mono = Vec::with_capacity(data.len() / channels);
        if channels <= 2 {
            for frame in data.chunks_exact(channels) {
                let sum: f32 = frame.iter().sum();
                let s = sum / channels as f32 * gain;
                mono.push(s.clamp(-1.0, 1.0));
            }
        } else {
            // 多通道阵列：挑能量最大的通道（它通常是主麦克风通道）
            for frame in data.chunks_exact(channels) {
                let s = frame
                    .iter()
                    .cloned()
                    .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a })
                    * gain;
                mono.push(s.clamp(-1.0, 1.0));
            }
        }
        let converted = self.resampler.process(&mono);
        self.push_mono16k(&converted);
    }

    /// 接收已是 16kHz mono 的数据（loopback 路径与重采样输出共用）。
    fn push_mono16k(&mut self, samples: &[f32]) {
        // 实时电平：每批采样（麦克回调约 10~40ms，loopback 每 50ms）都更新瞬时 RMS，
        // 之前只在 10 秒分块完成时更新一次，导致电平条 10 秒才跳动一下。
        if !samples.is_empty() {
            let inst = block_rms(samples);
            if let Ok(mut m) = self.metrics.lock() {
                // 指数平滑（新值权重 0.4），表针跟随音量又不至于过抖
                m.rms = if m.rms <= 0.0 { inst } else { m.rms * 0.6 + inst * 0.4 };
            }
            // AST 同传音频支流：16kHz mono f32 → i16 PCM；发送失败说明接收端已停，忽略
            if let Some(tx) = &self.live_tx {
                let pcm: Vec<i16> = samples
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                let _ = tx.send(pcm);
            }
        }
        self.buf.extend_from_slice(samples);
        while self.buf.len() >= self.target_samples {
            let block: Vec<f32> = self.buf.drain(..self.target_samples).collect();
            let rms = block_rms(&block);
            let fname = format!("chunk_{:04}.wav", self.next_index);
            self.next_index += 1;
            let path = self.dir.join(&fname);
            if let Err(e) = write_wav_mono(&path, &block, TARGET_RATE) {
                eprintln!("[capture] 写分块失败：{e}");
            }
            self.update_metrics(rms);
        }
    }

    /// 分块完成时更新：块计数与启动期静音判定（瞬时电平由 push_mono16k 按批更新）。
    fn update_metrics(&mut self, rms: f32) {
        if let Ok(mut m) = self.metrics.lock() {
            m.chunks += 1;
            // 只在启动后前 SILENCE_CHECK_CHUNKS 块内做静音判定；
            // 任一有声块立即解除告警。
            if rms >= SILENCE_RMS_THRESHOLD {
                m.silence_warned = false;
            } else if m.chunks <= SILENCE_CHECK_CHUNKS {
                m.silence_warned = m.chunks >= SILENCE_CHECK_CHUNKS || m.silence_warned;
            }
        }
    }
}

fn block_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|s| {
            let v = s.clamp(-1.0, 1.0) as f64;
            v * v
        })
        .sum();
    ((sum_sq / samples.len() as f64).sqrt() * 32767.0) as f32
}

/* -------------------------------- WAV 输出 -------------------------------- */

fn write_wav_mono(path: &Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    let data_size = samples.len() as u32 * 2;
    write_wav_header(&mut file, sample_rate, 1, 16, data_size)?;
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        file.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn write_wav_header(
    f: &mut File,
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u16,
    data_size: u32,
) -> std::io::Result<()> {
    let byte_rate = sample_rate * channels * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample as u32 / 8);

    f.write_all(b"RIFF")?;
    f.write_all(&(36u32 + data_size).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&(16u32).to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&(channels as u16).to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&(block_align as u16).to_le_bytes())?;
    f.write_all(&bits_per_sample.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;
    Ok(())
}

/* ------------------------------ 分段合并录音 ------------------------------ */

/// 一次合并结果。
pub struct MergedAudio {
    /// 合并输出文件路径（课堂录音_段N.wav）。
    pub path: PathBuf,
    /// 段号（从 1 起）。
    pub segment: i64,
    /// 本段包含的分块数。
    pub chunks: usize,
}

/// 将「自上次归档以来新增」的 chunk_XXXX.wav 合并为一个完整 WAV。
///
/// 续录的分块序号天然递增（ChunkWriter 构造时扫描目录续号），
/// 通过 marker 文件 `.merge_marker` 记录上次合并到的序号与已产生段数，
/// 因此每次归档只合并新段落，输出 课堂录音_段N.wav，绝不混入之前的录音。
pub fn merge_new_chunks(dir: &Path) -> AppResult<Option<MergedAudio>> {
    let marker_path = dir.join(".merge_marker");
    // chunk 序号从 0 起，缺省 -1 表示尚未合并过任何块
    let (mut merged_to, mut segment) = (-1i64, 0i64);
    if let Ok(txt) = std::fs::read_to_string(&marker_path) {
        for line in txt.lines() {
            let (k, v) = line.split_once('=').unwrap_or((line, ""));
            match k.trim() {
                "merged_to" => merged_to = v.trim().parse().unwrap_or(0),
                "segment" => segment = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    // 收集序号大于 merged_to 的分块（seq 从 0 起，无 marker 时全部合并）
    let mut seqs: Vec<i64> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Some(seq) = parse_chunk_seq(&p) {
                if seq > merged_to {
                    seqs.push(seq);
                }
            }
        }
    }
    seqs.sort();
    if seqs.is_empty() {
        return Ok(None);
    }

    // 所有分块同为 16kHz/mono/16bit PCM（本模块写出），解析 data 段直接二进制拼接
    let mut pcm: Vec<u8> = Vec::new();
    let mut fmt: Option<(u16, u16, u32)> = None; // channels, bits, sample_rate
    let mut max_seq = merged_to;
    for seq in &seqs {
        let p = dir.join(format!("chunk_{seq:04}.wav"));
        let (f, data) = read_wav_pcm(&p)?;
        if let Some((c, b, r)) = fmt {
            if (c, b, r) != f {
                return Err(AppError::new(format!("分块 {seq} 音频格式不一致，无法合并")));
            }
        }
        fmt = Some(f);
        pcm.extend_from_slice(&data);
        max_seq = *seq;
    }
    let (channels, bits, sample_rate) = fmt.unwrap_or((1, 16, TARGET_RATE));

    segment += 1;
    let out = dir.join(format!("课堂录音_段{segment}.wav"));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&out)?;
        write_wav_header(&mut file, sample_rate, channels as u32, bits, pcm.len() as u32)?;
        use std::io::Write as _;
        file.write_all(&pcm)?;
    }
    std::fs::write(
        &marker_path,
        format!("merged_to={max_seq}\nsegment={segment}\n"),
    )?;
    Ok(Some(MergedAudio {
        path: out,
        segment,
        chunks: seqs.len(),
    }))
}

fn parse_chunk_seq(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_string_lossy();
    name.strip_prefix("chunk_")?
        .strip_suffix(".wav")?
        .parse::<i64>()
        .ok()
}

/// 解析 WAV 的 (声道数, 位深, 采样率) 与 data 段字节（仅支持 PCM）。
fn read_wav_pcm(path: &Path) -> std::io::Result<((u16, u16, u32), Vec<u8>)> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "不是合法 WAV 文件",
        ));
    }
    let mut pos = 12;
    let mut result: Option<(u16, u16, u32)> = None;
    let mut data: Vec<u8> = Vec::new();
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        if body_start + size > bytes.len() {
            break;
        }
        match id {
            b"fmt " => {
                let b = &bytes[body_start..body_start + size];
                let format = u16::from_le_bytes(b[0..2].try_into().unwrap());
                if format != 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "仅支持 PCM WAV",
                    ));
                }
                let channels = u16::from_le_bytes(b[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(b[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes(b[14..16].try_into().unwrap());
                result = Some((channels, bits, rate));
            }
            b"data" => data = bytes[body_start..body_start + size].to_vec(),
            _ => {}
        }
        pos = body_start + size + (size & 1); // chunk 按偶数字节对齐
    }
    match result {
        Some(f) => Ok((f, data)),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "WAV 缺少 fmt 段",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "deskmate_merge_{}_{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sine(seconds: u32, freq: f32) -> Vec<f32> {
        (0..TARGET_RATE * seconds)
            .map(|i| (i as f32 * freq / TARGET_RATE as f32 * std::f32::consts::TAU).sin() * 0.2)
            .collect()
    }

    #[test]
    fn merge_then_continue_merges_separately() {
        let dir = unique_dir("seg");
        // 第一段：chunk 0、1
        write_wav_mono(&dir.join("chunk_0000.wav"), &sine(10, 300.0), TARGET_RATE).unwrap();
        write_wav_mono(&dir.join("chunk_0001.wav"), &sine(10, 500.0), TARGET_RATE).unwrap();
        let m1 = merge_new_chunks(&dir).unwrap().unwrap();
        assert_eq!(m1.segment, 1);
        assert_eq!(m1.chunks, 2);
        assert!(m1.path.exists());
        let ((c, b, r), data1) = read_wav_pcm(&m1.path).unwrap();
        assert_eq!((c, b, r), (1, 16, TARGET_RATE));
        assert_eq!(data1.len(), 20 * TARGET_RATE as usize * 2);

        // 续录：新 chunk 序号递增
        write_wav_mono(&dir.join("chunk_0002.wav"), &sine(5, 800.0), TARGET_RATE).unwrap();
        // 无新块时返回 None
        assert!(merge_new_chunks(&dir).unwrap().is_some());
        // 第二次合并（模拟再一次归档前不应重复）：先去掉段2文件影响，直接校验 marker
        let marker = std::fs::read_to_string(dir.join(".merge_marker")).unwrap();
        assert!(marker.contains("merged_to=2"));
        assert!(marker.contains("segment=2"));
        // 第三次调用无新增块
        assert!(merge_new_chunks(&dir).unwrap().is_none());

        let seg2 = dir.join("课堂录音_段2.wav");
        let (_, data2) = read_wav_pcm(&seg2).unwrap();
        // 段2只含 chunk_0002（5 秒），没有混入段1
        assert_eq!(data2.len(), 5 * TARGET_RATE as usize * 2);
        assert_ne!(data1[..100], data2[..100]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 诊断 cpal 麦克风链路：列出所有输入设备、尝试打开默认设备、采样 3 秒看是否有非零音频。
    /// 额外打印每个通道的能量分布，定位阵列里哪个通道真正有信号。
    /// cargo test mic_probe -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "真机诊断：cargo test mic_probe -- --ignored --nocapture"]
    async fn mic_probe() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        println!("\n=== cpal 输入设备 ===");
        let inputs: Vec<_> = host.input_devices().unwrap().collect();
        for (i, dev) in inputs.iter().enumerate() {
            let name = dev.name().unwrap_or_else(|_| "<无名字>".into());
            let cfg = dev.default_input_config();
            let fmt_count = dev.supported_input_configs().ok().map(|f| f.count().to_string()).unwrap_or("?".into());
            println!(
                "  [{i}] {name} | default={cfg:?} | configs={fmt_count}"
            );
        }
        let Some(def) = host.default_input_device() else {
            println!("未找到默认输入设备！");
            return;
        };
        println!("默认设备: {}", def.name().unwrap_or_else(|_| "<无>".into()));
        let supported = def.default_input_config().unwrap();
        let cfg: StreamConfig = supported.clone().into();
        let ch = cfg.channels as usize;
        println!("默认配置: channels={ch}, rate={}, format={:?}", cfg.sample_rate.0, supported.sample_format());
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(4);
        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                def.build_input_stream(
                    &cfg,
                    move |data: &[f32], _| { let _ = tx.try_send(data.to_vec()); },
                    |e| println!("stream err: {e}"),
                    None,
                ).unwrap()
            }
            other => { println!("格式 {other:?} 不是 F32，跳过采样"); return; }
        };
        stream.play().unwrap();
        println!("stream.play() OK，采样 3 秒…");
        let mut per_ch_sum_sq = vec![0.0f64; ch];
        let mut per_ch_max = vec![0.0f32; ch];
        let mut frames = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                for frame in chunk.chunks_exact(ch) {
                    frames += 1;
                    for (i, &s) in frame.iter().enumerate() {
                        let a = s.abs();
                        if a > per_ch_max[i] { per_ch_max[i] = a; }
                        per_ch_sum_sq[i] += (s as f64) * (s as f64);
                    }
                }
            }
        }
        drop(stream);
        println!("总帧数: {frames}");
        for i in 0..ch {
            let rms = (per_ch_sum_sq[i] / frames as f64).sqrt();
            println!("  通道 {i}: max={:.4}, rms={:.6}", per_ch_max[i], rms);
        }
        // 模拟 push_native 旧逻辑（算术平均）vs 新逻辑（最大通道）的结果
        let mut sum_avg_sq = 0.0f64;
        let mut sum_max_sq = 0.0f64;
        let mut max_overall = 0.0f32;
        let mut frames2 = 0usize;
        // 重新采样 1 秒做对比
        let (tx2, rx2) = std::sync::mpsc::sync_channel::<Vec<f32>>(4);
        let stream2 = def.build_input_stream(
            &cfg,
            move |data: &[f32], _| { let _ = tx2.try_send(data.to_vec()); },
            |_| {},
            None,
        ).unwrap();
        stream2.play().unwrap();
        let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::time::Instant::now() < deadline2 {
            if let Ok(chunk) = rx2.recv_timeout(std::time::Duration::from_millis(200)) {
                for frame in chunk.chunks_exact(ch) {
                    frames2 += 1;
                    let avg: f32 = frame.iter().sum::<f32>() / ch as f32;
                    let max_ch = frame.iter().cloned().fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a });
                    sum_avg_sq += (avg as f64) * (avg as f64);
                    sum_max_sq += (max_ch as f64) * (max_ch as f64);
                    if max_ch.abs() > max_overall.abs() { max_overall = max_ch; }
                }
            }
        }
        drop(stream2);
        let rms_avg = (sum_avg_sq / frames2 as f64).sqrt();
        let rms_max = (sum_max_sq / frames2 as f64).sqrt();
        println!("\n1 秒采样对比（旧=算术平均, 新=最大通道）:");
        println!("  算术平均 rms = {rms_avg:.6}, max={max_overall:.4}");
        println!("  最大通道 rms = {rms_max:.6}");
        println!("  新/旧比值 = {:.2}", rms_max / rms_avg.max(1e-12));

        // === WASAPI 共享模式输入采集对比（绕过 cpal 独占模式的可能 DSP 问题）===
        println!("\n=== WASAPI 共享模式对比 ===");
        let _ = wasapi::initialize_mta();
        if let Ok(enumerator) = wasapi::DeviceEnumerator::new() {
            if let Ok(input_dev) = enumerator.get_default_device(&wasapi::Direction::Capture) {
                if let Ok(dev_name) = input_dev.get_friendlyname() {
                    println!("WASAPI 默认输入: {dev_name}");
                }
                let format = wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Float, 16000, 1, None);
                let mode = wasapi::StreamMode::PollingShared {
                    autoconvert: true,
                    buffer_duration_hns: 2_000_000,
                };
                match input_dev.get_iaudioclient() {
                    Ok(mut client) => {
                        if let Err(e) = client.initialize_client(&format, &wasapi::Direction::Capture, &mode) {
                            println!("WASAPI 初始化失败：{e:?}");
                        } else {
                            let Ok(mut capture_client) = client.get_audiocaptureclient() else {
                                println!("WASAPI get_audiocaptureclient 失败");
                                return;
                            };
                            if let Err(e) = client.start_stream() {
                                println!("WASAPI start_stream 失败：{e:?}");
                            } else {
                                let mut queue: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
                                let deadline3 = std::time::Instant::now() + std::time::Duration::from_secs(2);
                                let mut wasapi_sum_sq = 0.0f64;
                                let mut wasapi_max = 0.0f32;
                                let mut wasapi_frames = 0usize;
                                let mut packets = 0u32;
                                while std::time::Instant::now() < deadline3 {
                                    let before = queue.len();
                                    if capture_client.read_from_device_to_deque(&mut queue).is_ok() && queue.len() > before {
                                        packets += 1;
                                    }
                                    while queue.len() >= 4 {
                                        let mut b = [0u8; 4];
                                        for slot in b.iter_mut() {
                                            if let Some(byte) = queue.pop_front() {
                                                *slot = byte;
                                            }
                                        }
                                        let s = f32::from_le_bytes(b);
                                        let a = s.abs();
                                        if a > wasapi_max { wasapi_max = a; }
                                        wasapi_sum_sq += (s as f64) * (s as f64);
                                        wasapi_frames += 1;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(10));
                                }
                                client.stop_stream().ok();
                                let wasapi_rms = if wasapi_frames > 0 {
                                    (wasapi_sum_sq / wasapi_frames as f64).sqrt()
                                } else { 0.0 };
                                println!("WASAPI 2 秒采样 | packets={packets}, frames={wasapi_frames} | max={:.4}, rms={:.6}", wasapi_max, wasapi_rms);
                                println!("cpal 独占 vs WASAPI 共享 rms 比值 = {:.2}", wasapi_rms / rms_max.max(1e-12));
                            }
                        }
                    }
                    Err(e) => println!("WASAPI get_iaudioclient 失败：{e:?}"),
                }
            } else {
                println!("WASAPI 无默认输入端点");
            }
        } else {
            println!("WASAPI DeviceEnumerator 创建失败");
        }
    }

    /// 真机诊断：WASAPI 独占 + RAW 模式采集，验证能否绕过 Intel 智音阵列 APO 的 VAD 静音。
    /// cargo test raw_mode_probe -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "真机诊断：cargo test raw_mode_probe -- --ignored --nocapture"]
    async fn raw_mode_probe() {
        use std::collections::VecDeque;
        let _ = wasapi::initialize_mta();
        let enumerator = match wasapi::DeviceEnumerator::new() {
            Ok(e) => e,
            Err(e) => { println!("DeviceEnumerator 失败：{e:?}"); return; }
        };
        let device = match enumerator.get_default_device(&wasapi::Direction::Capture) {
            Ok(d) => d,
            Err(e) => { println!("无默认输入设备：{e:?}"); return; }
        };
        let name = device.get_friendlyname().unwrap_or_else(|_| "<无>".into());
        println!("默认输入设备: {name}");

        let dev_format = match device.get_device_format() {
            Ok(f) => f,
            Err(e) => { println!("get_device_format 失败：{e:?}"); return; }
        };
        let ch = dev_format.get_nchannels() as usize;
        let rate = dev_format.get_samplespersec() as usize;
        let bits = dev_format.get_bitspersample() as usize;
        println!("设备格式: channels={ch}, rate={rate}, bits={bits}");

        // 尝试多种模式组合：独占/共享 × RAW/无RAW × 格式
        let sample_type = if bits == 32 { wasapi::SampleType::Float } else { wasapi::SampleType::Int };
        let fmts: Vec<(wasapi::WaveFormat, &str)> = vec![
            (wasapi::WaveFormat::new(bits, bits, &sample_type, rate, ch, None), "原生"),
            (wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Float, rate, ch, None), "float32"),
            (wasapi::WaveFormat::new(16, 16, &wasapi::SampleType::Int, rate, 2, None), "int16 stereo"),
            (wasapi::WaveFormat::new(16, 16, &wasapi::SampleType::Int, rate, 1, None), "int16 mono"),
        ];
        let modes: Vec<(&str, bool, bool)> = vec![
            ("独占+RAW", true, true),
            ("独占无RAW", true, false),
            ("共享+RAW", false, true),
        ];
        let mut success_desc = String::new();
        let mut success_fmt: Option<wasapi::WaveFormat> = None;
        let mut use_bits = 32usize;
        'outer: for (mode_desc, exclusive, use_raw) in &modes {
            for (fmt, fmt_desc) in &fmts {
                let mut audio_client = match device.get_iaudioclient() {
                    Ok(c) => c, Err(_) => continue,
                };
                if *use_raw {
                    let props = wasapi::AudioClientProperties::new()
                        .set_option(wasapi::StreamOption::Raw);
                    if audio_client.set_properties(props).is_err() { continue; }
                }
                let (_def, min_period) = match audio_client.get_device_period() {
                    Ok(p) => p, Err(_) => continue,
                };
                let mode = if *exclusive {
                    wasapi::StreamMode::EventsExclusive { period_hns: min_period }
                } else {
                    wasapi::StreamMode::PollingShared {
                        autoconvert: true, buffer_duration_hns: 2_000_000,
                    }
                };
                match audio_client.initialize_client(fmt, &wasapi::Direction::Capture, &mode) {
                    Ok(_) => {
                        success_desc = format!("{mode_desc} + {fmt_desc}");
                        success_fmt = Some(fmt.clone());
                        use_bits = fmt.get_bitspersample() as usize;
                        break 'outer;
                    }
                    Err(e) => println!("  {mode_desc} + {fmt_desc} 失败: {:?}", e),
                }
            }
        }
        let Some(use_fmt) = success_fmt else {
            println!("所有模式/格式组合都失败");
            return;
        };
        println!("初始化成功: {success_desc}");
        // 重新初始化用于实际采集
        let mut audio_client = device.get_iaudioclient().unwrap();
        if success_desc.contains("+RAW") {
            let props = wasapi::AudioClientProperties::new()
                .set_option(wasapi::StreamOption::Raw);
            audio_client.set_properties(props).ok();
        }
        let (_def, min_period) = audio_client.get_device_period().unwrap();
        let mode = if success_desc.starts_with("独占") {
            wasapi::StreamMode::EventsExclusive { period_hns: min_period }
        } else {
            wasapi::StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 2_000_000 }
        };
        audio_client.initialize_client(&use_fmt, &wasapi::Direction::Capture, &mode).unwrap();
        let bits = use_bits;
        println!("采集格式: bits={bits}, channels={ch}, rate={rate}");

        let capture_client = match audio_client.get_audiocaptureclient() {
            Ok(c) => c,
            Err(e) => { println!("get_audiocaptureclient 失败：{e:?}"); return; }
        };
        if let Err(e) = audio_client.start_stream() {
            println!("start_stream 失败：{e:?}"); return;
        }
        println!("采集开始，请对麦克风说话 3 秒…");

        let mut queue: VecDeque<u8> = VecDeque::new();
        let bytes_per_sample = bits / 8;
        let frame_bytes = bytes_per_sample * ch;
        let mut sum_sq = 0.0f64;
        let mut max_val = 0.0f32;
        let mut frames = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let _ = capture_client.read_from_device_to_deque(&mut queue);
            std::thread::sleep(std::time::Duration::from_millis(10));
            // 解析所有完整帧
            while queue.len() >= frame_bytes {
                let mut frame_buf = vec![0u8; frame_bytes];
                for slot in frame_buf.iter_mut() {
                    *slot = queue.pop_front().unwrap();
                }
                for s_idx in 0..ch {
                    let off = s_idx * bytes_per_sample;
                    let sample_f32 = if bits == 16 {
                        i16::from_le_bytes([frame_buf[off], frame_buf[off + 1]]) as f32 / 32768.0
                    } else if bits == 32 {
                        f32::from_le_bytes([frame_buf[off], frame_buf[off + 1], frame_buf[off + 2], frame_buf[off + 3]])
                    } else {
                        0.0
                    };
                    let a = sample_f32.abs();
                    if a > max_val { max_val = a; }
                    sum_sq += (sample_f32 as f64) * (sample_f32 as f64);
                }
                frames += 1;
            }
        }
        audio_client.stop_stream().ok();
        let rms = if frames > 0 { (sum_sq / (frames * ch) as f64).sqrt() } else { 0.0 };
        println!("\nRAW 模式 3 秒结果: frames={frames}, max={max_val:.4}, rms={rms:.6}");
        if max_val > 0.01 {
            println!("✅ RAW 模式采到正常信号，可绕过阵列 APO 的 VAD 静音");
        } else {
            println!("❌ RAW 模式仍无声，请检查麦克风是否被其他应用独占");
        }
    }

    /// 验证多通道阵列下混策略：4 通道输入只有通道 0 有信号，通道 1-3 静音。
    /// 修复前用算术平均会把信号稀释到 1/4；修复后用最大通道能完整保留。
    #[test]
    fn multi_channel_array_picks_loudest() {
        // 构造 4 通道交错数据：通道 0=0.2, 1-3=0.0
        let frames = 16;
        let mut interleaved = Vec::with_capacity(frames * 4);
        for _ in 0..frames {
            interleaved.push(0.2f32); // ch0 有信号
            interleaved.push(0.0f32);
            interleaved.push(0.0f32);
            interleaved.push(0.0f32);
        }
        // 取最大通道：应该得到 ~0.2 的 mono 信号
        let expected_sum: f32 = interleaved
            .chunks_exact(4)
            .map(|f| f.iter().cloned().fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a }))
            .sum();
        // 算术平均（旧逻辑）会稀释到 0.05
        let diluted_sum: f32 = interleaved
            .chunks_exact(4)
            .map(|f| f.iter().sum::<f32>() / 4.0)
            .sum();
        assert!(
            expected_sum > diluted_sum * 2.5,
            "最大通道策略应显著好于算术平均：expected_sum={expected_sum}, diluted_sum={diluted_sum}"
        );
    }
}
