//! 经济实时字幕引擎（低成本分块模式）。
//!
//! 与 AST 2.0 同传互为替代，用户在录制中可实时切换：
//! - 监听采集线程写出的 10 秒 WAV 分块，逐块上传硅基流动 SenseVoiceSmall（免费）转写；
//! - 跨块做语句重组：10 秒边界会把一句话切成两段，未说完的尾部留到下一块拼回，
//!   只在语句完整（遇到句末标点）时才落库/翻译/推送；
//! - 完整语句批量翻译（GLM-4.5-Air），同样写入 transcript_lines 并推送 subtitle://line，
//!   前端与归档侧无感知；
//! - 每 60 秒对最近字幕跑一次行动项检测（复用 ast::run_detect）；
//! - 文稿整理轮询与 AST 引擎共用 ast::spawn_tidy_task。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};

use crate::ast::{
    self, SubtitleLineEvent, SubtitleStatusEvent, EVENT_SUBTITLE_LINE, EVENT_SUBTITLE_STATUS,
};
use crate::db;
use crate::error::AppResult;
use crate::pipeline::{self, UsageInsert};

/// 分块目录轮询间隔。
const POLL_SECS: u64 = 1;
/// 行动项检测间隔。
const DETECT_SECS: u64 = 60;
/// 检测上下文行数（与 AST 引擎一致）。
const DETECT_CONTEXT_LINES: usize = 24;
/// 跨块等待的未完成尾部最长字符数，超过则强制断句落库，避免长期挂起。
const PENDING_MAX_CHARS: usize = 800;

/// 经济引擎配置。
pub struct CheapConfig {
    pub session_id: i64,
    pub chunks_dir: PathBuf,
    pub sf_key: String,
    pub source_language: String,
    pub target_language: String,
    pub start_at: String,
    pub model_tier: String,
}

/// 经济引擎句柄（接口对齐 ast::AstHandle）。
pub struct CheapHandle {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    session_id: i64,
}

impl CheapHandle {
    pub fn spawn(app: AppHandle, pool: SqlitePool, cfg: CheapConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let sid = cfg.session_id;
        // 文稿整理后台轮询（与 AST 引擎相同）
        ast::spawn_tidy_task(
            stop.clone(),
            pool.clone(),
            sid,
            &cfg.model_tier,
            Some(cfg.sf_key.clone()),
        );
        let s = stop.clone();
        let a = alive.clone();
        tauri::async_runtime::spawn(async move {
            run(app, pool, cfg, s).await;
            a.store(false, Ordering::SeqCst);
        });
        Self {
            stop,
            alive,
            session_id: sid,
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
}

/* --------------------------------- 主循环 --------------------------------- */

async fn run(app: AppHandle, pool: SqlitePool, cfg: CheapConfig, stop: Arc<AtomicBool>) {
    let sid = cfg.session_id;
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[cheap] HTTP 客户端创建失败：{e}");
            return;
        }
    };

    let _ = app.emit(
        EVENT_SUBTITLE_STATUS,
        SubtitleStatusEvent {
            session_id: sid,
            message: "经济转写模式：每 10 秒更新字幕（免费转写 + 低价翻译）".into(),
        },
    );

    // 引擎启动时已存在（含正在写）的分块都视为「本模式启动前的录音」，不补处理：
    // 它们可能已由 AST 出过字幕，重复转写会产生重复行；代价仅是切换边界最多 10 秒字幕。
    let mut processed: HashSet<i64> = HashSet::new();
    let mut prev_sizes: HashMap<i64, u64> = HashMap::new();
    if let Ok(rd) = std::fs::read_dir(&cfg.chunks_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Some(seq) = chunk_seq(&p) {
                if let Ok(meta) = e.metadata() {
                    prev_sizes.insert(seq, meta.len());
                }
                processed.insert(seq);
            }
        }
    }

    // 字幕 seq 从库中最大值续接
    let mut next_seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(seq), -1) FROM transcript_lines WHERE session_id=?1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap_or(-1)
            + 1;

    let mut pending = String::new();
    let mut detect_buf: VecDeque<(String, Option<String>)> = VecDeque::new();
    let mut last_detect = Instant::now();

    let prompt_translate = db::get_prompt(&pool, "translate_batch")
        .await
        .unwrap_or_else(|_| pipeline::DEFAULT_PROMPT_TRANSLATE_BATCH.to_string())
        .replace("{speak_language}", &cfg.source_language)
        .replace("{daily_language}", &cfg.target_language);
    let prompt_detect = db::get_prompt(&pool, "detect")
        .await
        .unwrap_or_else(|_| pipeline::default_prompt("detect").to_string());

    loop {
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let snapshot = list_chunk_sizes(&cfg.chunks_dir);
        let max_seq = snapshot.keys().copied().max().unwrap_or(-1);
        let mut ready: Vec<(i64, PathBuf)> = Vec::new();
        for (&seq, &size) in &snapshot {
            if processed.contains(&seq) {
                continue;
            }
            // 写完判定：已有更大序号的块（本块必然已封盘），或文件大小连续两轮不变
            let complete = seq < max_seq || prev_sizes.get(&seq) == Some(&size);
            if complete {
                ready.push((seq, cfg.chunks_dir.join(format!("chunk_{seq:04}.wav"))));
            }
        }
        prev_sizes = snapshot;
        ready.sort();

        for (seq, path) in ready {
            processed.insert(seq);
            // 转写（静音块返回空串）
            let text = match pipeline::asr_chunk(&client, &cfg.sf_key, &path).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[cheap] 分块 {seq} 转写失败：{}", e.0);
                    continue;
                }
            };
            if text.trim().is_empty() {
                continue;
            }

            // 跨块语句重组：拼上未完成尾部，只切出完整句子
            append_with_spacing(&mut pending, &text);
            let (sentences, rest) = split_sentences(&pending);
            pending = rest.unwrap_or_default();
            if sentences.is_empty() {
                continue;
            }

            // 翻译本批完整句（编号批量调用）
            let translations =
                translate_sentences(&client, &cfg.sf_key, &pool, sid, &cfg.model_tier, &prompt_translate, &sentences)
                    .await
                    .unwrap_or_default();

            // 落库 + 推送（时间戳按块序号 ×10s，句内偏移按序号比例近似）
            for (i, en) in sentences.iter().enumerate() {
                let offset = ((i as i64) * 10 / sentences.len() as i64).min(9);
                let ts = pipeline::ts_after_seconds(&cfg.start_at, seq * 10 + offset);
                let zh = translations.get(&(i + 1)).cloned();
                if let Err(e) =
                    db::insert_transcript_line(&pool, sid, &ts, next_seq, en, zh.as_deref(), "").await
                {
                    eprintln!("[cheap] 字幕落库失败：{}", e.0);
                    continue;
                }
                let _ = app.emit(
                    EVENT_SUBTITLE_LINE,
                    SubtitleLineEvent {
                        session_id: sid,
                        seq: next_seq,
                        ts,
                        en: en.clone(),
                        zh: zh.clone(),
                        speaker: String::new(),
                    },
                );
                detect_buf.push_back((en.clone(), zh));
                while detect_buf.len() > DETECT_CONTEXT_LINES {
                    detect_buf.pop_front();
                }
                next_seq += 1;
            }
        }

        // 行动项检测：独立任务调用，不阻塞分块处理
        if last_detect.elapsed() >= Duration::from_secs(DETECT_SECS) && !detect_buf.is_empty() {
            last_detect = Instant::now();
            // 取走并清空缓冲：否则下一轮会把同样的字幕再检测一次，导致同一行动项重复弹窗
            let mut buf: VecDeque<(String, Option<String>)> = detect_buf.drain(..).collect();
            let app2 = app.clone();
            let pool2 = pool.clone();
            let client2 = client.clone();
            let key2 = cfg.sf_key.clone();
            let prompt2 = prompt_detect.clone();
            let tier2 = cfg.model_tier.clone();
            tauri::async_runtime::spawn(async move {
                ast::run_detect(&app2, &pool2, &client2, &key2, &prompt2, sid, &tier2, &mut buf).await;
            });
        }
    }

    // 停止时若还有未发出的半句，强制落库（尽量翻译），避免句尾丢失
    if !pending.trim().is_empty() {
        let last = pending.trim().to_string();
        let translations =
            translate_sentences(&client, &cfg.sf_key, &pool, sid, &cfg.model_tier, &prompt_translate, &[last.clone()])
                .await
                .unwrap_or_default();
        let ts = chrono::Local::now().to_rfc3339();
        let zh = translations.get(&1).cloned();
        if db::insert_transcript_line(&pool, sid, &ts, next_seq, &last, zh.as_deref(), "")
            .await
            .is_ok()
        {
            let _ = app.emit(
                EVENT_SUBTITLE_LINE,
                SubtitleLineEvent {
                    session_id: sid,
                    seq: next_seq,
                    ts,
                    en: last,
                    zh,
                    speaker: String::new(),
                },
            );
        }
    }
}

/* -------------------------------- 小工具 -------------------------------- */

fn chunk_seq(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_string_lossy();
    name.strip_prefix("chunk_")?
        .strip_suffix(".wav")?
        .parse::<i64>()
        .ok()
}

fn list_chunk_sizes(dir: &Path) -> HashMap<i64, u64> {
    let mut map = HashMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Some(seq) = chunk_seq(&p) {
                if let Ok(meta) = e.metadata() {
                    map.insert(seq, meta.len());
                }
            }
        }
    }
    map
}

/// 拼接文本：两侧都是 ASCII 字母数字时补空格（CJK 直接相连）。
fn append_with_spacing(buf: &mut String, text: &str) {
    let t = text.trim();
    if t.is_empty() {
        return;
    }
    if !buf.is_empty() {
        let last_ascii = buf.chars().last().is_some_and(|c| c.is_ascii_alphanumeric() || c == '\'');
        let first_ascii = t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '\'');
        if last_ascii && first_ascii {
            buf.push(' ');
        }
    }
    buf.push_str(t);
}

/// 句末标点切句，返回（完整句列表，未完成尾部）。
fn split_sentences(text: &str) -> (Vec<String>, Option<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let next = chars.get(i + 1);
        let boundary = match c {
            '。' | '！' | '？' | '；' | '…' => true,
            '.' | '!' | '?' => next.is_none_or(|n| n.is_whitespace() || *n == '"' || *n == '\''),
            '\n' => true,
            _ => false,
        };
        if boundary {
            let s = cur.trim().trim_end_matches('\n').trim();
            if !s.is_empty() {
                sentences.push(s.to_string());
            }
            cur.clear();
        }
    }
    let tail = cur.trim().to_string();
    if tail.chars().count() > PENDING_MAX_CHARS {
        // 长期等不到标点：强制断句
        sentences.push(tail);
        (sentences, None)
    } else if tail.is_empty() {
        (sentences, None)
    } else {
        (sentences, Some(tail))
    }
}

/// 编号批量翻译；返回 {序号: 译文}。失败返回空 map（保留原文行）。
async fn translate_sentences(
    client: &reqwest::Client,
    key: &str,
    pool: &SqlitePool,
    session_id: i64,
    model_tier: &str,
    tpl: &str,
    sentences: &[String],
) -> AppResult<HashMap<usize, String>> {
    let numbered = sentences
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!("逐行翻译为目标语言，保留编号：\n{numbered}");
    let out = pipeline::chat(client, key, pipeline::TRANSLATE_MODEL_ID, tpl, &user).await?;
    let _ = db::insert_llm_log(
        pool,
        db::LlmLogInsert {
            session_id: Some(session_id),
            stage: "translate".into(),
            model: pipeline::TRANSLATE_MODEL_LABEL.into(),
            system_prompt: Some(tpl.to_string()),
            user_prompt: Some(user.clone()),
            response: Some(out.content.clone()),
            ok: true,
            error: None,
            input_tokens: out.prompt_tokens,
            cached_tokens: out.cached_tokens,
            output_tokens: out.completion_tokens,
        },
    )
    .await;
    let map = pipeline::parse_numbered_lines(&out.content);
    let rec = UsageInsert {
        session_id,
        ts: chrono::Local::now().to_rfc3339(),
        stage: "translate".into(),
        model: pipeline::TRANSLATE_MODEL_LABEL.into(),
        model_tier: model_tier.to_string(),
        input_tokens: out.prompt_tokens,
        cached_tokens: out.cached_tokens,
        output_tokens: out.completion_tokens,
        audio_seconds: 0,
        amount: pipeline::token_amount(
            pipeline::TRANSLATE_MODEL_LABEL,
            out.prompt_tokens,
            out.cached_tokens,
            out.completion_tokens,
        ),
    };
    let _ = db::batch_create_usage(pool, std::slice::from_ref(&rec)).await;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_at_sentence_end() {
        let (s, tail) = split_sentences("Hello world. How are");
        assert_eq!(s, vec!["Hello world.".to_string()]);
        assert_eq!(tail.as_deref(), Some("How are"));
    }

    #[test]
    fn cjk_boundary_and_carry() {
        // 模拟 10 秒切块把一句话切成两段
        let (s1, t1) = split_sentences("今天我们讲线性代数的矩阵");
        assert!(s1.is_empty());
        assert_eq!(t1.as_deref(), Some("今天我们讲线性代数的矩阵"));
        // 下一块到来：残句拼回，整句在中文句号处定稿
        let mut combined = t1.unwrap();
        append_with_spacing(&mut combined, "乘法。下面看例题。");
        let (s2, t2) = split_sentences(&combined);
        assert_eq!(s2, vec!["今天我们讲线性代数的矩阵乘法。".to_string(), "下面看例题。".to_string()]);
        assert!(t2.is_none());
    }

    #[test]
    fn ascii_spacing_between_chunks() {
        // 英文块边界默认补空格（单词被边界切开时极少，翻译模型可容错；与 AST 策略一致）
        let mut buf = String::from("the meet");
        append_with_spacing(&mut buf, "ing starts now.");
        assert_eq!(buf, "the meet ing starts now.");

        let mut cjk = String::from("矩阵");
        append_with_spacing(&mut cjk, "乘法");
        assert_eq!(cjk, "矩阵乘法");
    }

    #[test]
    fn force_flush_long_tail() {
        let long = "a".repeat(PENDING_MAX_CHARS + 50);
        let (s, tail) = split_sentences(&long);
        assert_eq!(s.len(), 1);
        assert!(tail.is_none());
    }
}
