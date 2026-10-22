//! AST 2.0 实时同声传译（火山引擎 · 豆包同传，WebSocket 流式，PRD 5.4 升级链路）。
//!
//! - 采集线程经 `live_tx` 支流送来 16kHz mono i16 采样，本模块攒成 80ms PCM 包流式上行；
//! - 协议为纯 protobuf over WebSocket 二进制帧（无自定义帧头），握手头 X-Api-Key；
//! - 651/652（源语字幕）与 653/654/655（译文）按 utterance 累积配对，定稿即落库并
//!   推送 `subtitle://line`；
//! - 154 用量事件按真实 token 写 usage_records（模型 Doubao-AST-2.0）；
//! - 行动项检测复用硅基流动 DeepSeek（未配置 SF Key 时自动跳过）；
//! - 断线退避重连；鉴权失败（11200）判定为致命错误，停止并通知前端。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager as _};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_tungstenite::tungstenite::Message;

use crate::db;
use crate::error::{AppError, AppResult};
use crate::models::ActionItemDto;
use crate::pipeline::{self, UsageInsert};

/* ------------------------------- 协议常量 -------------------------------- */

const WS_URL: &str = "wss://openspeech.bytedance.com/api/v4/ast/v2/translate";
const RESOURCE_ID: &str = "volc.service_type.10053";
/// 用量明细里记录的模型展示名（计费口径见 pipeline::model_price）。
const AST_MODEL_LABEL: &str = "Doubao-AST-2.0";
/// 上行音频包大小：80ms @ 16kHz mono = 1280 采样 = 2560 字节。
const PACKET_SAMPLES: usize = 1280;
/// 待发采样积压上限：5 秒。断连重连后只补发最近 5 秒音频，更早的缺口由归档管线
/// ASR 补写——避免瞬时超速补发触发服务端限流，同时保证实时字幕紧跟当前语音。
const MAX_PENDING: usize = 16000 * 5;
/// 部分字幕推送节流间隔（打字机效果，避免事件风暴）。
const PARTIAL_THROTTLE: Duration = Duration::from_millis(200);
/// 部分字幕事件：未定稿的边说边出字行，定稿后由 subtitle://line 取代。
const EVENT_SUBTITLE_PARTIAL: &str = "subtitle://partial";

/* ------------------------------ 前端事件定义 ------------------------------ */
// 原 live.rs（硅基流动分块转写链路）已移除，AST 为唯一实时字幕来源，事件格式不变。

/// 前端事件：新字幕行。
pub const EVENT_SUBTITLE_LINE: &str = "subtitle://line";
/// 前端事件：新行动项。
pub const EVENT_ACTION_ITEM: &str = "actionitem://new";
/// 前端事件：字幕通道状态提示（如同传失效、连接中断）。
pub const EVENT_SUBTITLE_STATUS: &str = "subtitle://status";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleLineEvent {
    pub session_id: i64,
    pub seq: i64,
    pub ts: String,
    pub en: String,
    pub zh: Option<String>,
    /// AST 原始说话人标识（''=未知）；归档后由 LLM 统一标注 teacher/other。
    pub speaker: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleStatusEvent {
    pub session_id: i64,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItemEvent {
    pub session_id: i64,
    pub item: ActionItemDto,
}

// 事件号（common/events.proto）
const EV_START_SESSION: i32 = 100;
const EV_FINISH_SESSION: i32 = 102;
const EV_TASK_REQUEST: i32 = 200;
const EV_SESSION_STARTED: i32 = 150;
const EV_SESSION_FINISHED: i32 = 152;
const EV_SESSION_FAILED: i32 = 153;
const EV_USAGE_RESPONSE: i32 = 154;
const EV_AUDIO_MUTED: i32 = 250;
const EV_SOURCE_START: i32 = 650;
const EV_SOURCE_RESPONSE: i32 = 651;
const EV_SOURCE_END: i32 = 652;
const EV_TRANSLATION_START: i32 = 653;
const EV_TRANSLATION_RESPONSE: i32 = 654;
const EV_TRANSLATION_END: i32 = 655;

/// 等待 SessionStarted 的超时。
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// Ping 保活周期。
const PING_INTERVAL: Duration = Duration::from_secs(30);
/// 行动项检测周期。周期越长，一次能看到的课堂内容越完整，越不容易把半句话当成任务。
const DETECT_INTERVAL: Duration = Duration::from_secs(20);
/// 检测上下文窗口（条数）。取够大的值，避免快速讲授时旧字幕在下次检测前被丢掉。
const DETECT_CONTEXT_LINES: usize = 24;
/// 连续重连上限（超过则放弃并通知前端）。
const MAX_RECONNECTS: u32 = 5;

/* --------------------------- protobuf 消息定义 --------------------------- */
// 字段号与官方 proto 严格一致（C:\...\ast\protos 下 rpcmeta/au_base/ast_service）。

pub mod pb {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct RequestMeta {
        #[prost(string, tag = "1")]
        pub endpoint: String,
        #[prost(string, tag = "2")]
        pub app_key: String,
        #[prost(string, tag = "3")]
        pub app_id: String,
        #[prost(string, tag = "4")]
        pub resource_id: String,
        #[prost(string, tag = "5")]
        pub connection_id: String,
        #[prost(string, tag = "6")]
        pub session_id: String,
        #[prost(int32, tag = "7")]
        pub sequence: i32,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct BillingItem {
        #[prost(string, tag = "1")]
        pub unit: String,
        #[prost(float, tag = "2")]
        pub quantity: f32,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Billing {
        #[prost(message, repeated, tag = "1")]
        pub items: Vec<BillingItem>,
        #[prost(int64, tag = "2")]
        pub duration_msec: i64,
        #[prost(int64, tag = "3")]
        pub word_count: i64,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ResponseMeta {
        #[prost(string, tag = "1")]
        pub session_id: String,
        #[prost(int32, tag = "2")]
        pub sequence: i32,
        #[prost(int32, tag = "3")]
        pub status_code: i32,
        #[prost(string, tag = "4")]
        pub message: String,
        #[prost(message, optional, tag = "5")]
        pub billing: Option<Billing>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct User {
        #[prost(string, tag = "1")]
        pub uid: String,
        #[prost(string, tag = "2")]
        pub did: String,
        #[prost(string, tag = "3")]
        pub platform: String,
        #[prost(string, tag = "4")]
        pub sdk_version: String,
        #[prost(string, tag = "5")]
        pub app_version: String,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Audio {
        #[prost(string, tag = "1")]
        pub data: String,
        #[prost(string, tag = "2")]
        pub url: String,
        #[prost(string, tag = "3")]
        pub url_type: String,
        #[prost(string, tag = "4")]
        pub format: String,
        #[prost(string, tag = "5")]
        pub codec: String,
        #[prost(string, tag = "6")]
        pub language: String,
        #[prost(int32, tag = "7")]
        pub rate: i32,
        #[prost(int32, tag = "8")]
        pub bits: i32,
        #[prost(int32, tag = "9")]
        pub channel: i32,
        #[prost(string, tag = "10")]
        pub tos_bucket: String,
        #[prost(string, tag = "11")]
        pub tos_access_key: String,
        #[prost(string, tag = "12")]
        pub audio_tos_object: String,
        #[prost(string, tag = "13")]
        pub role_trn: String,
        #[prost(bytes, tag = "14")]
        pub binary_data: Vec<u8>,
    }

    impl Audio {
        /// 16kHz/16bit/mono PCM 配置（每个请求都带上，与官方 demo 行为一致）。
        pub fn pcm16k() -> Self {
            Self {
                format: "wav".into(),
                codec: "raw".into(),
                rate: 16000,
                bits: 16,
                channel: 1,
                ..Default::default()
            }
        }
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ReqParams {
        #[prost(string, tag = "1")]
        pub mode: String,
        #[prost(string, tag = "2")]
        pub source_language: String,
        #[prost(string, tag = "3")]
        pub target_language: String,
        #[prost(string, tag = "4")]
        pub speaker_id: String,
        #[prost(int32, tag = "5")]
        pub speech_rate: i32,
        #[prost(bool, optional, tag = "6")]
        pub enable_source_language_detect: Option<bool>,
        #[prost(bool, tag = "7")]
        pub is_custom_speaker: bool,
        #[prost(string, tag = "8")]
        pub tts_resource_id: String,
        #[prost(string, tag = "80")]
        pub extra: String,
        // corpus(100) 为消息类型，S2T 不需要，proto3 缺省即省略
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct TranslateRequest {
        #[prost(message, optional, tag = "1")]
        pub request_meta: Option<RequestMeta>,
        #[prost(int32, tag = "2")]
        pub event: i32,
        #[prost(message, optional, tag = "3")]
        pub user: Option<User>,
        #[prost(message, optional, tag = "4")]
        pub source_audio: Option<Audio>,
        #[prost(message, optional, tag = "5")]
        pub target_audio: Option<Audio>,
        #[prost(message, optional, tag = "6")]
        pub request: Option<ReqParams>,
        #[prost(bool, optional, tag = "7")]
        pub denoise: Option<bool>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct TranslateResponse {
        #[prost(message, optional, tag = "1")]
        pub response_meta: Option<ResponseMeta>,
        #[prost(int32, tag = "2")]
        pub event: i32,
        #[prost(bytes, tag = "3")]
        pub data: Vec<u8>,
        #[prost(string, tag = "4")]
        pub text: String,
        #[prost(int32, tag = "5")]
        pub start_time: i32,
        #[prost(int32, tag = "6")]
        pub end_time: i32,
        #[prost(bool, tag = "7")]
        pub spk_chg: bool,
        #[prost(int32, tag = "8")]
        pub muted_duration_ms: i32,
        #[prost(string, tag = "9")]
        pub speaker_id: String,
        #[prost(string, optional, tag = "10")]
        pub detected_language: Option<String>,
        #[prost(double, optional, tag = "11")]
        pub language_confidence: Option<f64>,
    }
}

/* ------------------------------- 配置与句柄 ------------------------------- */

/// AST 会话配置（由 commands::maybe_spawn_ast 组装）。
pub struct AstConfig {
    pub session_id: i64,
    pub api_key: String,
    /// 源语言（设置页 speak_language：zh/en）。
    pub source_language: String,
    /// 目标语言（设置页 daily_language：zh/en）。
    pub target_language: String,
    /// 课堂开始时间（rfc3339），用于把 utterance 相对毫秒换算成绝对时间戳。
    pub start_at: String,
    pub model_tier: String,
    /// 硅基流动 Key：行动项检测用（可选，未配置则跳过检测）。
    pub sf_key: Option<String>,
}

/// AST 会话句柄：持有停止标志与上行通道，页面重进时复用。
pub struct AstHandle {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    session_id: i64,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
}

impl AstHandle {
    /// 创建上行通道并启动 tokio 会话任务；句柄内的 `live_tx()` 交给采集线程。
    pub fn spawn(app: AppHandle, pool: SqlitePool, cfg: AstConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
        let sid = cfg.session_id;
        // 文稿整理后台轮询所需字段（在 cfg 被 run() 消耗前克隆）
        let tidy_sid = cfg.session_id;
        let tidy_tier = cfg.model_tier.clone();
        let tidy_key = cfg.sf_key.clone().filter(|k| !k.trim().is_empty());
        let tidy_pool = pool.clone();
        let s = stop.clone();
        let a = alive.clone();
        tauri::async_runtime::spawn(async move {
            run(app, pool, cfg, rx, s).await;
            a.store(false, Ordering::SeqCst);
        });
        // 文稿整理轮询：与 AST 会话同生命周期，每 60s 检查未整理字幕，
        // 累计约 200 词单元调用一次整理（独立异步任务，不阻塞音频上行）
        spawn_tidy_task(stop.clone(), tidy_pool, tidy_sid, &tidy_tier, tidy_key.clone());
        Self {
            stop,
            alive,
            session_id: sid,
            tx,
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst)
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    /// 上行通道克隆（采集线程的 live_tx）。
    pub fn live_tx(&self) -> tokio::sync::mpsc::UnboundedSender<Vec<i16>> {
        self.tx.clone()
    }
}

/// 文稿整理后台轮询任务（AST 与经济转写两种引擎共用）：
/// 每 60s 检查未整理字幕，累计约 200 词单元调用一次整理；随 stop 标志退出。
pub(crate) fn spawn_tidy_task(
    stop: Arc<AtomicBool>,
    pool: SqlitePool,
    session_id: i64,
    model_tier: &str,
    sf_key: Option<String>,
) {
    let Some(sf_key) = sf_key.filter(|k| !k.trim().is_empty()) else {
        return;
    };
    let tier = model_tier.to_string();
    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if stop.load(Ordering::SeqCst) {
                break;
            }
            match crate::pipeline::tidy_remaining(
                &client,
                &sf_key,
                &pool,
                session_id,
                &tier,
                Some(crate::pipeline::TIDY_SPAN_UNITS),
            )
            .await
            {
                Ok(n) if n > 0 => eprintln!("[tidy] 课堂 {session_id} 实时整理 {n} 行字幕"),
                Ok(_) => {}
                Err(e) => eprintln!("[tidy] {e}"),
            }
        }
    });
}

/* ------------------------------ 会话 ID 生成 ------------------------------ */

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_id(prefix: &str) -> String {
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{n}", chrono::Local::now().timestamp_millis())
}

/* ------------------------------ WebSocket 连接 ---------------------------- */

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(api_key: &str) -> AppResult<(WsStream, String)> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{header::HeaderName, HeaderValue};

    let conn_id = new_id("conn");
    // 先用 URL 生成带完整 WS 握手头的请求，再追加鉴权头
    let mut req = WS_URL
        .into_client_request()
        .map_err(|e| AppError::new(format!("AST 握手请求构造失败：{e}")))?;
    let headers = req.headers_mut();
    for (name, value) in [
        ("x-api-key", api_key),
        ("x-api-resource-id", RESOURCE_ID),
        ("x-api-connect-id", conn_id.as_str()),
    ] {
        let v = HeaderValue::from_str(value)
            .map_err(|e| AppError::new(format!("AST 握手头非法：{e}")))?;
        headers.insert(HeaderName::from_static(name), v);
    }
    let (ws, resp) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| AppError::new(format!("AST WebSocket 连接失败：{e}")))?;
    let logid = resp
        .headers()
        .get("x-tt-logid")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    Ok((ws, logid))
}

/* ------------------------------ 请求构造 ------------------------------ */

fn base_request(event: i32, session_id: &str) -> pb::TranslateRequest {
    pb::TranslateRequest {
        request_meta: Some(pb::RequestMeta {
            session_id: session_id.to_string(),
            ..Default::default()
        }),
        event,
        user: Some(pb::User {
            uid: "deskmate".into(),
            did: "deskmate".into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn build_start(session_id: &str, source_language: &str, target_language: &str) -> pb::TranslateRequest {
    pb::TranslateRequest {
        source_audio: Some(pb::Audio::pcm16k()),
        request: Some(pb::ReqParams {
            mode: "s2t".into(),
            source_language: source_language.to_string(),
            target_language: target_language.to_string(),
            ..Default::default()
        }),
        ..base_request(EV_START_SESSION, session_id)
    }
}

fn build_task(session_id: &str, pcm: &[i16]) -> pb::TranslateRequest {
    let mut audio = pb::Audio::pcm16k();
    audio.binary_data = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    pb::TranslateRequest {
        source_audio: Some(audio),
        ..base_request(EV_TASK_REQUEST, session_id)
    }
}

fn build_finish(session_id: &str) -> pb::TranslateRequest {
    pb::TranslateRequest {
        source_audio: Some(pb::Audio::default()),
        ..base_request(EV_FINISH_SESSION, session_id)
    }
}

/* --------------------------- 字幕 utterance 累积 -------------------------- */

/// 单个 utterance 的双语累积器。
/// AST 的字幕增量有两种形态：累积式（本次文本以已累积文本开头，整体替换）
/// 与增量式（本次为新片段，直接拼接），这里按前缀匹配自动识别。
#[derive(Default)]
struct Utterance {
    src: String,
    dst: String,
    src_final: Option<String>,
    /// 首个源字幕事件的音频起点（毫秒，相对录制开始）。
    start_ms: Option<i32>,
    /// AST 给出的原始说话人标识（speaker_id），归档后再聚类为 teacher/other。
    speaker: String,
}

impl Utterance {
    fn active(&self) -> bool {
        !self.src.is_empty() || !self.dst.is_empty()
    }

    /// 合并一段新文本到既有累积串（CJK 相接不加空格，英文相接补空格）。
    fn merge(acc: &mut String, piece: &str) {
        let piece = piece.trim();
        if piece.is_empty() {
            return;
        }
        if acc.is_empty() {
            *acc = piece.to_string();
        } else if piece.starts_with(acc.as_str()) {
            *acc = piece.to_string();
        } else {
            let need_space = acc
                .chars()
                .last()
                .map(|c| c.is_ascii_alphanumeric())
                .unwrap_or(false)
                && piece
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphanumeric())
                    .unwrap_or(false);
            if need_space {
                acc.push(' ');
            }
            acc.push_str(piece);
        }
    }

    /// 定稿（655 或 flush）：返回 (源文, 译文, 起点ms, 说话人)，源文为空时返回 None 不落行。
    fn take_final(&mut self) -> Option<(String, Option<String>, i32, String)> {
        let src = self
            .src_final
            .take()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| self.src.clone());
        let dst = if self.dst.trim().is_empty() {
            None
        } else {
            Some(self.dst.trim().to_string())
        };
        let ms = self.start_ms.unwrap_or(0);
        let speaker = self.speaker.clone();
        *self = Utterance::default();
        if src.trim().is_empty() {
            None
        } else {
            Some((src.trim().to_string(), dst, ms, speaker))
        }
    }
}

/* ------------------------------- 主循环 ------------------------------- */

/// 诊断日志：release 版无控制台，写入数据目录 ast_debug.log，供排查上行速率问题。
struct DebugLog(Option<std::fs::File>);

impl DebugLog {
    fn open(app: &AppHandle) -> Self {
        let f = app
            .path()
            .app_data_dir()
            .ok()
            .and_then(|dir| {
                std::fs::create_dir_all(&dir).ok()?;
                // 每次启动截断重写：只保留最近一次会话的日志
                std::fs::File::create(dir.join("ast_debug.log")).ok()
            });
        Self(f)
    }

    fn log(&mut self, msg: &str) {
        use std::io::Write as _;
        if let Some(f) = &mut self.0 {
            let ts = chrono::Local::now().format("%H:%M:%S%.3f");
            let _ = writeln!(f, "[{ts}] {msg}");
            let _ = f.flush();
        }
        eprintln!("[ast] {msg}");
    }
}

async fn run(
    app: AppHandle,
    pool: SqlitePool,
    cfg: AstConfig,
    mut rx: UnboundedReceiver<Vec<i16>>,
    stop: Arc<AtomicBool>,
) {
    let AstConfig {
        session_id,
        api_key,
        source_language,
        target_language,
        start_at: _,
        model_tier,
        sf_key,
    } = cfg;

    // 字幕 seq 从库中最大值续接（续录/重进不重置）
    let mut next_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), -1) FROM transcript_lines WHERE session_id=?1",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .unwrap_or(-1)
        + 1;

    // 154 计费事件为「本连接累积快照」，这里记录上次快照值，只把增量入账，避免重复计费
    let mut bill_in: i64 = 0;
    let mut bill_out: i64 = 0;

    let prompt_detect = db::get_prompt(&pool, "detect")
        .await
        .unwrap_or_else(|_| pipeline::default_prompt("detect").to_string());
    // 行动项检测若阻塞主循环会停发音频，服务端 30s 即断开（AudioSendSlow）——
    // 因此检测必须在独立任务里跑，主循环只做非阻塞投递
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .ok();

    // 行动项检测任务：主循环投递字幕快照，任务内串行调 DeepSeek
    let (detect_tx, mut detect_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<(String, Option<String>)>>();
    let detect_active = sf_key.is_some() && http.is_some();
    if let (Some(key), Some(client)) = (sf_key.clone(), http.clone()) {
        let app2 = app.clone();
        let pool2 = pool.clone();
        let prompt = prompt_detect.clone();
        let tier = model_tier.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(snapshot) = detect_rx.recv().await {
                let mut buf: VecDeque<(String, Option<String>)> = snapshot.into();
                run_detect(&app2, &pool2, &client, &key, &prompt, session_id, &tier, &mut buf).await;
            }
        });
    }

    let mut utter = Utterance::default();
    let mut last_partial = tokio::time::Instant::now();
    // 上一条已落库字幕（去重：同文同译不重复落库）
    let mut last_emitted: Option<(String, Option<String>)> = None;
    let mut detect_buf: VecDeque<(String, Option<String>)> = VecDeque::new();
    let mut detect_timer = tokio::time::interval(DETECT_INTERVAL);
    detect_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut dlog = DebugLog::open(&app);
    dlog.log(&format!(
        "run start session={session_id} src={source_language} dst={target_language}"
    ));
    let mut sent_total: u64 = 0;
    let mut stats_timer = tokio::time::interval(Duration::from_secs(5));
    stats_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut reconnects: u32 = 0;
    let mut chan_closed = false;
    let mut fatal: Option<String> = None;

    'outer: while !stop.load(Ordering::SeqCst) && !chan_closed && fatal.is_none() {
        let (ws, logid) = match connect(&api_key).await {
            Ok(v) => {
                reconnects = 0;
                v
            }
            Err(e) => {
                reconnects += 1;
                eprintln!("[ast] 连接失败（{reconnects}/{MAX_RECONNECTS}）：{}", e.0);
                if reconnects >= MAX_RECONNECTS {
                    fatal = Some(format!("同传服务连接失败：{}", e.0));
                    break;
                }
                tokio::time::sleep(backoff(reconnects)).await;
                continue;
            }
        };
        eprintln!("[ast] session {session_id} 已连接（logid: {logid}）");
        dlog.log(&format!("connected logid={logid}"));
        let (mut tx, mut ws_rx) = ws.split();
        let ws_sid = new_id("ast");

        if let Err(e) = tx
            .send(Message::Binary(
                build_start(&ws_sid, &source_language, &target_language)
                    .encode_to_vec()
                    .into(),
            ))
            .await
        {
            eprintln!("[ast] StartSession 发送失败：{e}");
            tokio::time::sleep(backoff(1)).await;
            continue;
        }

        let mut session_started = false;
        let mut start_deadline = tokio::time::Instant::now() + START_TIMEOUT;
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut stop_tick = tokio::time::interval(Duration::from_millis(200));
        let mut pending: Vec<i16> = Vec::new();
        // 本连接是否收到了服务端的会话终止事件（152/153）
        let mut server_ended = false;

        'conn: loop {
            tokio::select! {
                _ = stop_tick.tick() => {
                    if stop.load(Ordering::SeqCst) {
                        break 'conn;
                    }
                }
                maybe = rx.recv(), if !chan_closed => {
                    match maybe {
                        Some(samples) => pending.extend_from_slice(&samples),
                        None => chan_closed = true,
                    }
                }
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Binary(bytes))) => {
                            match pb::TranslateResponse::decode(bytes.as_ref()) {
                                Ok(resp) => {
                                    let action = handle_response(
                                        &app, &pool, resp, session_id,
                                        &model_tier, &mut next_seq, &mut utter,
                                        &mut detect_buf, &mut session_started,
                                        &mut last_partial, &mut last_emitted,
                                        &mut bill_in, &mut bill_out,
                                    ).await;
                                    match action {
                                        RespAction::Continue => {}
                                        RespAction::SessionEnded => {
                                            server_ended = true;
                                            break 'conn;
                                        }
                                        RespAction::Fatal(msg) => {
                                            fatal = Some(msg);
                                            break 'conn;
                                        }
                                    }
                                }
                                Err(e) => eprintln!("[ast] protobuf 解码失败：{e}"),
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break 'conn,
                        Some(Err(e)) => {
                            eprintln!("[ast] WebSocket 读取错误：{e}");
                            break 'conn;
                        }
                        _ => {}
                    }
                }
                _ = ping.tick() => {
                    if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break 'conn;
                    }
                }
                _ = tokio::time::sleep_until(start_deadline), if !session_started => {
                    eprintln!("[ast] 等待 SessionStarted 超时");
                    break 'conn;
                }
                _ = detect_timer.tick() => {
                    if detect_active {
                        if !detect_buf.is_empty() {
                            // 非阻塞投递快照，检测在独立任务中执行，主循环绝不等待
                            let snapshot: Vec<(String, Option<String>)> =
                                detect_buf.drain(..).collect();
                            let _ = detect_tx.send(snapshot);
                        }
                    } else {
                        detect_buf.clear();
                    }
                }
                _ = stats_timer.tick() => {
                    dlog.log(&format!(
                        "stats sent={sent_total} pending={} started={session_started}",
                        pending.len()
                    ));
                }
            }

            // 非阻塞收完积压采样。select! 每轮只 recv 一批（采集线程 50ms 推 800 采样），
            // 而 651/654 下行在语音活跃时高频抢占迭代槽位——不耗尽积压会让音频上行
            // 有效速率跌破服务端阈值，约 30 秒后被 AudioSendSlow 踢断。
            loop {
                match rx.try_recv() {
                    Ok(samples) => pending.extend_from_slice(&samples),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        chan_closed = true;
                        break;
                    }
                }
            }
            // 断连重连后 rx 积压可能达数分钟：只保留最近 5 秒，更早的缺口交给归档
            // 管线 ASR 补写，避免瞬时超速补发触发服务端限流、保证字幕紧跟当前语音
            if pending.len() > MAX_PENDING {
                let drop_n = pending.len() - MAX_PENDING;
                pending.drain(..drop_n);
                dlog.log(&format!("pending 超限，丢弃最旧 {drop_n} 采样"));
            }

            // 会话已建立则一次性发完全部整包（每包 80ms）
            while session_started && pending.len() >= PACKET_SAMPLES {
                let pkt: Vec<i16> = pending.drain(..PACKET_SAMPLES).collect();
                if tx
                    .send(Message::Binary(build_task(&ws_sid, &pkt).encode_to_vec().into()))
                    .await
                    .is_err()
                {
                    break 'conn;
                }
                sent_total += 1;
            }
        }

        // 连接收尾：正常停止/音频通道关闭时，发完剩余采样 + FinishSession 并等 152
        if (stop.load(Ordering::SeqCst) || chan_closed) && !server_ended && fatal.is_none() {
            if session_started {
                if !pending.is_empty() {
                    let _ = tx
                        .send(Message::Binary(
                            build_task(&ws_sid, &pending).encode_to_vec().into(),
                        ))
                        .await;
                }
                let _ = tx
                    .send(Message::Binary(
                        build_finish(&ws_sid).encode_to_vec().into(),
                    ))
                    .await;
                // 最多等 3 秒让服务端回 152 并吐出最后的字幕
                let wait_end = tokio::time::Instant::now() + Duration::from_secs(3);
                while tokio::time::Instant::now() < wait_end {
                    match tokio::time::timeout_at(wait_end, ws_rx.next()).await {
                        Ok(Some(Ok(Message::Binary(bytes)))) => {
                            if let Ok(resp) = pb::TranslateResponse::decode(bytes.as_ref()) {
                                let action = handle_response(
                                    &app, &pool, resp, session_id, &model_tier,
                                    &mut next_seq, &mut utter, &mut detect_buf,
                                    &mut session_started, &mut last_partial, &mut last_emitted,
                                    &mut bill_in, &mut bill_out,
                                )
                                .await;
                                if matches!(action, RespAction::SessionEnded | RespAction::Fatal(_)) {
                                    break;
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
        }

        if stop.load(Ordering::SeqCst) || chan_closed || fatal.is_some() {
            break 'outer;
        }
        // 异常断线：退避后重连（采集线程仍在推流，pending 由 rx 继续攒）
        reconnects += 1;
        if reconnects >= MAX_RECONNECTS {
            fatal = Some("同传连接反复中断，已停止重试".into());
            break 'outer;
        }
        eprintln!("[ast] 连接断开，{:?} 后重连", backoff(reconnects));
        dlog.log(&format!(
            "disconnected, reconnect #{reconnects}, sent_total={sent_total}"
        ));
        tokio::time::sleep(backoff(reconnects)).await;
        // 重连后需重新等待 SessionStarted
        let _ = &mut start_deadline;
    }

    // 会话彻底结束：flush 残留 utterance，避免最后一句话丢失
    if let Some((src, dst, ms, spk)) = utter.take_final() {
        emit_line(&app, &pool, session_id, &mut next_seq, &mut last_emitted, &mut detect_buf, src, dst, ms, spk)
            .await;
    }
    if let Some(msg) = fatal {
        let _ = app.emit(
            EVENT_SUBTITLE_STATUS,
            SubtitleStatusEvent {
                session_id,
                message: format!("{msg}；可在设置页检查火山引擎 Key 后重新录制"),
            },
        );
    }
    eprintln!("[ast] session {session_id} 任务结束");
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(3)).min(8)) // 2/4/8/8…
}

/* ------------------------------ 响应事件处理 ------------------------------ */

/// 未定稿的部分字幕（边说边出字）。zh 为空表示译文尚未产出。
#[derive(Clone, serde::Serialize)]
pub struct PartialEvent {
    pub session_id: i64,
    pub en: String,
    pub zh: Option<String>,
}

/// 节流推送部分字幕（651/654 高频到达，按 200ms 节流）。
fn maybe_emit_partial(
    app: &AppHandle,
    session_id: i64,
    utter: &Utterance,
    last: &mut tokio::time::Instant,
) {
    let now = tokio::time::Instant::now();
    if now.duration_since(*last) < PARTIAL_THROTTLE || !utter.active() {
        return;
    }
    *last = now;
    let _ = app.emit(
        EVENT_SUBTITLE_PARTIAL,
        PartialEvent {
            session_id,
            en: utter.src.clone(),
            zh: if utter.dst.is_empty() { None } else { Some(utter.dst.clone()) },
        },
    );
}

/// 定稿后清空前端部分字幕残留（发一条空 partial）。
fn clear_partial(app: &AppHandle, session_id: i64) {
    let _ = app.emit(
        EVENT_SUBTITLE_PARTIAL,
        PartialEvent {
            session_id,
            en: String::new(),
            zh: None,
        },
    );
}

enum RespAction {
    Continue,
    SessionEnded,
    Fatal(String),
}

#[allow(clippy::too_many_arguments)]
async fn handle_response(
    app: &AppHandle,
    pool: &SqlitePool,
    resp: pb::TranslateResponse,
    session_id: i64,
    model_tier: &str,
    next_seq: &mut i64,
    utter: &mut Utterance,
    detect_buf: &mut VecDeque<(String, Option<String>)>,
    session_started: &mut bool,
    last_partial: &mut tokio::time::Instant,
    last_emitted: &mut Option<(String, Option<String>)>,
    bill_in: &mut i64,
    bill_out: &mut i64,
) -> RespAction {
    match resp.event {
        EV_SESSION_STARTED => {
            *session_started = true;
            eprintln!("[ast] SessionStarted");
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "ast".into(),
                    model: AST_MODEL_LABEL.into(),
                    system_prompt: None,
                    user_prompt: Some("AST 2.0 WebSocket 同传会话".into()),
                    response: Some("SessionStarted".into()),
                    ok: true,
                    error: None,
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                },
            )
            .await;
        }
        EV_SOURCE_START => {
            // 新 utterance：上一轮若未走完 655 则先 flush
            if utter.active() {
                if let Some((src, dst, ms, spk)) = utter.take_final() {
                    emit_line(app, pool, session_id, next_seq, last_emitted, detect_buf, src, dst, ms, spk)
                        .await;
                }
            }
            // 换人信号：记录本 utterance 的原始说话人 id
            if !resp.speaker_id.is_empty() {
                utter.speaker = resp.speaker_id.clone();
            }
            clear_partial(app, session_id);
        }
        EV_SOURCE_RESPONSE => {
            if utter.start_ms.is_none() {
                utter.start_ms = Some(resp.start_time);
            }
            if !resp.speaker_id.is_empty() {
                utter.speaker = resp.speaker_id.clone();
            }
            Utterance::merge(&mut utter.src, &resp.text);
            maybe_emit_partial(app, session_id, utter, last_partial);
        }
        EV_SOURCE_END => {
            if !resp.speaker_id.is_empty() {
                utter.speaker = resp.speaker_id.clone();
            }
            Utterance::merge(&mut utter.src, &resp.text);
            if !utter.src.is_empty() {
                utter.src_final = Some(utter.src.clone());
            }
            maybe_emit_partial(app, session_id, utter, last_partial);
        }
        EV_TRANSLATION_START => {}
        EV_TRANSLATION_RESPONSE => {
            Utterance::merge(&mut utter.dst, &resp.text);
            maybe_emit_partial(app, session_id, utter, last_partial);
        }
        EV_TRANSLATION_END => {
            Utterance::merge(&mut utter.dst, &resp.text);
            if let Some((src, dst, ms, spk)) = utter.take_final() {
                emit_line(app, pool, session_id, next_seq, last_emitted, detect_buf, src, dst, ms, spk)
                    .await;
            }
            clear_partial(app, session_id);
        }
        EV_USAGE_RESPONSE => {
            if let Some(meta) = &resp.response_meta {
                if let Some(billing) = &meta.billing {
                    record_usage(pool, session_id, model_tier, billing, bill_in, bill_out).await;
                }
            }
        }
        EV_AUDIO_MUTED => {
            eprintln!("[ast] 服务端提示音频静音 {}ms", resp.muted_duration_ms);
        }
        EV_SESSION_FINISHED => {
            eprintln!("[ast] SessionFinished");
            return RespAction::SessionEnded;
        }
        EV_SESSION_FAILED => {
            let (code, msg) = resp
                .response_meta
                .map(|m| (m.status_code, m.message))
                .unwrap_or((0, "未知错误".into()));
            eprintln!("[ast] SessionFailed：{code} {msg}");
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "ast".into(),
                    model: AST_MODEL_LABEL.into(),
                    system_prompt: None,
                    user_prompt: Some("AST 2.0 WebSocket 同传会话".into()),
                    response: None,
                    ok: false,
                    error: Some(format!("{code} {msg}")),
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                },
            )
            .await;
            // 11200 PERMISSION_DENIED：Key 无效，重连无意义
            if code == 11200 {
                return RespAction::Fatal(format!("火山引擎鉴权失败（{msg}）"));
            }
            return RespAction::SessionEnded;
        }
        _ => {}
    }
    RespAction::Continue
}

/// 154 用量事件 → usage_records。事件中的数量为本连接累积快照，
/// 因此只把相对上次快照的增量入账（重连后服务端从 0 重新累计，负值按 0 处理并重置基线）。
async fn record_usage(
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    billing: &pb::Billing,
    last_in: &mut i64,
    last_out: &mut i64,
) {
    let mut total_in: i64 = 0;
    let mut total_out: i64 = 0;
    for item in &billing.items {
        let q = item.quantity.round() as i64;
        match item.unit.as_str() {
            "input_audio_tokens" => total_in += q,
            "output_text_tokens" | "output_audio_tokens" => total_out += q,
            _ => {}
        }
    }
    if total_in < *last_in {
        *last_in = 0;
    }
    if total_out < *last_out {
        *last_out = 0;
    }
    let input = (total_in - *last_in).max(0);
    let output = (total_out - *last_out).max(0);
    *last_in = total_in;
    *last_out = total_out;
    if input == 0 && output == 0 {
        return;
    }
    let rec = UsageInsert {
        session_id,
        ts: chrono::Local::now().to_rfc3339(),
        stage: "ast".into(),
        model: AST_MODEL_LABEL.into(),
        model_tier: model_tier.to_string(),
        input_tokens: input,
        cached_tokens: 0,
        output_tokens: output,
        audio_seconds: 0,
        amount: pipeline::token_amount(AST_MODEL_LABEL, input, 0, output),
    };
    let _ = db::batch_create_usage(pool, std::slice::from_ref(&rec)).await;
}

/// 定稿一行字幕：去重 → 落库 + 推送事件 + 进检测缓冲。
/// ts 用本地当前时间（流式延迟仅 1~2s，墙钟即真实说话时刻；
/// 服务端 start_time 实测恒为 0，无法用于时间轴）。
#[allow(clippy::too_many_arguments)]
async fn emit_line(
    app: &AppHandle,
    pool: &SqlitePool,
    session_id: i64,
    next_seq: &mut i64,
    last_emitted: &mut Option<(String, Option<String>)>,
    detect_buf: &mut VecDeque<(String, Option<String>)>,
    src: String,
    dst: Option<String>,
    _start_ms: i32,
    speaker: String,
) {
    // 去重：服务端重试/重连场景可能对同一句重发定稿，同文同译不重复落库
    let norm_dst = dst.as_ref().map(|s| s.trim().to_string());
    let norm_src = src.trim().to_string();
    if last_emitted.as_ref() == Some(&(norm_src.clone(), norm_dst.clone())) {
        eprintln!("[ast] 跳过重复字幕行：{norm_src}");
        return;
    }
    *last_emitted = Some((norm_src.clone(), norm_dst.clone()));

    let ts = chrono::Local::now().to_rfc3339();
    let seq = *next_seq;
    if let Err(e) = db::insert_transcript_line(
        pool,
        session_id,
        &ts,
        seq,
        &src,
        dst.as_deref(),
        &speaker,
    )
    .await
    {
        eprintln!("[ast] 字幕落库失败：{}", e.0);
        return;
    }
    *next_seq += 1;
    let _ = app.emit(
        EVENT_SUBTITLE_LINE,
        SubtitleLineEvent {
            session_id,
            seq,
            ts,
            en: src.clone(),
            zh: dst.clone(),
            speaker: speaker.clone(),
        },
    );
    detect_buf.push_back((src, dst));
    while detect_buf.len() > DETECT_CONTEXT_LINES {
        detect_buf.pop_front();
    }
}

/* ------------------------------ 行动项检测 ------------------------------ */

/// 解析检测模型输出；type=none 或解析失败返回 None。
fn parse_detect_json(content: &str) -> Option<(String, String, String, String)> {
    let cleaned = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let v: serde_json::Value = serde_json::from_str(cleaned).ok()?;
    let ty = v["type"].as_str()?.trim().to_lowercase();
    if ty.is_empty() || ty == "none" {
        return None;
    }
    let content = v["content"].as_str()?.trim().to_string();
    if content.is_empty() {
        return None;
    }
    let guide = v["action_guide"].as_str().unwrap_or("").trim().to_string();
    let urgency = v["urgency"].as_str().unwrap_or("medium").trim().to_lowercase();
    Some((ty, content, guide, urgency))
}

/// 最近若干条字幕 → DeepSeek → 命中即落库并推送。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_detect(
    app: &AppHandle,
    pool: &SqlitePool,
    client: &reqwest::Client,
    sf_key: &str,
    prompt_detect: &str,
    session_id: i64,
    model_tier: &str,
    buffer: &mut VecDeque<(String, Option<String>)>,
) {
    if buffer.is_empty() {
        return;
    }
    // 本课堂已识别过的行动项：作为去重上下文注入，避免同一任务反复弹窗。
    let existing = db::list_action_items(pool, session_id)
        .await
        .unwrap_or_default();
    let body = buffer
        .iter()
        .map(|(en, zh)| match zh {
            Some(z) => format!("{en}\n{z}"),
            None => en.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n---\n");
    let context = if existing.is_empty() {
        body
    } else {
        let list = existing
            .iter()
            .rev()
            .take(20)
            .rev()
            .map(|a| format!("- {}", a.content))
            .collect::<Vec<_>>()
            .join("\n");
        format!("本课堂已识别过的行动项（同一件事不要再报）：\n{list}\n\n最近的字幕：\n{body}")
    };
    match pipeline::chat(
        client,
        sf_key,
        pipeline::DETECT_MODEL_ID,
        prompt_detect,
        &context,
    )
    .await
    {
        Ok(out) => {
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "detect".into(),
                    model: pipeline::DETECT_MODEL_LABEL.into(),
                    system_prompt: Some(prompt_detect.to_string()),
                    user_prompt: Some(context.clone()),
                    response: Some(out.content.clone()),
                    ok: true,
                    error: None,
                    input_tokens: out.prompt_tokens,
                    cached_tokens: out.cached_tokens,
                    output_tokens: out.completion_tokens,
                },
            )
            .await;
            if let Some((ty, content, guide, urgency)) = parse_detect_json(&out.content) {
                // 词面去重兜底：模型偶尔仍会重报，归一化后与已有行动项撞车则只记用量、不再落库
                let is_dup = existing
                    .iter()
                    .any(|a| pipeline::similar_item(&a.content, &content));
                let ts_now = chrono::Local::now().to_rfc3339();
                if is_dup {
                    eprintln!("[ast] 行动项与已有重复，跳过：{content}");
                } else {
                    match db::insert_action_item(pool, session_id, &ts_now, &ty, &content, &guide, &urgency, None)
                        .await
                    {
                        Ok(id) => {
                            let item = ActionItemDto {
                                id,
                                session_id,
                                ts: ts_now.clone(),
                                item_type: ty,
                                content,
                                action_guide: if guide.is_empty() { None } else { Some(guide) },
                                urgency: if urgency.is_empty() { None } else { Some(urgency) },
                                ddl: None,
                                added_to_tasks: false,
                            };
                            let _ = app.emit(
                                EVENT_ACTION_ITEM,
                                ActionItemEvent { session_id, item },
                            );
                        }
                        Err(e) => eprintln!("[ast] 行动项落库失败：{}", e.0),
                    }
                }
                let u = UsageInsert {
                    session_id,
                    ts: ts_now,
                    stage: "detect".into(),
                    model: pipeline::DETECT_MODEL_LABEL.into(),
                    model_tier: model_tier.to_string(),
                    input_tokens: out.prompt_tokens,
                    cached_tokens: out.cached_tokens,
                    output_tokens: out.completion_tokens,
                    audio_seconds: 0,
                    amount: pipeline::token_amount(
                        pipeline::DETECT_MODEL_LABEL,
                        out.prompt_tokens,
                        out.cached_tokens,
                        out.completion_tokens,
                    ),
                };
                let _ = db::batch_create_usage(pool, std::slice::from_ref(&u)).await;
            }
            buffer.clear();
        }
        Err(e) => {
            eprintln!("[ast] 行动项检测失败：{}", e.0);
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "detect".into(),
                    model: pipeline::DETECT_MODEL_LABEL.into(),
                    system_prompt: Some(prompt_detect.to_string()),
                    user_prompt: Some(context),
                    response: None,
                    ok: false,
                    error: Some(e.0.clone()),
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                },
            )
            .await;
        }
    }
}

/* ------------------------------ 连通性探测 ------------------------------ */

/// 设置页「测试连接」：建连 + StartSession，等 SessionStarted/Failed，立即收尾。
pub async fn probe(api_key: &str) -> AppResult<String> {
    let (mut ws, logid) = connect(api_key).await?;
    let sid = new_id("ast-probe");
    ws.send(Message::Binary(
        build_start(&sid, "zh", "en").encode_to_vec().into(),
    ))
    .await
    .map_err(|e| AppError::new(format!("StartSession 发送失败：{e}")))?;

    let result = tokio::time::timeout(Duration::from_secs(12), async {
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    let Ok(resp) = pb::TranslateResponse::decode(bytes.as_ref()) else {
                        continue;
                    };
                    match resp.event {
                        EV_SESSION_STARTED => return Ok(()),
                        EV_SESSION_FAILED => {
                            let (code, m) = resp
                                .response_meta
                                .map(|m| (m.status_code, m.message))
                                .unwrap_or((0, "会话建立失败".into()));
                            return Err(AppError::new(format!("服务端拒绝（{code}）：{m}")));
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) => return Err(AppError::new("连接被服务端关闭")),
                Err(e) => return Err(AppError::new(format!("连接错误：{e}"))),
                _ => {}
            }
        }
        Err(AppError::new("连接意外结束"))
    })
    .await;

    let _ = ws.send(Message::Binary(build_finish(&sid).encode_to_vec().into())).await;
    let _ = ws.close(None).await;

    match result {
        Ok(Ok(())) => Ok(if logid.is_empty() {
            "连接成功：AST 2.0 同传会话建立正常".to_string()
        } else {
            format!("连接成功：AST 2.0 同传会话建立正常（logid: {logid}）")
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AppError::new("等待服务端响应超时（12 秒）")),
    }
}

/* --------------------------------- 测试 --------------------------------- */

#[cfg(test)]
mod tests {
    use super::pb;
    use super::Utterance;
    use prost::Message as _;

    #[test]
    fn protobuf_roundtrip() {
        let req = super::build_start("s-1", "zh", "en");
        let bytes = req.encode_to_vec();
        let back = pb::TranslateRequest::decode(&bytes[..]).expect("解码失败");
        assert_eq!(back.event, super::EV_START_SESSION);
        assert_eq!(
            back.request_meta.map(|m| m.session_id),
            Some("s-1".to_string())
        );
        let audio = back.source_audio.expect("缺少 source_audio");
        assert_eq!((audio.rate, audio.bits, audio.channel), (16000, 16, 1));
        let params = back.request.expect("缺少 request");
        assert_eq!(params.mode, "s2t");
        assert_eq!(params.source_language, "zh");
        assert_eq!(params.target_language, "en");
    }

    #[test]
    fn task_request_carries_pcm_bytes() {
        let pcm: Vec<i16> = vec![0, 1, -1, 32767, -32768];
        let req = super::build_task("s-2", &pcm);
        let bytes = req.encode_to_vec();
        let back = pb::TranslateRequest::decode(&bytes[..]).expect("解码失败");
        assert_eq!(back.event, super::EV_TASK_REQUEST);
        let data = back.source_audio.expect("缺少音频").binary_data;
        let restored: Vec<i16> = data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(restored, pcm);
    }

    #[test]
    fn merge_cumulative_replaces() {
        let mut acc = String::new();
        Utterance::merge(&mut acc, "今天");
        Utterance::merge(&mut acc, "今天我们讲");
        Utterance::merge(&mut acc, "今天我们讲微积分");
        assert_eq!(acc, "今天我们讲微积分");
    }

    #[test]
    fn merge_incremental_appends() {
        let mut acc = String::new();
        Utterance::merge(&mut acc, "今天");
        Utterance::merge(&mut acc, "我们讲微积分");
        assert_eq!(acc, "今天我们讲微积分");
    }

    #[test]
    fn merge_english_words_get_space() {
        let mut acc = String::new();
        Utterance::merge(&mut acc, "hello");
        Utterance::merge(&mut acc, "world");
        assert_eq!(acc, "hello world");
    }

    #[test]
    fn take_final_prefers_final_source() {
        let mut u = Utterance::default();
        Utterance::merge(&mut u.src, "今天我们讲");
        Utterance::merge(&mut u.src, "今天我们讲微积分");
        u.src_final = Some(u.src.clone());
        Utterance::merge(&mut u.dst, "Today we talk about calculus");
        let (src, dst, _, _speaker) = u.take_final().expect("应有定稿");
        assert_eq!(src, "今天我们讲微积分");
        assert_eq!(dst.as_deref(), Some("Today we talk about calculus"));
        assert!(!u.active());
    }

    #[test]
    fn packetize_80ms() {
        // 1600 采样 → 1 个完整 1280 包 + 320 残留
        let mut pending: Vec<i16> = (0..1600).collect();
        let mut sent = 0usize;
        while pending.len() >= super::PACKET_SAMPLES {
            let _: Vec<i16> = pending.drain(..super::PACKET_SAMPLES).collect();
            sent += 1;
        }
        assert_eq!(sent, 1);
        assert_eq!(pending.len(), 320);
    }

    /* --------------------- 真机联调（需 VOLC_API_KEY 环境变量） --------------------- */

    use prost::Message as _;

    fn read_wav_pcm(path: &std::path::Path) -> Vec<i16> {
        let bytes = std::fs::read(path).expect("读取 WAV 失败");
        // 定位 data chunk（跳过可能存在的 LIST/fact 等附加块）
        let mut pos = 12usize;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            if id == b"data" {
                let data = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
                return data
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect();
            }
            pos += 8 + size + (size & 1);
        }
        panic!("WAV 中未找到 data chunk");
    }

    /// 真实探测：建连 + StartSession，断言收到 SessionStarted。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "需要真实 API Key：VOLC_API_KEY=xxx cargo test ast_live -- --ignored"]
    async fn ast_live_probe() {
        let key = std::env::var("VOLC_API_KEY").expect("缺少 VOLC_API_KEY 环境变量");
        let msg = super::probe(&key).await.expect("probe 失败");
        println!("probe 结果：{msg}");
    }

    /// 真实同传：把音频按 80ms 包流式发送，打印全部下行事件与字幕文本。
    /// 环境变量：VOLC_API_KEY 必需；AST_WAV=音频路径（默认 demo 中文音频）；
    /// AST_LANG=源:目标（默认 zh:en）。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "需要真实 API Key：VOLC_API_KEY=xxx cargo test ast_live_stream -- --ignored"]
    async fn ast_live_stream() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let key = std::env::var("VOLC_API_KEY").expect("缺少 VOLC_API_KEY 环境变量");
        let wav = std::env::var("AST_WAV")
            .unwrap_or_else(|_| r"C:\Users\24253\AppData\Local\Temp\ast\py\ast_python\test_audio.wav".into());
        let lang = std::env::var("AST_LANG").unwrap_or_else(|_| "zh:en".into());
        let (src_lang, dst_lang) = lang.split_once(':').expect("AST_LANG 格式应为 源:目标");
        let pcm = read_wav_pcm(std::path::Path::new(&wav));
        println!("音频：{wav} 采样 {}（约 {:.1}s），语言对 {src_lang}→{dst_lang}", pcm.len(), pcm.len() as f64 / 16000.0);

        let (ws, logid) = super::connect(&key).await.expect("连接失败");
        println!("已连接（logid: {logid}）");
        let (mut tx, mut ws_rx) = ws.split();
        let sid = super::new_id("ast-test");
        tx.send(Message::Binary(
            super::build_start(&sid, src_lang, dst_lang).encode_to_vec().into(),
        ))
        .await
        .expect("StartSession 发送失败");

        // 发送任务：80ms 包 + 按真实节奏间隔
        let sid2 = sid.clone();
        let sender = tokio::spawn(async move {
            let mut sent = 0usize;
            for chunk in pcm.chunks(super::PACKET_SAMPLES) {
                if tx
                    .send(Message::Binary(
                        super::build_task(&sid2, chunk).encode_to_vec().into(),
                    ))
                    .await
                    .is_err()
                {
                    break;
                }
                sent += 1;
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
            let _ = tx
                .send(Message::Binary(
                    super::build_finish(&sid2).encode_to_vec().into(),
                ))
                .await;
            sent
        });

        // 接收：打印事件，直到 152/153 或超时 60s
        let mut got_started = false;
        let mut subtitle_events = 0u32;
        let mut acc_src = String::new();
        let mut acc_dst = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            let Ok(Some(msg)) =
                tokio::time::timeout(std::time::Duration::from_secs(10), ws_rx.next()).await
            else {
                println!("接收超时，结束");
                break;
            };
            let Ok(Message::Binary(bytes)) = msg else { continue };
            let Ok(resp) = pb::TranslateResponse::decode(bytes.as_ref()) else { continue };
            match resp.event {
                150 => {
                    got_started = true;
                    println!("[150] SessionStarted");
                }
                651 => {
                    subtitle_events += 1;
                    super::Utterance::merge(&mut acc_src, &resp.text);
                    if let Some(dl) = &resp.detected_language {
                        println!("[651] 检测语言：{dl}（置信度 {:?}）", resp.language_confidence);
                    }
                    println!("[651] 源语：{}", acc_src);
                }
                652 => println!("[652] 源语定稿：{acc_src}"),
                654 => {
                    subtitle_events += 1;
                    super::Utterance::merge(&mut acc_dst, &resp.text);
                    println!("[654] 译文：{}", acc_dst);
                }
                655 => {
                    println!("[655] 译文定稿：{acc_dst}");
                    acc_src.clear();
                    acc_dst.clear();
                }
                152 => {
                    println!("[152] SessionFinished");
                    break;
                }
                153 => {
                    let meta = resp.response_meta.unwrap_or_default();
                    panic!("SessionFailed：{} {}", meta.status_code, meta.message);
                }
                154 => {
                    if let Some(meta) = resp.response_meta {
                        if let Some(b) = meta.billing {
                            for item in &b.items {
                                println!("[154] 用量 {} = {:.1}", item.unit, item.quantity);
                            }
                        }
                    }
                }
                other => println!("[{other}] text={}", resp.text),
            }
        }
        let sent = sender.await.unwrap_or(0);
        println!("共发送 {sent} 个音频包");
        assert!(got_started, "未收到 SessionStarted");
        assert!(subtitle_events > 0, "未收到任何字幕事件（651/654）");
    }

    /// 真机全链路（物理无声，可在用户使用电脑时运行）：EndpointMute 端点级静音整机
    /// 外放（用户听不到）→ 独立播放器播放英文 WAV → 按应用进程回环采集（生产路径）
    /// → AST 2.0 同传。这是"接管课堂 + 转录不全/AudioSendSlow"的联合交付闸门，断言：
    /// 1) 端点静音后进程回环仍能采到非静音音频（RMS，本测试最关键的假设）；
    /// 2) 采集速率 ≥ 80%（修复 1/5 丢包 bug 后应为实时速率）；
    /// 3) 全程无 SessionFailed（45000014 AudioSendSlow 等）；
    /// 4) 源语为英文（拉丁字符占比 ≥ 50%，语言参数生效）；
    /// 5) 译文定稿贯穿全程（首末定稿间隔 ≥ 30s，非"只有前几句"）。
    /// 环境变量：VOLC_API_KEY 必需；AST_E2E_WAV=播放文件（默认 60s 英文，含 1.5s 静音头）。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "真机全链路（物理无声）：VOLC_API_KEY=xxx cargo test loopback_ast_e2e -- --ignored --nocapture"]
    async fn loopback_ast_e2e() {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::time::Duration;
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        const RUN_SECS: u64 = 63;

        let key = std::env::var("VOLC_API_KEY").expect("缺少 VOLC_API_KEY");
        let wav = std::env::var("AST_E2E_WAV")
            .unwrap_or_else(|_| r"C:\Users\24253\AppData\Local\Temp\ast\en_silent_60.wav".into());
        assert!(std::path::Path::new(&wav).exists(), "播放文件不存在：{wav}");

        // 1) 先端点静音整机外放（物理无声；不影响回环取数）
        let mute = crate::win_audio::EndpointMute::engage().expect("端点静音失败");

        // 2) 启动专用播放器进程（用户听不到）
        let mut player = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(New-Object Media.SoundPlayer '{wav}').PlaySync()"),
            ])
            .spawn()
            .expect("播放器启动失败");
        let player_pid = player.id();

        // 3) 按应用进程回环采集（与网课模式选 Teams/浏览器 + 接管课堂同一代码路径）
        let chunks_dir = std::env::temp_dir().join("ast_e2e_chunks");
        let _ = std::fs::remove_dir_all(&chunks_dir);
        std::fs::create_dir_all(&chunks_dir).unwrap();
        let (live_tx, mut live_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
        let mut cap = crate::capture::CaptureThread::new(crate::capture::CaptureConfig {
            session_id: 990077,
            chunks_dir: chunks_dir.clone(),
            mode: "online".into(),
            target_pid: Some(player_pid),
            mic_gain: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            live_tx: Some(live_tx),
        });
        cap.start().expect("采集启动失败");
        let t0 = std::time::Instant::now();

        // 4) 建立 AST 会话
        let (ws, logid) = super::connect(&key).await.expect("AST 连接失败");
        println!("AST 已连接（logid: {logid}）");
        let (mut tx, mut ws_rx) = ws.split();
        let sid = super::new_id("ast-e2e");
        tx.send(Message::Binary(
            super::build_start(&sid, "en", "zh").encode_to_vec().into(),
        ))
        .await
        .expect("StartSession 失败");

        #[derive(Default)]
        struct Stats {
            samples: u64,
            sq_sum: f64,
            packets: u64,
            fail_codes: Vec<i32>,
            final_times: Vec<std::time::Instant>,
            /// 652 源语（英文）定稿文本
            src_texts: Vec<String>,
            /// 655 译文（中文）定稿文本
            zh_texts: Vec<String>,
        }
        let stats = Arc::new(std::sync::Mutex::new(Stats::default()));

        // 4) 上行任务：采集来多少就打包发多少（80ms/包）
        let stats_s = stats.clone();
        let sid2 = sid.clone();
        let sender = tokio::spawn(async move {
            let mut pending: Vec<i16> = Vec::new();
            let mut last_report = std::time::Instant::now();
            loop {
                match tokio::time::timeout(Duration::from_millis(500), live_rx.recv()).await {
                    Ok(Some(samples)) => {
                        {
                            let mut g = stats_s.lock().unwrap();
                            for &s in &samples {
                                g.samples += 1;
                                g.sq_sum += (s as f64) * (s as f64);
                            }
                        }
                        pending.extend_from_slice(&samples);
                        while pending.len() >= super::PACKET_SAMPLES {
                            let pkt: Vec<i16> = pending.drain(..super::PACKET_SAMPLES).collect();
                            if tx
                                .send(Message::Binary(
                                    super::build_task(&sid2, &pkt).encode_to_vec().into(),
                                ))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            stats_s.lock().unwrap().packets += 1;
                        }
                        if last_report.elapsed() >= Duration::from_secs(5) {
                            let g = stats_s.lock().unwrap();
                            println!(
                                "[上行] 已采 {} 采样（{:.1}s 音频），发包 {}，RMS={:.0}",
                                g.samples,
                                g.samples as f64 / 16000.0,
                                g.packets,
                                (g.sq_sum / g.samples.max(1) as f64).sqrt()
                            );
                            last_report = std::time::Instant::now();
                        }
                    }
                    Ok(None) => {
                        // 采集停止：通道关闭，发 FinishSession 让服务端吐出最后字幕
                        let _ = tx
                            .send(Message::Binary(super::build_finish(&sid2).encode_to_vec().into()))
                            .await;
                        return;
                    }
                    Err(_) => {
                        if t0.elapsed() >= Duration::from_secs(RUN_SECS + 3) {
                            return;
                        }
                    }
                }
            }
        });

        // 5) 下行任务：统计失败码与定稿时刻/文本
        let stats_r = stats.clone();
        let receiver = tokio::spawn(async move {
            while t0.elapsed() < Duration::from_secs(RUN_SECS + 2) {
                let Ok(Some(Ok(Message::Binary(bytes)))) =
                    tokio::time::timeout(Duration::from_secs(2), ws_rx.next()).await
                else {
                    continue;
                };
                let Ok(resp) = pb::TranslateResponse::decode(bytes.as_ref()) else { continue };
                match resp.event {
                    150 => println!("[150] SessionStarted"),
                    652 => {
                        let mut g = stats_r.lock().unwrap();
                        g.final_times.push(std::time::Instant::now());
                        g.src_texts.push(resp.text.clone());
                        println!("[652] 源语定稿：{}", resp.text);
                    }
                    655 => {
                        stats_r.lock().unwrap().zh_texts.push(resp.text.clone());
                        println!("[655] 译文定稿：{}", resp.text);
                    }
                    153 => {
                        let code = resp.response_meta.as_ref().map(|m| m.status_code).unwrap_or(-1);
                        stats_r.lock().unwrap().fail_codes.push(code);
                        eprintln!("[153] SessionFailed code={code} msg={}", resp.text);
                    }
                    _ => {}
                }
            }
        });

        // 6) 跑满时长后收尾
        tokio::time::sleep(Duration::from_secs(RUN_SECS)).await;
        cap.stop();
        let _ = sender.await;
        let _ = player.wait();
        let _ = tokio::time::timeout(Duration::from_secs(5), receiver).await;
        mute.release(); // 恢复端点声音（panic 时 Drop 也会兜底）

        let g = stats.lock().unwrap();
        let elapsed = t0.elapsed().as_secs_f64();
        let captured_secs = g.samples as f64 / 16000.0;
        let rms = (g.sq_sum / g.samples.max(1) as f64).sqrt();
        let span = match (g.final_times.first(), g.final_times.last()) {
            (Some(a), Some(b)) => b.duration_since(*a).as_secs_f64(),
            _ => 0.0,
        };
        // 源语拉丁字母占比（英文识别正确性；中文源语会落在 ~0%）
        let src_all: String = g.src_texts.concat();
        let letters = src_all.chars().filter(|c| c.is_ascii_alphabetic()).count() as f64;
        let latin_ratio = if src_all.trim().is_empty() {
            0.0
        } else {
            letters / src_all.chars().filter(|c| !c.is_whitespace()).count() as f64
        };
        // 落盘分块复核
        let chunks: Vec<_> = std::fs::read_dir(&chunks_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        println!("================ E2E 结果（无声版） ================");
        println!("墙钟 {elapsed:.1}s，采集音频 {captured_secs:.1}s，速率比 {:.2}", captured_secs / elapsed);
        println!("RMS={rms:.0}，发包 {}，落盘分块 {}：{chunks:?}", g.packets, chunks.len());
        println!("失败码 {:?}，源语定稿 {} 条（拉丁占比 {:.0}%），译文 {} 条，首末跨度 {span:.1}s",
            g.fail_codes, g.src_texts.len(), latin_ratio * 100.0, g.zh_texts.len());
        for (en, zh) in g.src_texts.iter().zip(g.zh_texts.iter()) {
            println!("  EN: {en}");
            println!("  ZH: {zh}");
        }

        assert!(captured_secs / elapsed >= 0.80, "采集速率不足 80%（{captured_secs:.1}s/{elapsed:.1}s）——回环仍在丢包");
        assert!(rms > 200.0, "端点静音后进程回环采到纯静音（RMS={rms:.0}）：假设不成立，接管课堂会掐断录音");
        assert!(g.fail_codes.is_empty(), "AST 会话失败码：{:?}", g.fail_codes);
        assert!(g.zh_texts.len() >= 3, "译文定稿不足 3 条：{:?}", g.zh_texts);
        assert!(span >= 30.0, "字幕只出现在前半段（首末跨度 {span:.1}s）——转录不全回归");
        assert!(latin_ratio >= 0.5, "源语不是英文（拉丁字符占比 {:.0}%）：语言参数未生效", latin_ratio * 100.0);
    }
}
