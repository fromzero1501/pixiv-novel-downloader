use calamine::{open_workbook_auto, Reader};
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::blocking::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::Emitter;

static PIXIV_SYNC_CANCELLATIONS: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();

fn pixiv_sync_cancelled(author_id: i64) -> bool {
    PIXIV_SYNC_CANCELLATIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|cancelled| cancelled.contains(&author_id))
        .unwrap_or(false)
}

fn clear_pixiv_sync_cancel(author_id: i64) {
    if let Some(cancelled) = PIXIV_SYNC_CANCELLATIONS.get() {
        let _ = cancelled.lock().map(|mut values| values.remove(&author_id));
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AuthorSummary {
    id: i64,
    name: String,
    homepage: String,
    avatar_path: String,
    notes: String,
    preview_dir: String,
    purchased_dir: String,
    match_threshold: i64,
    pixiv_last_sync_at: String,
    avatar_managed: bool,
    work_count: i64,
    purchased_count: i64,
    favorite_count: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AuthorInput {
    id: Option<i64>,
    name: String,
    homepage: String,
    avatar_path: String,
    avatar_managed: bool,
    notes: String,
    preview_dir: String,
    purchased_dir: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Work {
    author_id: i64,
    id: i64,
    title: String,
    release_date: String,
    preview_path: String,
    cover_path: String,
    purchased_path: String,
    favorite: bool,
    tags: String,
    pixiv_novel_id: String,
    series_id: String,
    series_title: String,
    series_order: i64,
    is_new: bool,
    author_name: String,
    word_count: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SeriesSummary {
    id: String,
    title: String,
    work_count: i64,
    purchased_count: i64,
    preview_count: i64,
    cover_path: String,
    max_order: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PixivAuthorProfile {
    id: Option<i64>,
    name: String,
    homepage: String,
    avatar_path: String,
    avatar_managed: bool,
    notes: String,
    preview_dir: String,
    purchased_dir: String,
    match_threshold: i64,
    pixiv_last_sync_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanPreviewResult {
    preview_count: usize,
    cover_count: usize,
    ambiguous_count: usize,
    created_count: usize,
    bound_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanPurchasedResult {
    bound_count: usize,
    selections: Vec<PurchasedSelection>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PurchasedSelection {
    path: String,
    candidates: Vec<WorkCandidate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkCandidate {
    work_id: i64,
    title: String,
    similarity: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportPreview {
    new_count: usize,
    duplicate_count: usize,
    invalid_count: usize,
    duplicates: Vec<String>,
}

#[derive(Serialize)]
struct ImportResult {
    created: usize,
    updated: usize,
    skipped: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    pixiv_cookie: String,
    excluded_tags: String,
    default_preview_dir: String,
    default_purchased_dir: String,
    auto_create_dirs: bool,
    minimum_file_size_bytes: u64,
    pixiv_delay_threshold: usize,
    pixiv_delay_seconds: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PixivSyncProgress {
    total: usize,
    current: usize,
    title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PixivSyncResult {
    downloaded_count: usize,
    skipped_existing_count: usize,
    skipped_date_count: usize,
    skipped_size_count: usize,
    failed_count: usize,
    cancelled: bool,
    last_sync_at: String,
}

struct PixivDownloadCandidate {
    novel_id: String,
    title: String,
    content: String,
    cover_url: String,
    release_date: String,
    tags: String,
    series_id: String,
    series_title: String,
    series_order: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CopyPreviewResult {
    copied_count: usize,
    bound_count: usize,
    skipped_count: usize,
}

fn app_data_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("无法定位程序目录")?.join("data");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn db() -> Result<Connection, String> {
    let path = app_data_dir()?.join("library.db");
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS authors (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL UNIQUE,
          homepage TEXT NOT NULL DEFAULT '',
          avatar_path TEXT NOT NULL DEFAULT '',
          notes TEXT NOT NULL DEFAULT '',
          preview_dir TEXT NOT NULL DEFAULT '',
          purchased_dir TEXT NOT NULL DEFAULT '',
          match_threshold INTEGER NOT NULL DEFAULT 70,
          pixiv_last_sync_at TEXT NOT NULL DEFAULT '',
          avatar_managed INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS works (
          id INTEGER PRIMARY KEY,
          author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
          title TEXT NOT NULL,
          release_date TEXT NOT NULL DEFAULT '',
          preview_path TEXT NOT NULL DEFAULT '',
          cover_path TEXT NOT NULL DEFAULT '',
          purchased_path TEXT NOT NULL DEFAULT '',
          favorite INTEGER NOT NULL DEFAULT 0,
          tags TEXT NOT NULL DEFAULT '',
          pixiv_novel_id TEXT NOT NULL DEFAULT '',
          series_id TEXT NOT NULL DEFAULT '',
          series_title TEXT NOT NULL DEFAULT '',
          series_order INTEGER NOT NULL DEFAULT 0,
          is_new INTEGER NOT NULL DEFAULT 0,
          UNIQUE(author_id, title, release_date)
        );
        CREATE TABLE IF NOT EXISTS series_catalog (
          author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE,
          id TEXT NOT NULL,
          title TEXT NOT NULL,
          PRIMARY KEY(author_id, id)
        );
        CREATE TABLE IF NOT EXISTS app_settings (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL DEFAULT ''
        );",
    )
    .map_err(|e| e.to_string())?;
    // Older portable libraries do not have this per-author setting yet.
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN match_threshold INTEGER NOT NULL DEFAULT 70",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN pixiv_last_sync_at TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN avatar_managed INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN tags TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN pixiv_novel_id TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN series_id TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN series_title TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN series_order INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN is_new INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS works_author_pixiv_novel_id ON works(author_id, pixiv_novel_id) WHERE pixiv_novel_id <> ''", []);
    conn.execute(
        "INSERT OR IGNORE INTO series_catalog (author_id, id, title) SELECT author_id, series_id, series_title FROM works WHERE series_id <> '' AND series_title <> ''",
        [],
    ).map_err(|e| e.to_string())?;
    Ok(conn)
}

fn setting(conn: &Connection, key: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key=?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
    .map(|value| value.unwrap_or_default())
}

fn read_settings(conn: &Connection) -> Result<AppSettings, String> {
    Ok(AppSettings {
        pixiv_cookie: setting(conn, "pixiv_cookie")?,
        excluded_tags: setting(conn, "excluded_tags")?,
        default_preview_dir: setting(conn, "default_preview_dir")?,
        default_purchased_dir: setting(conn, "default_purchased_dir")?,
        auto_create_dirs: setting(conn, "auto_create_dirs")? == "1",
        minimum_file_size_bytes: setting(conn, "minimum_file_size_bytes")?
            .parse()
            .unwrap_or(0),
        pixiv_delay_threshold: setting(conn, "pixiv_delay_threshold")?
            .parse()
            .unwrap_or(150),
        pixiv_delay_seconds: setting(conn, "pixiv_delay_seconds")?.parse().unwrap_or(1),
    })
}

fn safe_dir_name(value: &str) -> String {
    safe_sync_stem(value).trim_end_matches('.').to_string()
}

fn apply_default_dirs(conn: &Connection, author_id: i64) -> Result<(), String> {
    let settings = read_settings(conn)?;
    if !settings.auto_create_dirs {
        return Ok(());
    }
    let (name, preview, purchased): (String, String, String) = conn
        .query_row(
            "SELECT name, preview_dir, purchased_dir FROM authors WHERE id=?1",
            [author_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| e.to_string())?;
    let folder = safe_dir_name(&name);
    let preview_dir =
        if preview.trim().is_empty() && !settings.default_preview_dir.trim().is_empty() {
            let path = PathBuf::from(&settings.default_preview_dir).join(&folder);
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        } else {
            preview
        };
    let purchased_dir =
        if purchased.trim().is_empty() && !settings.default_purchased_dir.trim().is_empty() {
            let path = PathBuf::from(&settings.default_purchased_dir).join(&folder);
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        } else {
            purchased
        };
    conn.execute(
        "UPDATE authors SET preview_dir=?1, purchased_dir=?2 WHERE id=?3",
        params![preview_dir, purchased_dir, author_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn read_author(conn: &Connection, id: i64) -> Result<AuthorSummary, String> {
    conn.query_row(
        "SELECT a.id, a.name, a.homepage, a.avatar_path, a.notes, a.preview_dir, a.purchased_dir, a.match_threshold, a.pixiv_last_sync_at, a.avatar_managed,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.purchased_path <> ''),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.favorite = 1)
        FROM authors a WHERE a.id = ?1",
        [id],
        |row| Ok(AuthorSummary { id: row.get(0)?, name: row.get(1)?, homepage: row.get(2)?, avatar_path: row.get(3)?, notes: row.get(4)?, preview_dir: row.get(5)?, purchased_dir: row.get(6)?, match_threshold: row.get(7)?, pixiv_last_sync_at: row.get(8)?, avatar_managed: row.get::<_, i64>(9)? == 1, work_count: row.get(10)?, purchased_count: row.get(11)?, favorite_count: row.get(12)? })
    ).map_err(|e| e.to_string())
}

fn map_work(row: &rusqlite::Row<'_>) -> rusqlite::Result<Work> {
    Ok(Work {
        author_id: row.get(0)?,
        id: row.get(1)?,
        title: row.get(2)?,
        release_date: row.get(3)?,
        preview_path: row.get(4)?,
        cover_path: row.get(5)?,
        purchased_path: row.get(6)?,
        favorite: row.get::<_, i64>(7)? == 1,
        tags: row.get(8)?,
        pixiv_novel_id: row.get(9)?,
        series_id: row.get(10)?,
        series_title: row.get(11)?,
        series_order: row.get(12)?,
        is_new: row.get::<_, i64>(13)? == 1,
        author_name: row.get(14)?,
        word_count: None,
    })
}

fn text_word_count(path: &str) -> Option<usize> {
    if path.trim().is_empty() {
        return None;
    }
    fs::read_to_string(path).ok().map(|content| {
        content
            .chars()
            .filter(|character| !character.is_whitespace())
            .count()
    })
}

fn works_for_author(conn: &Connection, author_id: i64) -> Result<Vec<(i64, String)>, String> {
    let mut statement = conn
        .prepare("SELECT id, title FROM works WHERE author_id=?1")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([author_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn preview_works_for_author(
    conn: &Connection,
    author_id: i64,
) -> Result<Vec<(i64, String, String)>, String> {
    let mut statement = conn
        .prepare("SELECT id, title, release_date FROM works WHERE author_id=?1")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([author_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_authors() -> Result<Vec<AuthorSummary>, String> {
    let conn = db()?;
    let mut statement = conn.prepare(
        "SELECT a.id, a.name, a.homepage, a.avatar_path, a.notes, a.preview_dir, a.purchased_dir, a.match_threshold, a.pixiv_last_sync_at, a.avatar_managed,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.purchased_path <> ''),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.favorite = 1)
        FROM authors a ORDER BY a.name COLLATE NOCASE"
    ).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(AuthorSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                homepage: row.get(2)?,
                avatar_path: row.get(3)?,
                notes: row.get(4)?,
                preview_dir: row.get(5)?,
                purchased_dir: row.get(6)?,
                match_threshold: row.get(7)?,
                pixiv_last_sync_at: row.get(8)?,
                avatar_managed: row.get::<_, i64>(9)? == 1,
                work_count: row.get(10)?,
                purchased_count: row.get(11)?,
                favorite_count: row.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_author(author: AuthorInput) -> Result<AuthorSummary, String> {
    if author.name.trim().is_empty() {
        return Err("作者名称不能为空".into());
    }
    let conn = db()?;
    if !author.homepage.trim().is_empty() {
        let existing: Option<String> = conn
            .query_row(
                "SELECT name FROM authors WHERE homepage=?1 AND id <> COALESCE(?2, -1)",
                params![author.homepage.trim(), author.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(name) = existing {
            return Err(format!("作者主页已存在，名称为“{name}”"));
        }
    }
    let id = if let Some(id) = author.id {
        conn.execute("UPDATE authors SET name=?1, homepage=?2, avatar_path=?3, avatar_managed=?4, notes=?5, preview_dir=?6, purchased_dir=?7 WHERE id=?8", params![author.name.trim(), author.homepage.trim(), author.avatar_path, author.avatar_managed as i64, author.notes, author.preview_dir, author.purchased_dir, id]).map_err(|e| e.to_string())?;
        id
    } else {
        conn.execute("INSERT INTO authors (name, homepage, avatar_path, avatar_managed, notes, preview_dir, purchased_dir) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![author.name.trim(), author.homepage.trim(), author.avatar_path, author.avatar_managed as i64, author.notes, author.preview_dir, author.purchased_dir]).map_err(|e| e.to_string())?;
        conn.last_insert_rowid()
    };
    apply_default_dirs(&conn, id)?;
    read_author(&conn, id)
}

#[tauri::command]
fn delete_author(author_id: i64) -> Result<(), String> {
    let conn = db()?;
    let avatar: Option<(String, i64)> = conn
        .query_row(
            "SELECT avatar_path, avatar_managed FROM authors WHERE id=?1",
            [author_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM authors WHERE id=?1", [author_id])
        .map_err(|e| e.to_string())?;
    if let Some((path, managed)) = avatar {
        if managed == 1 && !path.is_empty() {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

#[tauri::command]
fn update_author_path(
    author_id: i64,
    field: String,
    path: String,
) -> Result<AuthorSummary, String> {
    let conn = db()?;
    let column = match field.as_str() {
        "preview" => "preview_dir",
        "purchased" => "purchased_dir",
        _ => return Err("不支持的路径类型".into()),
    };
    conn.execute(
        &format!("UPDATE authors SET {column} = ?1 WHERE id = ?2"),
        params![path, author_id],
    )
    .map_err(|e| e.to_string())?;
    read_author(&conn, author_id)
}

#[tauri::command]
fn set_match_threshold(author_id: i64, threshold: i64) -> Result<AuthorSummary, String> {
    if !(1..=100).contains(&threshold) {
        return Err("匹配相似度必须在 1 到 100 之间".into());
    }
    let conn = db()?;
    conn.execute(
        "UPDATE authors SET match_threshold=?1 WHERE id=?2",
        params![threshold, author_id],
    )
    .map_err(|e| e.to_string())?;
    read_author(&conn, author_id)
}

#[tauri::command]
fn get_app_settings() -> Result<AppSettings, String> {
    read_settings(&db()?)
}

#[tauri::command]
fn save_app_settings(settings: AppSettings) -> Result<AppSettings, String> {
    if settings.pixiv_delay_threshold == 0 {
        return Err("抓取数量阈值必须至少为 1".into());
    }
    if settings.pixiv_delay_seconds > 60 {
        return Err("抓取间隔不能超过 60 秒".into());
    }
    let conn = db()?;
    let values = [
        (
            "pixiv_cookie",
            normalize_pixiv_cookie(&settings.pixiv_cookie)?,
        ),
        ("excluded_tags", settings.excluded_tags),
        ("default_preview_dir", settings.default_preview_dir),
        ("default_purchased_dir", settings.default_purchased_dir),
        (
            "auto_create_dirs",
            if settings.auto_create_dirs {
                "1".into()
            } else {
                "0".into()
            },
        ),
        (
            "minimum_file_size_bytes",
            settings.minimum_file_size_bytes.to_string(),
        ),
        (
            "pixiv_delay_threshold",
            settings.pixiv_delay_threshold.to_string(),
        ),
        (
            "pixiv_delay_seconds",
            settings.pixiv_delay_seconds.to_string(),
        ),
    ];
    for (key, value) in values {
        conn.execute("INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value]).map_err(|e| e.to_string())?;
    }
    read_settings(&conn)
}

#[tauri::command]
fn read_pixiv_cookie_file(path: String) -> Result<String, String> {
    normalize_pixiv_cookie(&fs::read_to_string(path).map_err(|e| e.to_string())?)
}

#[tauri::command]
fn list_works(
    author_id: i64,
    query: String,
    search_field: String,
    status: String,
    favorites_only: bool,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let field = if search_field == "tags" {
        "tags"
    } else {
        "title"
    };
    let mut sql = format!("SELECT author_id, id, title, release_date, preview_path, cover_path, purchased_path, favorite, tags, pixiv_novel_id, series_id, series_title, series_order, is_new, '' AS author_name FROM works WHERE author_id = ?1 AND (?2 = '' OR {field} LIKE ?3)");
    match status.as_str() {
        "purchased" => sql.push_str(" AND purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND favorite = 1");
    }
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY release_date ASC, id ASC",
        "title_asc" => " ORDER BY title COLLATE NOCASE ASC",
        _ => " ORDER BY release_date DESC, id DESC",
    });
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let raw_query = query.trim();
    let rows = statement
        .query_map(
            params![author_id, raw_query, format!("%{raw_query}%")],
            map_work,
        )
        .map_err(|e| e.to_string())?;
    let mut works = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for work in &mut works {
        work.word_count = if !work.purchased_path.is_empty() {
            text_word_count(&work.purchased_path)
        } else {
            text_word_count(&work.preview_path)
        };
    }
    Ok(works)
}

#[tauri::command]
fn list_all_works(
    query: String,
    search_field: String,
    status: String,
    favorites_only: bool,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let field = if search_field == "tags" {
        "w.tags"
    } else {
        "w.title"
    };
    let mut sql = format!("SELECT w.author_id, w.id, w.title, w.release_date, w.preview_path, w.cover_path, w.purchased_path, w.favorite, w.tags, w.pixiv_novel_id, w.series_id, w.series_title, w.series_order, w.is_new, a.name AS author_name FROM works w JOIN authors a ON a.id=w.author_id WHERE (?1 = '' OR {field} LIKE ?2)");
    match status.as_str() {
        "purchased" => sql.push_str(" AND w.purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND w.purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND w.favorite = 1");
    }
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY w.release_date ASC, w.id ASC",
        "title_asc" => " ORDER BY w.title COLLATE NOCASE ASC",
        _ => " ORDER BY w.release_date DESC, w.id DESC",
    });
    let raw_query = query.trim();
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![raw_query, format!("%{raw_query}%")], map_work)
        .map_err(|e| e.to_string())?;
    let mut works = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for work in &mut works {
        work.word_count = if !work.purchased_path.is_empty() {
            text_word_count(&work.purchased_path)
        } else {
            text_word_count(&work.preview_path)
        };
    }
    Ok(works)
}

#[tauri::command]
fn list_series_works(author_id: i64, series_id: String) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT author_id, id, title, release_date, preview_path, cover_path, purchased_path, favorite, tags, pixiv_novel_id, series_id, series_title, series_order, is_new, '' AS author_name FROM works WHERE author_id=?1 AND series_id=?2 ORDER BY CASE WHEN series_order > 0 THEN 0 ELSE 1 END, series_order ASC, id ASC")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![author_id, series_id], map_work)
        .map_err(|e| e.to_string())?;
    let mut works = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for work in &mut works {
        work.word_count = if !work.purchased_path.is_empty() {
            text_word_count(&work.purchased_path)
        } else {
            text_word_count(&work.preview_path)
        };
    }
    Ok(works)
}

#[tauri::command]
fn list_series(author_id: i64) -> Result<Vec<SeriesSummary>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT s.id, s.title, COUNT(w.id), COALESCE(SUM(CASE WHEN w.purchased_path <> '' THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN w.purchased_path = '' THEN 1 ELSE 0 END), 0), COALESCE(MAX(NULLIF(w.cover_path, '')), ''), COALESCE(MAX(w.series_order), 0) FROM series_catalog s LEFT JOIN works w ON w.author_id=s.author_id AND w.series_id=s.id WHERE s.author_id=?1 GROUP BY s.id, s.title ORDER BY s.title COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([author_id], |row| {
            Ok(SeriesSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                work_count: row.get(2)?,
                purchased_count: row.get(3)?,
                preview_count: row.get(4)?,
                cover_path: row.get(5)?,
                max_order: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_work_series(
    author_id: i64,
    work_id: i64,
    series_id: String,
    series_order: i64,
) -> Result<(), String> {
    if series_order < 1 {
        return Err("系列序号必须从 1 开始".into());
    }
    let conn = db()?;
    let series_title: Option<String> = conn
        .query_row(
            "SELECT title FROM series_catalog WHERE author_id=?1 AND id=?2",
            params![author_id, series_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(series_title) = series_title else {
        return Err("所选系列不存在或已没有作品".into());
    };
    let occupied: Option<i64> = conn
        .query_row(
            "SELECT id FROM works WHERE author_id=?1 AND series_id=?2 AND series_order=?3 AND id<>?4",
            params![author_id, series_id, series_order, work_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if occupied.is_some() {
        return Err("该系列序号已有作品，请选择其他序号".into());
    }
    if conn
        .execute(
            "UPDATE works SET series_id=?1, series_title=?2, series_order=?3 WHERE id=?4 AND author_id=?5",
            params![series_id, series_title, series_order, work_id, author_id],
        )
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("作品不存在或不属于当前作者".into());
    }
    Ok(())
}

#[tauri::command]
fn leave_work_series(author_id: i64, work_id: i64) -> Result<(), String> {
    if db()?
        .execute(
            "UPDATE works SET series_id='', series_title='', series_order=0 WHERE id=?1 AND author_id=?2",
            params![work_id, author_id],
        )
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("作品不存在或不属于当前作者".into());
    }
    Ok(())
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn normalized_match_name(value: &str) -> Vec<char> {
    let value = name_key(value);
    let value = if value.len() >= 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
    {
        &value[10..]
    } else {
        &value
    };
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn similarity_percent(left: &str, right: &str) -> i64 {
    let left = normalized_match_name(left);
    let right = normalized_match_name(right);
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let mut previous = vec![0usize; right.len() + 1];
    let mut longest = 0usize;
    for left_character in &left {
        let mut current = vec![0usize; right.len() + 1];
        for (right_index, right_character) in right.iter().enumerate() {
            if left_character == right_character {
                current[right_index + 1] = previous[right_index] + 1;
                longest = longest.max(current[right_index + 1]);
            }
        }
        previous = current;
    }
    (longest * 100 / left.len().min(right.len())) as i64
}

fn name_key(value: &str) -> String {
    let value = value.trim();
    let extension = Path::new(value)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if ["txt", "jpg", "jpeg", "png", "webp"]
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        return Path::new(value)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(value)
            .trim()
            .to_string();
    }
    value.to_string()
}

fn pixiv_user_id(homepage: &str) -> Result<String, String> {
    let homepage = homepage.trim();
    if let Some((_, suffix)) = homepage.split_once("/users/") {
        let id: String = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    if let Some((_, suffix)) = homepage.split_once("id=") {
        let id: String = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    Err("请先在作者设置中填写有效的 Pixiv 作者主页链接。".into())
}

fn normalize_pixiv_cookie(raw: &str) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if let Ok(json) = serde_json::from_str::<Value>(raw) {
        let pairs: Vec<String> = json
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let domain = item
                    .get("domain")
                    .and_then(Value::as_str)
                    .unwrap_or(".pixiv.net");
                let name = item.get("name").and_then(Value::as_str)?;
                let value = item.get("value").and_then(Value::as_str)?;
                (domain.contains("pixiv.net") && name == "PHPSESSID")
                    .then(|| format!("{name}={value}"))
            })
            .collect();
        if !pairs.is_empty() {
            return Ok(pairs.join("; "));
        }
    }
    let pairs: Vec<&str> = raw
        .trim()
        .trim_start_matches("Cookie:")
        .split(';')
        .map(str::trim)
        .filter(|pair| pair.starts_with("PHPSESSID="))
        .collect();
    if !pairs.is_empty() {
        return Ok(pairs.join("; "));
    }
    Err("Cookie 中未找到 Pixiv 登录所需的 PHPSESSID。".into())
}

fn pixiv_client(cookie: Option<String>) -> Result<Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::REFERER,
        "https://www.pixiv.net/".parse().unwrap(),
    );
    headers.insert(
        reqwest::header::ACCEPT_LANGUAGE,
        "zh-CN,zh;q=0.9".parse().unwrap(),
    );
    if let Some(cookie) = cookie {
        headers.insert(
            reqwest::header::COOKIE,
            cookie
                .parse()
                .map_err(|e| format!("Cookie 格式无效：{e}"))?,
        );
    }
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36")
        .default_headers(headers)
        .build()
        .map_err(|e| e.to_string())
}

fn json_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn json_id_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| match item {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn json_i64(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|item| match item {
            Value::Number(value) => value.as_i64(),
            Value::String(value) => value.parse().ok(),
            _ => None,
        })
        .unwrap_or(0)
}

fn release_date(value: &str) -> Option<NaiveDate> {
    value
        .get(0..10)
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
}

fn is_within_date_range(value: &str, start: Option<NaiveDate>, end: Option<NaiveDate>) -> bool {
    let Some(date) = release_date(value) else {
        return start.is_none() && end.is_none();
    };
    !start.is_some_and(|bound| date < bound) && !end.is_some_and(|bound| date > bound)
}

fn is_after_last_sync(value: &str, last_sync: Option<DateTime<Utc>>) -> bool {
    let Some(last_sync) = last_sync else {
        return true;
    };
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc) > last_sync)
        .unwrap_or_else(|_| release_date(value).is_some_and(|date| date > last_sync.date_naive()))
}

// Pixiv's createDate is the original submission time. uploadDate can change when
// an author edits a work, so it must only be used as a fallback for older payloads.
fn pixiv_published_at(value: &Value) -> String {
    ["createDate", "uploadDate"]
        .iter()
        .map(|key| json_string(value, key))
        .find(|date| !date.is_empty())
        .unwrap_or_default()
}

fn parse_date_bound(value: &str, label: &str) -> Result<Option<NaiveDate>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| format!("{label}必须是 YYYY-MM-DD 格式。"))
}

fn safe_sync_stem(title: &str) -> String {
    let value: String = title
        .chars()
        .map(|character| {
            if matches!(
                character,
                '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            ) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let value = value
        .trim()
        .trim_matches('.')
        .chars()
        .take(120)
        .collect::<String>();
    if value.is_empty() {
        "Pixiv 小说".into()
    } else {
        value
    }
}

fn sync_paths(preview_dir: &Path, title: &str, novel_id: &str) -> (PathBuf, PathBuf) {
    let base = safe_sync_stem(title);
    let text = preview_dir.join(format!("{base}.txt"));
    let cover = preview_dir.join(format!("{base}.jpg"));
    if !text.exists() && !cover.exists() {
        return (text, cover);
    }
    (
        preview_dir.join(format!("{base}-{novel_id}.txt")),
        preview_dir.join(format!("{base}-{novel_id}.jpg")),
    )
}

fn fetch_pixiv_novel_detail(client: &Client, novel_id: &str) -> Result<Value, String> {
    client
        .get(format!(
            "https://www.pixiv.net/ajax/novel/{novel_id}?time={}",
            Utc::now().timestamp_millis()
        ))
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json())
        .map_err(|error| error.to_string())
}

fn fetch_pixiv_cover(client: &Client, cover_url: &str) -> Result<Vec<u8>, String> {
    client
        .get(cover_url)
        .header(reqwest::header::REFERER, "https://www.pixiv.net/")
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.bytes())
        .map(|bytes| bytes.to_vec())
        .map_err(|error| error.to_string())
}

fn existing_sync_target(
    conn: &Connection,
    author_id: i64,
    novel_id: &str,
    title: &str,
    threshold: i64,
) -> Result<Option<i64>, String> {
    let by_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM works WHERE author_id=?1 AND pixiv_novel_id=?2",
            params![author_id, novel_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if by_id.is_some() {
        return Ok(by_id);
    }
    let works = works_for_author(conn, author_id)?;
    Ok(works
        .iter()
        .find(|(_, existing_title)| similarity_percent(existing_title, title) >= threshold)
        .map(|(id, _)| *id))
}

fn pixiv_sync_impl(
    author_id: i64,
    start_date: String,
    end_date: String,
    app: tauri::AppHandle,
) -> Result<PixivSyncResult, String> {
    let conn = db()?;
    let minimum_file_size_bytes = setting(&conn, "minimum_file_size_bytes")?
        .parse::<u64>()
        .unwrap_or(0);
    let delay_threshold = setting(&conn, "pixiv_delay_threshold")?
        .parse::<usize>()
        .unwrap_or(150);
    let delay_seconds = setting(&conn, "pixiv_delay_seconds")?
        .parse::<u64>()
        .unwrap_or(1);
    let excluded_tags: Vec<String> = setting(&conn, "excluded_tags")?
        .split([',', '，'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.to_lowercase())
        .collect();
    let (homepage, preview_dir, threshold, last_sync, cookie): (String, String, i64, String, String) = conn.query_row(
        "SELECT a.homepage, a.preview_dir, a.match_threshold, a.pixiv_last_sync_at, COALESCE((SELECT value FROM app_settings WHERE key='pixiv_cookie'), '') FROM authors a WHERE a.id=?1",
        [author_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
    ).map_err(|e| e.to_string())?;
    if preview_dir.trim().is_empty() {
        return Err("请先在作者设置中绑定预览版文件夹。".into());
    }
    let preview_dir = PathBuf::from(preview_dir);
    fs::create_dir_all(&preview_dir).map_err(|e| format!("无法创建预览版文件夹：{e}"))?;
    let user_id = pixiv_user_id(&homepage)?;
    let start = parse_date_bound(&start_date, "开始日期")?;
    let end = parse_date_bound(&end_date, "结束日期")?;
    if start > end {
        return Err("开始日期不能晚于结束日期。".into());
    }
    let use_incremental_filter = start.is_none() && end.is_none();
    let last_sync = DateTime::parse_from_rfc3339(&last_sync)
        .ok()
        .map(|date| date.with_timezone(&Utc));
    let cookie = if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    };
    let client = pixiv_client(cookie)?;
    let list_url = format!("https://www.pixiv.net/ajax/user/{user_id}/profile/all");
    let listing: Value = client
        .get(list_url)
        .send()
        .map_err(|e| format!("无法读取 Pixiv 作者作品列表：{e}"))?
        .error_for_status()
        .map_err(|e| format!("读取 Pixiv 作者作品列表失败：{e}"))?
        .json()
        .map_err(|e| format!("Pixiv 作者作品列表格式异常：{e}"))?;
    if listing.get("error").and_then(Value::as_bool) == Some(true) {
        return Err("Pixiv 拒绝了作者作品列表请求；请检查作者链接或 Cookie。".into());
    }
    let mut novels: Vec<String> = listing
        .pointer("/body/novels")
        .and_then(Value::as_object)
        .map(|items| items.keys().cloned().collect())
        .unwrap_or_default();
    // profile/all intentionally only contains IDs for novels. Process newer IDs
    // first; the submission-time filter is applied after loading each detail.
    novels.sort_by(|left, right| right.cmp(left));
    let known_novel_ids: HashSet<String> = conn
        .prepare("SELECT pixiv_novel_id FROM works WHERE author_id=?1 AND pixiv_novel_id <> ''")
        .map_err(|e| e.to_string())?
        .query_map([author_id], |row| row.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<HashSet<String>, _>>()
        .map_err(|e| e.to_string())?;
    let mut result = PixivSyncResult {
        downloaded_count: 0,
        skipped_existing_count: 0,
        skipped_date_count: 0,
        skipped_size_count: 0,
        failed_count: 0,
        cancelled: false,
        last_sync_at: last_sync.map(|date| date.to_rfc3339()).unwrap_or_default(),
    };
    let total = novels.len();
    let use_request_delay = total > delay_threshold && delay_seconds > 0;
    let candidates: Vec<String> = novels
        .into_iter()
        .filter(|novel_id| {
            if known_novel_ids.contains(novel_id) {
                result.skipped_existing_count += 1;
                false
            } else {
                true
            }
        })
        .collect();
    let _ = app.emit(
        "pixiv-sync-progress",
        PixivSyncProgress {
            total: candidates.len(),
            current: 0,
            title: format!(
                "已跳过 {} 篇已同步作品，正在抓取详情",
                result.skipped_existing_count
            ),
        },
    );
    let mut details = Vec::with_capacity(candidates.len());
    if use_request_delay {
        for (index, novel_id) in candidates.iter().enumerate() {
            if pixiv_sync_cancelled(author_id) {
                result.cancelled = true;
                break;
            }
            if index > 0 {
                std::thread::sleep(Duration::from_secs(delay_seconds));
            }
            match fetch_pixiv_novel_detail(&client, novel_id) {
                Ok(detail) if detail.get("error").and_then(Value::as_bool) != Some(true) => {
                    details.push((novel_id.clone(), detail));
                }
                _ => result.failed_count += 1,
            }
            let _ = app.emit(
                "pixiv-sync-progress",
                PixivSyncProgress {
                    total: candidates.len(),
                    current: index + 1,
                    title: format!("正在抓取作品详情：{} / {}", index + 1, candidates.len()),
                },
            );
        }
    } else {
        const DETAIL_CONCURRENCY: usize = 6;
        for (batch_index, batch) in candidates.chunks(DETAIL_CONCURRENCY).enumerate() {
            if pixiv_sync_cancelled(author_id) {
                result.cancelled = true;
                break;
            }
            let fetched = std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|novel_id| {
                        let client = client.clone();
                        let novel_id = novel_id.clone();
                        scope.spawn(move || {
                            let detail = fetch_pixiv_novel_detail(&client, &novel_id);
                            (novel_id, detail)
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .filter_map(|handle| handle.join().ok())
                    .collect::<Vec<_>>()
            });
            for (novel_id, detail) in fetched {
                match detail {
                    Ok(detail) if detail.get("error").and_then(Value::as_bool) != Some(true) => {
                        details.push((novel_id, detail));
                    }
                    _ => result.failed_count += 1,
                }
            }
            let current = ((batch_index + 1) * DETAIL_CONCURRENCY).min(candidates.len());
            let _ = app.emit(
                "pixiv-sync-progress",
                PixivSyncProgress {
                    total: candidates.len(),
                    current,
                    title: format!("正在并发抓取作品详情：{} / {}", current, candidates.len()),
                },
            );
        }
    }
    let mut downloads = Vec::new();
    for (novel_id, detail) in details {
        if pixiv_sync_cancelled(author_id) {
            result.cancelled = true;
            break;
        }
        let body = detail.get("body").unwrap_or(&Value::Null);
        let title = json_string(body, "title");
        let content = json_string(body, "content");
        let cover_url = json_string(body, "coverUrl");
        let series = body.get("seriesNavData").unwrap_or(&Value::Null);
        let series_id = ["seriesId", "id"]
            .iter()
            .map(|key| json_id_string(series, key))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let series_title = ["title", "seriesTitle"]
            .iter()
            .map(|key| json_string(series, key))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        let series_order = json_i64(series, "order");
        if !series_id.is_empty() && !series_title.is_empty() {
            conn.execute(
                "INSERT INTO series_catalog (author_id, id, title) VALUES (?1, ?2, ?3) ON CONFLICT(author_id, id) DO UPDATE SET title=excluded.title",
                params![author_id, series_id, series_title],
            )
            .map_err(|e| e.to_string())?;
        }
        let published_at = pixiv_published_at(body);
        if !is_within_date_range(&published_at, start, end) {
            result.skipped_date_count += 1;
            continue;
        }
        if use_incremental_filter && !is_after_last_sync(&published_at, last_sync) {
            result.skipped_date_count += 1;
            continue;
        }
        let date = release_date(&published_at)
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        if title.is_empty() || content.is_empty() || cover_url.is_empty() {
            result.failed_count += 1;
            continue;
        }
        if (content.len() as u64) < minimum_file_size_bytes {
            result.skipped_size_count += 1;
            continue;
        }
        if let Some(existing_id) =
            existing_sync_target(&conn, author_id, &novel_id, &title, threshold)?
        {
            conn.execute(
                "UPDATE works SET pixiv_novel_id=CASE WHEN pixiv_novel_id='' THEN ?1 ELSE pixiv_novel_id END, series_id=?2, series_title=?3, series_order=?4 WHERE id=?5",
                params![novel_id, series_id, series_title, series_order, existing_id],
            )
            .map_err(|e| e.to_string())?;
            result.skipped_existing_count += 1;
            continue;
        }
        let tags = body
            .pointer("/tags/tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|tag| tag.get("tag").and_then(Value::as_str))
                    .filter(|tag| {
                        !excluded_tags
                            .iter()
                            .any(|excluded| tag.to_lowercase().contains(excluded))
                    })
                    .collect::<Vec<_>>()
                    .join("| ")
            })
            .unwrap_or_default();
        downloads.push(PixivDownloadCandidate {
            novel_id,
            title,
            content,
            cover_url,
            release_date: date,
            tags,
            series_id,
            series_title,
            series_order,
        });
    }
    const COVER_CONCURRENCY: usize = 4;
    let download_total = downloads.len();
    for (batch_index, batch) in downloads.chunks(COVER_CONCURRENCY).enumerate() {
        if pixiv_sync_cancelled(author_id) {
            result.cancelled = true;
            break;
        }
        let covers = std::thread::scope(|scope| {
            let handles = batch
                .iter()
                .map(|work| {
                    let client = client.clone();
                    let cover_url = work.cover_url.clone();
                    scope.spawn(move || fetch_pixiv_cover(&client, &cover_url))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("封面下载线程异常".into()))
                })
                .collect::<Vec<_>>()
        });
        for (work, cover) in batch.iter().zip(covers) {
            if pixiv_sync_cancelled(author_id) {
                result.cancelled = true;
                break;
            }
            let cover = match cover {
                Ok(bytes) => bytes,
                Err(_) => {
                    result.failed_count += 1;
                    continue;
                }
            };
            let (text_path, cover_path) = sync_paths(&preview_dir, &work.title, &work.novel_id);
            let text_temp = text_path.with_extension("txt.part");
            let cover_temp = cover_path.with_extension("jpg.part");
            let write_result = fs::write(&text_temp, work.content.as_bytes())
                .and_then(|_| fs::write(&cover_temp, &cover))
                .and_then(|_| fs::rename(&text_temp, &text_path))
                .and_then(|_| fs::rename(&cover_temp, &cover_path));
            if write_result.is_err() {
                let _ = fs::remove_file(&text_temp);
                let _ = fs::remove_file(&cover_temp);
                result.failed_count += 1;
                continue;
            }
            if conn.execute("INSERT INTO works (author_id, title, release_date, preview_path, cover_path, tags, pixiv_novel_id, series_id, series_title, series_order, is_new) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)", params![author_id, work.title, work.release_date, text_path.to_string_lossy(), cover_path.to_string_lossy(), work.tags, work.novel_id, work.series_id, work.series_title, work.series_order]).is_err() {
            result.failed_count += 1;
            continue;
        }
            result.downloaded_count += 1;
        }
        let current = ((batch_index + 1) * COVER_CONCURRENCY).min(download_total);
        let _ = app.emit(
            "pixiv-sync-progress",
            PixivSyncProgress {
                total: download_total,
                current,
                title: format!("正在并发下载封面并保存：{} / {}", current, download_total),
            },
        );
    }
    if !result.cancelled && result.failed_count == 0 {
        result.last_sync_at = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE authors SET pixiv_last_sync_at=?1 WHERE id=?2",
            params![result.last_sync_at, author_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(result)
}

#[tauri::command]
async fn sync_pixiv_novels(
    author_id: i64,
    start_date: String,
    end_date: String,
    app: tauri::AppHandle,
) -> Result<PixivSyncResult, String> {
    clear_pixiv_sync_cancel(author_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        pixiv_sync_impl(author_id, start_date, end_date, app)
    })
    .await
    .map_err(|e| e.to_string())?;
    clear_pixiv_sync_cancel(author_id);
    result
}

#[tauri::command]
fn cancel_pixiv_sync(author_id: i64) {
    let _ = PIXIV_SYNC_CANCELLATIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut cancelled| cancelled.insert(author_id));
}

fn sync_pixiv_author_profile_impl(
    author_id: Option<i64>,
    homepage: String,
) -> Result<PixivAuthorProfile, String> {
    let conn = db()?;
    let homepage = homepage.trim().to_string();
    let user_id = pixiv_user_id(&homepage)?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT name FROM authors WHERE homepage=?1 AND id <> COALESCE(?2, -1)",
            params![homepage, author_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(name) = existing {
        return Err(format!("作者主页已存在，名称为“{name}”"));
    }
    let cookie = setting(&conn, "pixiv_cookie")?;
    let client = pixiv_client(if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    })?;
    let response: Value = client
        .get(format!("https://www.pixiv.net/ajax/user/{user_id}"))
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;
    let body = response.get("body").unwrap_or(&Value::Null);
    let name = json_string(body, "name");
    if name.is_empty() {
        return Err("无法获取 Pixiv 作者名称。".into());
    }
    if let Some(id) = author_id {
        conn.execute(
            "UPDATE authors SET name=?1, homepage=?2 WHERE id=?3",
            params![name, homepage, id],
        )
        .map_err(|e| e.to_string())?;
    }
    let avatar_url = ["imageBig", "image", "profileImageUrl"]
        .iter()
        .map(|key| json_string(body, key))
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let mut avatar_path = String::new();
    let mut avatar_managed = false;
    if !avatar_url.is_empty() {
        if let Ok(bytes) = client
            .get(&avatar_url)
            .header(reqwest::header::REFERER, "https://www.pixiv.net/")
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.bytes())
        {
            let dir = app_data_dir()?.join("avatars");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let key = author_id
                .map(|id| format!("author-{id}"))
                .unwrap_or_else(|| format!("profile-{user_id}"));
            let path = dir.join(format!("pixiv-{key}.jpg"));
            fs::write(&path, bytes).map_err(|e| e.to_string())?;
            avatar_path = path.to_string_lossy().to_string();
            avatar_managed = true;
            if let Some(id) = author_id {
                conn.execute(
                    "UPDATE authors SET avatar_path=?1, avatar_managed=1 WHERE id=?2",
                    params![avatar_path, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    if let Some(id) = author_id {
        let author = read_author(&conn, id)?;
        return Ok(PixivAuthorProfile {
            id: Some(author.id),
            name: author.name,
            homepage: author.homepage,
            avatar_path: author.avatar_path,
            avatar_managed: author.avatar_managed,
            notes: author.notes,
            preview_dir: author.preview_dir,
            purchased_dir: author.purchased_dir,
            match_threshold: author.match_threshold,
            pixiv_last_sync_at: author.pixiv_last_sync_at,
        });
    }
    Ok(PixivAuthorProfile {
        id: None,
        name,
        homepage,
        avatar_path,
        avatar_managed,
        notes: String::new(),
        preview_dir: String::new(),
        purchased_dir: String::new(),
        match_threshold: 70,
        pixiv_last_sync_at: String::new(),
    })
}

#[tauri::command]
async fn sync_pixiv_author_profile(
    author_id: Option<i64>,
    homepage: String,
) -> Result<PixivAuthorProfile, String> {
    tauri::async_runtime::spawn_blocking(move || {
        sync_pixiv_author_profile_impl(author_id, homepage)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[allow(dead_code)]
fn scan_preview_existing(author_id: i64) -> Result<ScanPreviewResult, String> {
    let conn = db()?;
    let (dir, threshold): (String, i64) = conn
        .query_row(
            "SELECT preview_dir, match_threshold FROM authors WHERE id=?1",
            [author_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    if dir.trim().is_empty() {
        return Err("请先绑定预览版文件夹".into());
    }
    let entries = fs::read_dir(&dir).map_err(|e| format!("无法读取预览版文件夹：{e}"))?;
    let works = preview_works_for_author(&conn, author_id)?;
    let mut previews = 0;
    let mut covers = 0;
    let mut ambiguous = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let key = stem(&path);
        if key.is_empty() {
            continue;
        }
        let scores: Vec<(&(i64, String, String), i64)> = works
            .iter()
            .map(|work| (work, similarity_percent(&work.1, &key)))
            .filter(|(_, score)| *score >= threshold)
            .collect();
        let Some(best_score) = scores.iter().map(|(_, score)| *score).max() else {
            continue;
        };
        let mut best_matches = scores.into_iter().filter(|(_, score)| *score == best_score);
        let Some((matched, _)) = best_matches.next() else {
            continue;
        };
        if best_matches.next().is_some() {
            ambiguous += 1;
            continue;
        }
        let (id, _, _) = matched;
        {
            let is_jpg = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"))
                .unwrap_or(false);
            if is_jpg {
                conn.execute(
                    "UPDATE works SET cover_path=?1 WHERE id=?2",
                    params![path.to_string_lossy(), id],
                )
                .map_err(|e| e.to_string())?;
                covers += 1;
            } else if path.is_dir()
                || path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("txt"))
                    .unwrap_or(false)
            {
                conn.execute(
                    "UPDATE works SET preview_path=?1 WHERE id=?2",
                    params![path.to_string_lossy(), id],
                )
                .map_err(|e| e.to_string())?;
                previews += 1;
            }
        }
    }
    Ok(ScanPreviewResult {
        preview_count: previews,
        cover_count: covers,
        ambiguous_count: ambiguous,
        created_count: 0,
        bound_count: 0,
    })
}

#[tauri::command]
fn scan_preview(author_id: i64) -> Result<ScanPreviewResult, String> {
    let conn = db()?;
    let (dir, threshold): (String, i64) = conn
        .query_row(
            "SELECT preview_dir, match_threshold FROM authors WHERE id=?1",
            [author_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let minimum_file_size_bytes = setting(&conn, "minimum_file_size_bytes")?
        .parse::<u64>()
        .unwrap_or(0);
    if dir.trim().is_empty() {
        return Err("请先绑定预览版文件夹".into());
    }
    let entries = fs::read_dir(&dir).map_err(|e| format!("无法读取预览版文件夹：{e}"))?;
    let mut works = preview_works_for_author(&conn, author_id)?;
    let mut previews = 0;
    let mut covers = 0;
    let mut ambiguous = 0;
    let mut created = 0;
    let mut bound = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        let is_cover = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg"))
            .unwrap_or(false);
        let is_preview = path.is_dir()
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("txt"))
                .unwrap_or(false);
        if !is_cover && !is_preview {
            continue;
        }
        if path.is_file()
            && entry.metadata().map_err(|e| e.to_string())?.len() < minimum_file_size_bytes
        {
            continue;
        }
        let key = stem(&path);
        if key.is_empty() {
            continue;
        }
        let scores: Vec<(i64, i64)> = works
            .iter()
            .map(|(id, title, _)| (*id, similarity_percent(title, &key)))
            .filter(|(_, score)| *score >= threshold)
            .collect();
        let matched_id = if let Some(best_score) = scores.iter().map(|(_, score)| *score).max() {
            let best: Vec<i64> = scores
                .into_iter()
                .filter(|(_, score)| *score == best_score)
                .map(|(id, _)| id)
                .collect();
            if best.len() != 1 {
                ambiguous += 1;
                continue;
            }
            best[0]
        } else {
            let (release_date, title) = parse_line(&key).unwrap_or((String::new(), key.clone()));
            if let Some((id, _, _)) = works
                .iter()
                .find(|(_, existing_title, _)| existing_title == &title)
            {
                *id
            } else {
                conn.execute(
                    "INSERT INTO works (author_id, title, release_date) VALUES (?1, ?2, ?3)",
                    params![author_id, title, release_date],
                )
                .map_err(|e| e.to_string())?;
                let id = conn.last_insert_rowid();
                works.push((id, title, release_date));
                created += 1;
                id
            }
        };
        let path_value = path.to_string_lossy().to_string();
        if is_cover {
            let current: String = conn
                .query_row(
                    "SELECT cover_path FROM works WHERE id=?1",
                    [matched_id],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if current != path_value {
                conn.execute(
                    "UPDATE works SET cover_path=?1 WHERE id=?2",
                    params![path_value, matched_id],
                )
                .map_err(|e| e.to_string())?;
                covers += 1;
                bound += 1;
            }
        } else {
            let current: String = conn
                .query_row(
                    "SELECT preview_path FROM works WHERE id=?1",
                    [matched_id],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if current != path_value {
                conn.execute(
                    "UPDATE works SET preview_path=?1 WHERE id=?2",
                    params![path_value, matched_id],
                )
                .map_err(|e| e.to_string())?;
                previews += 1;
                bound += 1;
            }
        }
    }
    Ok(ScanPreviewResult {
        preview_count: previews,
        cover_count: covers,
        ambiguous_count: ambiguous,
        created_count: created,
        bound_count: bound,
    })
}

#[tauri::command]
fn scan_purchased(author_id: i64) -> Result<ScanPurchasedResult, String> {
    let conn = db()?;
    let (dir, threshold): (String, i64) = conn
        .query_row(
            "SELECT purchased_dir, match_threshold FROM authors WHERE id=?1",
            [author_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let entries = fs::read_dir(&dir).map_err(|e| format!("无法读取完整版文件夹：{e}"))?;
    let paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    let works = works_for_author(&conn, author_id)?;
    let scored_paths: Vec<(PathBuf, Vec<(i64, String, i64)>)> = paths
        .into_iter()
        .map(|path| {
            let mut candidates: Vec<(i64, String, i64)> = works
                .iter()
                .map(|(id, title)| {
                    (
                        *id,
                        title.clone(),
                        similarity_percent(title, &file_name(&path)),
                    )
                })
                .collect();
            candidates.sort_by(|left, right| right.2.cmp(&left.2));
            (path, candidates)
        })
        .collect();

    let mut strong_usage: HashMap<i64, usize> = HashMap::new();
    for (_, candidates) in &scored_paths {
        for (work_id, _, _) in candidates
            .iter()
            .filter(|(_, _, similarity)| *similarity >= threshold)
        {
            *strong_usage.entry(*work_id).or_default() += 1;
        }
    }

    let mut bound_count = 0;
    let mut selections = vec![];
    for (path, candidates) in scored_paths {
        let strong: Vec<(i64, String, i64)> = candidates
            .iter()
            .filter(|(_, _, similarity)| *similarity >= threshold)
            .cloned()
            .collect();
        if strong.len() == 1 && strong_usage.get(&strong[0].0) == Some(&1) {
            conn.execute(
                "UPDATE works SET purchased_path=?1 WHERE id=?2",
                params![path.to_string_lossy(), strong[0].0],
            )
            .map_err(|e| e.to_string())?;
            bound_count += 1;
            continue;
        }
        let options = if strong.is_empty() {
            candidates.into_iter().take(3).collect()
        } else {
            strong
        };
        if !options.is_empty() {
            selections.push(PurchasedSelection {
                path: path.to_string_lossy().to_string(),
                candidates: options
                    .into_iter()
                    .map(|(work_id, title, similarity)| WorkCandidate {
                        work_id,
                        title,
                        similarity,
                    })
                    .collect(),
            });
        }
    }
    Ok(ScanPurchasedResult {
        bound_count,
        selections,
    })
}

#[tauri::command]
fn bind_work(work_id: i64, path: String) -> Result<(), String> {
    db()?
        .execute(
            "UPDATE works SET purchased_path=?1 WHERE id=?2",
            params![path, work_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn copy_previews_to_purchased(
    author_id: i64,
    work_ids: Vec<i64>,
) -> Result<CopyPreviewResult, String> {
    if work_ids.is_empty() {
        return Ok(CopyPreviewResult {
            copied_count: 0,
            bound_count: 0,
            skipped_count: 0,
        });
    }
    let conn = db()?;
    let purchased_dir: String = conn
        .query_row(
            "SELECT purchased_dir FROM authors WHERE id=?1",
            [author_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if purchased_dir.trim().is_empty() {
        return Err("请先在作者设置中选择完整版文件夹。".into());
    }
    let purchased_dir = PathBuf::from(purchased_dir);
    fs::create_dir_all(&purchased_dir).map_err(|e| e.to_string())?;

    let mut result = CopyPreviewResult {
        copied_count: 0,
        bound_count: 0,
        skipped_count: 0,
    };
    for work_id in work_ids {
        let preview_path: Option<String> = conn
            .query_row(
                "SELECT preview_path FROM works WHERE id=?1 AND author_id=?2",
                params![work_id, author_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(preview_path) = preview_path else {
            result.skipped_count += 1;
            continue;
        };
        let source = PathBuf::from(preview_path);
        let Some(name) = source.file_name() else {
            result.skipped_count += 1;
            continue;
        };
        if !source.exists() {
            result.skipped_count += 1;
            continue;
        }
        let destination = purchased_dir.join(name);
        if source != destination && !destination.exists() {
            if source.is_dir() {
                copy_directory(&source, &destination)?;
            } else {
                fs::copy(&source, &destination).map_err(|e| e.to_string())?;
            }
            result.copied_count += 1;
        }
        conn.execute(
            "UPDATE works SET purchased_path=?1 WHERE id=?2",
            params![destination.to_string_lossy(), work_id],
        )
        .map_err(|e| e.to_string())?;
        result.bound_count += 1;
    }
    Ok(result)
}

#[tauri::command]
fn delete_work(work_id: i64) -> Result<(), String> {
    db()?
        .execute("DELETE FROM works WHERE id=?1", [work_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_works(work_ids: Vec<i64>) -> Result<(), String> {
    if work_ids.is_empty() {
        return Ok(());
    }
    let mut conn = db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for work_id in work_ids {
        tx.execute("DELETE FROM works WHERE id=?1", [work_id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn toggle_favorite(work_id: i64) -> Result<(), String> {
    db()?
        .execute(
            "UPDATE works SET favorite = CASE favorite WHEN 1 THEN 0 ELSE 1 END WHERE id=?1",
            [work_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn update_work_tags(work_id: i64, tags: Vec<String>) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    let tags = tags
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty() && seen.insert(tag.to_lowercase()))
        .collect::<Vec<_>>()
        .join("|");
    db()?
        .execute(
            "UPDATE works SET tags=?1 WHERE id=?2",
            params![tags, work_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_work(work_id: i64) -> Result<(), String> {
    let conn = db()?;
    let (purchased, preview): (String, String) = conn
        .query_row(
            "SELECT purchased_path, preview_path FROM works WHERE id=?1",
            [work_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let path = if !purchased.is_empty() {
        purchased
    } else {
        preview
    };
    if path.is_empty() {
        return Err("该作品没有完整版内容，也没有可打开的预览版".into());
    }
    if !Path::new(&path).exists() {
        return Err("绑定的本地文件已不存在，请重新绑定路径".into());
    }
    open::that(&path).map_err(|e| format!("无法打开内容：{e}"))?;
    conn.execute("UPDATE works SET is_new=0 WHERE id=?1", [work_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn parse_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.len() < 11 {
        return None;
    }
    let bytes = line.as_bytes();
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let date = &line[0..10];
    if !date.chars().enumerate().all(|(i, c)| {
        if i == 4 || i == 7 {
            c == '-'
        } else {
            c.is_ascii_digit()
        }
    }) {
        return None;
    }
    let title = line[10..]
        .trim()
        .trim_start_matches(['-', '_', ' ', '　'])
        .trim()
        .to_string();
    if title.is_empty() {
        None
    } else {
        Some((date.to_string(), title))
    }
}

#[tauri::command]
fn preview_import(author_id: i64, lines: Vec<String>) -> Result<ImportPreview, String> {
    let conn = db()?;
    let mut new_count = 0;
    let mut invalid_count = 0;
    let mut duplicates = vec![];
    for line in lines {
        if let Some((date, title)) = parse_line(&line) {
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT id FROM works WHERE author_id=?1 AND title=?2 AND release_date=?3",
                    params![author_id, title, date],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if exists.is_some() {
                duplicates.push(format!("{date} {title}"));
            } else {
                new_count += 1;
            }
        } else {
            invalid_count += 1;
        }
    }
    Ok(ImportPreview {
        new_count,
        duplicate_count: duplicates.len(),
        invalid_count,
        duplicates,
    })
}

#[tauri::command]
fn commit_import(
    author_id: i64,
    lines: Vec<String>,
    overwrite: bool,
) -> Result<ImportResult, String> {
    let mut conn = db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut created = 0;
    let mut updated = 0;
    let mut skipped = 0;
    for line in lines {
        let Some((date, title)) = parse_line(&line) else {
            skipped += 1;
            continue;
        };
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM works WHERE author_id=?1 AND title=?2 AND release_date=?3",
                params![author_id, title, date],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if exists.is_some() {
            if overwrite {
                updated += 1;
            } else {
                skipped += 1;
            }
        } else {
            tx.execute(
                "INSERT INTO works (author_id, title, release_date) VALUES (?1, ?2, ?3)",
                params![author_id, title, date],
            )
            .map_err(|e| e.to_string())?;
            created += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ImportResult {
        created,
        updated,
        skipped,
    })
}

#[tauri::command]
fn read_import_file(path: String, column: usize) -> Result<Vec<String>, String> {
    let mut values = vec![];
    if path.to_lowercase().ends_with(".csv") {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_path(path)
            .map_err(|e| e.to_string())?;
        for row in reader.records() {
            let record = row.map_err(|e| e.to_string())?;
            if let Some(value) = record.get(column.saturating_sub(1)) {
                values.push(value.to_string());
            }
        }
    } else {
        let mut workbook =
            open_workbook_auto(path).map_err(|e| format!("无法读取 Excel 文件：{e}"))?;
        let sheet = workbook
            .sheet_names()
            .first()
            .cloned()
            .ok_or("Excel 文件中没有工作表")?;
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|e| format!("无法读取工作表：{e}"))?;
        for row in range.rows() {
            if let Some(cell) = row.get(column.saturating_sub(1)) {
                values.push(cell.to_string());
            }
        }
    }
    Ok(values)
}

fn should_import_folder_entry(
    is_directory: bool,
    is_txt: bool,
    size: u64,
    minimum_size_bytes: u64,
) -> bool {
    is_directory || (is_txt && size >= minimum_size_bytes)
}

#[tauri::command]
fn read_import_folder(path: String, minimum_size_bytes: u64) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(&path).map_err(|e| format!("无法读取作品文件夹：{e}"))?;
    let mut names = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| format!("无法读取作品文件夹内容：{e}"))?;
        let entry_path = entry.path();
        let is_txt = entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("txt"))
            .unwrap_or(false);
        let is_directory = entry_path.is_dir();
        let size = if is_directory {
            0
        } else {
            entry
                .metadata()
                .map_err(|e| format!("无法读取文件大小：{e}"))?
                .len()
        };
        if should_import_folder_entry(is_directory, is_txt, size, minimum_size_bytes) {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    Ok(names)
}

#[tauri::command]
fn export_backup(path: String) -> Result<(), String> {
    let source = app_data_dir()?.join("library.db");
    if !source.exists() {
        let _ = db()?;
    }
    fs::copy(source, path).map_err(|e| format!("导出备份失败：{e}"))?;
    Ok(())
}

#[tauri::command]
fn restore_backup(path: String) -> Result<(), String> {
    if !Path::new(&path).is_file() {
        return Err("请选择有效的备份文件".into());
    }
    let target = app_data_dir()?.join("library.db");
    fs::copy(path, target).map_err(|e| format!("恢复备份失败：{e}"))?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_authors,
            save_author,
            delete_author,
            update_author_path,
            set_match_threshold,
            get_app_settings,
            save_app_settings,
            read_pixiv_cookie_file,
            list_works,
            list_all_works,
            list_series_works,
            list_series,
            set_work_series,
            leave_work_series,
            scan_preview,
            scan_purchased,
            bind_work,
            copy_previews_to_purchased,
            delete_work,
            delete_works,
            toggle_favorite,
            open_work,
            preview_import,
            commit_import,
            read_import_file,
            read_import_folder,
            sync_pixiv_novels,
            cancel_pixiv_sync,
            sync_pixiv_author_profile,
            update_work_tags,
            export_backup,
            restore_backup
        ])
        .run(tauri::generate_context!())
        .expect("启动应用时发生错误");
}

#[cfg(test)]
mod tests {
    use super::{
        file_name, is_after_last_sync, is_within_date_range, name_key, normalize_pixiv_cookie,
        pixiv_published_at, should_import_folder_entry, similarity_percent,
    };
    use chrono::{DateTime, NaiveDate, Utc};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn preview_matching_ignores_known_extensions() {
        assert_eq!(
            name_key("2025-10-05（插画）～希儿.txt"),
            "2025-10-05（插画）～希儿"
        );
        assert_eq!(name_key("月色图文辑"), "月色图文辑");
    }

    #[test]
    fn purchased_matching_uses_the_full_file_name() {
        assert_eq!(file_name(Path::new("作品名.part01.7z")), "作品名.part01.7z");
    }

    #[test]
    fn similarity_allows_a_full_title_inside_a_longer_file_name() {
        assert_eq!(
            similarity_percent("希儿布洛妮娅", "2025-10-05 希儿布洛妮娅 完整版.7z"),
            100
        );
        assert!(similarity_percent("夏日短篇集", "夏日短篇合集.pdf") >= 70);
    }

    #[test]
    fn folder_import_filters_only_small_txt_files() {
        assert!(should_import_folder_entry(true, false, 0, 1024));
        assert!(should_import_folder_entry(false, true, 1024, 1024));
        assert!(!should_import_folder_entry(false, true, 1023, 1024));
        assert!(!should_import_folder_entry(false, false, 4096, 1024));
    }

    #[test]
    fn cookie_normalization_keeps_only_pixiv_session() {
        assert_eq!(
            normalize_pixiv_cookie("foo=bar; PHPSESSID=session-value; other=value").unwrap(),
            "PHPSESSID=session-value"
        );
    }

    #[test]
    fn sync_date_range_is_inclusive_and_rejects_outside_dates() {
        let start = NaiveDate::from_ymd_opt(2025, 1, 1);
        let end = NaiveDate::from_ymd_opt(2025, 1, 31);
        assert!(is_within_date_range(
            "2025-01-01T00:00:00+00:00",
            start,
            end
        ));
        assert!(is_within_date_range(
            "2025-01-31T23:59:59+00:00",
            start,
            end
        ));
        assert!(!is_within_date_range(
            "2024-12-31T23:59:59+00:00",
            start,
            end
        ));
        assert!(!is_within_date_range(
            "2025-02-01T00:00:00+00:00",
            start,
            end
        ));
    }

    #[test]
    fn incremental_sync_requires_a_later_submission_time() {
        let last_sync = DateTime::parse_from_rfc3339("2025-01-31T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        assert!(is_after_last_sync(
            "2025-01-31T12:00:01+00:00",
            Some(last_sync)
        ));
        assert!(!is_after_last_sync(
            "2025-01-31T12:00:00+00:00",
            Some(last_sync)
        ));
        assert!(!is_after_last_sync(
            "2025-01-31T11:59:59+00:00",
            Some(last_sync)
        ));
        assert!(!is_after_last_sync("2025-01-31", Some(last_sync)));
        assert!(is_after_last_sync("2025-02-01", Some(last_sync)));
    }

    #[test]
    fn sync_uses_submission_time_instead_of_last_edit_time() {
        let detail = json!({
            "createDate": "2025-01-15T09:30:00+00:00",
            "uploadDate": "2025-04-10T11:00:00+00:00"
        });
        assert_eq!(pixiv_published_at(&detail), "2025-01-15T09:30:00+00:00");
        assert_eq!(
            pixiv_published_at(&json!({ "uploadDate": "2025-04-10T11:00:00+00:00" })),
            "2025-04-10T11:00:00+00:00"
        );
    }
}
