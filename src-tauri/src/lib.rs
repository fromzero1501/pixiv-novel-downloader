use calamine::{open_workbook_auto, Reader};
use chrono::{DateTime, NaiveDate, Utc};
use encoding_rs::{GBK, UTF_16BE, UTF_16LE};
use reqwest::blocking::Client;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Emitter};

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
    aliases: String,
    starred: bool,
    work_count: i64,
    purchased_count: i64,
    favorite_count: i64,
    images_count: i64,
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
    #[serde(default)]
    aliases: String,
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
    has_images: bool,
    image_count: i64,
    tags: String,
    pixiv_novel_id: String,
    series_id: String,
    series_title: String,
    series_order: i64,
    is_new: bool,
    author_name: String,
    word_count: Option<usize>,
    file_format: Option<String>,
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
    /// 已经绑定过（文件在库里能对上）而跳过的文件数，用来给用户一个交代
    skipped_count: usize,
    selections: Vec<PurchasedSelection>,
    /// 文件名里写了作者、但这位作者不在作者库里的文件。按作者名分组，纯展示不给下拉框。
    #[serde(default)]
    unknown_author_groups: Vec<UnknownAuthorGroup>,
    /// 文件名里写的作者是库里另一位作者（不是当前这位）的文件数，已跳过不参与匹配。
    #[serde(default)]
    other_author_count: usize,
}

/// 「作者不在库」的文件分组：组标题是作者名，下面列文件名
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UnknownAuthorGroup {
    author: String,
    files: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PurchasedSelection {
    path: String,
    candidates: Vec<WorkCandidate>,
    /// 文件名里认出来的作者名（没有就是空串）
    #[serde(default)]
    author_name: String,
    /// 按角色匹配时文件里命中到的角色名（按标题匹配时为空）
    #[serde(default)]
    matched_characters: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkCandidate {
    work_id: i64,
    title: String,
    similarity: i64,
    /// 按角色匹配时，这个作品标题与文件共有的角色名
    #[serde(default)]
    matched_characters: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoGroupResult {
    auto_moved_count: usize,
    manual_selections: Vec<ManualGroupSelection>,
    /// 同 ScanPurchasedResult.unknown_author_groups
    #[serde(default)]
    unknown_author_groups: Vec<UnknownAuthorGroup>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualGroupSelection {
    file_path: String,
    file_name: String,
    /// 文件相对自动分组目录的子文件夹路径（顶层文件为空），用于区分不同子目录里的同名文件
    sub_dir: String,
    candidates: Vec<GroupCandidate>,
    /// 文件名里认出来的作者名（没有就是空串）
    #[serde(default)]
    author_name: String,
    /// 按角色匹配时文件里命中到的角色名
    #[serde(default)]
    matched_characters: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GroupCandidate {
    author_id: i64,
    author_name: String,
    work_id: i64,
    work_title: String,
    similarity: i64,
    /// 相似度是否达到自动关联阈值（前端用来标记推荐项）
    #[serde(default)]
    recommended: bool,
    /// 目标目录下是否已存在同名文件（前端用来提示冲突）
    #[serde(default)]
    conflict: bool,
    /// 按角色匹配时，这个作品标题与文件共有的角色名
    #[serde(default)]
    matched_characters: Vec<String>,
}

/// 目标目录已有同名文件时的处理方式
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ConflictAction {
    Skip,
    Overwrite,
    KeepBoth,
}

fn default_conflict_action() -> ConflictAction {
    ConflictAction::Skip
}

/// 手动分组时，用户为一个文件勾选的作品
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManualGroupChoice {
    file_path: String,
    work_ids: Vec<i64>,
    #[serde(default = "default_conflict_action")]
    conflict_action: ConflictAction,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManualGroupResult {
    bound_count: usize,
    skipped_count: usize,
    duplicated_count: usize,
    failed: Vec<String>,
}

/// 一次文件分发的结果
struct Distributed {
    bound_count: usize,
    skipped_count: usize,
}

/// 一个待写入的分发目标
struct DistributeTarget {
    work_id: i64,
    dir: PathBuf,
    title: String,
    extension: String,
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

/// 一个「完整版搜索网站」：名字用来在右键菜单里显示，网址是它的搜索页
/// （搜索词的位置约定为最后一个 `=` 之后，见 `build_search_url`）。
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SearchSite {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
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
    auto_group_dir: String,
    similarity_threshold: i64,
    min_similarity_threshold: i64,
    match_title_length: i64,
    image_quality: String,
    /// 同步图文小说时自动保留的阅读版格式：`html`（单网页）或 `epub`（电子书）。
    #[serde(default)]
    sync_image_format: String,
    /// 完整版搜索网站列表：在作品卡上选中标题文字后右键，可以直接跳去这些站搜索。
    #[serde(default)]
    search_sites: Vec<SearchSite>,
    /// 启动时自动检查有没有新版本。
    #[serde(default = "default_true")]
    auto_check_update: bool,
    /// GitHub 加速镜像（直连失败后按顺序试）。用户可以自己增删，镜像挂了不用等发新版。
    #[serde(default = "default_update_mirrors")]
    update_mirrors: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PixivSyncProgress {
    author_id: i64,
    total: usize,
    current: usize,
    title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PixivSyncResult {
    downloaded_count: usize,
    reused_preview_count: usize,
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
    embedded_images: Value,
    cover_url: String,
    release_date: String,
    tags: String,
    series_id: String,
    series_title: String,
    series_order: i64,
    is_preview: bool,
}

#[derive(Clone)]
struct SyncPreviewEntry {
    path: PathBuf,
    name: String,
    is_preview: bool,
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
          avatar_managed INTEGER NOT NULL DEFAULT 0,
          sort_order INTEGER NOT NULL DEFAULT 0
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
          has_images INTEGER NOT NULL DEFAULT 0,
          image_count INTEGER NOT NULL DEFAULT 0,
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
        "ALTER TABLE authors ADD COLUMN starred INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // 作者库手动排序（v0.3.69）：0 = 还没手动排过（按名字排），拖过之后写 1..n
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
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
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN has_images INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN image_count INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN aliases TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS works_author_pixiv_novel_id ON works(author_id, pixiv_novel_id) WHERE pixiv_novel_id <> ''", []);
    // 常见角色名表：用于「按角色匹配」的自动分组 / 关联完整版文件。
    // game 只用来分组展示，不参与匹配；name 与 aliases（`|` 分隔）都会拿去匹配。
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS characters (
          id INTEGER PRIMARY KEY,
          game TEXT NOT NULL DEFAULT '',
          name TEXT NOT NULL,
          aliases TEXT NOT NULL DEFAULT '',
          heat INTEGER NOT NULL DEFAULT 0,
          source TEXT NOT NULL DEFAULT 'builtin',
          enabled INTEGER NOT NULL DEFAULT 1,
          UNIQUE(game, name)
        )",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS characters_game ON characters(game)",
        [],
    );
    conn.execute(
        "INSERT OR IGNORE INTO series_catalog (author_id, id, title) SELECT author_id, series_id, series_title FROM works WHERE series_id <> '' AND series_title <> ''",
        [],
    ).map_err(|e| e.to_string())?;
    // 作者主页历史上有可能是 .../users/16208053/novels 这种带后缀的写法，统一收敛成 .../users/16208053。
    // 只在真的需要改的时候才写库（幂等），所以放在这里每次连接检查一遍也无妨。
    {
        let mut homes: Vec<(i64, String)> = Vec::new();
        {
            let mut statement = conn
                .prepare("SELECT id, homepage FROM authors WHERE homepage <> ''")
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                homes.push(row.map_err(|e| e.to_string())?);
            }
        }
        for (id, homepage) in homes {
            let normalized = normalize_author_homepage(&homepage);
            if normalized != homepage {
                conn.execute(
                    "UPDATE authors SET homepage=?1 WHERE id=?2",
                    params![normalized, id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    // 库是空的（第一次运行，或用户把角色表清空了）就把内置的常见角色名灌进来。
    // 用户后续的增删都会保留，不会被这里覆盖。
    let character_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0))
        .unwrap_or(0);
    if character_count == 0 {
        import_builtin_characters(&conn);
    }
    Ok(conn)
}

/// 内置的常见角色名。由 scripts/build-character-*.py 生成：
/// 先从各游戏中文 wiki 拉全量角色名单，再按 pixiv 同人热度排序，只留有热度的。
const BUILTIN_CHARACTERS: &str = include_str!("../characters.json");

fn import_builtin_characters(conn: &Connection) {
    let Ok(root) = serde_json::from_str::<Value>(BUILTIN_CHARACTERS) else {
        return;
    };
    let Some(map) = root.as_object() else {
        return;
    };
    for (game, list) in map {
        let Some(items) = list.as_array() else {
            continue;
        };
        for item in items {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                continue;
            }
            let heat = item.get("heat").and_then(|v| v.as_i64()).unwrap_or(0);
            let _ = conn.execute(
                "INSERT OR IGNORE INTO characters (game, name, aliases, heat, source) VALUES (?1, ?2, '', ?3, 'builtin')",
                params![game, name, heat],
            );
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CharacterEntry {
    id: i64,
    game: String,
    name: String,
    aliases: String,
    heat: i64,
    source: String,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CharacterGroup {
    game: String,
    characters: Vec<CharacterEntry>,
}

/// 用户自建的游戏分组（可能还是空的，一个角色都没加）。
fn custom_character_games(conn: &Connection) -> Vec<String> {
    setting(conn, "character_games")
        .unwrap_or_default()
        .split('|')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn save_custom_character_games(conn: &Connection, games: &[String]) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES ('character_games', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![games.join("|")],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 匹配用索引：把角色名与别名摊平成 (名字, 所属游戏)。长的排前面，命中时优先算长名。
fn character_match_index(conn: &Connection) -> Vec<(String, String)> {
    let mut index: Vec<(String, String)> = vec![];
    let Ok(mut statement) = conn.prepare("SELECT game, name, aliases FROM characters WHERE enabled=1")
    else {
        return index;
    };
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    });
    if let Ok(rows) = rows {
        for (game, name, aliases) in rows.flatten() {
            let name = name.trim();
            if !name.is_empty() {
                index.push((name.to_string(), game.clone()));
            }
            for alias in aliases.split('|') {
                let alias = alias.trim();
                if !alias.is_empty() && alias != name {
                    index.push((alias.to_string(), game.clone()));
                }
            }
        }
    }
    index.sort_by(|a, b| b.0.chars().count().cmp(&a.0.chars().count()));
    index.dedup();
    index
}

/// 一段文本里出现了哪些角色名（子串命中；命中即算，不做相似度）。
fn characters_in_text(text: &str, index: &[(String, String)]) -> Vec<String> {
    if text.is_empty() || index.is_empty() {
        return vec![];
    }
    let mut hits: Vec<String> = vec![];
    for (name, _) in index {
        if !hits.iter().any(|hit| hit == name) && text.contains(name.as_str()) {
            hits.push(name.clone());
        }
    }
    hits
}

/// 两个角色名集合的交集（用于判断作品标题与文件名是不是同一个角色）。
fn shared_characters(left: &[String], right: &[String]) -> Vec<String> {
    left.iter()
        .filter(|name| right.iter().any(|other| other == *name))
        .cloned()
        .collect()
}

/// 匹配方式：按标题（相似度，原有逻辑）或按角色（角色名有交集即可）。
#[derive(Clone, Copy, PartialEq, Debug)]
enum MatchMode {
    Title,
    Character,
}

fn resolve_match_mode(value: &str) -> Result<MatchMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "title" => Ok(MatchMode::Title),
        "character" => Ok(MatchMode::Character),
        other => Err(format!("未知的匹配方式：{other}")),
    }
}

/// 文件名里能认出来的作者：库里的作者 id（认不出具体人就是 None）+ 文件名里写的作者名。
#[derive(Clone, Debug, PartialEq)]
struct DetectedAuthor {
    author_id: Option<i64>,
    name: String,
}

fn is_name_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            ':' | '：'
                | '-'
                | '—'
                | '–'
                | '='
                | '＝'
                | '|'
                | '｜'
                | '·'
                | '_'
                | '/'
                | '\\'
                | '('
                | '（'
                | '['
                | '【'
                | ')'
                | '）'
                | ']'
                | '】'
                | ','
                | '，'
                | '、'
                | '~'
                | '～'
                | '.'
                | '。'
                | '!'
                | '！'
                | '?'
                | '？'
        )
}

/// 第一层：文件名里的显式「作者」标记，如 `作者：AAA`、`作者 - AAA`。取最后一个标记。
/// 故意不收裸的 `by ` —— 英文标题里 "Stand by Me" 这种会把 "Me" 误当成作者名。
fn author_after_marker(stem: &str) -> Option<String> {
    const MARKERS: [&str; 3] = ["作者", "著者", "written by"];
    let mut best: Option<String> = None;
    let mut search_from = 0usize;
    while search_from < stem.len() {
        let rest = &stem[search_from..];
        let found = MARKERS
            .iter()
            .filter_map(|marker| rest.find(marker).map(|pos| (pos, marker.len())))
            .min_by_key(|(pos, _)| *pos);
        let Some((pos, len)) = found else {
            break;
        };
        let abs = search_from + pos + len;
        let after = &stem[abs..];
        // 跳过紧跟标记的分隔符，再取到下一个分隔符为止
        let trimmed = after.trim_start_matches(is_name_separator);
        let name: String = trimmed.chars().take_while(|c| !is_name_separator(*c)).collect();
        if !name.is_empty() && name.chars().count() <= 30 {
            best = Some(name);
        }
        search_from = abs;
    }
    best
}

/// 第二层：倒查作者库 —— 文件名尾部以「分隔符 + 作者名（或别名）」结尾。
/// 只在库里确实有这个人时才认，避免把普通词误当人名。
fn author_at_tail(stem: &str, authors: &[(i64, String, String)]) -> Option<DetectedAuthor> {
    let tokens: Vec<&str> = stem
        .split(is_name_separator)
        .map(|token| token.trim())
        .filter(|token| !token.is_empty())
        .collect();
    for token in tokens.iter().rev().take(3) {
        let token = token.trim();
        for (id, name, aliases) in authors {
            if token == name.trim() || aliases.split('|').any(|alias| alias.trim() == token) {
                return Some(DetectedAuthor {
                    author_id: Some(*id),
                    name: token.to_string(),
                });
            }
        }
    }
    None
}

/// 认出文件名里的作者（两层判定）。第一层抓到名字但库里没有这个人时，
/// author_id 是 None，调用方会把它归进「作者不在库」分组。
fn detect_author_in_name(stem: &str, authors: &[(i64, String, String)]) -> Option<DetectedAuthor> {
    let stem = stem.trim();
    if stem.is_empty() {
        return None;
    }
    if let Some(name) = author_after_marker(stem) {
        let matched = authors.iter().find(|(_, known, aliases)| {
            known.trim() == name || aliases.split('|').any(|alias| alias.trim() == name)
        });
        return Some(DetectedAuthor {
            author_id: matched.map(|(id, _, _)| *id),
            name,
        });
    }
    author_at_tail(stem, authors)
}

/// 认出的作者该不该参与匹配：
/// - 没认出作者 → 参与（走原来的全量匹配）
/// - 认出的就是当前正在扫的这位作者 → 参与
/// - 认出的是库里另一位作者 → 这个文件不属于这个目录，不参与
///
/// 单独抽成纯函数是为了能测：这里比的是**作者 ID**，绝不要拿它去和作品 ID 比。
fn scope_allows_path(detected_author_id: Option<i64>, current_author_id: i64) -> bool {
    detected_author_id.map_or(true, |id| id == current_author_id)
}

/// 文件名（不含目录、不含扩展名），用于认作者和各种匹配。
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[tauri::command]
fn list_characters() -> Result<Vec<CharacterGroup>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare(
            "SELECT id, game, name, aliases, heat, source, enabled FROM characters
             ORDER BY game, heat DESC, name",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(CharacterEntry {
                id: row.get(0)?,
                game: row.get(1)?,
                name: row.get(2)?,
                aliases: row.get(3)?,
                heat: row.get(4)?,
                source: row.get(5)?,
                enabled: row.get::<_, i64>(6)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut groups: Vec<CharacterGroup> = vec![];
    for entry in rows.flatten() {
        match groups.last_mut() {
            Some(group) if group.game == entry.game => group.characters.push(entry),
            _ => groups.push(CharacterGroup {
                game: entry.game.clone(),
                characters: vec![entry],
            }),
        }
    }
    for game in custom_character_games(&conn) {
        if !groups.iter().any(|group| group.game == game) {
            groups.push(CharacterGroup {
                game,
                characters: vec![],
            });
        }
    }
    Ok(groups)
}

#[tauri::command]
fn add_character(game: String, name: String, aliases: String) -> Result<(), String> {
    let game = game.trim();
    let name = name.trim();
    if game.is_empty() {
        return Err("请先选择或新建一个游戏分组。".into());
    }
    if name.is_empty() {
        return Err("角色名不能为空。".into());
    }
    let conn = db()?;
    conn.execute(
        "INSERT INTO characters (game, name, aliases, heat, source) VALUES (?1, ?2, ?3, 0, 'user')
         ON CONFLICT(game, name) DO UPDATE SET aliases=excluded.aliases, enabled=1",
        params![game, name, normalize_aliases(&aliases)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn update_character(id: i64, name: String, aliases: String, enabled: bool) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("角色名不能为空。".into());
    }
    let conn = db()?;
    conn.execute(
        "UPDATE characters SET name=?1, aliases=?2, enabled=?3 WHERE id=?4",
        params![name, normalize_aliases(&aliases), if enabled { 1 } else { 0 }, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_character(id: i64) -> Result<(), String> {
    db()?
        .execute("DELETE FROM characters WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn add_character_game(game: String) -> Result<(), String> {
    let game = game.trim();
    if game.is_empty() {
        return Err("游戏名不能为空。".into());
    }
    let conn = db()?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM characters WHERE game=?1",
            [game],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if exists > 0 {
        return Ok(());
    }
    let mut games = custom_character_games(&conn);
    if !games.iter().any(|item| item == game) {
        games.push(game.to_string());
    }
    save_custom_character_games(&conn, &games)
}

#[tauri::command]
fn rename_character_game(game: String, next: String) -> Result<(), String> {
    let game = game.trim();
    let next = next.trim();
    if game.is_empty() || next.is_empty() {
        return Err("游戏名不能为空。".into());
    }
    if game == next {
        return Ok(());
    }
    let conn = db()?;
    conn.execute(
        "UPDATE characters SET game=?1 WHERE game=?2",
        params![next, game],
    )
    .map_err(|e| e.to_string())?;
    let games: Vec<String> = custom_character_games(&conn)
        .into_iter()
        .map(|item| if item == game { next.to_string() } else { item })
        .collect();
    save_custom_character_games(&conn, &games)
}

#[tauri::command]
fn delete_character_game(game: String) -> Result<(), String> {
    let conn = db()?;
    conn.execute("DELETE FROM characters WHERE game=?1", [game.trim()])
        .map_err(|e| e.to_string())?;
    let games: Vec<String> = custom_character_games(&conn)
        .into_iter()
        .filter(|item| item != game.trim())
        .collect();
    save_custom_character_games(&conn, &games)
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
        auto_group_dir: setting(conn, "auto_group_dir")?,
        similarity_threshold: setting(conn, "similarity_threshold")?.parse().unwrap_or(70),
        min_similarity_threshold: setting(conn, "min_similarity_threshold")?.parse().unwrap_or(30),
        match_title_length: setting(conn, "match_title_length")?.parse().unwrap_or(0),
        image_quality: {
            let value = setting(conn, "image_quality")?;
            if value == "original" {
                "original".to_string()
            } else {
                "1200".to_string()
            }
        },
        sync_image_format: {
            let value = setting(conn, "sync_image_format")?;
            if value == "epub" {
                "epub".to_string()
            } else {
                "html".to_string()
            }
        },
        // 存的是 JSON 文本；解析不出来（手改坏了或旧库空值）就当没配过
        search_sites: serde_json::from_str::<Vec<SearchSite>>(&setting(conn, "search_sites")?)
            .unwrap_or_default(),
        auto_check_update: match setting(conn, "auto_check_update")?.as_str() {
            "" => true,
            value => value == "1",
        },
        update_mirrors: read_update_mirrors(conn)?,
    })
}

/// 加速镜像列表。存的是 JSON 文本；没存过或存坏了都退回内置那几条。
fn read_update_mirrors(conn: &Connection) -> Result<Vec<String>, String> {
    let raw = setting(conn, "update_mirrors")?;
    if raw.trim().is_empty() {
        return Ok(default_update_mirrors());
    }
    Ok(serde_json::from_str::<Vec<String>>(&raw).unwrap_or_else(|_| default_update_mirrors()))
}

/// 把搜索词填进配置好的搜索页网址：约定「最后一个 `=` 之后」是搜索词的位置，
/// 先把那后面的内容清掉，再补上编码后的关键词。
/// 例：`https://a.com/search.php?kw=图` + `希儿` → `https://a.com/search.php?kw=%E5%B8%8C%E5%84%BF`
fn build_search_url(template: &str, keyword: &str) -> String {
    let template = template.trim();
    let encoded = percent_encode_utf8(keyword.trim());
    match template.rfind('=') {
        Some(index) => format!("{}{}", &template[..=index], encoded),
        None => format!("{template}={encoded}"),
    }
}

/// URL 查询串用的百分号编码：只保留 `A-Za-z0-9-_.~`，其余（含中文）逐字节编码。
fn percent_encode_utf8(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
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
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.favorite = 1),
          a.aliases,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.has_images = 1),
          a.starred
        FROM authors a WHERE a.id = ?1",
        [id],
        |row| Ok(AuthorSummary { id: row.get(0)?, name: row.get(1)?, homepage: row.get(2)?, avatar_path: row.get(3)?, notes: row.get(4)?, preview_dir: row.get(5)?, purchased_dir: row.get(6)?, match_threshold: row.get(7)?, pixiv_last_sync_at: row.get(8)?, avatar_managed: row.get::<_, i64>(9)? == 1, work_count: row.get(10)?, purchased_count: row.get(11)?, favorite_count: row.get(12)?, aliases: row.get(13)?, images_count: row.get(14)?, starred: row.get::<_, i64>(15)? == 1 })
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
        has_images: row.get::<_, i64>(8)? == 1,
        tags: row.get(9)?,
        pixiv_novel_id: row.get(10)?,
        series_id: row.get(11)?,
        series_title: row.get(12)?,
        series_order: row.get(13)?,
        is_new: row.get::<_, i64>(14)? == 1,
        author_name: row.get(15)?,
        word_count: None,
        file_format: None,
        image_count: row.get::<_, i64>(16)?,
    })
}

fn text_file_word_count(path: &Path) -> Option<usize> {
    let bytes = fs::read(path).ok()?;
    let content = if bytes.starts_with(&[0xFF, 0xFE]) {
        UTF_16LE.decode(&bytes[2..]).0
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        UTF_16BE.decode(&bytes[2..]).0
    } else if let Ok(content) = std::str::from_utf8(&bytes) {
        content.into()
    } else {
        GBK.decode(&bytes).0
    };
    Some(
        content
            .chars()
            .filter(|character| !character.is_whitespace())
            .count(),
    )
}

fn text_word_count_at(path: &Path) -> Option<usize> {
    if path.is_file() {
        return text_file_word_count(path);
    }
    if !path.is_dir() {
        return None;
    }
    let mut total = 0;
    let mut found = false;
    for entry in fs::read_dir(path).ok()?.flatten() {
        let entry_path = entry.path();
        let is_text = entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("txt"))
            .unwrap_or(false);
        let count = if entry_path.is_dir() || is_text {
            text_word_count_at(&entry_path)
        } else {
            None
        };
        if let Some(count) = count {
            total += count;
            found = true;
        }
    }
    found.then_some(total)
}

fn text_word_count(path: &str) -> Option<usize> {
    (!path.trim().is_empty())
        .then(|| text_word_count_at(Path::new(path)))
        .flatten()
}

/// 封面路径兜底：作品搬家或用户手动整理过目录后，cover_path 可能成了死路径。
/// 正文旁边通常还留着同名封面，能就地改指过去；实在没有就返回空串，
/// 让界面显示「暂无封面」的占位图，而不是一张裂开的图片。
fn resolve_cover_path(cover_path: &str, preview_path: &str, purchased_path: &str) -> String {
    if cover_path.trim().is_empty() {
        return String::new();
    }
    if Path::new(cover_path).is_file() {
        return cover_path.to_string();
    }
    for text in [purchased_path, preview_path] {
        if text.trim().is_empty() {
            continue;
        }
        let candidate = Path::new(text).with_extension("jpg");
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    String::new()
}

fn populate_work_display_info(work: &mut Work) {
    work.cover_path = resolve_cover_path(&work.cover_path, &work.preview_path, &work.purchased_path);
    work.word_count = None;
    work.file_format = None;
    if work.purchased_path.is_empty() {
        work.word_count = text_word_count(&work.preview_path);
        return;
    }
    let purchased_path = Path::new(&work.purchased_path);
    if purchased_path.is_file() {
        let extension = purchased_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .map(str::to_ascii_uppercase)
            .unwrap_or_else(|| "文件".into());
        if extension != "TXT" {
            work.file_format = Some(extension);
            return;
        }
    }
    work.word_count = text_word_count(&work.purchased_path);
}

/// 「字数从多到少」排序用的比较键：EPUB / HTML 这类阅读版**没有字数**，改成按配图张数比。
/// 返回 `(分组, 数量)` —— 分组 1 ＝ 电子书 / HTML（整体排在前面），0 ＝ TXT 与预览版正文。
fn content_size_key(work: &Work) -> (u8, usize) {
    if work.file_format.is_some() {
        (1, usize::try_from(work.image_count).unwrap_or(0))
    } else {
        (0, work.word_count.unwrap_or(0))
    }
}

/// 「字数从多到少」（v0.3.67 用户要求）：TXT 按字数排，EPUB / HTML 按配图张数排，电子书整体排在 TXT 前面。
/// 字数是 `populate_work_display_info()` 读文件算出来的，SQL 里排不了，所以必须在这之后重排；
/// `sort_by` 是稳定排序，键一样的还保持 SQL 给的日期序。
fn sort_works_by_content_size(works: &mut [Work]) {
    works.sort_by(|left, right| content_size_key(right).cmp(&content_size_key(left)));
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

/// 作者库排序（v0.3.69）：手动拖过的（`sort_order` 1..n）按手动顺序排在前面，
/// 没拖过的（0）按名字排在其后。全都还是 0 时就是纯按名字 —— 与拖动排序上线前一致。
const AUTHOR_ORDER_BY: &str =
    " ORDER BY CASE WHEN a.sort_order = 0 THEN 1 ELSE 0 END, a.sort_order, a.name COLLATE NOCASE";

#[tauri::command]
fn list_authors() -> Result<Vec<AuthorSummary>, String> {
    let conn = db()?;
    let sql = format!(
        "SELECT a.id, a.name, a.homepage, a.avatar_path, a.notes, a.preview_dir, a.purchased_dir, a.match_threshold, a.pixiv_last_sync_at, a.avatar_managed,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.purchased_path <> ''),
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.favorite = 1),
          a.aliases,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.has_images = 1),
          a.starred
        FROM authors a{AUTHOR_ORDER_BY}"
    );
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
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
                aliases: row.get(13)?,
                images_count: row.get(14)?,
                starred: row.get::<_, i64>(15)? == 1,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 保存作者库的手动顺序（v0.3.69）。
///
/// 传入的是**完整**的作者 ID 列表（按用户拖完之后的显示顺序），按下标写 1..n；
/// 没出现在列表里的作者保持 0（= 还没手动排过，仍按名字排在手动排过的后面）。
#[tauri::command]
fn set_author_order(author_ids: Vec<i64>) -> Result<(), String> {
    let mut conn = db()?;
    let transaction = conn.transaction().map_err(|e| e.to_string())?;
    for (index, author_id) in author_ids.iter().enumerate() {
        transaction
            .execute(
                "UPDATE authors SET sort_order=?1 WHERE id=?2",
                params![index as i64 + 1, author_id],
            )
            .map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn save_author(author: AuthorInput) -> Result<AuthorSummary, String> {
    if author.name.trim().is_empty() {
        return Err("作者名称不能为空".into());
    }
    let conn = db()?;
    let homepage = normalize_author_homepage(&author.homepage);
    let aliases = normalize_aliases(&author.aliases);
    if !homepage.is_empty() {
        let existing: Option<String> = conn
            .query_row(
                "SELECT name FROM authors WHERE homepage=?1 AND id <> COALESCE(?2, -1)",
                params![homepage, author.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(name) = existing {
            return Err(format!("作者主页已存在，名称为“{name}”"));
        }
    }
    let id = if let Some(id) = author.id {
        conn.execute("UPDATE authors SET name=?1, homepage=?2, avatar_path=?3, avatar_managed=?4, notes=?5, preview_dir=?6, purchased_dir=?7, aliases=?8 WHERE id=?9", params![author.name.trim(), homepage, author.avatar_path, author.avatar_managed as i64, author.notes, author.preview_dir, author.purchased_dir, aliases, id]).map_err(|e| e.to_string())?;
        id
    } else {
        conn.execute("INSERT INTO authors (name, homepage, avatar_path, avatar_managed, notes, preview_dir, purchased_dir, aliases) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![author.name.trim(), homepage, author.avatar_path, author.avatar_managed as i64, author.notes, author.preview_dir, author.purchased_dir, aliases]).map_err(|e| e.to_string())?;
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
fn save_app_settings(mut settings: AppSettings) -> Result<AppSettings, String> {
    if settings.pixiv_delay_threshold == 0 {
        return Err("抓取数量阈值必须至少为 1".into());
    }
    if settings.pixiv_delay_seconds > 60 {
        return Err("抓取间隔不能超过 60 秒".into());
    }
    // 搜索网站：名字要有、网址得是 http(s) 且留着最后一个 `=`（它后面是搜索词的位置）
    if settings.search_sites.len() > 20 {
        return Err("搜索网站最多 20 个".into());
    }
    for site in settings.search_sites.iter_mut() {
        site.name = site.name.trim().to_string();
        site.url = site.url.trim().to_string();
        if site.name.is_empty() {
            return Err("搜索网站的名字不能为空".into());
        }
        if !(site.url.starts_with("https://") || site.url.starts_with("http://")) {
            return Err(format!(
                "搜索网站「{}」的网址需要以 http:// 或 https:// 开头",
                site.name
            ));
        }
        if !site.url.contains('=') {
            return Err(format!(
                "搜索网站「{}」的网址里要带上一个 =，它后面就是搜索词的位置",
                site.name
            ));
        }
    }
    // 加速镜像：统一存成 `https://xxx/` 的样子（拼地址时直接接在后面）
    if settings.update_mirrors.len() > 10 {
        return Err("加速镜像最多留 10 条".into());
    }
    let mut cleaned_mirrors = Vec::new();
    for mirror in &settings.update_mirrors {
        let mirror = mirror.trim().trim_end_matches('/').to_string();
        if mirror.is_empty() {
            continue;
        }
        if !(mirror.starts_with("https://") || mirror.starts_with("http://")) {
            return Err(format!("加速镜像「{mirror}」需要以 http:// 或 https:// 开头"));
        }
        cleaned_mirrors.push(format!("{mirror}/"));
    }
    settings.update_mirrors = cleaned_mirrors;
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
        ("auto_group_dir", settings.auto_group_dir),
        ("similarity_threshold", settings.similarity_threshold.to_string()),
        ("min_similarity_threshold", settings.min_similarity_threshold.to_string()),
        ("match_title_length", settings.match_title_length.to_string()),
        (
            "image_quality",
            if settings.image_quality == "original" {
                "original".into()
            } else {
                "1200".into()
            },
        ),
        (
            "sync_image_format",
            if settings.sync_image_format == "epub" {
                "epub".into()
            } else {
                "html".into()
            },
        ),
        (
            "search_sites",
            serde_json::to_string(&settings.search_sites).map_err(|e| e.to_string())?,
        ),
        (
            "auto_check_update",
            if settings.auto_check_update {
                "1".into()
            } else {
                "0".into()
            },
        ),
        (
            "update_mirrors",
            serde_json::to_string(&settings.update_mirrors).map_err(|e| e.to_string())?,
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
    images_only: bool,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let field = if search_field == "tags" {
        "tags"
    } else {
        "title"
    };
    let mut sql = format!("SELECT author_id, id, title, release_date, preview_path, cover_path, purchased_path, favorite, has_images, tags, pixiv_novel_id, series_id, series_title, series_order, is_new, '' AS author_name, image_count FROM works WHERE author_id = ?1 AND (?2 = '' OR {field} LIKE ?3)");
    match status.as_str() {
        "purchased" => sql.push_str(" AND purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND favorite = 1");
    }
    if images_only {
        sql.push_str(" AND has_images = 1");
    }
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY release_date ASC, id ASC",
        "title_asc" => " ORDER BY title COLLATE NOCASE ASC",
        // 「字数从多到少」的字数要读文件才知道，SQL 排不了：先按日期打个底，取回数据后再在 Rust 里重排
        "words_desc" => " ORDER BY release_date DESC, id DESC",
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
        populate_work_display_info(work);
    }
    if sort == "words_desc" {
        sort_works_by_content_size(&mut works);
    }
    Ok(works)
}

#[tauri::command]
fn list_all_works(
    query: String,
    search_field: String,
    status: String,
    favorites_only: bool,
    images_only: bool,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let field = if search_field == "tags" {
        "w.tags"
    } else {
        "w.title"
    };
    let mut sql = format!("SELECT w.author_id, w.id, w.title, w.release_date, w.preview_path, w.cover_path, w.purchased_path, w.favorite, w.has_images, w.tags, w.pixiv_novel_id, w.series_id, w.series_title, w.series_order, w.is_new, a.name AS author_name, w.image_count FROM works w JOIN authors a ON a.id=w.author_id WHERE (?1 = '' OR {field} LIKE ?2)");
    match status.as_str() {
        "purchased" => sql.push_str(" AND w.purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND w.purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND w.favorite = 1");
    }
    if images_only {
        sql.push_str(" AND w.has_images = 1");
    }
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY w.release_date ASC, w.id ASC",
        "title_asc" => " ORDER BY w.title COLLATE NOCASE ASC",
        // 同 list_works：「字数从多到少」只在 SQL 里给个日期底序，最终顺序在 Rust 里排
        "words_desc" => " ORDER BY w.release_date DESC, w.id DESC",
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
        populate_work_display_info(work);
    }
    if sort == "words_desc" {
        sort_works_by_content_size(&mut works);
    }
    Ok(works)
}

#[tauri::command]
fn list_series_works(author_id: i64, series_id: String) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT author_id, id, title, release_date, preview_path, cover_path, purchased_path, favorite, has_images, tags, pixiv_novel_id, series_id, series_title, series_order, is_new, '' AS author_name, image_count FROM works WHERE author_id=?1 AND series_id=?2 ORDER BY CASE WHEN series_order > 0 THEN 0 ELSE 1 END, series_order ASC, id ASC")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![author_id, series_id], map_work)
        .map_err(|e| e.to_string())?;
    let mut works = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for work in &mut works {
        populate_work_display_info(work);
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

/// 路径比较用的键：统一分隔符、去掉 Windows 长路径前缀 `\\?\`、忽略大小写。
/// 只用于「这两个字符串是不是同一个文件」的判断，不要拿去写库或展示。
fn path_key(path: &str) -> String {
    let trimmed = path.trim();
    let without_prefix = trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed);
    without_prefix.replace('/', "\\").to_lowercase()
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

/// 按设置截取作品名参与匹配的最大长度。
/// `limit <= 0` 表示不截取（匹配完整作品名），超过则只保留前 `limit` 个字。
fn clip_match_title(title: &str, limit: i64) -> String {
    if limit <= 0 {
        return title.to_string();
    }
    let limit = limit as usize;
    let mut clipped: String = title.chars().take(limit).collect();
    // 去掉截断后残留的尾部空白，避免影响归一化后的比较
    let trimmed_len = clipped.trim_end().len();
    clipped.truncate(trimmed_len);
    clipped
}

/// 作品名按设置截取后再与文件名估算相似度。
/// 作品名很长、而文件名只取了前面一段时，截取参与匹配的部分能提高匹配率。
fn similarity_with_title_limit(work_title: &str, file_name: &str, title_limit: i64) -> i64 {
    if title_limit <= 0 {
        return similarity_percent(work_title, file_name);
    }
    similarity_percent(&clip_match_title(work_title, title_limit), file_name)
}

/// 作品名与文件名的相似度（**分母是两边里较长的那个长度**，v0.3.71 改）。
///
/// 算法：两边各自归一化（去扩展名 → 去开头 `YYYY-MM-DD` → 只留字母数字并小写）后，
/// 取**最长公共连续子串**（连续、顺序一致），再除以**作品名与文件名里更长的那一边的长度**。
/// 于是「作品名 ABCD / 文件名 AB」= 2/4 = 50%，「作品名 ABC / 文件名 ABCDEF」= 3/6 = 50%；
/// 两边任意一侧多出来的内容都会按比例扣分 —— 文件名带日期 / 「完整版」这类后缀时不会再
/// 因为「整段包含了作品名」就直接拿满分。
fn similarity_percent(work_title: &str, file_name: &str) -> i64 {
    let left = normalized_match_name(work_title);
    let right = normalized_match_name(file_name);
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
    (longest * 100 / left.len().max(right.len())) as i64
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

/// 作者主页统一只保留到「/users/<数字>」为止。
/// `https://www.pixiv.net/users/16208053/novels` → `https://www.pixiv.net/users/16208053`；
/// 老式的 `.../member.php?id=16208053` 也会收敛成规范写法；识别不出的原样返回。
fn normalize_author_homepage(homepage: &str) -> String {
    let trimmed = homepage.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some((prefix, suffix)) = trimmed.split_once("/users/") {
        let id: String = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if !id.is_empty() {
            return format!("{}/users/{}", prefix.trim_end_matches('/'), id);
        }
    }
    if let Some((_, suffix)) = trimmed.split_once("id=") {
        let id: String = suffix
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if !id.is_empty() {
            return format!("https://www.pixiv.net/users/{id}");
        }
    }
    trimmed.to_string()
}

/// 别名统一存成 `a|b|c`（和作品标签一致）：去空白、去空项、去重（忽略大小写）
fn normalize_aliases(aliases: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    aliases
        .split('|')
        .map(|alias| alias.trim().to_string())
        .filter(|alias| !alias.is_empty() && seen.insert(alias.to_lowercase()))
        .collect::<Vec<_>>()
        .join("|")
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

fn pixiv_novel_id_from_url(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let value = value.split('#').next().unwrap_or(value);
    let Some((base, query)) = value.split_once('?') else {
        return Err("单篇同步请输入有效的 Pixiv 小说链接。".into());
    };
    if base != "https://www.pixiv.net/novel/show.php" {
        return Err("单篇同步请输入有效的 Pixiv 小说链接。".into());
    }
    let novel_id = query
        .split('&')
        .find_map(|item| item.strip_prefix("id="))
        .unwrap_or_default();
    if novel_id.is_empty() || !novel_id.chars().all(|character| character.is_ascii_digit()) {
        return Err("单篇同步请输入有效的 Pixiv 小说链接。".into());
    }
    Ok(Some(novel_id.to_string()))
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

fn synopsis_indicates_preview(description: &str) -> bool {
    let compact: String = description
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    // 「购买」是作者给完整版留的出处暗号：简介里带它，Pixiv 上这份就只是预览
    // （v0.3.63 加的是「购买途径」，v0.3.66 按用户要求放宽成一个「购买」）
    if compact.contains("购买") {
        return true;
    }
    compact.contains("全文") && !compact.contains("全文放出")
}

/// Pixiv 小说标题前 15 个字里出现「插画」或「图文」时，判定为带图版。
/// 只检查标题开头，避免标题尾部出现「无插画」这类说明时被误判。
fn title_indicates_images(title: &str) -> bool {
    let head: String = title.chars().take(15).collect();
    head.contains("插画") || head.contains("图文")
}

/// 新建本地作品（文件夹扫描 / 批量文本导入）时统一走这里。
/// 标题前 15 字带「插画」「图文」的，建库当场就标为带图版，判定规则与 Pixiv 同步完全一致，
/// 免得本地导入的作品还要事后补标。返回新作品 id。
fn insert_local_work(
    conn: &Connection,
    author_id: i64,
    title: &str,
    release_date: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO works (author_id, title, release_date, has_images) VALUES (?1, ?2, ?3, ?4)",
        params![
            author_id,
            title,
            release_date,
            title_indicates_images(title) as i64
        ],
    )?;
    Ok(conn.last_insert_rowid())
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

fn has_invalid_date_range(start: Option<NaiveDate>, end: Option<NaiveDate>) -> bool {
    matches!((start, end), (Some(start), Some(end)) if start > end)
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

/// 配图文件夹（`{标题}_images`）与图文 HTML 是同步的附属产物，
/// 不能当成「作品的本地文件」参与相似度匹配，否则会被误判成另一个版本。
fn is_generated_asset(path: &Path) -> bool {
    let named_images_dir = path
        .file_name()
        .map(|name| name.to_string_lossy().ends_with("_images"))
        .unwrap_or(false);
    named_images_dir
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| {
                extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("epub")
            })
            .unwrap_or(false)
}

/// 阅读版里到底有几张图。只有 `epub` / `html` 数得出来，其它格式返回 `None`。
/// 口径与「已下载配图数」一致：不含封面那一张。
fn count_reading_images(path: &Path, cover_name: &str) -> Option<usize> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("epub") => count_epub_images(path, cover_name),
        Some("html") | Some("htm") => fs::read(path)
            .ok()
            .map(|bytes| count_html_images(&String::from_utf8_lossy(&bytes), cover_name)),
        _ => None,
    }
}

fn is_image_file_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "avif"
            )
        })
        .unwrap_or(false)
}

/// 关联作品文件时要跳过的图像附属物：封面 / 插图（jpg、png…），
/// 以及以 `_images` 结尾的配图文件夹。能当作品文件的只有文本、电子书和 HTML。
fn is_image_like_asset(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.to_lowercase().ends_with("_images") {
        return true;
    }
    if path.is_dir() {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg"
                    | "jpeg"
                    | "png"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "tif"
                    | "tiff"
                    | "svg"
                    | "avif"
                    | "ico"
                    | "heic"
                    | "psd"
            )
        })
        .unwrap_or(false)
}

/// 这张图是不是封面：作品自己的封面文件名，或通行的 `cover` / `封面`。
fn is_cover_entry(file_name: &str, cover_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    if !cover_name.is_empty() && lower == cover_name.to_lowercase() {
        return true;
    }
    let stem = Path::new(&lower)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    stem == "cover" || stem == "封面"
}

/// 数 HTML 里的 `<img>`，封面那张（带 `cover` 类，或指向作品封面文件）不算配图。
fn count_html_images(content: &str, cover_name: &str) -> usize {
    let lower = content.to_lowercase();
    let cover = cover_name.to_lowercase();
    let mut count = 0;
    let mut cursor = 0;
    while let Some(offset) = lower.get(cursor..).and_then(|rest| rest.find("<img")) {
        let start = cursor + offset;
        let end = match lower[start..].find('>') {
            Some(value) => start + value,
            None => lower.len(),
        };
        let tag = &lower[start..end];
        let is_cover = tag.contains("class=\"cover\"")
            || tag.contains("class='cover'")
            || (!cover.is_empty() && tag.contains(&cover));
        if !is_cover {
            count += 1;
        }
        if end + 1 <= cursor {
            break;
        }
        cursor = end + 1;
    }
    count
}

/// 数 EPUB 包里的图片条目。只读 ZIP 尾部的中央目录（几 KB 的 IO），不解压。
fn count_epub_images(path: &Path, cover_name: &str) -> Option<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    if size < 22 {
        return None;
    }
    let tail_len = size.min(22 + 0xFFFF) as usize;
    let mut tail = vec![0u8; tail_len];
    file.seek(SeekFrom::Start(size - tail_len as u64)).ok()?;
    file.read_exact(&mut tail).ok()?;
    let mut eocd = None;
    let mut index = tail_len - 22;
    loop {
        if tail[index..index + 4] == [0x50, 0x4b, 0x05, 0x06] {
            eocd = Some(index);
            break;
        }
        if index == 0 {
            break;
        }
        index -= 1;
    }
    let eocd = eocd?;
    let declared = u16::from_le_bytes([tail[eocd + 10], tail[eocd + 11]]) as usize;
    let directory_len =
        u32::from_le_bytes([tail[eocd + 12], tail[eocd + 13], tail[eocd + 14], tail[eocd + 15]])
            as usize;
    let directory_start =
        u32::from_le_bytes([tail[eocd + 16], tail[eocd + 17], tail[eocd + 18], tail[eocd + 19]])
            as u64;
    // ZIP64 的字段是占位符，这里不猜
    if declared == u16::MAX as usize || directory_start == u32::MAX as u64 {
        return None;
    }
    let mut directory = vec![0u8; directory_len];
    file.seek(SeekFrom::Start(directory_start)).ok()?;
    file.read_exact(&mut directory).ok()?;
    let mut count = 0;
    let mut cursor = 0usize;
    while cursor + 46 <= directory.len() {
        if directory[cursor..cursor + 4] != [0x50, 0x4b, 0x01, 0x02] {
            break;
        }
        let name_len = u16::from_le_bytes([directory[cursor + 28], directory[cursor + 29]]) as usize;
        let extra_len = u16::from_le_bytes([directory[cursor + 30], directory[cursor + 31]]) as usize;
        let comment_len =
            u16::from_le_bytes([directory[cursor + 32], directory[cursor + 33]]) as usize;
        let name_start = cursor + 46;
        let name_end = name_start + name_len;
        if name_end > directory.len() {
            break;
        }
        let entry = String::from_utf8_lossy(&directory[name_start..name_end]).to_string();
        let file_name = entry.rsplit('/').next().unwrap_or("");
        if !entry.ends_with('/')
            && !entry.to_ascii_lowercase().starts_with("meta-inf/")
            && is_image_file_name(file_name)
            && !is_cover_entry(file_name, cover_name)
        {
            count += 1;
        }
        cursor = name_end + extra_len + comment_len;
    }
    Some(count)
}

/// 作品当前绑的文件若是 `epub` / `html`，把里面的图片数写进 `works.image_count`，
/// 作品卡上的图片角标就跟着更新。返回这次有没有真的数出来（数不出来就保持原值）：
/// 不是这两种格式、文件不在、ZIP 读不动，都会返回 `false`。
fn refresh_reading_image_count(conn: &Connection, work_id: i64) -> bool {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT preview_path, purchased_path, cover_path FROM works WHERE id=?1",
            [work_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .ok()
        .flatten();
    let Some((preview_path, purchased_path, cover_path)) = row else {
        return false;
    };
    let bound = if purchased_path.trim().is_empty() {
        preview_path
    } else {
        purchased_path
    };
    if bound.trim().is_empty() {
        return false;
    }
    let cover_name = Path::new(&cover_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let Some(count) = count_reading_images(Path::new(&bound), &cover_name) else {
        return false;
    };
    conn.execute(
        "UPDATE works SET image_count=?1 WHERE id=?2 AND image_count<>?1",
        params![count as i64, work_id],
    )
    .is_ok()
}

fn sync_preview_entries(
    preview_dir: &Path,
    minimum_file_size_bytes: u64,
) -> Result<Vec<SyncPreviewEntry>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(preview_dir).map_err(|e| format!("无法读取预览版文件夹：{e}"))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if is_generated_asset(&path) {
            continue;
        }
        let is_cover = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| {
                extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
            })
            .unwrap_or(false);
        let is_preview = path.is_dir()
            || path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("txt"))
                .unwrap_or(false);
        if !is_cover && !is_preview {
            continue;
        }
        if path.is_file()
            && is_preview
            && entry.metadata().map_err(|e| e.to_string())?.len() < minimum_file_size_bytes
        {
            continue;
        }
        let name = stem(&path);
        if !name.is_empty() {
            entries.push(SyncPreviewEntry {
                path,
                name,
                is_preview,
            });
        }
    }
    Ok(entries)
}

fn matched_sync_preview(
    entries: &[SyncPreviewEntry],
    title: &str,
    threshold: i64,
    title_limit: i64,
) -> Option<(PathBuf, Option<PathBuf>)> {
    let mut matches: Vec<(&SyncPreviewEntry, i64)> = entries
        .iter()
        .filter(|entry| entry.is_preview)
        .map(|entry| (entry, similarity_with_title_limit(title, &entry.name, title_limit)))
        .filter(|(_, score)| *score >= threshold)
        .collect();
    matches.sort_by(|left, right| right.1.cmp(&left.1));
    let (preview, best_score) = matches.first()?;
    if matches
        .iter()
        .filter(|(_, score)| *score == *best_score)
        .count()
        != 1
    {
        return None;
    }
    let cover = entries
        .iter()
        .find(|entry| !entry.is_preview && name_key(&entry.name) == name_key(&preview.name))
        .map(|entry| entry.path.clone());
    Some((preview.path.clone(), cover))
}

/// 移动文件或目录；跨盘导致 rename 失败时退回「复制 + 删源」。
/// 源不存在时直接返回成功（调用方常拿来搬可有可无的同名资源）。
fn move_path(source: &Path, destination: &Path) -> Result<(), String> {
    if source == destination || !source.exists() || destination.exists() {
        return Ok(());
    }
    if fs::rename(source, destination).is_ok() {
        return Ok(());
    }
    if source.is_dir() {
        copy_directory(source, destination)?;
        fs::remove_dir_all(source).map_err(|e| e.to_string())?;
    } else {
        fs::copy(source, destination).map_err(|e| e.to_string())?;
        fs::remove_file(source).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 与正文同名的附属产物：配图文件夹、图文 HTML、封面、EPUB。
/// 作品在预览版/完整版之间搬家时一起走。
fn sibling_assets(text_path: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(stem)) = (text_path.parent(), text_path.file_stem()) else {
        return Vec::new();
    };
    let stem = stem.to_string_lossy();
    vec![
        parent.join(format!("{stem}_images")),
        parent.join(format!("{stem}.html")),
        parent.join(format!("{stem}.jpg")),
        parent.join(format!("{stem}.epub")),
    ]
}

/// 把 cover_path 指向的那张封面也搬到目标目录。
///
/// `sibling_assets` 只认「与正文同名」的 `{正文名}.jpg`；封面若是别的文件名
/// （用户手动指定过、或 Pixiv 原始命名），就得单独跟着走一趟。
/// `move_path` 对「源不存在 / 目标已存在」是幂等的，重复调用安全。
fn move_cover_along(cover_path: &str, target_dir: &Path) -> Result<(), String> {
    if cover_path.trim().is_empty() {
        return Ok(());
    }
    let cover = Path::new(cover_path);
    if let Some(name) = cover.file_name() {
        move_path(cover, &target_dir.join(name))?;
    }
    Ok(())
}

/// 作品搬家后，算出库里 cover_path 应该改成什么。
///
/// 背景（2026-09-14 封面集体挂掉事故的根因）：以前只更新 `purchased_path` /
/// `preview_path`，`cover_path` 留在老目录不动。老目录平时还在，显示看着正常；
/// 一旦那个目录被清理，所有封面瞬间变成死路径。
///
/// 顺序：先认「新正文旁的同名图」（约定位置），再认「原封面文件名也搬到了新目录」。
/// 两者都没有就原样返回——不动库里的值，免得把用户手动指定的封面抹掉。
fn follow_cover_path(old_cover: &str, new_text: &Path) -> String {
    for ext in ["jpg", "jpeg", "png", "webp"] {
        let candidate = new_text.with_extension(ext);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    if !old_cover.trim().is_empty() {
        if let Some(name) = Path::new(old_cover).file_name() {
            let candidate = new_text.with_file_name(name);
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    old_cover.to_string()
}

/// 把预览版目录里的作品搬到完整版目录（单份模型：一个作品只占一边）。
/// 正文、配图文件夹、图文 HTML、封面都会跟着搬。
fn move_sync_preview_to_purchased(
    preview_path: &Path,
    cover_path: Option<&Path>,
    purchased_dir: &Path,
) -> Result<String, String> {
    let name = preview_path
        .file_name()
        .ok_or_else(|| "预览版文件名称无效".to_string())?;
    let purchased_preview = purchased_dir.join(name);
    move_path(preview_path, &purchased_preview)?;
    for asset in sibling_assets(preview_path) {
        if let Some(asset_name) = asset.file_name() {
            move_path(&asset, &purchased_dir.join(asset_name))?;
        }
    }
    if let Some(cover) = cover_path {
        if let Some(cover_name) = cover.file_name() {
            move_path(cover, &purchased_dir.join(cover_name))?;
        }
    }
    Ok(purchased_preview.to_string_lossy().to_string())
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

// ---------------------------------------------------------------------------
// 图文小说
//
// Pixiv 小说正文里的 `[uploadedimage:ID]` 是作者插进正文的配图，直链在同一个详情
// 响应的 `body.textEmbeddedImages[ID].urls` 里；`[pixivimage:ID]` 是引用别人的插画
// 作品，ID 是插画 ID，得再查一次插画接口。图片都放在防盗链后面，必须带 Referer。
// ---------------------------------------------------------------------------

/// 正文里的一处图片引用，按出现顺序排列并去重。
struct NovelImageRef {
    token: String,
    id: String,
    external: bool,
}

fn novel_image_refs(content: &str) -> Vec<NovelImageRef> {
    let mut refs: Vec<NovelImageRef> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rest = content;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else {
            break;
        };
        let inner = &after[..end];
        rest = &after[end + 1..];
        let (external, id) = if let Some(id) = inner.strip_prefix("uploadedimage:") {
            (false, id)
        } else if let Some(id) = inner.strip_prefix("pixivimage:") {
            (true, id)
        } else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || !id.chars().all(|character| character.is_ascii_digit()) {
            continue;
        }
        let token = format!("[{inner}]");
        if seen.insert(token.clone()) {
            refs.push(NovelImageRef {
                token,
                id: id.to_string(),
                external,
            });
        }
    }
    refs
}

fn novel_image_filename_extension(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "png",
        "gif" => "gif",
        "webp" => "webp",
        _ => "jpg",
    }
}

/// 设置项里的画质：默认取 1200 宽的 jpg，也可以要作者上传的原图（可能是几 MB 的 PNG）。
fn novel_image_quality_key(quality: &str) -> &'static str {
    if quality == "original" {
        "original"
    } else {
        "1200x1200"
    }
}

/// 从详情的 `textEmbeddedImages` 里取本文配图直链，按设置项画质挑一档。
fn novel_image_url(embedded: &Value, id: &str, quality: &str) -> Option<String> {
    let urls = embedded.get(id)?.get("urls")?.as_object()?;
    for key in [
        novel_image_quality_key(quality),
        "1200x1200",
        "original",
        "480mw",
        "240mw",
    ] {
        if let Some(url) = urls.get(key).and_then(Value::as_str) {
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// `[pixivimage:ID]` 引用的是别的插画作品，直链要单独查一遍。
fn fetch_pixiv_illust_url(client: &Client, illust_id: &str, quality: &str) -> Option<String> {
    let value: Value = client
        .get(format!("https://www.pixiv.net/ajax/illust/{illust_id}"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    if value.get("error").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let urls = value.get("body")?.get("urls")?.as_object()?;
    let keys: [&str; 3] = if quality == "original" {
        ["original", "regular", "small"]
    } else {
        ["regular", "original", "small"]
    };
    for key in keys {
        if let Some(url) = urls.get(key).and_then(Value::as_str) {
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// 一篇作品待下载的一张配图。
struct NovelImageSlot {
    token: String,
    order: usize,
    file_name: String,
    relative: String,
    url: String,
    external: bool,
}

fn novel_images_dir(text_path: &Path) -> PathBuf {
    let stem = text_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "Pixiv 小说".into());
    text_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}_images"))
}

/// 解析正文里的配图并定好落盘文件名；拿不到直链的引用直接跳过（正文里保留原标记）。
fn plan_novel_images(
    client: &Client,
    embedded: &Value,
    content: &str,
    text_path: &Path,
    quality: &str,
) -> Vec<NovelImageSlot> {
    let folder = novel_images_dir(text_path);
    let folder_name = folder
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut slots = Vec::new();
    for (index, reference) in novel_image_refs(content).iter().enumerate() {
        let url = if reference.external {
            fetch_pixiv_illust_url(client, &reference.id, quality)
        } else {
            novel_image_url(embedded, &reference.id, quality)
        };
        let Some(url) = url else {
            continue;
        };
        let file_name = format!(
            "{:03}.{}",
            index + 1,
            novel_image_filename_extension(&url)
        );
        slots.push(NovelImageSlot {
            token: reference.token.clone(),
            order: index + 1,
            relative: format!("{folder_name}/{file_name}"),
            file_name,
            url,
            external: reference.external,
        });
    }
    slots
}

/// 下载配图，返回（成功张数，失败张数）。已存在的图不重复下载。
fn download_novel_images(
    client: &Client,
    slots: &[NovelImageSlot],
    images_dir: &Path,
) -> (usize, usize) {
    if slots.is_empty() {
        return (0, 0);
    }
    if fs::create_dir_all(images_dir).is_err() {
        return (0, slots.len());
    }
    let mut saved = 0;
    let mut failed = 0;
    for slot in slots {
        let target = images_dir.join(&slot.file_name);
        if target.exists() {
            saved += 1;
            continue;
        }
        match fetch_pixiv_cover(client, &slot.url) {
            Ok(bytes) if !bytes.is_empty() => {
                let temp = target.with_extension("part");
                let written = fs::write(&temp, &bytes)
                    .and_then(|_| fs::rename(&temp, &target))
                    .is_ok();
                if written {
                    saved += 1;
                } else {
                    let _ = fs::remove_file(&temp);
                    failed += 1;
                }
            }
            _ => failed += 1,
        }
    }
    (saved, failed)
}

/// 正文里的配图标记换成指向同目录配图的指引，纯文本阅读器里也能看出插图位置。
fn rewrite_novel_txt(content: &str, slots: &[NovelImageSlot]) -> String {
    let mut text = content.to_string();
    for slot in slots {
        let label = if slot.external { "引用插画" } else { "插图" };
        text = text.replace(
            &slot.token,
            &format!("\n[{label} {}：{}]\n", slot.order, slot.relative),
        );
    }
    text
}

fn escape_novel_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 正文里剩下的 Pixiv 标记，HTML 版渲染成真正的排版。
enum NovelSegment {
    Text(String),
    Ruby(String, String),
    Chapter(String),
    NewPage,
    Image(usize, bool),
}

fn match_novel_marker(value: &str) -> Option<(NovelSegment, usize)> {
    debug_assert!(value.starts_with('['));
    let end = value.find(']')?;
    let inner = &value[1..end];
    if let Some(body) = inner.strip_prefix("[rb:") {
        let closing = value.find("]]")?;
        let (base, reading) = match body.split_once('>') {
            Some((base, reading)) => (base.trim(), reading.trim()),
            None => (body.trim(), ""),
        };
        return Some((
            NovelSegment::Ruby(base.to_string(), reading.to_string()),
            closing + 2,
        ));
    }
    if let Some(title) = inner.strip_prefix("chapter:") {
        return Some((NovelSegment::Chapter(title.trim().to_string()), end + 1));
    }
    if inner == "newpage" {
        return Some((NovelSegment::NewPage, end + 1));
    }
    if let Some(rest) = inner.strip_prefix("img:") {
        let mut parts = rest.splitn(2, ':');
        let order = parts.next()?.parse::<usize>().ok()?;
        let external = parts.next().unwrap_or("") == "引用插画";
        return Some((NovelSegment::Image(order, external), end + 1));
    }
    None
}

fn novel_segments(content: &str) -> Vec<NovelSegment> {
    let mut segments: Vec<NovelSegment> = Vec::new();
    let mut text = String::new();
    let mut rest = content;
    while let Some(start) = rest.find('[') {
        if let Some((segment, consumed)) = match_novel_marker(&rest[start..]) {
            // 标记前面的正文要先收进来，否则一段一段地丢字。
            if start > 0 {
                text.push_str(&rest[..start]);
            }
            if !text.is_empty() {
                segments.push(NovelSegment::Text(std::mem::take(&mut text)));
            }
            segments.push(segment);
            rest = &rest[start + consumed..];
            continue;
        }
        text.push_str(&rest[..start + 1]);
        rest = &rest[start + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        segments.push(NovelSegment::Text(text));
    }
    segments
}

struct NovelHtmlMeta<'a> {
    title: &'a str,
    author_name: &'a str,
    release_date: &'a str,
    tags: &'a str,
    cover_file: &'a str,
    image_count: usize,
}

/// 生成图文版 HTML：封面 + 标题头 + 正文（配图、注音、章节、分页都渲染出来）。
/// 图片走同目录相对路径，所以整个文件夹拷走也能照常显示。
fn render_novel_html(content: &str, slots: &[NovelImageSlot], meta: &NovelHtmlMeta<'_>) -> String {
    let mut work = content.to_string();
    for slot in slots {
        let label = if slot.external { "引用插画" } else { "插图" };
        work = work.replace(&slot.token, &format!("[img:{}:{label}]", slot.order));
    }
    let mut body = String::new();
    let mut paragraph = String::new();
    let flush = |body: &mut String, paragraph: &mut String| {
        let trimmed = paragraph.trim();
        if !trimmed.is_empty() {
            body.push_str("<p>");
            body.push_str(&trimmed.replace('\n', "<br>"));
            body.push_str("</p>\n");
        }
        paragraph.clear();
    };
    for segment in novel_segments(&work) {
        match segment {
            NovelSegment::Text(value) => {
                for line in value.split('\n') {
                    if line.trim().is_empty() {
                        flush(&mut body, &mut paragraph);
                    } else {
                        if !paragraph.is_empty() {
                            paragraph.push('\n');
                        }
                        paragraph.push_str(&escape_novel_html(line));
                    }
                }
            }
            NovelSegment::Ruby(base, reading) => {
                paragraph.push_str(&format!(
                    "<ruby>{}<rt>{}</rt></ruby>",
                    escape_novel_html(&base),
                    escape_novel_html(&reading)
                ));
            }
            NovelSegment::Chapter(title) => {
                flush(&mut body, &mut paragraph);
                body.push_str(&format!("<h2>{}</h2>\n", escape_novel_html(&title)));
            }
            NovelSegment::NewPage => {
                flush(&mut body, &mut paragraph);
                body.push_str("<hr class=\"page\">\n");
            }
            NovelSegment::Image(order, external) => {
                let Some(slot) = slots.iter().find(|slot| slot.order == order) else {
                    continue;
                };
                flush(&mut body, &mut paragraph);
                let caption = if external { "引用插画" } else { "插图" };
                body.push_str(&format!(
                    "<figure><img src=\"{}\" alt=\"{caption} {order}\" loading=\"lazy\"><figcaption>{caption} {order}</figcaption></figure>\n",
                    escape_novel_html(&slot.relative)
                ));
            }
        }
    }
    flush(&mut body, &mut paragraph);
    let cover = if meta.cover_file.is_empty() {
        String::new()
    } else {
        format!(
            "<img class=\"cover\" src=\"{}\" alt=\"封面\">",
            escape_novel_html(meta.cover_file)
        )
    };
    let tags = meta
        .tags
        .split(['|', ','])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(|tag| format!("<span class=\"tag\">{}</span>", escape_novel_html(tag)))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
:root {{ color-scheme: light; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; padding: 32px 16px 72px; background: #f6f7f9; color: #1f2328;
  font-family: "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", sans-serif; line-height: 1.9; }}
main {{ max-width: 760px; margin: 0 auto; background: #fff; border-radius: 14px;
  padding: 32px 40px 48px; box-shadow: 0 2px 16px rgba(15, 23, 42, .08); }}
h1 {{ margin: 18px 0 6px; font-size: 24px; line-height: 1.45; }}
h2 {{ margin: 34px 0 12px; font-size: 19px; border-left: 4px solid #4a90d9; padding-left: 10px; }}
p {{ margin: 0 0 16px; text-align: justify; }}
.meta {{ color: #667085; font-size: 13px; }}
.tags {{ margin: 10px 0 4px; }}
.tag {{ display: inline-block; margin: 0 6px 6px 0; padding: 2px 9px; border-radius: 999px;
  background: #eef2f7; color: #475467; font-size: 12px; }}
.cover {{ display: block; width: 100%; max-width: 460px; margin: 0 auto 18px; border-radius: 10px; }}
figure {{ margin: 22px 0; text-align: center; }}
figure img {{ max-width: 100%; border-radius: 10px; }}
figcaption {{ margin-top: 6px; color: #98a2b3; font-size: 12px; }}
hr.page {{ border: 0; border-top: 1px dashed #d0d5dd; margin: 28px 0; }}
ruby rt {{ font-size: 10px; color: #98a2b3; }}
</style></head>
<body><main>
{cover}
<h1>{title}</h1>
<p class="meta">{author_name}{date}{images}</p>
<div class="tags">{tags}</div>
{body}
</main></body></html>
"#,
        title = escape_novel_html(meta.title),
        cover = cover,
        author_name = escape_novel_html(meta.author_name),
        date = if meta.release_date.is_empty() {
            String::new()
        } else {
            format!(" · {}", escape_novel_html(meta.release_date))
        },
        images = if meta.image_count == 0 {
            String::new()
        } else {
            format!(" · 配图 {} 张", meta.image_count)
        },
        tags = tags,
        body = body,
    )
}

/// 图文版 HTML 与正文同目录、同名。
fn novel_html_path(text_path: &Path) -> PathBuf {
    text_path.with_extension("html")
}

/// 原子写文件：先写 .part 再改名，避免读到半截文件。
fn write_bytes_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("part");
    fs::write(&temp, content).map_err(|e| e.to_string())?;
    fs::rename(&temp, path).map_err(|e| e.to_string())
}

fn write_text_atomic(path: &Path, content: &str) -> Result<(), String> {
    write_bytes_atomic(path, content.as_bytes())
}

// ---------------------------------------------------------------------------
// EPUB 导出
//
// EPUB 本质就是个 zip：第一个条目必须是「不压缩、无扩展字段」的 mimetype，然后是
// META-INF/container.xml 指向包文档。小说的配图已经是 PNG/JPG 这类压缩格式，正文
// 相对图片不到百分之一，所以这里全部用「直存」（ZIP_STORED）——不用引 deflate 依赖，
// 打包耗时也基本等于写文件本身（实测 80 MB 从 1.7s 降到 0.1s，体积一模一样）。
// ---------------------------------------------------------------------------

fn zip_crc32(bytes: &[u8]) -> u32 {
    let mut table = [0_u32; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 == 1 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *slot = value;
    }
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in bytes {
        crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

struct ZipItem {
    name: String,
    crc: u32,
    offset: u32,
    size: u32,
}

/// 追加一个直存条目（local file header + 数据）。
fn zip_push(out: &mut Vec<u8>, items: &mut Vec<ZipItem>, name: &str, data: &[u8]) {
    let offset = out.len() as u32;
    let crc = zip_crc32(data);
    let size = data.len() as u32;
    out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
    out.extend_from_slice(&20_u16.to_le_bytes()); // 解压所需版本 2.0
    out.extend_from_slice(&0x0800_u16.to_le_bytes()); // 文件名按 UTF-8 解释
    out.extend_from_slice(&0_u16.to_le_bytes()); // 压缩方式：直存
    out.extend_from_slice(&0_u16.to_le_bytes()); // 修改时间（用固定值，保证输出可复现）
    out.extend_from_slice(&0x0021_u16.to_le_bytes()); // 修改日期 1980-01-01
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // 扩展字段长度
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(data);
    items.push(ZipItem {
        name: name.to_string(),
        crc,
        offset,
        size,
    });
}

/// 写中央目录与结尾记录，收尾整个 zip。
fn zip_finish(out: &mut Vec<u8>, items: &[ZipItem]) {
    let start = out.len() as u32;
    for item in items {
        out.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        out.extend_from_slice(&20_u16.to_le_bytes()); // 创建版本
        out.extend_from_slice(&20_u16.to_le_bytes()); // 解压所需版本
        out.extend_from_slice(&0x0800_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(&0x0021_u16.to_le_bytes());
        out.extend_from_slice(&item.crc.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&item.size.to_le_bytes());
        out.extend_from_slice(&(item.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes()); // 扩展字段
        out.extend_from_slice(&0_u16.to_le_bytes()); // 注释
        out.extend_from_slice(&0_u16.to_le_bytes()); // 起始磁盘号
        out.extend_from_slice(&0_u16.to_le_bytes()); // 内部属性
        out.extend_from_slice(&0_u32.to_le_bytes()); // 外部属性
        out.extend_from_slice(&item.offset.to_le_bytes());
        out.extend_from_slice(item.name.as_bytes());
    }
    let end = out.len() as u32;
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // 当前磁盘号
    out.extend_from_slice(&0_u16.to_le_bytes()); // 中央目录所在磁盘号
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    out.extend_from_slice(&(end - start).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes()); // 注释长度
}

fn epub_image_media_type(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

/// 正文转 XHTML（EPUB 是 XML，空元素必须自闭合），图片指向包内的 `images/`。
fn render_novel_xhtml(content: &str, slots: &[NovelImageSlot]) -> String {
    let mut work = content.to_string();
    for slot in slots {
        let label = if slot.external { "引用插画" } else { "插图" };
        work = work.replace(&slot.token, &format!("[img:{}:{label}]", slot.order));
    }
    let mut body = String::new();
    let mut paragraph = String::new();
    let flush = |body: &mut String, paragraph: &mut String| {
        let trimmed = paragraph.trim();
        if !trimmed.is_empty() {
            body.push_str("<p>");
            body.push_str(&trimmed.replace('\n', "<br/>"));
            body.push_str("</p>\n");
        }
        paragraph.clear();
    };
    for segment in novel_segments(&work) {
        match segment {
            NovelSegment::Text(value) => {
                for line in value.split('\n') {
                    if line.trim().is_empty() {
                        flush(&mut body, &mut paragraph);
                    } else {
                        if !paragraph.is_empty() {
                            paragraph.push('\n');
                        }
                        paragraph.push_str(&escape_novel_html(line));
                    }
                }
            }
            NovelSegment::Ruby(base, reading) => {
                paragraph.push_str(&format!(
                    "<ruby>{}<rt>{}</rt></ruby>",
                    escape_novel_html(&base),
                    escape_novel_html(&reading)
                ));
            }
            NovelSegment::Chapter(title) => {
                flush(&mut body, &mut paragraph);
                body.push_str(&format!("<h2>{}</h2>\n", escape_novel_html(&title)));
            }
            NovelSegment::NewPage => {
                flush(&mut body, &mut paragraph);
                body.push_str("<hr class=\"page\"/>\n");
            }
            NovelSegment::Image(order, external) => {
                let Some(slot) = slots.iter().find(|slot| slot.order == order) else {
                    continue;
                };
                flush(&mut body, &mut paragraph);
                let caption = if external { "引用插画" } else { "插图" };
                body.push_str(&format!(
                    "<div class=\"figure\"><img src=\"../images/{}\" alt=\"{caption} {order}\"/><p class=\"caption\">{caption} {order}</p></div>\n",
                    escape_novel_html(&slot.file_name)
                ));
            }
        }
    }
    flush(&mut body, &mut paragraph);
    body
}

const EPUB_STYLE: &str = r#"body{font-family:"Noto Serif CJK SC","Source Han Serif SC",serif;line-height:1.9;margin:1em;color:#1f2328;}
h1{font-size:1.35em;line-height:1.45;margin:0 0 .4em;}
h2{font-size:1.12em;margin:1.6em 0 .6em;padding-left:.5em;border-left:3px solid #4a90d9;}
p{margin:0 0 .9em;text-align:justify;}
.meta{color:#667085;font-size:.85em;margin-bottom:.2em;}
.tag{display:inline-block;margin:0 .35em .35em 0;padding:.1em .6em;border-radius:999px;background:#eef2f7;color:#475467;font-size:.78em;}
.cover{display:block;max-width:100%;margin:0 auto;}
.figure{text-align:center;margin:1.4em 0;}
.figure img{max-width:100%;}
.caption{color:#98a2b3;font-size:.75em;margin-top:.3em;text-align:center;}
ruby rt{font-size:.55em;color:#98a2b3;}
hr.page{border:0;border-top:1px dashed #d0d5dd;margin:1.6em 0;}
"#;

/// 组包：封面页 + 正文页 + 目录 + 元数据。图片按正文顺序编成 001.jpg 这类包内路径。
fn build_novel_epub(
    title: &str,
    author_name: &str,
    release_date: &str,
    novel_id: &str,
    tags: &str,
    content: &str,
    slots: &[NovelImageSlot],
    images: &[(String, Vec<u8>)],
    cover: Option<(&str, &[u8])>,
) -> Vec<u8> {
    let title_xml = escape_novel_html(title);
    let author_xml = escape_novel_html(author_name);
    let identifier = if novel_id.trim().is_empty() {
        format!("pixiv-novel-{}", zip_crc32(title.as_bytes()))
    } else {
        format!("pixiv-novel-{novel_id}")
    };
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let date_xml = {
        let candidate = release_date.trim();
        if candidate.len() >= 10
            && candidate.as_bytes()[..10]
                .iter()
                .enumerate()
                .all(|(index, byte)| {
                    if index == 4 || index == 7 {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_digit()
                    }
                })
        {
            format!("<dc:date>{}</dc:date>", &candidate[..10])
        } else {
            String::new()
        }
    };
    let subjects = tags
        .split(['|', ','])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(|tag| format!("<dc:subject>{}</dc:subject>", escape_novel_html(tag)))
        .collect::<Vec<_>>()
        .join("");

    let used = slots
        .iter()
        .filter(|slot| images.iter().any(|(name, _)| name == &slot.file_name))
        .collect::<Vec<_>>();
    let body = render_novel_xhtml(content, slots);

    let mut out: Vec<u8> = Vec::new();
    let mut items: Vec<ZipItem> = Vec::new();
    zip_push(&mut out, &mut items, "mimetype", b"application/epub+zip");
    zip_push(
        &mut out,
        &mut items,
        "META-INF/container.xml",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
    );
    zip_push(&mut out, &mut items, "OEBPS/style.css", EPUB_STYLE.as_bytes());

    let mut manifest = vec![
        r#"<item id="style" href="style.css" media-type="text/css"/>"#.to_string(),
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#
            .to_string(),
        r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#.to_string(),
        r#"<item id="novel" href="text/novel.xhtml" media-type="application/xhtml+xml"/>"#
            .to_string(),
    ];
    let mut spine: Vec<String> = Vec::new();
    let mut nav_items: Vec<String> = Vec::new();
    let mut ncx_items: Vec<String> = Vec::new();

    if let Some((cover_name, cover_bytes)) = cover {
        zip_push(
            &mut out,
            &mut items,
            &format!("OEBPS/images/{cover_name}"),
            cover_bytes,
        );
        manifest.push(format!(
            r#"<item id="cover-image" href="images/{cover_name}" media-type="{}"/>"#,
            epub_image_media_type(cover_name)
        ));
        manifest.push(
            r#"<item id="coverpage" href="text/cover.xhtml" media-type="application/xhtml+xml"/>"#
                .to_string(),
        );
        let cover_page = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>封面</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../style.css\"/></head>\n<body><div><img class=\"cover\" src=\"../images/{}\" alt=\"封面\"/></div></body></html>\n",
            escape_novel_html(cover_name)
        );
        zip_push(
            &mut out,
            &mut items,
            "OEBPS/text/cover.xhtml",
            cover_page.as_bytes(),
        );
        spine.push(r#"<itemref idref="coverpage"/>"#.to_string());
    }

    for (file_name, bytes) in images {
        zip_push(
            &mut out,
            &mut items,
            &format!("OEBPS/images/{file_name}"),
            bytes,
        );
        manifest.push(format!(
            r#"<item id="image-{file_name}" href="images/{file_name}" media-type="{}"/>"#,
            epub_image_media_type(file_name)
        ));
    }

    let meta_line = {
        let mut parts = vec![author_xml.clone()];
        if !release_date.trim().is_empty() {
            parts.push(escape_novel_html(release_date.trim()));
        }
        if !used.is_empty() {
            parts.push(format!("配图 {} 张", used.len()));
        }
        parts.retain(|part| !part.is_empty());
        parts.join(" · ")
    };
    let tag_line = tags
        .split(['|', ','])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(|tag| format!("<span class=\"tag\">{}</span>", escape_novel_html(tag)))
        .collect::<Vec<_>>()
        .join("");
    let novel_page = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>{title_xml}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../style.css\"/></head>\n<body>\n<h1>{title_xml}</h1>\n<p class=\"meta\">{meta_line}</p>\n<p>{tag_line}</p>\n{body}</body></html>\n"
    );
    zip_push(
        &mut out,
        &mut items,
        "OEBPS/text/novel.xhtml",
        novel_page.as_bytes(),
    );
    spine.push(r#"<itemref idref="novel"/>"#.to_string());

    nav_items.push(format!(r#"<li><a href="text/novel.xhtml">{title_xml}</a></li>"#));
    ncx_items.push(format!(
        r#"<navPoint id="nav-1" playOrder="1"><navLabel><text>{title_xml}</text></navLabel><content src="text/novel.xhtml"/></navPoint>"#
    ));
    if cover.is_some() {
        nav_items.insert(0, r#"<li><a href="text/cover.xhtml">封面</a></li>"#.to_string());
        ncx_items.insert(
            0,
            r#"<navPoint id="nav-0" playOrder="0"><navLabel><text>封面</text></navLabel><content src="text/cover.xhtml"/></navPoint>"#
                .to_string(),
        );
    }
    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>目录</title></head>\n<body><nav epub:type=\"toc\" id=\"toc\"><h1>目录</h1><ol>{}</ol></nav></body></html>\n",
        nav_items.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/nav.xhtml", nav.as_bytes());
    let ncx = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n<head><meta name=\"dtb:uid\" content=\"{identifier}\"/><meta name=\"dtb:depth\" content=\"1\"/></head>\n<docTitle><text>{title_xml}</text></docTitle>\n<navMap>{}</navMap>\n</ncx>\n",
        ncx_items.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/toc.ncx", ncx.as_bytes());

    let cover_meta = if cover.is_some() {
        r#"<meta name="cover" content="cover-image"/>"#
    } else {
        ""
    };
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"zh-CN\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"bookid\">{identifier}</dc:identifier><dc:title>{title_xml}</dc:title><dc:language>zh-CN</dc:language><dc:creator>{author_xml}</dc:creator>{date_xml}{subjects}<meta property=\"dcterms:modified\">{modified}</meta>{cover_meta}</metadata>\n<manifest>{}</manifest>\n<spine toc=\"ncx\">{}</spine>\n</package>\n",
        manifest.join(""),
        spine.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/content.opf", opf.as_bytes());
    zip_finish(&mut out, &items);
    out
}

/// EPUB 与作品正文同目录、同名（跟「单份」模型一致：作品在哪一侧，epub 就在哪一侧）。
fn epub_target_path(text_path: &Path, output_dir: &str) -> PathBuf {
    let stem = text_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "Pixiv 小说".into());
    let file_name = format!("{stem}.epub");
    let dir = output_dir.trim();
    if dir.is_empty() {
        return text_path.with_file_name(file_name);
    }
    Path::new(dir).join(file_name)
}

/// 同步图文小说时自动保留的阅读版格式，由设置项 `sync_image_format` 决定。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReadingFormat {
    Html,
    Epub,
}

fn reading_format_of(value: &str) -> ReadingFormat {
    if value.trim().eq_ignore_ascii_case("epub") {
        ReadingFormat::Epub
    } else {
        ReadingFormat::Html
    }
}

impl ReadingFormat {
    /// 另一种格式：设置改了、或者阅读入口找不到首选格式时用来兜底。
    fn other(self) -> Self {
        match self {
            ReadingFormat::Html => ReadingFormat::Epub,
            ReadingFormat::Epub => ReadingFormat::Html,
        }
    }

    /// 落库 / 前端用的格式标识。
    fn as_str(self) -> &'static str {
        match self {
            ReadingFormat::Html => "html",
            ReadingFormat::Epub => "epub",
        }
    }

    /// 界面上显示的名字。
    fn label(self) -> &'static str {
        match self {
            ReadingFormat::Html => "HTML",
            ReadingFormat::Epub => "EPUB",
        }
    }
}

/// 阅读版文件的落点：一律与作品正文同目录、同名（HTML 换扩展名，EPUB 换扩展名）。
/// 第三个参数是早期的「EPUB 输出目录」遗留形参，设置项已移除，所有调用点都传空串。
fn reading_output_path(text_path: &Path, format: ReadingFormat, output_dir: &str) -> PathBuf {
    match format {
        ReadingFormat::Html => novel_html_path(text_path),
        ReadingFormat::Epub => epub_target_path(text_path, output_dir),
    }
}

/// 阅读版渲染需要的元信息（HTML 与 EPUB 共用一套）。
struct ReadingWriteMeta<'a> {
    novel_id: &'a str,
    title: &'a str,
    author_name: &'a str,
    release_date: &'a str,
    tags: &'a str,
    image_count: usize,
}

/// 按设置的格式把图文小说写成阅读版文件，返回落盘路径。
/// 配图从同名的 `{stem}_images` 目录读本地文件，同步流程已经下过就不会重复下载。
fn write_reading_output(
    text_path: &Path,
    format: ReadingFormat,
    output_dir: &str,
    content: &str,
    slots: &[NovelImageSlot],
    meta: &ReadingWriteMeta<'_>,
) -> Result<PathBuf, String> {
    let cover_path = text_path.with_extension("jpg");
    let cover_file = cover_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let target = reading_output_path(text_path, format, output_dir);
    match format {
        ReadingFormat::Html => {
            let html = render_novel_html(
                content,
                slots,
                &NovelHtmlMeta {
                    title: meta.title,
                    author_name: meta.author_name,
                    release_date: meta.release_date,
                    tags: meta.tags,
                    cover_file: &cover_file,
                    image_count: meta.image_count,
                },
            );
            write_text_atomic(&target, &html)?;
        }
        ReadingFormat::Epub => {
            let images_dir = novel_images_dir(text_path);
            let images: Vec<(String, Vec<u8>)> = slots
                .iter()
                .filter_map(|slot| {
                    let bytes = fs::read(images_dir.join(&slot.file_name)).ok()?;
                    if bytes.is_empty() {
                        return None;
                    }
                    Some((slot.file_name.clone(), bytes))
                })
                .collect();
            let cover_bytes = fs::read(&cover_path)
                .ok()
                .filter(|bytes| !bytes.is_empty());
            let cover = cover_bytes
                .as_deref()
                .filter(|_| !cover_file.is_empty())
                .map(|bytes| (cover_file.as_str(), bytes));
            let epub = build_novel_epub(
                meta.title,
                meta.author_name,
                meta.release_date,
                meta.novel_id,
                meta.tags,
                content,
                slots,
                &images,
                cover,
            );
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("无法创建 EPUB 目录：{e}"))?;
            }
            write_bytes_atomic(&target, &epub)?;
        }
    }
    Ok(target)
}

fn existing_sync_target(
    conn: &Connection,
    author_id: i64,
    novel_id: &str,
    title: &str,
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
    let title = name_key(title);
    if title.is_empty() {
        return Ok(None);
    }
    let works = works_for_author(conn, author_id)?;
    Ok(works
        .iter()
        .find(|(_, existing_title)| name_key(existing_title) == title)
        .map(|(id, _)| *id))
}

fn pixiv_sync_impl(
    author_id: i64,
    start_date: String,
    end_date: String,
    novel_url: String,
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
    let match_title_length: i64 = setting(&conn, "match_title_length")?
        .parse()
        .unwrap_or(0);
    let sync_settings = read_settings(&conn)?;
    let image_quality = sync_settings.image_quality.clone();
    // 同步时就按设置生成阅读版：html 单网页，或者直接产出 epub
    let reading_format = reading_format_of(&sync_settings.sync_image_format);
    let (homepage, preview_dir, purchased_dir, threshold, last_sync, cookie, author_name): (String, String, String, i64, String, String, String) = conn.query_row(
        "SELECT a.homepage, a.preview_dir, a.purchased_dir, a.match_threshold, a.pixiv_last_sync_at, COALESCE((SELECT value FROM app_settings WHERE key='pixiv_cookie'), ''), a.name FROM authors a WHERE a.id=?1",
        [author_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))
    ).map_err(|e| e.to_string())?;
    if preview_dir.trim().is_empty() {
        return Err("请先在作者设置中绑定预览版文件夹。".into());
    }
    if purchased_dir.trim().is_empty() {
        return Err("同步作品前，请先在作者设置中绑定完整版文件夹。".into());
    }
    let preview_dir = PathBuf::from(preview_dir);
    fs::create_dir_all(&preview_dir).map_err(|e| format!("无法创建预览版文件夹：{e}"))?;
    let purchased_dir = PathBuf::from(purchased_dir);
    fs::create_dir_all(&purchased_dir).map_err(|e| format!("无法创建完整版文件夹：{e}"))?;
    let preview_entries = sync_preview_entries(&preview_dir, minimum_file_size_bytes)?;
    let single_novel_id = pixiv_novel_id_from_url(&novel_url)?;
    let is_single_sync = single_novel_id.is_some();
    let start = parse_date_bound(&start_date, "开始日期")?;
    let end = parse_date_bound(&end_date, "结束日期")?;
    if !is_single_sync && has_invalid_date_range(start, end) {
        return Err("开始日期不能晚于结束日期。".into());
    }
    let use_incremental_filter = !is_single_sync && start.is_none() && end.is_none();
    let last_sync = DateTime::parse_from_rfc3339(&last_sync)
        .ok()
        .map(|date| date.with_timezone(&Utc));
    let cookie = if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    };
    let client = pixiv_client(cookie)?;
    let mut novels: Vec<String> = if let Some(novel_id) = single_novel_id {
        vec![novel_id]
    } else {
        let user_id = pixiv_user_id(&homepage)?;
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
        listing
            .pointer("/body/novels")
            .and_then(Value::as_object)
            .map(|items| items.keys().cloned().collect())
            .unwrap_or_default()
    };
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
        reused_preview_count: 0,
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
            author_id,
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
                    author_id,
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
                    author_id,
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
        let description = json_string(body, "description");
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
        if !is_single_sync && !is_within_date_range(&published_at, start, end) {
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
        if title.is_empty() {
            result.failed_count += 1;
            continue;
        }
        if let Some(existing_id) = existing_sync_target(&conn, author_id, &novel_id, &title)? {
            // 标题命中「插画 / 图文」时补标记为带图版；未命中则保持原值，不会清掉已有标记
            conn.execute(
                "UPDATE works SET pixiv_novel_id=CASE WHEN pixiv_novel_id='' THEN ?1 ELSE pixiv_novel_id END, series_id=?2, series_title=?3, series_order=?4, has_images=CASE WHEN ?6=1 THEN 1 ELSE has_images END WHERE id=?5",
                params![novel_id, series_id, series_title, series_order, existing_id, title_indicates_images(&title) as i64],
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
        let is_preview = synopsis_indicates_preview(&description);
        if let Some((preview_path, cover_path)) =
            matched_sync_preview(&preview_entries, &title, threshold, match_title_length)
        {
            // 单份模型：预览版作品留在预览版目录，完整版作品整份搬去完整版目录，不再复制第二份。
            let cover_name = cover_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().to_string());
            let (preview_value, purchased_value, cover_value) = if is_preview {
                (
                    preview_path.to_string_lossy().to_string(),
                    String::new(),
                    cover_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_default(),
                )
            } else {
                match move_sync_preview_to_purchased(
                    &preview_path,
                    cover_path.as_deref(),
                    &purchased_dir,
                ) {
                    Ok(path) => (
                        String::new(),
                        path,
                        cover_name
                            .map(|name| {
                                purchased_dir.join(name).to_string_lossy().to_string()
                            })
                            .unwrap_or_default(),
                    ),
                    Err(_) => {
                        result.failed_count += 1;
                        continue;
                    }
                }
            };
            if conn.execute("INSERT INTO works (author_id, title, release_date, preview_path, cover_path, purchased_path, tags, pixiv_novel_id, series_id, series_title, series_order, has_images, is_new) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)", params![author_id, title, date, preview_value, cover_value, purchased_value, tags, novel_id, series_id, series_title, series_order, title_indicates_images(&title) as i64]).is_err() {
                result.failed_count += 1;
                continue;
            }
            result.reused_preview_count += 1;
            continue;
        }
        if content.is_empty() || cover_url.is_empty() {
            result.failed_count += 1;
            continue;
        }
        if (content.len() as u64) < minimum_file_size_bytes {
            result.skipped_size_count += 1;
            continue;
        }
        downloads.push(PixivDownloadCandidate {
            novel_id,
            title,
            content,
            embedded_images: body
                .get("textEmbeddedImages")
                .cloned()
                .unwrap_or(Value::Null),
            cover_url,
            release_date: date,
            tags,
            series_id,
            series_title,
            series_order,
            is_preview,
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
            // 单份模型：一个作品只占一边——预览版作品进预览版目录，完整版作品进完整版目录。
            let target_dir = if work.is_preview {
                &preview_dir
            } else {
                &purchased_dir
            };
            let (text_path, cover_path) = sync_paths(target_dir, &work.title, &work.novel_id);
            let slots = plan_novel_images(
                &client,
                &work.embedded_images,
                &work.content,
                &text_path,
                &image_quality,
            );
            let images_dir = novel_images_dir(&text_path);
            let (image_saved, _) = download_novel_images(&client, &slots, &images_dir);
            let text_body = rewrite_novel_txt(&work.content, &slots);
            let text_temp = text_path.with_extension("txt.part");
            let cover_temp = cover_path.with_extension("jpg.part");
            let write_result = fs::write(&text_temp, text_body.as_bytes())
                .and_then(|_| fs::write(&cover_temp, &cover))
                .and_then(|_| fs::rename(&text_temp, &text_path))
                .and_then(|_| fs::rename(&cover_temp, &cover_path));
            // 阅读版按设置写成 HTML 或 EPUB；配图刚下好，两种格式共用同一批文件。
            let reading_result = write_result.map_err(|e| e.to_string()).and_then(|_| {
                write_reading_output(
                    &text_path,
                    reading_format,
                    "",
                    &work.content,
                    &slots,
                    &ReadingWriteMeta {
                        novel_id: &work.novel_id,
                        title: &work.title,
                        author_name: &author_name,
                        release_date: &work.release_date,
                        tags: &work.tags,
                        image_count: image_saved,
                    },
                )
            });
            if reading_result.is_err() {
                let _ = fs::remove_file(&text_temp);
                let _ = fs::remove_file(&cover_temp);
                result.failed_count += 1;
                continue;
            }
            let (preview_value, purchased_value) = if work.is_preview {
                (text_path.to_string_lossy().to_string(), String::new())
            } else {
                (String::new(), text_path.to_string_lossy().to_string())
            };
            if conn.execute("INSERT INTO works (author_id, title, release_date, preview_path, cover_path, purchased_path, tags, pixiv_novel_id, series_id, series_title, series_order, has_images, image_count, is_new) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1)", params![author_id, work.title, work.release_date, preview_value, cover_path.to_string_lossy(), purchased_value, work.tags, work.novel_id, work.series_id, work.series_title, work.series_order, title_indicates_images(&work.title) as i64, image_saved as i64]).is_err() {
            result.failed_count += 1;
            continue;
        }
            result.downloaded_count += 1;
        }
        let current = ((batch_index + 1) * COVER_CONCURRENCY).min(download_total);
        let _ = app.emit(
            "pixiv-sync-progress",
            PixivSyncProgress {
                author_id,
                total: download_total,
                current,
                title: format!("正在并发下载封面并保存：{} / {}", current, download_total),
            },
        );
    }
    if !is_single_sync && !result.cancelled && result.failed_count == 0 {
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
    novel_url: String,
    app: tauri::AppHandle,
) -> Result<PixivSyncResult, String> {
    clear_pixiv_sync_cancel(author_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        pixiv_sync_impl(author_id, start_date, end_date, novel_url, app)
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
    let homepage = normalize_author_homepage(&homepage);
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

#[tauri::command]
async fn check_pixiv_cookie() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let conn = db()?;
        let cookie = setting(&conn, "pixiv_cookie")?;
        
        // 如果没有设置cookie，返回false
        if cookie.trim().is_empty() {
            return Ok(false);
        }
        
        // 尝试使用cookie请求Pixiv API
        let client = pixiv_client(Some(normalize_pixiv_cookie(&cookie)?))?;
        
        // 请求一个简单的API来验证cookie有效性
        // 使用用户自己的信息API，这个需要有效的cookie
        let response = client
            .get("https://www.pixiv.net/ajax/user/0")
            .send()
            .map_err(|e| e.to_string())?;
        
        // 检查响应状态
        // 如果cookie无效，Pixiv通常会返回401或403
        // 如果cookie有效，会返回200或重定向
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Ok(false);
        }
        
        // 如果状态码是200或重定向，认为cookie有效
        Ok(status == reqwest::StatusCode::OK || status.is_redirection())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[allow(dead_code)]
fn scan_preview_existing(author_id: i64) -> Result<ScanPreviewResult, String> {
    let conn = db()?;
    let settings = read_settings(&conn)?;
    let threshold = settings.similarity_threshold;
    let _min_threshold = settings.min_similarity_threshold;
    let match_title_length = settings.match_title_length;
    let dir: String = conn
        .query_row(
            "SELECT preview_dir FROM authors WHERE id=?1",
            [author_id],
            |row| row.get(0),
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
        if is_generated_asset(&path) {
            continue;
        }
        let key = stem(&path);
        if key.is_empty() {
            continue;
        }
        let scores: Vec<(&(i64, String, String), i64)> = works
            .iter()
            .map(|work| {
                (
                    work,
                    similarity_with_title_limit(&work.1, &key, match_title_length),
                )
            })
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
    let match_title_length: i64 = setting(&conn, "match_title_length")?
        .parse()
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
        if is_generated_asset(&path) {
            continue;
        }
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
            .map(|(id, title, _)| (*id, similarity_with_title_limit(title, &key, match_title_length)))
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
                let id = insert_local_work(&conn, author_id, &title, &release_date)
                    .map_err(|e| e.to_string())?;
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

/// 这件作品的完整版位置还空着吗（没绑过，或者当初绑的文件已经不在了）。
/// 只有这种作品才需要「关联完整版文件」去认领新文件；已经绑好且文件还在的，
/// 说明同步时就已经关联好了，不该再被翻出来重绑一遍。
fn needs_a_purchased_file(purchased_path: &str) -> bool {
    purchased_path.trim().is_empty() || !Path::new(purchased_path.trim()).exists()
}

/// 扫一遍完整版目录，挑出真正待关联的文件，同时报出跳过了多少个「早就绑好」的。
/// 排除两类：封面图 / 配图文件夹（不是作品文件），以及已经绑在某个作品上的文件。
fn collect_pending_purchased_files(
    dir: &Path,
    bound_files: &HashSet<String>,
) -> Result<(Vec<PathBuf>, usize), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("无法读取完整版文件夹：{e}"))?;
    let mut paths: Vec<PathBuf> = vec![];
    let mut skipped = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if is_image_like_asset(&path) {
            continue;
        }
        if bound_files.contains(&path_key(&path.to_string_lossy())) {
            skipped += 1;
            continue;
        }
        paths.push(path);
    }
    Ok((paths, skipped))
}

#[tauri::command]
fn scan_purchased(author_id: i64, mode: String) -> Result<ScanPurchasedResult, String> {
    let mode = resolve_match_mode(&mode)?;
    let conn = db()?;
    let settings = read_settings(&conn)?;
    let threshold = settings.similarity_threshold;
    let min_threshold = settings.min_similarity_threshold;
    let match_title_length = settings.match_title_length;
    let dir: String = conn
        .query_row(
            "SELECT purchased_dir FROM authors WHERE id=?1",
            [author_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    // 库里所有已经指到某个文件上的完整版路径：这些文件早就关联好了，
    // 每次点「关联完整版文件」都不该再把它们翻出来让人重绑一遍。
    let mut bound_statement = conn
        .prepare("SELECT purchased_path FROM works WHERE purchased_path <> ''")
        .map_err(|e| e.to_string())?;
    let bound_files: HashSet<String> = bound_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|value| path_key(&value))
        .collect();
    drop(bound_statement);
    // 待关联的只有「文本 / 电子书 / HTML（以及用户自己整理的文件夹）」里还没绑过的那些
    let (paths, skipped_count) = collect_pending_purchased_files(Path::new(&dir), &bound_files)?;
    // 参与匹配的作品：已经有完整版且文件还躺在磁盘上的，说明这件作品不用再关联了
    // （真要换文件，走作品卡右键的「绑定完整版文件」）。绑定丢失的仍留在候选里，方便补回来。
    let works: Vec<(i64, String)> = {
        let mut statement = conn
            .prepare("SELECT id, title, purchased_path FROM works WHERE author_id=?1")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([author_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(_, _, purchased)| needs_a_purchased_file(purchased))
            .map(|(id, title, _)| (id, title))
            .collect()
    };
    // 作者库：用来认文件名里写的作者名（名称与别名都认）
    let authors: Vec<(i64, String, String)> = {
        let mut statement = conn
            .prepare("SELECT id, name, aliases FROM authors")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    // 按角色匹配时才需要角色索引；作品标题的角色集先算好，免得每个文件都重算一遍
    let character_index = if mode == MatchMode::Character {
        character_match_index(&conn)
    } else {
        vec![]
    };
    let work_characters: HashMap<i64, Vec<String>> = works
        .iter()
        .map(|(id, title)| (*id, characters_in_text(title, &character_index)))
        .collect();
    let mut unknown_author_groups: Vec<UnknownAuthorGroup> = vec![];
    // 文件名写着别的作者（那位作者在库里）的文件数：不参与匹配，但要让用户知道
    let mut other_author_count = 0usize;
    let mut scored_paths: Vec<(PathBuf, Vec<(i64, String, i64, Vec<String>)>, String, Vec<String>)> = vec![];
    for path in paths {
        let stem = file_stem(&path);
        let detected = detect_author_in_name(&stem, &authors);
        // 文件名里写了作者、但库里没这个人 —— 不参与匹配，单独分组展示
        if let Some(author) = &detected {
            if author.author_id.is_none() {
                match unknown_author_groups
                    .iter_mut()
                    .find(|group| group.author == author.name)
                {
                    Some(group) => group.files.push(file_name(&path)),
                    None => unknown_author_groups.push(UnknownAuthorGroup {
                        author: author.name.clone(),
                        files: vec![file_name(&path)],
                    }),
                }
                continue;
            }
        }
        // 认出了作者就只在这位作者的作品里找。注意 works 已经按当前作者筛过：
        // 书名里写的正是这位作者 → 照常匹配；写的是库里另一位作者 → 这个文件不属于
        // 这里（要归到别人名下请用「完整版自动分组」），跳过并计数，不要拿作者 ID
        // 去和作品 ID 比大小，两者不是一套编号。
        if !scope_allows_path(detected.as_ref().and_then(|author| author.author_id), author_id) {
            other_author_count += 1;
            continue;
        }
        let scope: Vec<(i64, String)> = works.clone();
        let file_characters = if mode == MatchMode::Character {
            characters_in_text(&stem, &character_index)
        } else {
            vec![]
        };
        let mut candidates: Vec<(i64, String, i64, Vec<String>)> = match mode {
            // 按标题：原来的相似度逻辑，一个字没改
            MatchMode::Title => scope
                .iter()
                .map(|(id, title)| {
                    (
                        *id,
                        title.clone(),
                        similarity_with_title_limit(title, &file_name(&path), match_title_length),
                        vec![],
                    )
                })
                .filter(|(_, _, similarity, _)| *similarity >= min_threshold)  // 过滤低于最小阈值的
                .collect(),
            // 按角色：标题与文件名有共同角色即可，不算相似度、不看阈值
            MatchMode::Character => scope
                .iter()
                .filter_map(|(id, title)| {
                    let shared = shared_characters(
                        &file_characters,
                        work_characters.get(id).map(|list| list.as_slice()).unwrap_or(&[]),
                    );
                    if shared.is_empty() {
                        None
                    } else {
                        let count = shared.len() as i64;
                        Some((*id, title.clone(), count, shared))
                    }
                })
                .collect(),
        };
        candidates.sort_by(|left, right| right.2.cmp(&left.2));
        if candidates.is_empty() {
            continue;
        }
        scored_paths.push((
            path,
            candidates,
            detected.map(|author| author.name).unwrap_or_default(),
            file_characters,
        ));
    }

    let mut strong_usage: HashMap<i64, usize> = HashMap::new();
    for (_, candidates, _, _) in &scored_paths {
        for (work_id, _, similarity, _) in candidates
            .iter()
            .filter(|(_, _, similarity, _)| *similarity >= threshold)
        {
            let _ = similarity;
            *strong_usage.entry(*work_id).or_default() += 1;
        }
    }

    let mut bound_count = 0;
    let mut selections = vec![];
    for (path, candidates, author_name, file_characters) in scored_paths {
        let strong: Vec<(i64, String, i64, Vec<String>)> = candidates
            .iter()
            .filter(|(_, _, similarity, _)| *similarity >= threshold)
            .cloned()
            .collect();
        // 按角色匹配时一律不自动绑定 —— 命中的每一条都由用户手动确认
        if mode == MatchMode::Title
            && strong.len() == 1
            && strong_usage.get(&strong[0].0) == Some(&1)
        {
            conn.execute(
                "UPDATE works SET purchased_path=?1 WHERE id=?2",
                params![path.to_string_lossy(), strong[0].0],
            )
            .map_err(|e| e.to_string())?;
            refresh_reading_image_count(&conn, strong[0].0);
            bound_count += 1;
            continue;
        }
        let options: Vec<(i64, String, i64, Vec<String>)> = if mode == MatchMode::Character {
            candidates
        } else if strong.is_empty() {
            candidates.into_iter().take(3).collect()
        } else {
            strong
        };
        if !options.is_empty() {
            selections.push(PurchasedSelection {
                path: path.to_string_lossy().to_string(),
                author_name,
                matched_characters: file_characters,
                candidates: options
                    .into_iter()
                    .map(|(work_id, title, similarity, matched)| WorkCandidate {
                        work_id,
                        title,
                        similarity,
                        matched_characters: matched,
                    })
                    .collect(),
            });
        }
    }
    Ok(ScanPurchasedResult {
        bound_count,
        skipped_count,
        selections,
        unknown_author_groups,
        other_author_count,
    })
}

#[tauri::command]
fn bind_work(work_id: i64, path: String) -> Result<(), String> {
    let conn = db()?;
    // 能绑的只有文本 / 电子书 / HTML（以及用户自己整理的文件夹），封面图、配图文件夹不算
    if is_image_like_asset(Path::new(&path)) {
        return Err("封面图 / 配图文件夹不能当作作品文件绑定，请选择文本、电子书或 HTML 文件".into());
    }
    conn.execute(
        "UPDATE works SET purchased_path=?1 WHERE id=?2",
        params![path, work_id],
    )
    .map_err(|e| e.to_string())?;
    refresh_reading_image_count(&conn, work_id);
    Ok(())
}

#[tauri::command]
fn bind_work_with_rename(work_id: i64, path: String) -> Result<String, String> {
    let conn = db()?;
    // 封面图 / 配图文件夹不当作品文件，与「关联完整版文件」保持一致
    if is_image_like_asset(Path::new(&path)) {
        return Err("封面图 / 配图文件夹不能当作作品文件绑定，请选择文本、电子书或 HTML 文件".into());
    }
    // 1. 获取作品标题
    let title: String = conn
        .query_row(
            "SELECT title FROM works WHERE id=?1",
            [work_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("找不到作品：{e}"))?;

    // 2. 获取原文件扩展名
    let old_path = Path::new(&path);
    let extension = old_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    // 3. 生成新文件名（使用作品标题，保留原扩展名）
    let new_file_name = if extension.is_empty() {
        title.clone()
    } else {
        format!("{title}.{extension}")
    };

    // 4. 获取目标目录
    let parent_dir = old_path
        .parent()
        .ok_or_else(|| "无法获取文件目录".to_string())?;
    let new_path = parent_dir.join(&new_file_name);

    // 5. 处理文件名冲突（如果目标文件名已存在）
    let final_path = if new_path.exists() {
        // 如果目标文件已存在，检查是否是同一文件（相同路径）
        if new_path == old_path {
            // 文件已经是目标名称，无需重命名
            conn.execute(
                "UPDATE works SET purchased_path=?1 WHERE id=?2",
                params![path, work_id],
            )
            .map_err(|e| e.to_string())?;
            refresh_reading_image_count(&conn, work_id);
            return Ok(path);
        }
        // 生成带数字后缀的文件名
        let stem = Path::new(&new_file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&new_file_name);
        let mut counter = 1;
        loop {
            let candidate_name = if extension.is_empty() {
                format!("{stem} ({counter})")
            } else {
                format!("{stem} ({counter}).{extension}")
            };
            let candidate_path = parent_dir.join(&candidate_name);
            if !candidate_path.exists() {
                break candidate_path;
            }
            counter += 1;
        }
    } else {
        new_path
    };

    // 6. 重命名文件
    fs::rename(old_path, &final_path).map_err(|e| format!("重命名文件失败：{e}"))?;

    // 7. 更新数据库中的purchased_path
    let final_path_str = final_path.to_string_lossy().to_string();
    conn.execute(
        "UPDATE works SET purchased_path=?1 WHERE id=?2",
        params![final_path_str, work_id],
    )
    .map_err(|e| e.to_string())?;
    refresh_reading_image_count(&conn, work_id);

    Ok(final_path_str)
}

/// 去掉文件名中的非法字符，避免生成路径时失败
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            '\n' | '\r' | '\t' => ' ',
            _ => ch,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim().to_string();
    if trimmed.is_empty() {
        "未命名".to_string()
    } else {
        trimmed
    }
}

/// 作品在目标目录里对应的路径（不做重名处理，用于判断是否与已有文件冲突）
fn direct_target_path(target_dir: &Path, work_title: &str, extension: &str) -> PathBuf {
    let stem = sanitize_file_name(work_title);
    if extension.is_empty() {
        target_dir.join(stem)
    } else {
        target_dir.join(format!("{stem}.{extension}"))
    }
}

/// 在目标目录下生成一个不冲突的文件路径（重名时追加 (1)、(2)……）
fn unique_target_path(target_dir: &Path, work_title: &str, extension: &str) -> PathBuf {
    let candidate = direct_target_path(target_dir, work_title, extension);
    if !candidate.exists() {
        return candidate;
    }
    let stem = sanitize_file_name(work_title);
    let mut counter = 1;
    loop {
        let name = if extension.is_empty() {
            format!("{stem} ({counter})")
        } else {
            format!("{stem} ({counter}).{extension}")
        };
        let path = target_dir.join(&name);
        if !path.exists() {
            return path;
        }
        counter += 1;
    }
}

/// 把源文件分发到若干个目标：
/// 单个目标 = 移动文件；多个目标 = 复制多份（同一本书被多个作品选用时）。
/// 复制时全部成功才会删除源文件，任一失败则回滚已复制的副本。
/// `conflict` 决定目标已存在同名文件时是跳过、覆盖还是另存一份。
fn distribute_file(
    conn: &Connection,
    file_path: &Path,
    targets: &[DistributeTarget],
    conflict: ConflictAction,
) -> Result<Distributed, String> {
    // 防御：图像文件（封面、插图、配图文件夹）不是作品文件。匹配阶段已经滤过一遍，
    // 万一被别处直接调用，这里也当作无事发生，绝不把封面搬进完整版目录。
    if is_image_like_asset(file_path) {
        return Ok(Distributed {
            bound_count: 0,
            skipped_count: 0,
        });
    }
    let mut skipped_count = 0;
    // (work_id, 最终路径, 是否需要先删除已有文件)
    let mut resolved: Vec<(i64, PathBuf, bool)> = vec![];

    for target in targets {
        let direct = direct_target_path(&target.dir, &target.title, &target.extension);
        if !direct.exists() {
            resolved.push((target.work_id, direct, false));
            continue;
        }
        match conflict {
            ConflictAction::Skip => skipped_count += 1,
            ConflictAction::Overwrite => resolved.push((target.work_id, direct, true)),
            ConflictAction::KeepBoth => resolved.push((
                target.work_id,
                unique_target_path(&target.dir, &target.title, &target.extension),
                false,
            )),
        }
    }

    if resolved.is_empty() {
        return Ok(Distributed {
            bound_count: 0,
            skipped_count,
        });
    }

    if resolved.len() == 1 {
        let (work_id, path, overwrite) = &resolved[0];
        if *overwrite {
            fs::remove_file(path).map_err(|e| format!("无法覆盖已有文件：{e}"))?;
        }
        // 移动失败（例如跨盘）时退回复制 + 删除源文件
        if fs::rename(file_path, path).is_err() {
            fs::copy(file_path, path).map_err(|e| e.to_string())?;
            let _ = fs::remove_file(file_path);
        }
        conn.execute(
            "UPDATE works SET purchased_path=?1 WHERE id=?2",
            params![path.to_string_lossy().to_string(), work_id],
        )
        .map_err(|e| e.to_string())?;
        refresh_reading_image_count(conn, *work_id);
        return Ok(Distributed {
            bound_count: 1,
            skipped_count,
        });
    }

    let mut copied: Vec<&PathBuf> = vec![];
    for (_, path, overwrite) in &resolved {
        if *overwrite {
            let _ = fs::remove_file(path);
        }
        if let Err(error) = fs::copy(file_path, path) {
            for done in &copied {
                let _ = fs::remove_file(done);
            }
            return Err(error.to_string());
        }
        copied.push(path);
    }
    let _ = fs::remove_file(file_path);

    for (work_id, path, _) in &resolved {
        conn.execute(
            "UPDATE works SET purchased_path=?1 WHERE id=?2",
            params![path.to_string_lossy().to_string(), work_id],
        )
        .map_err(|e| e.to_string())?;
        refresh_reading_image_count(conn, *work_id);
    }
    Ok(Distributed {
        bound_count: resolved.len(),
        skipped_count,
    })
}

/// 递归收集目录下的所有文件（含子文件夹）。
/// 顶层目录读不动时报错；子目录读不动时跳过，不影响其余文件。
fn collect_files_recursively(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("无法读取文件夹 {}：{e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `{标题}_images` 是同步下来的配图文件夹，里面的图不是待分组的作品文件
            if is_generated_asset(&path) {
                continue;
            }
            let _ = collect_files_recursively(&path, out);
        } else if path.is_file() && !is_generated_asset(&path) {
            out.push(path);
        }
    }
    Ok(())
}

#[tauri::command]
fn auto_group_purchased_files(author_id: Option<i64>, mode: String) -> Result<AutoGroupResult, String> {
    let mode = resolve_match_mode(&mode)?;
    let conn = db()?;
    let settings = read_settings(&conn)?;

    // 1. 检查自动分组文件夹是否已设置
    if settings.auto_group_dir.is_empty() {
        return Err("请先在设置中选择完整版自动分组文件夹".into());
    }

    let auto_group_dir = Path::new(&settings.auto_group_dir);
    if !auto_group_dir.exists() {
        return Err("自动分组文件夹不存在".into());
    }

    // 2. 递归读取自动分组文件夹下的所有文件（含子文件夹里的文件）
    let mut files: Vec<PathBuf> = vec![];
    collect_files_recursively(auto_group_dir, &mut files)?;
    // 封面图、配图文件夹不是作品文件，别拿去匹配（否则会被当成作品搬进完整版目录）
    files.retain(|path| !is_image_like_asset(path));
    // 排序让处理顺序可预期（read_dir 本身不保证顺序）
    files.sort();

    if files.is_empty() {
        return Ok(AutoGroupResult {
            auto_moved_count: 0,
            manual_selections: vec![],
            unknown_author_groups: vec![],
        });
    }

    // 3. 获取参与匹配的作者及其完整版目录；指定作者时只取这一位
    let belongs_to_scope = |id: i64| author_id.map_or(true, |only| only == id);
    let authors: Vec<(i64, String, String, String)> = conn
        .prepare("SELECT id, name, purchased_dir, aliases FROM authors WHERE purchased_dir != ''")
        .map_err(|e| e.to_string())?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(id, _, _, _)| belongs_to_scope(*id))
        .collect();

    // 指定作者时先确认这位作者确实绑定了完整版目录，避免用户点了却"什么都没发生"
    if let Some(only) = author_id {
        let name: Option<String> = conn
            .query_row("SELECT name FROM authors WHERE id=?1", params![only], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let name = name.ok_or_else(|| "该作者不存在".to_string())?;
        if !authors.iter().any(|(id, _, _, _)| *id == only) {
            return Err(format!(
                "作者「{name}」还没有绑定完整版文件夹，请先在作者设置里填写"
            ));
        }
    }

    // 4. 获取参与匹配的作品（id, title, author_id）
    let all_works: Vec<(i64, String, i64)> = conn
        .prepare("SELECT id, title, author_id FROM works")
        .map_err(|e| e.to_string())?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, _, work_author_id)| belongs_to_scope(*work_author_id))
        .collect();

    // 5. 为每个文件计算与所有作品的相似度
    let threshold = settings.similarity_threshold;
    let min_threshold = settings.min_similarity_threshold;
    let match_title_length = settings.match_title_length;
    let mut auto_moved_count = 0;
    let mut manual_selections = vec![];

    // 作者识别用（含别名）：单独摊一份，免得动上面那个带完整版目录的查询
    let author_lookup: Vec<(i64, String, String)> = authors
        .iter()
        .map(|(id, name, _, aliases)| (*id, name.clone(), aliases.clone()))
        .collect();
    // 按角色匹配时才需要角色索引；作品标题的角色集先算好
    let character_index = if mode == MatchMode::Character {
        character_match_index(&conn)
    } else {
        vec![]
    };
    let work_characters: HashMap<i64, Vec<String>> = all_works
        .iter()
        .map(|(id, title, _)| (*id, characters_in_text(title, &character_index)))
        .collect();
    let mut unknown_author_groups: Vec<UnknownAuthorGroup> = vec![];

    for file_path in files {
        let file_name_str = file_name(&file_path);
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        let stem = file_stem(&file_path);
        let detected = detect_author_in_name(&stem, &author_lookup);
        // 文件名里写了作者、但库里没这个人 —— 不参与匹配，单独分组展示
        if let Some(author) = &detected {
            if author.author_id.is_none() {
                match unknown_author_groups
                    .iter_mut()
                    .find(|group| group.author == author.name)
                {
                    Some(group) => group.files.push(file_name_str.clone()),
                    None => unknown_author_groups.push(UnknownAuthorGroup {
                        author: author.name.clone(),
                        files: vec![file_name_str.clone()],
                    }),
                }
                continue;
            }
        }
        let detected_author_id = detected.as_ref().and_then(|author| author.author_id);
        let file_characters = if mode == MatchMode::Character {
            characters_in_text(&stem, &character_index)
        } else {
            vec![]
        };
        let mut candidates = vec![];

        for (work_id, work_title, work_author_id) in &all_works {
            // 认出了作者就只在这位作者的作品里找；两种匹配方式都受这条约束
            if let Some(only) = detected_author_id {
                if only != *work_author_id {
                    continue;
                }
            }
            let (similarity, hit_characters) = match mode {
                // 按标题：原来的相似度逻辑，一个字没改
                MatchMode::Title => {
                    let similarity =
                        similarity_with_title_limit(work_title, &file_name_str, match_title_length);
                    if similarity < min_threshold {
                        continue;
                    }
                    (similarity, vec![])
                }
                // 按角色：标题与文件名有共同角色即可，不算相似度、不看阈值
                MatchMode::Character => {
                    let shared = shared_characters(
                        &file_characters,
                        work_characters
                            .get(work_id)
                            .map(|list| list.as_slice())
                            .unwrap_or(&[]),
                    );
                    if shared.is_empty() {
                        continue;
                    }
                    let count = shared.len() as i64;
                    (count, shared)
                }
            };
            if let Some((_, author_name, purchased_dir, _)) =
                authors.iter().find(|(id, _, _, _)| id == work_author_id)
            {
                if purchased_dir.is_empty() {
                    continue;
                }
                // 目标目录已有同名文件 → 标为冲突，由用户决定覆盖/跳过/另存
                let conflict = direct_target_path(Path::new(purchased_dir), work_title, extension)
                    .exists();
                candidates.push(GroupCandidate {
                    author_id: *work_author_id,
                    author_name: author_name.clone(),
                    work_id: *work_id,
                    work_title: work_title.clone(),
                    similarity,
                    // 按角色匹配时不自动移动，所以一律不算「推荐自动处理」
                    recommended: mode == MatchMode::Title && similarity >= threshold,
                    conflict,
                    matched_characters: hit_characters,
                });
            }
        }

        candidates.sort_by(|a, b| b.similarity.cmp(&a.similarity));

        // 6. 只有「恰好一个达标作品且目标不冲突」时才自动移动。
        //    按角色匹配时一律不自动移动 —— 命中的每一条都由用户手动确认。
        let strong_count = if mode == MatchMode::Title {
            candidates.iter().filter(|item| item.recommended).count()
        } else {
            0
        };
        if strong_count == 1 {
            let candidate = candidates
                .iter()
                .find(|item| item.recommended)
                .cloned();
            if let Some(candidate) = candidate {
                if !candidate.conflict {
                    if let Some((_, _, purchased_dir, _)) = authors
                        .iter()
                        .find(|(id, _, _, _)| *id == candidate.author_id)
                    {
                        let target_dir = Path::new(purchased_dir);
                        if target_dir.exists() {
                            let target = DistributeTarget {
                                work_id: candidate.work_id,
                                dir: target_dir.to_path_buf(),
                                title: candidate.work_title.clone(),
                                extension: extension.to_string(),
                            };
                            if let Ok(outcome) = distribute_file(
                                &conn,
                                &file_path,
                                &[target],
                                ConflictAction::Skip,
                            ) {
                                if outcome.bound_count > 0 {
                                    auto_moved_count += 1;
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 其余情况（多候选、目标已存在同名文件、目录缺失、移动失败）交给用户手动选择
        if !candidates.is_empty() {
            let sub_dir = file_path
                .parent()
                .and_then(|parent| parent.strip_prefix(auto_group_dir).ok())
                .map(|relative| relative.to_string_lossy().to_string())
                .unwrap_or_default();
            manual_selections.push(ManualGroupSelection {
                file_path: file_path.to_string_lossy().to_string(),
                file_name: file_name_str,
                sub_dir,
                candidates,
                author_name: detected.map(|author| author.name).unwrap_or_default(),
                matched_characters: file_characters,
            });
        }
    }

    Ok(AutoGroupResult {
        auto_moved_count,
        manual_selections,
        unknown_author_groups,
    })
}

#[tauri::command]
fn confirm_manual_group(choices: Vec<ManualGroupChoice>) -> Result<ManualGroupResult, String> {
    let conn = db()?;
    let mut bound_count = 0;
    let mut skipped_count = 0;
    let mut duplicated_count = 0;
    let mut failed: Vec<String> = vec![];

    for choice in choices {
        if choice.work_ids.is_empty() {
            continue;
        }

        let file_path = PathBuf::from(&choice.file_path);
        if !file_path.exists() {
            failed.push(format!("{}（源文件已不存在）", choice.file_path));
            continue;
        }
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");

        // 为勾选的每个作品准备目标（目录 + 作品标题 + 原扩展名）
        let mut targets: Vec<DistributeTarget> = vec![];
        for work_id in &choice.work_ids {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT w.title, a.purchased_dir FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id=?1",
                    [work_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let Some((title, purchased_dir)) = row else {
                continue;
            };
            if purchased_dir.is_empty() {
                continue;
            }
            let dir = PathBuf::from(&purchased_dir);
            if !dir.exists() {
                continue;
            }
            targets.push(DistributeTarget {
                work_id: *work_id,
                dir,
                title,
                extension: extension.to_string(),
            });
        }

        if targets.is_empty() {
            continue;
        }

        // 勾选了多个作品 → 复制多份；只勾选一个 → 移动
        let duplicated = targets.len() > 1;
        match distribute_file(&conn, &file_path, &targets, choice.conflict_action) {
            Ok(outcome) => {
                bound_count += outcome.bound_count;
                skipped_count += outcome.skipped_count;
                if duplicated && outcome.bound_count > 1 {
                    duplicated_count += 1;
                }
            }
            Err(error) => failed.push(format!("{}（{error}）", choice.file_path)),
        }
    }

    Ok(ManualGroupResult {
        bound_count,
        skipped_count,
        duplicated_count,
        failed,
    })
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
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT preview_path, cover_path FROM works WHERE id=?1 AND author_id=?2",
                params![work_id, author_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((preview_path, old_cover)) = row else {
            result.skipped_count += 1;
            continue;
        };
        let source = PathBuf::from(&preview_path);
        let Some(name) = source.file_name().map(|name| name.to_os_string()) else {
            result.skipped_count += 1;
            continue;
        };
        if !source.exists() {
            result.skipped_count += 1;
            continue;
        }
        let destination = purchased_dir.join(&name);
        // 单份模型：整份搬过去（配图文件夹、图文 HTML、封面一起），预览版目录不留副本。
        if source != destination {
            move_path(&source, &destination)?;
            for asset in sibling_assets(&source) {
                if let Some(asset_name) = asset.file_name() {
                    move_path(&asset, &purchased_dir.join(asset_name))?;
                }
            }
            move_cover_along(&old_cover, &purchased_dir)?;
            result.copied_count += 1;
        }
        // 正文搬家了，库里那条封面路径必须跟着走：否则老目录一被清理，封面就集体变死路径
        // （2026-09-14 封面挂掉事故的根因）。
        let next_cover = follow_cover_path(&old_cover, &destination);
        conn.execute(
            "UPDATE works SET purchased_path=?1, preview_path='', cover_path=?3 WHERE id=?2",
            params![destination.to_string_lossy(), work_id, next_cover],
        )
        .map_err(|e| e.to_string())?;
        refresh_reading_image_count(&conn, work_id);
        result.bound_count += 1;
    }
    Ok(result)
}

/// 手动纠正「完整版 / 预览版」判定：把作品改回预览版。
/// 库里的绑定关系和磁盘上的文件一起改：完整版目录里的那份（含配图、图文 HTML、封面）
/// 搬回预览版目录，保证一个作品只占一边。
#[tauri::command]
fn mark_work_as_preview(work_id: i64) -> Result<(), String> {
    let conn = db()?;
    let (purchased_path, preview_path, preview_dir, old_cover): (String, String, String, String) = conn
        .query_row(
            "SELECT w.purchased_path, w.preview_path, a.preview_dir, w.cover_path FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id=?1",
            [work_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|e| e.to_string())?;
    if purchased_path.trim().is_empty() {
        return Ok(());
    }
    let source = PathBuf::from(&purchased_path);
    let can_move = !preview_dir.trim().is_empty() && source.exists();
    if can_move {
        let preview_dir = PathBuf::from(&preview_dir);
        fs::create_dir_all(&preview_dir).map_err(|e| format!("无法创建预览版文件夹：{e}"))?;
        let Some(name) = source.file_name() else {
            return Err("完整版文件名称无效".into());
        };
        let destination = preview_dir.join(name);
        move_path(&source, &destination)?;
        for asset in sibling_assets(&source) {
            if let Some(asset_name) = asset.file_name() {
                move_path(&asset, &preview_dir.join(asset_name))?;
            }
        }
        move_cover_along(&old_cover, &preview_dir)?;
        let next_cover = follow_cover_path(&old_cover, &destination);
        conn.execute(
            "UPDATE works SET purchased_path='', preview_path=?1, cover_path=?3 WHERE id=?2",
            params![destination.to_string_lossy(), work_id, next_cover],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    // 预览版目录没绑定或文件已不在：只改绑定关系，文件原地不动，作品至少还能打开。
    let next_preview = if preview_path.trim().is_empty() {
        purchased_path.clone()
    } else {
        preview_path
    };
    conn.execute(
        "UPDATE works SET purchased_path='', preview_path=?1 WHERE id=?2",
        params![next_preview, work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 手动纠正「完整版 / 预览版」判定：把作品设为完整版。
/// 复用「把预览版文件搬到完整版目录并绑定」的逻辑，同步时按简介误判成预览版的作品
/// 可以用它一键纠正。
#[tauri::command]
fn mark_work_as_full(work_id: i64) -> Result<(), String> {
    let author_id: i64 = db()?
        .query_row("SELECT author_id FROM works WHERE id=?1", [work_id], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    let result = copy_previews_to_purchased(author_id, vec![work_id])?;
    if result.bound_count == 0 {
        return Err(
            "该作品没有可用的预览版文件，无法搬成完整版；请改用「绑定完整版文件」手动指定。"
                .into(),
        );
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkReadingResult {
    /// 生成出来的阅读版格式：`html` 或 `epub`
    format: String,
    /// 阅读版文件路径，作品会绑定到它
    output_path: String,
    total_count: usize,
    saved_count: usize,
    failed_count: usize,
    /// EPUB 打包时缺图的张数（HTML 版恒为 0）
    missing_count: usize,
    /// EPUB 包体大小（HTML 版恒为 0）
    size_bytes: u64,
}

/// 一篇作品的图文素材（正文、配图计划、封面位置），配图下载与 EPUB 导出共用。
struct NovelAssets {
    novel_id: String,
    title: String,
    author_name: String,
    release_date: String,
    tags: String,
    text_path: PathBuf,
    cover_path: PathBuf,
    cover_url: String,
    content: String,
    slots: Vec<NovelImageSlot>,
}

/// 抓一篇作品的详情并定好配图计划；返回素材和复用的 HTTP 客户端。
/// 调用方负责真正下图片（便于「只导出」和「下配图」走同一套解析）。
fn prepare_novel_assets(work_id: i64) -> Result<(NovelAssets, Client), String> {
    let conn = db()?;
    let (novel_id, title, preview_path, purchased_path, release_date, tags, author_name): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT w.pixiv_novel_id, w.title, w.preview_path, w.purchased_path, w.release_date, w.tags, a.name FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id=?1",
            [work_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    if novel_id.trim().is_empty() {
        return Err("这篇作品没有 Pixiv 作品 ID，抓不到正文与配图；请先同步一次。".into());
    }
    let text_path = if purchased_path.trim().is_empty() {
        PathBuf::from(&preview_path)
    } else {
        PathBuf::from(&purchased_path)
    };
    if !text_path.is_file() {
        return Err("找不到这篇作品的本地正文文件。".into());
    }
    let cookie = setting(&conn, "pixiv_cookie")?;
    let cookie = if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    };
    let quality = read_settings(&conn)?.image_quality;
    let client = pixiv_client(cookie)?;
    let detail = fetch_pixiv_novel_detail(&client, &novel_id)?;
    if detail.get("error").and_then(Value::as_bool) == Some(true) {
        return Err("Pixiv 拒绝了这篇作品的请求，请检查 Cookie。".into());
    }
    let body = detail.get("body").unwrap_or(&Value::Null);
    let content = json_string(body, "content");
    if content.is_empty() {
        return Err("Pixiv 返回的正文是空的，处理不了。".into());
    }
    let embedded = body
        .get("textEmbeddedImages")
        .cloned()
        .unwrap_or(Value::Null);
    let slots = plan_novel_images(&client, &embedded, &content, &text_path, &quality);
    Ok((
        NovelAssets {
            novel_id,
            title,
            author_name,
            release_date,
            tags,
            cover_path: text_path.with_extension("jpg"),
            text_path,
            cover_url: json_string(body, "coverUrl"),
            content,
            slots,
        },
        client,
    ))
}

/// 确保封面已经落在本地（没下过就补一张），返回本地封面文件名。
fn ensure_novel_cover(client: &Client, assets: &NovelAssets) -> String {
    if !assets.cover_path.exists() && !assets.cover_url.is_empty() {
        if let Ok(bytes) = fetch_pixiv_cover(client, &assets.cover_url) {
            let _ = fs::write(&assets.cover_path, &bytes);
        }
    }
    assets
        .cover_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 「重新下载 TXT 版并绑定」：从 Pixiv 重抓正文与配图，把纯文本落回作品所在目录，
/// 并**把作品绑定到这份 txt 上**（v0.3.60 刚加时只落文件、不动绑定，v0.3.63 按用户要求改成绑定）——
/// 与「重新下载 HTML / EPUB 版并绑定」走同一套 `bind_work_to_reading`，作品原来在哪一侧就还留在哪一侧，
/// 以后「打开作品」打开的就是这份 txt。
/// 正文里的插图写成 `[插图 N：{正文名}_images/001.jpg]` 这种指引，和同步下来的预览版同款。
/// 绑定文件本身就是 `.txt` 就直接覆盖它；是 `.epub` / `.htm` 那种就在旁边生成同名 `.txt`。
fn redownload_novel_txt_impl(work_id: i64) -> Result<String, String> {
    let conn = db()?;
    let (novel_id, purchased_path, preview_path): (String, String, String) = conn
        .query_row(
            "SELECT ifnull(pixiv_novel_id,''), ifnull(purchased_path,''), ifnull(preview_path,'') FROM works WHERE id=?1",
            [work_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("找不到这篇作品：{e}"))?;
    if novel_id.trim().is_empty() {
        return Err("这篇作品没有 Pixiv 作品 ID，抓不到正文；请先同步一次。".into());
    }
    // 有完整版就写在完整版旁边（单份模型里“在册的那一边”）
    let bound = if purchased_path.trim().is_empty() {
        preview_path.trim().to_string()
    } else {
        purchased_path.trim().to_string()
    };
    if bound.is_empty() {
        return Err("这篇作品还没绑定本地文件，先生成或绑定一个再重下。".into());
    }
    let bound_path = PathBuf::from(&bound);
    let is_txt = bound_path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("txt"))
        .unwrap_or(false);
    let text_path = if is_txt {
        bound_path.clone()
    } else {
        bound_path.with_extension("txt")
    };

    let cookie = setting(&conn, "pixiv_cookie")?;
    let cookie = if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    };
    let quality = read_settings(&conn)?.image_quality;
    let client = pixiv_client(cookie)?;

    let detail = fetch_pixiv_novel_detail(&client, &novel_id)?;
    if detail.get("error").and_then(Value::as_bool) == Some(true) {
        return Err("Pixiv 拒绝了这篇作品的请求，请检查 Cookie。".into());
    }
    let body = detail.get("body").unwrap_or(&Value::Null);
    let content = json_string(body, "content");
    if content.is_empty() {
        return Err("Pixiv 返回的正文是空的，处理不了。".into());
    }
    let embedded = body.get("textEmbeddedImages").cloned().unwrap_or(Value::Null);
    let slots = plan_novel_images(&client, &embedded, &content, &text_path, &quality);
    let images_dir = novel_images_dir(&text_path);
    // 图片已有的会跳过，所以反复点也只是补缺
    let (saved, _failed) = download_novel_images(&client, &slots, &images_dir);
    write_text_atomic(&text_path, &rewrite_novel_txt(&content, &slots))?;
    bind_work_to_reading(&conn, work_id, &text_path.to_string_lossy())?;
    if !slots.is_empty() {
        // 刚下下来的配图数最准，盖掉绑定后重算出来的结果：卡片上的带图徽标要跟正文里的插图对得上
        conn.execute(
            "UPDATE works SET image_count=?1, has_images=1 WHERE id=?2",
            params![saved as i64, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(text_path.to_string_lossy().to_string())
}

/// 双版本清理的统计结果（前端确认弹窗与 toast 都用它）。
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
struct PreviewCleanupSummary {
    /// 库里同时挂着预览版与完整版的作品数
    candidates: usize,
    /// 完整版文件确实在磁盘上、可以安全清掉预览版的作品数
    ready: usize,
    /// 完整版文件找不到、跳过的作品数（这些绝不动）
    skipped_missing_full: usize,
    /// 预览版正文还在、真被清掉的作品数
    cleaned: usize,
    /// 预览版正文早就不在、只清了库记录的作品数
    record_only: usize,
    /// 送进 Windows 回收站的文件 / 空目录数（v0.3.61 起不再自建回收目录）
    recycled: usize,
    /// 送回收站失败、只能原样留下的个数（这类作品**不改库记录**，下次还能再清）
    recycle_failed: usize,
    /// 搬到完整版目录的配图目录数（完整版那边还没有就搬过去，清了会断图）
    images_moved: usize,
    /// 跟着搬到完整版目录的封面数
    covers_moved: usize,
    /// 清空后一起收走的预览版空目录数
    empty_dirs: usize,
}

/// 清理判定（纯函数，便于测试）：完整版文件真的在磁盘上，预览版才算多余。
/// 完整版丢了还去清预览版，等于把这篇作品从磁盘上抹掉。
fn preview_is_redundant(purchased_exists: bool) -> bool {
    purchased_exists
}

/// 把一份文件 / 目录送进 **Windows 回收站**（v0.3.61：用户要求别自建回收目录，
/// 要和资源管理器里按 Delete 一个效果 —— 本盘 `$Recycle.Bin` 里，右键能还原）。
///
/// 走 `SHFileOperationW` + `FOF_ALLOWUNDO`，正是资源管理器删除时用的那条路。
/// 两个坑：
/// 1. `pFrom` 是**双 NUL 结尾**的宽字符串列表（每项一个 NUL，末尾再来一个）；
/// 2. **绝不能先 `canonicalize()`** —— 那会返回 `\\?\D:\...` 的 UNC 形式，回收站认不出来，
///    结果是**真删除**，那就把用户的文件弄没了。
///
/// 无确认、无错误弹窗（`FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT`），
/// 错误靠返回值 + 事后检查文件还在不在。
#[cfg(windows)]
fn recycle_to_bin(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
        FOF_NOERRORUI, FOF_SILENT,
    };

    if !path.exists() {
        return Ok(());
    }
    let mut list: Vec<u16> = path.as_os_str().encode_wide().collect();
    list.push(0);
    list.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(list.as_ptr()),
        // FILEOPERATION_FLAGS 是 newtype，取 .0 再收到 u16
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0 | FOF_SILENT.0) as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("送回收站失败（错误码 {code}）：{}", path.display()));
    }
    // 返回 0 也可能是被中止，落地检查一下最稳
    if path.exists() {
        return Err(format!("送回收站后文件还在原地：{}", path.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn recycle_to_bin(_path: &Path) -> Result<(), String> {
    Err("只有 Windows 才有回收站".into())
}

/// 「已有完整版就把预览版清掉」（v0.3.60；v0.3.61 起改送系统回收站）。
///
/// 单份模型下同一篇作品不该两边各占一份，但用户本地很多是历史遗留：完整版从别的网站
/// 弄来之后，预览版没跟着清。清理规则（每条都是踩过坑定的）：
/// 1. **只动完整版文件确实存在的作品** —— 完整版丢了还清预览版，作品就没了；
/// 2. 预览版正文与阅读版（同名 `.html` / `.epub`）**送进 Windows 回收站**
///    （`recycle_to_bin`，和资源管理器按 Delete 一个效果，本盘 `$Recycle.Bin` 里能还原）；
/// 3. 预览版目录里的配图目录 `{正文名}_images`：完整版目录已有同名就一起送回收站，
///    没有就**搬到完整版目录** —— 完整版的图文阅读版还指着它，清了就断图；
/// 4. 封面只要还在预览版目录就跟着搬走并改写 `cover_path`（2026-09-14 封面集体失效的教训：
///    正文搬了封面没搬，老目录一清封面全挂）；
/// 5. 预览版目录清空后，把这个空目录也送回收站；
/// 6. 最后清空 `preview_path`（完整版判定只看 `purchased_path`，不受影响）。
///
/// 正文送回收站失败的作品**整篇跳过、库记录也不改**（记进 `recycle_failed`），
/// 免得出现「文件还在、库里却说清了」的半拉状态。
///
/// `apply=false` 只统计不落地，给前端弹确认框用。
fn cleanup_redundant_previews_impl(apply: bool) -> Result<PreviewCleanupSummary, String> {
    let mut conn = db()?;
    let rows: Vec<(i64, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, ifnull(preview_path,''), ifnull(purchased_path,''), ifnull(cover_path,'') \
                 FROM works WHERE ifnull(preview_path,'')<>'' AND ifnull(purchased_path,'')<>''",
            )
            .map_err(|e| e.to_string())?;
        let collected = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        collected
    };

    let mut summary = PreviewCleanupSummary {
        candidates: rows.len(),
        ..Default::default()
    };
    let mut empty_dirs: Vec<PathBuf> = Vec::new();
    let mut updates: Vec<(i64, String)> = Vec::new();

    for (id, preview, purchased, cover) in rows {
        let purchased_path = PathBuf::from(purchased.trim());
        if !preview_is_redundant(purchased_path.is_file()) {
            summary.skipped_missing_full += 1;
            continue;
        }
        summary.ready += 1;
        let Some(full_dir) = purchased_path.parent().map(|dir| dir.to_path_buf()) else {
            continue;
        };
        let preview_path = PathBuf::from(preview.trim());
        let mut new_cover = cover.trim().to_string();

        if preview_path.is_file() {
            if apply {
                // 先动正文：它一失败就整篇跳过，别留下「正文还在、库记录却清了」的半拉状态
                if recycle_to_bin(&preview_path).is_err() {
                    summary.recycle_failed += 1;
                    continue;
                }
                summary.recycled += 1;
            }
            summary.cleaned += 1;
            // 同名附属产物：封面单独走第 4 条，其余按「配图目录 / 阅读版文件」分开处理
            let cover_key = path_key(cover.trim());
            for asset in sibling_assets(&preview_path) {
                if !asset.exists() || path_key(&asset.to_string_lossy()) == cover_key {
                    continue;
                }
                let Some(asset_name) = asset.file_name().map(|value| value.to_owned()) else {
                    continue;
                };
                if asset.is_dir() {
                    let destination = full_dir.join(&asset_name);
                    if destination.exists() {
                        if apply {
                            if recycle_to_bin(&asset).is_ok() {
                                summary.recycled += 1;
                            } else {
                                summary.recycle_failed += 1;
                            }
                        }
                    } else {
                        if apply {
                            move_path(&asset, &destination)?;
                        }
                        summary.images_moved += 1;
                    }
                } else if apply {
                    if recycle_to_bin(&asset).is_ok() {
                        summary.recycled += 1;
                    } else {
                        summary.recycle_failed += 1;
                    }
                }
            }
            if let Some(parent) = preview_path.parent() {
                empty_dirs.push(parent.to_path_buf());
            }
        } else {
            summary.record_only += 1;
        }

        // 封面：只要还在预览版目录里，就跟去完整版目录并把库里的路径改写过去
        if !cover.trim().is_empty() {
            let cover_path = PathBuf::from(cover.trim());
            let inside_preview = cover_path.parent().is_some_and(|parent| {
                preview_path
                    .parent()
                    .is_some_and(|preview_dir| path_key(&parent.to_string_lossy()) == path_key(&preview_dir.to_string_lossy()))
            });
            if cover_path.is_file() && inside_preview {
                if apply {
                    move_cover_along(cover.trim(), &full_dir)?;
                }
                let followed = follow_cover_path(cover.trim(), &purchased_path);
                if followed != cover.trim() {
                    new_cover = followed;
                    summary.covers_moved += 1;
                    // 新封面已经在完整版目录了，预览版目录里那份（没能搬过去的那份）收走
                    if apply
                        && path_key(&new_cover) != path_key(cover.trim())
                        && cover_path.is_file()
                    {
                        if recycle_to_bin(&cover_path).is_ok() {
                            summary.recycled += 1;
                        } else {
                            summary.recycle_failed += 1;
                        }
                    }
                }
            }
        }
        updates.push((id, new_cover));
    }

    // 预览版目录清空后，把空目录一起收走（只收真成了空壳的那种）
    for dir in empty_dirs {
        let is_empty = fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            continue;
        }
        summary.empty_dirs += 1;
        if apply && recycle_to_bin(&dir).is_ok() {
            summary.recycled += 1;
        }
    }

    if apply {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (id, new_cover) in updates {
            tx.execute(
                "UPDATE works SET preview_path='', cover_path=?1 WHERE id=?2",
                params![new_cover, id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(summary)
}

/// 预览版让位：先 `apply=false` 拿统计给用户看，确认后再 `apply=true` 真清。
#[tauri::command]
async fn cleanup_redundant_previews(apply: bool) -> Result<PreviewCleanupSummary, String> {
    tauri::async_runtime::spawn_blocking(move || cleanup_redundant_previews_impl(apply))
        .await
        .map_err(|e| e.to_string())?
}

/// 单篇「重新下载 TXT 版并绑定」：抓一份 Pixiv 原版正文、落成 txt，并把作品绑到它上面。
#[tauri::command]
async fn redownload_novel_txt(work_id: i64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || redownload_novel_txt_impl(work_id))
        .await
        .map_err(|e| e.to_string())?
}

/// 单篇作品的「重新下载阅读版并绑定」：抓详情 → 下配图（已有的跳过）→ 改写 txt 里的
/// 占位符 → 补封面 → 按**显式指定的格式**写出阅读版（HTML 或 EPUB，与正文同目录同名）
/// → 把作品绑定到这份阅读版上。格式由调用方传入，不看设置，所以菜单上两个按钮
/// 只差一个参数。可以反复执行：配图不重复下，等于原地重打包。
fn download_reading_version_impl(
    work_id: i64,
    format: ReadingFormat,
) -> Result<WorkReadingResult, String> {
    let (assets, client) = prepare_novel_assets(work_id)?;
    let images_dir = novel_images_dir(&assets.text_path);
    let (saved, failed) = download_novel_images(&client, &assets.slots, &images_dir);
    if !assets.slots.is_empty() {
        // 占位符只能改写真正的 txt：作品可能已经绑定在 `.epub` / `.html` 上，
        // 直接 write 那个路径会把电子书本体覆盖成纯文本（v0.3.46 留下的隐患）。
        let is_txt = assets
            .text_path
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("txt"))
            .unwrap_or(false);
        let txt_path = if is_txt {
            Some(assets.text_path.clone())
        } else {
            novel_text_path_beside(&assets.text_path)
        };
        if let Some(txt_path) = txt_path.filter(|path| path.is_file()) {
            write_text_atomic(&txt_path, &rewrite_novel_txt(&assets.content, &assets.slots))?;
        }
        db()?
            .execute(
                "UPDATE works SET image_count=?1, has_images=1 WHERE id=?2",
                params![saved as i64, work_id],
            )
            .map_err(|e| e.to_string())?;
    }
    let cover_file = ensure_novel_cover(&client, &assets);
    let (output_path, missing_count, size_bytes) = match format {
        ReadingFormat::Html => {
            let html = render_novel_html(
                &assets.content,
                &assets.slots,
                &NovelHtmlMeta {
                    title: &assets.title,
                    author_name: &assets.author_name,
                    release_date: &assets.release_date,
                    tags: &assets.tags,
                    cover_file: &cover_file,
                    image_count: saved,
                },
            );
            let path = novel_html_path(&assets.text_path);
            write_text_atomic(&path, &html)?;
            (path, 0usize, 0u64)
        }
        ReadingFormat::Epub => {
            let mut images: Vec<(String, Vec<u8>)> = Vec::new();
            let mut missing_count = 0;
            for slot in &assets.slots {
                match fs::read(images_dir.join(&slot.file_name)) {
                    Ok(bytes) if !bytes.is_empty() => {
                        images.push((slot.file_name.clone(), bytes))
                    }
                    _ => missing_count += 1,
                }
            }
            let cover_bytes = if cover_file.is_empty() {
                None
            } else {
                fs::read(&assets.cover_path)
                    .ok()
                    .filter(|bytes| !bytes.is_empty())
            };
            let cover = cover_bytes
                .as_deref()
                .map(|bytes| (cover_file.as_str(), bytes));
            let epub = build_novel_epub(
                &assets.title,
                &assets.author_name,
                &assets.release_date,
                &assets.novel_id,
                &assets.tags,
                &assets.content,
                &assets.slots,
                &images,
                cover,
            );
            let path = epub_target_path(&assets.text_path, "");
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("无法创建 EPUB 目录：{e}"))?;
            }
            write_bytes_atomic(&path, &epub)?;
            (path, missing_count, epub.len() as u64)
        }
    };
    let output_path = output_path.to_string_lossy().to_string();
    bind_work_to_reading(&db()?, work_id, &output_path)?;
    Ok(WorkReadingResult {
        format: format.as_str().to_string(),
        output_path,
        total_count: assets.slots.len(),
        saved_count: saved,
        failed_count: failed,
        missing_count,
        size_bytes,
    })
}

/// 作品当前绑定的文件旁边那个同名 `.txt`（存在才返回）。绑定被换成 `.html` / `.epub`
/// 之后，正文 txt 仍然躺在同一个目录里。
fn novel_text_path_beside(bound: &Path) -> Option<PathBuf> {
    let candidate = bound.with_extension("txt");
    candidate.is_file().then_some(candidate)
}

/// 作品是否已经绑定到目标格式的阅读版，且文件还在。批量「补下」用它跳过已经好的。
fn reading_already_bound(bound_path: &str, format: ReadingFormat) -> bool {
    if bound_path.trim().is_empty() {
        return false;
    }
    let path = Path::new(bound_path);
    let matches_format = path
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case(format.as_str()))
        .unwrap_or(false);
    matches_format && path.is_file()
}

/// 取一个作品的标题（批量进度条上显示用）。
fn work_title(work_id: i64) -> String {
    db()
        .ok()
        .and_then(|conn| {
            conn.query_row("SELECT title FROM works WHERE id=?1", [work_id], |row| {
                row.get::<_, String>(0)
            })
            .ok()
        })
        .unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadingBatchResult {
    /// 实际使用的格式：`html` 或 `epub`
    format: String,
    exported_count: usize,
    /// 已绑定到目标格式、直接跳过的篇数
    skipped_count: usize,
    failed_count: usize,
    image_count: usize,
    total_bytes: u64,
    last_path: String,
    failed_titles: Vec<String>,
}

/// 批量下载阅读版的进度事件（前端右下角进度浮层用）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReadingProgress {
    total: usize,
    current: usize,
    title: String,
    /// 收尾那一条：前端收到后收起进度浮层
    done: bool,
}

const READING_PROGRESS_EVENT: &str = "reading-download-progress";

fn emit_reading_progress(app: &AppHandle, total: usize, current: usize, title: String, done: bool) {
    let _ = app.emit(
        READING_PROGRESS_EVENT,
        ReadingProgress {
            total,
            current,
            title,
            done,
        },
    );
}

/// 把作品绑定切到刚生成的阅读版上（HTML 或 EPUB），**不改变预览版 / 完整版属性**。
///
/// 判定和界面一致：`purchased_path` 非空就是完整版，否则是预览版。
/// 阅读版与正文同目录同名，所以只把原来挂着的那一侧指过去即可 ——
/// 界面上的「完整版 / 预览版」标签不会因为换绑定而变化。
fn bind_work_to_reading(conn: &Connection, work_id: i64, reading_path: &str) -> Result<(), String> {
    let purchased: String = conn
        .query_row(
            "SELECT purchased_path FROM works WHERE id=?1",
            [work_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if purchased.trim().is_empty() {
        conn.execute(
            "UPDATE works SET preview_path=?1 WHERE id=?2",
            params![reading_path, work_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "UPDATE works SET purchased_path=?1 WHERE id=?2",
            params![reading_path, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    refresh_reading_image_count(conn, work_id);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshImageCountResult {
    /// 绑定在 epub / html 上的作品数
    scanned_count: usize,
    /// 图片数真的被改写过的篇数
    updated_count: usize,
}

/// 把全库里绑定在 `epub` / `html` 上的作品的图片数重算一遍，给存量作品补角标。
/// 绑在 txt 上的作品一概不碰。
#[tauri::command]
fn refresh_reading_image_counts() -> Result<RefreshImageCountResult, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT id, image_count FROM works WHERE purchased_path <> '' OR preview_path <> ''")
        .map_err(|e| e.to_string())?;
    let candidates: Vec<(i64, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(statement);

    let mut result = RefreshImageCountResult {
        scanned_count: 0,
        updated_count: 0,
    };
    for (work_id, before) in candidates {
        if !refresh_reading_image_count(&conn, work_id) {
            continue;
        }
        result.scanned_count += 1;
        let after: i64 = conn
            .query_row("SELECT image_count FROM works WHERE id=?1", [work_id], |row| {
                row.get(0)
            })
            .unwrap_or(before);
        if after != before {
            result.updated_count += 1;
        }
    }
    Ok(result)
}

/// 作品当前绑定的文件：完整版优先，其次预览版。
fn bound_path_of(work_id: i64) -> String {
    db()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT purchased_path, preview_path FROM works WHERE id=?1",
                [work_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .ok()
        })
        .map(|(purchased, preview)| {
            if purchased.trim().is_empty() {
                preview
            } else {
                purchased
            }
        })
        .unwrap_or_default()
}

/// 批量下载阅读版的格式：传空串就跟随设置里的 `sync_image_format`。
fn resolve_reading_format(value: &str) -> Result<ReadingFormat, String> {
    if value.trim().is_empty() {
        let conn = db()?;
        return read_settings(&conn).map(|settings| reading_format_of(&settings.sync_image_format));
    }
    Ok(reading_format_of(value))
}

/// 单篇「重新下载 HTML / EPUB 版并绑定」：格式由菜单按钮显式指定。
#[tauri::command]
async fn download_reading_version(
    work_id: i64,
    format: String,
) -> Result<WorkReadingResult, String> {
    let format = reading_format_of(&format);
    tauri::async_runtime::spawn_blocking(move || download_reading_version_impl(work_id, format))
        .await
        .map_err(|e| e.to_string())?
}

/// 批量「重新下载阅读版并绑定」：逐篇串行、全程发进度事件，篇与篇之间留一点间隔。
/// 已经绑定到目标格式、且文件还在的直接跳过，不重复下载。
fn run_reading_batch(
    app: &AppHandle,
    work_ids: Vec<i64>,
    format: ReadingFormat,
) -> ReadingBatchResult {
    let total = work_ids.len();
    let mut result = ReadingBatchResult {
        format: format.as_str().to_string(),
        exported_count: 0,
        skipped_count: 0,
        failed_count: 0,
        image_count: 0,
        total_bytes: 0,
        last_path: String::new(),
        failed_titles: Vec::new(),
    };
    emit_reading_progress(
        app,
        total,
        0,
        format!("准备处理 {total} 篇作品的 {} 版", format.label()),
        false,
    );
    for (index, work_id) in work_ids.iter().enumerate() {
        let title = work_title(*work_id);
        let label = if title.trim().is_empty() {
            format!("作品 {work_id}")
        } else {
            title
        };
        if reading_already_bound(&bound_path_of(*work_id), format) {
            result.skipped_count += 1;
            emit_reading_progress(
                app,
                total,
                index + 1,
                format!("已跳过（已绑定 {} 版）：{label}", format.label()),
                false,
            );
            continue;
        }
        emit_reading_progress(
            app,
            total,
            index,
            format!("正在生成 {} 版：{label}", format.label()),
            false,
        );
        match download_reading_version_impl(*work_id, format) {
            Ok(outcome) => {
                result.exported_count += 1;
                result.image_count += outcome.saved_count;
                result.total_bytes += outcome.size_bytes;
                result.last_path = outcome.output_path;
            }
            Err(_) => {
                result.failed_count += 1;
                if result.failed_titles.len() < 8 {
                    result.failed_titles.push(label.clone());
                }
            }
        }
        emit_reading_progress(app, total, index + 1, label, false);
        if index + 1 < work_ids.len() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    emit_reading_progress(
        app,
        total,
        total,
        format!("已完成 {} 篇", result.exported_count),
        true,
    );
    result
}

/// 批量入口（作品库里勾选作品后调用）。`format` 传空串＝跟随设置里的格式，
/// 传 `"html"` / `"epub"` 就是强制那种格式。
#[tauri::command]
async fn download_reading_versions(
    app: AppHandle,
    work_ids: Vec<i64>,
    format: String,
) -> Result<ReadingBatchResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let format = resolve_reading_format(&format)?;
        Ok(run_reading_batch(&app, work_ids, format))
    })
    .await
    .map_err(|e| e.to_string())?
}

// 「补下配图」不再单独占一个命令：它现在等价于对**选中作品**按设置格式跑批量
// `download_reading_versions`（format 传空串），所以原来的 backfill_work_images 已移除。

/// 取一篇作品的封面写到 `target`（已经有文件就直接返回），返回落盘路径。
fn fetch_missing_cover(client: &Client, novel_id: &str, target: &Path) -> Result<PathBuf, String> {
    if target.is_file() {
        return Ok(target.to_path_buf());
    }
    let detail = fetch_pixiv_novel_detail(client, novel_id)?;
    if detail.get("error").and_then(Value::as_bool) == Some(true) {
        return Err("Pixiv 拒绝了这篇作品的请求，请检查 Cookie。".into());
    }
    let body = detail.get("body").unwrap_or(&Value::Null);
    let url = json_string(body, "coverUrl");
    if url.is_empty() {
        return Err("这篇作品没有封面直链".into());
    }
    let bytes = fetch_pixiv_cover(client, &url)?;
    if bytes.is_empty() {
        return Err("封面是空文件".into());
    }
    write_bytes_atomic(target, &bytes)?;
    Ok(target.to_path_buf())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackfillCoversResult {
    fixed_count: usize,
    failed_count: usize,
    skipped_count: usize,
    failed_titles: Vec<String>,
}

/// 补齐失效的封面。作品在预览版/完整版之间搬家、或目录被整理过之后，库里的
/// cover_path 可能指向已经不存在的文件，界面就会显示一片「暂无封面」。
/// 这里按 pixiv_novel_id 重新取封面直链，下载到正文旁边（与正文同名）并更正 cover_path。
#[tauri::command]
async fn backfill_work_covers(author_id: Option<i64>) -> Result<BackfillCoversResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = db()?;
        let scope = author_id.unwrap_or(-1);
        let works: Vec<(i64, String, String, String, String, String)> = {
            let mut statement = conn
                .prepare(
                    "SELECT id, title, preview_path, purchased_path, cover_path, pixiv_novel_id \
                     FROM works WHERE pixiv_novel_id <> '' AND (?1 = -1 OR author_id = ?1) ORDER BY id",
                )
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([scope], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        let mut result = BackfillCoversResult {
            fixed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            failed_titles: Vec::new(),
        };
        let mut pending: Vec<(i64, String, PathBuf, String)> = Vec::new();
        for (work_id, title, preview_path, purchased_path, cover_path, novel_id) in works {
            if !cover_path.trim().is_empty() && Path::new(&cover_path).is_file() {
                continue;
            }
            // 封面跟正文放一起，正文优先取完整版
            let text = if !purchased_path.trim().is_empty() && Path::new(&purchased_path).is_file()
            {
                PathBuf::from(purchased_path)
            } else if !preview_path.trim().is_empty() && Path::new(&preview_path).is_file() {
                PathBuf::from(preview_path)
            } else {
                // 正文也不在了，没地方放封面
                result.skipped_count += 1;
                continue;
            };
            pending.push((work_id, title, text, novel_id));
        }
        if pending.is_empty() {
            return Ok(result);
        }
        let cookie = setting(&conn, "pixiv_cookie")?;
        let cookie = if cookie.trim().is_empty() {
            None
        } else {
            Some(normalize_pixiv_cookie(&cookie)?)
        };
        let client = pixiv_client(cookie)?;
        const COVER_CONCURRENCY: usize = 4;
        for batch in pending.chunks(COVER_CONCURRENCY) {
            let fetched = std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|(_, _, text_path, novel_id)| {
                        let client = client.clone();
                        let target = text_path.with_extension("jpg");
                        let novel_id = novel_id.clone();
                        scope.spawn(move || fetch_missing_cover(&client, &novel_id, &target))
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
            for ((work_id, title, _, _), outcome) in batch.iter().zip(fetched) {
                match outcome {
                    Ok(target) => {
                        conn.execute(
                            "UPDATE works SET cover_path=?1 WHERE id=?2",
                            params![target.to_string_lossy(), work_id],
                        )
                        .map_err(|e| e.to_string())?;
                        result.fixed_count += 1;
                    }
                    Err(error) => {
                        result.failed_count += 1;
                        if result.failed_titles.len() < 8 {
                            result.failed_titles.push(format!("{title}（{error}）"));
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 打开阅读版：先按设置里的格式找（HTML 单网页或 EPUB 电子书），找不到再退另一种格式。
#[tauri::command]
fn open_work_reading(work_id: i64) -> Result<(), String> {
    let conn = db()?;
    let (purchased, preview): (String, String) = conn
        .query_row(
            "SELECT purchased_path, preview_path FROM works WHERE id=?1",
            [work_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let path = if purchased.trim().is_empty() {
        preview
    } else {
        purchased
    };
    if path.trim().is_empty() {
        return Err("该作品还没有绑定本地文件。".into());
    }
    let settings = read_settings(&conn)?;
    let preferred = reading_format_of(&settings.sync_image_format);
    let text_path = Path::new(&path);
    for format in [preferred, preferred.other()] {
        let candidate = reading_output_path(text_path, format, "");
        if !candidate.is_file() {
            continue;
        }
        return match open::that(&candidate) {
            Ok(()) => Ok(()),
            // EPUB 不一定有默认阅读器，别报一句看不懂的错误
            Err(_) if format == ReadingFormat::Epub => Err(format!(
                "已经生成好 EPUB，但系统里没有默认打开它的程序，请手动打开：{}",
                candidate.display()
            )),
            Err(error) => Err(format!("无法打开阅读版：{error}")),
        };
    }
    Err("这篇还没有阅读版文件，先用菜单里的「下载配图」生成一份。".into())
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
fn toggle_has_images(work_id: i64) -> Result<(), String> {
    db()?
        .execute(
            "UPDATE works SET has_images = CASE has_images WHEN 1 THEN 0 ELSE 1 END WHERE id=?1",
            [work_id],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

// 特别关注：只翻转 authors.starred 标记，不动其他字段（保存作者、同步作者信息都不会覆盖它）
#[tauri::command]
fn toggle_author_starred(author_id: i64) -> Result<bool, String> {
    let conn = db()?;
    conn.execute(
        "UPDATE authors SET starred = CASE starred WHEN 1 THEN 0 ELSE 1 END WHERE id=?1",
        [author_id],
    )
    .map_err(|e| e.to_string())?;
    let starred: i64 = conn
        .query_row("SELECT starred FROM authors WHERE id=?1", [author_id], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok(starred == 1)
}

#[tauri::command]
fn set_has_images(work_ids: Vec<i64>, has_images: bool) -> Result<(), String> {
    if work_ids.is_empty() {
        return Ok(());
    }
    let mut conn = db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for work_id in work_ids {
        tx.execute(
            "UPDATE works SET has_images=?1 WHERE id=?2",
            params![if has_images { 1 } else { 0 }, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
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
fn open_work_directory(work_id: i64) -> Result<(), String> {
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
        return Err("该作品还没有绑定本地文件".into());
    }
    let target = PathBuf::from(&path);
    // 绑定的本身就是目录（有些作品直接绑文件夹），那就只有打开目录这一种做法
    if target.is_dir() {
        open::that(&target).map_err(|e| format!("无法打开目录：{e}"))?;
        return Ok(());
    }
    let dir = target
        .parent()
        .map(Path::to_path_buf)
        .filter(|dir| dir.exists())
        .ok_or_else(|| "绑定的本地目录已不存在".to_string())?;
    // 用户要的是「打开就能看见那个文件」，不是丢个目录让他自己翻，所以让资源管理器顺手选中它；
    // 文件已经不在原地（自己挪走或删了）就别报错，打开它原来待的目录也算交代得过去
    if target.exists() {
        reveal_in_explorer(&target)?;
    } else {
        open::that(&dir).map_err(|e| format!("无法打开目录：{e}"))?;
    }
    Ok(())
}

/// 拼 explorer 的 `/select,` 参数：`/select,"<路径>"`（引号只包路径，不包 `/select,`）。
/// 单独抽出来是为了能单测 —— 引号位置错一格，症状就是「不管点谁固定打开文档」，很难一眼看出来。
#[cfg(windows)]
fn select_argument(path: &Path) -> std::ffi::OsString {
    let mut argument = std::ffi::OsString::from("/select,\"");
    argument.push(path.as_os_str());
    argument.push("\"");
    argument
}

/// 在资源管理器里打开文件所在目录并选中该文件 —— Windows 上只有 `explorer /select,<路径>` 这条路。
/// 三个坑：
/// ① **`/select,` 必须留在引号外面**，只给路径加引号（`explorer /select,"D:\a b\f.txt"`）。
///    explorer 不按标准规则拆命令行：整条参数被引号包住时它认不出开头的 `/select,`，
///    退回去打开「文档」。库里路径基本都带空格（`D:\400 个人\…`），所以 v0.3.62 一上线就
///    「不管点谁固定打开 C:\Users\<用户>\Documents」。
/// ② 因此只能用 `raw_arg` 原样写进命令行；用 `arg()` 的话 Rust 见参数带空格会自动给整条加引号，正好踩①。
/// ③ explorer 即便成功也常返回非 0 退出码，所以只看能不能启动、不看退出码。
#[cfg(windows)]
fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer")
        .raw_arg(select_argument(path))
        .spawn()
        .map_err(|e| format!("无法打开资源管理器：{e}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    let dir = path.parent().unwrap_or(path);
    open::that(dir).map_err(|e| format!("无法打开目录：{e}"))
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

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("仅支持打开 HTTP 或 HTTPS 链接".into());
    }
    open::that(url).map_err(|e| format!("无法打开系统浏览器：{e}"))
}

/// 用配置好的搜索网站搜一段文字：拼好搜索页网址后交给系统浏览器打开，
/// 把最终用的网址回给前端（提示里要显示它）。
#[tauri::command]
fn open_search_site(url: String, keyword: String) -> Result<String, String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("这个搜索网站的网址不是 http/https 开头，请到设置里改一下".into());
    }
    if keyword.trim().is_empty() {
        return Err("没有可搜索的文字".into());
    }
    let target = build_search_url(url, &keyword);
    open::that(&target).map_err(|e| format!("无法打开系统浏览器：{e}"))?;
    Ok(target)
}

// 帮助文档不再单独发一个外置 help.html：内容（docs/index.html）已经并进应用内
// 「帮助」页，见 src/help-doc.js 与其生成脚本 scripts/build-help-doc.mjs。
// 所以原来那个 open_help_document 命令一起删掉了。

// ============================ 软件自动更新 ============================
//
// 便携版没有安装器，更新就三步：**下载新 exe → 启动它 → 把旧的送进回收站**，全在这里。
//
// 版本号**不查 GitHub API**：`api.github.com` 国内经常连不上，而且未登录每 IP 每小时只有
// 60 次额度，一个用户随便点点就超了。改成读 `releases/latest` 的跳转地址 —— 它本来就落在
// `/releases/tag/vX.Y.Z` 上，没有限流、不用令牌。直连通不了时依次走加速镜像。

/// 发布仓库。以后换托管地址只改这一行（下面所有 URL 都是拼出来的）。
const RELEASE_REPO: &str = "fromzero1501/pixiv-novel-downloader";
/// 便携版文件名前缀，用来认出「哪些文件是自己人」。发布时请沿用这个命名。
const PORTABLE_FILE_PREFIX: &str = "PixivNovelDownloader-v";
/// 升级包小于这个大小一律当成镜像的错误页丢掉（正常包 20 MB 上下）。
const MIN_UPDATE_BYTES: u64 = 1_000_000;

/// 内置加速镜像：直连失败后按顺序试。用户可在「设置 → 维护与数据」里增删。
fn default_update_mirrors() -> Vec<String> {
    vec![
        "https://ghproxy.net/".to_string(),
        "https://gh-proxy.com/".to_string(),
        "https://ghfast.top/".to_string(),
        "https://gh.xxooo.cf/".to_string(),
    ]
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateAsset {
    name: String,
    size: u64,
    /// 直连地址在前，后面是各镜像拼出来的地址，按顺序试。
    urls: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateCheck {
    current_version: String,
    latest_version: String,
    has_update: bool,
    /// 发布页直连地址与镜像地址（自动下载失败时给用户手动下载用）。
    release_url: String,
    release_mirror_url: String,
    asset: Option<UpdateAsset>,
    /// 版本号是从哪个源探到的，「直连 GitHub」或某个镜像前缀。
    source: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateDownloadProgress {
    received: u64,
    total: u64,
    source: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OldVersionCleanup {
    removed: Vec<String>,
    failed: Vec<String>,
}

/// 从一段文本里抠出 `X.Y.Z`：`v1.2.3`、`releases/tag/v1.2.3`、`1.2.3` 都认。
/// 认不出（比如 `releases/latest` 没跳转成功）就返回 `None`。
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let (mut index, bytes) = (0usize, text.as_bytes());
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        let mut cursor = index;
        let mut parts: Vec<u64> = Vec::new();
        while parts.len() < 3 {
            let digits_start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if cursor == digits_start {
                break;
            }
            match text[digits_start..cursor].parse::<u64>() {
                Ok(value) => parts.push(value),
                Err(_) => break,
            }
            if cursor < bytes.len() && bytes[cursor] == b'.' {
                cursor += 1;
            } else {
                break;
            }
        }
        if parts.len() == 3 {
            return Some((parts[0], parts[1], parts[2]));
        }
        index = start + 1;
    }
    None
}

/// 从 URL 或页面正文里找 `releases/tag/vX.Y.Z`，返回规范化后的 `X.Y.Z`。
/// 只看 `releases/tag/` 后面的内容，免得把页面里别的数字（日期、下载量）当成版本号。
fn version_from_tag_text(text: &str) -> Option<String> {
    let needle = "releases/tag/";
    let mut search = text;
    while let Some(index) = search.find(needle) {
        let rest = &search[index + needle.len()..];
        let rest = rest.strip_prefix('v').unwrap_or(rest);
        let candidate: String = rest
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect();
        if let Some(version) = parse_version(&candidate) {
            return Some(format!("{}.{}.{}", version.0, version.1, version.2));
        }
        search = &search[index + needle.len()..];
    }
    None
}

/// 下载 / 探测源：第一个是直连（前缀为空），后面是各镜像前缀。
fn update_sources(mirrors: &[String]) -> Vec<(String, String)> {
    let mut sources = vec![("直连 GitHub".to_string(), String::new())];
    for mirror in mirrors {
        let mirror = mirror.trim().trim_end_matches('/');
        if mirror.is_empty() {
            continue;
        }
        sources.push((mirror.to_string(), mirror.to_string()));
    }
    sources
}

/// 把镜像前缀和原始 GitHub 地址拼起来：`https://ghproxy.net/` + `https://github.com/...`
/// → `https://ghproxy.net/https://github.com/...`。用户少打一个斜杠也能拼对。
fn join_mirror(prefix: &str, url: &str) -> String {
    format!("{}/{}", prefix.trim_end_matches('/'), url)
}

/// 原始地址 + 各镜像上的同一地址，按「直连优先」的顺序排。
fn prefixed_urls(url: &str, mirrors: &[String]) -> Vec<String> {
    let mut urls = vec![url.to_string()];
    for mirror in mirrors {
        let mirror = mirror.trim().trim_end_matches('/');
        if mirror.is_empty() {
            continue;
        }
        let candidate = join_mirror(mirror, url);
        if !urls.contains(&candidate) {
            urls.push(candidate);
        }
    }
    urls
}

fn url_host(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
}

/// 探测 / 更新的 http 客户端。`system-proxy` 已开，用户挂着系统代理时直连也能走通。
fn update_client(timeout: Duration) -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("PixivNovelDownloader/{}", env!("CARGO_PKG_VERSION")))
        // 直连 GitHub 不通时是「连不上」而不是「慢」，连接超时短一点，
        // 免得启动时的自动检查卡在第一条源上半天才轮到镜像。
        .connect_timeout(Duration::from_secs(8))
        .timeout(timeout)
        .build()
        .map_err(|e| format!("无法创建网络请求：{e}"))
}

/// 问发布页当前最新是哪个版本：GET `releases/latest`，看它最终落在哪个 tag 上。
/// 返回 `(版本号, 成功的来源说明)`。
fn probe_latest_version(client: &Client, mirrors: &[String]) -> Result<(String, String), String> {
    let base = format!("https://github.com/{RELEASE_REPO}/releases/latest");
    let mut last_error = String::from("没有可用的更新源");
    for (label, prefix) in update_sources(mirrors) {
        let url = if prefix.is_empty() {
            base.clone()
        } else {
            join_mirror(&prefix, &base)
        };
        let response = match client.get(&url).send() {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("{label}：{error}");
                continue;
            }
        };
        // 跟随跳转后地址里就带着 tag；有的镜像不跟随，会把 Location 露出来
        if let Some(version) = version_from_tag_text(&response.url().to_string()) {
            return Ok((version, label));
        }
        if let Some(location) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
        {
            if let Some(version) = version_from_tag_text(location) {
                return Ok((version, label));
            }
        }
        // 还有的镜像会自己跟到 tag 页并把整页 HTML 返回，从正文里再找一次
        let body = response.text().unwrap_or_default();
        let head = &body[..body.len().min(200_000)];
        if let Some(version) = version_from_tag_text(head) {
            return Ok((version, label));
        }
        last_error = format!("{label}：没找到版本号");
    }
    Err(last_error)
}

/// 新版发布包叫什么。正常是裸 exe；历史上发过 zip，所以两个都当候选。
fn update_asset_names(version: &str) -> Vec<String> {
    vec![
        format!("{PORTABLE_FILE_PREFIX}{version}.exe"),
        format!("{PORTABLE_FILE_PREFIX}{version}.zip"),
    ]
}

fn release_asset_url(version: &str, name: &str) -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/download/v{version}/{name}")
}

/// 用 Range 请求（只要头 2 个字节）探一下文件在不在，顺便拿到总大小。
/// 不用 HEAD —— 部分镜像不认 HEAD，但都认 Range；也不会把 20 MB 拉下来。
fn probe_asset_size(client: &Client, url: &str) -> Option<u64> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-1")
        .send()
        .ok()?;
    if !(response.status().is_success()
        || response.status() == reqwest::StatusCode::PARTIAL_CONTENT)
    {
        return None;
    }
    let total = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        // `content-range: bytes 0-1/23586509` 里斜杠后面就是总大小
        .and_then(|value| value.rsplit('/').next())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .or_else(|| response.content_length())
        .unwrap_or(0);
    (total >= MIN_UPDATE_BYTES).then_some(total)
}

/// 挨个候选文件名 / 候选源探，确认哪个真的能下。
fn probe_update_asset(client: &Client, version: &str, mirrors: &[String]) -> Option<UpdateAsset> {
    for name in update_asset_names(version) {
        let urls = prefixed_urls(&release_asset_url(version, &name), mirrors);
        for url in &urls {
            if let Some(size) = probe_asset_size(client, url) {
                return Some(UpdateAsset {
                    name,
                    size,
                    urls,
                });
            }
        }
    }
    None
}

// ------------------------- 更新说明（Release 正文） -------------------------

/// 更新说明太长就把弹窗撑爆了，只留前面一段，后面让用户去发布页看。
const MAX_RELEASE_NOTES_CHARS: usize = 4000;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ReleaseNotes {
    version: String,
    /// 发布标题（tag 之外那句名字），没写就是空串
    title: String,
    /// 纯文本更新说明，HTML 标签已经剥掉
    notes: String,
    /// 说明是从哪个源取到的：「直连 GitHub」或某个镜像前缀
    source: String,
}

/// 把 HTML / XML 实体还原成字符。atom 的正文是「转义过的 HTML」，
/// 所以要先用它解一次 XML 转义、剥完标签后再解一次 HTML 转义。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < text.len() {
        if text.as_bytes()[index] != b'&' {
            // 按字符推进，别把多字节汉字切成半个
            let character = text[index..].chars().next().unwrap_or(' ');
            out.push(character);
            index += character.len_utf8();
            continue;
        }
        let rest = &text[index + 1..];
        // 实体名最长也就十几个字符，找不到分号就当普通 & 处理
        let end = match rest.find(';') {
            Some(position) if position <= 12 => position,
            _ => {
                out.push('&');
                index += 1;
                continue;
            }
        };
        let name = &rest[..end];
        let decoded = match name {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" => Some("'".to_string()),
            "nbsp" => Some(" ".to_string()),
            "hellip" => Some("…".to_string()),
            "mdash" => Some("—".to_string()),
            "ndash" => Some("–".to_string()),
            "middot" => Some("·".to_string()),
            "times" => Some("×".to_string()),
            "rarr" => Some("→".to_string()),
            "laquo" => Some("«".to_string()),
            "raquo" => Some("»".to_string()),
            _ => name.strip_prefix('#').and_then(|digits| {
                let hex = digits
                    .strip_prefix('x')
                    .or_else(|| digits.strip_prefix('X'));
                let code = match hex {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => digits.parse::<u32>().ok(),
                };
                code.and_then(char::from_u32)
            })
            .map(|character| character.to_string()),
        };
        match decoded {
            Some(value) => {
                out.push_str(&value);
                index += 1 + end + 1;
            }
            None => {
                out.push('&');
                index += 1;
            }
        }
    }
    out
}

/// 断行只在「上一行还没断过」时插，免得连续块元素攒出一堆空行。
fn push_line_break(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// 把一小段 HTML 变成能直接塞进弹窗的纯文本。只求可读：
/// 段落/列表/标题各占一行，`<li>` 前面补「- 」。
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut index = 0usize;
    while index < html.len() {
        if html.as_bytes()[index] != b'<' {
            let character = html[index..].chars().next().unwrap_or(' ');
            out.push(character);
            index += character.len_utf8();
            continue;
        }
        let Some(offset) = html[index..].find('>') else {
            break;
        };
        let tag = &html[index + 1..index + offset];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|character: char| character.is_whitespace() || character == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "br" | "p" | "div" | "li" | "ul" | "ol" | "tr" | "td" | "blockquote" | "pre" | "h1" | "h2"
                | "h3" | "h4" | "h5" | "h6" | "table"
        ) {
            if name == "ul" || name == "ol" || name == "table" {
                // 容器标签不自己占行，靠里面的 li / tr 断行
            } else if name == "li" && !closing {
                push_line_break(&mut out);
                out.push_str("- ");
            } else {
                push_line_break(&mut out);
            }
        }
        index += offset + 1;
    }
    out
}

/// 收尾：去掉行尾空白、合并连续空行、砍掉超长部分。
fn tidy_release_notes(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        // 空行只在前面已经有内容、且上一行不是空行时才留一个
        if line.trim().is_empty() {
            if lines.last().is_some_and(|last| last.trim().is_empty()) {
                continue;
            }
            if lines.is_empty() {
                continue;
            }
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|last| last.trim().is_empty()) {
        lines.pop();
    }
    let joined = lines.join("\n");
    if joined.chars().count() <= MAX_RELEASE_NOTES_CHARS {
        return joined;
    }
    let mut truncated: String = joined.chars().take(MAX_RELEASE_NOTES_CHARS).collect();
    truncated.push_str("\n…（更新说明较长，完整内容见发布页）");
    truncated
}

/// 从 atom feed 里切出每条 `<entry>`。没有 XML 库，纯字符串扫描就够用了。
fn atom_entries(feed: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut search = feed;
    while let Some(start) = search.find("<entry") {
        let rest = &search[start..];
        match rest.find("</entry>") {
            Some(end) => {
                let close = "</entry>".len();
                entries.push(&rest[..end + close]);
                search = &rest[end + close..];
            }
            None => break,
        }
    }
    entries
}

/// 取一条 entry 里某个标签的原始文本（`<content type="html">` 这种带属性的也认）。
fn atom_field<'a>(entry: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = entry.find(&open)?;
    let after_open = &entry[start + open.len()..];
    let content_start = start + open.len() + after_open.find('>')? + 1;
    let rest = &entry[content_start..];
    let end = rest.find(&format!("</{tag}>"))?;
    Some(&rest[..end])
}

/// 取发布说明：走 `releases.atom` 这个公开 feed。
/// 用它而不是 `api.github.com`，是因为 API 对匿名请求有每小时限流，而 feed 没有。
/// 返回 `(标题, 说明正文, 来源说明)`。
fn probe_release_notes(
    client: &Client,
    version: &str,
    mirrors: &[String],
) -> Result<(String, String, String), String> {
    let feed_url = format!("https://github.com/{RELEASE_REPO}/releases.atom");
    let mut last_error = String::from("没有可用的更新源");
    for (label, prefix) in update_sources(mirrors) {
        let url = if prefix.is_empty() {
            feed_url.clone()
        } else {
            join_mirror(&prefix, &feed_url)
        };
        let response = match client.get(&url).send() {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("{label}：{error}");
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = format!("{label}：HTTP {}", response.status().as_u16());
            continue;
        }
        let Ok(feed) = response.text() else {
            last_error = format!("{label}：响应读不出来");
            continue;
        };
        let entries = atom_entries(&feed);
        // feed 按时间倒序，第一条就是最新那版；能按 tag 对上就用对上的那条
        let picked = entries
            .iter()
            .find(|entry| version_from_tag_text(entry).as_deref() == Some(version))
            .or_else(|| entries.first());
        let Some(entry) = picked else {
            last_error = format!("{label}：feed 是空的");
            continue;
        };
        let raw = atom_field(entry, "content")
            .map(|content| decode_entities(content))
            .unwrap_or_default();
        let notes = tidy_release_notes(&html_to_text(&raw));
        if notes.trim().is_empty() {
            return Err("这版发布没有填写更新说明，点「打开发布页」可以看发布页".into());
        }
        let title = atom_field(entry, "title")
            .map(|title| tidy_release_notes(&decode_entities(title)))
            .unwrap_or_default();
        return Ok((title, notes, label));
    }
    Err(last_error)
}

/// 取指定版本的更新说明，给更新弹窗显示用（只在真的有新版时才调用）。
#[tauri::command]
async fn fetch_release_notes(version: String) -> Result<ReleaseNotes, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = db()?;
        let mirrors = read_update_mirrors(&conn)?;
        let client = update_client(Duration::from_secs(20))?;
        let (title, notes, source) = probe_release_notes(&client, &version, &mirrors)?;
        Ok(ReleaseNotes {
            version,
            title,
            notes,
            source,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn executable_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| format!("无法定位程序目录：{e}"))?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法定位程序目录".to_string())
}

/// 升级文件要落在程序旁边，所以先确认那个文件夹真的写得进去 ——
/// 放在 `Program Files` 里时会被 UAC 挡住，早点告诉用户比下完 20 MB 再报错强。
fn ensure_directory_writable(directory: &Path) -> Result<(), String> {
    let probe = directory.join(format!(".update-write-test-{}", std::process::id()));
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(error) => Err(format!(
            "程序所在文件夹不能写入（{error}）。把程序挪到桌面或文档这类有写入权限的位置再更新。"
        )),
    }
}

fn looks_like_windows_program(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 2];
    file.read_exact(&mut header).is_ok() && &header == b"MZ"
}

/// 边下边发进度事件。中途断了、或者下回来是镜像的错误页，都在这里拦下来。
fn stream_update_to_file(
    client: &Client,
    url: &str,
    path: &Path,
    app: &AppHandle,
) -> Result<(), String> {
    let mut response = client.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "{} 返回 HTTP {}",
            url_host(url),
            response.status().as_u16()
        ));
    }
    let total = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(path).map_err(|e| format!("无法写入临时文件：{e}"))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut received: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|e| format!("下载中断：{e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("写入失败：{e}"))?;
        received += read as u64;
        if last_emit.elapsed() >= Duration::from_millis(200) {
            last_emit = std::time::Instant::now();
            let _ = app.emit(
                "update-download-progress",
                UpdateDownloadProgress {
                    received,
                    total,
                    source: url_host(url),
                },
            );
        }
    }
    let _ = app.emit(
        "update-download-progress",
        UpdateDownloadProgress {
            received,
            total,
            source: url_host(url),
        },
    );
    if received < MIN_UPDATE_BYTES {
        return Err(format!(
            "{} 只下到 {received} 字节，像是镜像的错误页",
            url_host(url)
        ));
    }
    Ok(())
}

/// 在解出来的目录里找便携版 exe。优先认自己的命名，找不到就退回「随便一个够大的 exe」。
fn find_program_in(directory: &Path) -> Option<PathBuf> {
    let mut fallback = None;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.to_ascii_lowercase().ends_with(".exe") {
                continue;
            }
            let big_enough = fs::metadata(&path)
                .map(|meta| meta.len() >= MIN_UPDATE_BYTES)
                .unwrap_or(false);
            if !big_enough {
                continue;
            }
            if name.starts_with(PORTABLE_FILE_PREFIX) {
                return Some(path);
            }
            fallback.get_or_insert(path);
        }
    }
    fallback
}

/// 历史版本发的是 zip 包。用 Windows 自带的 `tar.exe`（Win10 1803 起都有）解到临时目录，
/// **只把里面的 exe 拿走** —— 绝不解压进程序目录，免得包里的 `data/` 覆盖掉用户的数据库。
fn extract_program_from_zip(archive: &Path, program_path: &Path) -> Result<(), String> {
    let staging = std::env::temp_dir().join(format!("pixiv-novel-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("无法创建临时目录：{e}"))?;
    let result = match std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
    {
        Ok(status) if status.success() => find_program_in(&staging)
            .ok_or_else(|| "压缩包里没找到程序文件".to_string())
            .and_then(|found| {
                fs::copy(&found, program_path)
                    .map(|_| ())
                    .map_err(|e| format!("无法写入新版程序：{e}"))
            }),
        Ok(status) => Err(format!(
            "解压失败（tar 退出码 {}）",
            status.code().unwrap_or(-1)
        )),
        Err(error) => Err(format!("系统里没有解压工具，请手动下载压缩包：{error}")),
    };
    let _ = fs::remove_dir_all(&staging);
    result
}

/// 问发布页要最新版本号，并看新版的下载文件在不在（不下载正文）。
#[tauri::command]
async fn check_for_update() -> Result<UpdateCheck, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let conn = db()?;
        let mirrors = read_update_mirrors(&conn)?;
        let client = update_client(Duration::from_secs(30))?;
        let current = env!("CARGO_PKG_VERSION").to_string();
        let (latest, source) = probe_latest_version(&client, &mirrors)?;
        let current_tuple = parse_version(&current).unwrap_or((0, 0, 0));
        let latest_tuple = parse_version(&latest).unwrap_or((0, 0, 0));
        let release_url = format!("https://github.com/{RELEASE_REPO}/releases/tag/v{latest}");
        Ok(UpdateCheck {
            has_update: latest_tuple > current_tuple,
            asset: (latest_tuple > current_tuple)
                .then(|| probe_update_asset(&client, &latest, &mirrors))
                .flatten(),
            release_mirror_url: mirrors
                .iter()
                .find(|mirror| !mirror.trim().is_empty())
                .map(|mirror| join_mirror(mirror, &release_url))
                .unwrap_or_else(|| release_url.clone()),
            release_url,
            latest_version: latest,
            current_version: current,
            source,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 下载新版：先落到 `.part`，确认是完整的程序再改名 —— 半截文件不会被当成新版本。
/// 每个候选地址依次试，上一个失败了就换下一个镜像。
#[tauri::command]
async fn download_update(
    app: AppHandle,
    version: String,
    asset_name: String,
    urls: Vec<String>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let directory = executable_directory()?;
        ensure_directory_writable(&directory)?;
        let asset_name = Path::new(&asset_name)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "升级文件名无效".to_string())?
            .to_string();
        let program_path = directory.join(format!("{PORTABLE_FILE_PREFIX}{version}.exe"));
        let part_path = directory.join(format!("{asset_name}.part"));
        let client = update_client(Duration::from_secs(1800))?;
        let mut last_error = String::from("没有可用的下载地址");
        let mut finished = false;
        for url in &urls {
            if let Err(error) = stream_update_to_file(&client, url, &part_path, &app) {
                last_error = error;
                let _ = fs::remove_file(&part_path);
                continue;
            }
            let extracted = if asset_name.to_ascii_lowercase().ends_with(".zip") {
                extract_program_from_zip(&part_path, &program_path)
            } else {
                fs::rename(&part_path, &program_path).map_err(|e| format!("无法写入新版程序：{e}"))
            };
            match extracted {
                Ok(()) => {
                    finished = true;
                    break;
                }
                Err(error) => {
                    last_error = error;
                    let _ = fs::remove_file(&part_path);
                }
            }
        }
        let _ = fs::remove_file(&part_path);
        if !finished {
            let _ = fs::remove_file(&program_path);
            return Err(format!("所有下载源都失败了：{last_error}"));
        }
        if !looks_like_windows_program(&program_path) {
            let _ = fs::remove_file(&program_path);
            return Err("下载回来的文件不是可执行程序，请到发布页手动下载".into());
        }
        Ok(program_path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 启动新版并退出自己。新旧两个进程只在这个瞬间并存，旧进程随后就没了。
#[tauri::command]
fn apply_update(app: AppHandle, path: String) -> Result<(), String> {
    let program = PathBuf::from(&path);
    let directory = executable_directory()?;
    // 只允许启动「程序目录里、自己命名的」那个 exe —— 别让一个错路径把别的程序拉起来
    if program.parent() != Some(directory.as_path()) {
        return Err("升级文件不在程序所在目录里".into());
    }
    let name = program
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !name.starts_with(PORTABLE_FILE_PREFIX) || !name.to_ascii_lowercase().ends_with(".exe") {
        return Err("升级文件的名字不对".into());
    }
    if !program.is_file() {
        return Err("新版程序不见了，请重新下载".into());
    }
    std::process::Command::new(&program)
        .current_dir(&directory)
        .spawn()
        .map_err(|e| format!("无法启动新版本：{e}"))?;
    app.exit(0);
    Ok(())
}

/// 更新完之后旧版本还躺在同一个文件夹里。启动时扫一遍把它们送进回收站：
/// 只认自己的命名（`PixivNovelDownloader-vX.Y.Z.exe`）且版本比自己低，别的东西一律不碰。
#[tauri::command]
fn cleanup_old_portable_builds() -> Result<OldVersionCleanup, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable_directory()?;
    let current_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let current_version = parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0));
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    let entries = fs::read_dir(&directory).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|name| name.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        // 正在跑的这个自己当然不能删；`path` 和 `current_exe()` 的写法可能不完全一致，
        // 所以文件名也比一遍。
        if path == executable || name == current_name || !path.is_file() {
            continue;
        }
        let Some(rest) = name.strip_prefix(PORTABLE_FILE_PREFIX) else {
            continue;
        };
        if !rest.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        let Some(version) = parse_version(rest) else {
            continue;
        };
        if version >= current_version {
            continue;
        }
        match recycle_to_bin(&path) {
            Ok(()) => removed.push(name),
            Err(_) => failed.push(name),
        }
    }
    Ok(OldVersionCleanup { removed, failed })
}

/// 前端「恢复默认镜像」按钮用。
#[tauri::command]
fn default_update_mirror_list() -> Vec<String> {
    default_update_mirrors()
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
            insert_local_work(&tx, author_id, &title, &date).map_err(|e| e.to_string())?;
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
            set_author_order,
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
            bind_work_with_rename,
            auto_group_purchased_files,
            confirm_manual_group,
            copy_previews_to_purchased,
            mark_work_as_full,
            mark_work_as_preview,
            delete_work,
            delete_works,
            toggle_favorite,
            toggle_has_images,
            toggle_author_starred,
            set_has_images,
            open_work,
            open_work_directory,
            download_reading_version,
            download_reading_versions,
            redownload_novel_txt,
            cleanup_redundant_previews,
            backfill_work_covers,
            refresh_reading_image_counts,
            list_characters,
            add_character,
            update_character,
            delete_character,
            add_character_game,
            rename_character_game,
            delete_character_game,
            open_work_reading,
            open_external_url,
            open_search_site,
            preview_import,
            commit_import,
            read_import_file,
            read_import_folder,
            sync_pixiv_novels,
            cancel_pixiv_sync,
            sync_pixiv_author_profile,
            check_pixiv_cookie,
            update_work_tags,
            export_backup,
            restore_backup,
            check_for_update,
            fetch_release_notes,
            download_update,
            apply_update,
            cleanup_old_portable_builds,
            default_update_mirror_list
        ])
        .run(tauri::generate_context!())
        .expect("启动应用时发生错误");
}

#[cfg(test)]
mod tests {
    use super::{
        AUTHOR_ORDER_BY,
        bind_work_to_reading, build_novel_epub, build_search_url, clip_match_title,
        collect_files_recursively,
        collect_pending_purchased_files,
        direct_target_path, distribute_file, epub_target_path, existing_sync_target, file_name,
        follow_cover_path, preview_is_redundant, recycle_to_bin,
        has_invalid_date_range, insert_local_work, is_after_last_sync, is_generated_asset,
        is_image_like_asset,
        is_within_date_range, matched_sync_preview, move_cover_along, move_path, name_key,
        sibling_assets,
        characters_in_text, detect_author_in_name, file_stem, import_builtin_characters,
        resolve_match_mode, scope_allows_path, shared_characters, MatchMode,
        needs_a_purchased_file,
        normalize_aliases, normalize_author_homepage, normalize_pixiv_cookie,
        novel_image_filename_extension, novel_image_refs, novel_image_url, novel_images_dir,
        count_reading_images, novel_text_path_beside,
        path_key,
        pixiv_novel_id_from_url, pixiv_published_at, read_settings,
        populate_work_display_info, reading_already_bound, reading_format_of,
        reading_output_path, refresh_reading_image_count, render_novel_html,
        render_novel_xhtml, resolve_cover_path, rewrite_novel_txt,
        sanitize_file_name,
        select_argument,
        should_import_folder_entry, similarity_percent, similarity_with_title_limit,
        sort_works_by_content_size,
        default_update_mirrors, join_mirror, parse_version, prefixed_urls,
        probe_latest_version, probe_update_asset, update_asset_names, update_client,
        version_from_tag_text,
        atom_entries, atom_field, decode_entities, html_to_text, probe_release_notes, tidy_release_notes,
        synopsis_indicates_preview,
        text_word_count, title_indicates_images, unique_target_path, write_reading_output,
        zip_crc32, zip_finish, zip_push, ConflictAction,
        DistributeTarget, NovelHtmlMeta, NovelImageSlot, ReadingFormat, ReadingWriteMeta,
        SyncPreviewEntry, Work,
    };
    use chrono::{DateTime, NaiveDate, Utc};
    use rusqlite::Connection;
    use serde_json::json;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    /// 「打开本地目录」选中文件靠的是 explorer 的 `/select,`，引号只包路径这一条是硬要求：
    /// 整条被引号包住时 explorer 认不出 `/select,`，会退回打开「文档」（v0.3.62~63 的翻车点）。
    #[cfg(windows)]
    #[test]
    fn select_argument_quotes_only_the_path() {
        let argument = select_argument(Path::new(r"D:\400 个人\小说\已购版\鉄iron5\作品名 a.txt"));
        assert_eq!(
            argument.to_string_lossy().to_string(),
            "/select,\"D:\\400 个人\\小说\\已购版\\鉄iron5\\作品名 a.txt\""
        );
        assert!(argument.to_string_lossy().starts_with("/select,\""));
    }

    #[test]
    fn characters_are_found_inside_chinese_titles() {
        let index = vec![
            ("甘雨".to_string(), "原神".to_string()),
            ("雷电将军".to_string(), "原神".to_string()),
            ("神里绫华".to_string(), "原神".to_string()),
        ];
        let hits = characters_in_text("雷电将军与甘雨的午后", &index);
        assert!(hits.contains(&"雷电将军".to_string()));
        assert!(hits.contains(&"甘雨".to_string()));
        assert!(!hits.contains(&"神里绫华".to_string()));
        assert!(characters_in_text("标题里没有角色名", &index).is_empty());
    }

    #[test]
    fn character_mode_needs_only_one_shared_name() {
        let left = vec!["甘雨".to_string(), "刻晴".to_string()];
        assert_eq!(
            shared_characters(&left, &["甘雨".to_string()]),
            vec!["甘雨".to_string()]
        );
        assert!(shared_characters(&left, &["钟离".to_string()]).is_empty());
        assert!(shared_characters(&left, &[]).is_empty());
    }

    #[test]
    fn author_name_is_recognised_in_file_names() {
        let authors = vec![
            (7_i64, "蓝蓝天".to_string(), "蓝天|blue".to_string()),
            (9_i64, "某位".to_string(), String::new()),
        ];
        // 第一层：显式「作者」标记
        let found = detect_author_in_name("甘雨同人 作者：蓝蓝天", &authors).unwrap();
        assert_eq!(found.author_id, Some(7));
        assert_eq!(found.name, "蓝蓝天");
        // 第一层抓到名字但库里没有这个人 → author_id 为空，调用方会归进「作者不在库」
        let unknown = detect_author_in_name("甘雨同人 作者：查无此人", &authors).unwrap();
        assert_eq!(unknown.author_id, None);
        assert_eq!(unknown.name, "查无此人");
        // 第二层：没有「作者」二字，靠倒查作者库（别名也算）
        assert_eq!(
            detect_author_in_name("甘雨的午后 - blue", &authors)
                .unwrap()
                .author_id,
            Some(7)
        );
        assert_eq!(
            detect_author_in_name("甘雨的午后-某位", &authors)
                .unwrap()
                .author_id,
            Some(9)
        );
        // 认不出来就不返回，避免把普通词当人名
        assert!(detect_author_in_name("甘雨的午后", &authors).is_none());
        // 裸的 "by " 不算标记，英文标题里的 by 不能被当成人名
        assert!(detect_author_in_name("Stand by Me", &authors).is_none());
    }

    #[test]
    fn detected_other_author_is_out_of_scope() {
        // 没认出作者 → 参与匹配
        assert!(scope_allows_path(None, 5));
        // 认出的就是当前这位作者 → 参与匹配
        assert!(scope_allows_path(Some(5), 5));
        // 认出的作者是库里另一位（注意：比较的是作者 ID，不是作品 ID）→ 不参与
        assert!(!scope_allows_path(Some(7), 5));
    }

    #[test]
    fn match_mode_defaults_to_title() {
        assert_eq!(resolve_match_mode("").unwrap(), MatchMode::Title);
        assert_eq!(resolve_match_mode("title").unwrap(), MatchMode::Title);
        assert_eq!(resolve_match_mode("character").unwrap(), MatchMode::Character);
        assert!(resolve_match_mode("whatever").is_err());
    }

    #[test]
    fn file_stem_drops_directory_and_extension() {
        assert_eq!(file_stem(Path::new("D:/书库/甘雨同人 作者：蓝蓝天.txt")), "甘雨同人 作者：蓝蓝天");
        assert_eq!(file_stem(Path::new("D:/书库/无名作品.epub")), "无名作品");
    }

    #[test]
    fn builtin_characters_can_be_imported() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE characters (
               id INTEGER PRIMARY KEY,
               game TEXT NOT NULL DEFAULT '',
               name TEXT NOT NULL,
               aliases TEXT NOT NULL DEFAULT '',
               heat INTEGER NOT NULL DEFAULT 0,
               source TEXT NOT NULL DEFAULT 'builtin',
               enabled INTEGER NOT NULL DEFAULT 1,
               UNIQUE(game, name)
             );",
        )
        .unwrap();
        import_builtin_characters(&conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0))
            .unwrap();
        assert!(count > 200, "内置角色应该在 200 条以上，实际 {count}");
        let ganyu: i64 = conn
            .query_row("SELECT COUNT(*) FROM characters WHERE name='甘雨'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ganyu, 1);
        // 重复导入不会产生副本
        import_builtin_characters(&conn);
        let again: i64 = conn
            .query_row("SELECT COUNT(*) FROM characters", [], |row| row.get(0))
            .unwrap();
        assert_eq!(again, count);
    }

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
    fn target_file_name_is_sanitized_and_deduplicated() {
        let dir = std::env::temp_dir().join(format!("group-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        assert_eq!(sanitize_file_name("书名：番外/卷一"), "书名：番外_卷一");
        assert_eq!(
            unique_target_path(&dir, "同一本书", "7z"),
            dir.join("同一本书.7z")
        );
        // 已存在同名文件时应追加序号，保证每个作品都有独立的一份
        fs::write(dir.join("同一本书.7z"), b"x").unwrap();
        assert_eq!(
            unique_target_path(&dir, "同一本书", "7z"),
            dir.join("同一本书 (1).7z")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn distributing_one_file_to_many_works_keeps_a_copy_for_each() {
        let root = std::env::temp_dir().join(format!("distribute-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("自动分组");
        let dir_a = root.join("作者A");
        let dir_b = root.join("作者B");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        let file = source.join("某本书 完整版.7z");
        fs::write(&file, b"book").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (id INTEGER PRIMARY KEY, purchased_path TEXT NOT NULL DEFAULT '');
             INSERT INTO works (id) VALUES (1), (2);",
        )
        .unwrap();

        let targets = vec![
            DistributeTarget {
                work_id: 1,
                dir: dir_a.clone(),
                title: "A 的书名".into(),
                extension: "7z".into(),
            },
            DistributeTarget {
                work_id: 2,
                dir: dir_b.clone(),
                title: "B 的书名".into(),
                extension: "7z".into(),
            },
        ];
        let outcome = distribute_file(&conn, &file, &targets, ConflictAction::Skip).unwrap();
        assert_eq!(outcome.bound_count, 2);
        assert_eq!(outcome.skipped_count, 0);

        // 每个作品都拿到一份，源文件被清理
        assert_eq!(fs::read(dir_a.join("A 的书名.7z")).unwrap(), b"book");
        assert_eq!(fs::read(dir_b.join("B 的书名.7z")).unwrap(), b"book");
        assert!(!file.exists());
        let bound: i64 = conn
            .query_row("SELECT COUNT(*) FROM works WHERE purchased_path <> ''", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(bound, 2);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn existing_target_file_follows_the_chosen_conflict_action() {
        let root = std::env::temp_dir().join(format!("conflict-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("自动分组");
        let dir = root.join("作者A");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (id INTEGER PRIMARY KEY, purchased_path TEXT NOT NULL DEFAULT '');
             INSERT INTO works (id) VALUES (1);",
        )
        .unwrap();

        let make_source = |name: &str, body: &[u8]| {
            let path = source.join(name);
            fs::write(&path, body).unwrap();
            path
        };
        let target = DistributeTarget {
            work_id: 1,
            dir: dir.clone(),
            title: "某本书".into(),
            extension: "7z".into(),
        };
        let existing = direct_target_path(&dir, "某本书", "7z");
        assert_eq!(existing, dir.join("某本书.7z"));

        // 已有同名文件 + 跳过：保持原文件不动，源文件留在原处
        fs::write(&existing, b"old").unwrap();
        let file = make_source("某本书 完整版.7z", b"new");
        let outcome =
            distribute_file(&conn, &file, std::slice::from_ref(&target), ConflictAction::Skip)
                .unwrap();
        assert_eq!(outcome.skipped_count, 1);
        assert_eq!(outcome.bound_count, 0);
        assert_eq!(fs::read(&existing).unwrap(), b"old");
        assert!(file.exists());

        // 覆盖：原文件被替换
        let outcome = distribute_file(
            &conn,
            &file,
            std::slice::from_ref(&target),
            ConflictAction::Overwrite,
        )
        .unwrap();
        assert_eq!(outcome.bound_count, 1);
        assert_eq!(fs::read(&existing).unwrap(), b"new");
        assert!(!file.exists());

        // 另存一份：生成「某本书 (1).7z」，原文件保留
        fs::write(&existing, b"old").unwrap();
        let file = make_source("某本书 完整版 (1).7z", b"copy");
        let outcome = distribute_file(
            &conn,
            &file,
            std::slice::from_ref(&target),
            ConflictAction::KeepBoth,
        )
        .unwrap();
        assert_eq!(outcome.bound_count, 1);
        assert_eq!(fs::read(&existing).unwrap(), b"old");
        assert_eq!(fs::read(dir.join("某本书 (1).7z")).unwrap(), b"copy");
        assert!(!file.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn similarity_lowers_the_score_when_the_file_name_carries_extra_noise() {
        // v0.3.71 起分母是两边里更长的那个：文件名整段包含了作品名，
        // 但日期前缀 +「完整版」「.7z」这些多出来的字数照样扣分 → 6/11 = 54%
        assert_eq!(
            similarity_percent("希儿布洛妮娅", "2025-10-05 希儿布洛妮娅 完整版.7z"),
            54
        );
        // 作品名 5 个字（夏日短篇集），文件名归一化后 9 个字（夏日短篇合集 + pdf，
        // `.pdf` 不在 name_key 的去扩展名名单里），共同前缀 4 个字 → 4/9 = 44%
        assert_eq!(similarity_percent("夏日短篇集", "夏日短篇合集.pdf"), 44);
    }

    #[test]
    fn similarity_divides_by_the_longer_name() {
        // v0.3.71 起分母是**两边里较长的那个长度**（作品名与文件名谁长按谁算）：
        // 任意一侧多出来的内容都会按比例扣分
        assert_eq!(similarity_percent("ABCD", "AB"), 50);
        assert_eq!(similarity_percent("ABCD", "CD"), 50);
        // 最长公共**连续**子串：AD 在 ABCD 里不连续，只能算 1 个字 → 25%
        assert_eq!(similarity_percent("ABCD", "AD"), 25);
        assert_eq!(similarity_percent("ABCD", "ABCD"), 100);
        // 文件名更长：作品名 ABC 整段被包含，但多出来的 DEF 要扣分 → 3/6 = 50%
        assert_eq!(similarity_percent("ABC", "ABCDEF"), 50);
        // 纯后缀匹配：作品名 8 个字、文件名只对上后 2 个字 → 25%
        assert_eq!(similarity_percent("春夏秋冬梅兰竹菊", "兰竹"), 25);
    }

    #[test]
    fn author_order_puts_dragged_first_and_untouched_by_name_last() {
        // 用真 SQL（同一个 AUTHOR_ORDER_BY）跑一遍顺序语义：
        // 全都没拖过（sort_order=0）→ 纯按名字；拖过之后按 sort_order；新作者（0）排在拖过的后面。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE authors (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 sort_order INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO authors (id, name, sort_order) VALUES
                 (1, 'Beta', 0),
                 (2, 'Alpha', 0),
                 (3, 'Gamma', 0);",
        )
        .unwrap();
        let ordered = |conn: &Connection| -> Vec<String> {
            let sql = format!("SELECT a.name FROM authors a{AUTHOR_ORDER_BY}");
            let mut statement = conn.prepare(&sql).unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        // 谁都没拖过 → 按名字
        assert_eq!(ordered(&conn), vec!["Alpha", "Beta", "Gamma"]);

        // 手动把 Beta 拖到第一 → 只有它被写了 sort_order，其余仍是 0、排在它后面按名字
        conn.execute("UPDATE authors SET sort_order=1 WHERE id=1", [])
            .unwrap();
        assert_eq!(ordered(&conn), vec!["Beta", "Alpha", "Gamma"]);

        // 真正拖一次会把顺序表整个写一遍（下标从 1 开始）
        for (index, id) in [3_i64, 1, 2].iter().enumerate() {
            conn.execute(
                "UPDATE authors SET sort_order=?1 WHERE id=?2",
                (index as i64 + 1, *id),
            )
            .unwrap();
        }
        assert_eq!(ordered(&conn), vec!["Gamma", "Beta", "Alpha"]);

        // 之后新加的作者（sort_order=0）落在最后，不会插到手动顺序中间
        conn.execute(
            "INSERT INTO authors (id, name, sort_order) VALUES (4, 'Delta', 0)",
            [],
        )
        .unwrap();
        assert_eq!(ordered(&conn), vec!["Gamma", "Beta", "Alpha", "Delta"]);
    }

    #[test]
    fn image_flag_follows_the_title_head() {
        assert!(title_indicates_images("【插画】某部作品"));
        assert!(title_indicates_images("图文并茂的短篇集"));
        assert!(title_indicates_images("某作者作品 插画版 全本"));
        // 关键词出现在第 16 个字之后 → 不参与判定
        assert!(!title_indicates_images("这是一个足够长的作品标题占位符文本内容插画"));
        // 全文无关键词
        assert!(!title_indicates_images("某部普通小说无插图"));
    }

    #[test]
    fn local_works_are_flagged_on_insert() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 author_id INTEGER NOT NULL,
                 title TEXT NOT NULL,
                 release_date TEXT NOT NULL,
                 has_images INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();

        insert_local_work(&conn, 1, "（插画附）租借女友", "").unwrap();
        insert_local_work(&conn, 1, "某部普通小说", "").unwrap();

        let flags: Vec<i64> = conn
            .prepare("SELECT has_images FROM works ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(flags, vec![1, 0]);
    }

    #[test]
    fn image_files_never_join_the_association() {
        // 封面、插图、配图文件夹都不参与关联
        assert!(is_image_like_asset(Path::new("D:/书库/封面.jpg")));
        assert!(is_image_like_asset(Path::new("D:/书库/插图.PNG")));
        assert!(is_image_like_asset(Path::new("D:/书库/标题_images")));
        // 能绑的照旧：文本、电子书、HTML，以及用户自己整理的文件夹
        assert!(!is_image_like_asset(Path::new("D:/书库/标题.txt")));
        assert!(!is_image_like_asset(Path::new("D:/书库/标题.epub")));
        assert!(!is_image_like_asset(Path::new("D:/书库/标题.html")));
        assert!(!is_image_like_asset(Path::new("D:/书库/我的作品集")));
    }

    #[test]
    fn already_linked_files_are_left_alone() {
        // 路径键：分隔符、大小写、Windows 长路径前缀都不该影响「是不是同一个文件」
        assert_eq!(path_key(r"\\?\D:\书库\希儿.txt"), path_key("d:/书库/希儿.TXT"));
        assert_ne!(path_key("D:/书库/希儿.txt"), path_key("D:/书库/希儿2.txt"));

        // 没绑过、绑过但文件已经不在的作品，都可以被关联新文件
        assert!(needs_a_purchased_file(""));
        assert!(needs_a_purchased_file("   "));
        assert!(needs_a_purchased_file("D:/不存在的书库/幻想作品.txt"));

        // 绑好了、文件还在的作品不再进候选名单（同步时下下来就已经绑好了）
        let root = std::env::temp_dir().join(format!("purchased-skip-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let existing = root.join("已关联的书.txt");
        fs::write(&existing, "正文").unwrap();
        assert!(!needs_a_purchased_file(&existing.to_string_lossy()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn already_bound_purchased_files_are_not_offered_again() {
        // 目录里：一个早就绑好的、一个待关联的、一张封面、一个配图文件夹
        let root = std::env::temp_dir().join(format!("purchased-pending-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("已关联的书.txt"), "正文").unwrap();
        fs::write(root.join("还没关联的书.txt"), "正文").unwrap();
        fs::write(root.join("封面.jpg"), "x").unwrap();
        fs::create_dir_all(root.join("带图版的书_images")).unwrap();
        let bound: std::collections::HashSet<String> = ["已关联的书.txt"]
            .iter()
            .map(|name| path_key(&root.join(name).to_string_lossy()))
            .collect();

        let (pending, skipped) = collect_pending_purchased_files(&root, &bound).unwrap();
        let names: Vec<String> = pending.iter().map(|path| file_name(path)).collect();
        // 已绑定的、封面、配图文件夹都不在待关联列表里
        assert_eq!(names, vec!["还没关联的书.txt"]);
        assert_eq!(skipped, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn html_image_count_counts_only_real_illustrations() {
        let root = std::env::temp_dir().join(format!("reading-html-count-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let html = root.join("书.html");
        fs::write(
            &html,
            "<img class=\"cover\" src=\"书.jpg\"><figure><img src=\"书_images/001.jpg\"></figure><figure><img src=\"书_images/002.png\"></figure>",
        )
        .unwrap();
        // 封面按作品封面文件名排除
        assert_eq!(count_reading_images(&html, "书.jpg"), Some(2));
        // 库里没有封面路径时，`class="cover"` 兜底
        assert_eq!(count_reading_images(&html, ""), Some(2));
        // 正文 txt 数不出图片，这时的角标保持原样
        let txt = root.join("书.txt");
        fs::write(&txt, "正文").unwrap();
        assert_eq!(count_reading_images(&txt, "书.jpg"), None);
        assert_eq!(count_reading_images(&root.join("没有这个.html"), ""), None);
    }

    #[test]
    fn epub_image_count_reads_the_zip_directory_and_skips_the_cover() {
        let root = std::env::temp_dir().join(format!("reading-epub-count-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let epub = root.join("书.epub");
        let mut archive = Vec::new();
        let mut items = Vec::new();
        zip_push(&mut archive, &mut items, "mimetype", b"application/epub+zip");
        zip_push(&mut archive, &mut items, "OEBPS/images/书.jpg", b"cover");
        zip_push(&mut archive, &mut items, "OEBPS/images/001.jpg", b"a");
        zip_push(&mut archive, &mut items, "OEBPS/images/002.png", b"b");
        zip_push(&mut archive, &mut items, "OEBPS/text/novel.xhtml", b"<html/>");
        zip_finish(&mut archive, &mut items);
        fs::write(&epub, &archive).unwrap();

        assert_eq!(count_reading_images(&epub, "书.jpg"), Some(2));
        // 没给封面文件名时只能把封面也算进去（封面名认不出来，不猜）
        assert_eq!(count_reading_images(&epub, ""), Some(3));
    }

    #[test]
    fn binding_a_reading_file_records_its_image_count() {
        let root = std::env::temp_dir().join(format!("reading-count-db-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let html = root.join("书.html");
        fs::write(
            &html,
            "<img class=\"cover\" src=\"书.jpg\"><figure><img src=\"书_images/001.jpg\"></figure>",
        )
        .unwrap();
        let cover = root.join("书.jpg");
        fs::write(&cover, b"jpg").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 preview_path TEXT NOT NULL DEFAULT '',
                 purchased_path TEXT NOT NULL DEFAULT '',
                 cover_path TEXT NOT NULL DEFAULT '',
                 image_count INTEGER NOT NULL DEFAULT 0,
                 has_images INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        let html_path = html.to_string_lossy().to_string();
        let cover_path = cover.to_string_lossy().to_string();
        conn.execute(
            "INSERT INTO works (preview_path, cover_path) VALUES (?1, ?2)",
            [html_path.as_str(), cover_path.as_str()],
        )
        .unwrap();

        refresh_reading_image_count(&conn, 1);
        let count: i64 = conn
            .query_row("SELECT image_count FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        // 已经设过的带图版标记不因为这个功能被动过
        let has_images: i64 = conn
            .query_row("SELECT has_images FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(has_images, 0);

        // 绑定换回普通 txt：数不出来了，就保留原来记录的值
        conn.execute("UPDATE works SET preview_path='D:/书库/书.txt'", [])
            .unwrap();
        refresh_reading_image_count(&conn, 1);
        let count: i64 = conn
            .query_row("SELECT image_count FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn epub_binding_keeps_the_preview_or_full_flag() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 preview_path TEXT NOT NULL DEFAULT '',
                 purchased_path TEXT NOT NULL DEFAULT ''
             );
             INSERT INTO works (preview_path, purchased_path) VALUES ('D:/预览版/书.txt', '');
             INSERT INTO works (preview_path, purchased_path) VALUES ('', 'D:/已购版/书.txt');",
        )
        .unwrap();

        // 预览版作品：只换预览版那一侧的绑定，属性不变
        bind_work_to_reading(&conn, 1, "D:/预览版/书.epub").unwrap();
        let (preview, purchased): (String, String) = conn
            .query_row(
                "SELECT preview_path, purchased_path FROM works WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preview, "D:/预览版/书.epub");
        assert!(purchased.is_empty());

        // 完整版作品：换的是完整版那一侧，预览版那侧不会被顺手填上
        bind_work_to_reading(&conn, 2, "D:/已购版/书.epub").unwrap();
        let (preview, purchased): (String, String) = conn
            .query_row(
                "SELECT preview_path, purchased_path FROM works WHERE id=2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(purchased, "D:/已购版/书.epub");
        assert!(preview.is_empty());
    }

    #[test]
    fn already_bound_readings_are_skipped_only_when_the_file_is_really_there() {
        let root = std::env::temp_dir().join(format!("reading-bound-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let epub = root.join("书.epub");
        fs::write(&epub, b"epub").unwrap();
        let epub_path = epub.to_string_lossy().to_string();

        // 目标格式一致、文件在 → 跳过
        assert!(reading_already_bound(&epub_path, ReadingFormat::Epub));
        // 格式对不上不算已绑定：绑在 EPUB 上、这次要 HTML，还得重做
        assert!(!reading_already_bound(&epub_path, ReadingFormat::Html));
        // 文件没了也不算，免得跳过一个已经失效的绑定
        assert!(!reading_already_bound(
            &root.join("不存在.epub").to_string_lossy(),
            ReadingFormat::Epub
        ));
        assert!(!reading_already_bound("", ReadingFormat::Html));
        // 还绑在正文 txt 上：两种格式都要处理
        let txt = root.join("书.txt");
        fs::write(&txt, b"text").unwrap();
        let txt_path = txt.to_string_lossy().to_string();
        assert!(!reading_already_bound(&txt_path, ReadingFormat::Html));
        assert!(!reading_already_bound(&txt_path, ReadingFormat::Epub));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_original_txt_is_found_next_to_a_reading_version() {
        let suffix = std::process::id();
        let root = std::env::temp_dir().join(format!("txt-beside-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let epub = root.join("书.epub");
        fs::write(&epub, b"epub").unwrap();

        // 还没有同名 txt → None，调用方据此跳过「改写占位符」这一步
        assert_eq!(novel_text_path_beside(&epub), None);

        // txt 出现之后就能找到。改写只能写这个 txt，绝不能覆盖 .epub 本体
        let txt = root.join("书.txt");
        fs::write(&txt, b"text").unwrap();
        assert_eq!(novel_text_path_beside(&epub), Some(txt));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn author_homepage_keeps_only_the_user_id() {
        assert_eq!(
            normalize_author_homepage("https://www.pixiv.net/users/16208053/novels"),
            "https://www.pixiv.net/users/16208053"
        );
        assert_eq!(
            normalize_author_homepage("  https://www.pixiv.net/users/16208053/ "),
            "https://www.pixiv.net/users/16208053"
        );
        assert_eq!(
            normalize_author_homepage("https://www.pixiv.net/en/users/16208053/artworks"),
            "https://www.pixiv.net/en/users/16208053"
        );
        assert_eq!(
            normalize_author_homepage("https://www.pixiv.net/member.php?id=16208053"),
            "https://www.pixiv.net/users/16208053"
        );
        assert_eq!(
            normalize_author_homepage("https://www.pixiv.net/users/16208053"),
            "https://www.pixiv.net/users/16208053"
        );
        assert_eq!(normalize_author_homepage("   "), "");
        // 认不出的链接原样保留，不要瞎猜
        assert_eq!(
            normalize_author_homepage("https://example.com/authors/abc"),
            "https://example.com/authors/abc"
        );
    }

    #[test]
    fn aliases_are_trimmed_and_deduped() {
        assert_eq!(normalize_aliases(" Mori | mori |远野|  |"), "Mori|远野");
        assert_eq!(normalize_aliases(""), "");
        assert_eq!(normalize_aliases("   "), "");
    }

    #[test]
    fn recursive_scan_finds_files_in_sub_directories() {
        let root = std::env::temp_dir().join(format!("recursive-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("子目录A").join("更深一层")).unwrap();
        fs::write(root.join("顶层.txt"), b"top").unwrap();
        fs::write(root.join("子目录A").join("中层.zip"), b"mid").unwrap();
        fs::write(root.join("子目录A").join("更深一层").join("深层.txt"), b"deep").unwrap();

        let mut files = vec![];
        collect_files_recursively(&root, &mut files).unwrap();
        let names: Vec<String> = files.iter().map(|path| file_name(path)).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"顶层.txt".to_string()));
        assert!(names.contains(&"中层.zip".to_string()));
        assert!(names.contains(&"深层.txt".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn title_length_limit_clips_only_when_configured() {
        // 不限制时原样返回（含长度 0 与负数）
        assert_eq!(clip_match_title("斗破苍穹七到十章完结", 0), "斗破苍穹七到十章完结");
        assert_eq!(clip_match_title("斗破苍穹七到十章完结", -5), "斗破苍穹七到十章完结");
        // 超过限制只保留前面若干个字（按字符计，不是字节）
        assert_eq!(clip_match_title("斗破苍穹七到十章完结", 4), "斗破苍穹");
        assert_eq!(clip_match_title("abcdefgh", 3), "abc");
        // 截断后残留的空格被去掉
        assert_eq!(clip_match_title("斗破苍穹 后续章节", 5), "斗破苍穹");
    }

    #[test]
    fn title_length_limit_raises_similarity_when_the_title_is_the_longer_side() {
        // 作品名比文件名长：分母跟着作品名走，截掉作品名尾部那些对不上的字就能把分数顶上去
        let title = "斗破苍穹最终决战特别篇"; // 11 个字
        let file_name = "斗破苍穹最终决战.txt"; // 8 个字
        let unlimited = similarity_with_title_limit(title, &file_name, 0);
        let limited = similarity_with_title_limit(title, &file_name, 8);
        assert_eq!(unlimited, 72, "完整作品名比对：共同前缀 8 / 作品名 11 个字");
        assert_eq!(limited, 100, "只取前 8 个字：共同前缀 8 / 两边都是 8 个字");
        assert!(limited > unlimited);
    }

    #[test]
    fn title_length_limit_stops_helping_when_the_file_name_is_longer() {
        // v0.3.71 起分母取**较长一方**：文件名比作品名长时分母恒等于文件名长度，
        // 截作品名只会让分子越来越小，分数不变 ——「匹配作品名长度」在长文件名面前不再起作用
        let title = "斗破苍穹最终决战特别篇"; // 11 个字
        let file_name = "斗破苍穹最终决战番外合集第二部.txt"; // 15 个字
        assert_eq!(similarity_with_title_limit(title, &file_name, 0), 53);
        assert_eq!(similarity_with_title_limit(title, &file_name, 8), 53);
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
    fn sync_allows_an_open_ended_date_range() {
        let start = NaiveDate::from_ymd_opt(2025, 1, 1);
        let end = NaiveDate::from_ymd_opt(2025, 1, 31);
        assert!(!has_invalid_date_range(start, None));
        assert!(!has_invalid_date_range(None, end));
        assert!(has_invalid_date_range(end, start));
    }

    #[test]
    fn sync_reuses_a_unique_matching_preview_file() {
        let entries = vec![
            SyncPreviewEntry {
                path: PathBuf::from("D:/preview/2025-10-05 希儿与布洛妮娅.txt"),
                name: "2025-10-05 希儿与布洛妮娅".into(),
                is_preview: true,
            },
            SyncPreviewEntry {
                path: PathBuf::from("D:/preview/2025-10-05 希儿与布洛妮娅.jpg"),
                name: "2025-10-05 希儿与布洛妮娅".into(),
                is_preview: false,
            },
        ];
        let (preview, cover) = matched_sync_preview(&entries, "希儿与布洛妮娅", 70, 0).unwrap();
        assert_eq!(
            preview,
            PathBuf::from("D:/preview/2025-10-05 希儿与布洛妮娅.txt")
        );
        assert_eq!(
            cover,
            Some(PathBuf::from("D:/preview/2025-10-05 希儿与布洛妮娅.jpg"))
        );
    }

    #[test]
    fn bind_work_with_rename_generates_correct_filename() {
        // 测试文件名生成逻辑
        let title = "测试作品标题";
        let extension = "txt";
        let expected = "测试作品标题.txt";
        let result = if extension.is_empty() {
            title.to_string()
        } else {
            format!("{title}.{extension}")
        };
        assert_eq!(result, expected);

        // 测试无扩展名的情况
        let title = "无扩展名文件";
        let extension = "";
        let expected = "无扩展名文件";
        let result = if extension.is_empty() {
            title.to_string()
        } else {
            format!("{title}.{extension}")
        };
        assert_eq!(result, expected);
    }

    #[test]
    fn move_path_renames_a_file_and_ignores_a_missing_source() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("collection-library-test-{suffix}"));
        let preview = root.join("preview");
        let full = root.join("full");
        fs::create_dir_all(&preview).unwrap();
        fs::create_dir_all(&full).unwrap();
        let source = preview.join("novel.txt");
        fs::write(&source, "第一章 文字").unwrap();

        move_path(&source, &full.join("novel.txt")).unwrap();
        // 单份模型：源文件必须从预览版目录消失
        assert!(!source.exists());
        assert_eq!(text_word_count(&full.join("novel.txt").to_string_lossy()), Some(5));

        // 不存在的源（例如作品没有配图文件夹）不该报错
        move_path(&preview.join("novel_images"), &full.join("novel_images")).unwrap();
        assert!(move_path(&preview.join("gone.txt"), &full.join("gone.txt")).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_assets_are_kept_out_of_work_matching() {
        assert!(is_generated_asset(Path::new(
            "D:/preview/2025-10-05 希儿_images"
        )));
        assert!(is_generated_asset(Path::new("D:/preview/2025-10-05 希儿.html")));
        assert!(is_generated_asset(Path::new("D:/preview/2025-10-05 希儿.epub")));
        assert!(!is_generated_asset(Path::new("D:/preview/2025-10-05 希儿.txt")));
        assert!(!is_generated_asset(Path::new("D:/preview/2025-10-05 希儿.jpg")));
        assert!(!is_generated_asset(Path::new("D:/preview/番外 合集")));
    }

    #[test]
    fn novel_image_refs_follow_text_order_and_drop_duplicates() {
        let content =
            "开头[uploadedimage:20454900]中间[pixivimage:998877]结尾[uploadedimage:20454900]";
        let refs = novel_image_refs(content);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].token, "[uploadedimage:20454900]");
        assert!(!refs[0].external);
        assert_eq!(refs[1].id, "998877");
        assert!(refs[1].external);
        assert!(novel_image_refs("没有插图的正文").is_empty());
    }

    #[test]
    fn novel_image_url_prefers_the_configured_quality() {
        let embedded = json!({
            "20454900": { "urls": {
                "240mw": "https://i.pximg.net/c/240x480_80/novel-cover-master/img/a_master1200.jpg",
                "1200x1200": "https://i.pximg.net/c/1200x1200/novel-cover-master/img/a_master1200.jpg",
                "original": "https://i.pximg.net/novel-cover-original/img/a.png"
            }}
        });
        let wide = novel_image_url(&embedded, "20454900", "1200").unwrap();
        assert_eq!(novel_image_filename_extension(&wide), "jpg");
        let original = novel_image_url(&embedded, "20454900", "original").unwrap();
        assert_eq!(novel_image_filename_extension(&original), "png");
        // 作者只传了一档、或者 ID 对不上时要能安全退化
        let only_small = json!({ "1": { "urls": { "240mw": "https://i.pximg.net/x_master1200.jpg" } } });
        assert!(novel_image_url(&only_small, "1", "original").is_some());
        assert!(novel_image_url(&embedded, "999", "1200").is_none());
        assert_eq!(
            novel_images_dir(Path::new("D:/preview/标题-26410188.txt")),
            PathBuf::from("D:/preview/标题-26410188_images")
        );
    }

    #[test]
    fn txt_rewrite_points_at_the_sibling_image_folder() {        let content = "正文[uploadedimage:20454900]后续";
        let slots = vec![NovelImageSlot {
            token: "[uploadedimage:20454900]".into(),
            order: 1,
            file_name: "001.jpg".into(),
            relative: "标题_images/001.jpg".into(),
            url: "https://i.pximg.net/x.jpg".into(),
            external: false,
        }];
        let text = rewrite_novel_txt(content, &slots);
        assert!(text.contains("[插图 1：标题_images/001.jpg]"));
        assert!(!text.contains("uploadedimage"));
    }

    #[test]
    fn html_render_handles_images_ruby_and_chapters() {
        let content = "[chapter:第一章][[rb:汉字 > 读音]]正文\n[uploadedimage:20454900][newpage]尾巴";
        let slots = vec![NovelImageSlot {
            token: "[uploadedimage:20454900]".into(),
            order: 1,
            file_name: "001.jpg".into(),
            relative: "标题_images/001.jpg".into(),
            url: "https://i.pximg.net/x.jpg".into(),
            external: false,
        }];
        let html = render_novel_html(
            content,
            &slots,
            &NovelHtmlMeta {
                title: "标题",
                author_name: "作者",
                release_date: "2025-03-20",
                tags: "Pixiv|图文",
                cover_file: "标题.jpg",
                image_count: 1,
            },
        );
        assert!(html.contains("<h2>第一章</h2>"));
        assert!(html.contains("<ruby>汉字<rt>读音</rt></ruby>"));
        assert!(html.contains("src=\"标题_images/001.jpg\""));
        assert!(html.contains("class=\"page\""));
        assert!(html.contains("配图 1 张"));
        assert!(!html.contains("uploadedimage"));
        assert!(!html.contains("[newpage]"));
    }

    #[test]
    fn cover_path_falls_back_to_the_sibling_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cover-fallback-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let text = root.join("标题-123.txt");
        fs::write(&text, "正文").unwrap();
        let text_str = text.to_string_lossy().to_string();
        let cover = root.join("标题-123.jpg");
        let cover_str = cover.to_string_lossy().to_string();
        let dead = "D:/不存在的目录/标题-123.jpg";

        // 路径还有效时原样返回；本身为空时也返回空
        fs::write(&cover, [1_u8, 2, 3]).unwrap();
        assert_eq!(resolve_cover_path(&cover_str, "", &text_str), cover_str);
        assert_eq!(resolve_cover_path("", &text_str, ""), "");

        // 完整版缺失时回退到预览版那一侧的封面
        let purchased = root.join("已购-123.txt");
        fs::write(&purchased, "正文").unwrap();
        assert_eq!(
            resolve_cover_path(dead, &text_str, &purchased.to_string_lossy()),
            cover_str
        );

        // 两边都没有同名封面就返回空串，让界面显示占位图
        let other = root.join("别的作品-456.txt");
        fs::write(&other, "正文").unwrap();
        assert_eq!(resolve_cover_path(dead, &other.to_string_lossy(), ""), "");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cover_follows_the_text_into_the_purchased_dir() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cover-follow-{suffix}"));
        let preview = root.join("预览版");
        let purchased = root.join("已购版");
        fs::create_dir_all(&preview).unwrap();
        fs::create_dir_all(&purchased).unwrap();
        let old_text = preview.join("标题-123.txt");
        let old_cover = preview.join("标题-123.jpg");
        fs::write(&old_text, "正文").unwrap();
        fs::write(&old_cover, [1_u8, 2, 3]).unwrap();
        let old_cover_str = old_cover.to_string_lossy().to_string();

        // 模拟 copy_previews_to_purchased 的搬家：正文和同名封面一起走
        let new_text = purchased.join("标题-123.txt");
        move_path(&old_text, &new_text).unwrap();
        move_cover_along(&old_cover_str, &purchased).unwrap();

        // 库里那条 cover_path 必须跟着正文走，否则老目录一被清理封面就集体挂掉
        assert_eq!(
            follow_cover_path(&old_cover_str, &new_text),
            purchased
                .join("标题-123.jpg")
                .to_string_lossy()
                .to_string()
        );
        assert!(!old_cover.exists());
        assert!(new_text.is_file());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cover_with_a_custom_name_still_follows_the_text() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cover-custom-{suffix}"));
        let preview = root.join("预览版");
        let purchased = root.join("已购版");
        fs::create_dir_all(&preview).unwrap();
        fs::create_dir_all(&purchased).unwrap();
        let old_text = preview.join("标题-123.txt");
        // 封面不叫「与正文同名」，sibling_assets 认不出来，得靠 move_cover_along 单独搬
        let old_cover = preview.join("作者上传的封面.png");
        fs::write(&old_text, "正文").unwrap();
        fs::write(&old_cover, [1_u8, 2, 3]).unwrap();
        let old_cover_str = old_cover.to_string_lossy().to_string();

        let new_text = purchased.join("标题-123.txt");
        move_path(&old_text, &new_text).unwrap();
        move_cover_along(&old_cover_str, &purchased).unwrap();

        assert_eq!(
            follow_cover_path(&old_cover_str, &new_text),
            purchased
                .join("作者上传的封面.png")
                .to_string_lossy()
                .to_string()
        );
        assert!(!old_cover.exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_cover_keeps_whatever_the_db_had() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cover-missing-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let text = root.join("没封面-456.txt");
        fs::write(&text, "正文").unwrap();
        // 新位置找不到封面时保持库里的值不变，免得把用户手动指定的封面抹掉
        assert_eq!(
            follow_cover_path("D:/别处/手动指定的封面.jpg", &text),
            "D:/别处/手动指定的封面.jpg"
        );
        assert_eq!(follow_cover_path("", &text), "");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reading_format_defaults_to_html_and_epub_goes_to_the_output_dir() {
        assert_eq!(reading_format_of("html"), ReadingFormat::Html);
        assert_eq!(reading_format_of("EPUB"), ReadingFormat::Epub);
        assert_eq!(reading_format_of(" epub "), ReadingFormat::Epub);
        // 认不出来的值一律当 html，避免设置被写脏后没有阅读版可用
        assert_eq!(reading_format_of(""), ReadingFormat::Html);
        assert_eq!(reading_format_of("pdf"), ReadingFormat::Html);
        assert_eq!(ReadingFormat::Html.other(), ReadingFormat::Epub);
        assert_eq!(ReadingFormat::Epub.other(), ReadingFormat::Html);

        let text = Path::new("D:/已购/标题-26410188.txt");
        assert_eq!(
            reading_output_path(text, ReadingFormat::Html, ""),
            PathBuf::from("D:/已购/标题-26410188.html")
        );
        assert_eq!(
            reading_output_path(text, ReadingFormat::Epub, ""),
            PathBuf::from("D:/已购/标题-26410188.epub")
        );
        assert_eq!(
            reading_output_path(text, ReadingFormat::Epub, "D:/导出"),
            PathBuf::from("D:/导出/标题-26410188.epub")
        );
    }

    #[test]
    fn reading_output_writes_html_or_epub_next_to_the_text() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("reading-output-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let text_path = root.join("标题-26410188.txt");
        fs::write(&text_path, "正文").unwrap();
        let images_dir = root.join("标题-26410188_images");
        fs::create_dir_all(&images_dir).unwrap();
        fs::write(
            images_dir.join("001.jpg"),
            [0xFF_u8, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4],
        )
        .unwrap();
        let slots = vec![NovelImageSlot {
            token: "[uploadedimage:20454900]".into(),
            order: 1,
            file_name: "001.jpg".into(),
            relative: "标题-26410188_images/001.jpg".into(),
            url: "https://i.pximg.net/x.jpg".into(),
            external: false,
        }];
        let meta = ReadingWriteMeta {
            novel_id: "26410188",
            title: "标题",
            author_name: "作者",
            release_date: "2025-03-20",
            tags: "Pixiv|图文",
            image_count: 1,
        };
        let content = "[[rb:汉字 > 读音]]正文[uploadedimage:20454900]尾巴";

        let html = write_reading_output(
            &text_path,
            ReadingFormat::Html,
            "",
            content,
            &slots,
            &meta,
        )
        .unwrap();
        assert_eq!(html, root.join("标题-26410188.html"));
        let html_text = fs::read_to_string(&html).unwrap();
        assert!(html_text.contains("<ruby>汉字<rt>读音</rt></ruby>"));
        assert!(html_text.contains("标题-26410188_images/001.jpg"));

        let epub = write_reading_output(
            &text_path,
            ReadingFormat::Epub,
            "",
            content,
            &slots,
            &meta,
        )
        .unwrap();
        assert_eq!(epub, root.join("标题-26410188.epub"));
        let bytes = fs::read(&epub).unwrap();
        assert_eq!(&bytes[..2], b"PK", "EPUB 应该是个 zip");
        assert!(
            bytes.windows(7).any(|window| window == b"001.jpg"),
            "配图应该打进包里"
        );
        assert!(String::from_utf8_lossy(&bytes).contains("汉字"));

        // 设置里指定了输出目录时，EPUB 落到那个目录
        let out_dir = root.join("导出");
        let moved = write_reading_output(
            &text_path,
            ReadingFormat::Epub,
            &out_dir.to_string_lossy(),
            content,
            &slots,
            &meta,
        )
        .unwrap();
        assert_eq!(moved, out_dir.join("标题-26410188.epub"));
        assert!(moved.is_file());
        fs::remove_dir_all(&root).ok();
    }

    /// 「字数从多到少」（v0.3.67）：EPUB / HTML 按配图张数排、整体排在 TXT 前面；TXT 之间按字数排。
    #[test]
    fn content_size_sort_puts_ebooks_first_and_uses_words_for_txt() {
        let mut works = vec![
            content_size_work(1, Some(3000), None, 0),
            content_size_work(2, None, Some("EPUB"), 2),
            content_size_work(3, Some(9000), None, 0),
            content_size_work(4, None, Some("HTML"), 7),
        ];

        sort_works_by_content_size(&mut works);

        let order = works.iter().map(|work| work.id).collect::<Vec<_>>();
        assert_eq!(order, vec![4, 2, 3, 1]);
    }

    fn content_size_work(
        id: i64,
        word_count: Option<usize>,
        file_format: Option<&str>,
        image_count: i64,
    ) -> Work {
        Work {
            author_id: 1,
            id,
            title: format!("作品 {id}"),
            release_date: String::new(),
            preview_path: String::new(),
            cover_path: String::new(),
            purchased_path: String::new(),
            favorite: false,
            has_images: false,
            image_count,
            tags: String::new(),
            pixiv_novel_id: String::new(),
            series_id: String::new(),
            series_title: String::new(),
            series_order: 0,
            is_new: false,
            author_name: String::new(),
            word_count,
            file_format: file_format.map(str::to_string),
        }
    }

    #[test]
    fn purchased_non_text_file_shows_its_format_instead_of_a_word_count() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("collection-library-test-{suffix}.epub"));
        fs::write(&path, [1_u8, 2, 3]).unwrap();
        let mut work = Work {
            author_id: 1,
            id: 1,
            title: String::new(),
            release_date: String::new(),
            preview_path: String::new(),
            cover_path: String::new(),
            purchased_path: path.to_string_lossy().to_string(),
            favorite: false,
            has_images: false,
            image_count: 0,
            tags: String::new(),
            pixiv_novel_id: String::new(),
            series_id: String::new(),
            series_title: String::new(),
            series_order: 0,
            is_new: false,
            author_name: String::new(),
            word_count: None,
            file_format: None,
        };

        populate_work_display_info(&mut work);

        assert_eq!(work.word_count, None);
        assert_eq!(work.file_format.as_deref(), Some("EPUB"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn single_sync_accepts_only_a_pixiv_novel_url() {
        assert_eq!(
            pixiv_novel_id_from_url("https://www.pixiv.net/novel/show.php?id=28563270").unwrap(),
            Some("28563270".into())
        );
        assert_eq!(
            pixiv_novel_id_from_url("https://www.pixiv.net/novel/show.php?foo=bar&id=42#part")
                .unwrap(),
            Some("42".into())
        );
        assert!(pixiv_novel_id_from_url("https://www.pixiv.net/novel/show.php?id=abc").is_err());
        assert!(pixiv_novel_id_from_url("https://www.pixiv.net/users/123").is_err());
    }

    #[test]
    fn sync_uses_only_exact_file_names_after_pixiv_id_check() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (
                id INTEGER PRIMARY KEY,
                author_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                pixiv_novel_id TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO works (id, author_id, title, pixiv_novel_id)
            VALUES (1, 1, 'shared title', '27713149');",
        )
        .unwrap();

        assert_eq!(
            existing_sync_target(&conn, 1, "28748734", "shared title part two",).unwrap(),
            None
        );
        assert_eq!(
            existing_sync_target(&conn, 1, "28748734", "shared title.txt").unwrap(),
            Some(1)
        );
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

    #[test]
    fn synopsis_marks_previews_except_full_release_phrase() {
        assert!(synopsis_indicates_preview("这里是全文的前半部分"));
        assert!(synopsis_indicates_preview("全文 \n 将在其他平台发布"));
        assert!(!synopsis_indicates_preview("全文放出，感谢支持"));
        assert!(!synopsis_indicates_preview("完整内容已经发布"));
        // 简介里出现「购买」就是预览版（夹了空格 / 换行也算；「购买途径」「购买渠道」都命中）
        assert!(synopsis_indicates_preview("购买途径：某站"));
        assert!(synopsis_indicates_preview("购买 途径 \n ：某站有完整版"));
        assert!(synopsis_indicates_preview("完整版可在某站购买"));
        assert!(!synopsis_indicates_preview("本作已完结，感谢阅读"));
    }

    fn sample_novel_slots() -> Vec<NovelImageSlot> {
        vec![
            NovelImageSlot {
                token: "[uploadedimage:20454750]".into(),
                order: 1,
                file_name: "001.jpg".into(),
                relative: "标题_images/001.jpg".into(),
                url: "https://i.pximg.net/novel/001.jpg".into(),
                external: false,
            },
            NovelImageSlot {
                token: "[pixivimage:1234]".into(),
                order: 2,
                file_name: "002.png".into(),
                relative: "标题_images/002.png".into(),
                url: "https://i.pximg.net/illust/002.png".into(),
                external: true,
            },
        ]
    }

    /// 从中央目录读出 zip 条目名，用来验证 EPUB 结构。
    fn zip_entry_names(archive: &[u8]) -> Vec<String> {
        let signature = 0x0605_4b50_u32.to_le_bytes();
        let eocd = archive
            .windows(4)
            .rposition(|window| window == signature)
            .expect("没找到 zip 结尾记录");
        let count = u16::from_le_bytes([archive[eocd + 10], archive[eocd + 11]]) as usize;
        let start = u32::from_le_bytes([
            archive[eocd + 16],
            archive[eocd + 17],
            archive[eocd + 18],
            archive[eocd + 19],
        ]) as usize;
        let mut cursor = start;
        let mut names = Vec::new();
        for _ in 0..count {
            let name_len = u16::from_le_bytes([archive[cursor + 28], archive[cursor + 29]]) as usize;
            let extra_len = u16::from_le_bytes([archive[cursor + 30], archive[cursor + 31]]) as usize;
            let comment_len =
                u16::from_le_bytes([archive[cursor + 32], archive[cursor + 33]]) as usize;
            names.push(
                String::from_utf8_lossy(&archive[cursor + 46..cursor + 46 + name_len]).to_string(),
            );
            cursor += 46 + name_len + extra_len + comment_len;
        }
        names
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(zip_crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(zip_crc32(b""), 0);
    }

    #[test]
    fn epub_packs_mimetype_first_and_keeps_every_asset() {
        let slots = sample_novel_slots();
        let images = vec![
            ("001.jpg".to_string(), vec![1_u8, 2, 3]),
            ("002.png".to_string(), vec![9_u8, 8, 7, 6]),
        ];
        let epub = build_novel_epub(
            "测试标题",
            "测试作者",
            "2025-11-08",
            "24319453",
            "插画|图文",
            "第一段[[rb:汉字>かんじ]]\n[uploadedimage:20454750]\n[newpage]\n[pixivimage:1234]",
            &slots,
            &images,
            Some(("cover.jpg", &[4_u8, 5, 6])),
        );
        let names = zip_entry_names(&epub);
        assert_eq!(names.first().map(String::as_str), Some("mimetype"));
        for expected in [
            "META-INF/container.xml",
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/toc.ncx",
            "OEBPS/style.css",
            "OEBPS/text/cover.xhtml",
            "OEBPS/text/novel.xhtml",
            "OEBPS/images/cover.jpg",
            "OEBPS/images/001.jpg",
            "OEBPS/images/002.png",
        ] {
            assert!(names.iter().any(|name| name == expected), "缺少 {expected}");
        }
        // mimetype 必须是第一个条目且直存，否则阅读器会判定整个包无效
        assert_eq!(u16::from_le_bytes([epub[8], epub[9]]), 0);
        let raw = String::from_utf8_lossy(&epub);
        assert!(raw.contains("pixiv-novel-24319453"));
        assert!(raw.contains("../images/001.jpg"));
        assert!(raw.contains("<ruby>汉字<rt>かんじ</rt></ruby>"));
    }

    #[test]
    fn text_around_a_marker_survives_segmentation() {
        let slots = sample_novel_slots();
        let body = render_novel_xhtml(
            "第一段\n[uploadedimage:20454750]\n第二段\n[pixivimage:1234]\n第三段",
            &slots,
        );
        assert!(body.contains("<p>第一段</p>"), "实际输出：{body}");
        assert!(body.contains("<p>第二段</p>"), "实际输出：{body}");
        assert!(body.contains("<p>第三段</p>"), "实际输出：{body}");
        let html = render_novel_html(
            "开头文字[uploadedimage:20454750]结尾文字",
            &slots,
            &NovelHtmlMeta {
                title: "标题",
                author_name: "作者",
                release_date: "",
                tags: "",
                cover_file: "",
                image_count: 1,
            },
        );
        assert!(html.contains("开头文字"), "实际输出：{html}");
        assert!(html.contains("结尾文字"), "实际输出：{html}");
    }

    #[test]
    fn xhtml_renderer_closes_empty_tags_and_points_images_inside_the_package() {
        let slots = sample_novel_slots();
        let body = render_novel_xhtml(
            "段落一\n[uploadedimage:20454750]\n[newpage]\n[pixivimage:1234]",
            &slots,
        );
        assert!(body.contains("<p>段落一</p>"), "实际输出：{body}");
        assert!(body.contains(r#"src="../images/001.jpg""#));
        assert!(body.contains(r#"src="../images/002.png""#));
        assert!(body.contains(r#"<hr class="page"/>"#));
        assert!(!body.contains("[uploadedimage:20454750]"));
        assert!(!body.contains("[pixivimage:1234]"));
    }

    #[test]
    fn epub_target_follows_the_work_or_the_configured_output_dir() {
        let text = Path::new(r"D:\书\我的小说.txt");
        assert_eq!(
            epub_target_path(text, "").to_string_lossy(),
            r"D:\书\我的小说.epub"
        );
        assert_eq!(
            epub_target_path(text, r"E:\导出").to_string_lossy(),
            r"E:\导出\我的小说.epub"
        );
    }

    #[test]
    fn search_url_replaces_whatever_sits_after_the_last_equals() {
        let template = "https://sxsy45.com/search.php?mod=forum&searchid=83552&orderby=dateline&ascdesc=desc&searchsubmit=yes&kw=%E5%9B%BE";
        // 站点自带的「图」字被清掉，换成搜索词（这里同样编码回 %E5%9B%BE）
        assert_eq!(build_search_url(template, "图"), template);
        assert_eq!(
            build_search_url(template, "希儿"),
            "https://sxsy45.com/search.php?mod=forum&searchid=83552&orderby=dateline&ascdesc=desc&searchsubmit=yes&kw=%E5%B8%8C%E5%84%BF"
        );
        // 搜索词前后的空格不算内容
        assert_eq!(
            build_search_url(template, "  芙宁娜  "),
            "https://sxsy45.com/search.php?mod=forum&searchid=83552&orderby=dateline&ascdesc=desc&searchsubmit=yes&kw=%E8%8A%99%E5%AE%81%E5%A8%9C"
        );
    }

    #[test]
    fn search_url_encodes_special_characters() {
        // 空格、&、/ 都要编码：否则搜索词会被拆成另一个参数
        assert_eq!(
            build_search_url("https://a.com/s?kw=", "a b&c/d"),
            "https://a.com/s?kw=a%20b%26c%2Fd"
        );
        // 不编码的只有这几个安全字符
        assert_eq!(
            build_search_url("https://a.com/s?kw=", "a-b_c.d~e"),
            "https://a.com/s?kw=a-b_c.d~e"
        );
        // 网址里一个 = 都没有时补一个，免得搜索词粘在别的参数尾巴上
        assert_eq!(
            build_search_url("https://a.com/search", "图"),
            "https://a.com/search=%E5%9B%BE"
        );
    }

    #[test]
    fn settings_carry_the_configured_search_sites() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        // 没配过（键不存在）时是空列表，右键菜单就不会多出搜索项
        assert!(read_settings(&conn).unwrap().search_sites.is_empty());
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('search_sites', ?1)",
            [r#"[{"name":"书香","url":"https://a.com/search.php?kw="}]"#],
        )
        .unwrap();
        let sites = read_settings(&conn).unwrap().search_sites;
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "书香");
        assert_eq!(sites[0].url, "https://a.com/search.php?kw=");
        // 存的文本被改坏时不能连带整份设置都读不出来，退化成「没配过」
        conn.execute(
            "UPDATE app_settings SET value='{坏' WHERE key='search_sites'",
            [],
        )
        .unwrap();
        assert!(read_settings(&conn).unwrap().search_sites.is_empty());
    }

    #[test]
    fn redundant_preview_needs_a_real_purchased_file() {
        // 完整版文件真在磁盘上，预览版才叫多余
        assert!(preview_is_redundant(true));
        // 完整版丢了就绝不能清预览版 —— 清了这篇作品等于从磁盘上抹掉
        assert!(!preview_is_redundant(false));
    }

    #[test]
    fn recycle_to_bin_is_a_noop_for_paths_that_are_already_gone() {
        // 路径不存在就直接当成功 —— 清理流程里「正文早就不在」的作品要靠这条不报错地走过去。
        // 真送回收站的动作不在这里测（会动到真盘上的 $Recycle.Bin）。
        assert!(recycle_to_bin(Path::new(r"E:\从来不存在的目录\nope.txt")).is_ok());
    }

    /// 真机验证「送的是回收站，不是直接删」——要动到真的 `$Recycle.Bin`，所以默认 `#[ignore]`，
    /// 只有在想确认这条机制时才手动跑：`cargo test --offline recycle_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn recycle_probe_really_lands_in_the_bin() {
        // 探针放在 D 盘（和用户小说同盘），回收站就是 D:\$Recycle.Bin
        let probe = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".workbuddy")
            .join("recycle-probe.txt");
        fs::write(&probe, "probe").unwrap();
        // 每删一个文件，$Recycle.Bin 里会多出 $I 元数据 + $R 内容两个文件
        let bin_root = Path::new(r"D:\$Recycle.Bin");
        let count = || -> usize {
            let mut total = 0;
            let Ok(sids) = fs::read_dir(bin_root) else {
                return 0;
            };
            for sid in sids.flatten() {
                if let Ok(entries) = fs::read_dir(sid.path()) {
                    total += entries.flatten().count();
                }
            }
            total
        };
        let before = count();
        recycle_to_bin(&probe).expect("送回收站失败");
        assert!(!probe.exists(), "文件应该已从原位置消失");
        let after = count();
        println!("PROBE before={before} after={after} path={}", probe.display());
        assert!(
            after > before,
            "$Recycle.Bin 里没有多出东西 —— 说明是直接删了，不是送回收站"
        );
    }

    #[test]
    fn preview_sibling_assets_cover_everything_cleanup_touches() {
        // 清理预览版时要一起处理的附属产物：配图目录、图文 HTML、EPUB、同名封面
        let text = Path::new(r"E:\预览版\作者\我的小说.txt");
        let names: Vec<String> = sibling_assets(text)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        for expected in ["我的小说_images", "我的小说.html", "我的小说.epub", "我的小说.jpg"] {
            assert!(names.contains(&expected.to_string()), "缺少 {expected}");
        }
        // 配图目录必须能被 is_generated_asset 认出来，否则会被当成「另一个版本」参与匹配
        assert!(is_generated_asset(Path::new(
            r"E:\预览版\作者\我的小说_images"
        )));
    }

    #[test]
    fn version_numbers_are_read_from_urls_and_text() {
        assert_eq!(parse_version("1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_version("v0.3.21"), Some((0, 3, 21)));
        assert_eq!(parse_version("PixivNovelDownloader-v12.34.5.exe"), Some((12, 34, 5)));
        // 日期里也有数字，但不能被当成版本号的三段
        assert_eq!(parse_version("2026-09-15"), None);
        assert_eq!(parse_version("没有数字"), None);
        // 只有两段的不算版本
        assert_eq!(parse_version("v1.0"), None);
    }

    #[test]
    fn latest_tag_is_read_from_the_release_redirect() {
        // 直连跟随后落在 tag 页上
        assert_eq!(
            version_from_tag_text("https://github.com/fromzero1501/pixiv-novel-downloader/releases/tag/v0.3.21"),
            Some("0.3.21".to_string())
        );
        // 镜像的地址里套着原始地址，也认
        assert_eq!(
            version_from_tag_text("https://ghproxy.net/https://github.com/fromzero1501/pixiv-novel-downloader/releases/tag/v1.0.1"),
            Some("1.0.1".to_string())
        );
        // 没有 tag 的地址（比如仓库一个 release 都没有）不算命中
        assert_eq!(
            version_from_tag_text("https://github.com/fromzero1501/pixiv-novel-downloader/releases"),
            None
        );
    }

    #[test]
    fn newer_version_is_the_one_that_triggers_an_update() {
        let current = parse_version("1.0.0").unwrap();
        assert!(parse_version("1.0.1").unwrap() > current);
        assert!(parse_version("1.1.0").unwrap() > current);
        assert!(parse_version("2.0.0").unwrap() > current);
        // 同一版（重发了同一个 tag）不算更新，老版本当然也不算
        assert!(!(parse_version("1.0.0").unwrap() > current));
        assert!(!(parse_version("0.3.99").unwrap() > current));
    }

    #[test]
    fn mirrors_are_joined_in_front_of_the_original_url() {
        let original = "https://github.com/fromzero1501/pixiv-novel-downloader/releases/latest";
        assert_eq!(
            join_mirror("https://ghproxy.net/", original),
            format!("https://ghproxy.net/{original}")
        );
        // 用户少打一个斜杠也得拼对
        assert_eq!(
            join_mirror("https://ghproxy.net", original),
            format!("https://ghproxy.net/{original}")
        );
        let urls = prefixed_urls(original, &default_update_mirrors());
        assert_eq!(urls.len(), 1 + default_update_mirrors().len(), "直连排第一，后面每个镜像一条");
        assert_eq!(urls[0], original, "直连必须排最前面");
        assert!(urls[1].starts_with("https://ghproxy.net/https://github.com/"));
    }

    #[test]
    fn builtin_mirrors_are_usable_as_prefixes() {
        let mirrors = default_update_mirrors();
        assert!(!mirrors.is_empty());
        for mirror in &mirrors {
            assert!(mirror.starts_with("https://"), "{mirror} 必须是 https");
            assert!(mirror.ends_with('/'), "{mirror} 要以 / 结尾，拼地址时才不会少一个斜杠");
        }
    }

    #[test]
    fn update_asset_prefers_the_bare_exe() {
        let names = update_asset_names("1.0.1");
        assert_eq!(names[0], "PixivNovelDownloader-v1.0.1.exe", "裸 exe 是首选");
        assert_eq!(names[1], "PixivNovelDownloader-v1.0.1.zip", "历史发布过 zip，留作后备");
    }

    /// 真机联网验证版本探测与安装包探测（默认 `#[ignore]`，要手动跑：`cargo test -- --ignored`）。
    /// 断网或发布页改版时会失败，所以不进常规回归。
    #[test]
    #[ignore]
    fn probe_latest_version_hits_the_real_repository() {
        let mirrors = default_update_mirrors();
        let client = update_client(std::time::Duration::from_secs(30)).unwrap();
        let (version, source) = probe_latest_version(&client, &mirrors).unwrap();
        println!("最新版本 {version}（来源 {source}）");

        // 探测出来的这版必须真的能下到安装包（历史上发过 zip，所以两种后缀都接受）
        let asset = probe_update_asset(&client, &version, &mirrors).expect("应该探测到安装包");
        println!(
            "安装包 {}（{} 字节）可用地址 {} 个，首选 {}",
            asset.name,
            asset.size,
            asset.urls.len(),
            asset.urls[0]
        );
        assert!(asset.name.ends_with(".zip") || asset.name.ends_with(".exe"));
        assert!(asset.size > 1_000_000);
    }

    /// 真机联网验证更新说明能从 `releases.atom` 取回来。v1.0.0 的说明是写全了的，
    /// 取回来必须是有内容的中文，而不是一堆没剥干净的 HTML 标签。
    #[test]
    #[ignore]
    fn release_notes_are_fetched_from_the_real_repository() {
        let mirrors = default_update_mirrors();
        let client = update_client(std::time::Duration::from_secs(30)).unwrap();
        let (title, notes, source) = probe_release_notes(&client, "1.0.0", &mirrors).unwrap();
        println!("标题：{title}\n来源：{source}\n--- 说明 ---\n{notes}");
        assert!(!notes.trim().is_empty());
        assert!(notes.contains("自动更新"), "说明里应该有这版的更新内容");
        assert!(!notes.contains("<p>"), "HTML 标签应该已经剥掉");
        assert!(!notes.contains("&lt;"), "实体应该已经还原");
    }

    #[test]
    fn release_body_is_read_from_the_atom_feed() {
        // 精简过的真实 feed 结构：正文是「转义过的 HTML」，还带属性和 XML 实体
        let feed = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Release notes from pixiv-novel-downloader</title>
  <entry>
    <id>tag:github.com,2008:Repository/1/v1.0.1</id>
    <title>v1.0.1 &amp; 小修</title>
    <content type="html">&lt;p&gt;修了三个问题&lt;/p&gt;&lt;ul&gt;&lt;li&gt;图片变形&lt;/li&gt;&lt;li&gt;镜像失效&lt;/li&gt;&lt;/ul&gt;</content>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/v1.0.0</id>
    <title>v1.0.0</title>
    <link rel="alternate" type="text/html" href="https://github.com/fromzero1501/pixiv-novel-downloader/releases/tag/v1.0.0"/>
    <content type="html">&lt;p&gt;自动更新上线&lt;/p&gt;</content>
  </entry>
</feed>"#;
        let entries = atom_entries(feed);
        assert_eq!(entries.len(), 2, "只应该切出 entry，不该把 feed 头部算进来");

        // 按 tag 挑：要 v1.0.0 就得给 v1.0.0 那条，而不是最新的 v1.0.1
        let picked = entries
            .iter()
            .find(|entry| version_from_tag_text(entry).as_deref() == Some("1.0.0"))
            .expect("应该能按 tag 对上 v1.0.0");
        let raw = decode_entities(&atom_field(picked, "content").unwrap());
        assert_eq!(raw, "<p>自动更新上线</p>");
        assert_eq!(tidy_release_notes(&html_to_text(&raw)), "自动更新上线");

        // 正文里的 &amp; 只解一次：还原成 & 之后不该再被当成实体
        let title = decode_entities(&atom_field(&entries[0], "title").unwrap());
        assert_eq!(title, "v1.0.1 & 小修");
    }

    #[test]
    fn release_body_keeps_bullets_and_paragraphs_on_their_own_lines() {
        let html = "<h2>新增</h2><ul><li>自动更新</li><li>内置帮助</li></ul><p>另一段<br>换行后</p>";
        assert_eq!(
            tidy_release_notes(&html_to_text(html)),
            "新增\n- 自动更新\n- 内置帮助\n另一段\n换行后"
        );
    }

    #[test]
    fn release_body_entities_are_decoded_once_per_layer() {
        // atom 里写的 `&amp;lt;` → 解一层得到 `&lt;` → 剥标签后是纯文本 → 再解一层得到 `<`
        assert_eq!(decode_entities("&lt;p&gt;a &amp;amp; b&lt;/p&gt;"), "<p>a &amp; b</p>");
        assert_eq!(decode_entities("a &amp;amp; b"), "a &amp; b");
        assert_eq!(decode_entities("中文&#65292;加&#x27;引号"), "中文，加'引号");
        // 不是实体的裸 & 要原样留着，别把后面的文字吞掉
        assert_eq!(decode_entities("A & B"), "A & B");
        assert_eq!(decode_entities("5 & 6 是数字"), "5 & 6 是数字");
    }

    #[test]
    fn release_body_is_cut_short_when_it_is_too_long() {
        let long = format!("<p>{}</p>", "很长的一行".repeat(2000));
        let text = tidy_release_notes(&html_to_text(&long));
        assert!(text.chars().count() < 4200, "超长说明要被截断");
        assert!(text.ends_with("…（更新说明较长，完整内容见发布页）"));
        // 短说明不该被动
        assert_eq!(tidy_release_notes("就一行"), "就一行");
    }
}
