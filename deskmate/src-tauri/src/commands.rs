//! Tauri IPC 命令。命令名与参数（camelCase）必须与前端 src/api/ipc.ts 契约一致。

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use tauri::State;

use crate::ast;
use crate::cheap;
use crate::db;
use crate::error::AppResult;
use crate::mail;
use crate::models::*;
use crate::pipeline;
use crate::ppt;
use crate::presence;
use crate::secrets::{self, SecretConfigDto, SecretConfigPatch};
use crate::AppState;
use crate::capture;

/* --------------------------------- 课程 ---------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub async fn list_courses(state: State<'_, AppState>) -> AppResult<Vec<CourseDto>> {
    db::list_courses(&state.pool).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_course(
    input: CourseInputDto,
    state: State<'_, AppState>,
) -> AppResult<CourseDto> {
    db::create_course(&state.pool, input).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_course(
    id: i64,
    input: CourseInputDto,
    state: State<'_, AppState>,
) -> AppResult<CourseDto> {
    db::update_course(&state.pool, id, input).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_course(id: i64, state: State<'_, AppState>) -> AppResult<()> {
    db::delete_course(&state.pool, id).await
}

/* --------------------------------- 课堂 ---------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub async fn list_sessions(
    course_id: Option<i64>,
    state: State<'_, AppState>,
) -> AppResult<Vec<SessionDto>> {
    db::list_sessions(&state.pool, course_id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_session(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<Option<SessionDto>> {
    db::get_session(&state.pool, id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_session(
    input: SessionInputDto,
    state: State<'_, AppState>,
) -> AppResult<SessionDto> {
    db::create_session(&state.pool, input).await
}

/* ------------------------------- 课堂回收站 -------------------------------- */

/// 回收站列表（已软删除的课堂）。
#[tauri::command]
pub async fn list_deleted_sessions(state: State<'_, AppState>) -> AppResult<Vec<SessionDto>> {
    db::list_deleted_sessions(&state.pool).await
}

/// 软删除课堂（移入回收站，不在课程中显示）。
#[tauri::command]
pub async fn soft_delete_session(id: i64, state: State<'_, AppState>) -> AppResult<()> {
    db::soft_delete_session(&state.pool, id).await
}

/// 从回收站恢复课堂。
#[tauri::command]
pub async fn restore_session(id: i64, state: State<'_, AppState>) -> AppResult<()> {
    db::restore_session(&state.pool, id).await
}

/// 彻底删除课堂：清理数据库记录并删除录音/字幕等文件目录。
#[tauri::command]
pub async fn purge_session(id: i64, state: State<'_, AppState>) -> AppResult<()> {
    // 删除前先取课程归属，用于定位新结构目录
    let course_id = db::get_session(&state.pool, id)
        .await?
        .map(|s| s.course_id);
    db::purge_session(&state.pool, id).await?;
    // 新结构：整个课堂目录（课堂录音/文稿/附件）
    if let Some(cid) = course_id {
        let dir = crate::storage::session_dir(&state.data_dir, cid, id);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    // 兼容：旧结构残留
    let old = state.data_dir.join("captures").join(format!("session_{id}"));
    if old.exists() {
        std::fs::remove_dir_all(&old)
            .map_err(|e| crate::error::AppError::new(format!("删除录音目录失败：{e}")))?;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn finish_session(
    id: i64,
    payload: FinishPayloadDto,
    state: State<'_, AppState>,
) -> AppResult<SessionDto> {
    // 0. 停止两种实时字幕引擎（若为本课堂运行），稍等残留字幕 flush 落库
    {
        let ast_running = stop_ast_for(&state, id);
        let cheap_running = stop_cheap_for(&state, id);
        clear_engine_target(&state);
        if ast_running || cheap_running {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
    }

    // 1. 先拿到课堂元信息（model_tier / start_at）
    let sess = db::get_session(&state.pool, id)
        .await?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;

    // 2. 读取密钥与语言设置（无 Key 时管线自动降级 mock）
    let api_key = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        guard.get(secrets::key::SILICONFLOW_API_KEY)
    };
    let settings = db::get_settings(&state.pool).await?;
    let chunks_dir = crate::storage::audio_dir(&state.data_dir, sess.course_id, id);
    let doc_dir = crate::storage::doc_dir(&state.data_dir, sess.course_id, id);
    std::fs::create_dir_all(&chunks_dir).ok();
    std::fs::create_dir_all(&doc_dir).ok();

    // 3. 跑管线：真实 Stage 优先，失败降级 mock
    let report = pipeline::run_pipeline(
        &state.pool,
        id,
        payload.duration_sec,
        &sess.model_tier,
        &sess.start_at,
        &chunks_dir,
        api_key,
        &settings.speak_language,
        &settings.daily_language,
    )
    .await?;

    // 4. 双语文本 md 落盘（文稿/）：有整理稿用整理版，否则原始字幕；非老师发言加【同学】
    match db::list_transcript_lines(&state.pool, id).await {
        Ok(lines) if !lines.is_empty() => {
            let passages = db::list_tidy_passages(&state.pool, id).await?;
            let mark = |text: &str, speaker: &str| -> String {
                if speaker == "other" {
                    format!("【同学】{text}")
                } else {
                    text.to_string()
                }
            };
            let mut md = format!("# {} · 双语文本\n\n", sess.title);
            md.push_str("> 记号说明：以【同学】开头的内容为非授课老师（学生/其他人）的发言。\n\n");
            if passages.is_empty() {
                for l in &lines {
                    md.push_str(&format!(
                        "**[{}] {}**\n{}\n{}\n\n",
                        l.seq,
                        l.ts,
                        mark(&l.en, &l.speaker),
                        mark(l.zh.as_deref().unwrap_or("（未翻译）"), &l.speaker)
                    ));
                }
            } else {
                md.push_str("> 本文稿为整理版：已合并被过度分割的语句并去除口误。\n\n");
                // 整理段按覆盖的 seq 区间查该段主导说话人
                for p in &passages {
                    let speaker = lines
                        .iter()
                        .find(|l| l.seq >= p.from_seq && l.seq <= p.to_seq)
                        .map(|l| l.speaker.as_str())
                        .unwrap_or("");
                    md.push_str(&format!(
                        "{}\n{}\n\n",
                        mark(&p.en, speaker),
                        mark(p.zh.as_deref().unwrap_or("（未翻译）"), speaker)
                    ));
                }
            }
            let _ = std::fs::write(doc_dir.join("双语文本.md"), md);
        }
        _ => {}
    }

    // 4.5 合并本次段落的原音频分块为一个完整 WAV；续录的新段落单独成段，不混入之前的录音
    match capture::merge_new_chunks(&chunks_dir) {
        Ok(Some(m)) => eprintln!(
            "[merge] 课堂 {id} 第 {} 段录音合并完成（{} 个分块）→ {}",
            m.segment,
            m.chunks,
            m.path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
        ),
        Ok(None) => {}
        Err(e) => eprintln!("[merge] 课堂 {id} 录音合并不影响归档：{e}"),
    }

    // 5. 管线生成笔记时，若调用方未提供笔记则采用管线结果
    let mut payload = payload;
    if payload.note.as_ref().map_or(true, |n| n.trim().is_empty()) {
        if let Some(note) = report.note {
            payload.note = Some(note);
        }
    }

    // 6. 归档 session 状态（内部自动查 usage_records 汇总回填 cost_estimate）
    let result = db::finish_session(&state.pool, id, payload).await;
    // 归档即停止本机接管（若开了「接管课堂」则恢复应用声音）
    release_class_mute_impl(&state);
    result
}

/* --------------------------- 实时字幕/行动项 ------------------------------ */

#[tauri::command(rename_all = "camelCase")]
pub async fn list_transcript(
    session_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<TranscriptLineDto>> {
    db::list_transcript_lines(&state.pool, session_id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_action_items(
    session_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<ActionItemDto>> {
    db::list_action_items(&state.pool, session_id).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn mark_action_item(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<Option<TodoDto>> {
    let item = db::get_action_item(&state.pool, id)
        .await?
        .ok_or_else(|| crate::error::AppError::new("行动项不存在"))?;
    // 幂等：已加入过直接返回已有待办
    if let Some(existing) = db::todo_by_action_item(&state.pool, id).await? {
        db::mark_action_item(&state.pool, id).await?;
        return Ok(Some(existing));
    }
    // 先建待办、后置标记：任一步中断都不会出现「已加入却无待办」的死状态
    let detail = item
        .action_guide
        .clone()
        .filter(|s| !s.trim().is_empty());
    let todo = db::create_todo(
        &state.pool,
        Some(item.session_id),
        Some(id),
        "ai",
        &item.content,
        detail.as_deref(),
        item.ddl.as_deref(),
    )
    .await?;
    db::mark_action_item(&state.pool, id).await?;
    Ok(Some(todo))
}

/// 取消加入待办：删除对应待办并清除行动项标记。
#[tauri::command(rename_all = "camelCase")]
pub async fn unmark_action_item(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<()> {
    db::unmark_action_item(&state.pool, id).await
}

/* -------------------------------- 待办 ---------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub async fn list_todos(state: State<'_, AppState>) -> AppResult<Vec<TodoDto>> {
    db::list_todos(&state.pool).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn create_todo(
    input: CreateTodoPayload,
    state: State<'_, AppState>,
) -> AppResult<TodoDto> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(crate::error::AppError::new("待办内容不能为空"));
    }
    let ddl = normalize_ddl(input.ddl.as_deref());
    db::create_todo(
        &state.pool,
        input.session_id,
        None,
        "manual",
        title,
        input.detail.as_deref(),
        ddl.as_deref(),
    )
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn update_todo(
    id: i64,
    input: UpdateTodoPayload,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let ddl = normalize_ddl(input.ddl.as_deref());
    db::update_todo(
        &state.pool,
        id,
        input.title.trim(),
        input.detail.as_deref(),
        ddl.as_deref(),
    )
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn set_todo_done(
    id: i64,
    done: bool,
    state: State<'_, AppState>,
) -> AppResult<()> {
    db::set_todo_done(&state.pool, id, done).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn delete_todo(id: i64, state: State<'_, AppState>) -> AppResult<()> {
    db::delete_todo(&state.pool, id).await
}

/// 手动补行动项 DDL（同步给关联待办）。
#[tauri::command(rename_all = "camelCase")]
pub async fn set_action_item_ddl(
    id: i64,
    ddl: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let ddl = normalize_ddl(ddl.as_deref());
    db::set_action_item_ddl(&state.pool, id, ddl.as_deref()).await?;
    if let Some(todo) = db::todo_by_action_item(&state.pool, id).await? {
        db::update_todo(
            &state.pool,
            todo.id,
            &todo.title,
            todo.detail.as_deref(),
            ddl.as_deref(),
        )
        .await?;
    }
    Ok(())
}

/// DDL 字符串归一化：空串→None；纯日期 (YYYY-MM-DD) 补成当天 23:59 RFC3339（本地时区）。
fn normalize_ddl(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        use chrono::TimeZone;
        let dt = d.and_hms_opt(23, 59, 0)?;
        return Some(chrono::Local.from_local_datetime(&dt).single()?.to_rfc3339());
    }
    Some(s.to_string())
}

/// 启动实时字幕（AST 2.0 同传）。未配置火山引擎 Key 时返回 false（前端提示），
/// 不再降级到硅基流动分块转写链路。
#[tauri::command(rename_all = "camelCase")]
pub fn start_live(
    session_id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    // 兼容旧前端：未配置火山 Key 时返回 false 而非报错
    match spawn_ast_engine(&app, &state, session_id) {
        Ok(()) => Ok(true),
        Err(e) if e.0.contains("火山引擎") => Ok(false),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn stop_live(state: State<'_, AppState>) -> AppResult<()> {
    if let Ok(g) = state.ast.lock() {
        if let Some(h) = g.as_ref() {
            h.stop();
        }
    }
    if let Ok(g) = state.cheap.lock() {
        if let Some(h) = g.as_ref() {
            h.stop();
        }
    }
    clear_engine_target(&state);
    Ok(())
}

/// 用课件重新生成课堂笔记（PRD 5.8）：以字幕为主线、PPT 章节结构重组。
#[tauri::command(rename_all = "camelCase")]
pub async fn regenerate_note(id: i64, state: State<'_, AppState>) -> AppResult<SessionDto> {
    let key = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        guard.get(secrets::key::SILICONFLOW_API_KEY)
    }
    .ok_or_else(|| crate::error::AppError::new("未配置硅基流动 API Key"))?;

    let sess = db::get_session(&state.pool, id)
        .await?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;

    let lines = db::list_transcript_lines(&state.pool, id).await?;
    if lines.is_empty() {
        return Err(crate::error::AppError::new("本节课无转写文本，无法生成笔记"));
    }
    // 使用带说话人记号的文稿（整理稿优先，非老师发言带【同学】前缀）
    let transcript = pipeline::marked_transcript(&state.pool, id).await?;
    // 附件参考块（课件PPT 解析；作业题目/其他附件仅列名）
    let material_ref = pipeline::build_material_ref(&state.pool, id).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| crate::error::AppError::new(format!("HTTP 客户端创建失败：{e}")))?;
    let mut note_tpl = db::get_prompt(&state.pool, "note")
        .await
        .unwrap_or_else(|_| pipeline::default_prompt("note").to_string());
    // 注入说话人记号说明
    note_tpl.push_str("\n");
    note_tpl.push_str(pipeline::SPEAKER_MARKER_NOTE);
    let note_label = pipeline::note_model(&sess.model_tier).1;
    let (mut note, prompt_tokens, cached_tokens, completion_tokens, note_sys, note_user) =
        match pipeline::generate_note(
            &client,
            &key,
            &transcript,
            &material_ref,
            &sess.model_tier,
            &note_tpl,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = db::insert_llm_log(
                    &state.pool,
                    db::LlmLogInsert {
                        session_id: Some(id),
                        stage: "note".into(),
                        model: note_label.into(),
                        system_prompt: Some(note_tpl.clone()),
                        user_prompt: None,
                        response: None,
                        ok: false,
                        error: Some(e.0.clone()),
                        input_tokens: 0,
                        cached_tokens: 0,
                        output_tokens: 0,
                    },
                )
                .await;
                return Err(e);
            }
        };
    let _ = db::insert_llm_log(
        &state.pool,
        db::LlmLogInsert {
            session_id: Some(id),
            stage: "note".into(),
            model: note_label.into(),
            system_prompt: Some(note_sys),
            user_prompt: Some(note_user),
            response: Some(note.clone()),
            ok: true,
            error: None,
            input_tokens: prompt_tokens,
            cached_tokens,
            output_tokens: completion_tokens,
        },
    )
    .await;

    // 行动项附到笔记末尾
    let items = db::list_action_items(&state.pool, id).await?;
    if !items.is_empty() {
        let list = items
            .iter()
            .enumerate()
            .map(|(i, it)| format!("{}. {}", i + 1, it.content))
            .collect::<Vec<_>>()
            .join("\n");
        note = format!("{note}\n\n## 待办行动项\n{list}");
    }

    // 用量行 + 笔记写回 + 花费回填
    let rec = pipeline::UsageInsert {
        session_id: id,
        ts: chrono::Local::now().to_rfc3339(),
        stage: "note".into(),
        model: note_label.into(),
        model_tier: sess.model_tier.clone(),
        input_tokens: prompt_tokens,
        cached_tokens,
        output_tokens: completion_tokens,
        audio_seconds: 0,
        amount: pipeline::token_amount(note_label, prompt_tokens, cached_tokens, completion_tokens),
    };
    db::batch_create_usage(&state.pool, &[rec]).await?;
    db::update_session_note(&state.pool, id, &note).await?;
    db::refresh_session_cost(&state.pool, id).await?;

    db::get_session(&state.pool, id)
        .await?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))
}

/* ----------------------------- 继续录制/对话 ------------------------------ */

/// 归档后重新打开课堂继续录制（分块与字幕序号自动续接，时长累计）。
#[tauri::command(rename_all = "camelCase")]
pub async fn resume_session(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<SessionDto> {
    db::resume_session(&state.pool, id).await
}

/// 同桌小轩聊天历史轮次。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnDto {
    pub role: String,
    pub content: String,
}

/// 同桌小轩聊天（SSE 流式输出）：工具调用段非流式执行，最终回答通过 onEvent 逐字推送。
/// 事件类型：{type:"status",stage} / {type:"delta",text} / {type:"noteRequest",…}。
/// 返回 {reply, savedToNote}（savedToNote 表示本次有内容经用户批准写入了笔记）。
#[tauri::command(rename_all = "camelCase")]
pub async fn deskmate_chat_stream(
    scope: String,
    course_id: Option<i64>,
    session_id: Option<i64>,
    history: Vec<ChatTurnDto>,
    text: String,
    image_data_url: Option<String>,
    on_event: tauri::ipc::Channel<serde_json::Value>,
    state: State<'_, AppState>,
) -> AppResult<serde_json::Value> {
    if text.trim().is_empty() && image_data_url.as_deref().unwrap_or("").trim().is_empty() {
        return Err(crate::error::AppError::new("请输入问题或粘贴文件/图片"));
    }
    let key = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        guard.get(secrets::key::SILICONFLOW_API_KEY)
    }
    .ok_or_else(|| crate::error::AppError::new("未配置硅基流动 API Key"))?;

    let settings = db::get_settings(&state.pool).await?;
    let persona = pipeline::persona_block(settings.persona.as_deref());

    // 按范围构建系统提示词；usage_session 为 (课堂id, 模型档位)，仅 session 范围记账
    let (system, usage_session): (String, Option<(i64, String)>) = match scope.as_str() {
        "course" => {
            let courses = db::list_courses(&state.pool).await?;
            let course = course_id
                .and_then(|cid| courses.iter().find(|c| c.id == cid))
                .ok_or_else(|| crate::error::AppError::new("课程不存在"))?;
            let sessions = db::list_sessions(&state.pool, course_id).await?;
            let sessions_meta: Vec<serde_json::Value> = sessions
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "title": s.title,
                        "startAt": s.start_at,
                        "status": s.status,
                        "durationSec": s.duration_sec,
                    })
                })
                .collect();
            // 课程附件清单（含课程公共与本课程各课堂的附件）
            let mats = db::list_materials(&state.pool, course.id).await?;
            let mats_text = if mats.is_empty() {
                "（暂无附件）".to_string()
            } else {
                mats.iter()
                    .map(|m| {
                        format!(
                            "- {}（{}，{} KB{}）",
                            m.file_name,
                            m.category,
                            m.size_bytes / 1024,
                            match m.session_id {
                                None => "，课程公共",
                                Some(_) => "",
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (
                format!(
                    "{persona}\n当前界面：课程《{}》主页（当前未在上课）。\n\
                     本课程信息：{}\n本课程的课堂列表：{}\n本课程的附件清单：\n{}\n\
                     回答本课程相关问题时优先利用以上信息；需要某节课的字幕、行动项或笔记细节时，\
                     调用提供的工具查询，不要编造。",
                    course.name,
                    serde_json::json!({
                        "id": course.id, "name": course.name, "code": course.code,
                        "location": course.location, "startWeek": course.start_week,
                        "endWeek": course.end_week,
                    }),
                    serde_json::to_string(&sessions_meta).unwrap_or_default(),
                    mats_text
                ),
                None,
            )
        }
        "session" => {
            let sid = session_id.ok_or_else(|| crate::error::AppError::new("缺少课堂 id"))?;
            let sess = db::get_session(&state.pool, sid)
                .await?
                .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
            // 所属课程元数据（课堂可能未关联课程）
            let course_meta = match sess.course_id {
                Some(cid) => db::list_courses(&state.pool)
                    .await?
                    .into_iter()
                    .find(|c| c.id == cid)
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id, "name": c.name, "code": c.code,
                            "location": c.location, "startWeek": c.start_week,
                            "endWeek": c.end_week,
                        })
                        .to_string()
                    }),
                None => None,
            };
            // 附件：本课堂附件 + 课程公共附件（session_id 为空）
            let (mut session_mats, mut course_mats): (Vec<String>, Vec<String>) =
                (Vec::new(), Vec::new());
            if let Some(cid) = sess.course_id {
                for m in db::list_materials(&state.pool, cid).await? {
                    let label = format!("{}（{}，{} KB）", m.file_name, m.category, m.size_bytes / 1024);
                    if m.session_id == Some(sid) {
                        session_mats.push(label);
                    } else if m.session_id.is_none() {
                        course_mats.push(label);
                    }
                }
            }
            let join_or = |v: &[String]| {
                if v.is_empty() {
                    "（无）".to_string()
                } else {
                    v.iter()
                        .map(|s| format!("- {s}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            };
            // 本节课已识别的行动项（学生常问「一共几个答题/作业」）
            let items = db::list_action_items(&state.pool, sid).await?;
            let items_text = if items.is_empty() {
                "（本节课暂无识别到行动项）".to_string()
            } else {
                items
                    .iter()
                    .map(|a| {
                        let guide = a
                            .action_guide
                            .as_deref()
                            .filter(|g| !g.trim().is_empty())
                            .map(|g| format!("；做法：{}", trunc_str(g, 200)))
                            .unwrap_or_default();
                        format!(
                            "- [{}] {}（DDL：{}{}）",
                            a.item_type,
                            a.content,
                            a.ddl.as_deref().unwrap_or("未识别"),
                            guide
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let lines = db::list_transcript_lines(&state.pool, sid).await?;
            let recent: Vec<String> = lines
                .iter()
                .rev()
                .take(40)
                .rev()
                .map(|l| {
                    let mark = |t: &str| -> String {
                        if l.speaker == "other" {
                            format!("【同学】{t}")
                        } else {
                            t.to_string()
                        }
                    };
                    format!(
                        "[{}] {} / {}",
                        l.ts,
                        mark(&l.en),
                        l.zh.as_deref().map(|z| mark(z)).unwrap_or_else(|| "（未翻译）".to_string())
                    )
                })
                .collect();
            let recording = sess.status == "recording";
            let note_part = sess
                .note
                .as_deref()
                .map(|n| n.trim())
                .filter(|n| !n.is_empty())
                .map(|n| format!("本节课已生成的笔记摘录：\n{}\n", trunc_str(n, 1500)))
                .unwrap_or_default();
            (
                format!(
                    "{persona}\n当前界面：课堂《{}》的详情页，{}。\n\
                     所属课程：{}\n\
                     本节课信息：{}\n\
                     本节课附件：\n{}\n课程公共附件：\n{}\n\
                     本节课已识别的行动项（课后任务）：\n{}\n\
                     {note_part}\
                     下面是本节课截至最近的实时字幕（可能不完整）：\n{}\n\
                     {}\n\
                     学生提问可能附带课堂照片/板书或上传的文件，需要识图理解；\
                     上述信息不足以回答时（更早的字幕、其他课堂、课程全局情况）调用工具查询，不要编造。",
                    sess.title,
                    if recording {
                        "正在上课中，实时字幕进行中"
                    } else {
                        "课堂录制已结束"
                    },
                    course_meta.as_deref().unwrap_or("（未关联课程）"),
                    serde_json::json!({
                        "id": sess.id, "title": sess.title, "lectureNo": sess.lecture_no,
                        "weekNo": sess.week_no, "startAt": sess.start_at, "endAt": sess.end_at,
                        "location": sess.location, "mode": sess.mode, "status": sess.status,
                        "durationSec": sess.duration_sec,
                    }),
                    join_or(&session_mats),
                    join_or(&course_mats),
                    items_text,
                    if recent.is_empty() {
                        "（暂无字幕）".to_string()
                    } else {
                        recent.join("\n")
                    },
                    pipeline::SPEAKER_MARKER_NOTE
                ),
                Some((sid, sess.model_tier.clone())),
            )
        }
        _ => (
            format!(
                "{persona}\n当前界面：课程总览页，学生可能询问他所有课程与课堂的情况。\
                 不要假设任何课程/课堂数据——需要时调用提供的工具针对性查询\
                 （列课程、列课堂、查课堂详情、查字幕、搜字幕），再基于查询结果回答。"
            ),
            None,
        ),
    };

    // 组装消息历史（保留最近 24 条）与新消息
    let image = image_data_url.filter(|s| !s.trim().is_empty());
    let user_content = match &image {
        Some(url) => serde_json::json!([
            { "type": "text", "text": text.trim() },
            { "type": "image_url", "image_url": { "url": url } }
        ]),
        None => serde_json::json!(text.trim()),
    };
    let mut messages: Vec<serde_json::Value> = history
        .iter()
        .filter(|t| (t.role == "user" || t.role == "assistant") && !t.content.trim().is_empty())
        .map(|t| serde_json::json!({ "role": t.role, "content": t.content }))
        .collect();
    if messages.len() > 24 {
        let drop = messages.len() - 24;
        messages.drain(0..drop);
    }
    messages.push(serde_json::json!({ "role": "user", "content": user_content }));

    // 成功/失败日志共用的用户输入摘要（成功时再补工具轨迹）
    let user_prompt_base = if image.is_some() {
        format!("{} [附图片/文件]", text.trim())
    } else {
        text.trim().to_string()
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| crate::error::AppError::new(format!("HTTP 客户端创建失败：{e}")))?;

    let mut usage = (0i64, 0i64, 0i64);
    let mut tool_trace = String::new();
    let mut saved = false;
    let model_label: &str;
    let mut reply = String::new();

    if image.is_some() {
        // 带图：视觉模型单轮流式，无工具
        model_label = pipeline::CHAT_VISION_MODEL_LABEL;
        let _ = on_event.send(serde_json::json!({"type":"status","stage":"thinking"}));
        let out = match pipeline::chat_messages_stream(
            &client,
            &key,
            pipeline::CHAT_VISION_MODEL_ID,
            &system,
            &messages,
            None,
            |piece| {
                let _ = on_event.send(serde_json::json!({"type":"delta","text": piece}));
            },
        )
        .await
        {
            Ok(o) => o,
            Err(e) => {
                // 失败也要落日志（此前 EarlyReturn `?` 会绕过日志，导致排查无据）
                log_chat_error(
                    &state,
                    usage_session.as_ref().map(|(sid, _)| *sid),
                    model_label,
                    &system,
                    &user_prompt_base,
                    &e,
                )
                .await;
                return Err(e);
            }
        };
        usage = (out.prompt_tokens, out.cached_tokens, out.completion_tokens);
        reply = out.content;
    } else {
        model_label = pipeline::DETECT_MODEL_LABEL;
        let tools = chat_tools(usage_session.is_some());
        for _round in 0..5 {
            let channel_ref = &on_event;
            let out = match pipeline::chat_messages_stream(
                &client,
                &key,
                pipeline::DETECT_MODEL_ID,
                &system,
                &messages,
                Some(&tools),
                |piece| {
                    let _ = channel_ref.send(serde_json::json!({"type":"delta","text": piece}));
                },
            )
            .await
            {
                Ok(o) => o,
                Err(e) => {
                    let up = if tool_trace.is_empty() {
                        user_prompt_base.clone()
                    } else {
                        format!("{user_prompt_base}\n[工具轨迹]\n{}", tool_trace.trim_end())
                    };
                    log_chat_error(
                        &state,
                        usage_session.as_ref().map(|(sid, _)| *sid),
                        model_label,
                        &system,
                        &up,
                        &e,
                    )
                    .await;
                    return Err(e);
                }
            };
            usage.0 += out.prompt_tokens;
            usage.1 += out.cached_tokens;
            usage.2 += out.completion_tokens;
            if out.tool_calls.is_empty() {
                reply = out.content;
                break;
            }
            // 记录 assistant 的工具调用消息
            messages.push(serde_json::json!({
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": out.tool_calls,
            }));
            for tc in &out.tool_calls {
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let args: serde_json::Value =
                    serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                let _ = on_event.send(serde_json::json!({"type":"status","stage":"tool","tool": name}));
                tool_trace.push_str(&format!("--- 调用工具 {name}({args_str})\n"));
                let result = chat_dispatch_tool(
                    &state,
                    &on_event,
                    &name,
                    &args,
                    usage_session.as_ref(),
                )
                .await;
                let result_text = match &result {
                    Ok((s, approved)) => {
                        if name == "save_to_note" && *approved {
                            saved = true;
                        }
                        s.clone()
                    }
                    Err(e) => format!("{e}"),
                };
                tool_trace.push_str(&format!("→ {}\n", trunc_str(&result_text, 400)));
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tc["id"].as_str().unwrap_or(""),
                    "content": result_text
                }));
            }
        }
        if reply.is_empty() {
            reply = "这个问题信息量有点大，我没能在限定轮次里查完，换个问法试试？".to_string();
            let _ = on_event.send(serde_json::json!({"type":"delta","text": reply}));
        }
    }

    // 日志（所有范围）；用量仅 session 范围记账（usage_records 外键约束到课堂）
    let _ = db::insert_llm_log(
        &state.pool,
        db::LlmLogInsert {
            session_id: usage_session.as_ref().map(|(sid, _)| *sid),
            stage: "chat".into(),
            model: model_label.into(),
            system_prompt: Some(system),
            user_prompt: Some(if tool_trace.is_empty() {
                user_prompt_base.clone()
            } else {
                format!("{user_prompt_base}\n[工具轨迹]\n{}", tool_trace.trim_end())
            }),
            response: Some(reply.clone()),
            ok: true,
            error: None,
            input_tokens: usage.0,
            cached_tokens: usage.1,
            output_tokens: usage.2,
        },
    )
    .await;

    if let Some((sid, tier)) = &usage_session {
        let rec = pipeline::UsageInsert {
            session_id: *sid,
            ts: chrono::Local::now().to_rfc3339(),
            stage: "chat".into(),
            model: model_label.into(),
            model_tier: tier.clone(),
            input_tokens: usage.0,
            cached_tokens: usage.1,
            output_tokens: usage.2,
            audio_seconds: 0,
            amount: pipeline::token_amount(model_label, usage.0, usage.1, usage.2),
        };
        db::batch_create_usage(&state.pool, &[rec]).await?;
        db::refresh_session_cost(&state.pool, *sid).await?;
    }

    Ok(serde_json::json!({
        "reply": reply,
        "savedToNote": saved,
    }))
}

/// 前端答复「是否记入笔记」确认卡：唤醒聊天流程里等待的 oneshot。
#[tauri::command(rename_all = "camelCase")]
pub async fn respond_note_save(
    input: NoteSaveResponseDto,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let sender = {
        let Ok(mut map) = state.note_requests.lock() else {
            return Ok(false);
        };
        map.remove(&input.request_id)
    };
    if let Some(tx) = sender {
        let _ = tx.send(input.approved);
    }
    Ok(true)
}

/// 同桌小轩可用工具定义。session 范围额外提供 save_to_note。
fn chat_tools(with_save: bool) -> serde_json::Value {
    let mut defs = vec![
        serde_json::json!({"type":"function","function":{"name":"list_courses","description":"列出学生的全部课程（名称、代码、地点、起止周）","parameters":{"type":"object","properties":{}}}}),
        serde_json::json!({"type":"function","function":{"name":"list_sessions","description":"列出课堂记录（可按课程过滤），含标题、时间、状态、时长、笔记","parameters":{"type":"object","properties":{"course_id":{"type":"integer","description":"课程 id，可选"}}}}}),
        serde_json::json!({"type":"function","function":{"name":"get_session_detail","description":"获取某一节课的详情：状态、时长、行动项列表、笔记全文","parameters":{"type":"object","properties":{"session_id":{"type":"integer"}},"required":["session_id"]}}}),
        serde_json::json!({"type":"function","function":{"name":"get_transcript","description":"获取某一节课的双语字幕（按时间顺序，默认最近40行，可用 limit 调整最多200）","parameters":{"type":"object","properties":{"session_id":{"type":"integer"},"limit":{"type":"integer"}},"required":["session_id"]}}}),
        serde_json::json!({"type":"function","function":{"name":"search_transcript","description":"按关键词搜索全部（或某节课的）课堂字幕，定位某个知识点在哪节课讲过","parameters":{"type":"object","properties":{"keyword":{"type":"string"},"session_id":{"type":"integer","description":"可选，限定某节课"}},"required":["keyword"]}}}),
    ];
    if with_save {
        defs.push(serde_json::json!({"type":"function","function":{"name":"save_to_note","description":"把本次问答中对课后复习有价值的内容沉淀进课堂笔记（知识点澄清、题目解答、老师补充、板书讲解等）。简单闲聊不要调用","parameters":{"type":"object","properties":{"question":{"type":"string","description":"学生的问题"},"summary":{"type":"string","description":"要沉淀的答案要点"}},"required":["summary"]}}}));
    }
    serde_json::Value::Array(defs)
}

/// 同桌小轩工具分发。Err 的提示信息也会作为工具结果回给模型。
/// 返回 (工具结果文本, 是否经用户批准写入了笔记)。
async fn chat_dispatch_tool(
    state: &State<'_, AppState>,
    channel: &tauri::ipc::Channel<serde_json::Value>,
    name: &str,
    args: &serde_json::Value,
    session: Option<&(i64, String)>,
) -> AppResult<(String, bool)> {
    /// 给非老师发言加【同学】前缀。
    fn mark(speaker: &str, text: &str) -> String {
        if speaker == "other" {
            format!("【同学】{text}")
        } else {
            text.to_string()
        }
    }

    match name {
        "list_courses" => {
            let courses = db::list_courses(&state.pool).await?;
            Ok((serde_json::to_string_pretty(&courses).unwrap_or_default(), false))
        }
        "list_sessions" => {
            let cid = args["course_id"].as_i64();
            let sessions = db::list_sessions(&state.pool, cid).await?;
            Ok((serde_json::to_string_pretty(&sessions).unwrap_or_default(), false))
        }
        "get_session_detail" => {
            let sid = args["session_id"].as_i64().unwrap_or(0);
            let sess = db::get_session(&state.pool, sid)
                .await?
                .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
            let items = db::list_action_items(&state.pool, sid).await?;
            Ok((
                serde_json::json!({
                    "session": sess,
                    "actionItems": items
                        .iter()
                        .map(|it| serde_json::json!({"content": it.content, "urgency": it.urgency, "ts": it.ts}))
                        .collect::<Vec<_>>(),
                    "note": sess.note.as_deref().map(|n| trunc_str(n, 2000)).unwrap_or_default(),
                })
                .to_string(),
                false,
            ))
        }
        "get_transcript" => {
            let sid = args["session_id"].as_i64().unwrap_or(0);
            let limit = args["limit"].as_i64().unwrap_or(40).clamp(1, 200) as usize;
            let lines = db::list_transcript_lines(&state.pool, sid).await?;
            if lines.is_empty() {
                return Ok(("（该课堂暂无字幕）".into(), false));
            }
            let has_other = lines.iter().any(|l| l.speaker == "other");
            let picked: Vec<String> = lines
                .iter()
                .rev()
                .take(limit)
                .rev()
                .map(|l| {
                    format!(
                        "[{}] {} / {}",
                        l.ts,
                        mark(&l.speaker, &l.en),
                        l.zh.as_deref()
                            .map(|z| mark(&l.speaker, z))
                            .unwrap_or_else(|| "（未翻译）".to_string())
                    )
                })
                .collect();
            let mut out = String::new();
            if has_other {
                out.push_str("（带【同学】前缀的内容不是老师讲的，是课堂里其他同学的发言）\n");
            }
            out.push_str(&picked.join("\n"));
            Ok((out, false))
        }
        "search_transcript" => {
            let kw = args["keyword"].as_str().unwrap_or("").trim().to_string();
            if kw.is_empty() {
                return Err(crate::error::AppError::new("关键词不能为空"));
            }
            let hits =
                db::search_transcript(&state.pool, &kw, args["session_id"].as_i64()).await?;
            if hits.is_empty() {
                return Ok(("（没有匹配的字幕）".into(), false));
            }
            let has_other = hits.iter().any(|h| h.speaker == "other");
            let mut out = String::new();
            if has_other {
                out.push_str("（带【同学】前缀的内容不是老师讲的，是课堂里其他同学的发言）\n");
            }
            out.push_str(
                &hits
                    .iter()
                    .map(|h| {
                        format!(
                            "《{}》[{}] {} / {}",
                            h.session_title,
                            h.ts,
                            mark(&h.speaker, &h.en),
                            h.zh.as_deref().map(|z| mark(&h.speaker, z)).unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            Ok((out, false))
        }
        "save_to_note" => {
            let Some((sid, _tier)) = session else {
                return Err(crate::error::AppError::new(
                    "当前页面不在课堂内，无法记入笔记",
                ));
            };
            let sid = *sid;
            let summary = args["summary"].as_str().unwrap_or("").trim();
            let question = args["question"].as_str().unwrap_or("").trim();
            if summary.is_empty() {
                return Err(crate::error::AppError::new("缺少要沉淀的内容"));
            }
            // 组织确认卡内容与标题（批准后原样追加进笔记）
            let ts = chrono::Local::now().format("%H:%M").to_string();
            let mut block = String::from("## 课堂对话记录\n");
            if !question.is_empty() {
                block.push_str(&format!("**[{}] 问：** {}\n\n", ts, trunc_str(question, 300)));
            }
            block.push_str(&format!("**答：** {}", trunc_str(summary, 800)));
            let title = if question.is_empty() {
                "补充课堂记录".to_string()
            } else {
                trunc_str(question, 24)
            };

            // 推送确认卡，阻塞等待用户批准（最多 10 分钟）
            let req_id = format!(
                "nr_{}",
                chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
            );
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
            {
                let Ok(mut map) = state.note_requests.lock() else {
                    return Err(crate::error::AppError::new("确认通道不可用"));
                };
                map.insert(req_id.clone(), tx);
            }
            let _ = channel.send(serde_json::json!({
                "type": "noteRequest",
                "requestId": req_id,
                "sessionId": sid,
                "title": title,
                "content": block,
            }));
            let approved = match tokio::time::timeout(std::time::Duration::from_secs(600), rx).await {
                Ok(Ok(v)) => v,
                _ => false,
            };
            {
                if let Ok(mut map) = state.note_requests.lock() {
                    map.remove(&req_id);
                }
            }
            if !approved {
                return Ok(("学生选择不把这条内容记入笔记，请继续正常对话。".into(), false));
            }
            let sess = db::get_session(&state.pool, sid)
                .await?
                .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
            let mut note = sess.note.clone().unwrap_or_default();
            if !note.trim().is_empty() {
                note.push_str("\n\n");
            }
            note.push_str(&block);
            db::update_session_note(&state.pool, sid, &note).await?;
            Ok(("已记入笔记，继续正常对话即可，不要重复刚才的记录内容。".into(), true))
        }
        _ => Err(crate::error::AppError::new("未知工具")),
    }
}

fn trunc_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

/// 聊天链路模型调用失败时也落 llm_logs（stage=chat, ok=false），便于事后排查。
/// 此前失败会经 `?` 提前返回、绕过日志，导致「调用失败」无从复盘。
async fn log_chat_error(
    state: &AppState,
    session_id: Option<i64>,
    model: &str,
    system: &str,
    user_prompt: &str,
    err: &crate::error::AppError,
) {
    let _ = db::insert_llm_log(
        &state.pool,
        db::LlmLogInsert {
            session_id,
            stage: "chat".into(),
            model: model.into(),
            system_prompt: Some(system.to_string()),
            user_prompt: Some(user_prompt.to_string()),
            response: None,
            ok: false,
            error: Some(err.to_string()),
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
        },
    )
    .await;
}

/// 课堂整理稿（文稿整理结果，按覆盖范围与段内顺序）。
#[tauri::command(rename_all = "camelCase")]
pub async fn list_tidy_passages(
    session_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<TidyPassageDto>> {
    db::list_tidy_passages(&state.pool, session_id).await
}

/* ------------------------------- 附件 ---------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub async fn upload_material(
    input: UploadMaterialPayload,
    state: State<'_, AppState>,
) -> AppResult<UploadResult> {
    // 兼容前端传整段 dataURL（data:...;base64,xxx）的情况，剥离前缀后解码
    let raw = input.data_base64.trim();
    let raw = match raw.split_once(";base64,") {
        Some((_, b)) => b.trim(),
        None => raw,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw)
        .map_err(|e| crate::error::AppError::new(format!("base64 解码失败：{e}")))?;

    let safe_name = input
        .file_name
        .replace(|c: char| c == '/' || c == '\\' || c == ':', "_");

    // 分类：显式选择优先；缺省时 pptx/ppt/pdf 视作课件，其他为其他附件
    let ext = PathBuf::from(&safe_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let category: String = match input.category.as_deref() {
        Some(c @ ("slides" | "homework" | "other")) => c.to_string(),
        _ => {
            if matches!(ext.as_str(), "pptx" | "ppt" | "pdf") {
                "slides".to_string()
            } else {
                "other".to_string()
            }
        }
    };

    // 落盘：课程/课堂/附件
    let dir = crate::storage::attach_dir_for(&state.data_dir, input.course_id, input.session_id);
    std::fs::create_dir_all(&dir)?;
    let path = crate::storage::unique_path(&dir, &safe_name);
    std::fs::write(&path, &bytes)?;
    let saved_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&safe_name)
        .to_string();

    // 仅课件 PPT/PDF 做内容解析；解析失败也不影响上传（附件允许任意类型）
    let (page_count, keywords, file_type, confidence) =
        if category == "slides" && matches!(ext.as_str(), "pptx" | "ppt" | "pdf") {
            match ppt::parse_material(&path) {
                Ok(p) => (
                    p.page_count as i64,
                    p.keywords,
                    p.file_type,
                    p.confidence,
                ),
                Err(e) => {
                    eprintln!("[upload] 附件解析失败（不影响上传）：{e}");
                    (0, vec![], ext, "manual".into())
                }
            }
        } else {
            (0, vec![], ext, "manual".into())
        };

    // 入库
    let material = db::create_material(
        &state.pool,
        input.course_id,
        input.session_id,
        &saved_name,
        &file_type,
        bytes.len() as i64,
        path.to_string_lossy().as_ref(),
        &confidence,
        &category,
    )
    .await?;

    Ok(UploadResult {
        material,
        page_count,
        keywords,
        confidence,
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_materials(
    course_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<MaterialDto>> {
    db::list_materials(&state.pool, course_id).await
}

/// 用系统默认程序打开附件。
#[tauri::command(rename_all = "camelCase")]
pub async fn open_material(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let material = db::get_material(&state.pool, id)
        .await?
        .ok_or_else(|| crate::error::AppError::new("附件不存在"))?;
    let path = PathBuf::from(&material.file_path);
    if !path.exists() {
        return Err(crate::error::AppError::new("附件文件已丢失"));
    }
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| crate::error::AppError::new(format!("打开失败：{e}")))?;
    Ok(material.file_path)
}

/// 删除附件（记录 + 文件）。
#[tauri::command(rename_all = "camelCase")]
pub async fn delete_material(
    id: i64,
    state: State<'_, AppState>,
) -> AppResult<()> {
    if let Some(m) = db::get_material(&state.pool, id).await? {
        let _ = std::fs::remove_file(PathBuf::from(&m.file_path));
        db::delete_material(&state.pool, id).await?;
    }
    Ok(())
}

/* ------------------------------- 用量/设置 -------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub async fn list_usage(state: State<'_, AppState>) -> AppResult<Vec<UsageDto>> {
    db::list_usage(&state.pool).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_settings(state: State<'_, AppState>) -> AppResult<SettingsDto> {
    db::get_settings(&state.pool).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn save_settings(
    input: SettingsPatchDto,
    state: State<'_, AppState>,
) -> AppResult<SettingsDto> {
    if let Some(budget) = input.monthly_budget {
        if budget < 0.0 {
            return Err(crate::error::AppError::new("月度预算不能为负"));
        }
    }
    db::save_settings(&state.pool, input).await
}

/* ------------------------------- 音频采集 -------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub fn start_capture(
    session_id: i64,
    mode: String,
    target_pid: Option<u32>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let sess = tauri::async_runtime::block_on(db::get_session(&state.pool, session_id))?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
    let dir = crate::storage::audio_dir(&state.data_dir, sess.course_id, session_id);
    std::fs::create_dir_all(&dir)?;
    let mut guard = state.capture.lock().map_err(|e| {
        crate::error::AppError::new(format!("采集线程锁获取失败：{e}"))
    })?;
    // 同一课堂重复进入页面：复用既有采集线程，绝不能替换 guard 造成孤儿线程
    if let Some(existing) = guard.as_ref() {
        if existing.is_running() {
            if existing.config.session_id != session_id {
                return Err(crate::error::AppError::new("其他课堂正在录制，请先结束"));
            }
            // 采集目标变化（网课切换应用 / 系统声音↔应用声音）：重启采集线程。
            // 以「实际生效目标」为准：按应用采集若已自动回退系统声音，重新选该应用也要重启。
            let current_effective = existing
                .metrics
                .lock()
                .ok()
                .map(|m| m.effective_pid)
                .unwrap_or(existing.config.target_pid);
            if current_effective != target_pid {
                existing.stop();
                drop(guard);
                std::thread::sleep(std::time::Duration::from_millis(400));
                let relay_tx = install_audio_router(&state);
                let cfg = capture::CaptureConfig {
                    session_id,
                    chunks_dir: dir,
                    mode,
                    target_pid,
                    mic_gain: Arc::clone(&state.mic_gain),
                    live_tx: Some(relay_tx),
                };
                let mut guard = state.capture.lock().map_err(|e| {
                    crate::error::AppError::new(format!("采集线程锁获取失败：{e}"))
                })?;
                let mut thread = capture::CaptureThread::new(cfg);
                let started = thread.start()?;
                *guard = Some(thread);
                attach_default_engine(&app, &state, session_id);
                return Ok(started);
            }
            return Ok(true);
        }
    }
    // 音频中转：采集线程固定发往 relay_tx，路由任务按 engine_slot 转发给当前引擎，
    // 这样录制中热切换同传/经济模式无需重启采集
    let relay_tx = install_audio_router(&state);
    let cfg = capture::CaptureConfig {
        session_id,
        chunks_dir: dir,
        mode,
        target_pid,
        mic_gain: Arc::clone(&state.mic_gain),
        live_tx: Some(relay_tx),
    };
    let mut thread = capture::CaptureThread::new(cfg);
    let started = thread.start()?;
    *guard = Some(thread);
    drop(guard);
    // 默认引擎：有火山 Key 开 AST 同传；否则有硅基流动 Key 开经济转写
    attach_default_engine(&app, &state, session_id);
    Ok(started)
}

/// 建立「采集线程 → 当前引擎」的中转路由，返回采集线程持有的发送端。
/// 切换引擎只改 engine_slot 指向，通道本身不变。
fn install_audio_router(
    state: &State<'_, AppState>,
) -> tokio::sync::mpsc::UnboundedSender<Vec<i16>> {
    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    let slot: std::sync::Arc<std::sync::Mutex<Option<crate::EngineTarget>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    if let Ok(mut g) = state.engine_slot.lock() {
        *g = Some(slot.clone());
    }
    tauri::async_runtime::spawn(async move {
        while let Some(pcm) = relay_rx.recv().await {
            if let Ok(g) = slot.lock() {
                if let Some(tx) = g.as_ref() {
                    // 引擎已停时 send 失败即丢弃本批，不影响落盘
                    let _ = tx.send(pcm);
                }
            }
        }
    });
    relay_tx
}

/// 录制启动时按已配置密钥选择默认引擎（AST 优先，其次经济转写）。
fn attach_default_engine(app: &tauri::AppHandle, state: &State<'_, AppState>, session_id: i64) {
    let (volc_key, sf_key) = read_engine_keys(state);
    if volc_key.is_some() && spawn_ast_engine(app, state, session_id).is_ok() {
        return;
    }
    if sf_key.is_some() {
        let _ = spawn_cheap_engine(app, state, session_id);
    }
}

fn read_engine_keys(state: &State<'_, AppState>) -> (Option<String>, Option<String>) {
    let Ok(guard) = state.secrets.lock() else {
        return (None, None);
    };
    (
        guard
            .get(secrets::key::VOLC_API_KEY)
            .filter(|k| !k.trim().is_empty()),
        guard
            .get(secrets::key::SILICONFLOW_API_KEY)
            .filter(|k| !k.trim().is_empty()),
    )
}

/// 启动 AST 同传引擎（已在运行则复用），并把上行通道写入 engine_slot。
fn spawn_ast_engine(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
    session_id: i64,
) -> AppResult<()> {
    let (volc_key, sf_key) = read_engine_keys(state);
    let volc_key = volc_key
        .ok_or_else(|| crate::error::AppError::new("未配置火山引擎 API Key，无法开启同传"))?;
    let mut guard = state
        .ast
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("同传锁获取失败：{e}")))?;
    if let Some(h) = guard.as_ref() {
        if h.is_running() && h.session_id() == session_id {
            let tx = h.live_tx();
            drop(guard);
            set_engine_ast_target(state, tx)?;
            return Ok(());
        }
    }
    let settings = tauri::async_runtime::block_on(db::get_settings(&state.pool))?;
    let sess = tauri::async_runtime::block_on(db::get_session(&state.pool, session_id))?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
    let handle = ast::AstHandle::spawn(
        app.clone(),
        state.pool.clone(),
        ast::AstConfig {
            session_id,
            api_key: volc_key,
            source_language: settings.speak_language,
            target_language: settings.daily_language,
            start_at: sess.start_at,
            model_tier: sess.model_tier,
            sf_key,
        },
    );
    let tx = handle.live_tx();
    *guard = Some(handle);
    drop(guard);
    set_engine_ast_target(state, tx)?;
    Ok(())
}

/// 启动经济分块转写引擎（需要硅基流动 Key）；经济模式音频只需落盘，清空 AST 上行槽。
fn spawn_cheap_engine(
    app: &tauri::AppHandle,
    state: &State<'_, AppState>,
    session_id: i64,
) -> AppResult<()> {
    let (_, sf_key) = read_engine_keys(state);
    let sf_key = sf_key
        .ok_or_else(|| crate::error::AppError::new("未配置硅基流动 API Key，无法开启经济转写"))?;
    let mut guard = state
        .cheap
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("经济引擎锁获取失败：{e}")))?;
    if let Some(h) = guard.as_ref() {
        if h.is_running() && h.session_id() == session_id {
            drop(guard);
            clear_engine_target(state);
            return Ok(());
        }
    }
    let settings = tauri::async_runtime::block_on(db::get_settings(&state.pool))?;
    let sess = tauri::async_runtime::block_on(db::get_session(&state.pool, session_id))?
        .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
    let chunks_dir = crate::storage::audio_dir(&state.data_dir, sess.course_id, session_id);
    std::fs::create_dir_all(&chunks_dir)?;
    let handle = cheap::CheapHandle::spawn(
        app.clone(),
        state.pool.clone(),
        cheap::CheapConfig {
            session_id,
            chunks_dir,
            sf_key,
            source_language: settings.speak_language,
            target_language: settings.daily_language,
            start_at: sess.start_at,
            model_tier: sess.model_tier,
        },
    );
    *guard = Some(handle);
    drop(guard);
    clear_engine_target(state);
    Ok(())
}

fn set_engine_ast_target(
    state: &State<'_, AppState>,
    tx: tokio::sync::mpsc::UnboundedSender<Vec<i16>>,
) -> AppResult<()> {
    let g = state
        .engine_slot
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("引擎槽锁获取失败：{e}")))?;
    if let Some(inner) = g.as_ref() {
        if let Ok(mut ig) = inner.lock() {
            *ig = Some(tx);
        }
    }
    Ok(())
}

fn clear_engine_target(state: &State<'_, AppState>) {
    if let Ok(g) = state.engine_slot.lock() {
        if let Some(inner) = g.as_ref() {
            if let Ok(mut ig) = inner.lock() {
                *ig = None;
            }
        }
    }
}

/// 停止指定课堂的 AST 引擎，返回它是否本在运行（用于决定是否等待字幕 flush）。
fn stop_ast_for(state: &State<'_, AppState>, session_id: i64) -> bool {
    let Ok(guard) = state.ast.lock() else {
        return false;
    };
    match guard.as_ref() {
        Some(h) if h.session_id() == session_id => {
            h.stop();
            h.is_running()
        }
        Some(h) => {
            h.stop();
            false
        }
        None => false,
    }
}

/// 停止指定课堂的经济引擎。
fn stop_cheap_for(state: &State<'_, AppState>, session_id: i64) -> bool {
    let Ok(guard) = state.cheap.lock() else {
        return false;
    };
    match guard.as_ref() {
        Some(h) if h.session_id() == session_id => {
            h.stop();
            h.is_running()
        }
        Some(h) => {
            h.stop();
            false
        }
        None => false,
    }
}

/// 当前录制课堂的引擎状态："ast" | "cheap" | "off"。
fn current_engine(state: &State<'_, AppState>, session_id: Option<i64>) -> String {
    let Some(sid) = session_id else {
        return "off".into();
    };
    if let Ok(g) = state.ast.lock() {
        if g.as_ref().is_some_and(|h| h.is_running() && h.session_id() == sid) {
            return "ast".into();
        }
    }
    if let Ok(g) = state.cheap.lock() {
        if g.as_ref().is_some_and(|h| h.is_running() && h.session_id() == sid) {
            return "cheap".into();
        }
    }
    "off".into()
}

/// 录制中热切换字幕引擎：engine = "ast"（同传）| "cheap"（每 10 秒经济转写）| "off"。
/// 返回切换后实际生效的引擎名。
///
/// 必须是同步命令：spawn_ast_engine / spawn_cheap_engine 内部用
/// tauri::async_runtime::block_on 读库，若本命令是 async（跑在 tokio 运行时
/// 工作线程上），block_on 会 panic「Cannot start a runtime from within a runtime」，
/// 叠加 release 的 panic=abort 直接整应用闪退（0xc0000409，实测）。
#[tauri::command(rename_all = "camelCase")]
pub fn set_live_engine(
    session_id: i64,
    engine: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<String> {
    {
        let g = state
            .capture
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("采集线程锁获取失败：{e}")))?;
        let active = g
            .as_ref()
            .is_some_and(|t| t.is_running() && t.config.session_id == session_id);
        if !active {
            return Err(crate::error::AppError::new("该课堂未在录制，无法切换字幕模式"));
        }
    }
    match engine.as_str() {
        "ast" => {
            stop_cheap_for(&state, session_id);
            spawn_ast_engine(&app, &state, session_id)?;
        }
        "cheap" => {
            stop_ast_for(&state, session_id);
            clear_engine_target(&state);
            spawn_cheap_engine(&app, &state, session_id)?;
        }
        "off" => {
            stop_ast_for(&state, session_id);
            stop_cheap_for(&state, session_id);
            clear_engine_target(&state);
        }
        other => return Err(crate::error::AppError::new(format!("未知引擎：{other}"))),
    }
    Ok(current_engine(&state, Some(session_id)))
}

#[tauri::command]
pub fn stop_capture(state: State<'_, AppState>) -> AppResult<()> {
    let mut guard = state.capture.lock().map_err(|e| {
        crate::error::AppError::new(format!("采集线程锁获取失败：{e}"))
    })?;
    if let Some(t) = guard.as_mut() {
        t.stop();
    }
    let sid = guard.as_ref().map(|t| t.config.session_id);
    drop(guard);
    // 录制结束，两种字幕引擎一并停止（重进页面时由 attach_default_engine 重建）
    if let Some(sid) = sid {
        stop_ast_for(&state, sid);
        stop_cheap_for(&state, sid);
    }
    clear_engine_target(&state);
    release_class_mute_impl(&state);
    Ok(())
}

#[tauri::command]
pub fn capture_status(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let guard = state.capture.lock().map_err(|e| {
        crate::error::AppError::new(format!("采集线程锁获取失败：{e}"))
    })?;
    let running = guard.as_ref().map_or(false, |t| t.is_running());
    let session_id = guard.as_ref().map(|t| t.config.session_id);
    let (device_name, rms, silence_warned, chunks, effective_pid, capture_note) = guard
        .as_ref()
        .map(|t| {
            let m = t.metrics.lock().unwrap();
            (
                m.device_name.clone(),
                (m.rms * 100.0).round() / 100.0,
                m.silence_warned,
                m.chunks,
                m.effective_pid,
                m.note.clone(),
            )
        })
        .unwrap_or_default();
    let engine = current_engine(&state, session_id);
    Ok(serde_json::json!({
        "running": running,
        "sessionId": session_id,
        "engine": engine,
        "deviceName": device_name,
        "rms": rms,
        "silenceWarned": silence_warned,
        "chunks": chunks,
        // targetPid 报告“实际生效”的目标：按应用采集回退系统声音后会变为 null，
        // 前端据此把应用选择器同步回「系统全部声音」
        "targetPid": effective_pid,
        "captureNote": capture_note,
        "mutedPid": state.muted_pid.lock().ok().and_then(|g| *g),
    }))
}

/* --------------------------- 按应用采集 / 接管课堂 --------------------------- */

/// 列出当前默认输出设备上正在播放声音的应用（网课模式「按应用采集」选择器）。
#[tauri::command]
pub fn list_audio_apps() -> AppResult<Vec<crate::win_audio::AudioApp>> {
    crate::win_audio::list_audio_apps()
}

/// 接管课堂：端点级静音整机外放（系统/进程回环录音不受影响）。
/// 录音仍按所选目标进程过滤；静音是全局端点状态，无需持续轮询。
#[tauri::command(rename_all = "camelCase")]
pub fn take_over_class(pid: u32, state: State<'_, AppState>) -> AppResult<usize> {
    // 若已在接管其他应用，先恢复端点声音
    crate::release_takeover(&state);
    let mute = crate::win_audio::EndpointMute::engage()?;
    *state
        .endpoint_mute
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("接管状态锁获取失败：{e}")))? = Some(mute);
    *state
        .muted_pid
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("接管状态锁获取失败：{e}")))? = Some(pid);
    Ok(1)
}

/// 取消接管：恢复端点声音。
#[tauri::command]
pub fn release_class(state: State<'_, AppState>) -> AppResult<()> {
    release_class_mute_impl(&state);
    Ok(())
}

/// 内部工具：若处于接管状态则恢复端点声音（停止录制/归档时自动调用）。
fn release_class_mute_impl(state: &State<'_, AppState>) {
    crate::release_takeover(state);
}

/// 实时调整麦克风软件增益（同时持久化到设置）。
#[tauri::command(rename_all = "camelCase")]
pub async fn set_mic_gain(gain: f64, state: State<'_, AppState>) -> AppResult<f64> {
    let g = gain.clamp(0.5, 4.0);
    state
        .mic_gain
        .store((g as f32).to_bits(), std::sync::atomic::Ordering::SeqCst);
    db::set_mic_gain(&state.pool, g).await?;
    Ok(g)
}

/* ------------------------------- 密钥配置 -------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub fn get_secret_config(state: State<'_, AppState>) -> AppResult<SecretConfigDto> {
    let guard = state
        .secrets
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
    Ok(SecretConfigDto::from_store(&guard))
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_secret_config(
    patch: SecretConfigPatch,
    state: State<'_, AppState>,
) -> AppResult<SecretConfigDto> {
    let mut guard = state
        .secrets
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
    guard.apply_patch(patch)?;
    Ok(SecretConfigDto::from_store(&guard))
}

/// 用已保存的 API Key 做一次最小调用，验证连通性。
#[tauri::command(rename_all = "camelCase")]
pub async fn test_siliconflow(state: State<'_, AppState>) -> AppResult<String> {
    let key = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        guard.get(secrets::key::SILICONFLOW_API_KEY)
    }
    .ok_or_else(|| crate::error::AppError::new("未配置硅基流动 API Key"))?;
    pipeline::test_api_key(&key).await
}

/// 用已保存的火山引擎 API Key 建连 + StartSession，验证 AST 2.0 同传可用性。
#[tauri::command(rename_all = "camelCase")]
pub async fn test_volc(state: State<'_, AppState>) -> AppResult<String> {
    let key = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        guard.get(secrets::key::VOLC_API_KEY)
    }
    .ok_or_else(|| crate::error::AppError::new("未配置火山引擎 API Key"))?;
    ast::probe(&key).await
}

/// 发送一封测试邮件（QQ 邮箱）。
#[tauri::command(rename_all = "camelCase")]
pub async fn test_mail(state: State<'_, AppState>) -> AppResult<()> {
    let cfg = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        mail::MailConfig::from_store(&guard)
    }
    .ok_or_else(|| {
        crate::error::AppError::new("QQ 邮箱未配置：请填写邮箱地址与 SMTP 授权码")
    })?;

    tauri::async_runtime::spawn_blocking(move || {
        mail::send_with_retry(
            &cfg,
            "同桌小轩 · 测试邮件",
            "如果收到这封邮件，说明邮箱提醒已配置成功。\n\n—— 同桌小轩",
        )
    })
    .await
    .map_err(|e| crate::error::AppError::new(format!("邮件线程失败：{e}")))??;
    Ok(())
}

/// 发送行动项提醒邮件（不在位时的兜底通道，PRD 5.6）。
/// 未配置邮箱时静默跳过，不影响弹窗主通道。
#[tauri::command(rename_all = "camelCase")]
pub async fn send_reminder_email(
    subject: String,
    body: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let cfg = {
        let guard = state
            .secrets
            .lock()
            .map_err(|e| crate::error::AppError::new(format!("密钥仓库锁获取失败：{e}")))?;
        mail::MailConfig::from_store(&guard)
    };
    let Some(cfg) = cfg else {
        return Ok(());
    };
    tauri::async_runtime::spawn_blocking(move || mail::send_with_retry(&cfg, &subject, &body))
        .await
        .map_err(|e| crate::error::AppError::new(format!("邮件线程失败：{e}")))??;
    Ok(())
}

/* ------------------------------- 在位检测 -------------------------------- */

#[tauri::command(rename_all = "camelCase")]
pub fn start_presence(
    session_id: i64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let (interval, camera_enabled) = tauri::async_runtime::block_on(db::get_settings(&state.pool))
        .map(|s| (s.presence_interval_sec.max(0), s.presence_camera_enabled))
        .unwrap_or((10, true));

    let mut guard = state
        .presence
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("在位检测锁获取失败：{e}")))?;
    if let Some(t) = guard.as_ref() {
        if t.is_running() {
            return Ok(false);
        }
    }
    // 周期 <= 0 视为关闭在位检测
    if interval == 0 {
        return Ok(false);
    }
    let thread = presence::PresenceThread::start(
        app,
        state.pool.clone(),
        session_id,
        interval as u64,
        camera_enabled,
    );
    *guard = Some(thread);
    Ok(true)
}

#[tauri::command]
pub fn stop_presence(state: State<'_, AppState>) -> AppResult<()> {
    let guard = state
        .presence
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("在位检测锁获取失败：{e}")))?;
    if let Some(t) = guard.as_ref() {
        t.stop();
    }
    Ok(())
}

#[tauri::command]
pub fn presence_status(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let guard = state
        .presence
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("在位检测锁获取失败：{e}")))?;
    let running = guard.as_ref().map_or(false, |t| t.is_running());
    let current = guard.as_ref().map(|t| t.state().as_str().to_string());
    let session_id = guard.as_ref().map(|t| t.session_id());
    let detail = guard.as_ref().and_then(|t| t.detail());
    Ok(serde_json::json!({
        "running": running,
        "state": current,
        "sessionId": session_id,
        "detail": detail,
    }))
}

/// 测试摄像头在位检测是否可用（设置页「测试摄像头」按钮，只探测一次）。
#[tauri::command]
pub async fn probe_camera() -> AppResult<serde_json::Value> {
    let signal = tauri::async_runtime::spawn_blocking(crate::face::probe_once)
        .await
        .map_err(|e| crate::error::AppError::new(format!("探测线程失败：{e}")))?;
    Ok(match signal {
        Ok(crate::face::FaceSignal::Present) => serde_json::json!({
            "ok": true, "faces": true, "detail": null
        }),
        Ok(crate::face::FaceSignal::Absent) => serde_json::json!({
            "ok": true, "faces": false,
            "detail": "摄像头可用，但画面中未检测到人脸（请坐到镜头前再试）"
        }),
        Ok(crate::face::FaceSignal::Unavailable) => serde_json::json!({
            "ok": false, "faces": null, "detail": "摄像头画面读取失败，可能被其它程序占用"
        }),
        Err(e) => serde_json::json!({ "ok": false, "faces": null, "detail": e.0 }),
    })
}

/// 注入采样帧用于无摄像头环境验证：0=在位 1=缺席 2=不可用 3=恢复摄像头自动检测。
#[tauri::command(rename_all = "camelCase")]
pub fn simulate_presence(hint: u8, state: State<'_, AppState>) -> AppResult<()> {
    let guard = state
        .presence
        .lock()
        .map_err(|e| crate::error::AppError::new(format!("在位检测锁获取失败：{e}")))?;
    if let Some(t) = guard.as_ref() {
        t.set_hint(hint);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn list_presence_events(
    session_id: i64,
    state: State<'_, AppState>,
) -> AppResult<Vec<PresenceEventDto>> {
    db::list_presence_events(&state.pool, session_id).await
}

/* --------------------------- 提示词模板/调用日志 --------------------------- */

/// 全部可配置任务的模板（含默认值与覆盖状态）。
#[tauri::command]
pub async fn get_prompt_templates(
    state: State<'_, AppState>,
) -> AppResult<Vec<PromptTemplateDto>> {
    const TASKS: [(&str, &str); 6] = [
        ("tidy", "文稿整理"),
        ("translate_batch", "归档批量翻译"),
        ("detect", "实时行动项检测"),
        ("detect_batch", "归档行动项检测"),
        ("note", "笔记生成"),
        ("chat", "课堂对话"),
    ];
    let overrides: std::collections::HashMap<String, String> =
        db::list_prompt_overrides(&state.pool).await?.into_iter().collect();
    Ok(TASKS
        .iter()
        .map(|(task, label)| {
            let (template, is_default) = match overrides.get(*task) {
                Some(t) => (t.clone(), false),
                None => (pipeline::default_prompt(task).to_string(), true),
            };
            PromptTemplateDto {
                task: (*task).to_string(),
                label: (*label).to_string(),
                template,
                is_default,
            }
        })
        .collect())
}

/// 保存单个任务的模板覆盖。
#[tauri::command(rename_all = "camelCase")]
pub async fn save_prompt_template(
    task: String,
    template: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    if pipeline::default_prompt(&task).is_empty() {
        return Err(crate::error::AppError::new(format!("未知的任务标识：{task}")));
    }
    if template.trim().is_empty() {
        return Err(crate::error::AppError::new("模板不能为空"));
    }
    db::save_prompt_template(&state.pool, &task, &template).await
}

/// 全部恢复默认。
#[tauri::command]
pub async fn reset_prompt_templates(state: State<'_, AppState>) -> AppResult<()> {
    db::reset_prompt_templates(&state.pool).await
}

/// 最新的大模型调用日志（默认 200 条）。
#[tauri::command(rename_all = "camelCase")]
pub async fn list_llm_logs(
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> AppResult<Vec<LlmLogDto>> {
    db::list_llm_logs(&state.pool, limit.unwrap_or(200).clamp(1, 1000)).await
}

/// 清空调用日志。
#[tauri::command]
pub async fn clear_llm_logs(state: State<'_, AppState>) -> AppResult<()> {
    db::clear_llm_logs(&state.pool).await
}

/// 在文件资源管理器中打开某课堂的文件夹（内含 课堂录音/文稿/附件 三类）。
/// 不传 sessionId 时打开 courses 根目录。
#[tauri::command(rename_all = "camelCase")]
pub fn open_captures_dir(session_id: Option<i64>, state: State<'_, AppState>) -> AppResult<String> {
    let dir = match session_id {
        Some(id) => {
            let sess = tauri::async_runtime::block_on(db::get_session(&state.pool, id))?
                .ok_or_else(|| crate::error::AppError::new("课堂不存在"))?;
            crate::storage::session_dir(&state.data_dir, sess.course_id, id)
        }
        None => crate::storage::courses_root(&state.data_dir),
    };
    std::fs::create_dir_all(&dir)?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| crate::error::AppError::new(format!("打开文件夹失败：{e}")))?;
    Ok(dir.to_string_lossy().to_string())
}
