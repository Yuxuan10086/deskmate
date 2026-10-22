//! SQLite 数据层：连接初始化、建表迁移、与前端 IPC 契约对齐的 CRUD。

use std::path::Path;

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, Row, SqlitePool};

use crate::error::{AppError, AppResult};
use crate::models::*;

/* ------------------------------- 初始化/迁移 ------------------------------ */

pub async fn init(data_dir: &Path) -> AppResult<SqlitePool> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("deskmate.db");

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> AppResult<()> {
    const DDL: &[&str] = &[
        r#"CREATE TABLE IF NOT EXISTS courses (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            code        TEXT,
            location    TEXT,
            start_week  INTEGER NOT NULL,
            end_week    INTEGER NOT NULL,
            color       TEXT NOT NULL,
            slots       TEXT NOT NULL,
            created_at  TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS sessions (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            course_id     INTEGER,
            title         TEXT NOT NULL,
            lecture_no    INTEGER NOT NULL,
            start_at      TEXT NOT NULL,
            end_at        TEXT,
            weekday       INTEGER NOT NULL,
            week_no       INTEGER NOT NULL,
            start_minute  INTEGER NOT NULL,
            end_minute    INTEGER NOT NULL,
            location      TEXT,
            mode          TEXT NOT NULL,
            model_tier    TEXT NOT NULL,
            status        TEXT NOT NULL,
            duration_sec  INTEGER,
            cost_estimate REAL NOT NULL DEFAULT 0,
            note          TEXT,
            FOREIGN KEY (course_id) REFERENCES courses (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS materials (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            course_id   INTEGER NOT NULL,
            session_id  INTEGER,
            file_name   TEXT NOT NULL,
            file_type   TEXT NOT NULL,
            size_bytes  INTEGER NOT NULL,
            file_path   TEXT NOT NULL DEFAULT '',
            confidence  TEXT NOT NULL,
            category    TEXT NOT NULL DEFAULT 'other',
            created_at  TEXT NOT NULL,
            FOREIGN KEY (course_id)  REFERENCES courses  (id) ON DELETE CASCADE,
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE SET NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS usage_records (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id    INTEGER,
            ts            TEXT NOT NULL,
            stage         TEXT NOT NULL,
            model         TEXT NOT NULL,
            model_tier    TEXT NOT NULL,
            input_tokens  INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            audio_seconds INTEGER NOT NULL DEFAULT 0,
            amount        REAL NOT NULL DEFAULT 0,
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS settings (
            id                    INTEGER PRIMARY KEY CHECK (id = 1),
            semester_start        TEXT NOT NULL,
            monthly_budget        REAL NOT NULL,
            speak_language        TEXT NOT NULL,
            daily_language        TEXT NOT NULL,
            presence_interval_sec INTEGER NOT NULL,
            presence_camera_enabled INTEGER NOT NULL DEFAULT 1,
            schedule_start_hour   INTEGER NOT NULL DEFAULT 8,
            schedule_end_hour     INTEGER NOT NULL DEFAULT 18,
            mic_gain              REAL NOT NULL DEFAULT 1.0
        )"#,
        r#"CREATE TABLE IF NOT EXISTS presence_events (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER,
            ts         TEXT NOT NULL,
            state      TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS transcript_lines (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            ts         TEXT NOT NULL,
            seq        INTEGER NOT NULL,
            en_text    TEXT NOT NULL,
            zh_text    TEXT,
            speaker    TEXT NOT NULL DEFAULT '',
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS action_items (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id     INTEGER NOT NULL,
            ts             TEXT NOT NULL,
            item_type      TEXT NOT NULL,
            content        TEXT NOT NULL,
            action_guide   TEXT,
            urgency        TEXT,
            ddl            TEXT,
            added_to_tasks INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS prompt_templates (
            task     TEXT PRIMARY KEY,
            template TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS tidy_passages (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id INTEGER NOT NULL,
            from_seq   INTEGER NOT NULL,
            to_seq     INTEGER NOT NULL,
            ord        INTEGER NOT NULL,
            en_text    TEXT NOT NULL,
            zh_text    TEXT,
            FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS llm_logs (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            ts            TEXT NOT NULL,
            session_id    INTEGER,
            stage         TEXT NOT NULL,
            model         TEXT NOT NULL,
            system_prompt TEXT,
            user_prompt   TEXT,
            response      TEXT,
            ok            INTEGER NOT NULL DEFAULT 1,
            error         TEXT,
            input_tokens  INTEGER NOT NULL DEFAULT 0,
            cached_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0
        )"#,
        r#"CREATE TABLE IF NOT EXISTS todos (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id     INTEGER,
            action_item_id INTEGER,
            source         TEXT NOT NULL DEFAULT 'manual',
            title          TEXT NOT NULL,
            detail         TEXT,
            ddl            TEXT,
            done           INTEGER NOT NULL DEFAULT 0,
            created_at     TEXT NOT NULL,
            completed_at   TEXT,
            FOREIGN KEY (session_id)     REFERENCES sessions (id) ON DELETE CASCADE,
            FOREIGN KEY (action_item_id) REFERENCES action_items (id) ON DELETE SET NULL
        )"#,
    ];
    for stmt in DDL {
        sqlx::query(stmt).execute(pool).await?;
    }

    // 旧库迁移必须在任何引用新列的语句之前：为旧 settings 表补列
    migrate_settings_columns(pool).await?;
    // sessions 表补软删除列（回收站）
    migrate_session_columns(pool).await?;
    // 附件分类 / 说话人 / 行动项 DDL 列
    migrate_v3_columns(pool).await?;

    // 自愈：行动项标记了「已加入」却没有对应待办（历史版本先置标记后建待办中断导致），恢复为未加入
    sqlx::query(
        "UPDATE action_items SET added_to_tasks=0
          WHERE added_to_tasks=1
            AND id NOT IN (SELECT DISTINCT action_item_id FROM todos WHERE action_item_id IS NOT NULL)",
    )
    .execute(pool)
    .await?;

    // 首启写入默认设置（学期起点按当前月份推断）
    let default_start = default_semester_start(Local::now().date_naive());
    sqlx::query(
        "INSERT OR IGNORE INTO settings
            (id, semester_start, monthly_budget, speak_language, daily_language,
             presence_interval_sec, presence_camera_enabled, schedule_start_hour, schedule_end_hour, mic_gain)
         VALUES (1, ?1, 30, 'en', 'zh', 10, 1, 8, 18, 1.0)",
    )
    .bind(default_start.format("%Y-%m-%d").to_string())
    .execute(pool)
    .await?;

    Ok(())
}

/// 为旧版本 settings 表补充新增列
/// （presence_camera_enabled / schedule_start_hour / schedule_end_hour / mic_gain）。
async fn migrate_settings_columns(pool: &SqlitePool) -> AppResult<()> {
    // 直接用 PRAGMA（兼容所有 SQLite 版本），检查列是否已存在
    let rows = sqlx::query("PRAGMA table_info(settings)")
        .fetch_all(pool)
        .await?;
    let mut has_start = false;
    let mut has_end = false;
    let mut has_gain = false;
    let mut has_camera = false;
    let mut has_persona = false;
    for row in &rows {
        let name: String = row.try_get("name")?;
        match name.as_str() {
            "schedule_start_hour" => has_start = true,
            "schedule_end_hour" => has_end = true,
            "mic_gain" => has_gain = true,
            "presence_camera_enabled" => has_camera = true,
            "persona" => has_persona = true,
            _ => {}
        }
    }
    if !has_start {
        sqlx::query("ALTER TABLE settings ADD COLUMN schedule_start_hour INTEGER NOT NULL DEFAULT 8")
            .execute(pool)
            .await?;
    }
    if !has_end {
        sqlx::query("ALTER TABLE settings ADD COLUMN schedule_end_hour INTEGER NOT NULL DEFAULT 18")
            .execute(pool)
            .await?;
    }
    if !has_gain {
        sqlx::query("ALTER TABLE settings ADD COLUMN mic_gain REAL NOT NULL DEFAULT 1.0")
            .execute(pool)
            .await?;
    }
    if !has_camera {
        sqlx::query(
            "ALTER TABLE settings ADD COLUMN presence_camera_enabled INTEGER NOT NULL DEFAULT 1",
        )
        .execute(pool)
        .await?;
    }
    if !has_persona {
        sqlx::query("ALTER TABLE settings ADD COLUMN persona TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 为 sessions 表补充软删除列（deleted_at：NULL=正常，RFC3339 时间=已进回收站）。
async fn migrate_session_columns(pool: &SqlitePool) -> AppResult<()> {
    let rows = sqlx::query("PRAGMA table_info(sessions)").fetch_all(pool).await?;
    let mut has_deleted = false;
    for row in &rows {
        let name: String = row.try_get("name")?;
        if name == "deleted_at" {
            has_deleted = true;
        }
    }
    if !has_deleted {
        sqlx::query("ALTER TABLE sessions ADD COLUMN deleted_at TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 第三批列迁移：
/// - materials.category（附件分类：slides 课件PPT / homework 作业题目 / other 其他附件）；
/// - transcript_lines.speaker（说话人标注：'' 未标注 / A、B… AST 原始说话人 / teacher / other）；
/// - action_items.ddl（截止时间，RFC3339 或日期字符串，NULL=未知）。
async fn migrate_v3_columns(pool: &SqlitePool) -> AppResult<()> {
    let mat_cols = columns_of(pool, "materials").await?;
    if !mat_cols.iter().any(|c| c == "category") {
        // 历史附件全是 PPT 课件，默认归类为 slides
        sqlx::query("ALTER TABLE materials ADD COLUMN category TEXT NOT NULL DEFAULT 'slides'")
            .execute(pool)
            .await?;
    }

    let line_cols = columns_of(pool, "transcript_lines").await?;
    if !line_cols.iter().any(|c| c == "speaker") {
        sqlx::query("ALTER TABLE transcript_lines ADD COLUMN speaker TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
    }

    let item_cols = columns_of(pool, "action_items").await?;
    if !item_cols.iter().any(|c| c == "ddl") {
        sqlx::query("ALTER TABLE action_items ADD COLUMN ddl TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 读取某表的全部列名（PRAGMA table_info 兼容所有 SQLite 版本）。
async fn columns_of(pool: &SqlitePool, table: &str) -> AppResult<Vec<String>> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| r.try_get::<String, _>("name")).collect::<Result<_, _>>()?)
}

/// 推断本学期第一周周一：8–12 月取 9/1 所在周周一，其余月份取 2/20 所在周周一。
fn default_semester_start(today: NaiveDate) -> NaiveDate {
    let (m, d) = if (8..=12).contains(&today.month()) {
        (9u32, 1u32)
    } else {
        (2u32, 20u32)
    };
    let anchor = NaiveDate::from_ymd_opt(today.year(), m, d).unwrap_or(today);
    let offset = anchor.weekday().num_days_from_monday() as i64; // 周一=0
    anchor - Duration::days(offset)
}

/* -------------------------------- Row 映射 -------------------------------- */

#[derive(FromRow)]
struct CourseRow {
    id: i64,
    name: String,
    code: Option<String>,
    location: Option<String>,
    start_week: i64,
    end_week: i64,
    color: String,
    slots: String,
    created_at: String,
}

impl CourseRow {
    fn into_dto(self) -> CourseDto {
        let slots: Vec<SlotDto> =
            serde_json::from_str(&self.slots).unwrap_or_else(|_| Vec::new());
        CourseDto {
            id: self.id,
            name: self.name,
            code: self.code,
            location: self.location,
            start_week: self.start_week,
            end_week: self.end_week,
            color: self.color,
            slots,
            created_at: self.created_at,
        }
    }
}

#[derive(FromRow)]
struct SessionRow {
    id: i64,
    course_id: Option<i64>,
    title: String,
    lecture_no: i64,
    start_at: String,
    end_at: Option<String>,
    weekday: i64,
    week_no: i64,
    start_minute: i64,
    end_minute: i64,
    location: Option<String>,
    mode: String,
    model_tier: String,
    status: String,
    duration_sec: Option<i64>,
    cost_estimate: f64,
    note: Option<String>,
    deleted_at: Option<String>,
}

impl SessionRow {
    fn into_dto(self) -> SessionDto {
        SessionDto {
            id: self.id,
            course_id: self.course_id,
            title: self.title,
            lecture_no: self.lecture_no,
            start_at: self.start_at,
            end_at: self.end_at,
            weekday: self.weekday,
            week_no: self.week_no,
            start_minute: self.start_minute,
            end_minute: self.end_minute,
            location: self.location,
            mode: self.mode,
            model_tier: self.model_tier,
            status: self.status,
            duration_sec: self.duration_sec,
            cost_estimate: self.cost_estimate,
            note: self.note,
            deleted_at: self.deleted_at,
        }
    }
}

/* --------------------------------- 课程 ---------------------------------- */

pub async fn list_courses(pool: &SqlitePool) -> AppResult<Vec<CourseDto>> {
    let rows: Vec<CourseRow> =
        sqlx::query_as("SELECT * FROM courses ORDER BY created_at ASC")
            .fetch_all(pool)
            .await?;
    let out: Vec<CourseDto> = rows.into_iter().map(CourseRow::into_dto).collect();
    Ok(out)
}

pub async fn create_course(pool: &SqlitePool, input: CourseInputDto) -> AppResult<CourseDto> {
    validate_slots(&input.slots)?;
    if !(1..=30).contains(&input.start_week) || input.end_week < input.start_week {
        return Err(AppError::new("起止周不合法"));
    }
    let slots_json = serde_json::to_string(&input.slots)?;
    let color = input.color.unwrap_or_else(|| "#4f6df5".to_string());
    let created_at = Local::now().to_rfc3339();

    let res = sqlx::query(
        "INSERT INTO courses (name, code, location, start_week, end_week, color, slots, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&input.name)
    .bind(empty_to_null(&input.code))
    .bind(empty_to_null(&input.location))
    .bind(input.start_week)
    .bind(input.end_week)
    .bind(&color)
    .bind(&slots_json)
    .bind(&created_at)
    .execute(pool)
    .await?;

    get_course(pool, res.last_insert_rowid()).await
}

pub async fn update_course(
    pool: &SqlitePool,
    id: i64,
    input: CourseInputDto,
) -> AppResult<CourseDto> {
    validate_slots(&input.slots)?;
    if !(1..=30).contains(&input.start_week) || input.end_week < input.start_week {
        return Err(AppError::new("起止周不合法"));
    }
    let slots_json = serde_json::to_string(&input.slots)?;
    let affected = sqlx::query(
        "UPDATE courses
            SET name=?1, code=?2, location=?3, start_week=?4, end_week=?5, color=?6, slots=?7
          WHERE id=?8",
    )
    .bind(&input.name)
    .bind(empty_to_null(&input.code))
    .bind(empty_to_null(&input.location))
    .bind(input.start_week)
    .bind(input.end_week)
    .bind(input.color.unwrap_or_else(|| "#4f6df5".to_string()))
    .bind(&slots_json)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(AppError::new("课程不存在"));
    }
    // 课表变更：按新 slots 重算该课程全部历史课堂的讲次
    recount_course_lectures(pool, id).await?;
    get_course(pool, id).await
}

pub async fn delete_course(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let affected = sqlx::query("DELETE FROM courses WHERE id=?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::new("课程不存在"));
    }
    Ok(())
}

async fn get_course(pool: &SqlitePool, id: i64) -> AppResult<CourseDto> {
    let row = sqlx::query_as::<_, CourseRow>("SELECT * FROM courses WHERE id=?1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(row.into_dto())
}

/* --------------------------------- 课堂 ---------------------------------- */

pub async fn list_sessions(
    pool: &SqlitePool,
    course_id: Option<i64>,
) -> AppResult<Vec<SessionDto>> {
    let rows: Vec<SessionRow> = match course_id {
        Some(cid) => {
            sqlx::query_as(
                "SELECT * FROM sessions WHERE course_id=?1 AND deleted_at IS NULL ORDER BY start_at DESC",
            )
            .bind(cid)
            .fetch_all(pool)
            .await?
        }
        None => sqlx::query_as("SELECT * FROM sessions WHERE deleted_at IS NULL ORDER BY start_at DESC")
            .fetch_all(pool)
            .await?,
    };
    Ok(rows.into_iter().map(SessionRow::into_dto).collect())
}

/// 回收站列表（已软删除的课堂）。
pub async fn list_deleted_sessions(pool: &SqlitePool) -> AppResult<Vec<SessionDto>> {
    let rows: Vec<SessionRow> =
        sqlx::query_as("SELECT * FROM sessions WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(SessionRow::into_dto).collect())
}

/// 全部课堂 (id, course_id)，含回收站——文件迁移/清理用。
pub async fn all_session_dirs(pool: &SqlitePool) -> AppResult<Vec<(i64, Option<i64>)>> {
    let rows = sqlx::query("SELECT id, course_id FROM sessions")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get("id"), r.get("course_id")))
        .collect())
}

/// 全部附件记录（文件迁移用）。
pub async fn list_all_materials(pool: &SqlitePool) -> AppResult<Vec<MaterialDto>> {
    let rows = sqlx::query(
        "SELECT id, course_id, session_id, file_name, file_type, size_bytes,
                file_path, confidence, category, created_at FROM materials",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(material_row).collect())
}

/// 软删除课堂：仅已归档课堂可删除，录制中的需先结束归档。
pub async fn soft_delete_session(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM sessions WHERE id=?1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match status.as_deref() {
        None => return Err(AppError::new("课堂不存在或已在回收站")),
        Some("finished") => {}
        Some(_) => return Err(AppError::new("录制中的课堂请先结束归档再删除")),
    }
    sqlx::query("UPDATE sessions SET deleted_at=?1 WHERE id=?2")
        .bind(Local::now().to_rfc3339())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 从回收站恢复课堂。
pub async fn restore_session(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let affected = sqlx::query("UPDATE sessions SET deleted_at=NULL WHERE id=?1 AND deleted_at IS NOT NULL")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::new("课堂不存在或不在回收站"));
    }
    Ok(())
}

/// 彻底删除课堂（含字幕/行动项/用量/日志/在位记录；录音目录由调用方清理）。
pub async fn purge_session(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    // llm_logs 无外键，需手动清理；其余表经 FK ON DELETE CASCADE 联动删除
    sqlx::query("DELETE FROM llm_logs WHERE session_id=?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let affected = sqlx::query("DELETE FROM sessions WHERE id=?1 AND deleted_at IS NOT NULL")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(AppError::new("课堂不存在或不在回收站"));
    }
    tx.commit().await?;
    Ok(())
}

pub async fn get_session(pool: &SqlitePool, id: i64) -> AppResult<Option<SessionDto>> {
    let row = sqlx::query_as::<_, SessionRow>("SELECT * FROM sessions WHERE id=?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(SessionRow::into_dto))
}

pub async fn create_session(
    pool: &SqlitePool,
    input: SessionInputDto,
) -> AppResult<SessionDto> {
    if input.mode != "online" && input.mode != "offline" {
        return Err(AppError::new("课堂模式不合法"));
    }
    if !matches!(
        input.model_tier.as_str(),
        "free" | "eco" | "balanced" | "pro" | "custom"
    ) {
        return Err(AppError::new("模型系列不合法"));
    }
    if !(1..=7).contains(&input.weekday) || input.start_minute >= input.end_minute {
        return Err(AppError::new("星期或时段不合法"));
    }

    // 课程存在性与地点
    let course_location: Option<String> = match input.course_id {
        Some(cid) => {
            let loc: Option<Option<String>> =
                sqlx::query_scalar("SELECT location FROM courses WHERE id=?1")
                    .bind(cid)
                    .fetch_optional(pool)
                    .await?;
            match loc {
                Some(v) => v,
                None => return Err(AppError::new("所选课程不存在")),
            }
        }
        None => None,
    };

    // 讲次：该课程已有课堂数 + 1（自由记录按 course_id IS NULL 计数，与 mock 一致）
    let prev: i64 = if let Some(cid) = input.course_id {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE course_id=?1")
            .bind(cid)
            .fetch_one(pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE course_id IS NULL")
            .fetch_one(pool)
            .await?
    };
    let lecture_no = prev + 1;

    // 日期 = 学期第一周周一 + (weekNo-1)*7 + (weekday-1)
    let settings = get_settings(pool).await?;
    let semester = NaiveDate::parse_from_str(&settings.semester_start, "%Y-%m-%d")?;
    let date = semester
        + Duration::days((input.week_no - 1) * 7 + (input.weekday - 1));

    let title = format!(
        "第 {} 讲 · {}-{:02}",
        lecture_no,
        date.month(),
        date.day()
    );
    let start_at = input
        .start_at
        .clone()
        .unwrap_or_else(|| local_datetime_iso(date, input.start_minute));
    let location = input.location.or(course_location);

    let res = sqlx::query(
        "INSERT INTO sessions
            (course_id, title, lecture_no, start_at, end_at, weekday, week_no,
             start_minute, end_minute, location, mode, model_tier, status,
             duration_sec, cost_estimate, note)
         VALUES
            (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'recording', NULL, 0, NULL)",
    )
    .bind(input.course_id)
    .bind(&title)
    .bind(lecture_no)
    .bind(&start_at)
    .bind(input.weekday)
    .bind(input.week_no)
    .bind(input.start_minute)
    .bind(input.end_minute)
    .bind(&location)
    .bind(&input.mode)
    .bind(&input.model_tier)
    .execute(pool)
    .await?;

    let dto = get_session(pool, res.last_insert_rowid())
        .await?
        .expect("刚插入的课堂必然存在");
    // 关联课程：讲次按课表实际排到的第 N 次课计算（刚插入的这条也一并校正）
    if let Some(cid) = input.course_id {
        recount_course_lectures(pool, cid).await?;
        return get_session(pool, dto.id)
            .await?
            .ok_or_else(|| AppError::new("课堂不存在"));
    }
    Ok(dto)
}

/// 「第 X 讲」= 按课表 slots，截至本次上课时刻，这门课已排到的第几次课。
/// 每周多次课（如周一/周三各一节）会连续编号：第3周周三 = 第5讲。
fn lecture_index(
    slots: &[SlotDto],
    start_week: i64,
    week_no: i64,
    weekday: i64,
    start_minute: i64,
) -> i64 {
    let mut n = 0i64;
    for s in slots {
        if week_no < start_week {
            continue;
        }
        // 从起始周到上一周的完整周次
        n += week_no - start_week;
        // 本周内排在本节之前（含本节）的 slot
        if s.weekday < weekday || (s.weekday == weekday && s.start_minute <= start_minute) {
            n += 1;
        }
    }
    n.max(1)
}

/// 课表变更/新建课堂后：按课程 slots 重算该课程所有历史 sessions 的 lecture_no 与标题前缀。
pub async fn recount_course_lectures(pool: &SqlitePool, course_id: i64) -> AppResult<()> {
    // 课程可能已被删除，此时其课堂会随外键级联删除，无需重算
    let Ok(course) = get_course(pool, course_id).await else {
        return Ok(());
    };
    let settings = get_settings(pool).await?;
    let semester = NaiveDate::parse_from_str(&settings.semester_start, "%Y-%m-%d")?;

    let rows = sqlx::query(
        "SELECT id, week_no, weekday, start_minute FROM sessions
          WHERE course_id=?1 ORDER BY week_no, weekday, start_minute, id",
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;

    for r in rows {
        let id: i64 = r.get("id");
        let week_no: i64 = r.get("week_no");
        let weekday: i64 = r.get("weekday");
        let start_minute: i64 = r.get("start_minute");
        let no = lecture_index(
            &course.slots,
            course.start_week,
            week_no,
            weekday,
            start_minute,
        );
        let date = semester + Duration::days((week_no - 1) * 7 + (weekday - 1));
        let title = format!(
            "第 {} 讲 · {}-{:02}",
            no,
            date.month(),
            date.day()
        );
        sqlx::query("UPDATE sessions SET lecture_no=?2, title=?3 WHERE id=?1")
            .bind(id)
            .bind(no)
            .bind(&title)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn finish_session(
    pool: &SqlitePool,
    id: i64,
    payload: FinishPayloadDto,
) -> AppResult<SessionDto> {
    // 先确认存在并拿到旧 note
    let existing = get_session(pool, id)
        .await?
        .ok_or_else(|| AppError::new("课堂不存在"))?;
    let note = payload.note.or(existing.note);
    let end_at = Local::now().to_rfc3339();

    // PRD 5.9 计费口径：总花费 = 用量明细 amount 求和（管线接入前为 0）
    let total: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount), 0.0) FROM usage_records WHERE session_id=?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "UPDATE sessions
            SET status='finished',
                duration_sec=COALESCE(duration_sec,0)+?2,
                note=?3, end_at=?4, cost_estimate=?5
          WHERE id=?1",
    )
    .bind(id)
    .bind(payload.duration_sec.max(0))
    .bind(&note)
    .bind(&end_at)
    .bind(total)
    .execute(pool)
    .await?;

    get_session(pool, id)
        .await?
        .ok_or_else(|| AppError::new("课堂不存在"))
}

/// 重新打开已归档课堂继续录制：status 回到 recording、清空 end_at；
/// 历史字幕/分块/duration_sec 均保留，再次归档时时长累加。
pub async fn resume_session(pool: &SqlitePool, id: i64) -> AppResult<SessionDto> {
    let existing = get_session(pool, id)
        .await?
        .ok_or_else(|| AppError::new("课堂不存在"))?;
    if existing.status != "finished" {
        return Err(AppError::new("仅已归档课堂可以继续录制"));
    }
    sqlx::query(
        "UPDATE sessions SET status='recording', end_at=NULL WHERE id=?1",
    )
    .bind(id)
    .execute(pool)
    .await?;

    get_session(pool, id)
        .await?
        .ok_or_else(|| AppError::new("课堂不存在"))
}

/* --------------------------------- 课件 ---------------------------------- */

pub async fn list_materials(pool: &SqlitePool, course_id: i64) -> AppResult<Vec<MaterialDto>> {
    let rows = sqlx::query(
        "SELECT id, course_id, session_id, file_name, file_type, size_bytes,
                file_path, confidence, category, created_at
           FROM materials WHERE course_id=?1 ORDER BY created_at DESC",
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(material_row).collect())
}

pub async fn create_material(
    pool: &SqlitePool,
    course_id: i64,
    session_id: Option<i64>,
    file_name: &str,
    file_type: &str,
    size_bytes: i64,
    file_path: &str,
    confidence: &str,
    category: &str,
) -> AppResult<MaterialDto> {
    let created_at = Local::now().to_rfc3339();
    let res = sqlx::query(
        "INSERT INTO materials (course_id, session_id, file_name, file_type, size_bytes,
                                file_path, confidence, category, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(course_id)
    .bind(session_id)
    .bind(file_name)
    .bind(file_type)
    .bind(size_bytes)
    .bind(file_path)
    .bind(confidence)
    .bind(category)
    .bind(&created_at)
    .execute(pool)
    .await?;

    let id = res.last_insert_rowid();
    let row = sqlx::query(
        "SELECT id, course_id, session_id, file_name, file_type, size_bytes,
                file_path, confidence, category, created_at FROM materials WHERE id=?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;

    Ok(material_row(&row))
}

/// 附件按 id 查询（打开/删除用）。
pub async fn get_material(pool: &SqlitePool, id: i64) -> AppResult<Option<MaterialDto>> {
    let row = sqlx::query(
        "SELECT id, course_id, session_id, file_name, file_type, size_bytes,
                file_path, confidence, category, created_at FROM materials WHERE id=?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(material_row))
}

/// 删除附件记录（文件由命令层删除）。
pub async fn delete_material(pool: &SqlitePool, id: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM materials WHERE id=?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 迁移附件文件后回填新路径。
pub async fn update_material_path(pool: &SqlitePool, id: i64, path: &str) -> AppResult<()> {
    sqlx::query("UPDATE materials SET file_path=?2 WHERE id=?1")
        .bind(id)
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
}

/// sqlx Row → MaterialDto（所有 SELECT 的统一映射）。
fn material_row(r: &sqlx::sqlite::SqliteRow) -> MaterialDto {
    MaterialDto {
        id: r.get("id"),
        course_id: r.get("course_id"),
        session_id: r.get("session_id"),
        file_name: r.get("file_name"),
        file_type: r.get("file_type"),
        size_bytes: r.get("size_bytes"),
        file_path: r.get("file_path"),
        confidence: r.get("confidence"),
        category: r.get("category"),
        created_at: r.get("created_at"),
    }
}

/* ------------------------------- 用量明细 -------------------------------- */

/// 批量插入用量明细（pipeline 归档时调用）。
pub async fn batch_create_usage(
    pool: &SqlitePool,
    records: &[crate::pipeline::UsageInsert],
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    for r in records {
        sqlx::query(
            r#"INSERT INTO usage_records
               (session_id, ts, stage, model, model_tier,
                input_tokens, cached_tokens, output_tokens, audio_seconds, amount)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
        )
        .bind(r.session_id)
        .bind(&r.ts)
        .bind(&r.stage)
        .bind(&r.model)
        .bind(&r.model_tier)
        .bind(r.input_tokens)
        .bind(r.cached_tokens)
        .bind(r.output_tokens)
        .bind(r.audio_seconds)
        .bind(r.amount)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_usage(pool: &SqlitePool) -> AppResult<Vec<UsageDto>> {
    let rows = sqlx::query(
        "SELECT id, session_id, ts, stage, model, model_tier,
                input_tokens, cached_tokens, output_tokens, audio_seconds, amount
           FROM usage_records ORDER BY ts DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| UsageDto {
            id: r.get("id"),
            session_id: r.get("session_id"),
            ts: r.get("ts"),
            stage: r.get("stage"),
            model: r.get("model"),
            model_tier: r.get("model_tier"),
            input_tokens: r.get("input_tokens"),
            cached_tokens: r.get("cached_tokens"),
            output_tokens: r.get("output_tokens"),
            audio_seconds: r.get("audio_seconds"),
            amount: r.get("amount"),
        })
        .collect())
}

/* --------------------------------- 设置 ---------------------------------- */

pub async fn get_settings(pool: &SqlitePool) -> AppResult<SettingsDto> {
    let row = sqlx::query(
        "SELECT semester_start, monthly_budget, speak_language, daily_language,
                presence_interval_sec, presence_camera_enabled, schedule_start_hour,
                schedule_end_hour, mic_gain, persona
           FROM settings WHERE id=1",
    )
    .fetch_one(pool)
    .await?;
    Ok(SettingsDto {
        semester_start: row.get("semester_start"),
        monthly_budget: row.get("monthly_budget"),
        speak_language: row.get("speak_language"),
        daily_language: row.get("daily_language"),
        presence_interval_sec: row.get("presence_interval_sec"),
        presence_camera_enabled: row.get::<i64, _>("presence_camera_enabled") != 0,
        schedule_start_hour: row.get("schedule_start_hour"),
        schedule_end_hour: row.get("schedule_end_hour"),
        mic_gain: row.get("mic_gain"),
        persona: row.get("persona"),
    })
}

pub async fn save_settings(
    pool: &SqlitePool,
    patch: SettingsPatchDto,
) -> AppResult<SettingsDto> {
    // UPSERT：以现有行为基础（不存在则用默认），仅覆盖传入的 Some 字段
    let current = get_settings(pool).await.unwrap_or(SettingsDto {
        semester_start: default_semester_start(Local::now().date_naive())
            .format("%Y-%m-%d")
            .to_string(),
        monthly_budget: 30.0,
        speak_language: "en".to_string(),
        daily_language: "zh".to_string(),
        presence_interval_sec: 10,
        presence_camera_enabled: true,
        schedule_start_hour: 8,
        schedule_end_hour: 18,
        mic_gain: 1.0,
        persona: None,
    });

    let merged = SettingsDto {
        semester_start: patch.semester_start.unwrap_or(current.semester_start),
        monthly_budget: patch.monthly_budget.unwrap_or(current.monthly_budget),
        speak_language: patch.speak_language.unwrap_or(current.speak_language),
        daily_language: patch.daily_language.unwrap_or(current.daily_language),
        presence_interval_sec: patch
            .presence_interval_sec
            .unwrap_or(current.presence_interval_sec),
        presence_camera_enabled: patch
            .presence_camera_enabled
            .unwrap_or(current.presence_camera_enabled),
        schedule_start_hour: patch
            .schedule_start_hour
            .unwrap_or(current.schedule_start_hour),
        schedule_end_hour: patch
            .schedule_end_hour
            .unwrap_or(current.schedule_end_hour),
        mic_gain: patch.mic_gain.unwrap_or(current.mic_gain),
        persona: patch.persona.or(current.persona),
    };

    // 课程表时间范围：整点、合法区间、跨度至少 1 小时
    if !(0..=23).contains(&merged.schedule_start_hour)
        || !(1..=24).contains(&merged.schedule_end_hour)
        || merged.schedule_start_hour >= merged.schedule_end_hour
    {
        return Err(AppError::new("课程表时间范围不合法（开始需早于结束）"));
    }

    sqlx::query(
        "INSERT INTO settings
            (id, semester_start, monthly_budget, speak_language, daily_language,
             presence_interval_sec, presence_camera_enabled, schedule_start_hour,
             schedule_end_hour, mic_gain, persona)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            semester_start=excluded.semester_start,
            monthly_budget=excluded.monthly_budget,
            speak_language=excluded.speak_language,
            daily_language=excluded.daily_language,
            presence_interval_sec=excluded.presence_interval_sec,
            presence_camera_enabled=excluded.presence_camera_enabled,
            schedule_start_hour=excluded.schedule_start_hour,
            schedule_end_hour=excluded.schedule_end_hour,
            mic_gain=excluded.mic_gain,
            persona=excluded.persona",
    )
    .bind(&merged.semester_start)
    .bind(merged.monthly_budget)
    .bind(&merged.speak_language)
    .bind(&merged.daily_language)
    .bind(merged.presence_interval_sec)
    .bind(merged.presence_camera_enabled)
    .bind(merged.schedule_start_hour)
    .bind(merged.schedule_end_hour)
    .bind(merged.mic_gain)
    .bind(&merged.persona)
    .execute(pool)
    .await?;

    Ok(merged)
}

/// 单独更新麦克风增益（录制面板滑块实时调整用）。
pub async fn set_mic_gain(pool: &SqlitePool, gain: f64) -> AppResult<()> {
    sqlx::query("UPDATE settings SET mic_gain=?1 WHERE id=1")
        .bind(gain)
        .execute(pool)
        .await?;
    Ok(())
}

/* ------------------------------- 文稿整理稿 -------------------------------- */

/// 写入一条整理稿段落。
pub async fn insert_tidy_passage(
    pool: &SqlitePool,
    session_id: i64,
    from_seq: i64,
    to_seq: i64,
    ord: i64,
    en: &str,
    zh: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO tidy_passages (session_id, from_seq, to_seq, ord, en_text, zh_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(session_id)
    .bind(from_seq)
    .bind(to_seq)
    .bind(ord)
    .bind(en)
    .bind(zh)
    .execute(pool)
    .await?;
    Ok(())
}

/// 某课堂的整理稿（按覆盖范围与段内顺序）。
pub async fn list_tidy_passages(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<Vec<TidyPassageDto>> {
    let rows = sqlx::query(
        "SELECT from_seq, to_seq, en_text, zh_text FROM tidy_passages
          WHERE session_id=?1 ORDER BY from_seq ASC, ord ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| TidyPassageDto {
            from_seq: r.get("from_seq"),
            to_seq: r.get("to_seq"),
            en: r.get("en_text"),
            zh: r.get("zh_text"),
        })
        .collect())
}

/// 整理稿已覆盖到的最大字幕 seq（未整理过则 None）。
pub async fn max_tidy_to_seq(pool: &SqlitePool, session_id: i64) -> AppResult<Option<i64>> {
    let row = sqlx::query("SELECT MAX(to_seq) AS m FROM tidy_passages WHERE session_id=?1")
        .bind(session_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<Option<i64>, _>("m"))
}

/* ----------------------------- 字幕关键词搜索 ----------------------------- */

/// 跨课堂按关键词搜索字幕（LIKE 匹配原文/译文），最多 40 条，供同桌工具调用。
pub async fn search_transcript(
    pool: &SqlitePool,
    keyword: &str,
    session_id: Option<i64>,
) -> AppResult<Vec<TranscriptSearchHitDto>> {
    let kw = format!("%{}%", keyword.replace('%', "").replace('_', ""));
    let rows = if let Some(sid) = session_id {
        sqlx::query(
            "SELECT l.session_id, s.title, l.ts, l.en_text, l.zh_text, l.speaker
               FROM transcript_lines l JOIN sessions s ON s.id = l.session_id
              WHERE l.session_id = ?1 AND (l.en_text LIKE ?2 OR l.zh_text LIKE ?2)
              ORDER BY l.seq ASC LIMIT 40",
        )
        .bind(sid)
        .bind(&kw)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT l.session_id, s.title, l.ts, l.en_text, l.zh_text, l.speaker
               FROM transcript_lines l JOIN sessions s ON s.id = l.session_id
              WHERE l.en_text LIKE ?1 OR l.zh_text LIKE ?1
              ORDER BY l.session_id DESC, l.seq ASC LIMIT 40",
        )
        .bind(&kw)
        .fetch_all(pool)
        .await?
    };
    Ok(rows
        .iter()
        .map(|r| TranscriptSearchHitDto {
            session_id: r.get("session_id"),
            session_title: r.get("title"),
            ts: r.get("ts"),
            en: r.get("en_text"),
            zh: r.get("zh_text"),
            speaker: r.get("speaker"),
        })
        .collect())
}

/* ------------------------------- 在位事件 -------------------------------- */

/// 记录一次在位状态切换（PRD 5.6）。
pub async fn insert_presence_event(
    pool: &SqlitePool,
    session_id: Option<i64>,
    ts: &str,
    state: &str,
) -> AppResult<()> {
    sqlx::query("INSERT INTO presence_events (session_id, ts, state) VALUES (?1, ?2, ?3)")
        .bind(session_id)
        .bind(ts)
        .bind(state)
        .execute(pool)
        .await?;
    Ok(())
}

/// 某课堂的在位事件（倒序），用于课堂详情展示。
pub async fn list_presence_events(pool: &SqlitePool, session_id: i64) -> AppResult<Vec<PresenceEventDto>> {
    let rows = sqlx::query(
        "SELECT ts, state FROM presence_events WHERE session_id=?1 ORDER BY ts DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| PresenceEventDto {
            ts: r.get("ts"),
            state: r.get("state"),
        })
        .collect())
}

/* --------------------------------- 辅助 ---------------------------------- */

fn validate_slots(slots: &[SlotDto]) -> AppResult<()> {
    if slots.is_empty() {
        return Err(AppError::new("至少需要一个上课时段"));
    }
    for s in slots {
        if !(1..=7).contains(&s.weekday) {
            return Err(AppError::new("星期取值不合法"));
        }
        if !(0..1440).contains(&s.start_minute)
            || !(0..=1440).contains(&s.end_minute)
            || s.start_minute >= s.end_minute
        {
            return Err(AppError::new("上课时段不合法"));
        }
    }
    Ok(())
}

fn empty_to_null(v: &Option<String>) -> Option<String> {
    v.clone().filter(|s| !s.trim().is_empty())
}

/// 本地时区下，某日期的指定分钟 → RFC3339。
fn local_datetime_iso(date: NaiveDate, minute: i64) -> String {
    let naive = date
        .and_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)
        .unwrap();
    Local
        .from_local_datetime(&naive)
        .single()
        .unwrap_or_else(|| Local::now())
        .to_rfc3339()
}

/* ---------------------------- 实时转写/行动项 ----------------------------- */

/// 剥离同传/翻译模型偶发输出的表情符号（emoji、装饰符号、变体选择符、零宽连接符）。
/// 它们不是讲话内容；保留文字、常规标点与 CJK 字符。
pub(crate) fn strip_emoji(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let u = *c as u32;
            !(u == 0xFE0F
                || u == 0x200D
                || (0x1F000..=0x1FAFF).contains(&u)
                || (0x2600..=0x27BF).contains(&u)
                || (0x2B00..=0x2BFF).contains(&u)
                || (0x1F1E6..=0x1F1FF).contains(&u))
        })
        .collect()
}

/// 写入一条实时字幕行，返回行 id。落库前剥离表情符号（AST 同传模型偶发在译文里加 😊 等）。
pub async fn insert_transcript_line(
    pool: &SqlitePool,
    session_id: i64,
    ts: &str,
    seq: i64,
    en: &str,
    zh: Option<&str>,
    speaker: &str,
) -> AppResult<i64> {
    let en = strip_emoji(en);
    let zh = zh.map(strip_emoji);
    let res = sqlx::query(
        "INSERT INTO transcript_lines (session_id, ts, seq, en_text, zh_text, speaker)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(session_id)
    .bind(ts)
    .bind(seq)
    .bind(en)
    .bind(zh)
    .bind(speaker)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// 说话人标注：批量回填 (seq, speaker)。归档后 LLM 聚类阶段使用。
pub async fn update_speakers(
    pool: &SqlitePool,
    session_id: i64,
    speaker_for_seq: &[(i64, &str)],
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    for (seq, speaker) in speaker_for_seq {
        sqlx::query(
            "UPDATE transcript_lines SET speaker=?3 WHERE session_id=?1 AND seq=?2",
        )
        .bind(session_id)
        .bind(seq)
        .bind(*speaker)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 批量翻译完成后回填译文。
pub async fn update_transcript_zh(pool: &SqlitePool, id: i64, zh: &str) -> AppResult<()> {
    sqlx::query("UPDATE transcript_lines SET zh_text=?2 WHERE id=?1")
        .bind(id)
        .bind(zh)
        .execute(pool)
        .await?;
    Ok(())
}

/// 某课堂的双语字幕行（按时间片升序）。
pub async fn list_transcript_lines(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<Vec<TranscriptLineDto>> {
    let rows = sqlx::query(
        "SELECT ts, seq, en_text, zh_text, speaker FROM transcript_lines
          WHERE session_id=?1 ORDER BY seq ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| TranscriptLineDto {
            ts: r.get("ts"),
            seq: r.get("seq"),
            en: r.get("en_text"),
            zh: r.get("zh_text"),
            speaker: r.get("speaker"),
        })
        .collect())
}

/// 写入一条实时检测的行动项，返回 id。
pub async fn insert_action_item(
    pool: &SqlitePool,
    session_id: i64,
    ts: &str,
    item_type: &str,
    content: &str,
    action_guide: &str,
    urgency: &str,
    ddl: Option<&str>,
) -> AppResult<i64> {
    let res = sqlx::query(
        "INSERT INTO action_items
            (session_id, ts, item_type, content, action_guide, urgency, ddl, added_to_tasks)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
    )
    .bind(session_id)
    .bind(ts)
    .bind(item_type)
    .bind(content)
    .bind(action_guide)
    .bind(urgency)
    .bind(ddl)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// 某课堂的行动项（按时间升序）。
pub async fn list_action_items(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<Vec<ActionItemDto>> {
    let rows = sqlx::query(
        "SELECT id, session_id, ts, item_type, content, action_guide, urgency, ddl, added_to_tasks
           FROM action_items WHERE session_id=?1 ORDER BY ts ASC, id ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(action_item_row).collect())
}

/// 标记行动项已加入课后任务。
pub async fn mark_action_item(pool: &SqlitePool, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE action_items SET added_to_tasks=1 WHERE id=?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 取消加入：删除该行动项对应的待办并把标记清零（幂等）。
pub async fn unmark_action_item(pool: &SqlitePool, action_item_id: i64) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM todos WHERE action_item_id=?1")
        .bind(action_item_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE action_items SET added_to_tasks=0 WHERE id=?1")
        .bind(action_item_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 手动补/改行动项 DDL。
pub async fn set_action_item_ddl(pool: &SqlitePool, id: i64, ddl: Option<&str>) -> AppResult<()> {
    sqlx::query("UPDATE action_items SET ddl=?2 WHERE id=?1")
        .bind(id)
        .bind(ddl)
        .execute(pool)
        .await?;
    Ok(())
}

/// 按 id 取行动项（加入待办/改 DDL 时取课堂信息用）。
pub async fn get_action_item(pool: &SqlitePool, id: i64) -> AppResult<Option<ActionItemDto>> {
    let row = sqlx::query(
        "SELECT id, session_id, ts, item_type, content, action_guide, urgency, ddl, added_to_tasks
           FROM action_items WHERE id=?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(action_item_row))
}

fn action_item_row(r: &sqlx::sqlite::SqliteRow) -> ActionItemDto {
    ActionItemDto {
        id: r.get("id"),
        session_id: r.get("session_id"),
        ts: r.get("ts"),
        item_type: r.get("item_type"),
        content: r.get("content"),
        action_guide: r.get("action_guide"),
        urgency: r.get("urgency"),
        ddl: r.get("ddl"),
        added_to_tasks: r.get::<i64, _>("added_to_tasks") != 0,
    }
}

/// 归档笔记覆盖写（regenerate_note 用）。
pub async fn update_session_note(pool: &SqlitePool, id: i64, note: &str) -> AppResult<()> {
    sqlx::query("UPDATE sessions SET note=?2 WHERE id=?1")
        .bind(id)
        .bind(note)
        .execute(pool)
        .await?;
    Ok(())
}

/// 用量明细汇总回填 cost_estimate（PRD 5.9：总花费=明细求和）。
pub async fn refresh_session_cost(pool: &SqlitePool, id: i64) -> AppResult<()> {
    let total: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount), 0.0) FROM usage_records WHERE session_id=?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    sqlx::query("UPDATE sessions SET cost_estimate=?2 WHERE id=?1")
        .bind(id)
        .bind(total)
        .execute(pool)
        .await?;
    Ok(())
}

/// 某课堂绑定的课件（regenerate_note 用）。
pub async fn list_session_materials(
    pool: &SqlitePool,
    session_id: i64,
) -> AppResult<Vec<MaterialDto>> {
    let rows = sqlx::query(
        "SELECT id, course_id, session_id, file_name, file_type, size_bytes,
                file_path, confidence, category, created_at
           FROM materials WHERE session_id=?1 ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(material_row).collect())
}

/* -------------------------------- 待办 ----------------------------------- */

pub async fn create_todo(
    pool: &SqlitePool,
    session_id: Option<i64>,
    action_item_id: Option<i64>,
    source: &str,
    title: &str,
    detail: Option<&str>,
    ddl: Option<&str>,
) -> AppResult<TodoDto> {
    let created_at = Local::now().to_rfc3339();
    let res = sqlx::query(
        "INSERT INTO todos (session_id, action_item_id, source, title, detail, ddl, done, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
    )
    .bind(session_id)
    .bind(action_item_id)
    .bind(source)
    .bind(title)
    .bind(detail)
    .bind(ddl)
    .bind(&created_at)
    .execute(pool)
    .await?;
    Ok(TodoDto {
        id: res.last_insert_rowid(),
        session_id,
        action_item_id,
        source: source.to_string(),
        title: title.to_string(),
        detail: detail.map(str::to_string),
        ddl: ddl.map(str::to_string),
        done: false,
        created_at,
        completed_at: None,
    })
}

/// 全部待办：未完成在前、DDL 未知最前、再按 DDL 升序、最后按创建时间。
pub async fn list_todos(pool: &SqlitePool) -> AppResult<Vec<TodoDto>> {
    let rows = sqlx::query(
        "SELECT id, session_id, action_item_id, source, title, detail, ddl,
                done, created_at, completed_at
           FROM todos
          ORDER BY (done=1),
                   CASE WHEN ddl IS NULL OR ddl='' THEN 0 ELSE 1 END,
                   ddl ASC,
                   created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(todo_row).collect())
}

pub async fn update_todo(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    detail: Option<&str>,
    ddl: Option<&str>,
) -> AppResult<()> {
    sqlx::query("UPDATE todos SET title=?2, detail=?3, ddl=?4 WHERE id=?1")
        .bind(id)
        .bind(title)
        .bind(detail)
        .bind(ddl)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_todo_done(pool: &SqlitePool, id: i64, done: bool) -> AppResult<()> {
    let completed = if done {
        Some(Local::now().to_rfc3339())
    } else {
        None
    };
    sqlx::query("UPDATE todos SET done=?2, completed_at=?3 WHERE id=?1")
        .bind(id)
        .bind(if done { 1 } else { 0 })
        .bind(completed)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_todo(pool: &SqlitePool, id: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM todos WHERE id=?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 行动项是否已有待办（重复点「加入」时幂等）。
pub async fn todo_by_action_item(
    pool: &SqlitePool,
    action_item_id: i64,
) -> AppResult<Option<TodoDto>> {
    let row = sqlx::query(
        "SELECT id, session_id, action_item_id, source, title, detail, ddl,
                done, created_at, completed_at
           FROM todos WHERE action_item_id=?1 LIMIT 1",
    )
    .bind(action_item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(todo_row))
}

fn todo_row(r: &sqlx::sqlite::SqliteRow) -> TodoDto {
    TodoDto {
        id: r.get("id"),
        session_id: r.get("session_id"),
        action_item_id: r.get("action_item_id"),
        source: r.get("source"),
        title: r.get("title"),
        detail: r.get("detail"),
        ddl: r.get("ddl"),
        done: r.get::<i64, _>("done") != 0,
        created_at: r.get("created_at"),
        completed_at: r.get("completed_at"),
    }
}

/* --------------------------- 提示词模板 --------------------------- */

/// 读取某任务的提示词模板；无用户覆盖时返回内置默认（pipeline::default_prompt）。
pub async fn get_prompt(pool: &SqlitePool, task: &str) -> AppResult<String> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT template FROM prompt_templates WHERE task=?1")
            .bind(task)
            .fetch_optional(pool)
            .await?;
    Ok(stored.unwrap_or_else(|| crate::pipeline::default_prompt(task).to_string()))
}

/// 所有用户覆盖过的模板（task, template）。
pub async fn list_prompt_overrides(pool: &SqlitePool) -> AppResult<Vec<(String, String)>> {
    let rows = sqlx::query("SELECT task, template FROM prompt_templates ORDER BY task")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get::<String, _>("task"), r.get::<String, _>("template")))
        .collect())
}

/// 保存某任务的模板覆盖。
pub async fn save_prompt_template(pool: &SqlitePool, task: &str, template: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO prompt_templates (task, template) VALUES (?1, ?2)
         ON CONFLICT(task) DO UPDATE SET template=excluded.template",
    )
    .bind(task)
    .bind(template)
    .execute(pool)
    .await?;
    Ok(())
}

/// 清空全部模板覆盖（恢复默认）。
pub async fn reset_prompt_templates(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("DELETE FROM prompt_templates").execute(pool).await?;
    Ok(())
}

/* --------------------------- 大模型调用日志 --------------------------- */

/// 单条调用日志（插入用，无 id/ts）。
pub struct LlmLogInsert {
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

/// 日志保留条数上限（超出后删最旧）。
const LLM_LOG_KEEP: i64 = 1000;
/// 单字段最长字符数（防止超长上下文把库撑大）。
const LLM_LOG_FIELD_MAX: usize = 8000;

fn clip_log_field(s: &str) -> String {
    if s.chars().count() <= LLM_LOG_FIELD_MAX {
        s.to_string()
    } else {
        s.chars().take(LLM_LOG_FIELD_MAX).collect::<String>() + "…[截断]"
    }
}

/// 写入一条调用日志并裁剪历史；失败仅打印不阻断主流程。
pub async fn insert_llm_log(pool: &SqlitePool, entry: LlmLogInsert) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO llm_logs
            (ts, session_id, stage, model, system_prompt, user_prompt, response,
             ok, error, input_tokens, cached_tokens, output_tokens)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
    )
    .bind(chrono::Local::now().to_rfc3339())
    .bind(entry.session_id)
    .bind(&entry.stage)
    .bind(&entry.model)
    .bind(entry.system_prompt.as_deref().map(clip_log_field))
    .bind(entry.user_prompt.as_deref().map(clip_log_field))
    .bind(entry.response.as_deref().map(clip_log_field))
    .bind(entry.ok)
    .bind(entry.error.as_deref().map(clip_log_field))
    .bind(entry.input_tokens)
    .bind(entry.cached_tokens)
    .bind(entry.output_tokens)
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM llm_logs WHERE id NOT IN
            (SELECT id FROM llm_logs ORDER BY id DESC LIMIT ?1)",
    )
    .bind(LLM_LOG_KEEP)
    .execute(pool)
    .await?;
    Ok(())
}

/// 最新 N 条日志（新的在前）。
pub async fn list_llm_logs(pool: &SqlitePool, limit: i64) -> AppResult<Vec<LlmLogDto>> {
    let rows = sqlx::query(
        "SELECT id, ts, session_id, stage, model, system_prompt, user_prompt, response,
                ok, error, input_tokens, cached_tokens, output_tokens
           FROM llm_logs ORDER BY id DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| LlmLogDto {
            id: r.get("id"),
            ts: r.get("ts"),
            session_id: r.get("session_id"),
            stage: r.get("stage"),
            model: r.get("model"),
            system_prompt: r.get("system_prompt"),
            user_prompt: r.get("user_prompt"),
            response: r.get("response"),
            ok: r.get("ok"),
            error: r.get("error"),
            input_tokens: r.get("input_tokens"),
            cached_tokens: r.get("cached_tokens"),
            output_tokens: r.get("output_tokens"),
        })
        .collect())
}

/// 清空调用日志。
pub async fn clear_llm_logs(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("DELETE FROM llm_logs").execute(pool).await?;
    Ok(())
}

