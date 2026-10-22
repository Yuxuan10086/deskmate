//! 与前端 TypeScript 接口一一对应的 DTO（camelCase）。

use serde::{Deserialize, Serialize};

/* --------------------------------- 课程 --------------------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDto {
    pub weekday: i64,
    pub start_minute: i64,
    pub end_minute: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseDto {
    pub id: i64,
    pub name: String,
    pub code: Option<String>,
    pub location: Option<String>,
    pub start_week: i64,
    pub end_week: i64,
    pub color: String,
    pub slots: Vec<SlotDto>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CourseInputDto {
    pub name: String,
    pub code: Option<String>,
    pub location: Option<String>,
    pub start_week: i64,
    pub end_week: i64,
    pub color: Option<String>,
    pub slots: Vec<SlotDto>,
}

/* --------------------------------- 课堂 --------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: i64,
    pub course_id: Option<i64>,
    pub title: String,
    pub lecture_no: i64,
    pub start_at: String,
    pub end_at: Option<String>,
    pub weekday: i64,
    pub week_no: i64,
    pub start_minute: i64,
    pub end_minute: i64,
    pub location: Option<String>,
    pub mode: String,
    pub model_tier: String,
    pub status: String,
    pub duration_sec: Option<i64>,
    pub cost_estimate: f64,
    pub note: Option<String>,
    /// 回收站：NULL=正常；Some(时间)=已软删除。
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInputDto {
    pub course_id: Option<i64>,
    pub mode: String,
    pub model_tier: String,
    pub weekday: i64,
    pub week_no: i64,
    pub start_minute: i64,
    pub end_minute: i64,
    pub location: Option<String>,
    pub start_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishPayloadDto {
    pub duration_sec: i64,
    pub cost_estimate: f64,
    #[serde(default)]
    pub note: Option<String>,
}

/* --------------------------------- 课件 --------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialDto {
    pub id: i64,
    pub course_id: i64,
    pub session_id: Option<i64>,
    pub file_name: String,
    pub file_type: String,
    pub size_bytes: i64,
    pub file_path: String,
    pub confidence: String,
    /// 附件分类：slides 课件PPT / homework 作业题目 / other 其他附件
    pub category: String,
    pub created_at: String,
}

/* ------------------------------- 用量明细 -------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDto {
    pub id: i64,
    pub session_id: Option<i64>,
    pub ts: String,
    pub stage: String,
    pub model: String,
    pub model_tier: String,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub audio_seconds: i64,
    pub amount: f64,
}

/* --------------------------------- 设置 --------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub semester_start: String,
    pub monthly_budget: f64,
    pub speak_language: String,
    pub daily_language: String,
    pub presence_interval_sec: i64,
    /// 是否用摄像头人脸检测判定在位（false 时仅支持人工注入验证）。
    pub presence_camera_enabled: bool,
    /// 课程表可见时间范围（24 小时制整点），默认 8–18。
    pub schedule_start_hour: i64,
    pub schedule_end_hour: i64,
    /// 麦克风软件增益（1.0=原始电平，范围 0.5–4.0）。
    pub mic_gain: f64,
    /// 同桌小轩人设 JSON（人设矩阵档位 + 自定义补充），NULL=全部默认。
    pub persona: Option<String>,
}

/// 设置保存时允许部分字段更新（与前端 Partial<AppSettings> 对齐）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatchDto {
    pub semester_start: Option<String>,
    pub monthly_budget: Option<f64>,
    pub speak_language: Option<String>,
    pub daily_language: Option<String>,
    pub presence_interval_sec: Option<i64>,
    pub presence_camera_enabled: Option<bool>,
    pub schedule_start_hour: Option<i64>,
    pub schedule_end_hour: Option<i64>,
    pub mic_gain: Option<f64>,
    pub persona: Option<String>,
}

/* ------------------------------- 文稿整理稿 -------------------------------- */

/// 一段整理后的文稿（合并过度分割的语句、去除口误后的版本）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TidyPassageDto {
    /// 覆盖的原始字幕 seq 起止（含端点）
    pub from_seq: i64,
    pub to_seq: i64,
    pub en: String,
    pub zh: Option<String>,
}

/// 字幕关键词搜索命中（同桌工具用）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSearchHitDto {
    pub session_id: i64,
    pub session_title: String,
    pub ts: String,
    pub en: String,
    pub zh: Option<String>,
    pub speaker: String,
}

/* ------------------------------- 在位事件 -------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceEventDto {
    pub ts: String,
    pub state: String,
}

/* ----------------------------- 实时字幕/行动项 ---------------------------- */

/// 一条双语音频字幕行（seq 为 10 秒分块序号）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLineDto {
    pub ts: String,
    pub seq: i64,
    pub en: String,
    pub zh: Option<String>,
    /// 说话人：'' 未标注；归档标注后 teacher / other
    pub speaker: String,
}

/// 实时/归档检测出的行动项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItemDto {
    pub id: i64,
    pub session_id: i64,
    pub ts: String,
    pub item_type: String,
    pub content: String,
    pub action_guide: Option<String>,
    pub urgency: Option<String>,
    pub ddl: Option<String>,
    pub added_to_tasks: bool,
}

/// 待办事项（行动项加入或手动创建）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoDto {
    pub id: i64,
    pub session_id: Option<i64>,
    pub action_item_id: Option<i64>,
    /// manual 手动 / ai 行动项加入 / chat 聊天记录
    pub source: String,
    pub title: String,
    pub detail: Option<String>,
    pub ddl: Option<String>,
    pub done: bool,
    pub created_at: String,
    pub completed_at: Option<String>,
}

/* ------------------------------- 课件上传 ------------------------------- */

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadMaterialPayload {
    pub file_name: String,
    /// base64 编码（不含 data: 前缀）；兼容前端历史字段名 base64
    #[serde(alias = "base64")]
    pub data_base64: String,
    pub course_id: i64,
    pub session_id: Option<i64>,
    /// 附件分类：slides / homework / other，缺省按扩展名推断（pptx/ppt/pdf→slides）
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub material: MaterialDto,
    pub page_count: i64,
    pub keywords: Vec<String>,
    pub confidence: String,
}

/* -------------------------------- 待办 ---------------------------------- */

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTodoPayload {
    pub title: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub ddl: Option<String>,
    #[serde(default)]
    pub session_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTodoPayload {
    pub title: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// 空字符串视为清除 DDL
    #[serde(default)]
    pub ddl: Option<String>,
}

/* --------------------------- 聊天笔记保存请求 --------------------------- */

/// 前端对「是否记入笔记」确认卡的答复。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSaveResponseDto {
    pub request_id: String,
    pub approved: bool,
}

/* ------------------------------ 提示词模板 ------------------------------ */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplateDto {
    /// 任务标识：translate / translate_batch / detect / detect_batch / note / chat
    pub task: String,
    /// 展示名
    pub label: String,
    /// 当前生效的模板（用户覆盖或默认）
    pub template: String,
    /// 是否仍为内置默认（未被用户覆盖）
    pub is_default: bool,
}

/* ----------------------------- 大模型调用日志 ----------------------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmLogDto {
    pub id: i64,
    pub ts: String,
    pub session_id: Option<i64>,
    pub stage: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub user_prompt: Option<String>,
    pub response: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
}
