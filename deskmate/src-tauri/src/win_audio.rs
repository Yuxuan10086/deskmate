//! Windows 音频会话辅助：枚举正在播放声音的应用、接管课堂（端点级静音）。
//!
//! - 应用选择器：枚举默认输出设备上的音频会话（IAudioSessionManager2 → IAudioSessionEnumerator），
//!   结合 ToolHelp 进程快照解析进程名，供网课模式「按应用采集」选择目标。
//! - 接管课堂：通过 IAudioEndpointVolume 静音整个输出端点。
//!   注意不能用会话级 ISimpleAudioVolume::SetMute——会话音量位于进程回环取数点之前，
//!   静音目标应用会让进程回环录音一起变纯静音（2026-10-17 实测）；端点音量位于
//!   回环取数点之后，物理输出完全静音而回环录音正常。代价是接管期间整机静音。

use std::collections::HashMap;

use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    AudioSessionStateActive,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};

use crate::error::{AppError, AppResult};

/// 一个可选为采集目标/静音目标的应用。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioApp {
    pub pid: u32,
    /// 进程友好名（exe 文件名，去扩展名）。
    pub name: String,
    /// 会话当前是否处于 Active（正在播放声音）。
    pub active: bool,
}

fn win_err(context: &str, e: windows::core::Error) -> AppError {
    AppError::new(format!("{context}：{e}"))
}

/// PID → exe 文件名（如 "msedge.exe"）。
pub fn process_names() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len(&entry)]);
                map.entry(entry.th32ProcessID).or_insert(name);
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    map
}

/// 取 PROCESSENTRY32W.szExeFile 的实际长度（到 NUL 为止）。
fn name_len(entry: &PROCESSENTRY32W) -> usize {
    entry
        .szExeFile
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(entry.szExeFile.len())
}

fn display_name(exe: &str) -> String {
    exe.strip_suffix(".exe")
        .unwrap_or(exe)
        .to_string()
}

/// 线程安全初始化 COM（MTA，可重复调用）。
pub fn ensure_com() -> AppResult<()> {
    unsafe {
        windows::Win32::System::Com::CoIncrementMTAUsage()
            .map(|_| ())
            .map_err(|e| win_err("初始化 COM 失败", e))
    }
}

/// 枚举默认输出设备上的音频会话对应的应用（供前端选择采集目标）。
pub fn list_audio_apps() -> AppResult<Vec<AudioApp>> {
    ensure_com()?;
    let names = process_names();

    unsafe {
        let manager = default_render_manager()?;
        let session_enum = manager
            .GetSessionEnumerator()
            .map_err(|e| win_err("枚举音频会话失败", e))?;
        let count = session_enum
            .GetCount()
            .map_err(|e| win_err("读取会话数量失败", e))?;

        // 同一进程可能有多个会话，按 pid 合并；任一 Active 即视为 Active。
        let mut order: Vec<u32> = Vec::new();
        let mut apps: HashMap<u32, AudioApp> = HashMap::new();
        for i in 0..count {
            let Ok(control) = session_enum.GetSession(i) else {
                continue;
            };
            let Ok(ctl2) = control.cast::<IAudioSessionControl2>() else {
                continue;
            };
            let Ok(pid) = ctl2.GetProcessId() else {
                continue;
            };
            // 系统声音（无归属进程）与自身进程不列出
            if pid == 0 || pid == std::process::id() {
                continue;
            }
            let active = control.GetState().map(|s| s == AudioSessionStateActive) == Ok(true);
            let name = names
                .get(&pid)
                .map(|exe| display_name(exe))
                .unwrap_or_else(|| format!("PID {pid}"));
            let entry = apps.entry(pid).or_insert_with(|| {
                order.push(pid);
                AudioApp {
                    pid,
                    name: name.clone(),
                    active,
                }
            });
            entry.name = name;
            entry.active = entry.active || active;
        }

        // Active 的排前面，同组内按名称排序保证列表稳定
        order.sort_by_key(|pid| {
            let a = &apps[pid];
            (!a.active, a.name.to_lowercase())
        });
        Ok(order
            .into_iter()
            .filter_map(|pid| apps.remove(&pid))
            .collect())
    }
}

/// 该进程当前是否有处于 Active 的渲染会话（即正在播放声音）。
/// 用于按应用采集的自检：明明在放声音却采不到，说明选错了进程。
pub fn is_playing(pid: u32) -> bool {
    list_audio_apps()
        .map(|apps| apps.iter().any(|a| a.pid == pid && a.active))
        .unwrap_or(false)
}

/// 默认输出（渲染）设备。
unsafe fn default_render_device() -> AppResult<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| win_err("创建设备枚举器失败", e))?;
        enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .map_err(|e| win_err("获取默认输出设备失败", e))
    }
}

/// 取默认输出设备的会话管理器（枚举会话用）。
unsafe fn default_render_manager() -> AppResult<IAudioSessionManager2> {
    unsafe {
        let device = default_render_device()?;
        device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| win_err("激活会话管理器失败", e))
    }
}

/// 接管课堂的端点级静音句柄：构造时静音整个默认输出端点，Drop/显式释放时恢复原状。
///
/// 端点静音是全局单一状态（不像浏览器会话会动态新建），无需轮询守卫。
/// 端点音量位于回环取数点之后，静音不影响系统回环/进程回环采集。
pub struct EndpointMute {
    /// 接管前端点是否本来就是静音状态（释放时恢复，而不是强制取消静音）。
    was_muted: bool,
    released: bool,
}

impl EndpointMute {
    /// 静音默认输出端点（整机静音，录音不受影响）。
    pub fn engage() -> AppResult<Self> {
        ensure_com()?;
        unsafe {
            let device = default_render_device()?;
            let volume: IAudioEndpointVolume = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| win_err("激活端点音量控制失败", e))?;
            let was_muted = volume
                .GetMute()
                .map_err(|e| win_err("读取端点静音状态失败", e))?
                .as_bool();
            volume
                .SetMute(true, std::ptr::null())
                .map_err(|e| win_err("端点静音失败", e))?;
            Ok(Self {
                was_muted,
                released: false,
            })
        }
    }

    /// 恢复接管前的端点静音状态。
    pub fn release(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let was_muted = self.was_muted;
        let restore = (|| -> AppResult<()> {
            ensure_com()?;
            unsafe {
                let device = default_render_device()?;
                let volume: IAudioEndpointVolume = device
                    .Activate(CLSCTX_ALL, None)
                    .map_err(|e| win_err("激活端点音量控制失败", e))?;
                volume
                    .SetMute(was_muted, std::ptr::null())
                    .map_err(|e| win_err("恢复端点静音状态失败", e))?;
            }
            Ok(())
        })();
        if let Err(e) = restore {
            eprintln!("[win_audio] 恢复端点声音失败：{e:?}");
        }
    }
}

impl Drop for EndpointMute {
    fn drop(&mut self) {
        // 兜底：未显式 release 时也要恢复声音
        self.shutdown();
    }
}
