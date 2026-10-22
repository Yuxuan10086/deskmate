//! 用户文件目录结构（S10）：
//!
//! ```text
//! <data_dir>/courses/
//!   {courseId}/
//!     附件/                       <- 课程级附件（未绑定课堂）
//!     sessions/
//!       {sessionId}/
//!         课堂录音/               <- chunk wav、合并后的「课堂录音_段N.wav」
//!         文稿/                   <- 双语文本.md
//!         附件/                   <- 绑定课堂的课件 PPT / 作业题目 / 其他附件
//! ```
//!
//! 课程为空（手工课堂）的归入 `courses/_uncategorized/`。
//! 旧版 `captures/session_{id}/` 与 `materials/{courseId}/` 在启动时一次性迁移。

use std::path::{Path, PathBuf};
use sqlx::SqlitePool;

use crate::db;
use crate::error::AppResult;

const AUDIO_DIR: &str = "课堂录音";
const DOC_DIR: &str = "文稿";
const ATTACH_DIR: &str = "附件";
const NO_COURSE: &str = "_uncategorized";

fn course_folder(course_id: Option<i64>) -> String {
    course_id.map(|c| c.to_string()).unwrap_or_else(|| NO_COURSE.into())
}

pub fn courses_root(data_dir: &Path) -> PathBuf {
    data_dir.join("courses")
}

pub fn course_dir(data_dir: &Path, course_id: Option<i64>) -> PathBuf {
    courses_root(data_dir).join(course_folder(course_id))
}

pub fn course_attach_dir(data_dir: &Path, course_id: Option<i64>) -> PathBuf {
    course_dir(data_dir, course_id).join(ATTACH_DIR)
}

pub fn session_dir(data_dir: &Path, course_id: Option<i64>, session_id: i64) -> PathBuf {
    course_dir(data_dir, course_id)
        .join("sessions")
        .join(session_id.to_string())
}

pub fn audio_dir(data_dir: &Path, course_id: Option<i64>, session_id: i64) -> PathBuf {
    session_dir(data_dir, course_id, session_id).join(AUDIO_DIR)
}

pub fn doc_dir(data_dir: &Path, course_id: Option<i64>, session_id: i64) -> PathBuf {
    session_dir(data_dir, course_id, session_id).join(DOC_DIR)
}

pub fn session_attach_dir(data_dir: &Path, course_id: Option<i64>, session_id: i64) -> PathBuf {
    session_dir(data_dir, course_id, session_id).join(ATTACH_DIR)
}

/// 附件落盘目录：绑定课堂时进课堂附件夹，否则进课程附件夹。
pub fn attach_dir_for(
    data_dir: &Path,
    course_id: i64,
    session_id: Option<i64>,
) -> PathBuf {
    match session_id {
        Some(sid) => session_attach_dir(data_dir, Some(course_id), sid),
        None => course_attach_dir(data_dir, Some(course_id)),
    }
}

/// 目标路径已存在同名文件时追加 (1)/(2)…，避免覆盖。
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let target = dir.join(file_name);
    if !target.exists() {
        return target;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = Path::new(file_name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for i in 1..1000 {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    target
}

/// 启动迁移：旧 captures / materials 目录搬到新课程结构，并回填 DB 路径。
/// 全程尽力而为：单文件失败不阻断启动（录制目录可能正被占用）。
pub async fn migrate_legacy(pool: &SqlitePool, data_dir: &Path) {
    if let Err(e) = migrate_captures(pool, data_dir).await {
        eprintln!("[storage] 迁移录音目录失败：{e}");
    }
    if let Err(e) = migrate_materials(pool, data_dir).await {
        eprintln!("[storage] 迁移附件目录失败：{e}");
    }
}

async fn migrate_captures(pool: &SqlitePool, data_dir: &Path) -> AppResult<()> {
    let old_root = data_dir.join("captures");
    if !old_root.exists() {
        return Ok(());
    }
    let sessions = db::all_session_dirs(pool).await?;
    for (sid, course_id) in sessions {
        let old = old_root.join(format!("session_{sid}"));
        if !old.exists() {
            continue;
        }
        let new_audio = audio_dir(data_dir, course_id, sid);
        let new_doc = doc_dir(data_dir, course_id, sid);
        std::fs::create_dir_all(&new_audio).ok();
        std::fs::create_dir_all(&new_doc).ok();

        // 分块/整段 wav 搬到「课堂录音」，双语文本 md 搬到「文稿」
        if let Ok(entries) = std::fs::read_dir(&old) {
            for ent in entries.flatten() {
                let path = ent.path();
                let is_md = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("md"))
                    .unwrap_or(false);
                let target_dir = if is_md { &new_doc } else { &new_audio };
                let target = unique_path(target_dir, &ent.file_name().to_string_lossy());
                if std::fs::rename(&path, &target).is_err() {
                    // 跨卷或被占用时退回复制
                    if std::fs::copy(&path, &target).is_ok() {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&old);
    }
    // 旧 captures 根目录若已空则清掉
    let _ = std::fs::remove_dir(&old_root);
    Ok(())
}

async fn migrate_materials(pool: &SqlitePool, data_dir: &Path) -> AppResult<()> {
    let new_root = courses_root(data_dir);
    for m in db::list_all_materials(pool).await? {
        let path = PathBuf::from(&m.file_path);
        if m.file_path.is_empty() || !path.exists() {
            continue;
        }
        // 已在新结构内的不动
        if path.starts_with(&new_root) {
            continue;
        }
        let dir = attach_dir_for(data_dir, m.course_id, m.session_id);
        let _ = std::fs::create_dir_all(&dir);
        let target = unique_path(&dir, &m.file_name);
        if std::fs::rename(&path, &target).is_err() {
            if std::fs::copy(&path, &target).is_ok() {
                let _ = std::fs::remove_file(&path);
            } else {
                continue;
            }
        }
        db::update_material_path(pool, m.id, &target.to_string_lossy()).await?;
    }
    // 旧 materials 根：清空后删除
    let old_root = data_dir.join("materials");
    if old_root.exists() {
        if let Ok(entries) = std::fs::read_dir(&old_root) {
            let mut any = false;
            for ent in entries.flatten() {
                any = true;
                let p = ent.path();
                // 未被 DB 引用的孤儿文件搬到对应课程附件夹（找不到课程就放 _uncategorized）
                let folder = ent
                    .file_name()
                    .to_string_lossy()
                    .parse::<i64>()
                    .ok();
                let dir = course_attach_dir(data_dir, folder);
                let _ = std::fs::create_dir_all(&dir);
                if p.is_file() {
                    let target = unique_path(&dir, &ent.file_name().to_string_lossy());
                    let _ = std::fs::rename(&p, &target);
                }
            }
            if any {
                let _ = std::fs::remove_dir_all(&old_root);
            }
        }
        let _ = std::fs::remove_dir(&old_root);
    }
    Ok(())
}
