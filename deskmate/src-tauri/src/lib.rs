mod ast;
mod capture;
mod cheap;
mod commands;
mod db;
mod error;
mod face;
mod mail;
mod models;
mod pipeline;
mod ppt;
mod presence;
mod secrets;
mod storage;
mod win_audio;
use sqlx::SqlitePool;
use tauri::Manager;

/// AST 实时上行通道类型（16kHz mono i16 PCM 批次）。
pub type EngineTarget = tokio::sync::mpsc::UnboundedSender<Vec<i16>>;

/// 全局应用状态：SQLite 连接池 + 课件存储目录 + 音频采集线程句柄 + 密钥仓库 + 在位检测线程句柄 + AST 同传句柄。
pub struct AppState {
    pub pool: SqlitePool,
    pub data_dir: std::path::PathBuf,
    pub capture: std::sync::Mutex<Option<capture::CaptureThread>>,
    pub presence: std::sync::Mutex<Option<presence::PresenceThread>>,
    /// AST 2.0 同传会话句柄。
    pub ast: std::sync::Mutex<Option<ast::AstHandle>>,
    /// 经济转写（分块低成本）引擎句柄，与 ast 互斥。
    pub cheap: std::sync::Mutex<Option<cheap::CheapHandle>>,
    /// 音频中转槽：当前 AST 引擎的实时上行通道（None 时音频只落盘）。
    /// 外层随每次采集重启更换，内层由路由任务持有，切换引擎只改内层。
    pub engine_slot: std::sync::Arc<
        std::sync::Mutex<
            Option<std::sync::Arc<std::sync::Mutex<Option<EngineTarget>>>>,
        >,
    >,
    pub secrets: std::sync::Mutex<secrets::SecretsStore>,
    /// 麦克风软件增益（f32 位存储，采集线程实时读取）。
    pub mic_gain: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// 「接管课堂」选中的目标进程 pid（None=未接管）。
    pub muted_pid: std::sync::Mutex<Option<u32>>,
    /// 「接管课堂」端点静音句柄（接管期间整机静音，进程回环录音不受影响）。
    pub endpoint_mute: std::sync::Mutex<Option<win_audio::EndpointMute>>,
    /// 聊天「记入笔记」确认请求：request_id → 等待用户批准/拒绝的答复通道。
    pub note_requests: std::sync::Mutex<
        std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>,
    >,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 应用数据目录：%APPDATA%\com.deskmate.app（Windows）
            let data_dir = app.path().app_data_dir()?;
            let pool = tauri::async_runtime::block_on(db::init(&data_dir))?;
            // 旧 captures/materials 目录迁移到「课程-课堂-类型」结构
            tauri::async_runtime::block_on(storage::migrate_legacy(&pool, &data_dir));
            let secrets = secrets::SecretsStore::load(&data_dir)?;
            // 启动时从设置读取麦克风增益，供采集线程实时读取
            let mic_gain = tauri::async_runtime::block_on(db::get_settings(&pool))
                .map(|s| s.mic_gain)
                .unwrap_or(1.0) as f32;
            app.manage(AppState {
                pool,
                data_dir,
                capture: std::sync::Mutex::new(None),
                presence: std::sync::Mutex::new(None),
                ast: std::sync::Mutex::new(None),
                cheap: std::sync::Mutex::new(None),
                engine_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
                secrets: std::sync::Mutex::new(secrets),
                mic_gain: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(
                    mic_gain.to_bits(),
                )),
                muted_pid: std::sync::Mutex::new(None),
                endpoint_mute: std::sync::Mutex::new(None),
                note_requests: std::sync::Mutex::new(std::collections::HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_courses,
            commands::create_course,
            commands::update_course,
            commands::delete_course,
            commands::list_sessions,
            commands::get_session,
            commands::create_session,
            commands::finish_session,
            commands::list_deleted_sessions,
            commands::soft_delete_session,
            commands::restore_session,
            commands::purge_session,
            commands::upload_material,
            commands::list_materials,
            commands::open_material,
            commands::delete_material,
            commands::list_todos,
            commands::create_todo,
            commands::update_todo,
            commands::set_todo_done,
            commands::delete_todo,
            commands::set_action_item_ddl,
            commands::list_usage,
            commands::get_settings,
            commands::save_settings,
            commands::start_capture,
            commands::stop_capture,
            commands::capture_status,
            commands::get_secret_config,
            commands::save_secret_config,
            commands::test_siliconflow,
            commands::test_volc,
            commands::test_mail,
            commands::send_reminder_email,
            commands::start_presence,
            commands::stop_presence,
            commands::presence_status,
            commands::simulate_presence,
            commands::probe_camera,
            commands::list_presence_events,
            commands::list_transcript,
            commands::list_action_items,
            commands::mark_action_item,
            commands::unmark_action_item,
            commands::start_live,
            commands::stop_live,
            commands::set_live_engine,
            commands::regenerate_note,
            commands::resume_session,
            commands::deskmate_chat_stream,
            commands::respond_note_save,
            commands::list_tidy_passages,
            commands::get_prompt_templates,
            commands::save_prompt_template,
            commands::reset_prompt_templates,
            commands::list_llm_logs,
            commands::clear_llm_logs,
            commands::open_captures_dir,
            commands::list_audio_apps,
            commands::take_over_class,
            commands::release_class,
            commands::set_mic_gain,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // 应用退出时恢复端点静音，避免关闭同桌小轩后整机仍被静音
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(state) = app.try_state::<AppState>() {
                    release_takeover(&state);
                }
            }
        });
}

/// 若处于接管状态则释放端点静音并清除目标 pid（归档/停止采集/应用退出共用）。
pub fn release_takeover(state: &AppState) {
    // drop EndpointMute 即恢复接管前的端点静音状态
    if let Ok(mut g) = state.endpoint_mute.lock() {
        g.take();
    }
    if let Ok(mut g) = state.muted_pid.lock() {
        *g = None;
    }
}
