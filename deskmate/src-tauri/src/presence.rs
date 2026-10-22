//! 在位检测：固定周期采样 + 连续 2 帧去抖，维护「在位 / 不在位 / 不可用」状态机。
//!
//! PRD 5.6：每 10s 检测一次；连续 2 次（约 20s）未检测到人脸判定「不在位」，
//! 连续 2 次检测到人脸判定「在位」。摄像头被占用或无权限时标记「不可用」并提示。
//!
//! 信号源默认为摄像头人脸检测（`face` 模块）。`simulate_presence` 可注入固定帧，
//! 用于无摄像头环境下的全流程验证；注入 3 表示交回摄像头自动检测。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};

use crate::face::{CameraFaceSource, FaceSignal};

/// 去抖帧数：连续 2 帧同向才切换状态。
const DEBOUNCE_FRAMES: i32 = 2;
/// 人工注入值：0=在位 1=缺席 2=不可用；3=交回摄像头自动检测。
const HINT_AUTO: u8 = 3;
/// 摄像头打开/读帧失败后的冷却轮数（约 60s 再重试一次）。
const REOPEN_COOLDOWN: u32 = 6;
/// 摄像头已打开却连续多少轮取不到画面即判定「不可用」（约 30s）。
const EMPTY_ROUNDS_LIMIT: u32 = 3;

/// 前端事件名：在位状态变化。
pub const EVENT_PRESENCE_CHANGED: &str = "presence://changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    Present,
    Away,
    Unavailable,
}

impl PresenceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PresenceState::Present => "present",
            PresenceState::Away => "away",
            PresenceState::Unavailable => "unavailable",
        }
    }
}

/// 单帧采样结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Present,
    Absent,
    Unavailable,
}

impl Frame {
    fn from_hint(v: u8) -> Self {
        match v {
            1 => Frame::Absent,
            2 => Frame::Unavailable,
            _ => Frame::Present,
        }
    }
}

/// 去抖状态机（纯逻辑，可单测）。
pub struct PresenceMachine {
    state: PresenceState,
    last: Option<Frame>,
    count: i32,
}

impl PresenceMachine {
    pub fn new() -> Self {
        Self {
            state: PresenceState::Present,
            last: None,
            count: 0,
        }
    }

    pub fn state(&self) -> PresenceState {
        self.state
    }

    /// 喂入一帧；仅当状态真正切换时返回新状态。
    pub fn feed(&mut self, frame: Frame) -> Option<PresenceState> {
        if frame == Frame::Unavailable {
            let changed = self.state != PresenceState::Unavailable;
            self.state = PresenceState::Unavailable;
            self.last = Some(frame);
            self.count = 1;
            return if changed { Some(self.state) } else { None };
        }
        if self.last == Some(frame) {
            self.count += 1;
        } else {
            self.last = Some(frame);
            self.count = 1;
        }
        if self.count >= DEBOUNCE_FRAMES {
            let next = if frame == Frame::Absent {
                PresenceState::Away
            } else {
                PresenceState::Present
            };
            if next != self.state {
                self.state = next;
                return Some(self.state);
            }
        }
        None
    }
}

/// 状态变化事件载荷（推送给前端做双通道提醒）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceEvent {
    pub state: PresenceState,
    pub session_id: i64,
    pub ts: String,
}

/// 在位检测线程句柄：持有停止标志、人工注入值与信号源状态。
pub struct PresenceThread {
    stop: Arc<AtomicBool>,
    hint: Arc<AtomicU8>,
    machine: Arc<Mutex<PresenceMachine>>,
    detail: Arc<Mutex<Option<String>>>,
    session_id: i64,
}

impl PresenceThread {
    /// 启动检测线程：按 `interval_sec` 周期采样，切换时落库并向前端发事件。
    ///
    /// `camera_enabled` 为 false 时不做摄像头判定（仅支持人工注入验证）。
    pub fn start(
        app: AppHandle,
        pool: SqlitePool,
        session_id: i64,
        interval_sec: u64,
        camera_enabled: bool,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let hint = Arc::new(AtomicU8::new(HINT_AUTO));
        let machine = Arc::new(Mutex::new(PresenceMachine::new()));
        let detail = Arc::new(Mutex::new(None));

        let (s, h, m, d) = (
            stop.clone(),
            hint.clone(),
            machine.clone(),
            detail.clone(),
        );
        let interval = interval_sec.max(1);
        let thread_name = format!("deskmate-presence-{session_id}");
        let _ = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                run_loop(s, h, m, d, pool, app, session_id, interval, camera_enabled);
            });

        Self {
            stop,
            hint,
            machine,
            detail,
            session_id,
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> PresenceState {
        self.machine
            .lock()
            .map(|g| g.state())
            .unwrap_or(PresenceState::Present)
    }

    /// 摄像头不可用等异常原因（正常时为 None）。
    pub fn detail(&self) -> Option<String> {
        self.detail.lock().ok().and_then(|g| g.clone())
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    /// 注入采样帧：0=在位 1=缺席 2=不可用 3=恢复摄像头自动检测。
    pub fn set_hint(&self, hint: u8) {
        self.hint.store(hint, Ordering::Relaxed);
    }
}

/// 检测线程主体：周期采样 → 去抖 → 状态切换时落库并推送。
#[allow(clippy::too_many_arguments)]
fn run_loop(
    stop: Arc<AtomicBool>,
    hint: Arc<AtomicU8>,
    machine: Arc<Mutex<PresenceMachine>>,
    detail: Arc<Mutex<Option<String>>>,
    pool: SqlitePool,
    app: AppHandle,
    session_id: i64,
    interval: u64,
    camera_enabled: bool,
) {
    let mut source: Option<CameraFaceSource> = None;
    let mut cooldown = 0u32;
    let mut empty_rounds = 0u32;

    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if stop.load(Ordering::Relaxed) {
            break;
        }

        let injected = hint.load(Ordering::Relaxed);
        let frame = if injected != HINT_AUTO {
            // 人工注入优先，便于无摄像头环境验证整条链路
            Some(Frame::from_hint(injected))
        } else if !camera_enabled {
            // 未启用摄像头检测：明确标记「不可用」，此前按「在位」处理会让用户误以为检测在工作
            set_detail(&detail, Some("已在设置中关闭摄像头在位检测，无法判断离开".into()));
            Some(Frame::Unavailable)
        } else {
            sample_camera(&mut source, &mut cooldown, &mut empty_rounds, &detail)
        };
        let Some(frame) = frame else {
            continue; // 首帧尚未就绪，跳过本轮，不参与去抖
        };

        let transition = machine.lock().ok().and_then(|mut g| g.feed(frame));
        if let Some(next) = transition {
            let ts = chrono::Local::now().to_rfc3339();
            tauri::async_runtime::block_on(async {
                if let Err(e) =
                    crate::db::insert_presence_event(&pool, Some(session_id), &ts, next.as_str())
                        .await
                {
                    eprintln!("[presence] 事件落库失败：{}", e.0);
                }
            });
            let _ = app.emit(
                EVENT_PRESENCE_CHANGED,
                PresenceEvent {
                    state: next,
                    session_id,
                    ts,
                },
            );
        }
    }

    if let Some(src) = source.take() {
        src.close();
    }
}

/// 采样一帧摄像头画面。
///
/// 返回 `None` 仅表示「本轮画面尚未就绪」（首次打开时的 1–2 轮过渡）；
/// 一旦确认摄像头不可用（打不开 / 读帧失败 / 连续取不到画面），返回
/// `Frame::Unavailable`——绝不再静默按「在位」处理（那会让用户以为检测在工作）。
fn sample_camera(
    source: &mut Option<CameraFaceSource>,
    cooldown: &mut u32,
    empty_rounds: &mut u32,
    detail: &Arc<Mutex<Option<String>>>,
) -> Option<Frame> {
    if *cooldown > 0 {
        *cooldown -= 1;
        if source.is_none() {
            // 已知打不开，冷却期间持续报「不可用」
            return Some(Frame::Unavailable);
        }
    } else if source.is_none() {
        match CameraFaceSource::open() {
            Ok(src) => {
                *source = Some(src);
                *empty_rounds = 0;
                set_detail(detail, None);
            }
            Err(e) => {
                set_detail(detail, Some(e.0.clone()));
                *cooldown = REOPEN_COOLDOWN;
                return Some(Frame::Unavailable);
            }
        }
    }

    let Some(src) = source.as_ref() else {
        return Some(Frame::Unavailable);
    };
    match src.detect() {
        Some(FaceSignal::Present) => {
            *empty_rounds = 0;
            Some(Frame::Present)
        }
        Some(FaceSignal::Absent) => {
            *empty_rounds = 0;
            Some(Frame::Absent)
        }
        Some(FaceSignal::Unavailable) => {
            if let Some(old) = source.take() {
                old.close();
            }
            set_detail(detail, Some("摄像头画面读取失败，可能被其它程序占用".into()));
            *cooldown = REOPEN_COOLDOWN;
            Some(Frame::Unavailable)
        }
        None => {
            // 持续取不到画面：判定不可用并进入冷却重试
            *empty_rounds += 1;
            if *empty_rounds >= EMPTY_ROUNDS_LIMIT {
                if let Some(old) = source.take() {
                    old.close();
                }
                set_detail(
                    detail,
                    Some("摄像头已打开但取不到画面，请检查是否被其它程序占用".into()),
                );
                *cooldown = REOPEN_COOLDOWN;
                return Some(Frame::Unavailable);
            }
            None
        }
    }
}

fn set_detail(slot: &Arc<Mutex<Option<String>>>, value: Option<String>) {
    if let Ok(mut g) = slot.lock() {
        *g = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机自测：摄像头人脸检测链路（需物理摄像头，默认忽略）。
    ///
    /// 运行：`cargo test --lib camera_live -- --ignored --nocapture`
    /// 交付前必跑：历史上 WinRT 异步阻塞式 `GetResults()` 会让整条链路静默失效。
    #[test]
    #[ignore]
    fn camera_live_probe() {
        match CameraFaceSource::open() {
            Ok(src) => {
                let mut got = None;
                for _ in 0..12 {
                    if let Some(sig) = src.detect() {
                        got = Some(sig);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                src.close();
                assert!(got.is_some(), "摄像头已打开但取不到画面");
                eprintln!("[camera_live] 采样成功，信号 = {got:?}");
            }
            Err(e) => panic!("摄像头打不开：{}", e.0),
        }
    }

    #[test]
    fn debounce_two_frames() {
        let mut m = PresenceMachine::new();
        // 单帧缺席不切换
        assert_eq!(m.feed(Frame::Absent), None);
        assert_eq!(m.state(), PresenceState::Present);
        // 第二帧缺席 → 不在位
        assert_eq!(m.feed(Frame::Absent), Some(PresenceState::Away));
        // 回到座位同样需要两帧
        assert_eq!(m.feed(Frame::Present), None);
        assert_eq!(m.feed(Frame::Present), Some(PresenceState::Present));
    }

    #[test]
    fn unavailable_switches_immediately() {
        let mut m = PresenceMachine::new();
        assert_eq!(m.feed(Frame::Unavailable), Some(PresenceState::Unavailable));
        // 已不可用时不再重复上报
        assert_eq!(m.feed(Frame::Unavailable), None);
    }
}
