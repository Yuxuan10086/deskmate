//! 管线模块：课堂归档后生成用量明细。
//!
//! - 已配置硅基流动 API Key 时走真实 HTTP Stage：ASR → 翻译 → 行动项检测 → 归档笔记；
//! - 未配置 Key、无录音分块或调用失败时降级为确定性 mock，保证离线可跑。
//! - 两条路径的计费口径完全一致（PRD 5.9：课堂花费 = 用量明细 amount 求和）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Client;
use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};
use crate::db;

/* ------------------------------- 真实 Stage ------------------------------ */

const SILICONFLOW_BASE: &str = "https://api.siliconflow.cn/v1";
/// 硅基流动语音转写接口的模型 id 与记录用的展示名。
const ASR_MODEL_ID: &str = "FunAudioLLM/SenseVoiceSmall";
const ASR_MODEL_LABEL: &str = "SenseVoiceSmall";
/// 翻译 / 检测 / 笔记模型（id 用于调用，label 用于用量记录）。
/// 注：GLM 系列在硅基流动的组织名为 zai-org（旧 THUDM 前缀已失效）。
pub(crate) const TRANSLATE_MODEL_ID: &str = "zai-org/GLM-4.5-Air";
pub(crate) const TRANSLATE_MODEL_LABEL: &str = "GLM-4.5-Air";
pub(crate) const DETECT_MODEL_ID: &str = "deepseek-ai/DeepSeek-V3.2";
pub(crate) const DETECT_MODEL_LABEL: &str = "DeepSeek-V3.2";
const NOTE_MODEL_ID: &str = "deepseek-ai/DeepSeek-V3.2";
const NOTE_MODEL_PRO_ID: &str = "Qwen/Qwen3.6-27B";
/// 课堂对话（含图片识别）使用的视觉模型。
pub(crate) const CHAT_VISION_MODEL_ID: &str = "zai-org/GLM-4.5V";
pub(crate) const CHAT_VISION_MODEL_LABEL: &str = "GLM-4.5V";

/// 单次归档最多转写的录音分块数（30 块 ≈ 5 分钟），用于控制请求数与花费。
const MAX_ASR_CHUNKS: usize = 30;
/// 单次模型调用的入参上限（字符），避免超出上下文窗口。
const MAX_INPUT_CHARS: usize = 12_000;

/* --------------------------- 提示词默认模板 --------------------------- */

/// 归档时编号批量翻译。支持占位符 {speak_language} {daily_language}。
pub const DEFAULT_PROMPT_TRANSLATE_BATCH: &str =
    "你是课堂同传助手。把编号的课堂原文逐条翻译为目标语言：保留「编号. 」前缀，每行一条，只输出译文，不要合并或省略行。";
/// 实时行动项检测（输入为最近若干条字幕 + 本课堂已识别过的行动项）。
/// 只报告老师【明确要求学生去做某件具体事】的指令；自问自答、设问、推测一律不报。
pub const DEFAULT_PROMPT_DETECT: &str =
    "你是课堂行动项检测器。只报告老师【明确要求学生去做某件具体事】的行动项，必须同时满足两个条件：\
     ① 句中有明确的指令/祈使（要学生「做…/提交…/完成…/打开…/扫码…/举手…/分组讨论后汇报…」等），\
     是老师在下达任务，而不是在讲课；② 能说清「要学生做的具体事」是什么。\
     原则：宁可少报，也不要猜。下面这些一律不报告：\
     ① 老师自问自答、以及老师提问后又自己接着回答（讲课为引出知识点而发问）；\
     ② 设问句/反问句/口头禅式提问（如「为什么不是 B」「大家觉得呢」「对不对」「明白了吗」「我们来看…」）——这是讲课不是任务；\
     ③ 只提到日期、数字、章节或页码，却没说要求学生做什么（如「4 月 2 日」「第六章」「第 3 题」）；\
     ④ 纯听课要求（听一下/看一下/记一下/想一想/注意/观察/举个例子）与普通讲授内容；\
     ⑤ 任何带「可能/似乎/也许/大概/应该是」的推测——拿不准就不报；\
     ⑥ 与学生行动无关的闲聊、设备调试、考勤与流程说明。\
     另外：如果本次内容与下面「已识别过的行动项」是同一件事（或只是同一件事的重复/补充说明），不要再报一次。\
     只输出一个 JSON 对象：{\"type\":\"quiz|interaction|deadline|other\",\
     \"content\":\"要学生做的具体事（一句话，含明确的动作）\",\"action_guide\":\"建议当场怎么做\",\"urgency\":\"high|medium|low\"}；\
     没有则输出 {\"type\":\"none\"}。";
/// 归档时行动项检测（兼容旧提示词标识；实际归档已合并为 archive 环节）。
pub const DEFAULT_PROMPT_DETECT_BATCH: &str =
    "你是课堂行动项检测器。从课堂内容中提取需要学生处理的行动项：\
     课后作业、提交任务、阅读/预习复习、测验考试提醒、需要准备的材料、当场未完成的互动等。\
     每条写清楚任务内容、资源获取方式与提交方式。\
     尺度适度：明确的必报，拿不准但有迹象的也可报告（允许少量误报）。\
     只输出一个 JSON 字符串数组，元素为具体的中文行动项，没有则输出 []。";
/// 归档后说话人标注（输入带序号与声道提示的字幕）。
pub const DEFAULT_PROMPT_SPEAKER: &str =
    "你是课堂录音说话人标注器。下面是按序号排列的课堂双语字幕，每行末尾标注了同传系统识别到的声道编号（A/B…，? 表示未知）。\
     一节课中讲话最多、持续讲授内容的人是授课老师；其余发言（学生提问与回答、课堂讨论、旁人交谈）为其他人。\
     请逐行判断说话人身份：根据称呼语（“老师/我问一下”等）、内容是讲授还是提问应答来判断；声道切换是重要线索；\
     证据不足、拿不准的一律标为老师。\
     只输出 JSON 数组，每个元素形如 {\"seq\":12,\"who\":\"teacher\"} 或 {\"seq\":13,\"who\":\"other\"}，必须为输入的每一行给出判定，不要输出其他内容。";
/// 归档合并环节：详细行动项 + 笔记一次调用。占位符：{marker_note}。
pub const DEFAULT_PROMPT_ARCHIVE: &str =
    "你是课堂归档助手。输入是一节课的双语字幕文稿（按序号排列）与课堂附件信息。请一次性完成两件事。\n\
     一、课后行动项：提取所有需要学生课后或后续处理的事项——课后作业、提交任务、阅读/预习/复习、测验考试提醒、\
     需要准备或携带的材料、登记报名扫码等，也包括当堂没做完的互动任务。\n\
     - 尺度适度：明确的必报，拿不准但确有迹象的也可以列出（允许少量误报）；普通讲课内容不要报。\n\
     - 每条 content 要具体说明是什么任务；action_guide 要写清操作细节：任务内容与范围、\
     需要的资源或材料从哪里获取（课件页码/网络平台/教材章节/邮箱等）、提交方式与渠道、提交格式等，\
     凡上下文能推断的都写上，不要写“按老师要求完成”这种空话。\n\
     - ddl：根据明确的截止表述结合给出的课堂日期推断，输出 \"YYYY-MM-DD\"（只有相对说法如“下周一”就按课堂日期推算）；无法确定输出 null。\n\
     - urgency：high（三天内/当堂截止）、medium（本周内）、low（更远或无明确期限）。\n\
     二、课堂笔记：把内容整理为结构化 Markdown 笔记，包含课堂大纲、关键知识点、例题与结论，语言精炼。\
     {material_instruction}\n\
     - 笔记篇幅与课堂长度匹配：课堂不足 3 分钟或实质内容过少时 note_markdown 输出空字符串 \"\"；\
     3–15 分钟约 300–600 字；15–45 分钟约 800–2000 字；45 分钟以上不少于 2000 字。\n\
     {marker_note}\
     只输出一个 JSON 对象，不要输出多余文字：\n\
     {\"action_items\":[{\"type\":\"homework|submission|reading|exam|material|other\",\
     \"content\":\"具体任务\",\"action_guide\":\"操作细节：内容/资源获取/提交方式\",\"urgency\":\"high|medium|low\",\
     \"ddl\":\"YYYY-MM-DD 或 null\"}],\"note_markdown\":\"Markdown 笔记正文或空字符串\"}";
/// 非老师发言记号说明（注入归档/聊天/笔记等所有使用标注文稿的提示词）。
pub const SPEAKER_MARKER_NOTE: &str =
    "记号说明：以【同学】开头的行是非授课老师（学生或其他人）的发言；这些提问可作为常见疑问参考，\
     但不要把学生发言里的闲聊、玩笑当成老师布置的任务。";
/// 笔记生成。占位符 {material_instruction}：有课件时替换为课件指引，无课件时替换为空。
pub const DEFAULT_PROMPT_NOTE: &str =
    "你是课堂笔记整理助手。把课堂内容整理为结构化的 Markdown 笔记，包含课堂大纲、关键知识点、例题与结论，语言精炼。{material_instruction}";
/// 有附件时替换进笔记模板的指引句。
pub const NOTE_MATERIAL_SUFFIX: &str =
    "以字幕内容为主线，结合课件附件的章节结构组织笔记；来自附件而非字幕的信息需标注「附件参考」。";
/// 文稿整理：合并被过度分割的语句、去除口误；英文不改措辞，中文译文轻度润色并去表情。
pub const DEFAULT_PROMPT_TIDY: &str =
    "你是课堂文稿整理助手。输入是按序号排列的课堂实时字幕（可能存在被过度切分的短句和口误）。\
     只做两件事：① 把明显属于同一句话的相邻碎片合并成完整语句；\
     ② 去掉重复、结巴、口误和无意义的语气词。\
     英文原文严格保持原意、用词和语序：不增删任何信息，不润色，不翻译，不概括，不调整先后的叙述顺序。\
     中文译文在忠实原意的前提下轻度润色：把直译腔调整得通顺自然、符合中文表达习惯，\
     但不得增删实质信息、不得改变句意。\
     一律删除所有表情符号（emoji、颜文字、装饰符号），它们不是讲话内容。\
     只输出一个 JSON 数组，按原文顺序输出整理后的全部内容，每个元素形如\
     {\"en\":\"整理后的原文\",\"zh\":\"润色后的对应译文（无对应译文则为空字符串）\"}。";
/// 课堂对话。支持占位符 {title} {context}。
pub const DEFAULT_PROMPT_CHAT: &str =
    "你是「同桌小轩」，正在随堂辅助学生。下面是本节课「{title}」截至目前的增量课堂记录。\n\
     请回答学生的提问（可能附带课堂照片/板书图片，需要你识图理解）。\n\
     判定规则：若这条问答对课后复习有沉淀价值（知识点澄清、题目解答、老师补充、板书讲解等），save_to_note=true；\n\
     简单闲聊、临时操作、与课程无关的问题 save_to_note=false。\n\
     只输出 JSON，不要输出多余内容：{\"save_to_note\":true或false,\"reply\":\"你的回答，简洁中文，可用 Markdown\"}\n\n\
     课堂记录：\n{context}";

/// 任务标识 → 内置默认模板（未知任务返回空串）。
pub fn default_prompt(task: &str) -> &'static str {
    match task {
        "translate_batch" => DEFAULT_PROMPT_TRANSLATE_BATCH,
        "detect" => DEFAULT_PROMPT_DETECT,
        "detect_batch" => DEFAULT_PROMPT_DETECT_BATCH,
        "speaker" => DEFAULT_PROMPT_SPEAKER,
        "archive" => DEFAULT_PROMPT_ARCHIVE,
        "tidy" => DEFAULT_PROMPT_TIDY,
        "note" => DEFAULT_PROMPT_NOTE,
        "chat" => DEFAULT_PROMPT_CHAT,
        _ => "",
    }
}

/// 单条待入库的用量明细（不包含自增 id）。
pub struct UsageInsert {
    pub session_id: i64,
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

/// 归档管线的执行报告（返回给 finish_session 决定是否写入笔记）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineReport {
    /// "real"（真实 HTTP 调用）或 "mock"（降级）
    pub mode: String,
    /// 降级原因（真实模式为 None）
    pub reason: Option<String>,
    /// 生成的归档笔记
    pub note: Option<String>,
    /// 检测到的行动项
    pub action_items: Vec<String>,
}

impl PipelineReport {
    fn mock(reason: impl Into<String>) -> Self {
        Self {
            mode: "mock".into(),
            reason: Some(reason.into()),
            note: None,
            action_items: Vec::new(),
        }
    }
}

/* ============================ 管线入口 ============================ */

/// 归档管线入口：优先真实 Stage，不可用时降级 mock。
pub async fn run_pipeline(
    pool: &SqlitePool,
    session_id: i64,
    duration_sec: i64,
    model_tier: &str,
    start_at: &str,
    chunks_dir: &Path,
    api_key: Option<String>,
    speak_language: &str,
    daily_language: &str,
) -> AppResult<PipelineReport> {
    let Some(key) = api_key.filter(|k| !k.trim().is_empty()) else {
        run_mock_pipeline(pool, session_id, duration_sec, model_tier, start_at).await?;
        return Ok(PipelineReport::mock("未配置硅基流动 API Key"));
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| AppError::new(format!("HTTP 客户端创建失败：{e}")))?;

    let dur_min = (duration_sec as f64 / 60.0).floor() as i64;
    let mut records: Vec<UsageInsert> = Vec::new();

    // 实时字幕线程已落库的双语行：非空则复用，跳过 ASR/翻译 Stage（PRD 5.4）
    let live_lines = db::list_transcript_lines(pool, session_id).await?;

    let (transcript, translated_all) = if !live_lines.is_empty() {
        // 实时引擎（AST 同传或经济转写）已完成转写/翻译并各自记过用量，
        // 这里直接复用字幕，不再补 ASR 0 元行（避免花费明细出现误导性的 ASR 条目）
        let en = live_lines
            .iter()
            .map(|l| l.en.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let zh = live_lines
            .iter()
            .filter_map(|l| l.zh.clone())
            .collect::<Vec<_>>();
        (en, zh)
    } else {
        let chunks = collect_chunks(chunks_dir);
        if chunks.is_empty() {
            run_mock_pipeline(pool, session_id, duration_sec, model_tier, start_at).await?;
            return Ok(PipelineReport::mock("未找到课堂录音分块"));
        }
        // Stage 1+2 · 逐块 ASR 落库 + 批量翻译回填（ASR 免费，仅记一条 0 元用量行）
        records.push(asr_usage_row(session_id, start_at, duration_sec));
        match asr_and_translate(
            &client,
            &key,
            pool,
            session_id,
            start_at,
            model_tier,
            speak_language,
            daily_language,
            &chunks,
            &mut records,
        )
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[pipeline] ASR 失败，降级 mock：{}", e.0);
                // 真实分支已写入的 0 元 ASR 行不影响金额；mock 补齐其余明细
                run_mock_pipeline(pool, session_id, duration_sec, model_tier, start_at).await?;
                return Ok(PipelineReport::mock(format!("ASR 调用失败：{}", e.0)));
            }
        }
    };

    // 没有任何有效语音：立即结束，不调用检测/笔记，不产生费用（PRD：空录音不收费）
    if transcript.trim().is_empty() {
        db::batch_create_usage(pool, &records).await?;
        return Ok(PipelineReport {
            mode: "real".into(),
            reason: Some("未识别到有效语音，已跳过笔记生成".into()),
            note: None,
            action_items: Vec::new(),
        });
    }

    // Stage 2.5 · 文稿整理：把尚未整理的字幕行合并为整理稿（归档兜底；实时线程已整理过的不重复）
    if !transcript.trim().is_empty() {
        match tidy_remaining(&client, &key, pool, session_id, model_tier, None).await {
            Ok(n) if n > 0 => eprintln!("[pipeline] 归档文稿整理完成，共整理 {n} 行字幕"),
            Ok(_) => {}
            Err(e) => eprintln!("[pipeline] 文稿整理失败（保留原始文稿）：{}", e.0),
        }
    }

    // Stage 2.8 · 说话人标注：按 AST 换人/声道信号 + 语义，把每行标为 teacher/other。
    // 失败不阻断归档（未标注行视作老师，后续不印记号）。
    let _ = translated_all; // 译文已在字幕行内，后续统一从 DB 取标注文稿
    if let Err(e) =
        attribute_speakers(&client, &key, pool, session_id, model_tier, start_at).await
    {
        eprintln!("[pipeline] 说话人标注失败（按全部老师处理）：{}", e.0);
    }

    // Stage 3 · 归档合并环节：详细课后行动项 + 课堂笔记一次调用
    let (action_items, note) =
        match archive_stage(&client, &key, pool, session_id, model_tier, start_at, dur_min).await {
            Ok((items, note, mut stage_records)) => {
                records.append(&mut stage_records);
                (items, note)
            }
            Err(e) => {
                eprintln!("[pipeline] 归档环节失败：{}", e.0);
                // 降级：至少保留实时检出的行动项
                let existing = db::list_action_items(pool, session_id).await?;
                (existing.iter().map(|it| it.content.clone()).collect(), None)
            }
        };

    db::batch_create_usage(pool, &records).await?;

    Ok(PipelineReport {
        mode: "real".into(),
        reason: None,
        note,
        action_items,
    })
}

/// ASR 用量行：SenseVoiceSmall 免费，金额恒为 0，仅记录音频秒数。
fn asr_usage_row(session_id: i64, start_at: &str, audio_seconds: i64) -> UsageInsert {
    UsageInsert {
        session_id,
        ts: start_at.to_string(),
        stage: "asr".into(),
        model: ASR_MODEL_LABEL.into(),
        model_tier: "free".into(),
        input_tokens: 0,
        cached_tokens: 0,
        output_tokens: 0,
        audio_seconds,
        amount: 0.0,
    }
}

/* ---------------------------- 真实 Stage 实现 ---------------------------- */

pub(crate) struct ChatOutcome {
    pub(crate) content: String,
    pub(crate) prompt_tokens: i64,
    pub(crate) cached_tokens: i64,
    pub(crate) completion_tokens: i64,
}

/// 单块音频转写（实时线程与归档管线共用）。静音块返回空串。
pub(crate) async fn asr_chunk(client: &Client, key: &str, path: &Path) -> AppResult<String> {
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "chunk.wav".to_string());
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(name)
        .mime_str("audio/wav")
        .map_err(|e| AppError::new(format!("音频分块封装失败：{e}")))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", ASR_MODEL_ID);

    let resp = client
        .post(format!("{SILICONFLOW_BASE}/audio/transcriptions"))
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AppError::new(format!("ASR 请求失败：{e}")))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::new(format!("ASR 响应读取失败：{e}")))?;
    if !status.is_success() {
        return Err(AppError::new(format!(
            "ASR 返回 {status}：{}",
            truncate_chars(&body, 200)
        )));
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    Ok(v["text"].as_str().unwrap_or("").trim().to_string())
}

/// chunk_0007.wav → 7（解析失败返回 None）。
fn chunk_seq(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_string_lossy();
    let rest = name.strip_prefix("chunk_")?.strip_suffix(".wav")?;
    rest.parse::<i64>().ok()
}

/// 起始时间 + 秒偏移 → RFC3339（实时字幕行时间片）。
pub(crate) fn ts_after_seconds(start_at: &str, secs: i64) -> String {
    let start_ms = chrono::DateTime::parse_from_rfc3339(start_at)
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|_| chrono::Local::now().timestamp_millis());
    chrono::DateTime::from_timestamp_millis(start_ms + secs * 1000)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| start_at.to_string())
}

/// 解析「编号. 译文」逐行输出 → {编号: 译文}。
pub(crate) fn parse_numbered_lines(content: &str) -> std::collections::HashMap<usize, String> {
    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some((head, rest)) = t.split_once('.') {
            let head = head.trim();
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = head.parse::<usize>() {
                    let text = rest.trim();
                    if !text.is_empty() {
                        map.insert(n, text.to_string());
                    }
                }
            }
        }
    }
    map
}

/// 归档 Stage 1+2：逐块 ASR 落库（en 先入库）→ 批量翻译回填 zh。
/// 返回（原文拼接, 译文列表）。字幕分块固定 10 秒（capture.rs 同步）。
async fn asr_and_translate(
    client: &Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    start_at: &str,
    model_tier: &str,
    speak_language: &str,
    daily_language: &str,
    chunks: &[PathBuf],
    records: &mut Vec<UsageInsert>,
) -> AppResult<(String, Vec<String>)> {
    let mut rows: Vec<(i64, String)> = Vec::new();
    for path in chunks {
        let text = match asr_chunk(client, key, path).await {
            Ok(t) => t,
            Err(e) => {
                let _ = db::insert_llm_log(
                    pool,
                    db::LlmLogInsert {
                        session_id: Some(session_id),
                        stage: "asr".into(),
                        model: ASR_MODEL_LABEL.into(),
                        system_prompt: None,
                        user_prompt: Some(format!(
                            "音频分块 {}",
                            path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                        )),
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
        if text.trim().is_empty() {
            continue;
        }
        let seq = chunk_seq(path).unwrap_or(rows.len() as i64);
        let ts = ts_after_seconds(start_at, seq * 10);
        let id = db::insert_transcript_line(pool, session_id, &ts, seq, text.trim(), None, "").await?;
        rows.push((id, text.trim().to_string()));
    }
    if rows.is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    let transcript = rows.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>().join("\n");

    // 批量翻译：一次调用翻译全部行，按行号回填（解析失败的行 zh 保持 null）
    let numbered = rows
        .iter()
        .enumerate()
        .map(|(i, (_, t))| format!("{}. {}", i + 1, t.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n");
    let input = truncate_chars(&numbered, MAX_INPUT_CHARS);
    let translate_tpl = db::get_prompt(pool, "translate_batch")
        .await
        .unwrap_or_else(|_| DEFAULT_PROMPT_TRANSLATE_BATCH.to_string())
        .replace("{speak_language}", speak_language)
        .replace("{daily_language}", daily_language);
    let translate_user = format!("源语言：{speak_language}，目标语言：{daily_language}\n原文：\n{input}");
    match chat(client, key, TRANSLATE_MODEL_ID, &translate_tpl, &translate_user).await {
        Ok(out) => {
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "translate".into(),
                    model: TRANSLATE_MODEL_LABEL.into(),
                    system_prompt: Some(translate_tpl.clone()),
                    user_prompt: Some(translate_user.clone()),
                    response: Some(out.content.clone()),
                    ok: true,
                    error: None,
                    input_tokens: out.prompt_tokens,
                    cached_tokens: out.cached_tokens,
                    output_tokens: out.completion_tokens,
                },
            )
            .await;
            let map = parse_numbered_lines(&out.content);
            let mut translated: Vec<String> = Vec::new();
            for (i, (id, _)) in rows.iter().enumerate() {
                if let Some(zh) = map.get(&(i + 1)) {
                    let _ = db::update_transcript_zh(pool, *id, zh).await;
                    translated.push(zh.clone());
                }
            }
            records.push(UsageInsert {
                session_id,
                ts: minute_ts(start_at, 0),
                stage: "translate".into(),
                model: TRANSLATE_MODEL_LABEL.into(),
                model_tier: model_tier.to_string(),
                input_tokens: out.prompt_tokens,
                cached_tokens: out.cached_tokens,
                output_tokens: out.completion_tokens,
                audio_seconds: 0,
                amount: token_amount(
                    TRANSLATE_MODEL_LABEL,
                    out.prompt_tokens,
                    out.cached_tokens,
                    out.completion_tokens,
                ),
            });
            Ok((transcript, translated))
        }
        Err(e) => {
            eprintln!("[pipeline] 批量翻译失败（仅保留原文）：{}", e.0);
            let _ = db::insert_llm_log(
                pool,
                db::LlmLogInsert {
                    session_id: Some(session_id),
                    stage: "translate".into(),
                    model: TRANSLATE_MODEL_LABEL.into(),
                    system_prompt: Some(translate_tpl.clone()),
                    user_prompt: Some(translate_user.clone()),
                    response: None,
                    ok: false,
                    error: Some(e.0.clone()),
                    input_tokens: 0,
                    cached_tokens: 0,
                    output_tokens: 0,
                },
            )
            .await;
            Ok((transcript, Vec::new()))
        }
    }
}

/// 展开 reqwest 错误的完整原因链（Display 只含最外层，连接/TLS 细节在 source 里）。
fn reqwest_err_chain(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut cur: Option<&dyn std::error::Error> = std::error::Error::source(e);
    while let Some(src) = cur {
        out.push_str(" → ");
        out.push_str(&src.to_string());
        cur = src.source();
    }
    out
}

/// 调用 chat/completions（翻译 / 检测 / 笔记共用）。
pub(crate) async fn chat(
    client: &Client,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> AppResult<ChatOutcome> {
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0.2,
        "max_tokens": 3000
    });

    let resp = client
        .post(format!("{SILICONFLOW_BASE}/chat/completions"))
        .bearer_auth(key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| AppError::new(format!("模型调用失败：{}", reqwest_err_chain(&e))))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::new(format!("模型响应读取失败：{}", reqwest_err_chain(&e))))?;
    if !status.is_success() {
        return Err(AppError::new(format!(
            "模型返回 {status}：{}",
            truncate_chars(&body, 200)
        )));
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    Ok(ChatOutcome {
        content: v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string(),
        prompt_tokens: v["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
        cached_tokens: v["usage"]["prompt_tokens_details"]["cached_tokens"]
            .as_i64()
            .unwrap_or(0),
        completion_tokens: v["usage"]["completion_tokens"].as_i64().unwrap_or(0),
    })
}

/// 可指定温度与 max_tokens 的单轮调用（归档合并环节需要更长输出）。
pub(crate) async fn chat_full(
    client: &Client,
    key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: i64,
) -> AppResult<ChatOutcome> {
    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": temperature,
        "max_tokens": max_tokens
    });

    let resp = client
        .post(format!("{SILICONFLOW_BASE}/chat/completions"))
        .bearer_auth(key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| AppError::new(format!("模型调用失败：{}", reqwest_err_chain(&e))))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::new(format!("模型响应读取失败：{}", reqwest_err_chain(&e))))?;
    if !status.is_success() {
        return Err(AppError::new(format!(
            "模型返回 {status}：{}",
            truncate_chars(&body, 200)
        )));
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    Ok(ChatOutcome {
        content: v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string(),
        prompt_tokens: v["usage"]["prompt_tokens"].as_i64().unwrap_or(0),
        cached_tokens: v["usage"]["prompt_tokens_details"]["cached_tokens"]
            .as_i64()
            .unwrap_or(0),
        completion_tokens: v["usage"]["completion_tokens"].as_i64().unwrap_or(0),
    })
}

/* ------------------------------ 文稿整理 ------------------------------ */

/// 文稿整理触发阈值：累计约 200 个「词单元」（英文单词 / CJK 字）调用一次整理。
pub(crate) const TIDY_SPAN_UNITS: usize = 200;
/// 整理任务串行锁：实时整理与归档兜底整理互斥，避免重复写入整理稿。
static TIDY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 估算文本的「词单元」数：英文按空格分词，CJK 每字计 1。
pub(crate) fn text_units(s: &str) -> usize {
    let mut units = 0usize;
    let mut in_word = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '\'' {
            in_word = true;
        } else {
            if in_word {
                units += 1;
            }
            in_word = false;
            if ('\u{4e00}'..='\u{9fff}').contains(&c) {
                units += 1;
            }
        }
    }
    if in_word {
        units += 1;
    }
    units
}

/// 整理尚未被整理稿覆盖的字幕行。
/// - `min_units = Some(n)`：未覆盖部分累计不足 n 个词单元时跳过（实时轮询用，摊薄调用成本）；
/// - `min_units = None`：归档兜底，有多少整理多少（仍按 ~200 单元分批调用）。
/// 返回本次实际整理的字幕行数。
pub(crate) async fn tidy_remaining(
    client: &Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    min_units: Option<usize>,
) -> AppResult<usize> {
    let _guard = TIDY_LOCK.lock().await;
    let lines = db::list_transcript_lines(pool, session_id).await?;
    if lines.is_empty() {
        return Ok(0);
    }
    let covered_to = db::max_tidy_to_seq(pool, session_id).await?.unwrap_or(0);
    let pending: Vec<crate::models::TranscriptLineDto> =
        lines.into_iter().filter(|l| l.seq > covered_to).collect();
    if pending.is_empty() {
        return Ok(0);
    }
    let total: usize = pending
        .iter()
        .map(|l| text_units(&l.en) + l.zh.as_deref().map_or(0, text_units))
        .sum();
    if let Some(min) = min_units {
        if total < min {
            return Ok(0);
        }
    }

    let tpl = db::get_prompt(pool, "tidy")
        .await
        .unwrap_or_else(|_| DEFAULT_PROMPT_TIDY.to_string());
    let mut done_rows = 0usize;
    let mut batch: Vec<crate::models::TranscriptLineDto> = Vec::new();
    let mut batch_units = 0usize;
    let flush = |batch: &mut Vec<crate::models::TranscriptLineDto>,
                 done: &mut usize|
     -> Vec<crate::models::TranscriptLineDto> {
        let taken = std::mem::take(batch);
        *done += taken.len();
        taken
    };
    for l in pending {
        batch.push(l.clone());
        batch_units += text_units(&l.en) + l.zh.as_deref().map_or(0, text_units);
        if batch_units >= TIDY_SPAN_UNITS {
            let taken = flush(&mut batch, &mut done_rows);
            tidy_batch(client, key, pool, session_id, model_tier, &tpl, &taken).await?;
        }
    }
    if !batch.is_empty() {
        let taken = flush(&mut batch, &mut done_rows);
        tidy_batch(client, key, pool, session_id, model_tier, &tpl, &taken).await?;
    }
    Ok(done_rows)
}

/// 整理一批字幕行（from_seq..=to_seq），成功则写入整理稿表并记录用量/日志。
async fn tidy_batch(
    client: &Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    tpl: &str,
    batch: &[crate::models::TranscriptLineDto],
) -> AppResult<()> {
    let from_seq = batch.first().map(|l| l.seq).unwrap_or(0);
    let to_seq = batch.last().map(|l| l.seq).unwrap_or(0);
    let mut input = String::new();
    for (i, l) in batch.iter().enumerate() {
        input.push_str(&format!("{}. {}\n", i + 1, l.en));
        if let Some(zh) = l.zh.as_deref() {
            if !zh.trim().is_empty() {
                input.push_str(&format!("   （译）{zh}\n"));
            }
        }
    }

    // 200 词单元批次的 JSON 数组输出可达数千 token，3000 上限会被截断成非法 JSON；
    // 用 chat_full 放宽到 6000。
    let out = chat_full(client, key, TRANSLATE_MODEL_ID, tpl, &input, 0.2, 6000).await?;
    let cleaned = out
        .content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => v,
        Err(e) => {
            tidy_log_failure(pool, session_id, tpl, &input, &out, &format!("输出解析失败：{e}")).await;
            return Err(AppError::new(format!("文稿整理输出解析失败：{e}")));
        }
    };
    let arr: Vec<serde_json::Value> = match parsed {
        serde_json::Value::Array(a) => a,
        // 模型偶发对整批只回一个对象而非数组，按单元素数组处理
        obj @ serde_json::Value::Object(_) => vec![obj],
        other => {
            let msg = format!("输出既非数组也非对象：{other}");
            tidy_log_failure(pool, session_id, tpl, &input, &out, &msg).await;
            return Err(AppError::new(msg));
        }
    };

    let mut ord = 0i64;
    for item in &arr {
        let en = item["en"].as_str().unwrap_or("").trim().to_string();
        if en.is_empty() {
            continue;
        }
        let zh = item["zh"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        db::insert_tidy_passage(pool, session_id, from_seq, to_seq, ord, &en, zh.as_deref())
            .await?;
        ord += 1;
    }
    if ord == 0 {
        return Err(AppError::new("文稿整理模型未返回有效内容"));
    }

    let usage = token_amount(
        TRANSLATE_MODEL_LABEL,
        out.prompt_tokens,
        out.cached_tokens,
        out.completion_tokens,
    );
    let records = vec![UsageInsert {
        session_id,
        ts: chrono::Local::now().to_rfc3339(),
        stage: "tidy".into(),
        model: TRANSLATE_MODEL_LABEL.into(),
        model_tier: model_tier.to_string(),
        input_tokens: out.prompt_tokens,
        cached_tokens: out.cached_tokens,
        output_tokens: out.completion_tokens,
        audio_seconds: 0,
        amount: usage,
    }];
    db::batch_create_usage(pool, &records).await?;
    let _ = db::insert_llm_log(
        pool,
        db::LlmLogInsert {
            session_id: Some(session_id),
            stage: "tidy".into(),
            model: TRANSLATE_MODEL_LABEL.into(),
            system_prompt: Some(tpl.to_string()),
            user_prompt: Some(input),
            response: Some(out.content),
            ok: true,
            error: None,
            input_tokens: out.prompt_tokens,
            cached_tokens: out.cached_tokens,
            output_tokens: out.completion_tokens,
        },
    )
    .await;
    Ok(())
}

/// 整理批次失败时补写 llm_logs（ok=false）。此前失败批次无日志，异常输出无从复盘。
async fn tidy_log_failure(
    pool: &SqlitePool,
    session_id: i64,
    tpl: &str,
    input: &str,
    out: &ChatOutcome,
    error: &str,
) {
    let _ = db::insert_llm_log(
        pool,
        db::LlmLogInsert {
            session_id: Some(session_id),
            stage: "tidy".into(),
            model: TRANSLATE_MODEL_LABEL.into(),
            system_prompt: Some(tpl.to_string()),
            user_prompt: Some(input.to_string()),
            response: Some(out.content.clone()),
            ok: false,
            error: Some(error.to_string()),
            input_tokens: out.prompt_tokens,
            cached_tokens: out.cached_tokens,
            output_tokens: out.completion_tokens,
        },
    )
    .await;
}

/* --------------------------- 聊天基础设施 --------------------------- */

/// 流式一轮对话的结果：正文增量已通过 on_delta 推给前端；tool_calls 为拼装后的工具调用。
pub(crate) struct StreamOutcome {
    pub(crate) content: String,
    pub(crate) tool_calls: Vec<serde_json::Value>,
    pub(crate) prompt_tokens: i64,
    pub(crate) cached_tokens: i64,
    pub(crate) completion_tokens: i64,
}

/// 完整消息数组的流式调用（工具循环的每一轮都走它）：
/// - delta.content 实时回调 on_delta（仅最终回答轮会产生正文，工具轮为空）；
/// - delta.tool_calls 分片按 index 拼装，结束后以 OpenAI 消息形态返回；
/// - 开启 stream_options.include_usage，最后一个 chunk 带用量。
pub(crate) async fn chat_messages_stream<F>(
    client: &Client,
    key: &str,
    model_id: &str,
    system: &str,
    messages: &[serde_json::Value],
    tools: Option<&serde_json::Value>,
    mut on_delta: F,
) -> AppResult<StreamOutcome>
where
    F: FnMut(&str),
{
    use futures_util::StreamExt;

    let mut msgs = vec![serde_json::json!({ "role": "system", "content": system })];
    msgs.extend_from_slice(messages);
    let mut payload = serde_json::json!({
        "model": model_id,
        "messages": msgs,
        "temperature": 0.3,
        "max_tokens": 2000,
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    if let Some(t) = tools {
        payload["tools"] = t.clone();
    }
    let resp = client
        .post(format!("{SILICONFLOW_BASE}/chat/completions"))
        .bearer_auth(key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| AppError::new(format!("聊天请求发送失败：{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp
            .text()
            .await
            .unwrap_or_else(|_| "（无法读取错误响应）".into());
        return Err(AppError::new(format!(
            "聊天接口错误（{status}）：{}",
            truncate_chars(&body, 200)
        )));
    }

    // 模型会把推理内联写在 content 里（<thinking>…</thinking>），这里流式剔除后只推正文
    let mut think = ThinkFilter::new();
    // index → (id, name, arguments 累积串)
    let mut tc_acc: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();
    let (mut pt, mut ct, mut ot) = (0i64, 0i64, 0i64);
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| AppError::new(format!("聊天流中断：{e}")))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        // SSE 以空行/换行分隔事件；按行消费，保留最后半行
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(u) = v.get("usage") {
                pt = u["prompt_tokens"].as_i64().unwrap_or(pt);
                ct = u["prompt_tokens_details"]["cached_tokens"].as_i64().unwrap_or(ct);
                ot = u["completion_tokens"].as_i64().unwrap_or(ot);
            }
            if let Some(delta) = v["choices"][0].get("delta") {
                if let Some(piece) = delta["content"].as_str() {
                    if !piece.is_empty() {
                        let delta_text = think.feed(piece);
                        if !delta_text.is_empty() {
                            on_delta(&delta_text);
                        }
                    }
                }
                if let Some(calls) = delta["tool_calls"].as_array() {
                    for c in calls {
                        let idx = c["index"].as_u64().unwrap_or(0) as usize;
                        let entry = tc_acc.entry(idx).or_default();
                        if let Some(id) = c["id"].as_str() {
                            entry.0 = id.to_string();
                        }
                        if let Some(name) = c["function"]["name"].as_str() {
                            entry.1 = name.to_string();
                        }
                        if let Some(args) = c["function"]["arguments"].as_str() {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }
    }

    let tool_calls = tc_acc
        .into_values()
        .filter(|(_, name, _)| !name.is_empty())
        .map(|(id, name, args)| {
            serde_json::json!({
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": args }
            })
        })
        .collect();

    Ok(StreamOutcome {
        content: think.finish().trim().to_string(),
        tool_calls,
        prompt_tokens: pt,
        cached_tokens: ct,
        completion_tokens: ot,
    })
}

/// 流式剔除内联思维链的过滤器。
/// 模型（DeepSeek-V3.2）会把推理写在 content 里，形如 `<thinking>…</thinking>\n\n正文`；
/// 标签可能被逐字切成多片到达，因此把疑似“半个标签”的尾部暂存，定论后再放行。
struct ThinkFilter {
    in_think: bool,
    /// 是否已推送过首段可见正文（用于丢弃思维链闭合后的起始空行/空白）
    started: bool,
    /// 暂存尾部（可能是半个标签或半个标签之后的思维链内容）
    hold: String,
    /// 已确认可见的正文
    visible: String,
}

impl ThinkFilter {
    fn new() -> Self {
        Self {
            in_think: false,
            started: false,
            hold: String::new(),
            visible: String::new(),
        }
    }

    /// 送入一片增量原文，返回本轮可推送给前端的正文增量。
    fn feed(&mut self, piece: &str) -> String {
        self.hold.push_str(piece);
        let mut out = String::new();
        loop {
            if self.in_think {
                match find_ci(&self.hold, CLOSE_TAG) {
                    Some(i) => {
                        self.hold.drain(..i + CLOSE_TAG.len());
                        self.in_think = false;
                    }
                    None => {
                        // 思维链内容直接丢弃，只留疑似半个闭合标签的尾部
                        let safe = safe_prefix_len(&self.hold, CLOSE_TAG);
                        self.hold.drain(..safe);
                        break;
                    }
                }
            } else {
                match find_ci(&self.hold, OPEN_TAG) {
                    Some(i) => {
                        out.push_str(&self.hold[..i]);
                        self.hold.drain(..i + OPEN_TAG.len());
                        self.in_think = true;
                    }
                    None => {
                        let safe = safe_prefix_len(&self.hold, OPEN_TAG);
                        out.push_str(&self.hold[..safe]);
                        self.hold.drain(..safe);
                        break;
                    }
                }
            }
        }
        // 首段可见正文之前丢弃空白，避免思维链闭合后的空行被推到前端
        if !self.started {
            let lead = out.len() - out.trim_start().len();
            if lead > 0 {
                out.drain(..lead);
            }
            if !out.is_empty() {
                self.started = true;
            }
        }
        self.visible.push_str(&out);
        out
    }

    /// 流结束：并入暂存正文（未闭合的思维链内容丢弃），返回完整可见正文。
    fn finish(&mut self) -> String {
        if !self.in_think {
            self.visible.push_str(&self.hold);
        }
        self.hold.clear();
        std::mem::take(&mut self.visible)
    }
}

const OPEN_TAG: &str = "<thinking>";
const CLOSE_TAG: &str = "</thinking>";

/// 大小写不敏感的 ASCII 子串查找，返回字节下标（保证落在字符边界上）。
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len())
        .find(|&i| hay.is_char_boundary(i) && h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// 返回不含“半个标签”的安全前缀长度：若尾部恰为 tag 的真前缀，则把该尾部排除。
fn safe_prefix_len(s: &str, tag: &str) -> usize {
    let b = s.as_bytes();
    let n = tag.as_bytes();
    // 允许 k == 整个 s 的长度（此时 hold 全部是标签前缀，整段暂存）；
    // k == tag 整长不可能命中——命中说明标签已在串内，find_ci 早就返回了。
    let max = n.len().min(b.len());
    for k in (1..=max).rev() {
        if b[b.len() - k..].eq_ignore_ascii_case(&n[..k]) {
            return b.len() - k;
        }
    }
    s.len()
}

/// 由设置中的人设 JSON 生成注入聊天系统提示词的人设段落。
/// 结构：{"detail":"low|mid|high","warmth":"cold|mid|warm","humor":"serious|mid|fun","extra":"..."}。
pub(crate) fn persona_block(persona_json: Option<&str>) -> String {
    let mut s = String::from(
        "你是「同桌小轩」，学生的专属智能同桌与学习伴侣：随堂陪伴上课、答疑解惑、帮忙整理知识。\
         回答使用中文，语气自然，像坐在一起的同学。\n",
    );
    if let Some(p) = persona_json {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(p) {
            match v["detail"].as_str() {
                Some("low") => s.push_str("人设：回答尽量简短直接，只给要点，不展开。\n"),
                Some("high") => s.push_str("人设：回答详细展开，讲清来龙去脉，必要时举例。\n"),
                _ => {}
            }
            match v["warmth"].as_str() {
                Some("cold") => s.push_str("人设：语气克制冷静，少用感叹和表情。\n"),
                Some("warm") => s.push_str("人设：语气热情友好，主动鼓励同学。\n"),
                _ => {}
            }
            match v["humor"].as_str() {
                Some("serious") => s.push_str("人设：风格严肃专业，不插科打诨。\n"),
                Some("fun") => s.push_str("人设：可以适度幽默，用轻松的比喻把事讲明白。\n"),
                _ => {}
            }
            if let Some(extra) = v["extra"].as_str() {
                let extra = extra.trim();
                if !extra.is_empty() {
                    s.push_str("用户自定义人设补充（优先级最高）：");
                    s.push_str(&truncate_chars(extra, 800));
                    s.push('\n');
                }
            }
        }
    }
    s
}

/* ------------------------ Stage 2.8 说话人标注 ------------------------ */

/// 每行送标注的最大条数；超过分批调用。
const SPEAKER_BATCH_LINES: usize = 120;

/// 归档说话人标注：利用 AST 换人/声道信号 + 语义，把字幕行标成 teacher/other 并回写 DB。
/// 已标注（teacher/other）的行跳过；全部标注过则直接返回。
async fn attribute_speakers(
    client: &Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    start_at: &str,
) -> AppResult<()> {
    let lines = db::list_transcript_lines(pool, session_id).await?;
    let pending: Vec<&crate::models::TranscriptLineDto> = lines
        .iter()
        .filter(|l| l.speaker != "teacher" && l.speaker != "other")
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let tpl = db::get_prompt(pool, "speaker")
        .await
        .unwrap_or_else(|_| DEFAULT_PROMPT_SPEAKER.to_string());

    for chunk in pending.chunks(SPEAKER_BATCH_LINES) {
        let mut input = String::new();
        for l in chunk {
            let text = l
                .zh
                .as_deref()
                .filter(|z| !z.trim().is_empty())
                .unwrap_or(&l.en);
            let hint = if l.speaker.trim().is_empty() { "?" } else { l.speaker.as_str() };
            input.push_str(&format!("[{}] {} ｜声道:{}\n", l.seq, text.replace('\n', " "), hint));
        }
        let out = match chat(client, key, TRANSLATE_MODEL_ID, &tpl, &input).await {
            Ok(o) => o,
            Err(e) => {
                let _ = db::insert_llm_log(
                    pool,
                    db::LlmLogInsert {
                        session_id: Some(session_id),
                        stage: "speaker".into(),
                        model: TRANSLATE_MODEL_LABEL.into(),
                        system_prompt: Some(tpl.clone()),
                        user_prompt: Some(input),
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
        let cleaned = strip_json_fence(&out.content);
        let arr: Vec<serde_json::Value> = match serde_json::from_str(cleaned) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[pipeline] 说话人标注输出解析失败：{e}");
                let _ = db::insert_llm_log(
                    pool,
                    db::LlmLogInsert {
                        session_id: Some(session_id),
                        stage: "speaker".into(),
                        model: TRANSLATE_MODEL_LABEL.into(),
                        system_prompt: Some(tpl.clone()),
                        user_prompt: Some(input),
                        response: Some(out.content.clone()),
                        ok: false,
                        error: Some(format!("解析失败：{e}")),
                        input_tokens: out.prompt_tokens,
                        cached_tokens: out.cached_tokens,
                        output_tokens: out.completion_tokens,
                    },
                )
                .await;
                continue;
            }
        };
        let mut updates: Vec<(i64, &str)> = Vec::new();
        for item in &arr {
            let Some(seq) = item["seq"].as_i64() else { continue };
            let who = if item["who"].as_str() == Some("other") {
                "other"
            } else {
                "teacher"
            };
            updates.push((seq, who));
        }
        if let Err(e) = db::update_speakers(pool, session_id, &updates).await {
            eprintln!("[pipeline] 说话人回写失败：{}", e.0);
        }
        db::batch_create_usage(
            pool,
            &[UsageInsert {
                session_id,
                ts: chrono::Local::now().to_rfc3339(),
                stage: "speaker".into(),
                model: TRANSLATE_MODEL_LABEL.into(),
                model_tier: model_tier.to_string(),
                input_tokens: out.prompt_tokens,
                cached_tokens: out.cached_tokens,
                output_tokens: out.completion_tokens,
                audio_seconds: 0,
                amount: token_amount(
                    TRANSLATE_MODEL_LABEL,
                    out.prompt_tokens,
                    out.cached_tokens,
                    out.completion_tokens,
                ),
            }],
        )
        .await?;
        let _ = db::insert_llm_log(
            pool,
            db::LlmLogInsert {
                session_id: Some(session_id),
                stage: "speaker".into(),
                model: TRANSLATE_MODEL_LABEL.into(),
                system_prompt: Some(tpl.clone()),
                user_prompt: Some(input),
                response: Some(out.content),
                ok: true,
                error: None,
                input_tokens: out.prompt_tokens,
                cached_tokens: out.cached_tokens,
                output_tokens: out.completion_tokens,
            },
        )
        .await;
    }
    let _ = start_at;
    Ok(())
}

/* --------------------------- Stage 3 归档合并环节 --------------------------- */

/// 归档环节入参上限（长课字幕可达两万字，DeepSeek 上下文足够）。
const ARCHIVE_MAX_CHARS: usize = 24_000;

/// 给一条文本按说话人加非老师记号。
fn mark_other(text: &str, speaker: &str) -> String {
    if speaker == "other" {
        format!("【同学】{text}")
    } else {
        text.to_string()
    }
}

/// 构造带说话人记号的编号文稿：优先整理稿（按覆盖 seq 取主导说话人），否则原始字幕行。
pub(crate) async fn marked_transcript(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<String> {
    let lines = db::list_transcript_lines(pool, session_id).await?;
    let passages = db::list_tidy_passages(pool, session_id).await?;
    let mut out = String::new();
    if passages.is_empty() {
        for (i, l) in lines.iter().enumerate() {
            let text = l
                .zh
                .as_deref()
                .filter(|z| !z.trim().is_empty())
                .unwrap_or(&l.en);
            out.push_str(&format!("{}. {}\n", i + 1, mark_other(&text.replace('\n', " "), &l.speaker)));
        }
    } else {
        for (i, p) in passages.iter().enumerate() {
            // 该整理段覆盖行中 other 占多数则整段标【同学】
            let others = lines
                .iter()
                .filter(|l| l.seq >= p.from_seq && l.seq <= p.to_seq)
                .filter(|l| l.speaker == "other")
                .count();
            let covered = lines
                .iter()
                .filter(|l| l.seq >= p.from_seq && l.seq <= p.to_seq)
                .count()
                .max(1);
            let speaker = if others * 2 > covered { "other" } else { "teacher" };
            let text = p
                .zh
                .as_deref()
                .filter(|z| !z.trim().is_empty())
                .unwrap_or(&p.en);
            out.push_str(&format!("{}. {}\n", i + 1, mark_other(&text.replace('\n', " "), speaker)));
        }
    }
    Ok(out)
}

/// 组装附件参考块（课件 PPT 解析页数/关键词/章节；作业题目与其他附件仅列名）。
/// 包含本课堂附件 + 同课程的公共附件（课程页批量上传、未绑定具体课堂的资料）。
pub(crate) async fn build_material_ref(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<String> {
    // 先收集本课堂附件，再补课程公共附件，按出现顺序去重
    let mut mats = db::list_session_materials(pool, session_id).await?;
    if let Some(sess) = db::get_session(pool, session_id).await? {
        if let Some(cid) = sess.course_id {
            let mut public = Vec::new();
            for m in db::list_materials(pool, cid).await? {
                if m.session_id.is_none() && !mats.iter().any(|x| x.id == m.id) {
                    public.push(m);
                }
            }
            mats.extend(public);
        }
    }
    let mut material_ref = String::new();
    for m in mats {
        if m.file_path.is_empty() {
            continue;
        }
        let scope = if m.session_id == Some(session_id) { "" } else { "（课程公共）" };
        let (label, parseable) = match m.category.as_str() {
            "homework" => ("作业题目附件", false),
            "slides" => ("课件PPT", true),
            _ => ("其他附件", false),
        };
        if !parseable {
            material_ref.push_str(&format!("{label}{scope}：《{}》\n", m.file_name));
            continue;
        }
        let path = std::path::PathBuf::from(&m.file_path);
        if let Ok(parsed) = crate::ppt::parse_material(&path) {
            material_ref.push_str(&format!(
                "{label}{scope}《{}》：共 {} 页",
                m.file_name, parsed.page_count
            ));
            if !parsed.keywords.is_empty() {
                material_ref.push_str(&format!("；关键词：{}", parsed.keywords.join("、")));
            }
            let titles: Vec<String> = parsed
                .slides
                .iter()
                .take(12)
                .map(|s| {
                    format!(
                        "第{}页 {}",
                        s.index,
                        s.texts.first().cloned().unwrap_or_default()
                    )
                })
                .collect();
            if !titles.is_empty() {
                material_ref.push_str(&format!("；章节：{}", titles.join(" / ")));
            }
            material_ref.push('\n');
        } else {
            material_ref.push_str(&format!("{label}{scope}：《{}》\n", m.file_name));
        }
    }
    Ok(material_ref)
}

/// 归档合并环节：一次调用同时产出详细行动项与笔记，并把新行动项落库。
/// 返回（当前全部行动项内容, 笔记, 用量明细）。
async fn archive_stage(
    client: &Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    start_at: &str,
    dur_min: i64,
) -> AppResult<(Vec<String>, Option<String>, Vec<UsageInsert>)> {
    let marked = marked_transcript(pool, session_id).await?;
    if marked.trim().is_empty() {
        return Ok((Vec::new(), None, Vec::new()));
    }
    let material_ref = build_material_ref(pool, session_id).await?;

    let class_date = chrono::DateTime::parse_from_rfc3339(start_at)
        .map(|d| d.format("%Y-%m-%d %A").to_string())
        .unwrap_or_else(|_| start_at.to_string());
    let tpl = db::get_prompt(pool, "archive")
        .await
        .unwrap_or_else(|_| DEFAULT_PROMPT_ARCHIVE.to_string());
    let system = tpl
        .replace("{material_instruction}", NOTE_MATERIAL_SUFFIX)
        .replace("{marker_note}", SPEAKER_MARKER_NOTE);
    let mut user = format!("课堂日期：{class_date}；课堂时长：约 {dur_min} 分钟。\n\n");
    if material_ref.trim().is_empty() {
        user.push_str("【附件】无\n\n");
    } else {
        user.push_str("【附件】\n");
        user.push_str(&material_ref);
        user.push('\n');
    }
    user.push_str("【带序号的课堂文稿】\n");
    user.push_str(&truncate_chars(&marked, ARCHIVE_MAX_CHARS));

    let (model_id, model_label) = note_model(model_tier);
    let out = chat_full(
        client,
        key,
        model_id,
        &system,
        &user,
        0.2,
        4096,
    )
    .await?;

    let parsed: serde_json::Value = serde_json::from_str(strip_json_fence(&out.content))
        .map_err(|e| AppError::new(format!("归档结果 JSON 解析失败：{e}")))?;

    // 行动项落库（与实时检出/本次已有的去重）
    let existing = db::list_action_items(pool, session_id).await?;
    let mut contents: Vec<String> = existing.iter().map(|it| it.content.clone()).collect();
    let now = chrono::Local::now().to_rfc3339();
    if let Some(items) = parsed["action_items"].as_array() {
        for it in items {
            let content = it["content"].as_str().unwrap_or("").trim().to_string();
            if content.is_empty() || contents.iter().any(|c| similar_item(c, &content)) {
                continue;
            }
            let ty = it["type"].as_str().unwrap_or("other").to_string();
            let guide = it["action_guide"]
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            let urgency = it["urgency"].as_str().unwrap_or("low").to_string();
            let ddl = it["ddl"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
                .and_then(ddl_to_rfc3339);
            if let Err(e) = db::insert_action_item(
                pool,
                session_id,
                &now,
                &ty,
                &content,
                &guide,
                &urgency,
                ddl.as_deref(),
            )
            .await
            {
                eprintln!("[pipeline] 归档行动项落库失败：{}", e.0);
            }
            contents.push(content);
        }
    }

    let note = parsed["note_markdown"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let records = vec![UsageInsert {
        session_id,
        ts: chrono::Local::now().to_rfc3339(),
        stage: "archive".into(),
        model: model_label.to_string(),
        model_tier: model_tier.to_string(),
        input_tokens: out.prompt_tokens,
        cached_tokens: out.cached_tokens,
        output_tokens: out.completion_tokens,
        audio_seconds: 0,
        amount: token_amount(
            model_label,
            out.prompt_tokens,
            out.cached_tokens,
            out.completion_tokens,
        ),
    }];
    db::batch_create_usage(pool, &records).await?;
    let _ = db::insert_llm_log(
        pool,
        db::LlmLogInsert {
            session_id: Some(session_id),
            stage: "archive".into(),
            model: model_label.into(),
            system_prompt: Some(system),
            user_prompt: Some(user),
            response: Some(out.content),
            ok: true,
            error: None,
            input_tokens: out.prompt_tokens,
            cached_tokens: out.cached_tokens,
            output_tokens: out.completion_tokens,
        },
    )
    .await;
    Ok((contents, note, records))
}

/// 去掉模型输出常见的 ```json 代码块包裹。
fn strip_json_fence(s: &str) -> &str {
    s.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
}

/// 行动项去重：归一化后任一方包含另一方即视为重复。
pub(crate) fn similar_item(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && !"，。,.!?！？、：:;；".contains(*c))
            .collect::<String>()
    };
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.contains(&b) || b.contains(&a)
}

/// 模型给的 YYYY-MM-DD → 本地当天 23:59 的 RFC3339；其他格式原样保留。
fn ddl_to_rfc3339(s: &str) -> Option<String> {
    use chrono::TimeZone;
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(23, 59, 0)?;
        chrono::Local
            .from_local_datetime(&dt)
            .single()
            .map(|x| x.to_rfc3339())
    } else {
        Some(s.to_string())
    }
}

/* ------------------------------ 笔记生成共用 ------------------------------ */

/// 笔记模型选择：pro 用 Qwen3.6-27B，其余用 DeepSeek-V3.2。返回 (id, 展示名)。
pub fn note_model(model_tier: &str) -> (&'static str, &'static str) {
    if model_tier == "pro" {
        (NOTE_MODEL_PRO_ID, "Qwen3.6-27B")
    } else {
        (NOTE_MODEL_ID, "DeepSeek-V3.2")
    }
}

/// 生成归档/重生成笔记（管线与 regenerate_note 共用）。
/// system_template 来自设置页（db::get_prompt "note"），{material_instruction}
/// 占位符在有课件时替换为课件指引、无课件时替换为空。
/// 返回（笔记正文, 输入 tokens, 缓存命中 tokens, 输出 tokens, 实际 system, 实际 user）。
pub async fn generate_note(
    client: &Client,
    key: &str,
    transcript: &str,
    material_ref: &str,
    model_tier: &str,
    system_template: &str,
) -> AppResult<(String, i64, i64, i64, String, String)> {
    let (model_id, _) = note_model(model_tier);
    let has_material = !material_ref.trim().is_empty();
    let system = system_template.replace(
        "{material_instruction}",
        if has_material { NOTE_MATERIAL_SUFFIX } else { "" },
    );
    let mut user =
        format!("课堂双语文本（笔记以它为主线）：\n{}", truncate_chars(transcript, MAX_INPUT_CHARS));
    if has_material {
        user.push_str("\n\n课件参考（用它的章节结构组织笔记）：\n");
        user.push_str(material_ref);
    }
    let out = chat(client, key, model_id, &system, &user).await?;
    Ok((
        out.content,
        out.prompt_tokens,
        out.cached_tokens,
        out.completion_tokens,
        system,
        user,
    ))
}

/* -------------------------------- 小工具 -------------------------------- */

/// 收集录音分块（仅 chunk_*.wav），按序截断到 MAX_ASR_CHUNKS。
fn collect_chunks(dir: &Path) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with("chunk_") && n.ends_with(".wav")
                        })
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list.truncate(MAX_ASR_CHUNKS);
    list
}

/// 归档起始时间 + 分钟偏移 → RFC3339。
fn minute_ts(start_at: &str, mins: i64) -> String {
    let start_ms = chrono::DateTime::parse_from_rfc3339(start_at)
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|_| chrono::Local::now().timestamp_millis());
    chrono::DateTime::from_timestamp_millis(start_ms + mins * 60_000)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| start_at.to_string())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// 硅基流动官方单价（元 / 百万 token，2026-10 价格中心）：
/// 返回 (普通输入单价, 输出单价, 缓存命中输入单价)。
/// 未知模型返回 None（调用方按 0 元处理，避免凭空计费）。
pub fn model_price(model: &str) -> Option<(f64, f64, f64)> {
    match model {
        // https://siliconflow.cn/pricing
        "GLM-4.5-Air" | "GLM-4.5V" => Some((1.0, 6.0, 1.0)),
        "DeepSeek-V3.2" => Some((4.0, 6.0, 0.4)),
        "Qwen3.6-27B" => Some((3.0, 18.0, 3.0)),
        // 豆包同声传译大模型 2.0：输入音频 80 元/百万 token、输出文本 80 元/百万 token
        // （S2T 模式无音频输出；无缓存概念，缓存价取输入价占位）
        // https://docs.volcengine.com/docs/DoubaoVoice/Billinginstructions-21
        "Doubao-AST-2.0" => Some((80.0, 80.0, 80.0)),
        _ => None,
    }
}

/// 按真实 token 用量计算单次调用花费（元），保留 4 位小数（最小 0.0001 元）。
pub fn token_amount(model: &str, input_tokens: i64, cached_tokens: i64, output_tokens: i64) -> f64 {
    let Some((p_in, p_out, p_cache)) = model_price(model) else {
        return 0.0;
    };
    let normal_in = (input_tokens - cached_tokens).max(0) as f64;
    let yuan = normal_in * p_in / 1_000_000.0
        + cached_tokens.max(0) as f64 * p_cache / 1_000_000.0
        + output_tokens.max(0) as f64 * p_out / 1_000_000.0;
    (yuan * 10_000.0).round() / 10_000.0
}

/* ============================ Mock 管线 ============================ */

/// 归档时调用：根据课堂时长 + 模型档位生成用量明细并落库。
pub async fn run_mock_pipeline(
    pool: &SqlitePool,
    session_id: i64,
    duration_sec: i64,
    model_tier: &str,
    start_at: &str,
) -> AppResult<()> {
    let records = generate_mock_usage(session_id, duration_sec, model_tier, start_at);
    db::batch_create_usage(pool, &records).await?;
    Ok(())
}

/// 确定性生成：与前端 mock 的 `generateUsage()` 口径一致。
/// 笔记改为手动生成后，mock 归档不再产生 note 用量行。
pub fn generate_mock_usage(
    session_id: i64,
    duration_sec: i64,
    model_tier: &str,
    start_at: &str,
) -> Vec<UsageInsert> {
    let start_ms = chrono::DateTime::parse_from_rfc3339(start_at)
        .map(|d| d.timestamp_millis())
        .unwrap_or(chrono::Local::now().timestamp_millis());

    let fmt_ts = |mins_after: i64| -> String {
        let ms = start_ms + mins_after * 60_000;
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| start_at.to_string())
    };

    let mut out: Vec<UsageInsert> = Vec::new();
    let dur_min = (duration_sec as f64 / 60.0).floor() as i64;

    // ASR：SenseVoiceSmall，免费，用量 = 音频时长（秒）
    out.push(UsageInsert {
        session_id,
        ts: fmt_ts(0),
        stage: "asr".into(),
        model: "SenseVoiceSmall".into(),
        model_tier: "free".into(),
        input_tokens: 0,
        cached_tokens: 0,
        output_tokens: 0,
        audio_seconds: duration_sec,
        amount: 0.0,
    });

    // 翻译：每 5 分钟一条（金额按官方单价 × 模拟 token）
    let blocks = (dur_min / 5).max(0);
    for i in 0..blocks {
        out.push(UsageInsert {
            session_id,
            ts: fmt_ts(i * 5),
            stage: "translate".into(),
            model: "GLM-4.5-Air".into(),
            model_tier: model_tier.to_string(),
            input_tokens: 1200,
            cached_tokens: 0,
            output_tokens: 980,
            audio_seconds: 0,
            amount: token_amount("GLM-4.5-Air", 1200, 0, 980),
        });
    }

    // 检测：每 15 分钟一条
    let detects = (dur_min / 15).max(0);
    for i in 0..detects {
        out.push(UsageInsert {
            session_id,
            ts: fmt_ts(i * 15),
            stage: "detect".into(),
            model: "DeepSeek-V3.2".into(),
            model_tier: model_tier.to_string(),
            input_tokens: 520,
            cached_tokens: 260,
            output_tokens: 0,
            audio_seconds: 0,
            amount: token_amount("DeepSeek-V3.2", 520, 260, 0),
        });
    }

    out
}

/// 用最小请求验证 API Key 连通性（设置页「测试连接」）。
pub async fn test_api_key(key: &str) -> AppResult<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::new(format!("HTTP 客户端创建失败：{e}")))?;
    let out = chat(
        &client,
        key,
        TRANSLATE_MODEL_ID,
        "你是连接测试助手。",
        "请只回复四个字：连接成功",
    )
    .await?;
    Ok(if out.content.trim().is_empty() {
        "连接成功".to_string()
    } else {
        out.content
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一串原文按给定切片依次 feed，返回每次推送的增量与最终可见正文。
    fn run(pieces: &[&str]) -> (String, String) {
        let mut f = ThinkFilter::new();
        let mut streamed = String::new();
        for p in pieces {
            streamed.push_str(&f.feed(p));
        }
        let full = f.finish();
        (streamed, full)
    }

    #[test]
    fn think_filter_strips_complete_block() {
        let (streamed, full) = run(&["<thinking>推理过程</thinking>\n\n正文内容"]);
        assert_eq!(streamed, "正文内容");
        assert_eq!(full, "正文内容");
    }

    #[test]
    fn think_filter_handles_split_tags() {
        // 标签被逐字切片的极端情形：增量拼接必须等于最终可见文本
        let (streamed, full) = run(&["<thin", "king>推理", "过程</thin", "king>正文", "内容"]);
        assert_eq!(streamed, "正文内容");
        assert_eq!(full, "正文内容");
        assert_eq!(streamed, full);
    }

    #[test]
    fn think_filter_handles_partial_open_tag_prefix() {
        // "<" 单独到达时应暂存，不能立刻当作正文推送
        let (streamed, full) = run(&["你", "好<", "thinking>内部", "</thinking>哦"]);
        assert_eq!(streamed, "你好哦");
        assert_eq!(full, "你好哦");
    }

    #[test]
    fn think_filter_plain_text_untouched() {
        let (streamed, full) = run(&["普通", "回答", "，没有思维链"]);
        assert_eq!(streamed, "普通回答，没有思维链");
        assert_eq!(full, "普通回答，没有思维链");
    }

    #[test]
    fn think_filter_unclosed_think_dropped() {
        // 思维链未闭合（截断）时，其内容不得进入可见正文
        let (streamed, full) = run(&["正文开头。", "<thinking>没闭合的推理"]);
        assert_eq!(streamed, "正文开头。");
        assert_eq!(full, "正文开头。");
    }

    #[test]
    fn think_filter_incremental_matches_full() {
        let pieces = ["<thin", "king>", "分析中…", "</thin", "king>\n\n", "结论：", "完成"];
        let (streamed, full) = run(&pieces);
        assert_eq!(streamed, full);
        assert_eq!(full, "结论：完成");
    }

    #[test]
    fn strip_emoji_keeps_text_and_cjk() {
        // 同传模型偶发表情被剥离，文字/标点/CJK 保留
        assert_eq!(
            crate::db::strip_emoji("所以复制。😊你看不到过程"),
            "所以复制。你看不到过程"
        );
        assert_eq!(crate::db::strip_emoji("ok ⭐ test ✂️ end"), "ok  test  end");
        assert_eq!(crate::db::strip_emoji("正常中文，含标点！English 123."), "正常中文，含标点！English 123.");
        assert_eq!(crate::db::strip_emoji(""), "");
    }

    /// 重整指定课堂的整理稿（先删 tidy_passages 该课堂行，再用当前提示词全量重跑）。
    /// TIDY_REBUILD_SID=4 cargo test tidy_rebuild -- --ignored --nocapture
    /// 需真实 SF Key（DPAPI 读取）；只跑整理环节，不动笔记/行动项/说话人。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "真实 API 全量重整：TIDY_REBUILD_SID=4 cargo test tidy_rebuild -- --ignored --nocapture"]
    async fn tidy_rebuild() {
        let sid: i64 = std::env::var("TIDY_REBUILD_SID")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let data_dir = std::path::Path::new(&std::env::var("APPDATA").expect("无 APPDATA"))
            .join("com.deskmate.app");
        let pool = crate::db::init(&data_dir).await.expect("打开真实数据库失败");
        let store = crate::secrets::SecretsStore::load(&data_dir).expect("读取密钥失败");
        let key = store
            .get(crate::secrets::key::SILICONFLOW_API_KEY)
            .expect("未配置硅基流动 API Key");
        let sess = crate::db::get_session(&pool, sid)
            .await
            .expect("查询课堂失败")
            .unwrap_or_else(|| panic!("课堂 {sid} 不存在"));
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .expect("HTTP 客户端创建失败");
        // 预检：先用同一客户端打一次最小请求，区分「客户端连不上」与「整理调用本身失败」
        let t0 = std::time::Instant::now();
        let pre = client
            .post(format!("{SILICONFLOW_BASE}/chat/completions"))
            .bearer_auth(&key)
            .json(&serde_json::json!({
                "model": TRANSLATE_MODEL_ID,
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 5
            }))
            .send()
            .await;
        match pre {
            Ok(r) => println!("[preflight] 状态 {} 耗时 {:?}", r.status(), t0.elapsed()),
            Err(e) => println!("[preflight] 失败 耗时 {:?}：{}", t0.elapsed(), reqwest_err_chain(&e)),
        }
        let n = tidy_remaining(&client, &key, &pool, sid, &sess.model_tier, None)
            .await
            .expect("整理失败");
        println!("课堂 {sid} 整理完成，共整理 {n} 行字幕（prompt 已含润色与去表情口径）");
    }
}
