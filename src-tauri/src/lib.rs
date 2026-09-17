use calamine::{open_workbook_auto, Reader};
use chrono::{DateTime, Local, NaiveDate, Utc};
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
    /// 上次同步之后新加进来、还没点开看过的作品数（作者卡上提示「新增 N 篇」用）。
    new_count: i64,
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
    /// Pixiv 简介（同步时拿到的 description）。老库升级后是空的，可以用「补抓简介」填上。
    synopsis: String,
    /// 向 Pixiv 要过简介、而且确认作者就是没写（`works.synopsis_checked = 1`）。
    /// 前端靠它把「还没补抓」和「补了也没有」这两件事分开说 —— 两者 synopsis 都是空串。
    synopsis_checked: bool,
    /// 阅读状态：0 = 未读（从没打开过），1 = 在读（打开过），2 = 已读（手动标的）。
    /// 注意：外部阅读器读完不会回调，所以只有「已读」是手动的。
    read_state: i64,
    /// 个人评分 0-5，0 = 未评。
    rating: i64,
    /// 一句话笔记，最多 200 字。
    note: String,
    /// 「待补完整版」工作台上的处理状态：0 = 未处理，1 = 找过、确实没有，
    /// 2 = 不打算补。只有 0 的会列在工作台默认视图里。
    need_full_state: i64,
    /// 上面那个状态是什么时候标的（RFC3339）。没标过＝空串，
    /// 恢复成「未处理」也会清空 —— 工作台要靠它显示「何时找过的」，别过俩月又翻一遍。
    need_full_marked_at: String,
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
    /// 系列里「读过」的篇数 —— 「已读」和「在读」都算。
    ///
    /// 口径改成含「在读」是为了让进度真的动起来（v1.2.8）：外部阅读器读完不会回调，
    /// 打开一篇只会把它标成「在读」，如果只数「已读」，进度就永远停在 0
    /// （用户报的就是「这个已读一直是零」）。不另存一份进度数字，免得和用户手动标的状态对不上。
    read_count: i64,
    /// 系列里「缺的序号」（v1.2.9）：1..max_order 里没有作品占的号。
    ///
    /// 只算序号 ≥ 1 的 —— 序号 0 意思是「还没排进系列」，不是「第 0 篇」。
    /// 序号唯一是 set_work_series 保证的（占位会拒绝），所以这儿只找空号、不查重。
    gap_orders: Vec<i64>,
}

/// 「我的收藏」页上的一个收藏夹卡片。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionSummary {
    id: i64,
    name: String,
    work_count: i64,
    /// 拿这个夹子里最新那篇的封面当卡片图，空夹子就是空字符串
    cover_path: String,
    created_at: String,
}

/// 「浏览历史」页的一行：作品本身 + 什么时候看的、看了几次。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryEntry {
    work: Work,
    viewed_at: String,
    view_count: i64,
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
    /// 打开作品 / 阅读版时是否记一笔浏览历史。
    #[serde(default = "default_true")]
    record_history: bool,
    /// 每天第一次启动时自动备份数据库（滚动的，只留最近若干份）。
    #[serde(default = "default_true")]
    auto_backup_enabled: bool,
    /// 自动备份保留最近几份。
    #[serde(default = "default_backup_keep")]
    auto_backup_keep: usize,
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
    /// Pixiv 简介，同步时顺手带上，落进 works.synopsis。
    synopsis: String,
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

/// 老库里 `works.favorite=1` 的作品，升级时统一收进这个收藏夹（v1.1.0）。
const DEFAULT_COLLECTION_NAME: &str = "我的收藏";

/// 浏览历史最多留多少条：超了就按时间从旧到新丢掉尾巴。
const HISTORY_LIMIT: i64 = 500;

/// 作品的文件路径一变，缓存下来的字数就作废（标回 -1 等后台重算，v1.2.8）。
///
/// 抽成常量是为了让单测能用**同一份** DDL 建触发器 —— 测试里自己手抄一遍的话，
/// 哪天改了这里、测试还在替旧的守门。
///
/// 两个细节都不能省：`WHEN` 那半句（SQLite 的 `UPDATE OF` 只看列在不在 SET 里、
/// 不看值有没有变，没这句的话每次「重扫文件」都会把全库字数标回未算）；
/// 触发器体内只写 `word_count`，所以不会再触发自己。
const WORD_COUNT_TRIGGER_DDL: &str = "CREATE TRIGGER IF NOT EXISTS works_path_change_resets_word_count
     AFTER UPDATE OF preview_path, purchased_path ON works
     FOR EACH ROW
     WHEN OLD.preview_path <> NEW.preview_path OR OLD.purchased_path <> NEW.purchased_path
     BEGIN
       UPDATE works SET word_count = -1 WHERE id = NEW.id;
     END;";

/// v1.1.0 把「收藏」升级成「收藏夹」：老库里 `works.favorite=1` 的作品全部收进一个
/// 默认收藏夹，升级后「我的收藏」才不是空的。用 app_settings 的标记位保证只跑一次 ——
/// **不能**改成「collections 表为空就迁」：用户把夹子删空了，下次启动不该又被塞回来。
fn migrate_favorites_into_collections(conn: &Connection) -> Result<(), String> {
    if setting(conn, "collections_migrated")? == "1" {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO collections (name, sort_order, created_at) VALUES (?1, 0, ?2)",
        params![DEFAULT_COLLECTION_NAME, now],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR IGNORE INTO collection_works (collection_id, work_id, added_at)
         SELECT c.id, w.id, ?2 FROM collections c, works w
         WHERE c.name = ?1 AND w.favorite = 1",
        params![DEFAULT_COLLECTION_NAME, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES ('collections_migrated', '1')
         ON CONFLICT(key) DO UPDATE SET value='1'",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
          synopsis TEXT NOT NULL DEFAULT '',
          synopsis_checked INTEGER NOT NULL DEFAULT 0,
          read_state INTEGER NOT NULL DEFAULT 0,
          rating INTEGER NOT NULL DEFAULT 0,
          note TEXT NOT NULL DEFAULT '',
          need_full_state INTEGER NOT NULL DEFAULT 0,
          -- 「待补完整版」那个状态是什么时候标的（RFC3339，空串＝没标过）。
          need_full_marked_at TEXT NOT NULL DEFAULT '',
          -- 正文字数：-1 = 还没算过，0 = 读不出来（绑的是 EPUB / 目录里没 txt），> 0 = 字数。
          -- 落库的理由是「列表页不能再读文件」：全库 1350 个 txt、98 MB，一次要 1.4 秒，
          -- 而列表每次改筛选 / 搜索都会重查一遍（v1.2.8）。
          word_count INTEGER NOT NULL DEFAULT -1,
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
        );
        CREATE TABLE IF NOT EXISTS collections (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL UNIQUE,
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS collection_works (
          collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
          work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
          added_at TEXT NOT NULL DEFAULT '',
          PRIMARY KEY (collection_id, work_id)
        );
        CREATE TABLE IF NOT EXISTS work_history (
          work_id INTEGER PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,
          viewed_at TEXT NOT NULL DEFAULT '',
          view_count INTEGER NOT NULL DEFAULT 1
        );
        -- 「筛选视图」（v1.2.9）：把当前那套筛选条件取个名字存下来，侧栏一键回到同一条件。
        -- 它和「收藏夹」是两种东西 —— 收藏夹是**手动往里放作品**（存结果），
        -- 这里只存**条件**（payload 里是一份 JSON），每次进来自个儿现算，
        -- 所以「未读」这类条件会随着阅读自然变少。两者千万别做成一个入口。
        CREATE TABLE IF NOT EXISTS filter_views (
          id INTEGER PRIMARY KEY,
          name TEXT NOT NULL UNIQUE,
          payload TEXT NOT NULL DEFAULT '',
          sort_order INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS work_history_viewed_at ON work_history(viewed_at DESC);",
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
    // 作品详情弹窗用到的四个个人字段（v1.2.0）：简介、阅读状态、评分、笔记。
    // 简介同步时本来就抓到了（只拿去判断预览版），这里只是给它一个落盘的位置。
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN synopsis TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN read_state INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN rating INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN note TEXT NOT NULL DEFAULT ''",
        [],
    );
    // 补抓简介（v1.2.4）：这一篇向 Pixiv 要过简介了、而且确认作者就是没写。
    // 没有这一位的话，这些「永远补不出来」的作品每次补抓都要再请求一遍，
    // 既白等又白喂风控 —— 而且头几十篇全是这种，浮层的「已更新」会一直停在 0，
    // 看着像卡死（用户就是这么报的）。
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN synopsis_checked INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // 「待补完整版」工作台（v1.2.7）：0 = 还没处理，1 = 去找过、确实没有，
    // 2 = 不打算补。只有 0 的才会在默认视图里列出来 —— 记这一位就是为了
    // **同一篇不用每次重新去找一遍**（这是那个工作台存在的全部理由）。
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN need_full_state INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // 标记「待补完整版」状态的时间（v1.2.8）：标过之后工作台要显示「什么时候找的」，
    // 不然过俩月翻到一篇，根本想不起来自己找没找过。恢复成「未处理」时清空。
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN need_full_marked_at TEXT NOT NULL DEFAULT ''",
        [],
    );
    // 正文字数落库（v1.2.8）：-1 = 未算，0 = 读不出来，> 0 = 字数。
    // 之前是列表时现读文件算的，全库 1350 个 txt / 98 MB 要 1.4 秒，
    // 而「所有作品」每次改筛选、每敲一下搜索都会重查 —— 那就叫不动了。
    let _ = conn.execute(
        "ALTER TABLE works ADD COLUMN word_count INTEGER NOT NULL DEFAULT -1",
        [],
    );
    // 字数缓存跟着路径走：路径一变，算好的字数就不作数了，标回「没算过」让后台重算。
    // 用触发器而不是去改那二十来处 `UPDATE ... SET preview_path/purchased_path` ——
    // 那些地方散在整个文件里，漏一处就是「改了文件、卡片上还挂着旧字数」，
    // 而且以后新写的代码也会自动被兜住。
    let _ = conn.execute_batch(WORD_COUNT_TRIGGER_DDL);
    let _ = conn.execute(
        "ALTER TABLE authors ADD COLUMN aliases TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS works_author_pixiv_novel_id ON works(author_id, pixiv_novel_id) WHERE pixiv_novel_id <> ''", []);
    // 收藏 → 收藏夹（v1.1.0）：老库里 `works.favorite=1` 的作品统一收进一个默认收藏夹，
    // 这样升级后「我的收藏」不会是空的。
    let _ = migrate_favorites_into_collections(&conn);
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
        record_history: match setting(conn, "record_history")?.as_str() {
            "" => true,
            value => value == "1",
        },
        auto_backup_enabled: match setting(conn, "auto_backup_enabled")?.as_str() {
            "" => true,
            value => value == "1",
        },
        // 存坏了 / 存成 0 都退回默认的 7；上限 50，免得用户填个天文数字把盘占满
        auto_backup_keep: {
            let value: usize = setting(conn, "auto_backup_keep")?.parse().unwrap_or(0);
            if value == 0 {
                default_backup_keep()
            } else {
                value.min(50)
            }
        },
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
          a.starred,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.is_new = 1)
        FROM authors a WHERE a.id = ?1",
        [id],
        |row| Ok(AuthorSummary { id: row.get(0)?, name: row.get(1)?, homepage: row.get(2)?, avatar_path: row.get(3)?, notes: row.get(4)?, preview_dir: row.get(5)?, purchased_dir: row.get(6)?, match_threshold: row.get(7)?, pixiv_last_sync_at: row.get(8)?, avatar_managed: row.get::<_, i64>(9)? == 1, work_count: row.get(10)?, purchased_count: row.get(11)?, favorite_count: row.get(12)?, aliases: row.get(13)?, images_count: row.get(14)?, starred: row.get::<_, i64>(15)? == 1, new_count: row.get(16)? })
    ).map_err(|e| e.to_string())
}

// 列号约定（五处 SELECT 必须完全一致）：0 author_id / 1 id / 2 title / 3 release_date /
// 4 preview_path / 5 cover_path / 6 purchased_path / 7 favorite / 8 has_images / 9 tags /
// 10 pixiv_novel_id / 11 series_id / 12 series_title / 13 series_order / 14 is_new /
// 15 author_name / 16 image_count / 17 synopsis / 18 read_state / 19 rating / 20 note /
// 21 synopsis_checked / 22 need_full_state / 23 word_count / 24 need_full_marked_at。
// **新列一律加在末尾** ——
// 插在中间会让所有列号整体后移，那种错编译器看不出来，只会静默读错列。
// （浏览历史在那之后再接 viewed_at / view_count，行号见 HISTORY_VIEWED_AT_INDEX）
fn map_work(row: &rusqlite::Row<'_>) -> rusqlite::Result<Work> {
    // -1 = 还没算过；0 = 读不出来；> 0 = 字数
    let stored_word_count: i64 = row.get(23)?;
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
        word_count: usize::try_from(stored_word_count).ok(),
        file_format: None,
        image_count: row.get::<_, i64>(16)?,
        synopsis: row.get(17)?,
        read_state: row.get(18)?,
        rating: row.get(19)?,
        note: row.get(20)?,
        synopsis_checked: row.get::<_, i64>(21)? == 1,
        need_full_state: row.get(22)?,
        need_full_marked_at: row.get(24)?,
    })
}

/// 五处 SELECT 共用的作品列清单，避免手写列号时漏改一处。
const WORK_COLUMNS: &str = "author_id, id, title, release_date, preview_path, cover_path, purchased_path, favorite, has_images, tags, pixiv_novel_id, series_id, series_title, series_order, is_new, '' AS author_name, image_count, synopsis, read_state, rating, note, synopsis_checked, need_full_state, word_count, need_full_marked_at";
const WORK_COLUMNS_W: &str = "w.author_id, w.id, w.title, w.release_date, w.preview_path, w.cover_path, w.purchased_path, w.favorite, w.has_images, w.tags, w.pixiv_novel_id, w.series_id, w.series_title, w.series_order, w.is_new, a.name AS author_name, w.image_count, w.synopsis, w.read_state, w.rating, w.note, w.synopsis_checked, w.need_full_state, w.word_count, w.need_full_marked_at";
/// 接在 `WORK_COLUMNS` / `WORK_COLUMNS_W` 之后的第一列下标（浏览历史把 `h.viewed_at`
/// 拼在作品列后面）。抽成常量是因为往列清单里加字段时最容易漏改这里：
/// 数字写错编译器不会报错，只会静默读错列。
const HISTORY_VIEWED_AT_INDEX: usize = 25;

/// 读一个文本文件并按「BOM → UTF-8 → GBK」的顺序猜编码。
/// 字数统计和合集 EPUB 都走这儿，免得两处各猜一套、同一份文件读出两种结果。
fn read_text_file(path: &Path) -> Option<String> {
    Some(decode_text_bytes(&fs::read(path).ok()?))
}

/// 字节 → 文本。顺序不能改：BOM → UTF-8 → GBK。
/// 全库的正文都走这一条路（字数统计、合集导出、正文检索），
/// 换顺序会让同一份文件在三处读出不同结果。
fn decode_text_bytes(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        UTF_16LE.decode(&bytes[2..]).0.into_owned()
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        UTF_16BE.decode(&bytes[2..]).0.into_owned()
    } else if let Ok(content) = std::str::from_utf8(bytes) {
        content.to_string()
    } else {
        GBK.decode(bytes).0.into_owned()
    }
}

fn text_file_word_count(path: &Path) -> Option<usize> {
    let content = read_text_file(path)?;
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

/**
 * 补齐「只有看了文件才知道」的那几位。**这里不再读正文文件。**
 *
 * 之前 `word_count` 是在这里现读文件算的 —— 全库 1350 个 txt、98 MB，一次 1.4 秒，
 * 而列表每次改筛选 / 敲搜索都会重跑一遍，界面就卡住了（v1.2.8 用户报的「加载得有点慢」）。
 * 现在字数走 `works.word_count` 那一列（由 `refresh_word_counts` 在后台一次算好），
 * 这里只做两件便宜事：封面兜底、认出 EPUB/HTML 这类非 txt 的阅读版格式。
 * 想再加「要读文件」的字段，请走后台补算那条路，别加回这个函数。
 */
fn populate_work_display_info(work: &mut Work) {
    work.cover_path = resolve_cover_path(&work.cover_path, &work.preview_path, &work.purchased_path);
    work.file_format = None;
    if work.purchased_path.is_empty() {
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
        }
    }
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
          a.starred,
          (SELECT COUNT(*) FROM works w WHERE w.author_id = a.id AND w.is_new = 1)
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
                new_count: row.get(16)?,
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
        (
            "record_history",
            if settings.record_history {
                "1".into()
            } else {
                "0".into()
            },
        ),
        (
            "auto_backup_enabled",
            if settings.auto_backup_enabled {
                "1".into()
            } else {
                "0".into()
            },
        ),
        (
            "auto_backup_keep",
            // 与 read_settings 同一套规则：0 当没填、退回默认；上限 50
            if settings.auto_backup_keep == 0 {
                default_backup_keep()
            } else {
                settings.auto_backup_keep.min(50)
            }
            .to_string(),
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

/// 搜索范围 → SQL 里的匹配条件。三处作品查询（作者作品库 / 所有作品 / 收藏夹）共用。
///
/// 列一律用 `COALESCE(..,'')` 拼：SQLite 里 `'x' || NULL` 结果是 NULL，会让整条 LIKE
/// 永远不成立（老库里 synopsis 可能是 NULL）。固定只用两个占位符 —— 空的短路判断 +
/// 「%词%」，所以三处调用点的 params 顺序完全一致，不用为每种范围数占位符。
fn search_match_clause(
    search_field: &str,
    prefix: &str,
    empty_param: &str,
    like_param: &str,
) -> String {
    let title = format!("{prefix}title");
    let body = match search_field {
        "tags" => format!("{prefix}tags LIKE {like_param}"),
        "title_synopsis" => format!(
            "(COALESCE({title},'') || ' ' || COALESCE({prefix}synopsis,'')) LIKE {like_param}"
        ),
        "title_synopsis_tags" => format!(
            "(COALESCE({title},'') || ' ' || COALESCE({prefix}synopsis,'') || ' ' || COALESCE({prefix}tags,'')) LIKE {like_param}"
        ),
        _ => format!("{title} LIKE {like_param}"),
    };
    format!("({empty_param} = '' OR {body})")
}

/* ======================== 库内正文全文检索（v1.2.11） ======================== */

/// 命中片段前后各留多少**字符**（不是字节）。太小读不出上下文，太大卡片上排不下。
const TEXT_SEARCH_CONTEXT_CHARS: usize = 36;
/// 每篇最多给几段。卡片上留三段就到顶了。
const TEXT_SEARCH_MAX_SNIPPETS: usize = 3;
/// 命中篇目上限。搜「的」这种字会命中九成作品，全返回既没意义又拖慢渲染。
const TEXT_SEARCH_MAX_HITS: usize = 200;
/// 连正文都没找到的作品只报总数 + 前几个名字，列全了没地方放。
const TEXT_SEARCH_LISTED_MISSING: usize = 12;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TextSnippet {
    /// 命中处前面的正文（已压成单行）
    before: String,
    /// 命中的那几个字，原样保留大小写，前端拿它做高亮
    hit: String,
    /// 命中处后面的正文
    after: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TextSearchHit {
    work_id: i64,
    hit_count: usize,
    snippets: Vec<TextSnippet>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TextSearchResult {
    hits: Vec<TextSearchHit>,
    /// 真扫过正文的作品数
    scanned_count: usize,
    /// 没找到正文的作品（没绑阅读版 / 文件丢了），只列前几个
    missing: Vec<String>,
    missing_count: usize,
    elapsed_ms: u64,
    /// 命中数超过上限、列表被截断了
    truncated: bool,
}

/// 一段正文里出现多少处，外加头几处的前后文。
/// 计数要把整段扫完（不然报的数是假的），但片段只留前三段。
fn text_search_matches(text: &str, query: &str) -> (usize, Vec<TextSnippet>) {
    let mut count = 0usize;
    let mut snippets = Vec::new();
    for (start, matched) in text.match_indices(query) {
        count += 1;
        if snippets.len() < TEXT_SEARCH_MAX_SNIPPETS {
            snippets.push(text_snippet_at(text, start, start + matched.len()));
        }
    }
    (count, snippets)
}

/// 抠出命中处前后各 N 个**字符**。
/// 中文一个字三字节，这里必须按字符走 —— 直接切字节会切出半个字，
/// 轻则显示成乱码，重则因为不是 char 边界直接 panic。
fn text_snippet_at(text: &str, start: usize, end: usize) -> TextSnippet {
    let before: String = text[..start]
        .chars()
        .rev()
        .take(TEXT_SEARCH_CONTEXT_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let after: String = text[end..].chars().take(TEXT_SEARCH_CONTEXT_CHARS).collect();
    TextSnippet {
        before: single_line(&before),
        hit: text[start..end].to_string(),
        after: single_line(&after),
    }
}

/// 正文里到处都是换行，片段得压成一行才好排在卡片上
fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 丢掉 `<script>…</script>` / `<style>…</style>` 里的**内容**。
/// 现成的 `html_to_text` 只丢标签本身，脚本原文会原样留下来 ——
/// 不掐掉的话搜「function」会在每一篇带 JS 的 HTML 里搜出一堆假命中。
fn strip_element_block(source: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    // 只动 ASCII 大小写，字节长度不变 —— 所以拿它算出来的偏移能直接切原文
    let lowered = source.to_ascii_lowercase();
    let mut out = String::with_capacity(source.len());
    let mut cursor = 0usize;
    while let Some(offset) = lowered[cursor..].find(&open) {
        let start = cursor + offset;
        out.push_str(&source[cursor..start]);
        let Some(close_offset) = lowered[start..].find(&close) else {
            return out; // 没闭合就当后面全是脚本，整段丢掉
        };
        let mut end = start + close_offset;
        match lowered[end..].find('>') {
            Some(gt) => end += gt + 1,
            None => return out,
        }
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

/// HTML / XHTML 正文 → 纯文本
fn document_text(source: &str) -> String {
    let without_script = strip_element_block(source, "script");
    let without_style = strip_element_block(&without_script, "style");
    html_to_text(&without_style)
}

/// EPUB 里哪些条目算正文 —— 按扩展名认。
/// 图片条目**一个字节都不读**：zip 只解压点名要的那几条，
/// 所以 20 MB 的图文 epub 抽正文跟 2 MB 的一样快（实测都是毫秒级）。
fn is_epub_text_entry(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    if lowered.contains("__macosx") {
        return false;
    }
    matches!(
        Path::new(&lowered)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("xhtml" | "html" | "htm" | "txt")
    )
}

/// 从 EPUB 里抽出正文。失败（文件坏了 / 加密 / 没见过的 zip 写法）就返回 None，
/// 让这一篇进「没找到正文」名单 —— 一次搜索不能因为一本坏书整个失败。
fn epub_text(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    // by_index 会可变借用 archive，所以先把名字拷出来，下面再按名字取内容
    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        if let Ok(entry) = archive.by_index(index) {
            let name = entry.name().to_string();
            if is_epub_text_entry(&name) {
                names.push(name);
            }
        }
    }
    names.sort();
    let mut out = String::new();
    for name in names {
        let Ok(mut entry) = archive.by_name(&name) else {
            continue;
        };
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        out.push_str(&document_text(&decode_text_bytes(&bytes)));
        out.push('\n');
    }
    (!out.trim().is_empty()).then_some(out)
}

/// 一篇作品的正文纯文本：txt/md 直接读，html 剥标签，epub 抽内页。
/// 认不出的扩展名返回 None（不是错误 —— 大部分作品本来就没绑阅读版）。
fn text_search_document(path: &Path) -> Option<String> {
    if path.is_dir() {
        return anthology_text_file(path).and_then(|file| text_search_document(&file));
    }
    if !path.is_file() {
        return None;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "txt" | "md" => read_text_file(path),
        "html" | "htm" | "xhtml" => read_text_file(path).map(|source| document_text(&source)),
        "epub" => {
            // 同名 .txt 比 epub 里那堆 xhtml 干净得多，有就直接用
            let beside = path.with_extension("txt");
            if beside.is_file() {
                return read_text_file(&beside);
            }
            epub_text(path)
        }
        _ => None,
    }
}

fn search_full_text_impl(query: &str, limit: Option<usize>) -> Result<TextSearchResult, String> {
    let rows: Vec<(i64, String, String, String)> = {
        let conn = db()?;
        let mut statement = conn
            .prepare(
                "SELECT id, COALESCE(title,''), COALESCE(purchased_path,''), COALESCE(preview_path,'')
                 FROM works",
            )
            .map_err(|error| error.to_string())?;
        let collected = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        collected
    };
    search_full_text_over(&rows, query, limit)
}

/// 扫描本体：只认「一篇作品 = 一个 id + 名字 + 几条候选路径」，
/// 跟数据库脱开 —— 单测拿一批临时文件就能把整条读取链路跑通，
/// 不用去碰真的库文件。
fn search_full_text_over(
    rows: &[(i64, String, String, String)],
    query: &str,
    limit: Option<usize>,
) -> Result<TextSearchResult, String> {
    let needle = query.trim();
    if needle.is_empty() {
        return Err("先输入要搜的内容".into());
    }
    let cap = limit.unwrap_or(TEXT_SEARCH_MAX_HITS).clamp(1, TEXT_SEARCH_MAX_HITS);
    let started = std::time::Instant::now();

    // 整库一百来兆正文，单线程也就零点几秒 —— 但读文件大半时间在等 IO，
    // 几个线程重叠起来能省掉不少。上限压在 6：线程再多也是抢同一块盘。
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .clamp(1, 6);
    let chunk = rows.len().div_ceil(workers).max(1);

    let mut collected: Vec<(i64, usize, Vec<TextSnippet>)> = Vec::new();
    let mut scanned_count = 0usize;
    let mut missing: Vec<String> = Vec::new();

    std::thread::scope(|scope| {
        let handles: Vec<_> = rows
            .chunks(chunk)
            .map(|slice| {
                scope.spawn(move || {
                    let mut hits: Vec<(i64, usize, Vec<TextSnippet>)> = Vec::new();
                    let mut scanned = 0usize;
                    let mut missing: Vec<String> = Vec::new();
                    for (work_id, title, purchased, preview) in slice {
                        // 完整版在前、预览版在后。两份都在就都扫 ——
                        // 预览版和完整版的正文不一定是同一份，漏掉哪边都可能少命中。
                        let mut sources: Vec<&str> = Vec::new();
                        if !purchased.is_empty() {
                            sources.push(purchased.as_str());
                        }
                        if !preview.is_empty() && preview != purchased {
                            sources.push(preview.as_str());
                        }
                        let mut document = String::new();
                        for source in sources {
                            if let Some(part) = text_search_document(Path::new(source)) {
                                document.push_str(&part);
                                document.push('\n');
                            }
                        }
                        if document.trim().is_empty() {
                            missing.push(title.clone());
                            continue;
                        }
                        scanned += 1;
                        let (count, snippets) = text_search_matches(&document, needle);
                        if count > 0 {
                            hits.push((*work_id, count, snippets));
                        }
                    }
                    (hits, scanned, missing)
                })
            })
            .collect();
        for handle in handles {
            let (hits, scanned, missed) = handle
                .join()
                .unwrap_or_else(|_| (Vec::new(), 0usize, Vec::new()));
            collected.extend(hits);
            scanned_count += scanned;
            missing.extend(missed);
        }
    });

    // 命中多的排前面，同样多的按 work_id 稳定排 —— 免得每次搜出来顺序都不一样
    collected.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let truncated = collected.len() > cap;
    collected.truncate(cap);
    let missing_count = missing.len();
    missing.sort();
    missing.truncate(TEXT_SEARCH_LISTED_MISSING);

    Ok(TextSearchResult {
        hits: collected
            .into_iter()
            .map(|(work_id, hit_count, snippets)| TextSearchHit {
                work_id,
                hit_count,
                snippets,
            })
            .collect(),
        scanned_count,
        missing,
        missing_count,
        elapsed_ms: started.elapsed().as_millis() as u64,
        truncated,
    })
}

/// 库内正文全文检索。**不建索引、不落地** —— 每次现扫绑定的本地正文。
/// 实测整库一百来兆正文 + 一千四百个文件约 0.2～0.4 秒（epub 只解压正文条目，
/// 图片一个字节不读），所以没必要维护一份索引；代价是每次都要真的读一遍盘，
/// 因此**只该在回车时调用**，不能挂在输入事件上（每敲一个字扫一遍库，必卡）。
#[tauri::command]
async fn search_full_text(query: String, limit: Option<usize>) -> Result<TextSearchResult, String> {
    tauri::async_runtime::spawn_blocking(move || search_full_text_impl(&query, limit))
        .await
        .map_err(|error| error.to_string())?
}

/// 「配图」三档拼出来的条件（v1.2.13）：`all` 不限 / `has` 有图 / `none` 无图。
///
/// `prefix` 是表别名 —— 作者库那条 SQL 没别名（传空串），另外两条是 `w.`。
///
/// 抽成函数只为一件事：**这几处 SQL 是运行时拼的字符串，写错了编译器不管**，
/// 只有真点一下才发现「有图和无图筛出来一模一样」。所以给它一个拿内存库真跑的单测。
fn images_filter_clause(filter: &str, prefix: &str) -> String {
    match filter {
        "has" => format!(" AND {prefix}has_images = 1"),
        "none" => format!(" AND {prefix}has_images = 0"),
        _ => String::new(),
    }
}

/// 「收藏夹」那一档：`0` = 不限、`-1` = 任意收藏夹、正数 = 指定夹子。
///
/// 放 EXISTS 而不是 JOIN：作品可能同时在多个夹子里，JOIN 会把它变成多行。
/// 条件写成恒真的形式（而不是拼不拼这段），这样占位符永远存在于 SQL 里，
/// params 的数量不用分情况。
///
/// 「任意收藏」不必另写一段 SQL：内层 `{placeholder} = -1` 恒真，EXISTS 就退化成
/// 「在任何一个夹子里」。收藏夹主键都是正数，-1 撞不上真的夹子。
///
/// `alias` 是子查询里 `collection_works` 的别名 —— 收藏夹视图那条 SQL 外层已经用了
/// `cw`，内层不能重名，传 `cw2`。
fn collection_filter_clause(work_column: &str, alias: &str, placeholder: &str) -> String {
    format!(
        " AND ({placeholder} = 0 OR EXISTS (SELECT 1 FROM collection_works {alias} WHERE {alias}.work_id = {work_column} AND ({placeholder} = -1 OR {alias}.collection_id = {placeholder})))"
    )
}

#[tauri::command]
fn list_works(
    author_id: i64,
    query: String,
    search_field: String,
    status: String,
    favorites_only: bool,
    images_filter: String,
    collection_id: i64,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let condition = search_match_clause(&search_field, "", "?2", "?3");
    let mut sql = format!("SELECT {WORK_COLUMNS} FROM works WHERE author_id = ?1 AND {condition}");
    match status.as_str() {
        "purchased" => sql.push_str(" AND purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND favorite = 1");
    }
    sql.push_str(&images_filter_clause(&images_filter, ""));
    sql.push_str(&collection_filter_clause("works.id", "cw", "?4"));
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY release_date ASC, id ASC",
        "title_asc" => " ORDER BY title COLLATE NOCASE ASC",
        // 「字数从多到少」的字数要读文件才知道，SQL 排不了：先按日期打个底，取回数据后再在 Rust 里重排
        "words_desc" => " ORDER BY release_date DESC, id DESC",
        // 评分从高到低，没打分的（0）自动沉底
        "rating_desc" => " ORDER BY rating DESC, release_date DESC, id DESC",
        _ => " ORDER BY release_date DESC, id DESC",
    });
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let raw_query = query.trim();
    let rows = statement
        .query_map(
            params![
                author_id,
                raw_query,
                format!("%{raw_query}%"),
                collection_id
            ],
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
    images_filter: String,
    collection_id: i64,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let condition = search_match_clause(&search_field, "w.", "?1", "?2");
    let mut sql = format!("SELECT {WORK_COLUMNS_W} FROM works w JOIN authors a ON a.id=w.author_id WHERE {condition}");
    match status.as_str() {
        "purchased" => sql.push_str(" AND w.purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND w.purchased_path = ''"),
        _ => {}
    }
    if favorites_only {
        sql.push_str(" AND w.favorite = 1");
    }
    sql.push_str(&images_filter_clause(&images_filter, "w."));
    sql.push_str(&collection_filter_clause("w.id", "cw", "?3"));
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY w.release_date ASC, w.id ASC",
        "title_asc" => " ORDER BY w.title COLLATE NOCASE ASC",
        // 同 list_works：「字数从多到少」只在 SQL 里给个日期底序，最终顺序在 Rust 里排
        "words_desc" => " ORDER BY w.release_date DESC, w.id DESC",
        "rating_desc" => " ORDER BY w.rating DESC, w.release_date DESC, w.id DESC",
        _ => " ORDER BY w.release_date DESC, w.id DESC",
    });
    let raw_query = query.trim();
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(
            params![raw_query, format!("%{raw_query}%"), collection_id],
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
fn list_series_works(author_id: i64, series_id: String) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare(&format!("SELECT {WORK_COLUMNS} FROM works WHERE author_id=?1 AND series_id=?2 ORDER BY CASE WHEN series_order > 0 THEN 0 ELSE 1 END, series_order ASC, id ASC"))
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
    list_series_impl(&db()?, author_id)
}

fn list_series_impl(conn: &Connection, author_id: i64) -> Result<Vec<SeriesSummary>, String> {
    let mut statement = conn
        .prepare("SELECT s.id, s.title, COUNT(w.id), COALESCE(SUM(CASE WHEN w.purchased_path <> '' THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN w.purchased_path = '' THEN 1 ELSE 0 END), 0), COALESCE(MAX(NULLIF(w.cover_path, '')), ''), COALESCE(MAX(w.series_order), 0), COALESCE(SUM(CASE WHEN w.read_state IN (1, 2) THEN 1 ELSE 0 END), 0) FROM series_catalog s LEFT JOIN works w ON w.author_id=s.author_id AND w.series_id=s.id WHERE s.author_id=?1 GROUP BY s.id, s.title ORDER BY s.title COLLATE NOCASE")
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
                read_count: row.get(7)?,
                gap_orders: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut series = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    drop(statement);
    let gaps = series_gap_orders(conn, author_id)?;
    for item in series.iter_mut() {
        if let Some(orders) = gaps.get(&item.id) {
            item.gap_orders = orders.clone();
        }
    }
    Ok(series)
}

/// 每个系列缺哪几号。**一条查询把这位作者所有系列的序号一次取回来**，
/// 别按系列循环查库 —— 一个作者几十个系列就是几十次去重查询。
fn series_gap_orders(
    conn: &Connection,
    author_id: i64,
) -> Result<std::collections::HashMap<String, Vec<i64>>, String> {
    let mut statement = conn
        .prepare("SELECT series_id, series_order FROM works WHERE author_id=?1 AND series_order > 0")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([author_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut present: std::collections::HashMap<String, HashSet<i64>> = std::collections::HashMap::new();
    for row in rows {
        let (series_id, order) = row.map_err(|e| e.to_string())?;
        present.entry(series_id).or_default().insert(order);
    }
    let mut gaps: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    for (series_id, orders) in present {
        let max = orders.iter().copied().max().unwrap_or(0);
        let missing: Vec<i64> = (1..=max).filter(|order| !orders.contains(order)).collect();
        if !missing.is_empty() {
            gaps.insert(series_id, missing);
        }
    }
    Ok(gaps)
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

/// 同步时复用本地已有的预览版文件（顺带把它同名的封面也带上）。
///
/// **只看名字（`name_key` 去掉扩展名后）是否完全相同，不比相似度** —— 与
/// `existing_sync_target()` 的去重规则保持一致。相似度只属于「本地文件关联」，
/// 拿它做同步复用会把「其一 / 其二」这类长共同前缀的不同作品认成同一篇，
/// 直接复用错正文（宁可多下一份，也不能复用错内容）。
///
/// 同名文件有两份（理论上不该出现）时不猜，返回 `None` 走正常下载。
fn matched_sync_preview(
    entries: &[SyncPreviewEntry],
    title: &str,
) -> Option<(PathBuf, Option<PathBuf>)> {
    let key = name_key(title);
    let mut matches = entries
        .iter()
        .filter(|entry| entry.is_preview && name_key(&entry.name) == key);
    let preview = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let cover = entries
        .iter()
        .find(|entry| !entry.is_preview && name_key(&entry.name) == key)
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

/// 封面重压质量。依据见 `compress_cover`：在界面真实显示尺寸下实测肉眼无差别，
/// 而全库体积降到 24%。**改这个值等于改变所有新同步作品的封面画质**，别随手调。
const COVER_JPEG_QUALITY: u8 = 75;

/// 封面一律在落盘前压一道。
///
/// **为什么压**：Pixiv 封面走 `c/600x600` 档，本身只有 600px 高，却用极高的编码质量
/// （实测 415×600 占 206 KB ≈ 0.83 字节/像素），是同尺寸 q75 的四倍大。而它在界面里
/// 从没 1:1 显示过 —— 卡片 185 CSS px（DPR 2 → 370 物理像素）、详情页 `.detail-cover`
/// 只有 118 CSS px，都是把原图缩下来看，压缩痕迹被二次缩放又抹掉一道（实测 +2.4 dB）。
/// 实测：卡片真实尺寸下 q75 与原图的平均差 0.82%、PSNR 约 38 dB；缩略图场景更小。
///
/// **为什么放在「下载 → 写盘」这一步**：源头永远是刚从网络下来的原图，只压一次，
/// 天然不会出现「对已压过的文件再压一遍」的代际劣化。**不要去改成读本地文件再压** ——
/// 那样每个重新生成封面的路径（搬家、补封面、重新下载并绑定）都会再压一轮，JPEG 的
/// 劣化是累加的，几轮就糊了。
///
/// 解不开、编码失败、或压完反而变大（灰度/CMYK 之类的来源），一律原样返回 ——
/// 宁可存一张大图，也不能让同步断在一次编码上。
fn compress_cover(bytes: &[u8]) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageReader};
    use std::io::Cursor;

    let Ok(decoded) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())
        .and_then(|reader| reader.decode().map_err(|error| error.to_string()))
    else {
        return bytes.to_vec();
    };
    let rgb = decoded.to_rgb8();
    let mut out: Vec<u8> = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut out, COVER_JPEG_QUALITY);
    if encoder
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
        .is_err()
        || out.is_empty()
        || out.len() >= bytes.len()
    {
        return bytes.to_vec();
    }
    out
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
    /// 简介原文（Pixiv 那种 HTML），渲染前自己净化。
    synopsis: &'a str,
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
    // 简介（v1.2.11）：阅读版里折起来放，不打扰正文。作者没写就整块不出现。
    let synopsis = {
        let plain = synopsis_plain_text(meta.synopsis);
        if plain.is_empty() {
            String::new()
        } else {
            let paragraphs = plain
                .split('\n')
                .map(|line| format!("<p>{}</p>", escape_novel_html(line)))
                .collect::<Vec<_>>()
                .join("");
            format!("<details class=\"synopsis\"><summary>作品简介</summary>{paragraphs}</details>")
        }
    };
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
details.synopsis {{ margin: 14px 0 4px; padding: 10px 14px; border-radius: 10px;
  background: #f8fafc; border: 1px solid #e6eaf0; color: #475467; font-size: 13px; }}
details.synopsis summary {{ cursor: pointer; color: #475467; font-weight: 600; }}
details.synopsis p {{ margin: 8px 0 0; font-size: 13px; text-align: left; }}
</style></head>
<body><main>
{cover}
<h1>{title}</h1>
<p class="meta">{author_name}{date}{images}</p>
<div class="tags">{tags}</div>
{synopsis}
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
        synopsis = synopsis,
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
details.synopsis{margin:0 0 1.4em;padding:.6em .8em;border:1px solid #e6eaf0;border-radius:.5em;background:#f8fafc;color:#475467;font-size:.9em;}
details.synopsis summary{cursor:pointer;font-weight:600;}
details.synopsis p{margin:.6em 0 0;font-size:.95em;text-align:left;}
"#;

/// 元数据里的简介留多长 —— 一部长篇的简介能有两千字，塞进 EPUB 元数据没意义。
const SYNOPSIS_META_MAX_CHARS: usize = 1200;

/// Pixiv 简介是 HTML（库里存原文，只在显示层净化）。
/// 写进 EPUB / Markdown 元数据时要先变成纯文本：剥脚本样式 → 剥标签 → 还原实体。
/// 段落之间保留换行，段内压平 —— 一坨两千字连成一行在 Calibre 里没法看。
fn synopsis_plain_text(synopsis: &str) -> String {
    if synopsis.trim().is_empty() {
        return String::new();
    }
    let text = decode_entities(&document_text(synopsis));
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let flat = single_line(line);
        if !flat.is_empty() {
            lines.push(flat);
        }
    }
    let mut plain = lines.join("\n");
    if plain.chars().count() > SYNOPSIS_META_MAX_CHARS {
        plain = plain.chars().take(SYNOPSIS_META_MAX_CHARS).collect::<String>();
        plain.push('…');
    }
    plain
}

/// 组包：封面页 + 正文页 + 目录 + 元数据。图片按正文顺序编成 001.jpg 这类包内路径。
fn build_novel_epub(
    title: &str,
    author_name: &str,
    release_date: &str,
    novel_id: &str,
    tags: &str,
    synopsis: &str,
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
    // 简介（v1.2.11）：写进 `<dc:description>`，Calibre 导入后能直接看到。
    // 库里存的是 Pixiv 那种 HTML，先净化成纯文本再截短 —— 元数据里放一坨标签没意义。
    let description_xml = {
        let plain = synopsis_plain_text(synopsis);
        if plain.is_empty() {
            String::new()
        } else {
            format!("<dc:description>{}</dc:description>", escape_novel_html(&plain))
        }
    };

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
    // 简介（v1.2.11）：标题页上也折一份 —— 元数据是给书库看的，这一份才是给读者看的。
    let synopsis_block = {
        let plain = synopsis_plain_text(synopsis);
        if plain.is_empty() {
            String::new()
        } else {
            let paragraphs = plain
                .split('\n')
                .map(|line| format!("<p>{}</p>", escape_novel_html(line)))
                .collect::<Vec<_>>()
                .join("");
            format!("<details class=\"synopsis\"><summary>作品简介</summary>{paragraphs}</details>\n")
        }
    };
    let novel_page = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>{title_xml}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../style.css\"/></head>\n<body>\n<h1>{title_xml}</h1>\n<p class=\"meta\">{meta_line}</p>\n<p>{tag_line}</p>\n{synopsis_block}{body}</body></html>\n"
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
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"zh-CN\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"bookid\">{identifier}</dc:identifier><dc:title>{title_xml}</dc:title><dc:language>zh-CN</dc:language><dc:creator>{author_xml}</dc:creator>{date_xml}{subjects}{description_xml}<meta property=\"dcterms:modified\">{modified}</meta>{cover_meta}</metadata>\n<manifest>{}</manifest>\n<spine toc=\"ncx\">{}</spine>\n</package>\n",
        manifest.join(""),
        spine.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/content.opf", opf.as_bytes());
    zip_finish(&mut out, &items);
    out
}

// ---------------------------------------------------------------------------
// 合集 EPUB（v1.2.9）：把一个系列 / 一个收藏夹里的多篇作品打成一本书
//
// 和单篇那套的区别只有「一章变多章」：单篇的 `build_novel_epub` 只写
// `text/novel.xhtml` 一页、目录也就一条；这里一章一个 xhtml，目录按章列。
// 素材全部取自**本地文件**（正文 txt + 同名 `_images` 目录），不联网 ——
// 用户要的是「把已经躺在硬盘上的那批打成一本书」，不是重新抓一遍。
// 联网重抓是「重新下载 EPUB 版」那条路，两者别混。
// ---------------------------------------------------------------------------

/// 合集里的一章（＝一篇作品）。
struct AnthologyChapter {
    title: String,
    /// 「作者 · 日期 · N 字」
    meta: String,
    tags: String,
    body: String,
    /// 这一章的配图：`(包内文件名, 字节)`。文件名带章节前缀，跨章不会撞。
    images: Vec<(String, Vec<u8>)>,
}

fn build_anthology_epub(
    title: &str,
    author_name: &str,
    // 这批作品同属一个系列时填系列名，写进 calibre:series
    series: Option<&str>,
    chapters: &[AnthologyChapter],
    cover: Option<(&str, &[u8])>,
) -> Vec<u8> {
    let title_xml = escape_novel_html(title);
    let author_xml = escape_novel_html(author_name);
    let identifier = format!("pixiv-anthology-{}", zip_crc32(title.as_bytes()));
    let modified = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // 合集没有「作者写的简介」，就自己报一份目录当简介 —— 书库里一眼能看出收了哪几篇。
    let subjects = chapters
        .iter()
        .flat_map(|chapter| chapter.tags.split(['|', ',']))
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .fold(Vec::<String>::new(), |mut acc, tag| {
            if !acc.iter().any(|existing| existing == tag) {
                acc.push(tag.to_string());
            }
            acc
        });
    let subjects_xml = subjects
        .iter()
        .map(|tag| format!("<dc:subject>{}</dc:subject>", escape_novel_html(tag)))
        .collect::<Vec<_>>()
        .join("");
    let description_xml = {
        let mut listed = chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| format!("{}.{}", index + 1, chapter.title))
            .collect::<Vec<_>>()
            .join("；");
        if listed.chars().count() > SYNOPSIS_META_MAX_CHARS {
            listed = listed.chars().take(SYNOPSIS_META_MAX_CHARS).collect::<String>();
            listed.push('…');
        }
        format!(
            "<dc:description>共收录 {} 篇：{}</dc:description>",
            chapters.len(),
            escape_novel_html(&listed)
        )
    };
    let series_meta = match series.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!(
            "<meta name=\"calibre:series\" content=\"{}\"/>",
            escape_novel_html(name)
        ),
        None => String::new(),
    };

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
        nav_items.push(r#"<li><a href="text/cover.xhtml">封面</a></li>"#.to_string());
        ncx_items.push(
            r#"<navPoint id="nav-cover" playOrder="1"><navLabel><text>封面</text></navLabel><content src="text/cover.xhtml"/></navPoint>"#
                .to_string(),
        );
    }

    for (index, chapter) in chapters.iter().enumerate() {
        let number = index + 1;
        let page_name = format!("chapter{number:03}.xhtml");
        let page_id = format!("chapter{number}");

        for (image_index, (image_name, bytes)) in chapter.images.iter().enumerate() {
            zip_push(
                &mut out,
                &mut items,
                &format!("OEBPS/images/{image_name}"),
                bytes,
            );
            // XML 的 id 不能带点，所以这里另起一个规整的 id —— 单篇那版直接把
            // 「001.jpg」拼进 id 里，是能凑合用，但没必要再复制一遍这个毛病。
            manifest.push(format!(
                r#"<item id="img{number}-{}" href="images/{image_name}" media-type="{}"/>"#,
                image_index + 1,
                epub_image_media_type(image_name)
            ));
        }

        let tag_line = chapter
            .tags
            .split(['|', ','])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| format!("<span class=\"tag\">{}</span>", escape_novel_html(tag)))
            .collect::<Vec<_>>()
            .join("");
        let heading = escape_novel_html(&chapter.title);
        let page = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>{heading}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../style.css\"/></head>\n<body>\n<h1>{heading}</h1>\n<p class=\"meta\">{}</p>\n<p>{tag_line}</p>\n{}</body></html>\n",
            escape_novel_html(&chapter.meta),
            chapter.body
        );
        zip_push(
            &mut out,
            &mut items,
            &format!("OEBPS/text/{page_name}"),
            page.as_bytes(),
        );
        manifest.push(format!(
            r#"<item id="{page_id}" href="text/{page_name}" media-type="application/xhtml+xml"/>"#
        ));
        spine.push(format!(r#"<itemref idref="{page_id}"/>"#));
        let order = ncx_items.len() + 1;
        nav_items.push(format!(
            r#"<li><a href="text/{page_name}">{heading}</a></li>"#
        ));
        ncx_items.push(format!(
            r#"<navPoint id="nav-{number}" playOrder="{order}"><navLabel><text>{heading}</text></navLabel><content src="text/{page_name}"/></navPoint>"#
        ));
    }

    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"zh-CN\" lang=\"zh-CN\">\n<head><meta charset=\"utf-8\"/><title>目录</title></head>\n<body><nav epub:type=\"toc\" id=\"toc\"><h1>目录</h1><ol>{}</ol></nav></body></html>\n",
        nav_items.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/nav.xhtml", nav.as_bytes());
    let depth = if ncx_items.is_empty() { 0 } else { 1 };
    let ncx = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n<head><meta name=\"dtb:uid\" content=\"{identifier}\"/><meta name=\"dtb:depth\" content=\"{depth}\"/></head>\n<docTitle><text>{title_xml}</text></docTitle>\n<navMap>{}</navMap>\n</ncx>\n",
        ncx_items.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/toc.ncx", ncx.as_bytes());

    let cover_meta = if cover.is_some() {
        r#"<meta name="cover" content="cover-image"/>"#
    } else {
        ""
    };
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"zh-CN\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"bookid\">{identifier}</dc:identifier><dc:title>{title_xml}</dc:title><dc:language>zh-CN</dc:language><dc:creator>{author_xml}</dc:creator>{subjects_xml}{description_xml}{series_meta}<meta property=\"dcterms:modified\">{modified}</meta>{cover_meta}</metadata>\n<manifest>{}</manifest>\n<spine toc=\"ncx\">{}</spine>\n</package>\n",
        manifest.join(""),
        spine.join("")
    );
    zip_push(&mut out, &mut items, "OEBPS/content.opf", opf.as_bytes());
    zip_finish(&mut out, &items);
    out
}

/// 合集取正文的那个文件：绑的是 txt / md / html 就用它本身；绑的是 `.epub` 这类
/// 电子书，就用同目录那份同名 `.txt`（程序自己维护的正文副本）；绑的是目录就在里面找一个 txt。
/// 找不到就返回 `None`，调用方把这篇记进「跳过」名单、别把整本书卡掉。
fn anthology_text_file(bound: &Path) -> Option<PathBuf> {
    if bound.is_dir() {
        let mut found = fs::read_dir(bound)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map(|extension| extension.eq_ignore_ascii_case("txt"))
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        found.sort();
        return found.into_iter().next();
    }
    if !bound.is_file() {
        return None;
    }
    let extension = bound
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(extension.as_str(), "txt" | "md" | "html" | "htm" | "xhtml") {
        return Some(bound.to_path_buf());
    }
    let beside = bound.with_extension("txt");
    beside.is_file().then_some(beside)
}

/// 把本地正文里 `[插图 N：xxx_images/003.jpg]` 这类标记编成配图槽位。
///
/// 只认**落盘后**的友好标记，不认 Pixiv 原始 token（`[uploadedimage:…]`）——
/// 后者在这份 txt 里没有对应文件，硬编个空槽位只会让正文里多一块破图。
///
/// 返回 `(槽位, 图片字节)`：槽位给 `render_novel_xhtml` 定位插图，字节给 zip 直接写。
/// 字节不塞进 `NovelImageSlot` 里 —— 那个结构在「下配图」那条路上按值传来传去，
/// 挂上几十兆图片会跟着复制。
fn anthology_image_slots(
    content: &str,
    text_path: &Path,
    chapter: usize,
) -> (Vec<NovelImageSlot>, Vec<(String, Vec<u8>)>) {
    let dir = novel_images_dir(text_path);
    let folder_name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut slots: Vec<NovelImageSlot> = Vec::new();
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut rest = content;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else {
            break;
        };
        let inner = &after[..end];
        rest = &after[end + 1..];
        let external = if inner.starts_with("引用插画") {
            true
        } else if inner.starts_with("插图") {
            false
        } else {
            continue;
        };
        let reference = inner
            .rsplit(['：', ':'])
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if reference.is_empty() {
            continue;
        }
        let file_name = reference
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&reference)
            .trim()
            .to_string();
        if file_name.is_empty() {
            continue;
        }
        let token = format!("[{inner}]");
        if !seen.insert(token.clone()) {
            continue;
        }
        let source = dir.join(&file_name);
        let Ok(bytes) = fs::read(&source) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let order = slots.len() + 1;
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("jpg");
        let packaged = format!("c{:02}_{:03}.{}", chapter + 1, order, extension);
        images.push((packaged.clone(), bytes));
        slots.push(NovelImageSlot {
            token,
            order,
            file_name: packaged,
            relative: format!("{folder_name}/{file_name}"),
            url: String::new(),
            external,
        });
    }
    (slots, images)
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
    /// 简介原文（HTML），HTML / EPUB 各自净化后写入。
    synopsis: &'a str,
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
                    synopsis: meta.synopsis,
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
                meta.synopsis,
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
    let sync_settings = read_settings(&conn)?;
    let image_quality = sync_settings.image_quality.clone();
    // 同步时就按设置生成阅读版：html 单网页，或者直接产出 epub
    let reading_format = reading_format_of(&sync_settings.sync_image_format);
    // `_threshold`（作者级的 match_threshold）在同步里已经不用了：复用本地预览版
    // 只认文件名完全相同，不再比相似度。留在 SELECT 里只是为了不改 tuple 的取列顺序。
    let (homepage, preview_dir, purchased_dir, _threshold, last_sync, cookie, author_name): (String, String, String, i64, String, String, String) = conn.query_row(
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
                "UPDATE works SET pixiv_novel_id=CASE WHEN pixiv_novel_id='' THEN ?1 ELSE pixiv_novel_id END, series_id=?2, series_title=?3, series_order=?4, has_images=CASE WHEN ?6=1 THEN 1 ELSE has_images END, synopsis=CASE WHEN ?7='' THEN synopsis ELSE ?7 END WHERE id=?5",
                params![novel_id, series_id, series_title, series_order, existing_id, title_indicates_images(&title) as i64, description],
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
        if let Some((preview_path, cover_path)) = matched_sync_preview(&preview_entries, &title) {
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
            if conn.execute("INSERT INTO works (author_id, title, release_date, preview_path, cover_path, purchased_path, tags, pixiv_novel_id, series_id, series_title, series_order, has_images, is_new, synopsis) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13)", params![author_id, title, date, preview_value, cover_value, purchased_value, tags, novel_id, series_id, series_title, series_order, title_indicates_images(&title) as i64, description]).is_err() {
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
            synopsis: description,
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
                // 封面在这里压完再落盘（见 compress_cover）：只写压缩版，不留原图。
                Ok(bytes) => compress_cover(&bytes),
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
                        synopsis: &work.synopsis,
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
            if conn.execute("INSERT INTO works (author_id, title, release_date, preview_path, cover_path, purchased_path, tags, pixiv_novel_id, series_id, series_title, series_order, has_images, image_count, is_new, synopsis) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, ?14)", params![author_id, work.title, work.release_date, preview_value, cover_path.to_string_lossy(), purchased_value, work.tags, work.novel_id, work.series_id, work.series_title, work.series_order, title_indicates_images(&work.title) as i64, image_saved as i64, work.synopsis]).is_err() {
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

// ==================== 补抓简介（v1.2.0） ====================

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SynopsisBackfillResult {
    total: usize,
    updated: usize,
    /// 请求成功、但作者就是没写简介 —— 这种永远补不出来，得跟「失败」分开报，
    /// 否则整批都是这种人时「已更新 0 篇」看着像卡住了。
    no_synopsis: usize,
    failed: usize,
    cancelled: bool,
    /// 连续失败太多，判定为被 Pixiv 限流、提前停下了
    throttled: bool,
}

/// 补抓前先问一下还有多少可抓的，省得「全是确认过没简介的」时白起一次浮层。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SynopsisBackfillStatus {
    /// 还没问过 Pixiv、可能补得出来的篇数
    pending: usize,
    /// 问过了、确认作者没写简介的篇数
    checked_no_synopsis: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SynopsisBackfillProgress {
    total: usize,
    current: usize,
    title: String,
    /// 预计还要多少秒（按当前间隔 × 剩余篇数估）
    eta_seconds: u64,
}

/// 补抓简介时的并发数，和同步抓详情保持一致（只在篇数没到阈值时才用并发）。
const SYNOPSIS_CONCURRENCY: usize = 6;
/// 连着失败这么多篇就先认为「可能被限流」，把间隔翻倍再说
const SYNOPSIS_THROTTLE_STREAK: usize = 4;
/// 连着失败到这个数就别硬撑了 —— 继续跑只是白白喂风控、白等
const SYNOPSIS_ABORT_STREAK: usize = 10;
/// 间隔翻倍的上限，别翻到天荒地老
const SYNOPSIS_MAX_DELAY_SECONDS: u64 = 60;

/// 连续失败之后的新间隔：翻倍，但封顶。
/// 抽成函数是为了能单测 —— 这段决定了被限流时是「等一等再试」还是「干脆停下」。
fn throttled_delay_seconds(current: u64) -> u64 {
    (current.max(1) * 2).min(SYNOPSIS_MAX_DELAY_SECONDS)
}

/// 浮层第二行的文案。顶上已经有「正在补抓简介」和「15 / 294」了，这里只报顶上没有的：
/// 更新了几篇、其中多少篇是作者根本没写简介（这类永远补不出来，不单独报的话
/// 整批都是这种人时「已更新 0 篇」看着就像卡死了）、以及当前间隔 / 失败数。
fn synopsis_progress_title(
    updated: usize,
    no_synopsis: usize,
    failed: usize,
    delay_seconds: Option<u64>,
) -> String {
    let mut text = format!("已更新 {updated} 篇 · 作者没写简介 {no_synopsis} 篇");
    if failed > 0 {
        text.push_str(&format!(" · 失败 {failed} 篇"));
    }
    match delay_seconds {
        Some(0) | None => text,
        Some(delay) if failed > 0 => format!("{text}，间隔已拉到 {delay} 秒"),
        Some(delay) => format!("{text} · 每篇间隔 {delay} 秒"),
    }
}

/// 补抓简介的「待办体检」：没问过的有多少篇、问过确认没简介的有多少篇。
/// 前端点按钮时先看一眼 —— 没得抓就别起浮层了。
#[tauri::command]
fn synopsis_backfill_status() -> Result<SynopsisBackfillStatus, String> {
    let conn = db()?;
    let pending: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM works WHERE pixiv_novel_id <> '' AND synopsis = '' AND synopsis_checked = 0",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let checked_no_synopsis: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM works WHERE synopsis = '' AND synopsis_checked = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(SynopsisBackfillStatus {
        pending: pending.max(0) as usize,
        checked_no_synopsis: checked_no_synopsis.max(0) as usize,
    })
}

/// 给老作品补抓 Pixiv 简介。`author_id = 0` 表示整库。
/// `recheck = true` 时连「上次问过、确认作者没写简介」的作品也再问一遍。
/// 取消标记复用同步那一套：整库补抓的键就是 0（前端调 `cancel_pixiv_sync(0)`）。
#[tauri::command]
async fn backfill_synopses(
    author_id: i64,
    recheck: Option<bool>,
    app: tauri::AppHandle,
) -> Result<SynopsisBackfillResult, String> {
    clear_pixiv_sync_cancel(author_id);
    let recheck = recheck.unwrap_or(false);
    let result = tauri::async_runtime::spawn_blocking(move || {
        backfill_synopses_impl(author_id, recheck, app)
    })
    .await
    .map_err(|e| e.to_string())?;
    clear_pixiv_sync_cancel(author_id);
    result
}

fn backfill_synopses_impl(
    author_id: i64,
    recheck: bool,
    app: tauri::AppHandle,
) -> Result<SynopsisBackfillResult, String> {
    let conn = db()?;
    let cookie = setting(&conn, "pixiv_cookie")?;
    let cookie = if cookie.trim().is_empty() {
        None
    } else {
        Some(normalize_pixiv_cookie(&cookie)?)
    };
    let client = pixiv_client(cookie)?;
    // author_id 是命令参数里的数字，直接拼进 SQL 不涉及注入；用参数化反而让
    // 「0 = 全部」这个分支要写两遍 query，不值得。
    let filter = if author_id > 0 {
        format!(" AND author_id = {author_id}")
    } else {
        String::new()
    };
    // 默认跳过「上次问过、确认作者没写简介」的（synopsis_checked=1）：
    // 那些再问一百遍也是空的，只会白等 + 白喂风控。
    let checked_filter = if recheck {
        ""
    } else {
        " AND synopsis_checked = 0"
    };
    let targets: Vec<(i64, String)> = {
        let sql = format!(
            "SELECT id, pixiv_novel_id FROM works WHERE pixiv_novel_id <> '' AND synopsis = ''{checked_filter}{filter} ORDER BY id"
        );
        let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    let total = targets.len();
    // 和「作品同步」共用同一套间隔设置（设置 → 抓取间隔：超过 N 篇时每篇间隔 M 秒）。
    // 全库补抓动辄上千篇，原来这边 6 并发一路猛冲、完全没吃设置，正是上次触发风控的原因。
    let delay_threshold = setting(&conn, "pixiv_delay_threshold")?
        .parse::<usize>()
        .unwrap_or(150);
    let base_delay = setting(&conn, "pixiv_delay_seconds")?
        .parse::<u64>()
        .unwrap_or(1);
    // 篇数没到阈值（或者把间隔设成 0）才用并发；到了阈值就一篇一篇来，中间 sleep
    let sequential = total > delay_threshold && base_delay > 0;
    let mut delay_seconds = base_delay;
    let mut updated = 0usize;
    let mut no_synopsis = 0usize;
    let mut failed = 0usize;
    let mut cancelled = false;
    let mut throttled = false;
    let mut streak = 0usize;

    if sequential {
        for (index, (work_id, novel_id)) in targets.iter().enumerate() {
            if pixiv_sync_cancelled(author_id) {
                cancelled = true;
                break;
            }
            if index > 0 && delay_seconds > 0 {
                std::thread::sleep(Duration::from_secs(delay_seconds));
            }
            let ok = match fetch_pixiv_novel_detail(&client, novel_id) {
                Ok(detail) if detail.get("error").and_then(Value::as_bool) != Some(true) => {
                    let body = detail.get("body").unwrap_or(&Value::Null);
                    let description = json_string(body, "description");
                    if description.trim().is_empty() {
                        // 作者没写简介不算失败，也别让它把「连续失败」的计数冲掉；
                        // 但要把「问过了、就是没有」记下来，下次别再白跑这一篇。
                        conn.execute(
                            "UPDATE works SET synopsis_checked=1 WHERE id=?1",
                            params![work_id],
                        )
                        .map_err(|e| e.to_string())?;
                        no_synopsis += 1;
                        true
                    } else {
                        conn.execute(
                            "UPDATE works SET synopsis=?1, synopsis_checked=1 WHERE id=?2",
                            params![description, work_id],
                        )
                        .map_err(|e| e.to_string())?;
                        updated += 1;
                        true
                    }
                }
                // 限流时 Pixiv 通常直接返回错误/403，全都归到失败里。
                // 失败不落 synopsis_checked —— 下次还得重试。
                _ => false,
            };
            if ok {
                streak = 0;
            } else {
                failed += 1;
                streak += 1;
                if streak % SYNOPSIS_THROTTLE_STREAK == 0 {
                    // 连着失败＝大概率被限流了：间隔翻倍；还继续失败就干脆停下
                    delay_seconds = throttled_delay_seconds(delay_seconds);
                    if streak >= SYNOPSIS_ABORT_STREAK {
                        throttled = true;
                        break;
                    }
                }
            }
            let current = index + 1;
            let remaining = total.saturating_sub(current) as u64;
            let _ = app.emit(
                "synopsis-backfill-progress",
                SynopsisBackfillProgress {
                    total,
                    current,
                    title: synopsis_progress_title(
                        updated,
                        no_synopsis,
                        failed,
                        Some(delay_seconds),
                    ),
                    eta_seconds: remaining * delay_seconds,
                },
            );
        }
    } else {
        for (batch_index, batch) in targets.chunks(SYNOPSIS_CONCURRENCY).enumerate() {
            if pixiv_sync_cancelled(author_id) {
                cancelled = true;
                break;
            }
            let fetched = std::thread::scope(|scope| {
                let handles = batch
                    .iter()
                    .map(|(work_id, novel_id)| {
                        let client = client.clone();
                        let novel_id = novel_id.clone();
                        let work_id = *work_id;
                        scope.spawn(move || {
                            let detail = fetch_pixiv_novel_detail(&client, &novel_id);
                            (work_id, detail)
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .filter_map(|handle| handle.join().ok())
                    .collect::<Vec<_>>()
            });
            for (work_id, detail) in fetched {
                match detail {
                    Ok(detail) if detail.get("error").and_then(Value::as_bool) != Some(true) => {
                        let body = detail.get("body").unwrap_or(&Value::Null);
                        let description = json_string(body, "description");
                        if description.trim().is_empty() {
                            conn.execute(
                                "UPDATE works SET synopsis_checked=1 WHERE id=?1",
                                params![work_id],
                            )
                            .map_err(|e| e.to_string())?;
                            no_synopsis += 1;
                            continue;
                        }
                        conn.execute(
                            "UPDATE works SET synopsis=?1, synopsis_checked=1 WHERE id=?2",
                            params![description, work_id],
                        )
                        .map_err(|e| e.to_string())?;
                        updated += 1;
                    }
                    _ => failed += 1,
                }
            }
            let current = ((batch_index + 1) * SYNOPSIS_CONCURRENCY).min(total);
            let _ = app.emit(
                "synopsis-backfill-progress",
                SynopsisBackfillProgress {
                    total,
                    current,
                    title: synopsis_progress_title(updated, no_synopsis, failed, None),
                    eta_seconds: 0,
                },
            );
        }
    }
    Ok(SynopsisBackfillResult {
        total,
        updated,
        no_synopsis,
        failed,
        cancelled,
        throttled,
    })
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

/// Cookie 体检结果。设置面板的「测试 Cookie」和启动自检共用这一份。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct PixivCookieProbe {
    /// 唯一需要判断的字段：true 才算通过
    ok: bool,
    /// `ok` / `missing` / `invalid` / `network` / `unknown` —— 前端按它决定说什么话
    status: String,
    /// 直接显示给用户的中文说明
    message: String,
    /// 登录账号名，拿得到才有
    user_name: Option<String>,
    /// 检测时刻（毫秒时间戳），前端自己格式化
    checked_at_ms: i64,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 拿一份 Cookie 去问 Pixiv「我现在是谁」。
///
/// `cookie` 传了就用传进来的那份（设置面板里刚粘贴、还没轮到自动保存的），
/// 没传就读数据库里存的那份 —— 这样「测完再存」和「存完再测」都是同一个答案。
#[tauri::command]
async fn check_pixiv_cookie(cookie: Option<String>) -> Result<PixivCookieProbe, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw = match cookie {
            Some(value) if !value.trim().is_empty() => value,
            _ => {
                let conn = db()?;
                setting(&conn, "pixiv_cookie")?
            }
        };
        if raw.trim().is_empty() {
            return Ok(PixivCookieProbe {
                ok: false,
                status: "missing".into(),
                message: "还没填 Pixiv Cookie —— 同步作品列表、正文和 R-18 内容都需要它。".into(),
                user_name: None,
                checked_at_ms: now_millis(),
            });
        }
        let normalized = normalize_pixiv_cookie(&raw)?;
        Ok(probe_pixiv_cookie(&normalized))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 从 PHPSESSID 里抠出账号 UID。Pixiv 的格式是 `<uid>_<随机串>`，前缀那串数字就是账号自己的 UID。
///
/// **只用来显示账号名**，不参与「Cookie 还有效吗」的判断 —— 抠不出来就当没有，
/// 不能让一个解析细节决定体检结论。
fn pixiv_uid_from_cookie(cookie: &str) -> Option<String> {
    cookie
        .split(';')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix("PHPSESSID="))
        .and_then(|value| value.split('_').next())
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

/// 用 UID 查一个**公开**接口拿账号名 —— 这个接口谁都能查，所以它只负责显示，
/// 判断登录态一律靠 `probe_pixiv_cookie` 里的 `/ajax/user/extra`。
/// 任何一步失败都返回 None：拿不到名字只该让提示少半句，不该让体检报错。
fn fetch_pixiv_user_name(client: &Client, uid: &str) -> Option<String> {
    let text = client
        .get(format!("https://www.pixiv.net/ajax/user/{uid}"))
        .send()
        .ok()?
        .text()
        .ok()?;
    let body: Value = serde_json::from_str(&text).ok()?;
    body.get("body")?
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// 用 `/ajax/user/extra` 探一次登录态 —— 这个接口必须带有效 Cookie 才回得出当前账号信息。
///
/// 判断口径是「HTTP 成功 + 接口没报 error」。**不要**去要求 body 里有 userId / name ——
/// 实测这个接口成功时的 body 只有 `{following, followers, mypixivCount, background}`，
/// 压根没有 userId（v1.2.15 第一版按「拿得到 userId」判，结果对每一份有效 Cookie 都报失效）。
/// 它不带参数返回的就是**当前登录账号自己**的关注数，未登录拿不到（400/401 + error:true），
/// 所以「200 且没报 error」本身就等于「这份 Cookie 是活的」。
/// 失败时把「网络不通」和「Cookie 失效」分开报 —— 这两件事该做的处理完全不同。
fn probe_pixiv_cookie(cookie: &str) -> PixivCookieProbe {
    let checked_at_ms = now_millis();
    let unknown = |message: String| PixivCookieProbe {
        ok: false,
        status: "unknown".into(),
        message,
        user_name: None,
        checked_at_ms,
    };
    // 失败原因不同，用户该做的事也不同，所以分开三段说；但结构体长得一模一样，统一从这里出
    let invalid = |message: &str| PixivCookieProbe {
        ok: false,
        status: "invalid".into(),
        message: message.into(),
        user_name: None,
        checked_at_ms,
    };
    let client = match pixiv_client(Some(cookie.to_string())) {
        Ok(client) => client,
        Err(error) => return unknown(error),
    };
    let response = match client.get("https://www.pixiv.net/ajax/user/extra").send() {
        Ok(response) => response,
        Err(error) => {
            return PixivCookieProbe {
                ok: false,
                status: "network".into(),
                message: format!(
                    "连不上 Pixiv（{error}）。这多半不是 Cookie 的问题，先确认浏览器/代理能打开 pixiv.net 再测一次。"
                ),
                user_name: None,
                checked_at_ms,
            };
        }
    };
    let status = response.status();
    let text = response.text().unwrap_or_default();
    let body: Option<Value> = serde_json::from_str(&text).ok();
    let Some(body) = body else {
        return unknown(format!(
            "Pixiv 回了 HTTP {}，但内容读不出来（可能是被拦截或接口改了）。稍后再试。",
            status.as_u16()
        ));
    };
    let api_error = body
        .get("error")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if status.is_success() && !api_error {
        // 名字只是「附赠」：拿 UID 再查一次公开接口，拿不到就少半句话，不影响结论
        let user_name =
            pixiv_uid_from_cookie(cookie).and_then(|uid| fetch_pixiv_user_name(&client, &uid));
        return PixivCookieProbe {
            ok: true,
            status: "ok".into(),
            message: match &user_name {
                Some(name) => format!("Cookie 有效，当前登录：{name}"),
                None => "Cookie 有效。".into(),
            },
            user_name,
            checked_at_ms,
        };
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return invalid("Pixiv 拒绝了这份 Cookie（登录已失效）。重新导出一次 PHPSESSID 再填进来。");
    }
    if status == reqwest::StatusCode::BAD_REQUEST {
        // 实测：一份已作废的 PHPSESSID 会走到这里（400，不是 401）
        return invalid("Pixiv 说这次请求里没有登录凭证 —— 这份 PHPSESSID 多半已经作废。重新导出一次再填进来。");
    }
    if status.is_success() {
        // 兜底：200 但接口自己说 error，同样按失效算
        return invalid("Pixiv 认出这是未登录状态 —— 这份 Cookie 已经不能用。重新导出一次 PHPSESSID 再填进来。");
    }
    unknown(format!(
        "没测出结果：Pixiv 返回了 HTTP {}。稍后再试，或先确认浏览器里能正常打开 pixiv.net。",
        status.as_u16()
    ))
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
    /// 简介原文（HTML）。优先用库里存的，缺了就拿 Pixiv 这次返回的补。
    synopsis: String,
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
    let (novel_id, title, preview_path, purchased_path, release_date, tags, author_name, stored_synopsis):
        (String, String, String, String, String, String, String, String) = conn
        .query_row(
            "SELECT w.pixiv_novel_id, w.title, w.preview_path, w.purchased_path, w.release_date, w.tags, a.name, w.synopsis FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id=?1",
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
                    row.get(7)?,
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
    // 简介优先用库里那份（和界面上看到的一致）；老库里没存过就用这次 Pixiv 返回的补上，
    // 但**不回头写库** —— 导出是只读操作，别在这儿偷偷改作品数据。
    let synopsis = if stored_synopsis.trim().is_empty() {
        json_string(body, "description")
    } else {
        stored_synopsis
    };
    Ok((
        NovelAssets {
            novel_id,
            title,
            author_name,
            release_date,
            tags,
            synopsis,
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
            let _ = fs::write(&assets.cover_path, compress_cover(&bytes));
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
                    synopsis: &assets.synopsis,
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
                &assets.synopsis,
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
    write_bytes_atomic(target, &compress_cover(&bytes))?;
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

/// 阅读版（HTML / EPUB）的落盘路径；没生成过就返回空串。
/// 详情弹窗的「文件」块用它列一行「阅读版」—— 比按扩展名猜有没有阅读版准得多。
#[tauri::command]
fn work_reading_path(work_id: i64) -> Result<String, String> {
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
        return Ok(String::new());
    }
    let settings = read_settings(&conn)?;
    let preferred = reading_format_of(&settings.sync_image_format);
    let text_path = Path::new(&path);
    for format in [preferred, preferred.other()] {
        let candidate = reading_output_path(text_path, format, "");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().to_string());
        }
    }
    Ok(String::new())
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
            Ok(()) => {
                let _ = mark_work_in_progress(&conn, work_id);
                let _ = record_history(&conn, work_id);
                Ok(())
            }
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

// ============================ 收藏夹与浏览历史（v1.1.0） ============================

/// 收藏夹名字：去掉首尾空白、不许空、给个长度上限（侧栏卡片放不下太长的）。
fn clean_collection_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("收藏夹名字不能为空".into());
    }
    if name.chars().count() > 24 {
        return Err("收藏夹名字最多 24 个字".into());
    }
    Ok(name.to_string())
}

/// `works.favorite` 是「在任意收藏夹里」的缓存位：作者卡上的收藏数、各页的
/// 「仅看收藏」筛选都还读它，所以每次动过收藏夹成员都要回写一次。
fn sync_work_favorite(conn: &Connection, work_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE works SET favorite = (SELECT COUNT(*) FROM collection_works WHERE work_id=?1) > 0 WHERE id=?1",
        [work_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn collection_summary(conn: &Connection, id: i64) -> Result<CollectionSummary, String> {
    conn.query_row(
        "SELECT c.id, c.name, c.created_at,
                (SELECT COUNT(*) FROM collection_works cw WHERE cw.collection_id = c.id),
                COALESCE((SELECT w.cover_path FROM collection_works cw JOIN works w ON w.id = cw.work_id
                          WHERE cw.collection_id = c.id AND w.cover_path <> ''
                          ORDER BY cw.added_at DESC LIMIT 1), '')
         FROM collections c WHERE c.id = ?1",
        [id],
        |row| {
            Ok(CollectionSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                work_count: row.get(3)?,
                cover_path: row.get(4)?,
            })
        },
    )
    .map_err(|e| format!("没找到这个收藏夹：{e}"))
}

const COLLECTION_SELECT: &str = "SELECT c.id, c.name, c.created_at,
    (SELECT COUNT(*) FROM collection_works cw WHERE cw.collection_id = c.id),
    COALESCE((SELECT w.cover_path FROM collection_works cw JOIN works w ON w.id = cw.work_id
              WHERE cw.collection_id = c.id AND w.cover_path <> ''
              ORDER BY cw.added_at DESC LIMIT 1), '')
 FROM collections c";

fn map_collection(row: &rusqlite::Row<'_>) -> rusqlite::Result<CollectionSummary> {
    Ok(CollectionSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        work_count: row.get(3)?,
        cover_path: row.get(4)?,
    })
}

#[tauri::command]
fn list_collections() -> Result<Vec<CollectionSummary>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare(&format!(
            "{COLLECTION_SELECT} ORDER BY c.sort_order ASC, c.id ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], map_collection)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn create_collection(name: String) -> Result<CollectionSummary, String> {
    let name = clean_collection_name(&name)?;
    let conn = db()?;
    let used: i64 = conn
        .query_row("SELECT COUNT(*) FROM collections WHERE name=?1", [&name], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    if used > 0 {
        return Err(format!("已经有一个叫「{name}」的收藏夹了"));
    }
    let order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM collections",
            [],
            |row| row.get(0),
        )
        .unwrap_or(1);
    conn.execute(
        "INSERT INTO collections (name, sort_order, created_at) VALUES (?1, ?2, ?3)",
        params![name, order, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    collection_summary(&conn, conn.last_insert_rowid())
}

#[tauri::command]
fn rename_collection(id: i64, name: String) -> Result<CollectionSummary, String> {
    let name = clean_collection_name(&name)?;
    let conn = db()?;
    let used: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM collections WHERE name=?1 AND id<>?2",
            params![name, id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if used > 0 {
        return Err(format!("已经有一个叫「{name}」的收藏夹了"));
    }
    conn.execute(
        "UPDATE collections SET name=?1 WHERE id=?2",
        params![name, id],
    )
    .map_err(|e| e.to_string())?;
    collection_summary(&conn, id)
}

#[tauri::command]
fn delete_collection(id: i64) -> Result<(), String> {
    let conn = db()?;
    // 先记下夹子里的作品：删完要回写它们的 favorite 缓存位
    let mut statement = conn
        .prepare("SELECT work_id FROM collection_works WHERE collection_id=?1")
        .map_err(|e| e.to_string())?;
    let work_ids = statement
        .query_map([id], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM collections WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    for work_id in work_ids {
        sync_work_favorite(&conn, work_id)?;
    }
    Ok(())
}

/// 这篇作品现在在哪几个收藏夹里（弹窗里勾选状态就靠它）。
#[tauri::command]
fn work_collections(work_id: i64) -> Result<Vec<i64>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT collection_id FROM collection_works WHERE work_id=?1")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([work_id], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 一次性替换这篇作品在各收藏夹里的归属（弹窗里勾完点确定走的就是这条路）。
#[tauri::command]
fn set_work_collections(work_id: i64, collection_ids: Vec<i64>) -> Result<(), String> {
    let mut conn = db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM collection_works WHERE work_id=?1", [work_id])
        .map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();
    for collection_id in collection_ids {
        tx.execute(
            "INSERT OR IGNORE INTO collection_works (collection_id, work_id, added_at) VALUES (?1, ?2, ?3)",
            params![collection_id, work_id, now],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute(
        "UPDATE works SET favorite = (SELECT COUNT(*) FROM collection_works WHERE work_id=?1) > 0 WHERE id=?1",
        [work_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 筛选视图（v1.2.9）
//
// 存的只是**条件**，不是作品 —— 和收藏夹是两码事。`payload` 里是一份 JSON，
// 后端不解释它（前端加一档筛选项不用改库），只保证名字唯一、长度别太离谱。
// 这样做的代价是「条件口径变了老视图会失效」，但换来的是加筛选条件不用迁移。
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilterView {
    id: i64,
    name: String,
    /// 筛选条件的 JSON 原文（阅读状态 / 评分 / 字数 / 收藏夹 / 搜索词 / 范围 / 排序…）
    payload: String,
    created_at: String,
}

const FILTER_VIEW_MAX_NAME: usize = 30;

fn clean_filter_view_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("视图名字不能空着".into());
    }
    if name.chars().count() > FILTER_VIEW_MAX_NAME {
        return Err(format!("视图名字最多 {FILTER_VIEW_MAX_NAME} 个字"));
    }
    Ok(name.to_string())
}

fn filter_view_by_id(conn: &Connection, id: i64) -> Result<FilterView, String> {
    conn.query_row(
        "SELECT id, name, payload, created_at FROM filter_views WHERE id=?1",
        [id],
        |row| {
            Ok(FilterView {
                id: row.get(0)?,
                name: row.get(1)?,
                payload: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .map_err(|e| format!("没找到这个筛选视图：{e}"))
}

#[tauri::command]
fn list_filter_views() -> Result<Vec<FilterView>, String> {
    let conn = db()?;
    let mut statement = conn
        .prepare("SELECT id, name, payload, created_at FROM filter_views ORDER BY sort_order ASC, id ASC")
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(FilterView {
                id: row.get(0)?,
                name: row.get(1)?,
                payload: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_filter_view(name: String, payload: String) -> Result<FilterView, String> {
    let name = clean_filter_view_name(&name)?;
    let conn = db()?;
    let used: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM filter_views WHERE name=?1",
            [&name],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if used > 0 {
        return Err(format!("已经有一个叫「{name}」的筛选视图了"));
    }
    let order: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM filter_views",
            [],
            |row| row.get(0),
        )
        .unwrap_or(1);
    conn.execute(
        "INSERT INTO filter_views (name, payload, sort_order, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![name, payload, order, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    filter_view_by_id(&conn, conn.last_insert_rowid())
}

#[tauri::command]
fn rename_filter_view(id: i64, name: String) -> Result<FilterView, String> {
    let name = clean_filter_view_name(&name)?;
    let conn = db()?;
    let used: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM filter_views WHERE name=?1 AND id<>?2",
            params![name, id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if used > 0 {
        return Err(format!("已经有一个叫「{name}」的筛选视图了"));
    }
    conn.execute(
        "UPDATE filter_views SET name=?1 WHERE id=?2",
        params![name, id],
    )
    .map_err(|e| e.to_string())?;
    filter_view_by_id(&conn, id)
}

/// 用当前条件覆盖已有视图（「更新为当前条件」）—— 名字保持不变。
#[tauri::command]
fn update_filter_view(id: i64, payload: String) -> Result<FilterView, String> {
    let conn = db()?;
    let changed = conn
        .execute(
            "UPDATE filter_views SET payload=?1 WHERE id=?2",
            params![payload, id],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("没找到这个筛选视图".into());
    }
    filter_view_by_id(&conn, id)
}

#[tauri::command]
fn delete_filter_view(id: i64) -> Result<(), String> {
    let conn = db()?;
    conn.execute("DELETE FROM filter_views WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 批量把作品**加进**若干收藏夹（并集，不覆盖已有归属）——
/// 批量操作里用。单篇的「一次性替换」走 set_work_collections。
#[tauri::command]
fn add_works_to_collections(work_ids: Vec<i64>, collection_ids: Vec<i64>) -> Result<usize, String> {
    add_works_to_collections_impl(&mut db()?, &work_ids, &collection_ids)
}

fn add_works_to_collections_impl(
    conn: &mut Connection,
    work_ids: &[i64],
    collection_ids: &[i64],
) -> Result<usize, String> {
    if work_ids.is_empty() || collection_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();
    for work_id in work_ids {
        for collection_id in collection_ids {
            tx.execute(
                "INSERT OR IGNORE INTO collection_works (collection_id, work_id, added_at) VALUES (?1, ?2, ?3)",
                params![collection_id, work_id, now],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "UPDATE works SET favorite = (SELECT COUNT(*) FROM collection_works WHERE work_id=?1) > 0 WHERE id=?1",
            [work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(work_ids.len())
}

#[tauri::command]
fn list_collection_works(
    collection_id: i64,
    query: String,
    search_field: String,
    status: String,
    images_filter: String,
    also_in_collection_id: i64,
    sort: String,
) -> Result<Vec<Work>, String> {
    let conn = db()?;
    let condition = search_match_clause(&search_field, "w.", "?2", "?3");
    let mut sql = format!("SELECT {WORK_COLUMNS_W} FROM collection_works cw JOIN works w ON w.id=cw.work_id JOIN authors a ON a.id=w.author_id WHERE cw.collection_id=?1 AND {condition}");
    match status.as_str() {
        "purchased" => sql.push_str(" AND w.purchased_path <> ''"),
        "unpurchased" => sql.push_str(" AND w.purchased_path = ''"),
        _ => {}
    }
    sql.push_str(&images_filter_clause(&images_filter, "w."));
    // 这个视图本身已经锁在一个夹子里了，面板里再选一个就是「同时也在那个夹子里」的收窄
    sql.push_str(&collection_filter_clause("w.id", "cw2", "?4"));
    sql.push_str(match sort.as_str() {
        "date_asc" => " ORDER BY w.release_date ASC, w.id ASC",
        "title_asc" => " ORDER BY w.title COLLATE NOCASE ASC",
        "date_desc" => " ORDER BY w.release_date DESC, w.id DESC",
        // 默认按「什么时候收进来的」：收藏夹里最自然的就是新收的排前面
        _ => " ORDER BY cw.added_at DESC, w.id DESC",
    });
    let raw_query = query.trim();
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(
            params![
                collection_id,
                raw_query,
                format!("%{raw_query}%"),
                also_in_collection_id
            ],
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

/// 打开作品 / 阅读版时记一笔。设置里关掉「记录浏览历史」就完全不写。
fn record_history(conn: &Connection, work_id: i64) -> Result<(), String> {
    if !read_settings(conn)?.record_history {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO work_history (work_id, viewed_at, view_count) VALUES (?1, ?2, 1)
         ON CONFLICT(work_id) DO UPDATE SET viewed_at=excluded.viewed_at, view_count=work_history.view_count + 1",
        params![work_id, now],
    )
    .map_err(|e| e.to_string())?;
    // 超出上限就从最旧的开始丢（`LIMIT -1 OFFSET n` = 取第 n 条之后的全部）
    conn.execute(
        "DELETE FROM work_history WHERE work_id IN (
            SELECT work_id FROM work_history ORDER BY viewed_at DESC LIMIT -1 OFFSET ?1
         )",
        [HISTORY_LIMIT],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_history(query: String, limit: i64) -> Result<Vec<HistoryEntry>, String> {
    let conn = db()?;
    let raw_query = query.trim().to_string();
    let limit = if limit <= 0 {
        HISTORY_LIMIT
    } else {
        limit.min(HISTORY_LIMIT)
    };
    let mut statement = conn
        .prepare(&format!("SELECT {WORK_COLUMNS_W}, h.viewed_at, h.view_count FROM work_history h JOIN works w ON w.id=h.work_id JOIN authors a ON a.id=w.author_id WHERE (?1='' OR w.title LIKE ?2 OR w.tags LIKE ?2) ORDER BY h.viewed_at DESC LIMIT ?3"))
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![raw_query, format!("%{raw_query}%"), limit], |row| {
            Ok(HistoryEntry {
                work: map_work(row)?,
                viewed_at: row.get(HISTORY_VIEWED_AT_INDEX)?,
                view_count: row.get(HISTORY_VIEWED_AT_INDEX + 1)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut entries = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for entry in &mut entries {
        populate_work_display_info(&mut entry.work);
    }
    Ok(entries)
}

#[tauri::command]
fn clear_history() -> Result<(), String> {
    db()?
        .execute("DELETE FROM work_history", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn remove_history(work_id: i64) -> Result<(), String> {
    db()?
        .execute("DELETE FROM work_history WHERE work_id=?1", [work_id])
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

// ============ 作品个人元数据：阅读状态 / 评分 / 笔记（v1.2.0） ============

/// 阅读状态：0 = 未读，1 = 在读，2 = 已读。
const READ_UNREAD: i64 = 0;
const READ_IN_PROGRESS: i64 = 1;
const READ_DONE: i64 = 2;
/// 笔记长度上限：卡片和详情弹窗都放不下更长的东西，写长了也没人看。
const NOTE_MAX_CHARS: usize = 200;

/// 打开作品时把它从「未读」推进到「在读」。
/// **绝不覆盖「已读」** —— 读过的好书再打开一次，不该又变回在读。
fn mark_work_in_progress(conn: &Connection, work_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE works SET read_state=?1 WHERE id=?2 AND read_state=?3",
        params![READ_IN_PROGRESS, work_id, READ_UNREAD],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 一次性写回评分 / 阅读状态 / 笔记。三个值都传 `None` 就什么都不改。
/// 前端每次只改一项，另两项传当前值即可。
#[tauri::command]
fn set_work_meta(
    work_id: i64,
    read_state: Option<i64>,
    rating: Option<i64>,
    note: Option<String>,
) -> Result<(), String> {
    let conn = db()?;
    if let Some(state) = read_state {
        if !(READ_UNREAD..=READ_DONE).contains(&state) {
            return Err("阅读状态只能是 0 / 1 / 2".into());
        }
        conn.execute(
            "UPDATE works SET read_state=?1 WHERE id=?2",
            params![state, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(value) = rating {
        if !(0..=5).contains(&value) {
            return Err("评分只能是 0-5".into());
        }
        conn.execute(
            "UPDATE works SET rating=?1 WHERE id=?2",
            params![value, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(text) = note {
        let text = text.trim().to_string();
        if text.chars().count() > NOTE_MAX_CHARS {
            return Err(format!("笔记最多 {NOTE_MAX_CHARS} 个字"));
        }
        conn.execute(
            "UPDATE works SET note=?1 WHERE id=?2",
            params![text, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 批量设置阅读状态（「设已读 / 设未读」用）。
#[tauri::command]
fn set_works_read_state(work_ids: Vec<i64>, read_state: i64) -> Result<usize, String> {
    if !(READ_UNREAD..=READ_DONE).contains(&read_state) {
        return Err("阅读状态只能是 0 / 1 / 2".into());
    }
    if work_ids.is_empty() {
        return Ok(0);
    }
    let mut conn = db()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for work_id in &work_ids {
        tx.execute(
            "UPDATE works SET read_state=?1 WHERE id=?2",
            params![read_state, work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(work_ids.len())
}

/// 批量改标签：`add` 里的标签追加进去，`remove` 里的标签摘掉。
/// **只做追加与移除，不做整体替换** —— 手滑一次就会毁掉一批作品的标签。
#[tauri::command]
fn update_works_tags(work_ids: Vec<i64>, add: String, remove: String) -> Result<usize, String> {
    update_works_tags_impl(&mut db()?, &work_ids, &add, &remove)
}

fn update_works_tags_impl(
    conn: &mut Connection,
    work_ids: &[i64],
    add: &str,
    remove: &str,
) -> Result<usize, String> {
    let add_tags = split_tags(add);
    let remove_tags = split_tags(remove);
    if add_tags.is_empty() && remove_tags.is_empty() {
        return Err("没填要追加或要移除的标签".into());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for work_id in work_ids {
        let current: String = tx
            .query_row("SELECT tags FROM works WHERE id=?1", [work_id], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        let mut tags = split_tags(&current);
        let before = tags.clone();
        for tag in &add_tags {
            if !tags.iter().any(|existing| existing.eq_ignore_ascii_case(tag)) {
                tags.push(tag.clone());
            }
        }
        tags.retain(|tag| {
            !remove_tags
                .iter()
                .any(|remove| remove.eq_ignore_ascii_case(tag))
        });
        if tags != before {
            tx.execute(
                "UPDATE works SET tags=?1 WHERE id=?2",
                params![tags.join("| "), work_id],
            )
            .map_err(|e| e.to_string())?;
            changed += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed)
}

/// 标签串（`|` 分隔）拆成单个标签，去空白、去空项。
fn split_tags(raw: &str) -> Vec<String> {
    raw.split('|')
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect()
}

// ============ 文件体检：失效关联 + 磁盘占用（v1.2.0） ============

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MissingFileEntry {
    work_id: i64,
    title: String,
    /// `preview` = 预览版绑定失效，`purchased` = 完整版绑定失效
    kind: String,
    path: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AuthorDiskUsage {
    author_id: i64,
    author_name: String,
    bytes: u64,
    file_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkFileReport {
    /// 检查了多少篇「有绑定文件」的作品
    checked: usize,
    missing: Vec<MissingFileEntry>,
    total_bytes: u64,
    total_files: usize,
    authors: Vec<AuthorDiskUsage>,
}

/// 一篇作品实际占用的文件：绑定的正文 + 封面 + 已经生成好的阅读版。
/// 去重过，避免正文和阅读版指向同一个文件时被算两次。
fn work_file_paths(work: &Work) -> Vec<PathBuf> {
    let mut candidates = vec![
        work.purchased_path.clone(),
        work.preview_path.clone(),
        work.cover_path.clone(),
    ];
    let text_path = if work.purchased_path.trim().is_empty() {
        &work.preview_path
    } else {
        &work.purchased_path
    };
    if !text_path.trim().is_empty() {
        let text = Path::new(text_path);
        for format in [ReadingFormat::Html, ReadingFormat::Epub] {
            let candidate = reading_output_path(text, format, "");
            if candidate.is_file() {
                candidates.push(candidate.to_string_lossy().to_string());
            }
        }
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = PathBuf::from(trimmed);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// 整库的作品（带作者名），列顺序与 WORK_COLUMNS_W 一致。
fn all_works(conn: &Connection) -> Result<Vec<Work>, String> {
    let sql = format!(
        "SELECT {WORK_COLUMNS_W} FROM works w JOIN authors a ON a.id=w.author_id ORDER BY w.id"
    );
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement.query_map([], map_work).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 体检：逐个 stat 绑定的文件，找出「数据库里还指着、硬盘上已经没了」的关联；
/// 顺便统计整库与各作者的磁盘占用。
fn scan_work_files_impl(conn: &Connection) -> Result<WorkFileReport, String> {
    let works = all_works(conn)?;
    let mut missing: Vec<MissingFileEntry> = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_files = 0usize;
    let mut per_author: HashMap<i64, (String, u64, usize)> = HashMap::new();
    let mut checked = 0usize;
    for work in &works {
        let bound = [
            ("preview", work.preview_path.as_str()),
            ("purchased", work.purchased_path.as_str()),
        ];
        if bound.iter().any(|(_, path)| !path.trim().is_empty()) {
            checked += 1;
        }
        for (kind, path) in bound {
            if path.trim().is_empty() {
                continue;
            }
            if !Path::new(path).exists() {
                missing.push(MissingFileEntry {
                    work_id: work.id,
                    title: work.title.clone(),
                    kind: kind.to_string(),
                    path: path.to_string(),
                });
            }
        }
        for path in work_file_paths(work) {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            total_bytes += meta.len();
            total_files += 1;
            let entry = per_author
                .entry(work.author_id)
                .or_insert_with(|| (work.author_name.clone(), 0, 0));
            entry.1 += meta.len();
            entry.2 += 1;
        }
    }
    let mut authors: Vec<AuthorDiskUsage> = per_author
        .into_iter()
        .map(
            |(author_id, (author_name, bytes, file_count))| AuthorDiskUsage {
                author_id,
                author_name,
                bytes,
                file_count,
            },
        )
        .collect();
    authors.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    Ok(WorkFileReport {
        checked,
        missing,
        total_bytes,
        total_files,
        authors,
    })
}

#[tauri::command]
fn scan_work_files() -> Result<WorkFileReport, String> {
    scan_work_files_impl(&db()?)
}

/// 把已经失效的绑定清掉，作品回到「未关联」状态。
/// **只改数据库里的路径，绝不动硬盘上的任何文件** —— 你自己挪走的文件不该被这里删掉。
/// 这里重新扫一遍再清（不用前端传来的清单），避免清单过期导致误清。
#[tauri::command]
fn clear_missing_bindings() -> Result<usize, String> {
    let mut conn = db()?;
    let report = scan_work_files_impl(&conn)?;
    if report.missing.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut cleared = 0usize;
    for entry in &report.missing {
        // column 只可能是这两个写死的值，不来自外部输入
        let column = if entry.kind == "purchased" {
            "purchased_path"
        } else {
            "preview_path"
        };
        tx.execute(
            &format!("UPDATE works SET {column}='' WHERE id=?1"),
            [entry.work_id],
        )
        .map_err(|e| e.to_string())?;
        cleared += 1;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(cleared)
}

// ============ 导出作品清单：CSV / Markdown（v1.2.0） ============

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    written: usize,
    path: String,
}

/// 导出列（顺序即 CSV 列顺序，Markdown 也按这个取下标）。
const EXPORT_HEADERS: [&str; 18] = [
    "标题",
    "作者",
    "发布日期",
    "字数",
    "格式",
    "标签",
    "系列",
    "版本状态",
    "收藏夹",
    "评分",
    "阅读状态",
    "笔记",
    "Pixiv 链接",
    "正文文件路径",
    "封面路径",
    "作品 ID",
    "配图张数",
    "简介",
];

// 导出列的位置：CSV 和 Markdown 吃的是同一份行数据，两边必须对齐。
// 一律用这些名字，别在下面写裸数字 —— 行号写死过一次，读错列还不报错。
const EXPORT_COL_TITLE: usize = 0;
const EXPORT_COL_AUTHOR: usize = 1;
const EXPORT_COL_DATE: usize = 2;
const EXPORT_COL_WORDS: usize = 3;
const EXPORT_COL_FORMAT: usize = 4;
const EXPORT_COL_TAGS: usize = 5;
const EXPORT_COL_SERIES: usize = 6;
const EXPORT_COL_VERSION: usize = 7;
const EXPORT_COL_COLLECTIONS: usize = 8;
const EXPORT_COL_RATING: usize = 9;
const EXPORT_COL_READ: usize = 10;
const EXPORT_COL_NOTE: usize = 11;
const EXPORT_COL_PIXIV_URL: usize = 12;
const EXPORT_COL_TEXT_PATH: usize = 13;
const EXPORT_COL_COVER_PATH: usize = 14;
const EXPORT_COL_NOVEL_ID: usize = 15;
const EXPORT_COL_IMAGES: usize = 16;
const EXPORT_COL_SYNOPSIS: usize = 17;

fn read_state_label(state: i64) -> &'static str {
    match state {
        READ_DONE => "已读",
        READ_IN_PROGRESS => "在读",
        _ => "未读",
    }
}

/// work_id → 它所在的收藏夹名字列表（按收藏夹自己的顺序）。
fn collection_names_by_work(conn: &Connection) -> Result<HashMap<i64, Vec<String>>, String> {
    let mut statement = conn
        .prepare(
            "SELECT cw.work_id, c.name FROM collection_works cw JOIN collections c ON c.id=cw.collection_id ORDER BY c.sort_order ASC, c.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for row in rows {
        let (work_id, name) = row.map_err(|e| e.to_string())?;
        map.entry(work_id).or_default().push(name);
    }
    Ok(map)
}

fn export_rows(conn: &Connection, works: Vec<Work>) -> Result<Vec<Vec<String>>, String> {
    let collections = collection_names_by_work(conn)?;
    let mut rows = Vec::with_capacity(works.len());
    for mut work in works {
        populate_work_display_info(&mut work);
        let series = if work.series_title.trim().is_empty() {
            String::new()
        } else if work.series_order > 0 {
            format!("{}（第 {} 篇）", work.series_title, work.series_order)
        } else {
            work.series_title.clone()
        };
        let pixiv_url = if work.pixiv_novel_id.trim().is_empty() {
            String::new()
        } else {
            format!(
                "https://www.pixiv.net/novel/show.php?id={}",
                work.pixiv_novel_id
            )
        };
        // 本地正文文件：完整版用 purchased，预览版只有 preview —— 导出时给个能直接双击的路径
        let text_path = if work.purchased_path.trim().is_empty() {
            work.preview_path.clone()
        } else {
            work.purchased_path.clone()
        };
        rows.push(vec![
            work.title.clone(),
            work.author_name.clone(),
            work.release_date.clone(),
            work.word_count.map(|count| count.to_string()).unwrap_or_default(),
            work.file_format.clone().unwrap_or_default(),
            split_tags(&work.tags).join("、"),
            series,
            if work.purchased_path.trim().is_empty() {
                "预览版".to_string()
            } else {
                "完整版".to_string()
            },
            collections
                .get(&work.id)
                .map(|names| names.join("、"))
                .unwrap_or_default(),
            if work.rating > 0 {
                format!("{} 星", work.rating)
            } else {
                String::new()
            },
            read_state_label(work.read_state).to_string(),
            work.note.replace('\n', " ").replace('\r', " "),
            pixiv_url,
            text_path,
            work.cover_path.clone(),
            work.pixiv_novel_id.clone(),
            if work.image_count > 0 {
                work.image_count.to_string()
            } else {
                String::new()
            },
            synopsis_plain_text(&work.synopsis),
        ]);
    }
    Ok(rows)
}

fn write_export_csv(path: &str, rows: &[Vec<String>]) -> Result<(), String> {
    // UTF-8 BOM：不加的话 Excel 打开中文全是乱码 —— 这是导 CSV 最容易漏的一步
    let mut buffer: Vec<u8> = vec![0xEF, 0xBB, 0xBF];
    {
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::CRLF)
            .from_writer(&mut buffer);
        writer
            .write_record(EXPORT_HEADERS)
            .map_err(|e| e.to_string())?;
        for row in rows {
            writer.write_record(row).map_err(|e| e.to_string())?;
        }
        writer.flush().map_err(|e| e.to_string())?;
    }
    fs::write(path, buffer).map_err(|e| format!("写入 CSV 失败：{e}"))
}

fn write_export_markdown(path: &str, rows: &[Vec<String>]) -> Result<(), String> {
    let mut text = format!(
        "# 作品清单\n\n共 {} 篇 · 导出于 {}\n",
        rows.len(),
        Utc::now().format("%Y-%m-%d %H:%M")
    );
    for row in rows {
        let get = |index: usize| row.get(index).map(String::as_str).unwrap_or("");
        text.push_str(&format!("\n## {}\n\n", get(EXPORT_COL_TITLE)));
        let mut facts: Vec<String> = Vec::new();
        if !get(EXPORT_COL_AUTHOR).trim().is_empty() {
            facts.push(format!("作者：{}", get(EXPORT_COL_AUTHOR)));
        }
        if !get(EXPORT_COL_DATE).trim().is_empty() {
            facts.push(format!("发布：{}", get(EXPORT_COL_DATE)));
        }
        if !get(EXPORT_COL_WORDS).trim().is_empty() {
            facts.push(format!("字数：{}", get(EXPORT_COL_WORDS)));
        }
        if !get(EXPORT_COL_FORMAT).trim().is_empty() {
            facts.push(format!("格式：{}", get(EXPORT_COL_FORMAT)));
        }
        if !get(EXPORT_COL_IMAGES).trim().is_empty() {
            facts.push(format!("配图：{} 张", get(EXPORT_COL_IMAGES)));
        }
        facts.push(get(EXPORT_COL_VERSION).to_string());
        if !get(EXPORT_COL_SERIES).trim().is_empty() {
            facts.push(get(EXPORT_COL_SERIES).to_string());
        }
        if !get(EXPORT_COL_COLLECTIONS).trim().is_empty() {
            facts.push(format!("收藏夹：{}", get(EXPORT_COL_COLLECTIONS)));
        }
        if !get(EXPORT_COL_RATING).trim().is_empty() {
            facts.push(get(EXPORT_COL_RATING).to_string());
        }
        facts.push(get(EXPORT_COL_READ).to_string());
        facts.retain(|fact| !fact.trim().is_empty());
        text.push_str(&format!("{}\n", facts.join(" · ")));
        if !get(EXPORT_COL_TAGS).trim().is_empty() {
            text.push_str(&format!("\n标签：{}\n", get(EXPORT_COL_TAGS)));
        }
        if !get(EXPORT_COL_NOTE).trim().is_empty() {
            text.push_str(&format!("\n> {}\n", get(EXPORT_COL_NOTE)));
        }
        // 简介按段落引起来，段落之间的换行不能丢
        if !get(EXPORT_COL_SYNOPSIS).trim().is_empty() {
            text.push_str("\n简介：\n\n");
            for line in get(EXPORT_COL_SYNOPSIS).lines().filter(|line| !line.trim().is_empty()) {
                text.push_str(&format!("> {line}\n"));
            }
        }
        // 本地文件与 Pixiv 页都留一句，导出来的清单能直接当索引用
        text.push_str("\n");
        if !get(EXPORT_COL_PIXIV_URL).trim().is_empty() {
            text.push_str(&format!("[Pixiv 原页]({})", get(EXPORT_COL_PIXIV_URL)));
            if !get(EXPORT_COL_NOVEL_ID).trim().is_empty() {
                text.push_str(&format!("（ID {}）", get(EXPORT_COL_NOVEL_ID)));
            }
            text.push_str("\n");
        } else if !get(EXPORT_COL_NOVEL_ID).trim().is_empty() {
            text.push_str(&format!("Pixiv ID：{}\n", get(EXPORT_COL_NOVEL_ID)));
        }
        if !get(EXPORT_COL_TEXT_PATH).trim().is_empty() {
            text.push_str(&format!(
                "\n正文：`{}`\n",
                get(EXPORT_COL_TEXT_PATH).replace('`', "'")
            ));
        }
        if !get(EXPORT_COL_COVER_PATH).trim().is_empty() {
            text.push_str(&format!(
                "封面：`{}`\n",
                get(EXPORT_COL_COVER_PATH).replace('`', "'")
            ));
        }
    }
    fs::write(path, text).map_err(|e| format!("写入 Markdown 失败：{e}"))
}

/// 合集 EPUB 的导出进度（复用阅读版那套浮层，另起一个事件名免得两边串台）。
const ANTHOLOGY_PROGRESS_EVENT: &str = "anthology-export-progress";

fn emit_anthology_progress(app: &AppHandle, total: usize, current: usize, title: String, done: bool) {
    let _ = app.emit(
        ANTHOLOGY_PROGRESS_EVENT,
        ReadingProgress {
            total,
            current,
            title,
            done,
        },
    );
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnthologyResult {
    title: String,
    output_path: String,
    chapters: usize,
    image_count: usize,
    size_bytes: u64,
    /// 没找到本地正文、被跳过的作品名（前端照着提示一句，别让用户以为少打包是他点错了）
    skipped: Vec<String>,
}

/// 把一个系列 / 一个收藏夹打成一整本 EPUB。作品顺序**按传进来的顺序**，
/// 前端已经把系列按 `series_order` 排好、收藏夹按收藏时间排好，后端不再自作主张重排。
#[tauri::command]
async fn export_anthology_epub(
    app: AppHandle,
    path: String,
    title: String,
    author_name: String,
    work_ids: Vec<i64>,
) -> Result<AnthologyResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        export_anthology_epub_impl(&app, &path, &title, &author_name, &work_ids)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn export_anthology_epub_impl(
    app: &AppHandle,
    path: &str,
    title: &str,
    author_name: &str,
    work_ids: &[i64],
) -> Result<AnthologyResult, String> {
    if path.trim().is_empty() {
        return Err("请先选择保存位置".into());
    }
    if work_ids.is_empty() {
        return Err("这批作品是空的，没什么可打包的".into());
    }
    let book_title = if title.trim().is_empty() {
        "合集".to_string()
    } else {
        title.trim().to_string()
    };
    let conn = db()?;
    let total = work_ids.len();
    let mut chapters: Vec<AnthologyChapter> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut cover: Option<(String, Vec<u8>)> = None;
    // 这批作品全都属于同一个系列时，把系列名写进 calibre:series；
    // 混着选的（比如一个收藏夹）就不写，免得给书库塞个假系列。
    let mut series_names: Vec<String> = Vec::new();
    emit_anthology_progress(app, total, 0, "正在准备合集…".into(), false);
    for (index, work_id) in work_ids.iter().enumerate() {
        let row = conn
            .query_row(
                "SELECT w.title, a.name, w.release_date, w.tags, w.preview_path, w.purchased_path, w.cover_path, w.series_title FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id=?1",
                [work_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((
            work_title,
            work_author,
            release_date,
            tags,
            preview_path,
            purchased_path,
            cover_path,
            series_title,
        )) = row
        else {
            continue;
        };
        let series_title = series_title.trim().to_string();
        if !series_title.is_empty() && !series_names.iter().any(|name| name == &series_title) {
            series_names.push(series_title);
        }
        emit_anthology_progress(app, total, index, work_title.clone(), false);
        let bound = if purchased_path.trim().is_empty() {
            PathBuf::from(&preview_path)
        } else {
            PathBuf::from(&purchased_path)
        };
        let Some(text_path) = anthology_text_file(&bound) else {
            skipped.push(work_title);
            continue;
        };
        let Some(raw) = read_text_file(&text_path) else {
            skipped.push(work_title);
            continue;
        };
        if raw.trim().is_empty() {
            skipped.push(work_title);
            continue;
        }
        let extension = text_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_default();
        let content = if matches!(extension.as_str(), "html" | "htm" | "xhtml") {
            html_to_text(&raw)
        } else {
            raw
        };
        let (slots, images) = anthology_image_slots(&content, &text_path, chapters.len());
        let body = render_novel_xhtml(&content, &slots);
        let words = content
            .chars()
            .filter(|character| !character.is_whitespace())
            .count();
        let mut parts = vec![work_author];
        if !release_date.trim().is_empty() {
            parts.push(release_date.trim().to_string());
        }
        if words > 0 {
            parts.push(format!("{words} 字"));
        }
        chapters.push(AnthologyChapter {
            title: work_title,
            meta: parts.join(" · "),
            tags,
            body,
            images,
        });
        if cover.is_none() {
            let resolved = resolve_cover_path(&cover_path, &preview_path, &purchased_path);
            if !resolved.trim().is_empty() {
                let candidate = PathBuf::from(&resolved);
                if let Ok(bytes) = fs::read(&candidate) {
                    if !bytes.is_empty() {
                        let name = candidate
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| "cover.jpg".into());
                        cover = Some((name, bytes));
                    }
                }
            }
        }
    }
    if chapters.is_empty() {
        return Err("这批作品里没有找到可用的本地正文，打不出合集。先给它们下一份阅读版试试。".into());
    }
    let chapter_count = chapters.len();
    let image_count = chapters.iter().map(|chapter| chapter.images.len()).sum();
    let cover_ref = cover
        .as_ref()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()));
    let series = if series_names.len() == 1 {
        Some(series_names[0].as_str())
    } else {
        None
    };
    let epub = build_anthology_epub(
        &book_title,
        author_name.trim(),
        series,
        &chapters,
        cover_ref,
    );
    let target = PathBuf::from(path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("无法创建目录：{e}"))?;
    }
    write_bytes_atomic(&target, &epub)?;
    emit_anthology_progress(app, total, total, "合集已生成".into(), true);
    Ok(AnthologyResult {
        title: book_title,
        output_path: path.to_string(),
        chapters: chapter_count,
        image_count,
        size_bytes: epub.len() as u64,
        skipped,
    })
}

/// 导出作品清单。
/// - `scope` = `all`（整库） / `collection`（某个收藏夹，配 `scope_id`） / `ids`（指定作品，按传入顺序）
/// - `format` = `csv` 或 `markdown`
#[tauri::command]
fn export_work_list(
    path: String,
    format: String,
    scope: String,
    scope_id: Option<i64>,
    work_ids: Option<Vec<i64>>,
) -> Result<ExportResult, String> {
    if path.trim().is_empty() {
        return Err("请先选择导出位置".into());
    }
    let conn = db()?;
    let works = match scope.as_str() {
        "collection" => {
            let id = scope_id.ok_or_else(|| "缺少收藏夹 ID".to_string())?;
            let sql = format!(
                "SELECT {WORK_COLUMNS_W} FROM works w JOIN authors a ON a.id=w.author_id JOIN collection_works cw ON cw.work_id=w.id WHERE cw.collection_id={id} ORDER BY w.release_date DESC, w.id DESC"
            );
            query_works_with(&conn, &sql)?
        }
        "ids" => {
            let ids = work_ids.unwrap_or_default();
            if ids.is_empty() {
                return Err("没有要导出的作品".into());
            }
            let list = ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT {WORK_COLUMNS_W} FROM works w JOIN authors a ON a.id=w.author_id WHERE w.id IN ({list})"
            );
            let mut found = query_works_with(&conn, &sql)?;
            // 按前端给的顺序排（＝列表里看到的顺序），而不是数据库的顺序
            let mut by_id: HashMap<i64, Work> = found.drain(..).map(|work| (work.id, work)).collect();
            ids.iter()
                .filter_map(|id| by_id.remove(id))
                .collect::<Vec<_>>()
        }
        _ => all_works(&conn)?,
    };
    let rows = export_rows(&conn, works)?;
    match format.as_str() {
        "markdown" | "md" => write_export_markdown(&path, &rows)?,
        _ => write_export_csv(&path, &rows)?,
    }
    Ok(ExportResult {
        written: rows.len(),
        path,
    })
}

fn query_works_with(conn: &Connection, sql: &str) -> Result<Vec<Work>, String> {
    let mut statement = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = statement.query_map([], map_work).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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

/// 打开任意一个本地路径：`parent=false` 直接打开它，`parent=true` 打开它所在的目录。
///
/// 详情页每条路径旁边都有「打开」「所在目录」两个按钮，传进来的是路径字符串本身
/// （可能是 txt、epub、封面图片，也可能直接是个目录），所以这里收 `path` 而不是 `work_id`。
/// 这个命令原先只在文档里被前端调用过、后端从没实现 —— 详情页那排按钮点了只会报
/// 「命令不存在」。别再删。
#[tauri::command]
fn open_local_path(path: String, parent: bool) -> Result<(), String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("路径是空的".into());
    }
    let target = PathBuf::from(raw);
    if !target.exists() {
        return Err(format!("路径已不存在：{raw}"));
    }
    if !parent {
        open::that(&target).map_err(|e| format!("无法打开：{e}"))?;
        return Ok(());
    }
    if target.is_dir() {
        open::that(&target).map_err(|e| format!("无法打开目录：{e}"))?;
    } else {
        // 要的是「打开所在目录」，顺手在资源管理器里选中这个文件才算交代清楚
        reveal_in_explorer(&target)?;
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
    // 打开过就算「在读」（不会覆盖手动标的「已读」）
    let _ = mark_work_in_progress(&conn, work_id);
    // 记浏览历史失败不该影响「文件已经打开了」这件事
    let _ = record_history(&conn, work_id);
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

/// 自动备份默认保留最近 7 份（每天首次启动备一份，够回滚一周）。
fn default_backup_keep() -> usize {
    7
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


// ============ 字数后台补算（v1.2.8） ============

/// 本次算了几篇、还剩几篇没算。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WordCountProgress {
    updated: i64,
    remaining: i64,
}

/// 把一批还没算过字数的作品算出来写回 `works.word_count`。
///
/// **为什么必须放后台，不能留在列表里现算**：全库 1350 个 txt、98 MB，一次要 1.4 秒，
/// 而列表每次改筛选、每敲一下搜索都会重跑一遍 —— 那就是「所有作品加载得有点慢」的根。
/// 算一次落库，之后列表只读这一列。
///
/// 传 `limit = 0` 就是「不干活、只报还剩多少」，前端用它来显示进度和决定还要不要继续调。
/// 每篇一个自动提交的短事务：这个库是默认的 rollback journal、不是 WAL，
/// 攒成一个大事务会把写锁按住好几秒，界面那边的读就得干等。
/// 结果里 0 ＝「读不出来」（绑的是 EPUB、或目录里没有 txt），**不会重算**；
/// 只有路径变了才会被触发器标回 -1 重新排上队。
#[tauri::command]
fn refresh_word_counts(limit: usize) -> Result<WordCountProgress, String> {
    refresh_word_counts_impl(&db()?, limit)
}

fn refresh_word_counts_impl(conn: &Connection, limit: usize) -> Result<WordCountProgress, String> {
    let rows: Vec<(i64, String, String)> = {
        let mut statement = conn
            .prepare("SELECT id, preview_path, purchased_path FROM works WHERE word_count < 0 LIMIT ?1")
            .map_err(|e| e.to_string())?;
        let mapped = statement
            .query_map([limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut updated = 0;
    for (id, preview_path, purchased_path) in rows {
        // 和 `populate_work_display_info` 同一套取路径的规矩：完整版优先，没有就看预览版
        let path = if purchased_path.trim().is_empty() {
            preview_path
        } else {
            purchased_path
        };
        let count = text_word_count(&path).unwrap_or(0) as i64;
        conn.execute(
            "UPDATE works SET word_count=?1 WHERE id=?2",
            params![count, id],
        )
        .map_err(|e| e.to_string())?;
        updated += 1;
    }
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM works WHERE word_count < 0", [], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    Ok(WordCountProgress { updated, remaining })
}

// ============ 「待补完整版」工作台（v1.2.7） ============

/// 全库「还没有完整版」的作品（`purchased_path` 为空）。
///
/// 带上作者名，前端自己按作者分组 —— 分组结构放前端，后端不用再定一套。
/// **不在 SQL 里过滤 `need_full_state`**：前端要在「未处理 / 已找过 / 不打算补」
/// 之间切换，全量给回去最省事。
#[tauri::command]
fn list_missing_full() -> Result<Vec<Work>, String> {
    list_missing_full_impl(&db()?)
}

fn list_missing_full_impl(conn: &Connection) -> Result<Vec<Work>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {WORK_COLUMNS_W} FROM works w JOIN authors a ON a.id=w.author_id
             WHERE w.purchased_path = ''
             ORDER BY a.name COLLATE NOCASE ASC, w.release_date DESC, w.id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], map_work)
        .map_err(|e| e.to_string())?;
    let mut works = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for work in &mut works {
        populate_work_display_info(work);
    }
    Ok(works)
}

/// 给一批作品标「待补完整版」的处理状态：0 = 未处理，1 = 找过、确实没有，2 = 不打算补。
#[tauri::command]
fn set_works_need_full_state(work_ids: Vec<i64>, state: i64) -> Result<usize, String> {
    set_works_need_full_state_impl(&mut db()?, &work_ids, state)
}

fn set_works_need_full_state_impl(
    conn: &mut Connection,
    work_ids: &[i64],
    state: i64,
) -> Result<usize, String> {
    if !(0..=2).contains(&state) {
        return Err("未知的处理状态".into());
    }
    // 0 = 恢复未处理，那就把时间也清掉 —— 留着时间会让工作台显示
    // 「3 天前处理」却又是未处理，自相矛盾。
    let marked_at = if state == 0 {
        String::new()
    } else {
        Utc::now().to_rfc3339()
    };
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for work_id in work_ids {
        changed += tx
            .execute(
                "UPDATE works SET need_full_state=?1, need_full_marked_at=?3 WHERE id=?2",
                params![state, work_id, marked_at],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed)
}

// ============ 批量操作补充（v1.2.7） ============

/// 把作品从若干收藏夹里**移除**。对照 `add_works_to_collections` 的并集语义：
/// 那边只加不减，移出必须单独一条路，否则用户一勾就把作品从别的夹子里踢出去了。
#[tauri::command]
fn remove_works_from_collections(
    work_ids: Vec<i64>,
    collection_ids: Vec<i64>,
) -> Result<usize, String> {
    remove_works_from_collections_impl(&mut db()?, &work_ids, &collection_ids)
}

fn remove_works_from_collections_impl(
    conn: &mut Connection,
    work_ids: &[i64],
    collection_ids: &[i64],
) -> Result<usize, String> {
    if work_ids.is_empty() || collection_ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut removed = 0;
    for work_id in work_ids {
        for collection_id in collection_ids {
            removed += tx
                .execute(
                    "DELETE FROM collection_works WHERE collection_id=?1 AND work_id=?2",
                    params![collection_id, work_id],
                )
                .map_err(|e| e.to_string())?;
        }
        // 作品已经不在任何夹子里了 → favorite 那个缓存位跟着落下来
        tx.execute(
            "UPDATE works SET favorite = (SELECT COUNT(*) FROM collection_works WHERE work_id=?1) > 0 WHERE id=?1",
            [work_id],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(removed)
}

/// 批量设评分（0 = 清除评分）。
#[tauri::command]
fn set_works_rating(work_ids: Vec<i64>, rating: i64) -> Result<usize, String> {
    set_works_rating_impl(&mut db()?, &work_ids, rating)
}

fn set_works_rating_impl(
    conn: &mut Connection,
    work_ids: &[i64],
    rating: i64,
) -> Result<usize, String> {
    if !(0..=5).contains(&rating) {
        return Err("评分只能是 0 到 5".into());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut changed = 0;
    for work_id in work_ids {
        changed += tx
            .execute(
                "UPDATE works SET rating=?1 WHERE id=?2",
                params![rating, work_id],
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed)
}

// ============ 自动备份（v1.2.7） ============

/// 自动备份只认这个前缀 —— 滚动清理不会误删用户自己导出的备份。
const AUTO_BACKUP_PREFIX: &str = "library-auto-";

/// 自动备份落在 `<程序目录>/data/backup/auto`，和用户手动导出的备份分开放。
fn auto_backup_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join("backup").join("auto");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// 备份文件名的时间戳（本地时间，用户翻文件夹时对得上自己的钟）。
fn backup_stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S%3f").to_string()
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    /// 备份文件的完整路径（恢复时直接交给 `restore_backup`）
    path: String,
    name: String,
    size: u64,
    /// 显示用的时间（本地时间，形如 `2026-09-16 23:40`）
    created_at: String,
}

/// 只列出自动备份目录里的 `library-auto-*.db`，按新的在前。
fn list_auto_backups_impl() -> Result<Vec<BackupEntry>, String> {
    let dir = auto_backup_dir()?;
    let mut entries: Vec<BackupEntry> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(AUTO_BACKUP_PREFIX) || !name.ends_with(".db") {
            continue;
        }
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            continue;
        }
        entries.push(BackupEntry {
            path: path.to_string_lossy().to_string(),
            name: name.to_string(),
            size: metadata.len(),
            created_at: metadata
                .modified()
                .ok()
                .map(|time| DateTime::<Local>::from(time).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
        });
    }
    entries.sort_by(|left, right| right.name.cmp(&left.name));
    Ok(entries)
}

#[tauri::command]
fn list_backups() -> Result<Vec<BackupEntry>, String> {
    list_auto_backups_impl()
}

/// 做一份备份：`VACUUM INTO` 出一致性快照，然后按份数滚动清理。
///
/// **不能直接 `fs::copy` library.db** —— 连接开着的时候可能有还没落盘的内容，
/// 拷出来可能是半个事务。`VACUUM INTO` 由 SQLite 自己保证快照一致。
fn create_auto_backup(keep: usize) -> Result<BackupEntry, String> {
    let dir = auto_backup_dir()?;
    let name = format!("{AUTO_BACKUP_PREFIX}{}.db", backup_stamp());
    let path = dir.join(&name);
    if path.exists() {
        return Err("同一秒内已经备过一份了，稍后再试".into());
    }
    {
        let conn = db()?;
        conn.execute("VACUUM INTO ?1", [path.to_string_lossy().to_string()])
            .map_err(|e| format!("备份失败：{e}"))?;
    }
    prune_auto_backups(keep)?;
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    Ok(BackupEntry {
        path: path.to_string_lossy().to_string(),
        name,
        size: metadata.len(),
        created_at: Local::now().format("%Y-%m-%d %H:%M").to_string(),
    })
}

/// 滚动清理：只留最近 `keep` 份。**只删自己生成的 `library-auto-*.db`**，
/// 用户手动导出的备份和其它文件一概不碰。
fn prune_auto_backups(keep: usize) -> Result<Vec<String>, String> {
    let dir = auto_backup_dir()?;
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(AUTO_BACKUP_PREFIX) && name.ends_with(".db") {
            entries.push((name.to_string(), path));
        }
    }
    entries.sort_by(|left, right| right.0.cmp(&left.0));
    let mut removed = Vec::new();
    for (name, path) in entries.into_iter().skip(keep.max(1)) {
        if fs::remove_file(&path).is_ok() {
            removed.push(name);
        }
    }
    Ok(removed)
}

/// 手动「立即备份一份」（设置页的按钮）。
#[tauri::command]
fn backup_database_now() -> Result<BackupEntry, String> {
    let settings = read_settings(&db()?)?;
    create_auto_backup(settings.auto_backup_keep)
}

/// 每天第一次启动时自动备份一次。
///
/// 「一天只备一次」靠 app_settings 里的日期标记，**不能**改成「今天还没有备份文件就备」——
/// 用户手动删掉今天那份之后，每次启动都会再补一份。
fn auto_backup_on_startup() -> Result<Option<BackupEntry>, String> {
    let conn = db()?;
    let settings = read_settings(&conn)?;
    if !settings.auto_backup_enabled {
        return Ok(None);
    }
    let today = Local::now().format("%Y-%m-%d").to_string();
    if setting(&conn, "auto_backup_date")? == today {
        return Ok(None);
    }
    // 新装的程序库里一篇作品都没有，这时候备份只会留一堆空库
    let works: i64 = conn
        .query_row("SELECT COUNT(*) FROM works", [], |row| row.get(0))
        .unwrap_or(0);
    let entry = if works > 0 {
        Some(create_auto_backup(settings.auto_backup_keep)?)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES ('auto_backup_date', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [&today],
    )
    .map_err(|e| e.to_string())?;
    Ok(entry)
}

/// 前端启动时调一次；真的备了才返回一份记录（用来提示用户）。
#[tauri::command]
fn auto_backup_if_due() -> Result<Option<BackupEntry>, String> {
    auto_backup_on_startup()
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
            toggle_has_images,
            toggle_author_starred,
            list_collections,
            create_collection,
            rename_collection,
            delete_collection,
            list_collection_works,
            work_collections,
            set_work_collections,
            list_filter_views,
            search_full_text,
            save_filter_view,
            rename_filter_view,
            update_filter_view,
            delete_filter_view,
            list_history,
            clear_history,
            remove_history,
            set_has_images,
            set_work_meta,
            set_works_read_state,
            add_works_to_collections,
            update_works_tags,
            backfill_synopses,
            synopsis_backfill_status,
            scan_work_files,
            clear_missing_bindings,
            export_work_list,
            export_anthology_epub,
            open_work,
            open_work_directory,
            open_local_path,
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
            work_reading_path,
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
            // v1.2.7：待补完整版工作台 / 批量补充 / 自动备份
            // v1.2.8：字数后台补算
            refresh_word_counts,
            list_missing_full,
            set_works_need_full_state,
            remove_works_from_collections,
            set_works_rating,
            list_backups,
            backup_database_now,
            auto_backup_if_due,
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
        compress_cover,
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
        pixiv_uid_from_cookie, probe_pixiv_cookie,
        novel_image_filename_extension, novel_image_refs, novel_image_url, novel_images_dir,
        count_reading_images, novel_text_path_beside,
        path_key,
        pixiv_novel_id_from_url, pixiv_published_at, read_settings, setting,
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
        clean_collection_name, record_history, sync_work_favorite, DEFAULT_COLLECTION_NAME,
        HISTORY_LIMIT, migrate_favorites_into_collections,
        add_works_to_collections_impl, update_works_tags_impl,
        set_works_rating_impl, set_works_need_full_state_impl,
        remove_works_from_collections_impl, search_match_clause, list_missing_full_impl,
        list_series_impl, refresh_word_counts_impl, WORD_COUNT_TRIGGER_DDL,
        anthology_image_slots, clean_filter_view_name, FILTER_VIEW_MAX_NAME,
        document_text, is_epub_text_entry, single_line, strip_element_block,
        search_full_text_over, text_search_document, text_search_matches,
        TEXT_SEARCH_CONTEXT_CHARS, TEXT_SEARCH_MAX_SNIPPETS,
        mark_work_in_progress, read_state_label, split_tags, READ_DONE, READ_IN_PROGRESS,
        READ_UNREAD,
        throttled_delay_seconds, synopsis_progress_title,
        SYNOPSIS_ABORT_STREAK, SYNOPSIS_MAX_DELAY_SECONDS,
        SYNOPSIS_THROTTLE_STREAK, SYNOPSIS_META_MAX_CHARS,
        synopsis_plain_text, build_anthology_epub, AnthologyChapter,
        images_filter_clause, collection_filter_clause,
        write_export_markdown, EXPORT_HEADERS, EXPORT_COL_TITLE, EXPORT_COL_AUTHOR,
        EXPORT_COL_DATE, EXPORT_COL_WORDS, EXPORT_COL_FORMAT, EXPORT_COL_TAGS,
        EXPORT_COL_SERIES, EXPORT_COL_VERSION, EXPORT_COL_COLLECTIONS, EXPORT_COL_RATING,
        EXPORT_COL_READ, EXPORT_COL_NOTE, EXPORT_COL_PIXIV_URL, EXPORT_COL_TEXT_PATH,
        EXPORT_COL_COVER_PATH, EXPORT_COL_NOVEL_ID, EXPORT_COL_IMAGES, EXPORT_COL_SYNOPSIS,
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
        io::Write,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    /// 封面重压：产物必须是更小的 JPEG，且**像素尺寸一点不能动** —— 尺寸变了卡片会变形。
    #[test]
    fn cover_compression_shrinks_the_file_and_keeps_the_pixel_size() {
        // 高频细节是最难压的情况；按 Pixiv 那种高画质（q95）存出来当输入。
        let mut source = image::RgbImage::new(400, 600);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            *pixel = image::Rgb([
                ((x * 7 + y * 3) % 251) as u8,
                ((y * 11) % 241) as u8,
                ((x + y * 13) % 233) as u8,
            ]);
        }
        let mut raw: Vec<u8> = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut raw, 95)
            .encode(source.as_raw(), 400, 600, image::ExtendedColorType::Rgb8)
            .unwrap();

        let packed = compress_cover(&raw);

        assert!(packed.len() < raw.len(), "重压后应当变小");
        assert_eq!(&packed[..2], &[0xFF, 0xD8], "产物仍要是 JPEG 文件头");
        let back = image::load_from_memory(&packed).expect("产物要能解回来");
        assert_eq!((back.width(), back.height()), (400, 600), "像素尺寸不能变");
    }

    /// 解不开的输入必须**原样返回**：同步不能因为一张坏封面就整体中断。
    #[test]
    fn cover_compression_falls_back_to_the_original_bytes() {
        let junk = vec![0u8, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(compress_cover(&junk), junk);
        assert!(compress_cover(&[]).is_empty());
    }

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

    /// 「配图」「收藏夹」这两档拼出来的 SQL，拿内存库真跑一遍。
    ///
    /// 为什么值得写：三处命令里的 SQL 都是**运行时拼的字符串**，写错（括号少一个、
    /// 别名撞了、-1 那档忘了加）编译器一个字都不会说，只有用户真点一下才发现
    /// ——「有图」和「无图」筛出来一样、或者「任意收藏」恒为空。这类错静默且难查。
    #[test]
    fn image_and_collection_filters_do_what_their_labels_say() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE works (id INTEGER PRIMARY KEY, has_images INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE collection_works (collection_id INTEGER NOT NULL, work_id INTEGER NOT NULL);
             INSERT INTO works (id, has_images) VALUES (1, 1), (2, 0), (3, 1), (4, 0);
             INSERT INTO collection_works (collection_id, work_id) VALUES (1, 1), (2, 2), (2, 3);",
        )
        .unwrap();
        // 拼法跟三处命令完全一致
        let hits = |filter: &str, collection: i64| -> Vec<i64> {
            let sql = format!(
                "SELECT id FROM works w WHERE 1=1{}{} ORDER BY id",
                images_filter_clause(filter, "w."),
                collection_filter_clause("w.id", "cw", "?1")
            );
            let mut statement = conn.prepare(&sql).unwrap();
            let rows = statement
                .query_map([collection], |row| row.get::<_, i64>(0))
                .unwrap();
            rows.collect::<Result<Vec<_>, _>>().unwrap()
        };
        assert_eq!(hits("all", 0), vec![1, 2, 3, 4], "不限就是不加条件");
        assert_eq!(hits("has", 0), vec![1, 3], "「有图」只能出 has_images=1");
        assert_eq!(hits("none", 0), vec![2, 4], "「无图」是 has_images=0，不是「没绑配图」");
        assert_eq!(hits("all", 1), vec![1], "指定夹子");
        assert_eq!(hits("all", 2), vec![2, 3]);
        assert_eq!(hits("all", -1), vec![1, 2, 3], "「任意收藏」＝在任何一个夹子里");
        assert!(hits("all", 999).is_empty(), "不存在的夹子当然是空的");
        assert_eq!(hits("has", 2), vec![3], "两档叠起来也要对");

        // 收藏夹视图那条 SQL 外层已经用了 `cw`，内层别名换成 `cw2` —— 顺手证一下不冲突
        let sql = format!(
            "SELECT w.id FROM collection_works cw JOIN works w ON w.id = cw.work_id WHERE 1=1{} ORDER BY w.id",
            collection_filter_clause("w.id", "cw2", "?1")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows = statement.query_map([-1_i64], |row| row.get::<_, i64>(0)).unwrap();
        let ids = rows.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(ids, vec![1, 2, 3], "收藏夹视图里的「任意收藏」恒真，但别名不能撞");
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
    fn pixiv_uid_is_read_from_the_session_cookie_prefix() {
        // 真实格式 `<uid>_<随机串>`。**只用来显示账号名**，抠不出来就当没有 ——
        // 判断 Cookie 死活一律靠接口返回，不看这里
        assert_eq!(
            pixiv_uid_from_cookie("PHPSESSID=22871380_pq05W3eyaiA8hVDS2FT4yVhghHW79fOM").as_deref(),
            Some("22871380")
        );
        // 混在别的 cookie 里也能挑出来
        assert_eq!(
            pixiv_uid_from_cookie("a=b; PHPSESSID=123_xyz; c=d").as_deref(),
            Some("123")
        );
        // 没有下划线前缀：认不出就返回 None，绝不拿整串去拼 URL
        assert_eq!(pixiv_uid_from_cookie("PHPSESSID=garbage").as_deref(), None);
        // 前缀不是纯数字：同样不认
        assert_eq!(pixiv_uid_from_cookie("PHPSESSID=abc_123").as_deref(), None);
        assert_eq!(pixiv_uid_from_cookie("").as_deref(), None);
    }

    /// 真机联网验证 Cookie 体检的判据（默认 `#[ignore]`）。
    ///
    /// 这条为什么必须存在：判据写错时**前端 mock 和全部单测都是绿的** ——
    /// mock 是我们自己写的假接口，假接口只会按「我们以为的样子」回话，
    /// 判据错在哪它一点都看不出来。v1.2.15 就是栽在这儿（要求 body 里有 userId，
    /// 而真实成功响应里根本没这个字段），只有真接口证得伪。
    /// 手动跑：`WB_PIXIV_COOKIE="PHPSESSID=…" cargo test -- --ignored --nocapture pixiv_cookie`
    #[test]
    #[ignore]
    fn pixiv_cookie_probe_matches_the_real_api() {
        let Ok(cookie) = std::env::var("WB_PIXIV_COOKIE") else {
            eprintln!("没设 WB_PIXIV_COOKIE，跳过");
            return;
        };
        let valid = probe_pixiv_cookie(&cookie);
        println!("有效 Cookie → {valid:?}");
        if valid.status == "network" {
            eprintln!("连不上 Pixiv（先确认代理），跳过：{}", valid.message);
            return;
        }
        assert!(valid.ok, "这份 Cookie 应该被判为有效：{valid:?}");
        assert_eq!(valid.status, "ok");
        assert!(
            valid.user_name.is_some(),
            "账号名应该能从 PHPSESSID 前缀查出来：{valid:?}"
        );

        // 同前缀、令牌改坏：Pixiv 认不出 → 必须翻成 invalid（而不是 unknown 或 ok）
        let broken = match cookie.split_once('_') {
            Some((head, tail)) => format!("{head}_definitely-wrong-{tail}"),
            None => format!("{cookie}-definitely-wrong"),
        };
        let invalid = probe_pixiv_cookie(&broken);
        println!("改坏的 Cookie → {invalid:?}");
        assert!(!invalid.ok);
        assert_eq!(
            invalid.status, "invalid",
            "失效的 Cookie 要报 invalid，不能报 unknown"
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
    fn sync_reuses_a_preview_file_only_when_the_name_is_identical() {
        // 名字完全一致（去掉扩展名后）才复用，顺手带上同名封面
        let entries = vec![
            SyncPreviewEntry {
                path: PathBuf::from("D:/preview/希儿与布洛妮娅.txt"),
                name: "希儿与布洛妮娅".into(),
                is_preview: true,
            },
            SyncPreviewEntry {
                path: PathBuf::from("D:/preview/希儿与布洛妮娅.jpg"),
                name: "希儿与布洛妮娅".into(),
                is_preview: false,
            },
        ];
        let (preview, cover) = matched_sync_preview(&entries, "希儿与布洛妮娅").unwrap();
        assert_eq!(preview, PathBuf::from("D:/preview/希儿与布洛妮娅.txt"));
        assert_eq!(
            cover,
            Some(PathBuf::from("D:/preview/希儿与布洛妮娅.jpg"))
        );

        // 「其一 / 其二」这类长共同前缀不能再被认成同一篇 —— 这正是旧相似度逻辑的翻车点
        let similar_but_different = vec![SyncPreviewEntry {
            path: PathBuf::from("D:/preview/希儿与布洛妮娅 其二.txt"),
            name: "希儿与布洛妮娅 其二".into(),
            is_preview: true,
        }];
        assert!(matched_sync_preview(&similar_but_different, "希儿与布洛妮娅").is_none());

        // 用户手改过、带日期前缀的文件名同样不复用（宁可重下一份，也不能复用错内容）
        let dated = vec![SyncPreviewEntry {
            path: PathBuf::from("D:/preview/2025-10-05 希儿与布洛妮娅.txt"),
            name: "2025-10-05 希儿与布洛妮娅".into(),
            is_preview: true,
        }];
        assert!(matched_sync_preview(&dated, "希儿与布洛妮娅").is_none());

        // 同名文件有两份时不猜
        let duplicated = vec![
            SyncPreviewEntry {
                path: PathBuf::from("D:/a/同名.txt"),
                name: "同名".into(),
                is_preview: true,
            },
            SyncPreviewEntry {
                path: PathBuf::from("D:/b/同名.txt"),
                name: "同名".into(),
                is_preview: true,
            },
        ];
        assert!(matched_sync_preview(&duplicated, "同名").is_none());
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
                synopsis: "简介<b>重点</b><br>第二行",
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
        // 简介折起来放，标签剥掉
        assert!(html.contains("<details class=\"synopsis\"><summary>作品简介</summary>"));
        assert!(html.contains("<p>简介重点</p>"));
        assert!(html.contains("<p>第二行</p>"));
        assert!(!html.contains("</b>"), "简介里的标签应该被剥掉");
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
            synopsis: "作者写的一段简介 &amp; 一点补充",
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
        assert!(html_text.contains("<details class=\"synopsis\">"), "HTML 阅读版应该带简介");
        assert!(html_text.contains("作者写的一段简介 &amp; 一点补充"));

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
        let raw_epub = String::from_utf8_lossy(&bytes);
        assert!(raw_epub.contains("汉字"));
        assert!(raw_epub.contains("<dc:description>作者写的一段简介 &amp; 一点补充"));

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
            synopsis: String::new(),
            read_state: 0,
            rating: 0,
            note: String::new(),
            synopsis_checked: false,
            need_full_state: 0,
            need_full_marked_at: String::new(),
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
            synopsis: String::new(),
            read_state: 0,
            rating: 0,
            note: String::new(),
            synopsis_checked: false,
            need_full_state: 0,
            need_full_marked_at: String::new(),
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

    /// 导出列的位置写死过一次、读错列还不报错，所以这里钉住：
    /// 列常量必须正好覆盖 `0..EXPORT_HEADERS.len()`，不重不漏。
    #[test]
    fn export_columns_stay_in_sync_with_the_header_row() {
        let columns = [
            EXPORT_COL_TITLE,
            EXPORT_COL_AUTHOR,
            EXPORT_COL_DATE,
            EXPORT_COL_WORDS,
            EXPORT_COL_FORMAT,
            EXPORT_COL_TAGS,
            EXPORT_COL_SERIES,
            EXPORT_COL_VERSION,
            EXPORT_COL_COLLECTIONS,
            EXPORT_COL_RATING,
            EXPORT_COL_READ,
            EXPORT_COL_NOTE,
            EXPORT_COL_PIXIV_URL,
            EXPORT_COL_TEXT_PATH,
            EXPORT_COL_COVER_PATH,
            EXPORT_COL_NOVEL_ID,
            EXPORT_COL_IMAGES,
            EXPORT_COL_SYNOPSIS,
        ];
        assert_eq!(columns.len(), EXPORT_HEADERS.len(), "列常量数量和表头对不上");
        let mut sorted = columns.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), columns.len(), "有两个列常量指到了同一格");
        assert_eq!(
            sorted,
            (0..EXPORT_HEADERS.len()).collect::<Vec<_>>(),
            "列号应该正好是 0..N"
        );
    }

    #[test]
    fn markdown_export_carries_synopsis_paths_and_ids() {
        let mut row = vec![String::new(); EXPORT_HEADERS.len()];
        row[EXPORT_COL_TITLE] = "旧城的信".into();
        row[EXPORT_COL_AUTHOR] = "远野".into();
        row[EXPORT_COL_TAGS] = "悬疑、完结".into();
        row[EXPORT_COL_VERSION] = "完整版".into();
        row[EXPORT_COL_READ] = "未读".into();
        row[EXPORT_COL_IMAGES] = "7".into();
        row[EXPORT_COL_NOVEL_ID] = "26410189".into();
        row[EXPORT_COL_PIXIV_URL] = "https://www.pixiv.net/novel/show.php?id=26410189".into();
        row[EXPORT_COL_TEXT_PATH] = r"D:\已购\旧城的信.txt".into();
        row[EXPORT_COL_COVER_PATH] = r"D:\已购\旧城的信.jpg".into();
        row[EXPORT_COL_SYNOPSIS] = "第一段\n第二段".into();

        let stamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("wb-export-{stamp}.md"));
        write_export_markdown(&path.to_string_lossy(), &[row]).unwrap();
        let markdown = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(markdown.contains("## 旧城的信"), "实际输出：{markdown}");
        assert!(markdown.contains("作者：远野"));
        assert!(markdown.contains("配图：7 张"));
        assert!(markdown.contains("标签：悬疑、完结"));
        // 简介要按段落引起来，段落之间的换行不能拍成一行
        assert!(markdown.contains("> 第一段\n> 第二段"), "实际输出：{markdown}");
        assert!(markdown.contains(
            "[Pixiv 原页](https://www.pixiv.net/novel/show.php?id=26410189)（ID 26410189）"
        ));
        assert!(markdown.contains(r"正文：`D:\已购\旧城的信.txt`"));
        assert!(markdown.contains(r"封面：`D:\已购\旧城的信.jpg`"));

        // 列少的行（老数据 / 手写行）不能把导出整崩，缺的格子当空串
        let short = vec!["只有标题".to_string()];
        let path = std::env::temp_dir().join(format!("wb-export-short-{stamp}.md"));
        write_export_markdown(&path.to_string_lossy(), &[short]).unwrap();
        let markdown = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).ok();
        assert!(markdown.contains("## 只有标题"));
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
            "简介第一段<br>第二段 &amp; 尾巴",
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
        // 简介进 OPF：标签剥掉、实体还原、再按 XML 规则重新转义
        assert!(raw.contains("<dc:description>简介第一段"), "缺少简介元数据");
        assert!(raw.contains("第二段 &amp; 尾巴"), "简介实体应还原后再转义");
        assert!(!raw.contains("<br>"), "简介里不该留 HTML 标签");
        assert!(raw.contains("<dc:subject>插画</dc:subject>"));
    }

    #[test]
    fn synopsis_plain_text_drops_markup_keeps_paragraphs_and_clips() {
        let plain = synopsis_plain_text("<p>第一段&amp;补充</p><p>第二段</p><ul><li>条目</li></ul>");
        assert_eq!(plain, "第一段&补充\n第二段\n- 条目");

        // 空简介和纯标签的简介都不该产出空白块
        assert!(synopsis_plain_text("").is_empty());
        assert!(synopsis_plain_text("   ").is_empty());
        assert!(synopsis_plain_text("<p></p>").is_empty());

        // 脚本内容不算简介
        assert!(!synopsis_plain_text("<script>alert(1)</script><p>正文</p>").contains("alert"));

        // 超长要截短，按字符切（中文一个字 3 字节，切字节会切碎字符）
        let long = "字".repeat(SYNOPSIS_META_MAX_CHARS + 50);
        let clipped = synopsis_plain_text(&long);
        assert_eq!(clipped.chars().count(), SYNOPSIS_META_MAX_CHARS + 1);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn anthology_opf_reports_contents_and_series() {
        let chapters = vec![
            AnthologyChapter {
                title: "第一篇".into(),
                meta: "作者 · 2025-01-01".into(),
                tags: "奇幻|连载".into(),
                body: "<p>正文一</p>".into(),
                images: Vec::new(),
            },
            AnthologyChapter {
                title: "第二篇".into(),
                meta: "作者 · 2025-02-01".into(),
                tags: "奇幻|日常".into(),
                body: "<p>正文二</p>".into(),
                images: Vec::new(),
            },
        ];
        let epub = build_anthology_epub("某某系列", "作者", Some("某某系列"), &chapters, None);
        let raw = String::from_utf8_lossy(&epub).to_string();
        assert!(raw.contains("<dc:description>共收录 2 篇：1.第一篇；2.第二篇</dc:description>"));
        assert!(raw.contains(r#"<meta name="calibre:series" content="某某系列"/>"#));
        // 标签取并集且不重复
        assert!(raw.contains("<dc:subject>奇幻</dc:subject>"));
        assert!(raw.contains("<dc:subject>连载</dc:subject>"));
        assert!(raw.contains("<dc:subject>日常</dc:subject>"));
        assert_eq!(raw.matches("<dc:subject>奇幻</dc:subject>").count(), 1);

        // 混着选（不属于同一系列）时不能硬写一个假系列名
        let loose = build_anthology_epub("杂集", "作者", None, &chapters, None);
        let loose_raw = String::from_utf8_lossy(&loose).to_string();
        assert!(!loose_raw.contains("calibre:series"));
        // 空白系列名同样不写
        let blank = build_anthology_epub("杂集", "作者", Some("   "), &chapters, None);
        assert!(!String::from_utf8_lossy(&blank).contains("calibre:series"));
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
                synopsis: "",
                cover_file: "",
                image_count: 1,
            },
        );
        assert!(html.contains("开头文字"), "实际输出：{html}");
        assert!(html.contains("结尾文字"), "实际输出：{html}");
        // 作者没写简介时整块不出现，别留个空框
        assert!(!html.contains("作品简介"), "实际输出：{html}");
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

    /// 收藏夹与浏览历史（v1.1.0）用到的三张表 —— 跟 db() 里那段建表语句保持一致
    fn create_collection_tables(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '');
             CREATE TABLE works (
               id INTEGER PRIMARY KEY,
               author_id INTEGER NOT NULL DEFAULT 1,
               title TEXT NOT NULL DEFAULT '',
               favorite INTEGER NOT NULL DEFAULT 0,
               read_state INTEGER NOT NULL DEFAULT 0,
               rating INTEGER NOT NULL DEFAULT 0,
               note TEXT NOT NULL DEFAULT '',
               tags TEXT NOT NULL DEFAULT '',
               synopsis TEXT NOT NULL DEFAULT '',
               synopsis_checked INTEGER NOT NULL DEFAULT 0,
               need_full_state INTEGER NOT NULL DEFAULT 0,
               need_full_marked_at TEXT NOT NULL DEFAULT '',
               word_count INTEGER NOT NULL DEFAULT -1
             );
             CREATE TABLE collections (
               id INTEGER PRIMARY KEY,
               name TEXT NOT NULL UNIQUE,
               sort_order INTEGER NOT NULL DEFAULT 0,
               created_at TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE collection_works (
               collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
               work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
               added_at TEXT NOT NULL DEFAULT '',
               PRIMARY KEY (collection_id, work_id)
             );
             CREATE TABLE work_history (
               work_id INTEGER PRIMARY KEY REFERENCES works(id) ON DELETE CASCADE,
               viewed_at TEXT NOT NULL DEFAULT '',
               view_count INTEGER NOT NULL DEFAULT 1
             );",
        )
        .unwrap();
    }

    /// 建一张「列齐全」的 works 表 + authors：专门用来验证 `WORK_COLUMNS` 与 `map_work`
    /// 的列号对齐。列清单少一列时 SELECT 会直接报错，比在真机上静默读错列好得多。
    fn create_full_work_tables(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE authors (
               id INTEGER PRIMARY KEY,
               name TEXT NOT NULL DEFAULT '',
               homepage TEXT NOT NULL DEFAULT '',
               avatar_path TEXT NOT NULL DEFAULT '',
               notes TEXT NOT NULL DEFAULT '',
               preview_dir TEXT NOT NULL DEFAULT '',
               purchased_dir TEXT NOT NULL DEFAULT '',
               match_threshold INTEGER NOT NULL DEFAULT 70,
               pixiv_last_sync_at TEXT NOT NULL DEFAULT '',
               avatar_managed INTEGER NOT NULL DEFAULT 0,
               sort_order INTEGER NOT NULL DEFAULT 0,
               starred INTEGER NOT NULL DEFAULT 0,
               aliases TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE works (
               id INTEGER PRIMARY KEY,
               author_id INTEGER NOT NULL DEFAULT 1,
               title TEXT NOT NULL DEFAULT '',
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
               synopsis TEXT NOT NULL DEFAULT '',
               synopsis_checked INTEGER NOT NULL DEFAULT 0,
               read_state INTEGER NOT NULL DEFAULT 0,
               rating INTEGER NOT NULL DEFAULT 0,
               note TEXT NOT NULL DEFAULT '',
               need_full_state INTEGER NOT NULL DEFAULT 0,
               need_full_marked_at TEXT NOT NULL DEFAULT '',
               word_count INTEGER NOT NULL DEFAULT -1
             );",
        )
        .unwrap();
    }

    #[test]
    fn search_clause_widens_with_the_selected_scope() {
        // 标题：只比 title，不碰简介
        let title = search_match_clause("title", "", "?2", "?3");
        assert!(title.starts_with("(?2 = ''"));
        assert!(title.contains("title LIKE ?3"));
        assert!(!title.contains("synopsis"));
        // 标题 + 简介
        let with_synopsis = search_match_clause("title_synopsis", "", "?2", "?3");
        assert!(with_synopsis.contains("synopsis"));
        assert!(with_synopsis.contains("COALESCE"));
        // 标题 + 简介 + 标签（三处调用点里前缀带 w.）
        let widest = search_match_clause("title_synopsis_tags", "w.", "?2", "?3");
        assert!(widest.contains("w.synopsis"));
        assert!(widest.contains("w.tags"));
        // 标签
        assert!(search_match_clause("tags", "w.", "?2", "?3").contains("w.tags LIKE ?3"));
        // 不认识的范围退回「标题」；占位符编号必须原样带出去（三处调用点各自不同）
        let fallback = search_match_clause("mystery", "w.", "?1", "?2");
        assert!(fallback.contains("w.title LIKE ?2"));
        assert!(fallback.contains("?1 = ''"));
        assert!(!fallback.contains("?3"));
    }

    #[test]
    fn missing_full_lists_only_works_without_a_purchased_file() {
        let conn = Connection::open_in_memory().unwrap();
        create_full_work_tables(&conn);
        conn.execute_batch(
            "INSERT INTO authors (id, name) VALUES (1, 'A 站'), (2, 'B 站');
             INSERT INTO works (id, author_id, title, release_date, preview_path, purchased_path) VALUES
               (1, 1, '只有预览版', '2025-01-01', 'D:/预览/A/a.txt', ''),
               (2, 1, '已经补齐', '2025-01-02', 'D:/预览/A/b.txt', 'D:/已购/A/b.txt'),
               (3, 2, '预览版二', '2025-03-03', 'D:/预览/B/c.txt', '');
             INSERT INTO works (id, author_id, title, release_date) VALUES (4, 2, '两边都没有', '2024-01-01');",
        )
        .unwrap();
        let works = list_missing_full_impl(&conn).unwrap();
        let ids: Vec<i64> = works.iter().map(|work| work.id).collect();
        // 先按作者名，再按发布日期倒序。4 号两边都没有也算「缺完整版」——
        // 它同样得去补，不该被漏掉。
        assert_eq!(ids, vec![1, 3, 4]);
        // 作者名带了回来，且按作者名排序
        assert_eq!(works[0].author_name, "A 站");
        assert_eq!(works[1].author_name, "B 站");
        // 靠后读的那几列（标题 / 状态）都落在正确位置上
        assert_eq!(works[0].title, "只有预览版");
        assert_eq!(works[0].need_full_state, 0);
        assert_eq!(works[0].synopsis_checked, false);
    }


    #[test]
    fn bulk_rating_accepts_zero_but_rejects_out_of_range() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute_batch("INSERT INTO works (id, title) VALUES (1, '甲'), (2, '乙');")
            .unwrap();
        assert_eq!(set_works_rating_impl(&mut conn, &[1, 2], 4).unwrap(), 2);
        let value: i64 = conn
            .query_row("SELECT rating FROM works WHERE id=2", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 4);
        // 0 是「清除评分」，不是非法值
        assert_eq!(set_works_rating_impl(&mut conn, &[2], 0).unwrap(), 1);
        assert!(set_works_rating_impl(&mut conn, &[1], 6).is_err());
        assert!(set_works_rating_impl(&mut conn, &[1], -1).is_err());
    }

    #[test]
    fn removing_works_from_collections_clears_the_favorite_cache() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute_batch(
            "INSERT INTO works (id, title, favorite) VALUES (1, '甲', 1), (2, '乙', 1);
             INSERT INTO collections (id, name) VALUES (7, 'A'), (8, 'B');
             INSERT INTO collection_works (collection_id, work_id) VALUES (7, 1), (8, 1), (8, 2);",
        )
        .unwrap();
        // 只从 A 里移出：作品 1 还在 B 里 → favorite 缓存位不该落下
        assert_eq!(remove_works_from_collections_impl(&mut conn, &[1], &[7]).unwrap(), 1);
        let favorite: i64 = conn
            .query_row("SELECT favorite FROM works WHERE id=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(favorite, 1);
        // 再从 B 里移出：一个夹子都不在了 → 归零
        assert_eq!(remove_works_from_collections_impl(&mut conn, &[1], &[8]).unwrap(), 1);
        let favorite: i64 = conn
            .query_row("SELECT favorite FROM works WHERE id=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(favorite, 0);
        // 本来就不在夹子里的不移除任何东西
        assert_eq!(remove_works_from_collections_impl(&mut conn, &[2], &[7]).unwrap(), 0);
    }

    #[test]
    fn need_full_marks_reject_unknown_states() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute_batch("INSERT INTO works (id, title) VALUES (1, '甲');")
            .unwrap();
        assert_eq!(set_works_need_full_state_impl(&mut conn, &[1], 1).unwrap(), 1);
        let value: i64 = conn
            .query_row("SELECT need_full_state FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, 1);
        assert!(set_works_need_full_state_impl(&mut conn, &[1], 3).is_err());
    }

    #[test]
    fn need_full_marks_record_and_clear_the_time() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute_batch("INSERT INTO works (id, title) VALUES (1, '甲'), (2, '乙');")
            .unwrap();
        // 新作品是「没标过」：状态 0、时间也是空的
        let (state, marked): (i64, String) = conn
            .query_row(
                "SELECT need_full_state, need_full_marked_at FROM works WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, 0);
        assert!(marked.is_empty());

        // 标完之后要留下时间 —— 工作台得显示「什么时候找过的」，
        // 不然过俩月翻到同一篇，想不起来自己找没找过（这就是这列存在的理由）
        assert_eq!(set_works_need_full_state_impl(&mut conn, &[1, 2], 2).unwrap(), 2);
        let marked: String = conn
            .query_row(
                "SELECT need_full_marked_at FROM works WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            DateTime::parse_from_rfc3339(&marked).is_ok(),
            "标记时间应当是可解析的 RFC3339，实际是 {marked:?}"
        );

        // 恢复「未处理」要把时间一起清掉，否则「未处理」还挂着「刚刚处理」，
        // 两句话互相打架
        assert_eq!(set_works_need_full_state_impl(&mut conn, &[1], 0).unwrap(), 1);
        let (state, marked): (i64, String) = conn
            .query_row(
                "SELECT need_full_state, need_full_marked_at FROM works WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, 0);
        assert!(marked.is_empty());
        // 另一篇还留着时间：清的是被点的那一篇，不是整批
        let kept: String = conn
            .query_row(
                "SELECT need_full_marked_at FROM works WHERE id=2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!kept.is_empty());
    }

    #[test]
    fn collection_names_are_trimmed_and_rejected_when_unusable() {
        assert_eq!(clean_collection_name("  短篇向 ").unwrap(), "短篇向");
        assert!(clean_collection_name("").is_err());
        assert!(clean_collection_name("   ").is_err());
        // 24 个汉字可以，25 个不行（侧栏卡片放不下）
        assert!(clean_collection_name(&"长".repeat(24)).is_ok());
        assert!(clean_collection_name(&"长".repeat(25)).is_err());
        assert_eq!(DEFAULT_COLLECTION_NAME, "我的收藏");
    }

    #[test]
    fn favorite_flag_follows_collection_membership() {
        let conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute("INSERT INTO works (id, title) VALUES (1, '甲'), (2, '乙')", [])
            .unwrap();
        conn.execute("INSERT INTO collections (id, name) VALUES (10, '我的收藏')", [])
            .unwrap();
        // 一开始两边都不在收藏夹里
        sync_work_favorite(&conn, 1).unwrap();
        assert_eq!(favorite_of(&conn, 1), 0);
        // 加进一个夹子 → 缓存位变 1
        conn.execute(
            "INSERT INTO collection_works (collection_id, work_id, added_at) VALUES (10, 1, '2026-09-16T00:00:00Z')",
            [],
        )
        .unwrap();
        sync_work_favorite(&conn, 1).unwrap();
        assert_eq!(favorite_of(&conn, 1), 1);
        // 移出去 → 变回 0，另一篇没被牵连
        conn.execute("DELETE FROM collection_works WHERE work_id=1", [])
            .unwrap();
        sync_work_favorite(&conn, 1).unwrap();
        assert_eq!(favorite_of(&conn, 1), 0);
        assert_eq!(favorite_of(&conn, 2), 0);
    }

    fn favorite_of(conn: &Connection, work_id: i64) -> i64 {
        conn.query_row("SELECT favorite FROM works WHERE id=?1", [work_id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn history_keeps_the_latest_view_and_drops_the_oldest() {
        let conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for id in 1..=(HISTORY_LIMIT + 3) {
            conn.execute("INSERT INTO works (id, title) VALUES (?1, '作品')", [id])
                .unwrap();
        }
        // 时间逐条推后，保证「最旧的是 id 最小的」
        for id in 1..=(HISTORY_LIMIT + 3) {
            conn.execute(
                "INSERT INTO work_history (work_id, viewed_at, view_count) VALUES (?1, ?2, 1)",
                (id, format!("2026-09-16T00:{:02}:{:02}Z", id / 60, id % 60)),
            )
            .unwrap();
        }
        // 再看一次 id=1 的作品：应该原地更新，而不是多出一行
        conn.execute("UPDATE work_history SET viewed_at='2026-12-31T23:59:59Z' WHERE work_id=1", [])
            .unwrap();
        record_history(&conn, 1).unwrap();
        let (count, views): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(view_count) FROM work_history WHERE work_id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "同一篇作品在历史里只占一行");
        assert_eq!(views, 2, "重复看同一篇只叠次数");
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM work_history", [], |row| row.get(0))
            .unwrap();
        assert!(total <= HISTORY_LIMIT, "超出上限要丢掉最旧的：{total}");
    }

    #[test]
    fn history_is_not_written_when_recording_is_switched_off() {
        let conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('record_history', '0')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO works (id, title) VALUES (7, '作品')", [])
            .unwrap();
        record_history(&conn, 7).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM work_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0, "关掉开关后一条都不该写");
    }

    #[test]
    fn old_favorites_are_moved_into_the_default_collection_only_once() {
        let conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute(
            "INSERT INTO works (id, title, favorite) VALUES (1, '收了的', 1), (2, '没收的', 0)",
            [],
        )
        .unwrap();

        migrate_favorites_into_collections(&conn).unwrap();

        let (id, name): (i64, String) = conn
            .query_row("SELECT id, name FROM collections", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(name, DEFAULT_COLLECTION_NAME, "老收藏要落进默认收藏夹");
        let moved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_works WHERE collection_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved, 1, "只搬 favorite=1 的作品，没收的不动");
        let only: i64 = conn
            .query_row("SELECT work_id FROM collection_works", [], |row| row.get(0))
            .unwrap();
        assert_eq!(only, 1);

        // 标记位挡着：第二次（乃至以后每次启动）都不许再搬，否则用户删空后又被塞回来
        conn.execute(
            "INSERT INTO works (id, title, favorite) VALUES (3, '升级后才收的', 1)",
            [],
        )
        .unwrap();
        migrate_favorites_into_collections(&conn).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM collection_works", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1, "迁移只跑一次，升级后新收的作品不该被重复搬进默认夹");
        assert_eq!(setting(&conn, "collections_migrated").unwrap(), "1");
    }

    fn read_state_of(conn: &Connection, work_id: i64) -> i64 {
        conn.query_row("SELECT read_state FROM works WHERE id=?1", [work_id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn opening_a_work_marks_it_reading_but_never_unreads_a_finished_one() {
        let conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute("INSERT INTO works (id, title) VALUES (1, '甲'), (2, '乙')", [])
            .unwrap();
        // 未读 → 打开过 → 在读
        mark_work_in_progress(&conn, 1).unwrap();
        assert_eq!(read_state_of(&conn, 1), READ_IN_PROGRESS);
        // 手动标了已读之后再打开，不该被打回「在读」
        conn.execute("UPDATE works SET read_state=?1 WHERE id=1", [READ_DONE])
            .unwrap();
        mark_work_in_progress(&conn, 1).unwrap();
        assert_eq!(read_state_of(&conn, 1), READ_DONE);
        // 另一篇没被牵连
        assert_eq!(read_state_of(&conn, 2), READ_UNREAD);
    }

    #[test]
    fn read_state_labels_cover_all_three_states() {
        assert_eq!(read_state_label(READ_UNREAD), "未读");
        assert_eq!(read_state_label(READ_IN_PROGRESS), "在读");
        assert_eq!(read_state_label(READ_DONE), "已读");
        // 库里万一出现别的值，宁可显示「未读」也不要 panic
        assert_eq!(read_state_label(99), "未读");
    }

    #[test]
    fn tags_split_on_pipes_and_drop_empties() {
        assert_eq!(
            split_tags("原创 | 短篇 | 治愈 "),
            vec!["原创", "短篇", "治愈"]
        );
        assert!(split_tags("").is_empty());
        assert!(split_tags(" | | ").is_empty());
        // 标签里夹了换行也不能带进导出结果
        assert_eq!(split_tags("A|\nB|"), vec!["A", "B"]);
    }

    fn tags_of(conn: &Connection, work_id: i64) -> String {
        conn.query_row("SELECT tags FROM works WHERE id=?1", [work_id], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn bulk_tag_edit_appends_and_removes_but_keeps_the_rest() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute(
            "INSERT INTO works (id, title, tags) VALUES (1, '甲', '原创|短篇|治愈'), (2, '乙', '短篇')",
            [],
        )
        .unwrap();
        // 追加：第一篇加「新坑」，第二篇加「新坑」
        let changed =
            update_works_tags_impl(&mut conn, &[1, 2], "新坑", "").expect("追加该成功");
        assert_eq!(changed, 2);
        assert_eq!(tags_of(&conn, 1), "原创| 短篇| 治愈| 新坑");
        // 重复追加（含大小写差异）不该再变，也不该重复塞进同一个标签
        let again = update_works_tags_impl(&mut conn, &[1, 2], "新坑|新坑", "").unwrap();
        assert_eq!(again, 0);
        assert_eq!(tags_of(&conn, 1), "原创| 短篇| 治愈| 新坑");
        // 移除只摘匹配上的，别的标签原样保留
        let removed = update_works_tags_impl(&mut conn, &[1, 2], "", "短篇|治愈").unwrap();
        assert_eq!(removed, 2);
        assert_eq!(tags_of(&conn, 1), "原创| 新坑");
        assert_eq!(tags_of(&conn, 2), "新坑");
        // 两个框都空要报错，而不是把标签清空
        assert!(update_works_tags_impl(&mut conn, &[1], "", "").is_err());
        assert_eq!(tags_of(&conn, 1), "原创| 新坑");
    }

    #[test]
    fn bulk_add_to_collections_is_union_not_replace() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_collection_tables(&conn);
        conn.execute(
            "INSERT INTO works (id, title) VALUES (1, '甲'), (2, '乙')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collections (id, name) VALUES (1, '待读'), (2, '短篇向')",
            [],
        )
        .unwrap();
        // 第一篇本来就在「待读」里
        conn.execute(
            "INSERT INTO collection_works (collection_id, work_id) VALUES (1, 1)",
            [],
        )
        .unwrap();
        let added =
            add_works_to_collections_impl(&mut conn, &[1, 2], &[2]).expect("批量加夹该成功");
        assert_eq!(added, 2);
        // 并集：第一篇同时留在「待读」+ 进「短篇向」，绝不能被踢出「待读」
        let first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_works WHERE work_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first, 2);
        // favorite 缓存位跟着回写
        assert_eq!(favorite_of(&conn, 1), 1);
        assert_eq!(favorite_of(&conn, 2), 1);
        // 重复加同一个夹不会变成两行（PRIMARY KEY 挡住）
        add_works_to_collections_impl(&mut conn, &[1, 2], &[2]).unwrap();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM collection_works", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 3);
        // 空入参直接返回 0，不写库
        assert_eq!(add_works_to_collections_impl(&mut conn, &[], &[1]).unwrap(), 0);
        assert_eq!(add_works_to_collections_impl(&mut conn, &[1], &[]).unwrap(), 0);
    }

    /// 补抓简介被限流时的退避：间隔翻倍、封顶；并且「先加间隔」一定早于「直接放弃」。
    #[test]
    fn synopsis_backoff_doubles_then_caps() {
        assert_eq!(throttled_delay_seconds(1), 2);
        assert_eq!(throttled_delay_seconds(2), 4);
        assert_eq!(throttled_delay_seconds(16), 32);
        // 翻过上限就贴着上限，不能一路翻到几小时
        assert_eq!(throttled_delay_seconds(40), SYNOPSIS_MAX_DELAY_SECONDS);
        assert_eq!(throttled_delay_seconds(60), SYNOPSIS_MAX_DELAY_SECONDS);
        // 万一有人把间隔设成 0，也不能算出 0 秒（那样等于没退避）
        assert_eq!(throttled_delay_seconds(0), 2);
        // 顺序：先翻倍观察，连续失败更多才放弃，别一上来就停
        assert!(SYNOPSIS_THROTTLE_STREAK > 0);
        assert!(SYNOPSIS_THROTTLE_STREAK < SYNOPSIS_ABORT_STREAK);
        // 从默认 1 秒一路翻倍，到放弃那一刻还没超过上限
        let mut delay = 1;
        for _ in 0..(SYNOPSIS_ABORT_STREAK / SYNOPSIS_THROTTLE_STREAK) + 1 {
            delay = throttled_delay_seconds(delay);
        }
        assert!(delay <= SYNOPSIS_MAX_DELAY_SECONDS);
    }

    /// 补抓浮层第二行的文案：必须把「作者没写简介」单独报出来。
    /// 用户的报障就是「一直在正常补抓，但一直显示已更新 0 篇」——
    /// 头几十篇正好全是作者没写简介的，只报「已更新 0」看着就像卡死了。
    #[test]
    fn synopsis_progress_reports_works_without_description() {
        let running = synopsis_progress_title(0, 15, 0, Some(1));
        assert!(running.contains("作者没写简介 15 篇"), "{running}");
        assert!(running.contains("每篇间隔 1 秒"), "{running}");

        // 有失败时要说清是失败，并且间隔被拉大了也得报出来
        let failing = synopsis_progress_title(2, 3, 8, Some(16));
        assert!(failing.contains("已更新 2 篇"), "{failing}");
        assert!(failing.contains("失败 8 篇"), "{failing}");
        assert!(failing.contains("间隔已拉到 16 秒"), "{failing}");

        // 并发分支没有间隔，别硬塞一句「每篇间隔 0 秒」
        let concurrent = synopsis_progress_title(4, 2, 0, None);
        assert_eq!(concurrent, "已更新 4 篇 · 作者没写简介 2 篇");
        assert_eq!(
            synopsis_progress_title(1, 0, 0, Some(0)),
            "已更新 1 篇 · 作者没写简介 0 篇"
        );
    }

    /// v1.2.8：字数缓存落库 + 路径一变就作废。
    ///
    /// 起因是「所有作品」加载得有点慢 —— 原来每跑一次列表（每次改筛选、每敲一下搜索）
    /// 都要把全库的正文读一遍来数字数，真机 1350 篇 / 98 MB 要 1.4 秒。
    /// 现在算一次写进 `works.word_count`，列表只读这一列；算不出来的记 0（不会重算），
    /// 只有路径变了才由触发器标回 -1 重新排队。
    #[test]
    fn word_count_is_cached_and_invalidated_when_the_path_changes() {
        let root = std::env::temp_dir().join(format!("word-count-cache-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let text = root.join("书.txt");
        fs::write(&text, "一二三四五").unwrap();
        let text_path = text.to_string_lossy().to_string();

        let conn = Connection::open_in_memory().unwrap();
        create_full_work_tables(&conn);
        conn.execute_batch(WORD_COUNT_TRIGGER_DDL).unwrap();
        conn.execute(
            "INSERT INTO works (id, title, preview_path) VALUES (1, '甲', ?1)",
            [text_path.as_str()],
        )
        .unwrap();

        // limit = 0：只体检不干活，剩一篇待算
        let probe = refresh_word_counts_impl(&conn, 0).unwrap();
        assert_eq!(probe.updated, 0);
        assert_eq!(probe.remaining, 1);

        let done = refresh_word_counts_impl(&conn, 10).unwrap();
        assert_eq!(done.updated, 1);
        assert_eq!(done.remaining, 0);
        let stored: i64 = conn
            .query_row("SELECT word_count FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(stored > 0, "算出来的字数要落库，实际 {stored}");
        // 已经算过的不会重复排队（-1 才排，0 和正数都不排）
        assert_eq!(refresh_word_counts_impl(&conn, 10).unwrap().updated, 0);

        // 改无关列不该把缓存清掉（触发器里那句 WHEN 就是为这个）
        conn.execute("UPDATE works SET title='乙' WHERE id=1", [])
            .unwrap();
        let stored: i64 = conn
            .query_row("SELECT word_count FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(stored > 0, "改标题不该作废字数缓存");

        // 绑定换到另一份文件 —— 字数缓存必须作废，标回 -1 重新排队
        conn.execute(
            "UPDATE works SET preview_path='D:/书库/别的.txt' WHERE id=1",
            [],
        )
        .unwrap();
        let stored: i64 = conn
            .query_row("SELECT word_count FROM works WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, -1, "路径变了字数缓存必须作废");
        assert_eq!(refresh_word_counts_impl(&conn, 0).unwrap().remaining, 1);
    }

    /// v1.2.8：「系列连读」的进度把「在读」也算进去。
    ///
    /// 之前只数 `read_state = 2`，而这套软件打开一篇只会把它推到「在读」——
    /// 外部阅读器读完不会回调回来。于是进度永远停在 0，用户报的就是「这个已读一直是零」。
    #[test]
    fn series_read_count_treats_in_progress_as_read() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE series_catalog (
                 author_id INTEGER NOT NULL,
                 id TEXT NOT NULL,
                 title TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (author_id, id)
             );
             CREATE TABLE works (
                 id INTEGER PRIMARY KEY,
                 author_id INTEGER NOT NULL DEFAULT 1,
                 series_id TEXT NOT NULL DEFAULT '',
                 series_order INTEGER NOT NULL DEFAULT 0,
                 read_state INTEGER NOT NULL DEFAULT 0,
                 purchased_path TEXT NOT NULL DEFAULT '',
                 cover_path TEXT NOT NULL DEFAULT ''
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO series_catalog (author_id, id, title) VALUES (1, 's1', '长夜')",
            [],
        )
        .unwrap();
        // 5 篇：1 篇已读(2)、2 篇在读(1)、2 篇没碰过(0)
        conn.execute(
            "INSERT INTO works (author_id, series_id, read_state) VALUES
               (1, 's1', 2), (1, 's1', 1), (1, 's1', 1), (1, 's1', 0), (1, 's1', 0)",
            [],
        )
        .unwrap();
        let series = list_series_impl(&conn, 1).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].work_count, 5);
        assert_eq!(series[0].read_count, 3, "已读 1 + 在读 2 都要算「读过」");
        // 5 篇都没排序号（series_order 默认 0）—— 那不是「第 0 篇」，不该报成缺口
        assert!(series[0].gap_orders.is_empty(), "没排序号 ≠ 缺号");
    }

    #[test]
    fn series_gap_orders_report_the_empty_numbers() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE series_catalog (
                 author_id INTEGER NOT NULL,
                 id TEXT NOT NULL,
                 title TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (author_id, id)
             );
             CREATE TABLE works (
                 id INTEGER PRIMARY KEY,
                 author_id INTEGER NOT NULL DEFAULT 1,
                 series_id TEXT NOT NULL DEFAULT '',
                 series_order INTEGER NOT NULL DEFAULT 0,
                 read_state INTEGER NOT NULL DEFAULT 0,
                 purchased_path TEXT NOT NULL DEFAULT '',
                 cover_path TEXT NOT NULL DEFAULT ''
             );",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO series_catalog (author_id, id, title) VALUES (1, 's1', '有缺口'), (1, 's2', '连着的'), (1, 's3', '没排序号');
             INSERT INTO works (author_id, series_id, series_order) VALUES
               (1, 's1', 1), (1, 's1', 4), (1, 's1', 5),
               (1, 's2', 1), (1, 's2', 2),
               (1, 's3', 0), (1, 's3', 0);
             -- 另一位作者的号码不该串进来
             INSERT INTO works (author_id, series_id, series_order) VALUES (2, 's1', 3);",
        )
        .unwrap();
        let series = list_series_impl(&conn, 1).unwrap();
        let by_id = |id: &str| series.iter().find(|item| item.id == id).unwrap();
        assert_eq!(by_id("s1").max_order, 5);
        assert_eq!(by_id("s1").gap_orders, vec![2, 3], "1/4/5 之间缺 2、3");
        assert!(by_id("s2").gap_orders.is_empty(), "1、2 连着，没有缺口");
        assert!(
            by_id("s3").gap_orders.is_empty(),
            "序号 0 是「还没排进系列」，不该被当成第 0 篇"
        );
    }

    #[test]
    fn anthology_slots_only_take_images_that_are_actually_on_disk() {
        let root = std::env::temp_dir().join(format!("anthology-slots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let images = root.join("书_images");
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("001.jpg"), b"a").unwrap();
        fs::write(images.join("002.png"), b"b").unwrap();
        let text_path = root.join("书.txt");

        let content = "开头\n[插图 1：书_images/001.jpg]\n中间\n[引用插画 2：书_images/002.png]\n\
                       [插图 3：书_images/003.jpg]\n[a pixiv 原始标记 leftover: 这里不该当图]\n\
                       [uploads]\n结尾";
        let (slots, bundled) = anthology_image_slots(content, &text_path, 0);
        // 003.jpg 不在盘上 → 这个槽位整个丢掉，正文里那句就留成普通文字
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].file_name, "c01_001.jpg");
        assert!(!slots[0].external);
        assert_eq!(slots[1].file_name, "c01_002.png");
        assert!(slots[1].external, "「引用插画」要按外链插画标注");
        assert_eq!(bundled.len(), 2);
        assert_eq!(bundled[1].1, b"b");
        // 槽位的 token 必须是正文里原样的那一段，render 时靠它做替换
        assert_eq!(slots[0].token, "[插图 1：书_images/001.jpg]");
        // 换个章节号，包内文件名跟着变，跨章不会撞
        let (second, _) = anthology_image_slots(content, &text_path, 7);
        assert_eq!(second[0].file_name, "c08_001.jpg");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn filter_view_names_are_trimmed_and_length_checked() {
        assert_eq!(clean_filter_view_name("  待读长篇  ").unwrap(), "待读长篇");
        assert!(clean_filter_view_name("   ").is_err());
        assert!(clean_filter_view_name(&"字".repeat(FILTER_VIEW_MAX_NAME)).is_ok());
        assert!(clean_filter_view_name(&"字".repeat(FILTER_VIEW_MAX_NAME + 1)).is_err());
    }

    #[test]
    fn document_text_drops_script_and_style_content() {
        let html = "<html><head><style>body{color:red}</style>\
                    <script>function boot(){return 1}</script></head>\
                    <body><p>正文第一段</p><p>第二段</p></body></html>";
        let text = document_text(html);
        assert!(text.contains("正文第一段"));
        assert!(text.contains("第二段"));
        // 只丢标签的话，JS / CSS 原文会被当成正文搜到 —— 这是这个函数存在的理由
        assert!(!text.contains("function"), "script 内容必须整个丢掉：{text}");
        assert!(!text.contains("color"), "style 内容必须整个丢掉：{text}");
    }

    #[test]
    fn element_block_strip_survives_missing_close_tag() {
        assert_eq!(strip_element_block("<p>留着</p>", "script"), "<p>留着</p>");
        assert_eq!(
            strip_element_block("<p>前</p><script>没闭合", "script"),
            "<p>前</p>"
        );
        // 大小写混写也要认出来（HTML 不区分大小写）
        assert_eq!(
            strip_element_block("A<SCRIPT>x</Script>B", "script"),
            "AB"
        );
    }

    #[test]
    fn text_snippets_are_cut_on_character_boundaries() {
        // 中文一个字三字节：按字节切会切出半个字，轻则乱码、重则 panic
        let text = "前面的话。".repeat(20) + "关键句子" + &"后面的话。".repeat(20);
        let (count, snippets) = text_search_matches(&text, "关键句子");
        assert_eq!(count, 1);
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].hit, "关键句子");
        assert!(snippets[0].before.ends_with('。'), "{:?}", snippets[0].before);
        assert!(snippets[0].after.starts_with('后'), "{:?}", snippets[0].after);
        // 前后文各截到 N 个字符，不是整段
        assert_eq!(snippets[0].before.chars().count(), TEXT_SEARCH_CONTEXT_CHARS);
    }

    #[test]
    fn text_search_counts_every_hit_but_keeps_few_snippets() {
        let text = "书".repeat(50);
        let (count, snippets) = text_search_matches(&text, "书");
        assert_eq!(count, 50);
        assert_eq!(snippets.len(), TEXT_SEARCH_MAX_SNIPPETS);
        assert_eq!(single_line("  换行\n和  空格 "), "换行 和 空格");
    }

    #[test]
    fn epub_text_entries_take_content_files_only() {
        assert!(is_epub_text_entry("OEBPS/chapter001.xhtml"));
        assert!(is_epub_text_entry("text/part.html"));
        assert!(is_epub_text_entry("note.TXT"));
        // 图片（哪怕是最大的那几个）绝不该被当正文读进来
        assert!(!is_epub_text_entry("OEBPS/images/001.jpg"));
        assert!(!is_epub_text_entry("OEBPS/cover.png"));
        assert!(!is_epub_text_entry("OEBPS/content.opf"));
        // 只在别的地方出现的同类目录
        assert!(!is_epub_text_entry("__MACOSX/._chapter.xhtml"));
        assert!(!is_epub_text_entry("OEBPS/"));
    }

    /// 搭一个真会走盘的临时素材集：纯文本 / HTML / EPUB 各一份
    fn text_search_fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("text-search-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn text_search_document_reads_txt_html_and_epub() {
        let root = text_search_fixture("read");

        let txt = root.join("纯文本.txt");
        fs::write(&txt, "第一段\n关键句子在这里\n").unwrap();
        assert!(text_search_document(&txt).unwrap().contains("关键句子"));

        // HTML：标签和脚本内容都要剥掉
        let html = root.join("网页.html");
        fs::write(&html, "<p>关键句子</p><script>var 关键句子=1</script>").unwrap();
        assert!(text_search_document(&html).unwrap().contains("关键句子"));

        // EPUB：正文在内页里，图片条目一个字节都不该被读进来。
        // 名字特意跟上面那个 txt 岔开 —— 同名 .txt 会被优先取走（下面单独验那条）。
        let epub = root.join("电子书.epub");
        {
            let mut writer = zip::ZipWriter::new(fs::File::create(&epub).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("OEBPS/chapter1.xhtml", options).unwrap();
            writer.write_all("<p>关键句子在正文里</p>".as_bytes()).unwrap();
            writer.start_file("OEBPS/images/001.jpg", options).unwrap();
            writer.write_all(b"not-really-a-jpeg").unwrap();
            writer.finish().unwrap();
        }
        let text = text_search_document(&epub).unwrap();
        assert!(text.contains("关键句子在正文里"));
        assert!(!text.contains("not-really"), "图片条目不该被当正文读：{text}");

        // 同名 .txt 优先 —— 比 epub 里那堆 xhtml 干净，也省一次解压
        fs::write(root.join("电子书.txt"), "同名文本优先").unwrap();
        assert!(text_search_document(&epub).unwrap().contains("同名文本优先"));

        // 目录：找里面的 txt
        let folder = root.join("目录版");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("正文.txt"), "目录里的关键句子").unwrap();
        assert!(text_search_document(&folder).unwrap().contains("目录里的关键句子"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn full_text_search_reports_hits_missing_and_truncation() {
        let root = text_search_fixture("scan");
        let first = root.join("一.txt");
        fs::write(&first, "关键句子。又一句关键句子。").unwrap();
        let second = root.join("二.txt");
        fs::write(&second, "这里也有关键句子").unwrap();
        let gone = root.join("没了.txt"); // 故意不建：模拟绑定了但文件丢了

        let rows = vec![
            (
                1i64,
                "第一篇".to_string(),
                first.to_string_lossy().into_owned(),
                String::new(),
            ),
            (
                2,
                "第二篇".to_string(),
                second.to_string_lossy().into_owned(),
                String::new(),
            ),
            (
                3,
                "文件丢了".to_string(),
                gone.to_string_lossy().into_owned(),
                String::new(),
            ),
        ];

        let result = search_full_text_over(&rows, "关键句子", None).unwrap();
        assert_eq!(result.scanned_count, 2, "只有两篇真有正文");
        assert_eq!(result.hits.len(), 2);
        // 命中多的排前面：第一篇有两处
        assert_eq!(result.hits[0].work_id, 1);
        assert_eq!(result.hits[0].hit_count, 2);
        assert_eq!(result.hits[0].snippets[0].hit, "关键句子");
        assert!(!result.truncated);
        // 文件不在原处的作品要点名报出来，不能静默跳过
        assert_eq!(result.missing_count, 1);
        assert_eq!(result.missing, vec!["文件丢了".to_string()]);

        // 上限：命中两篇但只留一篇，得给截断标记
        let capped = search_full_text_over(&rows, "关键句子", Some(1)).unwrap();
        assert_eq!(capped.hits.len(), 1);
        assert!(capped.truncated);

        // 空词直接报错，别让一次全库扫描白跑
        assert!(search_full_text_over(&rows, "   ", None).is_err());

        let _ = fs::remove_dir_all(&root);
    }

    /// 拿**真库**跑一遍，只关心两件事：扫得动、耗时是亚秒级。
    ///
    /// 平时不跑（`#[ignore]`）：它会读一整个库的正文（含几个 G 的 epub），
    /// 不该拖慢日常 `cargo test`。想手动量一遍：
    /// `WB_REAL_LIBRARY_DB="…\library.db" cargo test -- --ignored --nocapture real_library`
    #[test]
    #[ignore]
    fn real_library_full_text_search_stays_subsecond() {
        let Ok(path) = std::env::var("WB_REAL_LIBRARY_DB") else {
            eprintln!("没设 WB_REAL_LIBRARY_DB，跳过");
            return;
        };
        if !Path::new(&path).is_file() {
            eprintln!("库文件不存在：{path}");
            return;
        }
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("打不开库");
        let rows: Vec<(i64, String, String, String)> = {
            let mut statement = conn
                .prepare(
                    "SELECT id, COALESCE(title,''), COALESCE(purchased_path,''), COALESCE(preview_path,'')
                     FROM works",
                )
                .unwrap();
            let collected = statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            collected
        };
        drop(conn);

        // 「的」是最坏情况：几乎每篇都命中，且每篇都要数到最后一个字
        let result = search_full_text_over(&rows, "的", None).unwrap();
        eprintln!(
            "作品 {} 篇 / 扫过正文 {} 篇 / 命中 {} 篇 / 没正文 {} 篇 / 用时 {} ms",
            rows.len(),
            result.scanned_count,
            result.hits.len(),
            result.missing_count,
            result.elapsed_ms
        );
        assert!(result.scanned_count > 0, "一篇正文都没读到，路径口径有问题");
        assert!(
            result.elapsed_ms < 3000,
            "整库扫描慢到 {} ms，超出「亚秒级」的预期",
            result.elapsed_ms
        );
    }
}
