import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { HELP_DOC_CSS, HELP_DOC_HTML } from "./help-doc.js";
import "./styles.css";

const app = document.querySelector("#app");
const APP_VERSION = "1.1.0";
const state = {
  authors: [],
  activeAuthor: null,
  works: [],
  authorQuery: "",
  workQuery: "",
  /**
   * 「所有作品」自己那份搜索词（v0.3.68 从 workQuery 拆出来）：
   * 点作者名跳进作品库再返回时，能把离开前的搜索词还原回来。
   */
  allWorksQuery: "",
  searchField: "title",
  status: "all",
  authorFavoritesOnly: false,
  authorsStarredOnly: false,
  allWorksFavoritesOnly: false,
  authorImagesOnly: false,
  allWorksImagesOnly: false,
  serialLatestOnly: false,
  sort: "date_desc",
  bulkMode: false,
  selectedWorkIds: new Set(),
  pendingConfirmation: null,
  /** 等待用户选择"匹配角色 / 匹配标题"的操作意图 */
  pendingMatch: null,
  /** 常见角色名列表（按游戏分组） */
  characters: [],
  /** 设置里配的「完整版搜索网站」：选中标题右键时的搜索入口，启动时读一次、保存设置后刷新 */
  searchSites: [],
  /** 启动时读回来的整份设置（自动更新要不要跑得看它） */
  settings: null,
  /** 设置面板自动保存的防抖定时器：关弹窗时要抢在 DOM 被移除之前把改动落盘 */
  settingsSaveTimer: 0,
  seriesView: null,
  seriesItems: [],
  /**
   * 作者作品库是从哪儿进来的（v0.3.68）：从「所有作品」点作者名进来时记 "allWorks"，
   * 这样左上角返回按钮回到「所有作品」，而不是默认的「作者库」列表。
   */
  authorReturnTo: null,
  homeView: "authors",
  allWorks: [],
  syncTask: null,
  /**
   * 自动更新（v1.0.0 起）：`check_for_update` 的结果。有新版时左下角版本号会变成
   * 「↑ v1.0.1」按钮；`progress` 是下载进度（下载中才非空）。
   */
  update: null,
  updateProgress: null,
  updateDownloading: false,
  /**
   * 更新说明（Release 正文）的取回状态：`{ version, state: "loading"|"ready"|"error", title, notes, source, error }`。
   * 只在真的有新版时才去取 —— 没新版时一次请求都不发。
   */
  updateNotes: null,
  /** 卡片按下时的坐标：用来区分「点击」和「按住拖动选中文字」 */
  cardPressPoint: null,
  /** 单击打开文件前的短定时器句柄：双击选中标题里的词时会把它取消掉 */
  pendingCardOpen: 0,
  /** 右键按下那一刻卡片里选中的文字（浏览器随后会清掉选区，所以先拍下来） */
  cardSelectionText: "",
  /** 卡片「复制」菜单打开时的内容：点按钮时选区已经没了，只能靠这份快照 */
  copyPayload: null,
  /** 「我的收藏」页：收藏夹列表 / 当前打开的收藏夹 / 夹子里的作品 */
  collections: [],
  activeCollection: null,
  collectionWorks: [],
  collectionQuery: "",
  collectionImagesOnly: false,
  /** 收藏夹里默认按「什么时候收进来的」排，所以单独一个排序状态，不跟作者库共用 */
  collectionSort: "added_desc",
  /** 收藏夹选择器弹窗：正在改归属的作品 id + 已勾选的夹子 id */
  pickerWorkId: null,
  pickerSelected: [],
  /** 「浏览历史」页：`{ work, viewedAt, viewCount }` 列表 + 搜索词 */
  history: [],
  historyQuery: "",
};

const previewAuthors = [{ id: 1, name: "雾海档案", aliases: "雾海|档案屋", homepage: "https://www.pixiv.net/users/16208053", avatarPath: "", notes: "", previewDir: "D:\\预览", purchasedDir: "D:\\已购", matchThreshold: 70, workCount: 48, purchasedCount: 19, imagesCount: 6, favoriteCount: 7 }, { id: 2, name: "Mori", aliases: "", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 126, purchasedCount: 52, imagesCount: 14, favoriteCount: 16 }, { id: 3, name: "远野", aliases: "远野老师", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 33, purchasedCount: 8, imagesCount: 2, favoriteCount: 4 }];
const previewWorks = [{ id: 1, title: "（插画附+改编图文）～希儿&布洛妮娅", releaseDate: "2025-10-05", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\希儿.epub", wordCount: 12680, favorite: true }, { id: 2, title: "夏日短篇集", releaseDate: "2025-09-20", previewPath: "", coverPath: "", purchasedPath: "", wordCount: 4380, favorite: false }, { id: 3, title: "旧城的信", releaseDate: "2025-08-18", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\旧城的信", wordCount: 20750, favorite: false }, { id: 4, title: "月色图文辑", releaseDate: "2025-07-09", previewPath: "", coverPath: "", purchasedPath: "", favorite: true }];

previewWorks.forEach((work, index) => { work.tags = ["Pixiv|小说", "短篇|日常", "小说", "插画|图文"][index]; work.pixivNovelId = ["26410188", "", "", ""][index]; work.imageCount = [4, 0, 0, 0][index]; });
// 「我的收藏」与「浏览历史」在浏览器预览里的假数据（真数据是 SQL 出来的）
const previewCollections = [
  { id: 1, name: "我的收藏", createdAt: "2026-09-01T00:00:00+00:00" },
  { id: 2, name: "短篇向", createdAt: "2026-09-10T00:00:00+00:00" },
  { id: 3, name: "待读", createdAt: "2026-09-14T00:00:00+00:00" },
];
previewWorks[0].collectionIds = [1, 2];
previewWorks[1].collectionIds = [];
previewWorks[2].collectionIds = [3];
previewWorks[3].collectionIds = [1];
previewWorks.forEach((work) => { work.favorite = (work.collectionIds || []).length > 0; });
// 历史时间按「今天 / 昨天 / 3 天前」算出来，浏览器预览里分组标题才稳定
const previewDayAt = (offsetDays, hour, minute) => { const moment = new Date(); moment.setDate(moment.getDate() + offsetDays); moment.setHours(hour, minute, 0, 0); return moment.toISOString(); };
const previewHistory = [
  { workId: 1, viewedAt: previewDayAt(0, 9, 20), viewCount: 4 },
  { workId: 3, viewedAt: previewDayAt(-1, 21, 5), viewCount: 2 },
  { workId: 4, viewedAt: previewDayAt(-3, 17, 40), viewCount: 1 },
];
previewWorks.forEach((work, index) => { work.seriesId = index < 2 ? "demo-series-1" : ""; work.seriesTitle = index < 2 ? "雾海档案短篇系列" : ""; });
// 「所有作品」卡片要显示作者名、点作者名跳作品库，mock 里也得带上（真数据由 SQL JOIN 出来）
previewWorks.forEach((work, index) => { work.authorId = [1, 1, 2, 3][index]; work.authorName = ["雾海档案", "雾海档案", "Mori", "远野"][index]; });

previewWorks.forEach((work, index) => {
  work.seriesOrder = index < 2 ? index + 1 : 0;
  work.isNew = index === 0;
  work.authorName = index < 3 ? "雾海档案" : "Mori";
  work.authorId = index < 3 ? 1 : 2;
});

// 浏览器预览用的角色数据（真机数据在 Rust 侧的 characters 表里）
let previewCharacterGames = ["原神", "崩坏星穹铁道", "碧蓝航线"];
let previewCharacterNextId = 900;
let previewCharacters = [
  { id: 1, game: "原神", name: "甘雨", aliases: "Ganyu", heat: 19, source: "builtin", enabled: true },
  { id: 2, game: "原神", name: "雷电将军", aliases: "影", heat: 16, source: "builtin", enabled: true },
  { id: 3, game: "原神", name: "胡桃", aliases: "", heat: 14, source: "builtin", enabled: false },
  { id: 4, game: "崩坏星穹铁道", name: "希儿", aliases: "Seele", heat: 22, source: "builtin", enabled: true },
  { id: 5, game: "崩坏星穹铁道", name: "布洛妮娅", aliases: "Bronya", heat: 18, source: "builtin", enabled: true },
  { id: 6, game: "碧蓝航线", name: "绫波", aliases: "綾波|Ayanami", heat: 12, source: "builtin", enabled: true },
];

async function invoke(command, args = {}) {
  if (window.__TAURI_INTERNALS__) return tauriInvoke(command, args);
  // 无头浏览器验证用：最后一份入参 + 完整调用流水（判断"有没有真的打开文件"要看流水）
  window.__lastInvoke = { command, args };
  window.__invokeLog = (window.__invokeLog || []).concat([{ command, args }]);
  if (command === "list_authors") return previewAuthors;
  if (command === "list_works") {
    return previewWorks.filter((work) => (!args.query || work.title.includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite));
  }
  if (command === "list_all_works") return previewWorks.filter((work) => (!args.query || (args.searchField === "tags" ? work.tags : work.title).includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite));
  if (command === "list_series_works") return previewWorks.filter((work) => work.seriesId === args.seriesId);
  if (command === "list_series") return [{ id: "demo-series-1", title: "雾海档案短篇系列", workCount: 2, purchasedCount: 1, previewCount: 1, coverPath: "", maxOrder: 2 }];
  if (command === "set_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = args.seriesId; work.seriesTitle = "雾海档案短篇系列"; work.seriesOrder = args.seriesOrder; } return; }
  if (command === "leave_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = ""; work.seriesTitle = ""; work.seriesOrder = 0; } return; }
  // 收藏夹：真数据在 collection_works 关联表里，mock 里挂在作品对象上（collectionIds）
  if (command === "list_collections") return previewCollections.map((collection) => {
    const members = previewWorks.filter((work) => (work.collectionIds || []).includes(collection.id));
    return { id: collection.id, name: collection.name, createdAt: collection.createdAt, workCount: members.length, coverPath: members[0]?.coverPath || "" };
  });
  if (command === "create_collection") {
    const id = previewCollections.reduce((max, item) => Math.max(max, item.id), 0) + 1;
    const createdAt = new Date().toISOString();
    previewCollections.push({ id, name: args.name, createdAt });
    return { id, name: args.name, workCount: 0, coverPath: "", createdAt };
  }
  if (command === "rename_collection") { const target = previewCollections.find((item) => item.id === args.id); if (target) target.name = args.name; return { id: args.id, name: args.name, workCount: 0, coverPath: "", createdAt: "" }; }
  if (command === "delete_collection") {
    const index = previewCollections.findIndex((item) => item.id === args.id);
    if (index >= 0) previewCollections.splice(index, 1);
    previewWorks.forEach((work) => { work.collectionIds = (work.collectionIds || []).filter((id) => id !== args.id); work.favorite = work.collectionIds.length > 0; });
    return;
  }
  if (command === "work_collections") return previewWorks.find((work) => work.id === args.workId)?.collectionIds || [];
  if (command === "set_work_collections") {
    const work = previewWorks.find((item) => item.id === args.workId);
    if (work) { work.collectionIds = [...args.collectionIds]; work.favorite = work.collectionIds.length > 0; }
    return;
  }
  if (command === "list_collection_works") return previewWorks.filter((work) => (work.collectionIds || []).includes(args.collectionId) && (!args.query || work.title.includes(args.query)) && (!args.imagesOnly || work.hasImages));
  // 浏览历史：mock 里存 workId，出口时再挂上作品对象（真机是一条 SQL JOIN 出来）
  if (command === "list_history") return previewHistory.map((entry) => ({ ...entry, work: previewWorks.find((work) => work.id === entry.workId) })).filter((entry) => entry.work && (!args.query || entry.work.title.includes(args.query)));
  if (command === "clear_history") { previewHistory.length = 0; return; }
  if (command === "remove_history") { const index = previewHistory.findIndex((entry) => entry.workId === args.workId); if (index >= 0) previewHistory.splice(index, 1); return; }
  if (command === "toggle_has_images") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.hasImages = !work.hasImages; return; }
  if (command === "delete_work") { const index = previewWorks.findIndex((item) => item.id === args.workId); if (index >= 0) previewWorks.splice(index, 1); return; }
  if (command === "delete_works") { for (const workId of args.workIds) { const index = previewWorks.findIndex((item) => item.id === workId); if (index >= 0) previewWorks.splice(index, 1); } return; }
  if (command === "set_match_threshold") { const author = previewAuthors.find((item) => item.id === args.authorId); if (author) author.matchThreshold = args.threshold; return author; }
  if (command === "toggle_author_starred") { const author = previewAuthors.find((item) => item.id === args.authorId); if (author) author.starred = !author.starred; return Boolean(author?.starred); }
  if (command === "set_author_order") {
    const byId = new Map(previewAuthors.map((author) => [author.id, author]));
    const ordered = args.authorIds.map((id) => byId.get(id)).filter(Boolean);
    if (ordered.length === previewAuthors.length) previewAuthors.splice(0, previewAuthors.length, ...ordered);
    return;
  }
  // 搜索网站给了两条示例，方便在浏览器里直接看设置面板和右键菜单长什么样
  if (command === "get_app_settings") return { pixivCookie: "", excludedTags: "", defaultPreviewDir: "", defaultPurchasedDir: "", autoGroupDir: "", autoCreateDirs: false, minimumFileSizeBytes: 0, pixivDelayThreshold: 150, pixivDelaySeconds: 1, similarityThreshold: 70, minSimilarityThreshold: 30, matchTitleLength: 0, imageQuality: "1200", syncImageFormat: "html", autoCheckUpdate: true, recordHistory: true, updateMirrors: ["https://ghproxy.net/", "https://gh-proxy.com/", "https://ghfast.top/", "https://gh.xxooo.cf/"], searchSites: [{ name: "书香", url: "https://sxsy45.com/search.php?mod=forum&searchid=83552&orderby=dateline&ascdesc=desc&searchsubmit=yes&kw=%E5%9B%BE" }, { name: "示例站", url: "https://example.com/search?q=" }] };
  if (command === "save_app_settings") return args.settings;
  // 浏览器预览默认当作「已是最新版」；要看更新界面就把 window.__mockUpdateCheck 塞进来
  if (command === "check_for_update") return window.__mockUpdateCheck || { currentVersion: APP_VERSION, latestVersion: APP_VERSION, hasUpdate: false, releaseUrl: "https://github.com/fromzero1501/pixiv-novel-downloader/releases", releaseMirrorUrl: "https://ghproxy.net/https://github.com/fromzero1501/pixiv-novel-downloader/releases", asset: null, source: "直连 GitHub" };
  if (command === "download_update") {
    window.__mockUpdateDownload = { version: args.version, assetName: args.assetName, urls: args.urls };
    return `D:\\示例\\程序目录\\${String(args.assetName || "").replace(/\.zip$/i, ".exe")}`;
  }
  if (command === "apply_update") { window.__mockUpdateApplied = args.path; return; }
  // 更新说明：默认给一段像样的示例；要看「取不到」的样子就设 window.__mockReleaseNotesError
  if (command === "fetch_release_notes") {
    if (window.__mockReleaseNotesError) throw new Error(window.__mockReleaseNotesError);
    return window.__mockReleaseNotes || {
      version: args.version,
      title: `v${args.version} —— 自动更新 + 内置图文帮助`,
      notes: "软件自动更新（可配置加速镜像）\n- 版本识别走发布页跳转解析，不调用 GitHub API\n- 内置 4 条国内镜像，查版本与下载都自动回退\n\n帮助文档不再外置\n- 应用内帮助页用 Shadow DOM 装载，截图不再变形\n- 删除外置 help.html（文件取不到时不影响使用）",
      source: "直连 GitHub",
    };
  }
  if (command === "cleanup_old_portable_builds") return { removed: [], failed: [] };
  if (command === "default_update_mirror_list") return ["https://ghproxy.net/", "https://gh-proxy.com/", "https://ghfast.top/", "https://gh.xxooo.cf/"];
  if (command === "refresh_reading_image_counts") return { scannedCount: 0, updatedCount: 0 };
  if (command === "read_pixiv_cookie_file") return "";
  if (command === "sync_pixiv_author_profile") return { id: args.authorId ?? null, name: "Pixiv 作者", homepage: args.homepage, avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, pixivLastSyncAt: "", avatarManaged: false };
  if (command === "update_work_tags") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.tags = args.tags.join("|"); return; }
  if (command === "copy_previews_to_purchased") return { copiedCount: args.workIds.length, boundCount: args.workIds.length, skippedCount: 0 };
  // 真机打开的是作品绑定的阅读版文件；浏览器预览里没有本地文件，什么都不做
  if (command === "open_work_reading") return;
  if (command === "redownload_novel_txt") {
    // 真机是从 Pixiv 重抓正文写成 txt，这里只回一个像样的路径让界面能跑通
    const work = previewWorks.find((item) => item.id === args.workId);
    return `D:\\400 个人\\小说\\已购版\\希儿\\${(work?.title || "示例作品").slice(0, 12)}.txt`;
  }
  if (command === "cleanup_redundant_previews") {
    const scan = { candidates: 3, ready: 2, skippedMissingFull: 1, cleaned: 2, recordOnly: 1, recycled: 5, recycleFailed: 0, imagesMoved: 1, coversMoved: 2, emptyDirs: 1 };
    if (!args.apply) return { ...scan, recycled: 0, imagesMoved: 0, coversMoved: 0, emptyDirs: 0 };
    return scan;
  }
  if (command === "download_reading_version") {
    const format = args.format === "epub" ? "epub" : "html";
    const work = previewWorks.find((item) => item.id === args.workId);
    const outputPath = `D:\\已购\\希儿.${format}`;
    if (work) {
      work.imageCount = work.imageCount || 4;
      work.hasImages = true;
      if (work.purchasedPath) work.purchasedPath = outputPath; else work.previewPath = outputPath;
    }
    return { format, outputPath, totalCount: 4, savedCount: 4, failedCount: 0, missingCount: 0, sizeBytes: format === "epub" ? 3355443 : 0 };
  }
  if (command === "download_reading_versions") {
    // format 传空串＝跟随设置，这里也照着设置里选的走，方便浏览器里试用
    const format = args.format || (await invoke("get_app_settings")).syncImageFormat || "html";
    // 稍微等一下，好让进度浮层在浏览器预览里也能看清（真机上是真实耗时）
    await new Promise((resolve) => setTimeout(resolve, 700));
    return { format, exportedCount: args.workIds.length, skippedCount: 0, failedCount: 0, imageCount: args.workIds.length * 4, totalBytes: args.workIds.length * 3355443, lastPath: `D:\\已购\\希儿.${format}`, failedTitles: [] };
  }
  if (command === "scan_purchased") {
    const byCharacter = args.mode === "character";
    // 故意给一个超长文件名，用来在浏览器预览里检查弹窗宽度与省略号
    return {
      boundCount: byCharacter ? 0 : 2,
      skippedCount: 12,
      selections: [
        {
          path: "D:\\完整版\\这是一个特别特别长的小说文件名用来验证关联弹窗会不会被撑宽（插画附+改编图文）～整份合集～第一卷到第七卷全收录～特别加长版用来触发省略号显示效果确认没有横向滚动条",
          authorName: "雾海档案",
          matchedCharacters: byCharacter ? ["希儿", "布洛妮娅"] : [],
          candidates: [
            { workId: 1, title: "（插画附+改编图文）～希儿&布洛妮娅", similarity: 92, matchedCharacters: byCharacter ? ["希儿", "布洛妮娅"] : [] },
            { workId: 2, title: "夏日短篇集", similarity: 61, matchedCharacters: [] },
            { workId: 4, title: "月色图文辑～甘雨", similarity: 44, matchedCharacters: byCharacter ? ["甘雨"] : [] },
          ],
        },
      ],
      unknownAuthorGroups: [
        { author: "藤原さくら", files: ["D:\\完整版\\【作者：藤原さくら】甘雨と旅人の夜.txt", "D:\\完整版\\【作者：藤原さくら】绫波、結婚します.epub"] },
        { author: "夜凪", files: ["D:\\完整版\\【作者：夜凪】长离的雨夜.txt"] },
      ],
    };
  }
  if (command === "auto_group_purchased_files") {
    const byCharacter = args.mode === "character";
    return {
      autoMovedCount: byCharacter ? 0 : 3,
      manualSelections: [
        {
          filePath: "D:\\待分组\\【作者：雾海档案】希儿与布洛妮娅的假日.txt",
          fileName: "【作者：雾海档案】希儿与布洛妮娅的假日.txt",
          subDir: "",
          authorName: "雾海档案",
          matchedCharacters: byCharacter ? ["希儿", "布洛妮娅"] : [],
          candidates: [
            // 按角色匹配时后端不会给「推荐」（recommended 恒为 false），这里跟着一样，免得预览里看到假的默认勾选
            { authorId: 1, authorName: "雾海档案", workId: 1, workTitle: "（插画附+改编图文）～希儿&布洛妮娅", similarity: 74, recommended: !byCharacter, conflict: false, matchedCharacters: byCharacter ? ["希儿", "布洛妮娅"] : [] },
            { authorId: 2, authorName: "Mori", workId: 2, workTitle: "夏日短篇集", similarity: 38, recommended: false, conflict: true, matchedCharacters: [] },
          ],
        },
      ],
      unknownAuthorGroups: [
        { author: "藤原さくら", files: ["D:\\待分组\\【作者：藤原さくら】甘雨と旅人の夜.txt"] },
      ],
    };
  }
  if (command === "list_characters") {
    const games = previewCharacterGames.slice();
    previewCharacters.forEach((entry) => { if (!games.includes(entry.game)) games.push(entry.game); });
    return games.map((game) => ({ game, characters: previewCharacters.filter((entry) => entry.game === game) }));
  }
  if (command === "add_character") {
    if (previewCharacters.some((entry) => entry.game === args.game && entry.name === args.name)) throw new Error(`「${args.game}」里已经有「${args.name}」了`);
    previewCharacters.push({ id: previewCharacterNextId++, game: args.game, name: args.name, aliases: args.aliases || "", heat: 0, source: "user", enabled: true });
    if (!previewCharacterGames.includes(args.game)) previewCharacterGames.push(args.game);
    return;
  }
  if (command === "add_character_game") { if (!previewCharacterGames.includes(args.game)) previewCharacterGames.push(args.game); return; }
  if (command === "update_character") { const entry = previewCharacters.find((item) => item.id === args.id); if (entry) { entry.name = args.name; entry.aliases = args.aliases; entry.enabled = args.enabled; } return; }
  if (command === "delete_character") { previewCharacters = previewCharacters.filter((item) => item.id !== args.id); return; }
  if (command === "rename_character_game") {
    previewCharacters.forEach((entry) => { if (entry.game === args.game) entry.game = args.next; });
    previewCharacterGames = previewCharacterGames.map((game) => (game === args.game ? args.next : game));
    return;
  }
  if (command === "delete_character_game") {
    previewCharacters = previewCharacters.filter((entry) => entry.game !== args.game);
    previewCharacterGames = previewCharacterGames.filter((game) => game !== args.game);
    return;
  }
  if (command === "bind_work_with_rename") return args.path;
  if (command === "sync_pixiv_novels") return { downloadedCount: 0, reusedPreviewCount: 0, skippedExistingCount: 0, skippedDateCount: 0, skippedSizeCount: 0, failedCount: 0, lastSyncAt: new Date().toISOString() };
  if (command === "open_work") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.isNew = false; return; }
  if (command === "open_external_url") { window.open(args.url, "_blank", "noopener,noreferrer"); return; }
  if (command === "open_search_site") {
    // 和 Rust 侧同一条规则：最后一个 = 后面的内容清掉，填进编码后的搜索词
    const index = String(args.url || "").lastIndexOf("=");
    const target = (index >= 0 ? String(args.url).slice(0, index + 1) : `${args.url}=`) + encodeURIComponent(String(args.keyword || "").trim());
    window.open(target, "_blank", "noopener,noreferrer");
    return target;
  }
  throw new Error("普通浏览器预览仅展示界面；本地文件功能请在 Tauri 程序中使用。");
}

const icon = (name, size = 18) => {
  const paths = {
    plus: '<path d="M12 5v14M5 12h14"/>',
    search: '<circle cx="11" cy="11" r="6"/><path d="m16 16 4 4"/>',
    settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.04 2.04-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.02 1.56V20h-2.88v-.09A1.7 1.7 0 0 0 10.9 18.4a1.7 1.7 0 0 0-1.88.34l-.06.06-2.04-2.04.06-.06A1.7 1.7 0 0 0 7.32 14.8 1.7 1.7 0 0 0 5.76 13.8H5.7v-2.88h.06a1.7 1.7 0 0 0 1.56-1.02A1.7 1.7 0 0 0 6.98 8l-.06-.06L8.96 5.9l.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1.02-1.56V4.7h2.88v.06a1.7 1.7 0 0 0 1.02 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2.04 2.04-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1.02h.06v2.88H21a1.7 1.7 0 0 0-1.6 1.2Z"/>',
    gear: '<polygon points="21 16 12 21 3 16 3 8 12 3 21 8 21 16"/><circle cx="12" cy="12" r="3"/>',
    heart: '<path d="M20.8 8.6c0 5.1-8.8 10.5-8.8 10.5S3.2 13.7 3.2 8.6A4.4 4.4 0 0 1 12 8a4.4 4.4 0 0 1 8.8.6Z"/>',
    star: '<path d="m12 4 2.5 5.1 5.6.8-4 4 .9 5.6-5-2.7-5 2.7.9-5.6-4-4 5.6-.8Z"/>',
    starFilled: '<path d="m12 4 2.5 5.1 5.6.8-4 4 .9 5.6-5-2.7-5 2.7.9-5.6-4-4 5.6-.8Z"/>',
    check: '<path d="m5 12 4 4L19 6"/>',
    x: '<path d="m7 7 10 10M17 7 7 17"/>',
    folder: '<path d="M3 6.7A1.7 1.7 0 0 1 4.7 5H10l2 2h7.3A1.7 1.7 0 0 1 21 8.7v9.6a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 18.3Z"/>',
    upload: '<path d="M12 16V4M7 9l5-5 5 5"/><path d="M5 20h14"/>',
    more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
    arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
    back: '<path d="M19 12H5M11 18l-6-6 6-6"/>',
    up: '<path d="m6 14.5 6-6 6 6"/>',
    down: '<path d="m6 9.5 6 6 6-6"/>',
    database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v7c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12v7c0 1.7 3.6 3 8 3s8-1.3 8-3v-7"/>',
    image: '<rect x="3" y="4" width="18" height="16" rx="1"/><circle cx="8.5" cy="9" r="1.5"/><path d="m21 15-4.5-4.5L7 20"/>',
    sync: '<path d="M20 7v5h-5"/><path d="M4 17v-5h5"/><path d="M6.1 9a7 7 0 0 1 11.8-2L20 9M4 15l2.1 2A7 7 0 0 0 17.9 15"/>',
    tag: '<path d="M20 13.5 13.5 20a2.1 2.1 0 0 1-3 0L4 13.5V4h9.5L20 10.5a2.1 2.1 0 0 1 0 3Z"/><circle cx="8.5" cy="8.5" r="1"/>',
    series: '<rect x="4" y="5" width="16" height="14" rx="1"/><path d="M8 3v4M16 3v4M8 11h8M8 15h5"/>',
    file: '<path d="M7 3h7l4 4v14H7z"/><path d="M14 3v5h5M10 13h5M10 16h5"/>',
    help: '<circle cx="12" cy="12" r="9"/><path d="M9.8 9a2.3 2.3 0 1 1 3.8 1.7c-.9.7-1.6 1.2-1.6 2.5"/><path d="M12 16h.01"/>',
    link: '<path d="M10 13a5 5 0 0 0 7.5.5l2-2A5 5 0 0 0 12.5 4.5l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-2 2A5 5 0 0 0 11.5 19.5l1.7-1.7"/>',
    copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
    grip: '<circle cx="9.5" cy="6" r="1.5" fill="currentColor"/><circle cx="14.5" cy="6" r="1.5" fill="currentColor"/><circle cx="9.5" cy="12" r="1.5" fill="currentColor"/><circle cx="14.5" cy="12" r="1.5" fill="currentColor"/><circle cx="9.5" cy="18" r="1.5" fill="currentColor"/><circle cx="14.5" cy="18" r="1.5" fill="currentColor"/>',
    layers: '<path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 13 9 5 9-5"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5.2l3.2 1.9"/>',
    folderHeart: '<path d="M3 6.7A1.7 1.7 0 0 1 4.7 5H10l2 2h7.3A1.7 1.7 0 0 1 21 8.7v9.6a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 18.3Z"/><path d="M12 16.4c-1.8-1.2-2.8-2.4-2.8-3.6a1.5 1.5 0 0 1 2.8-.6 1.5 1.5 0 0 1 2.8.6c0 1.2-1 2.4-2.8 3.6Z"/>',
  };
  // 名字以 Filled 结尾的图标用实心填充（如 starFilled）
  const fill = name.endsWith("Filled") ? "currentColor" : "none";
  return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="${fill}" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
};

const appLogo = (size = 44) => `<svg width="${size}" height="${size}" viewBox="0 0 100 100" aria-hidden="true"><rect width="100" height="100" rx="25" fill="#1595E8"/><path fill="#fff" d="M25 14h27c21 0 34 13 34 33S73 80 52 80H41v10H25V14Zm16 16v34h10c11 0 18-6 18-17s-7-17-18-17H41Z"/><path d="M68 76c5-3 10-3 14 0v13c-4-3-9-3-14 0-5-3-10-3-14 0V76c4-3 9-3 14 0Z" fill="#1595E8" stroke="#fff" stroke-width="3.5" stroke-linejoin="round"/><path d="M68 76v13" fill="none" stroke="#fff" stroke-width="3" stroke-linecap="round"/></svg>`;

function escapeHtml(value = "") {
  return String(value).replace(/[&<>'"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" }[char]));
}

function asset(path) {
  return path ? convertFileSrc(path) : "";
}

function initials(name) {
  return (name || "新作者").trim().slice(0, 2);
}

function toast(message, tone = "info") {
  const element = document.createElement("div");
  element.className = `toast toast-${tone}`;
  element.textContent = message;
  document.body.append(element);
  window.setTimeout(() => element.remove(), 3200);
}

function dateLabel(value) {
  return value || "未解析日期";
}

function wordCountLabel(value) {
  if (!Number.isFinite(value) || value <= 0) return "";
  return `${new Intl.NumberFormat("zh-CN").format(value)} 字`;
}

function workContentMeta(work) {
  if (work.wordCount) return `<span class="work-word-count">${wordCountLabel(work.wordCount)}</span>`;
  if (work.fileFormat) return `<span class="work-word-count">${escapeHtml(work.fileFormat)}</span>`;
  return "";
}

function syncLabel(value) {
  return value ? `上次同步 ${value.replace("T", " ").slice(0, 16)}` : "尚未同步";
}

async function refreshAuthors() {
  state.authors = await invoke("list_authors");
}

/**
 * 「完整版搜索网站」只在设置里改，但作品卡右键菜单要用它，
 * 所以启动时读一次缓存进 state，保存设置后再刷新，右键就不用等异步。
 * 这里顺手清洗一遍：老版本存下来的网址可能带着 searchid（清掉才有用）。
 */
async function loadSearchSites() {
  try {
    const settings = await invoke("get_app_settings");
    // 整份留着：启动时要不要自动查更新得看 autoCheckUpdate，不必再跑一趟
    state.settings = settings || {};
    state.searchSites = Array.isArray(settings?.searchSites)
      ? settings.searchSites.filter((site) => site?.name && site?.url).map((site) => ({ name: site.name, url: cleanSearchUrl(site.url) }))
      : [];
  } catch {
    state.settings = {};
    state.searchSites = [];
  }
}

async function refreshActiveAuthor() {
  await refreshAuthors();
  if (state.activeAuthor) {
    state.activeAuthor = state.authors.find((author) => author.id === state.activeAuthor.id) || state.activeAuthor;
  }
}

async function refreshWorks() {
  if (!state.activeAuthor) return;
  state.works = await invoke("list_works", {
    authorId: state.activeAuthor.id,
    query: state.workQuery,
    searchField: state.searchField,
    status: state.status,
    favoritesOnly: state.authorFavoritesOnly,
    imagesOnly: state.authorImagesOnly,
    sort: state.sort,
  });
}

async function refreshAllWorks() {
  state.allWorks = await invoke("list_all_works", {
    query: state.allWorksQuery,
    searchField: state.searchField,
    status: state.status,
    favoritesOnly: state.allWorksFavoritesOnly,
    imagesOnly: state.allWorksImagesOnly,
    sort: state.sort,
  });
}

/**
 * 收藏 / 带图版这两个标记既影响作品卡，也影响作者卡上的统计（收藏数、带图版数），
 * 而「所有作品」与「作者作品库」用的是两份数据 —— 改完统一在这里补齐再重绘。
 */
async function refreshAfterWorkFlagChange() {
  await refreshActiveAuthor();
  if (state.activeAuthor) await refreshWorks();
  else if (state.homeView === "allWorks") await refreshAllWorks();
  else if (state.homeView === "collections") {
    await refreshCollections();
    if (state.activeCollection) await refreshCollectionWorks();
  }
  render();
}

async function refreshCollections() {
  state.collections = await invoke("list_collections");
}

async function refreshCollectionWorks() {
  if (!state.activeCollection) return;
  state.collectionWorks = await invoke("list_collection_works", {
    collectionId: state.activeCollection.id,
    query: state.collectionQuery,
    searchField: "title",
    status: state.status,
    imagesOnly: state.collectionImagesOnly,
    sort: state.collectionSort,
  });
}

async function refreshHistory() {
  state.history = await invoke("list_history", { query: state.historyQuery, limit: 0 });
}

function render() {
  app.innerHTML = state.activeAuthor
    ? (state.seriesView ? renderSeriesView() : renderWorks())
    : (state.homeView === "allWorks" ? renderAllWorks()
      : state.homeView === "collections" ? renderCollections()
      : state.homeView === "history" ? renderHistory()
      : state.homeView === "help" ? renderHelp() : renderAuthors());
  document.body.classList.toggle("is-bulk", Boolean(state.bulkMode));
  mountHelpDocument();
  bindEvents();
}

function restoreSearchFocus(id) {
  window.requestAnimationFrame(() => {
    const input = app.querySelector(`#${id}`);
    if (!input) return;
    input.focus();
    const position = input.value.length;
    input.setSelectionRange(position, position);
  });
}

function findWork(workId) {
  const target = Number(workId);
  return [...state.works, ...state.seriesItems, ...state.allWorks, ...state.collectionWorks, ...state.history.map((entry) => entry.work)].find((work) => work.id === target);
}

async function openSeriesDetail(seriesId, seriesTitle, returnTo = "works") {
  state.seriesView = { kind: "detail", id: seriesId, title: seriesTitle, returnTo };
  state.seriesItems = await invoke("list_series_works", { authorId: state.activeAuthor.id, seriesId });
  render();
}

async function openAllWorksSeries(authorId, seriesId, seriesTitle) {
  const author = state.authors.find((item) => item.id === Number(authorId));
  if (!author) throw new Error("未找到该系列所属作者");
  state.activeAuthor = author;
  state.homeView = "allWorks";
  await openSeriesDetail(seriesId, seriesTitle, "allWorks");
}

/**
 * 从「所有作品」点卡片上的作者名 → 落到该作者的作品库（v0.3.67 用户要求）。
 * 顺手把搜索词和两个「仅看」筛选复位：在「所有作品」里搜的词带进作者库只会让人一脸问号。
 */
async function openAuthorLibrary(authorId) {
  const author = state.authors.find((item) => item.id === Number(authorId));
  if (!author) { toast("没找到这位作者，先回作者库刷新一下", "error"); return; }
  // 记住来路：从「所有作品」进来的，返回按钮得回「所有作品」（v0.3.68）
  state.authorReturnTo = state.homeView === "allWorks" ? "allWorks" : null;
  state.activeAuthor = author;
  state.homeView = "authors";
  state.seriesView = null;
  state.seriesItems = [];
  state.workQuery = "";
  state.authorFavoritesOnly = false;
  state.authorImagesOnly = false;
  await refreshWorks();
  render();
}

async function openSeriesLibrary() {
  state.seriesView = { kind: "overview" };
  state.seriesItems = await invoke("list_series", { authorId: state.activeAuthor.id });
  render();
}

async function closeSeriesView() {
  if (state.seriesView?.kind === "detail") {
    const returnTo = state.seriesView.returnTo;
    if (returnTo === "overview") {
      await openSeriesLibrary();
      return;
    }
    state.seriesView = null;
    state.seriesItems = [];
    if (returnTo === "allWorks") {
      state.activeAuthor = null;
      state.homeView = "allWorks";
      await refreshAllWorks();
    } else {
      await refreshWorks();
    }
    render();
    return;
  }
  state.seriesView = null;
  state.seriesItems = [];
  render();
}

function renderShell(content) {
  const authorLibraryActive = state.homeView === "authors" || Boolean(state.activeAuthor);
  return `<div class="app-shell">
    <aside class="side-rail">
      <div class="brand-mark" title="Pixiv小说下载管理器" aria-label="Pixiv小说下载管理器">${appLogo(44)}</div>
      <nav class="rail-nav">
        <button class="rail-button ${authorLibraryActive ? "is-active" : ""}" title="作者库" data-action="go-home">${icon("image", 20)}</button>
      </nav>
      <nav class="rail-nav rail-nav-secondary">
        <button class="rail-button ${state.homeView === "allWorks" && !state.activeAuthor ? "is-active" : ""}" title="\u6240\u6709\u4f5c\u54c1" data-action="go-all-works">${icon("database", 20)}</button>
        <button class="rail-button ${state.homeView === "collections" && !state.activeAuthor ? "is-active" : ""}" title="我的收藏" aria-label="我的收藏" data-action="go-collections">${icon("folderHeart", 20)}</button>
        <button class="rail-button ${state.homeView === "history" && !state.activeAuthor ? "is-active" : ""}" title="浏览历史" aria-label="浏览历史" data-action="go-history">${icon("clock", 20)}</button>
        <button class="rail-button ${state.homeView === "help" && !state.activeAuthor ? "is-active" : ""}" title="帮助" aria-label="帮助" data-action="help">${icon("help", 20)}</button>
      </nav>
      <div class="rail-footer">
        ${appVersionBadge()}
        <button class="rail-button rail-settings-button" title="设置" aria-label="设置" data-action="settings">${icon("gear", 25)}<span>设置</span></button>
      </div>
    </aside>
    <main class="workspace">${content}</main>
  </div>${syncFloater()}${scrollJump()}`;
}

// 右下角的「回到顶部 / 滚到底部」：作者或作品很多时不用一路滚
function scrollJump() {
  return `<div class="scroll-jump is-hidden" id="scroll-jump">
    <button class="icon-button" title="回到顶部" aria-label="回到顶部" data-action="scroll-top">${icon("up", 18)}</button>
    <button class="icon-button" title="滚到底部" aria-label="滚到底部" data-action="scroll-bottom">${icon("down", 18)}</button>
  </div>`;
}

function libraryScroller() {
  return document.scrollingElement || document.documentElement;
}

// 只有列表长到需要滚动时才显示按钮；已经在顶部/底部时把对应箭头置灰
function updateScrollJump() {
  const box = document.querySelector("#scroll-jump");
  if (!box) return;
  const scroller = libraryScroller();
  const maxScroll = scroller.scrollHeight - scroller.clientHeight;
  const scrollable = maxScroll > 60;
  box.classList.toggle("is-hidden", !scrollable);
  if (!scrollable) return;
  const top = scroller.scrollTop;
  const topButton = box.querySelector('[data-action="scroll-top"]');
  const bottomButton = box.querySelector('[data-action="scroll-bottom"]');
  if (topButton) topButton.disabled = top <= 4;
  if (bottomButton) bottomButton.disabled = top >= maxScroll - 4;
  // 同步进度浮层也在右下角，按钮组要抬到它上面，别互相压住
  const floater = document.querySelector("#sync-floater");
  box.style.setProperty("--floater-offset", `${floater ? Math.round(floater.getBoundingClientRect().height) + 12 : 0}px`);
}

function scrollLibraryTo(where) {
  const scroller = libraryScroller();
  window.scrollTo({ top: where === "bottom" ? scroller.scrollHeight : 0, behavior: "smooth" });
  // 平滑滚动走完再刷新一次按钮状态
  window.setTimeout(updateScrollJump, 450);
}

// 进度浮层右下角那个按钮：同步任务＝「终止同步」；批量下载阅读版＝「隐藏进度」
// （后端没有中断接口，隐藏只是把浮层收起来，任务继续跑，结束时照常弹结果提示）
function floaterFootButton(task) {
  const action = task.cancelAction || "cancel-pixiv-sync";
  const idle = task.cancelText || "终止同步";
  const busy = task.cancelAction ? idle : "正在终止…";
  return `<button class="quiet-button" data-action="${action}" data-floater-foot ${task.cancelling ? "disabled" : ""}>${task.cancelling ? busy : idle}</button>`;
}

// 后台同步进度浮层：同步不阻塞主界面，用户可继续浏览，右下角显示进度并可随时终止
// 单作者同步＝一根进度条；「同步所有作者」＝每位作者一行、各自一根进度条
function syncFloater() {
  const task = state.syncTask;
  if (!task) return "";
  if (task.authors) return batchSyncFloater(task);
  return `<div class="sync-floater" id="sync-floater">
    <div class="sync-floater-head">
      <strong>${icon("sync", 16)}<span id="sync-progress-label">${escapeHtml(task.label)}</span></strong>
      <span class="sync-floater-count" id="sync-progress-count">${task.current} / ${task.total}</span>
    </div>
    <progress id="sync-progress-bar" value="${task.current}" max="${Math.max(task.total, 1)}"></progress>
    <div class="sync-floater-foot">
      <p id="sync-progress-title" title="${escapeHtml(task.title || "")}">${escapeHtml(task.title || "正在读取作品列表...")}</p>
      ${floaterFootButton(task)}
    </div>
  </div>`;
}

// 批量同步时每一行的状态说明文字
function syncRowStatusText(item) {
  if (item.status === "pending") return "等待中";
  if (item.status === "running") return item.title || "正在读取作品列表...";
  return item.note || (item.status === "failed" ? "同步失败" : "已完成");
}

function batchSyncFloater(task) {
  const rows = task.authors.map((item) => `<div class="sync-floater-row is-${item.status}" data-sync-author-id="${item.authorId}">
      <div class="sync-floater-row-head">
        <span class="sync-row-name" title="${escapeHtml(item.name)}">${escapeHtml(item.name)}</span>
        <span class="sync-row-count" data-sync-count>${item.current} / ${item.total}</span>
      </div>
      <progress data-sync-bar value="${item.current}" max="${Math.max(item.total, 1)}"></progress>
      <p class="sync-row-title" data-sync-title title="${escapeHtml(syncRowStatusText(item))}">${escapeHtml(syncRowStatusText(item))}</p>
    </div>`).join("");
  const finished = task.authors.filter((item) => item.status !== "pending" && item.status !== "running").length;
  return `<div class="sync-floater is-batch" id="sync-floater">
    <div class="sync-floater-head">
      <strong>${icon("sync", 16)}<span id="sync-progress-label">${escapeHtml(task.label)}</span></strong>
      <span class="sync-floater-count" id="sync-progress-count">${finished} / ${task.authors.length}</span>
    </div>
    <div class="sync-floater-rows">${rows}</div>
    <div class="sync-floater-foot">
      <p id="sync-progress-title">正在逐位同步作者，可随时终止</p>
      <button class="quiet-button" data-action="cancel-pixiv-sync" data-floater-foot ${task.cancelling ? "disabled" : ""}>${task.cancelling ? "正在终止…" : "终止同步"}</button>
    </div>
  </div>`;
}

function updateSyncFloater() {
  const task = state.syncTask;
  const box = document.querySelector("#sync-floater");
  if (!task || !box) return;
  // 底部按钮：同步＝终止同步；批量下载阅读版＝隐藏进度
  const foot = box.querySelector("[data-floater-foot]");
  if (foot) {
    foot.disabled = Boolean(task.cancelling);
    foot.textContent = task.cancelling && !task.cancelAction ? "正在终止…" : task.cancelText || "终止同步";
  }
  if (task.authors) {
    const finished = task.authors.filter((item) => item.status !== "pending" && item.status !== "running").length;
    box.querySelector("#sync-progress-count").textContent = `${finished} / ${task.authors.length}`;
    box.querySelector("#sync-progress-label").textContent = task.label;
    task.authors.forEach((item) => {
      const line = box.querySelector(`[data-sync-author-id="${item.authorId}"]`);
      if (!line) return;
      line.className = `sync-floater-row is-${item.status}`;
      const bar = line.querySelector("[data-sync-bar]");
      bar.max = Math.max(item.total, 1);
      bar.value = item.current;
      line.querySelector("[data-sync-count]").textContent = `${item.current} / ${item.total}`;
      const status = line.querySelector("[data-sync-title]");
      status.textContent = syncRowStatusText(item);
      status.title = status.textContent;
    });
    return;
  }
  const bar = box.querySelector("#sync-progress-bar");
  bar.max = Math.max(task.total, 1);
  bar.value = task.current;
  box.querySelector("#sync-progress-count").textContent = `${task.current} / ${task.total}`;
  const title = box.querySelector("#sync-progress-title");
  title.textContent = task.title || "正在读取作品列表...";
  title.title = task.title || "";
  box.querySelector("#sync-progress-label").textContent = task.label;
}

/**
 * 帮助页 = docs/index.html 那份图文指南本体（用户要求：内容只维护一份，不再另做外置文档）。
 * 内容由 scripts/build-help-doc.mjs 从 docs/index.html 生成到 src/help-doc.js，
 * 配图生成到 public/help/images/，程序里按 help/images/xxx.png 取。
 *
 * 为什么套 Shadow DOM：这份指南自带一整套页面级 CSS（:root / body / h1 / img …），
 * 直接塞进应用里会和软件自己的样式互相打架（标题字号、背景、图片尺寸都会被顶掉），
 * 这也正是当初把帮助页挪到外面的原因。Shadow DOM 两头都隔开：指南的样式出不去，
 * 应用的样式也进不来，图片按自然比例显示。
 */
function renderHelp() {
  return renderShell(`
    <section class="help-content">
      <div class="help-doc" id="help-doc"></div>
    </section>`);
}

function mountHelpDocument() {
  const host = app.querySelector("#help-doc");
  if (!host || host.dataset.helpMounted === "true") return;
  host.dataset.helpMounted = "true";
  const shadow = host.attachShadow({ mode: "open" });
  shadow.innerHTML = `<style>${HELP_DOC_CSS}</style>${HELP_DOC_HTML.replace(/__APP_VERSION__/g, APP_VERSION)}`;
  shadow.addEventListener("click", async (event) => {
    const link = event.target.closest ? event.target.closest("a") : null;
    if (!link) return;
    const href = link.getAttribute("href") || "";
    // 目录和正文里「见第 3 节」这类锚点：Shadow DOM 里的元素浏览器找不到，自己滚
    if (href.startsWith("#")) {
      event.preventDefault();
      const target = shadow.getElementById(href.slice(1));
      if (target) target.scrollIntoView({ behavior: "smooth", block: "start" });
      return;
    }
    if (/^https?:/i.test(href)) {
      event.preventDefault();
      try {
        await invoke("open_external_url", { url: href });
      } catch (error) {
        toast(`无法打开链接：${error}`, "error");
      }
    }
  });
}

async function openExternalUrl(url) {
  if (!/^https?:\/\//i.test(url || "")) throw new Error("仅支持在浏览器中打开 HTTP 或 HTTPS 链接");
  await invoke("open_external_url", { url });
}

function authorAliasList(aliases) {
  const items = String(aliases || "").split("|").map((alias) => alias.trim()).filter(Boolean);
  if (!items.length) return "";
  return `<div class="author-aliases">${items.map((alias) => `<span>${escapeHtml(alias)}</span>`).join("")}</div>`;
}

// 作者搜索：名称和别名任一命中就算匹配
function authorMatchesQuery(author, query) {
  if (!query) return true;
  if (author.name.toLowerCase().includes(query)) return true;
  return String(author.aliases || "").split("|").some((alias) => alias.trim().toLowerCase().includes(query));
}

/**
 * 作者卡拖动排序（v0.3.69）：按住卡片上的拖动按钮约 0.2 秒进入拖动态，再挪到目标位置松手。
 * 顺序存到 `authors.sort_order`（1..n），全都没手动排过时是 0 → 仍按名字排。
 */
const AUTHOR_DRAG_HOLD_MS = 200;
let authorDragSession = null;

/** 拖动态下把卡片按指针位置挪到对应槽位：指针在哪张卡之前就插到它前面 */
function authorDragTarget(grid, dragged, x, y) {
  return [...grid.querySelectorAll(".author-card")].find((card) => {
    if (card === dragged) return false;
    const rect = card.getBoundingClientRect();
    if (y < rect.top) return true;        // 指针在这张卡上方 → 插它前面
    if (y > rect.bottom) return false;    // 指针在下方 → 继续往后找
    return x < rect.left + rect.width / 2; // 同一行：比左右
  }) || null;
}

/**
 * 长按到点：把卡片「拎起来」。
 * 克隆一张同尺寸的浮层挂在 body 上跟着指针走（fixed 定位，不受滚动影响），
 * 原卡片留在网格里淡化成占位槽 —— 这样既不破坏布局，松手时也不用重建 DOM。
 */
function liftAuthorCard() {
  const session = authorDragSession;
  if (!session || session.armed) return;
  const { card, grid } = session;
  if (!card.isConnected) { authorDragSession = null; return; }
  // 先把原卡片变成占位槽，再按槽位坐标立浮层 —— 卡片 hover 时会上浮 2px，
  // 若按旧坐标摆浮层就会跟槽位错开一小截。读坐标前临时掐掉过渡，
  // 不然量到的是"还在往槽位滑"的中间位置。
  card.style.transition = "none";
  card.classList.add("is-dragging");
  grid.classList.add("is-sorting");
  document.body.classList.add("is-author-dragging");
  const rect = card.getBoundingClientRect();
  card.style.transition = "";
  const ghost = document.createElement("div");
  ghost.className = "author-drag-ghost";
  ghost.style.left = `${rect.left}px`;
  ghost.style.top = `${rect.top}px`;
  ghost.style.width = `${rect.width}px`;
  ghost.style.height = `${rect.height}px`;
  const inner = card.cloneNode(true);
  inner.classList.add("author-drag-inner");
  // 克隆体只是个跟着指针走的影子：去掉占位槽样式，但保留 data-bound，
  // 免得它被当成真卡片重复绑事件
  inner.classList.remove("is-dragging");
  ghost.appendChild(inner);
  document.body.appendChild(ghost);
  // 抓取点相对卡片左上角的偏移：拖起来时指针在哪，卡片就跟着保持在哪
  session.armed = true;
  session.ghost = ghost;
  session.grabX = session.startX - rect.left;
  session.grabY = session.startY - rect.top;
  // 让"拎起来"那一下有个放大的过渡（下一帧再加类，否则 transition 不会跑）
  requestAnimationFrame(() => ghost.classList.add("is-lifting"));
  moveAuthorGhost(session.startX, session.startY);
}

/** 浮层跟手：位移走 transform，避免每帧改 left/top 触发重排 */
function moveAuthorGhost(x, y) {
  const session = authorDragSession;
  if (!session?.ghost) return;
  const offsetX = x - session.grabX - parseFloat(session.ghost.style.left || "0");
  const offsetY = y - session.grabY - parseFloat(session.ghost.style.top || "0");
  session.ghost.style.transform = `translate3d(${offsetX}px, ${offsetY}px, 0)`;
}

/**
 * 换位 + FLIP 让位动画：先把每张卡当前的位置记下来，插完 DOM 再按差值把它们
 * 「瞬移回原位」然后过渡到新位置 —— 于是别的卡片是滑过去的，不是啪一下跳过去。
 */
function reorderAuthorCards(grid, card, target) {
  const cards = [...grid.querySelectorAll(".author-card")];
  const before = new Map(cards.map((item) => [item, item.getBoundingClientRect()]));
  if (target) grid.insertBefore(card, target); else grid.appendChild(card);
  cards.forEach((item) => {
    if (item === card) return;
    const previous = before.get(item);
    const current = item.getBoundingClientRect();
    const offsetX = previous.left - current.left;
    const offsetY = previous.top - current.top;
    if (!offsetX && !offsetY) return;
    item.style.transition = "none";
    item.style.transform = `translate(${offsetX}px, ${offsetY}px)`;
    requestAnimationFrame(() => {
      item.style.transition = "transform .2s cubic-bezier(.2,.7,.3,1)";
      item.style.transform = "";
    });
    window.setTimeout(() => {
      if (item.isConnected) { item.style.transition = ""; item.style.transform = ""; }
    }, 260);
  });
}

/** 松手：浮层原地缩小淡出（像"放下去了"），提交失败则把顺序还原 */
function dropAuthorGhost(session, commit) {
  const ghost = session.ghost;
  session.ghost = null;
  if (!ghost) return;
  if (!commit) { ghost.remove(); return; }
  ghost.classList.remove("is-lifting");
  ghost.classList.add("is-dropping");
  window.setTimeout(() => ghost.remove(), 200);
}

async function finishAuthorDrag(commit) {
  const session = authorDragSession;
  authorDragSession = null;
  if (!session) return;
  window.clearTimeout(session.timer);
  dropAuthorGhost(session, commit);
  session.card.classList.remove("is-dragging");
  session.grid.classList.remove("is-sorting");
  document.body.classList.remove("is-author-dragging");
  const grid = session.card.parentElement;
  if (!grid) return;
  if (!commit) {
    // 中途被打断（系统抢走指针）→ 顺序还原回去，别留个半拉子顺序
    const byId = new Map([...grid.querySelectorAll(".author-card")].map((card) => [Number(card.dataset.authorId), card]));
    session.orderBefore.forEach((id) => { const card = byId.get(id); if (card) grid.appendChild(card); });
    return;
  }
  const ids = [...grid.querySelectorAll(".author-card")].map((card) => Number(card.dataset.authorId));
  // 原地长长按一下、位置没动就别白写一次库
  if (ids.join(",") === session.orderBefore.join(",")) return;
  await commitAuthorOrder(ids);
}

/** 把当前可见（可能被搜索/筛选过）的新顺序合并进完整顺序，再整体存库 */
async function commitAuthorOrder(visibleIds) {
  const all = state.authors.map((author) => author.id);
  const visible = new Set(visibleIds);
  const slots = all.reduce((list, id, index) => (visible.has(id) ? [...list, index] : list), []);
  const next = all.slice();
  slots.forEach((slot, index) => { next[slot] = visibleIds[index]; });
  try {
    await invoke("set_author_order", { authorIds: next });
    const byId = new Map(state.authors.map((author) => [author.id, author]));
    state.authors = next.map((id) => byId.get(id)).filter(Boolean);
    render();
  } catch (error) {
    toast(`保存作者顺序失败：${error}`, "error");
    await refreshAuthors();
    render();
  }
}

function renderAuthors() {
  const query = state.authorQuery.trim().toLowerCase();
  const authors = state.authors.filter((author) => (!state.authorsStarredOnly || author.starred) && authorMatchesQuery(author, query));
  const cards = authors.map((author) => `
    <article class="author-card${author.starred ? " is-starred" : ""}" data-author-id="${author.id}" tabindex="0">
      <div class="author-avatar ${author.avatarPath ? "has-image" : ""}">
        ${author.avatarPath ? `<img src="${asset(author.avatarPath)}" alt="${escapeHtml(author.name)} 的头像">` : `<span>${escapeHtml(initials(author.name))}</span>`}
      </div>
      <div class="author-card-body">
        <div class="author-card-title-row"><h2>${escapeHtml(author.name)}</h2><div class="author-card-actions"><button class="icon-button card-drag" title="长按拖动，调整作者顺序" aria-label="长按拖动调整顺序">${icon("grip", 17)}</button><button class="icon-button card-star${author.starred ? " is-on" : ""}" title="${author.starred ? "取消特别关注" : "设为特别关注"}" data-action="toggle-author-starred" data-author-id="${author.id}">${icon(author.starred ? "starFilled" : "star", 17)}</button><button class="icon-button card-edit" title="编辑作者" data-action="edit-author" data-author-id="${author.id}">${icon("more", 18)}</button></div></div>
        ${authorAliasList(author.aliases)}
        <dl class="author-stats"><div><dt>作品</dt><dd>${author.workCount}</dd></div><div><dt>完整版</dt><dd>${author.purchasedCount}</dd></div><div><dt>带图版</dt><dd>${author.imagesCount || 0}</dd></div><div><dt>收藏</dt><dd>${author.favoriteCount}</dd></div></dl>
      </div>
      <div class="card-enter">${icon("arrow", 18)}</div>
    </article>`).join("");

  return renderShell(`
    <section class="topbar">
      <div><p class="section-kicker">私人作品档案</p><h1>作者库</h1></div>
      <div class="topbar-actions">
        <button class="icon-text-button" data-action="settings">${icon("database", 18)}<span>备份与恢复</span></button>
        <button class="icon-text-button" data-action="auto-group">${icon("upload", 18)}<span>完整版自动分组</span></button>
        <button class="icon-text-button" data-action="auto-group-by-author">${icon("folder", 18)}<span>指定作者自动分组</span></button>
        <button class="icon-text-button" data-action="sync-all-authors">${icon("sync", 18)}<span>同步所有作者</span></button>
        <button class="primary-button" data-action="new-author">${icon("plus", 18)}<span>新增作者</span></button>
      </div>
    </section>
    <section class="authors-content">
      <div class="authors-toolbar">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="author-search" type="search" placeholder="搜索作者" value="${escapeHtml(state.authorQuery)}" autocomplete="off"></label>
        <button class="icon-text-button star-filter${state.authorsStarredOnly ? " is-active" : ""}" data-action="authors-starred-only" title="只显示已设为特别关注的作者">${icon(state.authorsStarredOnly ? "starFilled" : "star", 17)}<span>只看特别关注</span></button>
      </div>
      <div class="section-row"><p>${authors.length ? `共 ${authors.length} 位作者${state.authorsStarredOnly ? "（已筛选特别关注）" : ""}` : ""}</p></div>
      <div class="author-grid">${cards || (state.authors.length ? renderNoMatchedAuthors() : renderEmptyAuthors())}</div>
    </section>`);
}

function renderEmptyAuthors() {
  return `<div class="empty-state"><div class="empty-icon">${icon("image", 26)}</div><h2>还没有作者</h2><p>建立第一位作者后，即可导入作品并绑定本地文件。</p><button class="primary-button" data-action="new-author">${icon("plus", 18)}<span>新增作者</span></button></div>`;
}

// 有作者但被搜索词或「只看特别关注」筛空了
function renderNoMatchedAuthors() {
  const onlyStarred = state.authorsStarredOnly;
  return `<div class="empty-state"><div class="empty-icon">${icon(onlyStarred ? "star" : "search", 26)}</div><h2>${onlyStarred ? "还没有特别关注的作者" : "没有匹配的作者"}</h2><p>${onlyStarred ? "点作者卡右上角的星标，即可把作者设为特别关注。" : "换个关键词试试，或清空搜索框。"}</p>${onlyStarred ? `<button class="primary-button" data-action="authors-starred-only">${icon("x", 18)}<span>显示全部作者</span></button>` : ""}</div>`;
}

// 作品卡上只有「封面图」和「标题」带 work-open 类，只有点这两处才会打开文件
function workCover(work) {
  if (work.coverPath) return `<img class="work-open" src="${asset(work.coverPath)}" alt="${escapeHtml(work.title)} 的封面" loading="lazy" onerror="this.closest('.work-cover')?.classList.add('is-missing');this.remove()">`;
  return `<div class="cover-placeholder work-open"><span>${icon("image", 28)}</span><small>暂无封面</small></div>`;
}

function workSeries(work) {
  if (!work.seriesId || !work.seriesTitle) return "";
  return `<button class="work-series" title="查看系列：${escapeHtml(work.seriesTitle)}" data-action="open-series" data-series-id="${escapeHtml(work.seriesId)}" data-series-title="${escapeHtml(work.seriesTitle)}">${icon("series", 13)}<span>${escapeHtml(work.seriesTitle)}</span></button>`;
}

function workSeriesLabel(work) {
  if (!work.seriesId || !work.seriesTitle) return "";
  return `<div class="work-series"><span>${icon("series", 13)}${escapeHtml(work.seriesTitle)}</span></div>`;
}

function allWorkSeries(work) {
  if (!work.seriesId || !work.seriesTitle) return "";
  return `<button class="work-series" title="查看系列：${escapeHtml(work.seriesTitle)}" data-action="open-all-series" data-author-id="${work.authorId}" data-series-id="${escapeHtml(work.seriesId)}" data-series-title="${escapeHtml(work.seriesTitle)}">${icon("series", 13)}<span>${escapeHtml(work.seriesTitle)}</span></button>`;
}

// 连载作品判定：连载合集的标题必然包含「数字-数字」区间结构（如「斗破苍穹7-10」），
// 区间后面不管跟什么说明文字（（完结）、精校版、连载中…）都一律忽略，前缀相同即视为同一部连载
function serialPrefixOf(raw) {
  return String(raw || "").replace(/[\s\-_·：:（(【\[《「『]+$/g, "").trim().toLowerCase();
}

function serialGroupInfo(title) {
  const text = String(title || "").trim();
  if (!text) return null;
  // 形如「2025-10-05 某作品」的日期开头标题不参与分组，避免按年月误合并
  if (/^\d{4}\s*[-/.年]\s*\d{1,2}\s*[-/.月]\s*\d{1,2}/.test(text)) return null;

  // 情形一：含「数字-数字」区间，取区间上界，后面跟什么都无所谓
  const range = text.match(/^(.*?)[\s\-_·：:]*(\d+)\s*[-~—－〜到至]\s*(\d+)/);
  if (range) {
    const prefix = serialPrefixOf(range[1]);
    const number = Number(range[3]);
    if (prefix && Number.isFinite(number)) return { prefix, number };
  }

  // 情形二：没有区间时，退回到「标题以数字结尾」，此时要求尾部只剩标点，避免误判
  const single = text.match(/^(.*?)[\s\-_·：:]*(\d+)[\s)）\]】}》」』。.·…!！?？、,，]*$/);
  if (!single) return null;
  const prefix = serialPrefixOf(single[1]);
  const number = Number(single[2]);
  if (!prefix || !Number.isFinite(number)) return null;
  return { prefix, number };
}

// 开启「连载只看最新」后，同一前缀的作品只保留章节号最大的一个
function collapseSerialWorks(works) {
  if (!state.serialLatestOnly || !Array.isArray(works)) return works;
  const entries = works.map((work) => ({ work, info: serialGroupInfo(work.title) }));
  const latest = new Map();
  entries.forEach((entry) => {
    if (!entry.info) return;
    const current = latest.get(entry.info.prefix);
    if (!current || entry.info.number > current.info.number) latest.set(entry.info.prefix, entry);
  });
  return entries.filter((entry) => !entry.info || latest.get(entry.info.prefix) === entry).map((entry) => entry.work);
}

function serialHiddenCount(works) {
  if (!state.serialLatestOnly || !Array.isArray(works)) return 0;
  return works.length - collapseSerialWorks(works).length;
}

function serialFilterButton(works) {
  const hidden = serialHiddenCount(works);
  const suffix = state.serialLatestOnly && hidden ? `（隐藏 ${hidden}）` : "";
  return `<button class="icon-text-button favorite-filter serial-filter ${state.serialLatestOnly ? "is-active" : ""}" data-action="serial-latest" title="连载作品只显示最新章节">${icon("layers", 17)}<span>连载只看最新${suffix}</span></button>`;
}

/** 作品在 Pixiv 上的页面地址；没作品 ID 时给空串（本地导入的作品就没有） */
function pixivNovelUrl(novelId) {
  const id = String(novelId || "").trim();
  return id ? `https://www.pixiv.net/novel/show.php?id=${id}` : "";
}

/** 封面第二排的「打开作品网址」徽标（第一排留给状态 / 收藏 / 带图版，别把一排挤满）：有 Pixiv 作品 ID 才给 */
function workLinkBadge(work) {
  if (!work.pixivNovelId) return "";
  return `<button class="favorite-badge link-badge" title="在浏览器里打开 Pixiv 作品页" data-action="open-work-url" data-work-id="${work.id}">${icon("link", 17)}</button>`;
}

/**
 * 封面第一排的徽标（状态 / 收藏 / 带图版）——作者作品库、系列作品、所有作品三处共用，
 * 免得以后改一个地方漏掉另外两个；链接徽标属于第二排 `.work-links`，不在这里。
 */
function workBadges(work) {
  return `<div class="work-badges">
          <span class="status-badge ${work.purchasedPath ? "owned" : "unowned"}" title="${work.purchasedPath ? "完整版已绑定" : "预览版"}">${icon(work.purchasedPath ? "check" : "x", 17)}</span>
          <button class="favorite-badge ${work.favorite ? "is-favorite" : ""}" title="${work.favorite ? "已在收藏夹里，点击调整" : "加入收藏夹"}" data-action="pick-collection" data-work-id="${work.id}">${icon("heart", 17)}</button>
          <button class="favorite-badge images-badge ${work.hasImages ? "is-images" : ""}" title="${work.hasImages ? "已设为带图版，点击取消" : "未设为带图版，点击设置"}${work.imageCount ? ` · 共 ${work.imageCount} 张配图` : ""}" data-action="toggle-has-images" data-work-id="${work.id}">${icon("image", 17)}${work.imageCount ? `<span class="badge-count">${work.imageCount}</span>` : ""}</button>
        </div>`;
}

function renderWorks() {
  const author = state.activeAuthor;
  const works = collapseSerialWorks(state.works);
  const cards = works.map((work) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"} ${state.bulkMode ? "is-selecting" : ""}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${state.bulkMode ? `<button class="selection-badge ${state.selectedWorkIds.has(work.id) ? "is-selected" : ""}" title="${state.selectedWorkIds.has(work.id) ? "取消选择" : "选择作品"}" data-action="toggle-select" data-work-id="${work.id}">${state.selectedWorkIds.has(work.id) ? icon("check", 16) : ""}</button>` : `<button class="work-menu" title="更多操作" data-action="work-menu" data-work-id="${work.id}">${icon("more", 18)}</button>`}
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${workSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`).join("");

  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="${state.authorReturnTo === "allWorks" ? "返回所有作品" : "返回作者库"}" data-action="${state.authorReturnTo === "allWorks" ? "back-to-all-works" : "go-home"}">${icon("back", 20)}</button><div><p class="section-kicker">作者作品库${author.homepage ? `<button class="homepage-link" title="打开作者主页：${escapeHtml(author.homepage)}" data-action="open-external-url" data-url="${escapeHtml(author.homepage)}">${icon("link", 13)}<span>作者主页</span></button>` : ""}</p><h1>${escapeHtml(author.name)}</h1></div></div>
      <div class="topbar-actions">${state.bulkMode ? `<button class="quiet-button" data-action="bulk-mode">取消</button><button class="quiet-button" data-action="select-all" ${works.length ? "" : "disabled"}>${works.length && works.every((work) => state.selectedWorkIds.has(work.id)) ? "取消全选" : "全选"}</button><button class="quiet-button" data-count-target data-count-label="设为完整版" data-action="copy-selected-full" ${state.selectedWorkIds.size ? "" : "disabled"}>设为完整版（${state.selectedWorkIds.size}）</button><button class="quiet-button" data-count-target data-count-label="设为带图版" data-action="set-images-selected" ${state.selectedWorkIds.size ? "" : "disabled"}>设为带图版（${state.selectedWorkIds.size}）</button><button class="quiet-button" data-count-target data-count-label="补下配图" data-action="backfill-images" title="按设置里选的格式，给勾选的作品重新下载阅读版并绑定" ${state.selectedWorkIds.size ? "" : "disabled"}>补下配图（${state.selectedWorkIds.size}）</button><button class="quiet-button" data-count-target data-count-label="重新下载 EPUB 版并绑定" data-action="download-selected-reading" data-format="epub" ${state.selectedWorkIds.size ? "" : "disabled"}>重新下载 EPUB 版并绑定（${state.selectedWorkIds.size}）</button><button class="danger-button" data-count-target data-count-label="删除已选" data-action="delete-selected" ${state.selectedWorkIds.size ? "" : "disabled"}>删除已选（${state.selectedWorkIds.size}）</button>` : `<button class="icon-text-button" data-action="bulk-mode">${icon("more", 18)}<span>批量操作</span></button><button class="icon-text-button" data-action="edit-author" data-author-id="${author.id}">${icon("settings", 18)}<span>作者设置</span></button><button class="icon-text-button" data-action="import-works">${icon("plus", 18)}<span>导入作品</span></button><button class="primary-button" data-action="sync-pixiv">${icon("sync", 18)}<span>作品同步</span></button>`}</div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${state.searchField === "tags" ? "搜索标签" : "搜索作品名称"}" value="${escapeHtml(state.workQuery)}" autocomplete="off"></label>
        <select class="sort-select search-mode-select" id="search-field" aria-label="搜索范围"><option value="title" ${state.searchField === "title" ? "selected" : ""}>标题</option><option value="tags" ${state.searchField === "tags" ? "selected" : ""}>标签</option></select>
        <div class="filter-group" role="group" aria-label="版本状态">${[ ["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"] ].map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.authorFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <button class="icon-text-button favorite-filter images-filter ${state.authorImagesOnly ? "is-active" : ""}" data-action="images-only">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.works)}
        <select class="sort-select" id="sort-select" aria-label="排序"><option value="date_desc" ${state.sort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.sort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.sort === "title_asc" ? "selected" : ""}>名称 A-Z</option><option value="words_desc" ${state.sort === "words_desc" ? "selected" : ""}>字数从多到少</option></select>
      </div>
      <div class="binding-bar"><div><strong>本地文件</strong><span>${author.previewDir ? "预览版目录已绑定" : "尚未绑定预览版目录"} · ${author.purchasedDir ? "完整版目录已绑定" : "尚未绑定完整版目录"}</span><em class="sync-status">Pixiv ${syncLabel(author.pixivLastSyncAt)}</em></div><div><button class="quiet-button" data-action="scan-preview">${icon("folder", 17)}关联预览版文件</button><button class="quiet-button" data-action="scan-purchased">${icon("upload", 17)}关联完整版文件</button></div></div>
      <div class="works-grid">${cards || renderEmptyWorks()}</div>
    </section>`);
}

function renderAllWorks() {
  const works = collapseSerialWorks(state.allWorks);
  const cards = works.map((work) => `
    <article class="work-card is-read-only ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
      </div>
      <div class="work-copy">${work.authorName ? `<button class="work-author is-link" title="打开「${escapeHtml(work.authorName)}」的作品库" data-action="open-author" data-author-id="${work.authorId}">${escapeHtml(work.authorName)}</button>` : `<p class="work-author"></p>`}<div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${allWorkSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`).join("");
  return renderShell(`
    <section class="topbar work-topbar"><div><p class="section-kicker">全部作者</p><h1>所有作品</h1></div></section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${state.searchField === "tags" ? "搜索标签" : "搜索作品名称"}" value="${escapeHtml(state.allWorksQuery)}" autocomplete="off"></label>
        <select class="sort-select search-mode-select" id="search-field" aria-label="搜索范围"><option value="title" ${state.searchField === "title" ? "selected" : ""}>标题</option><option value="tags" ${state.searchField === "tags" ? "selected" : ""}>标签</option></select>
        <div class="filter-group" role="group" aria-label="版本状态">${[ ["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"] ].map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.allWorksFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <button class="icon-text-button favorite-filter images-filter ${state.allWorksImagesOnly ? "is-active" : ""}" data-action="images-only">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.allWorks)}
        <select class="sort-select" id="sort-select" aria-label="排序"><option value="date_desc" ${state.sort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.sort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.sort === "title_asc" ? "selected" : ""}>名称 A-Z</option><option value="words_desc" ${state.sort === "words_desc" ? "selected" : ""}>字数从多到少</option></select>
      </div>
      <div class="read-only-note">所有作品仅供搜索、筛选与打开查看。</div>
      <div class="works-grid">${cards || '<div class="empty-state works-empty"><h2>没有符合条件的作品</h2></div>'}</div>
    </section>`);
}

/** 跨作者的作品卡（收藏夹 / 浏览历史用）：作者名可以点，直接落到那位作者的作品库 */
function linkedWorkCard(work) {
  return `
    <article class="work-card is-read-only ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
      </div>
      <div class="work-copy">${work.authorName ? `<button class="work-author is-link" title="打开「${escapeHtml(work.authorName)}」的作品库" data-action="open-author" data-author-id="${work.authorId}">${escapeHtml(work.authorName)}</button>` : `<p class="work-author"></p>`}<div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${allWorkSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`;
}

/* ============================ 我的收藏（收藏夹） ============================ */

function renderCollections() {
  if (state.activeCollection) return renderCollectionWorks();
  const total = state.collections.reduce((sum, item) => sum + item.workCount, 0);
  const cards = state.collections.map((collection) => `
    <article class="collection-card" data-collection-id="${collection.id}" tabindex="0">
      <button class="collection-open" data-action="open-collection" data-collection-id="${collection.id}" title="打开「${escapeHtml(collection.name)}」">
        <div class="collection-cover">${collection.coverPath ? `<img src="${asset(collection.coverPath)}" alt="" loading="lazy" onerror="this.remove()">` : `<span>${icon("folderHeart", 30)}</span>`}</div>
        <div class="collection-copy"><h2>${escapeHtml(collection.name)}</h2><p>${collection.workCount} 篇作品</p></div>
      </button>
      <button class="work-menu" title="更多操作" data-action="collection-menu" data-collection-id="${collection.id}">${icon("more", 18)}</button>
    </article>`).join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div><p class="section-kicker">收藏夹</p><h1>我的收藏</h1></div>
      <div class="topbar-actions"><button class="primary-button" data-action="create-collection">${icon("plus", 18)}<span>新建收藏夹</span></button></div>
    </section>
    <section class="library-content">
      <div class="binding-bar"><div><strong>收藏夹</strong><span>${state.collections.length ? `${state.collections.length} 个收藏夹 · 共 ${total} 篇作品。同一篇作品可以同时放进多个夹子。` : "还没有收藏夹"}</span></div></div>
      <div class="collection-grid">${cards || `<div class="empty-state works-empty"><h2>还没有收藏夹</h2><p>点右上角「新建收藏夹」建一个，再到作品卡上点心形图标把作品放进去。</p></div>`}</div>
    </section>`);
}

function renderCollectionWorks() {
  const collection = state.activeCollection;
  const works = collapseSerialWorks(state.collectionWorks);
  const cards = works.map(linkedWorkCard).join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="返回收藏夹列表" data-action="back-to-collections">${icon("back", 20)}</button><div><p class="section-kicker">收藏夹</p><h1>${escapeHtml(collection.name)}</h1></div></div>
      <div class="topbar-actions"><button class="icon-text-button" data-action="rename-collection" data-collection-id="${collection.id}">${icon("settings", 18)}<span>重命名</span></button><button class="quiet-button" data-action="delete-collection" data-collection-id="${collection.id}">删除收藏夹</button></div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="collection-search" type="search" placeholder="搜索这个收藏夹里的作品名称" value="${escapeHtml(state.collectionQuery)}" autocomplete="off"></label>
        <div class="filter-group" role="group" aria-label="版本状态">${[ ["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"] ].map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button images-filter ${state.collectionImagesOnly ? "is-active" : ""}" data-action="collection-images-only">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.collectionWorks)}
        <select class="sort-select" id="collection-sort" aria-label="排序"><option value="added_desc" ${state.collectionSort === "added_desc" ? "selected" : ""}>最近收藏</option><option value="date_desc" ${state.collectionSort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.collectionSort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.collectionSort === "title_asc" ? "selected" : ""}>名称 A-Z</option><option value="words_desc" ${state.collectionSort === "words_desc" ? "selected" : ""}>字数从多到少</option></select>
      </div>
      <div class="works-grid">${cards || `<div class="empty-state works-empty"><h2>${state.collectionQuery || state.status !== "all" || state.collectionImagesOnly ? "没有符合条件的作品" : "这个收藏夹还是空的"}</h2><p>在任意作品卡上点心形图标，就能把它收进来。</p></div>`}</div>
    </section>`);
}

/* ============================ 浏览历史 ============================ */

/** 「今天 / 昨天 / N 天前 / 具体日期」——历史列表按这个分组 */
function historyDayLabel(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "更早";
  const startOfDay = (moment) => new Date(moment.getFullYear(), moment.getMonth(), moment.getDate()).getTime();
  const days = Math.round((startOfDay(new Date()) - startOfDay(date)) / 86400000);
  if (days <= 0) return "今天";
  if (days === 1) return "昨天";
  if (days < 7) return `${days} 天前`;
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function historyTimeLabel(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

function renderHistory() {
  // 已经按时间倒序回来，顺序扫一遍就是分组，不用再排序
  const groups = [];
  for (const entry of state.history) {
    const label = historyDayLabel(entry.viewedAt);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.items.push(entry);
    else groups.push({ label, items: [entry] });
  }
  const body = groups.map((group) => `
    <div class="history-group">
      <h2 class="history-day">${group.label}<span>${group.items.length} 篇</span></h2>
      <div class="history-list">${group.items.map((entry) => `
        <article class="history-item" data-work-id="${entry.work.id}">
          <button class="history-thumb" data-action="open-history-work" data-work-id="${entry.work.id}" title="打开《${escapeHtml(entry.work.title)}》">${entry.work.coverPath ? `<img src="${asset(entry.work.coverPath)}" alt="" loading="lazy" onerror="this.remove()">` : `<span>${icon("image", 20)}</span>`}</button>
          <div class="history-copy">
            <button class="history-title" data-action="open-history-work" data-work-id="${entry.work.id}" title="${escapeHtml(entry.work.title)}">${escapeHtml(entry.work.title)}</button>
            <p class="history-meta">${entry.work.authorName ? `<button class="history-author" data-action="open-author" data-author-id="${entry.work.authorId}">${escapeHtml(entry.work.authorName)}</button>` : ""}<span>${historyTimeLabel(entry.viewedAt)}</span><span class="history-state ${entry.work.purchasedPath ? "is-full" : "is-preview"}">${entry.work.purchasedPath ? "完整版" : "预览版"}</span>${entry.viewCount > 1 ? `<span>看过 ${entry.viewCount} 次</span>` : ""}</p>
          </div>
          <button class="history-remove" title="从浏览历史里移除" data-action="remove-history" data-work-id="${entry.work.id}">${icon("x", 16)}</button>
        </article>`).join("")}</div>
    </div>`).join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div><p class="section-kicker">最近打开过的作品</p><h1>浏览历史</h1></div>
      <div class="topbar-actions"><button class="quiet-button" data-action="clear-history" ${state.history.length ? "" : "disabled"}>清空历史</button></div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="history-search" type="search" placeholder="搜索标题或标签" value="${escapeHtml(state.historyQuery)}" autocomplete="off"></label>
      </div>
      <div class="read-only-note">打开作品或阅读版时自动记一笔，最多保留最近 500 条。不想记可以去「设置 → 6 维护与数据 → 浏览历史」关掉。</div>
      ${body || `<div class="empty-state works-empty"><h2>${state.historyQuery ? "没有匹配的记录" : "还没有浏览记录"}</h2><p>打开任意作品后，这里会按时间排出来。</p></div>`}
    </section>`);
}

/* ============================ 收藏夹选择器 ============================ */

/** 弹窗正文单独抽出来：勾选后只换这一块，不重开弹窗（重开会把滚动位置丢掉） */
function collectionPickerBody() {
  const selected = new Set(state.pickerSelected);
  const rows = state.collections.map((collection) => `
    <button class="picker-row ${selected.has(collection.id) ? "is-selected" : ""}" data-action="picker-toggle" data-collection-id="${collection.id}">
      <span class="picker-check">${selected.has(collection.id) ? icon("check", 15) : ""}</span>
      <span class="picker-name">${escapeHtml(collection.name)}</span>
      <small>${collection.workCount} 篇</small>
    </button>`).join("");
  return `
    <div class="picker-list">${rows || '<p class="match-note">还没有收藏夹，在下面建一个。</p>'}</div>
    <div class="picker-new">
      <input id="picker-new-name" placeholder="新建收藏夹，输入名字后回车" autocomplete="off">
      <button class="quiet-button" data-action="picker-create">${icon("plus", 16)}新建</button>
    </div>`;
}

function refreshPickerDom() {
  const holder = document.querySelector("#collection-picker-body");
  if (!holder) return;
  holder.innerHTML = collectionPickerBody();
  bindEvents();
}

async function openCollectionPicker(workId) {
  state.pickerWorkId = workId;
  state.pickerSelected = await invoke("work_collections", { workId });
  showModal(modal("收藏到…", `<div id="collection-picker-body">${collectionPickerBody()}</div>`, `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="picker-save">确定</button>`));
}

/** 新建 / 重命名收藏夹共用的「输个名字」弹窗 */
function collectionNameModal(mode, collectionId = 0, currentName = "") {
  const creating = mode === "create";
  showModal(modal(creating ? "新建收藏夹" : "重命名收藏夹",
    `<div class="form-stack"><label>收藏夹名字 <input id="collection-name-input" maxlength="24" value="${escapeHtml(currentName)}" placeholder="例如：短篇向、待读、希儿系列" autocomplete="off"></label></div><p class="match-note">最多 24 个字，同一份收藏里不能重名。</p>`,
    `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="submit-collection-name" data-mode="${mode}" data-collection-id="${collectionId}">${creating ? "创建" : "保存"}</button>`));
  window.requestAnimationFrame(() => document.querySelector("#collection-name-input")?.focus());
}

async function submitCollectionName(mode, collectionId) {
  const input = document.querySelector("#collection-name-input");
  const name = (input?.value || "").trim();
  if (!name) { toast("收藏夹名字不能为空", "error"); return; }
  const created = mode === "create"
    ? await invoke("create_collection", { name })
    : await invoke("rename_collection", { id: Number(collectionId), name });
  closeModal();
  await refreshCollections();
  // 从收藏夹里改的名字要同步到标题上，否则顶栏还挂着旧名字
  if (state.activeCollection && state.activeCollection.id === Number(collectionId)) {
    state.activeCollection = state.collections.find((item) => item.id === Number(collectionId)) || state.activeCollection;
  }
  render();
  toast(mode === "create" ? `已创建收藏夹「${created?.name || name}」` : "收藏夹已重命名", "success");
}

function collectionMenu(collectionId) {
  const collection = state.collections.find((item) => item.id === Number(collectionId));
  if (!collection) return;
  showModal(modal(collection.name, `<div class="menu-list"><button data-action="open-collection" data-collection-id="${collection.id}">${icon("arrow", 18)}打开收藏夹</button><button data-action="rename-collection" data-collection-id="${collection.id}">${icon("settings", 18)}重命名</button><button class="menu-danger" data-action="delete-collection" data-collection-id="${collection.id}">${icon("more", 18)}删除收藏夹</button></div>`));
}

function deleteCollection(collectionId) {
  const collection = state.collections.find((item) => item.id === Number(collectionId));
  if (!collection) return;
  confirmAction("删除收藏夹", `确定删除「${collection.name}」吗？夹子里的 ${collection.workCount} 篇作品不会被删除，只是不再属于这个收藏夹。`, "删除", async () => {
    await invoke("delete_collection", { id: collection.id });
    if (state.activeCollection?.id === collection.id) state.activeCollection = null;
    await refreshCollections();
    if (state.activeCollection) await refreshCollectionWorks();
    render();
    toast("收藏夹已删除", "success");
  });
}

function clearHistory() {
  if (!state.history.length) return;
  confirmAction("清空浏览历史", `确定清空全部 ${state.history.length} 条浏览记录吗？这不影响作品本身。`, "清空", async () => {
    await invoke("clear_history");
    await refreshHistory();
    render();
    toast("浏览历史已清空", "success");
  });
}

function renderSeriesWorkCards(works) {
  return works.map((work, index) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        <button class="work-menu" title="更多操作" data-action="work-menu" data-work-id="${work.id}">${icon("more", 18)}</button>
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}"><span class="work-index">${work.seriesOrder || index + 1}.</span>${escapeHtml(work.title)}</h2>${workSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`).join("");
}

function renderSeriesView() {
  const author = state.activeAuthor;
  if (state.seriesView.kind === "detail") {
    return renderShell(`
      <section class="topbar work-topbar">
        <div class="crumb-heading"><button class="back-button" title="返回系列作品" data-action="close-series-view">${icon("back", 20)}</button><div><p class="section-kicker">系列作品</p><h1>${escapeHtml(state.seriesView.title)}</h1></div></div>
      </section>
      <section class="library-content"><div class="works-grid">${renderSeriesWorkCards(state.seriesItems) || '<div class="empty-state works-empty"><h2>该系列没有作品</h2></div>'}</div></section>`);
  }
  const cards = state.seriesItems.map((series) => `<article class="series-card" data-action="open-series-card" data-series-id="${escapeHtml(series.id)}" data-series-title="${escapeHtml(series.title)}" tabindex="0">${series.coverPath ? `<img src="${asset(series.coverPath)}" alt="${escapeHtml(series.title)} 的封面">` : `<div class="series-card-placeholder">${icon("series", 30)}</div>`}<div class="series-card-copy"><h2>${escapeHtml(series.title)}</h2><p><strong>${series.workCount}</strong> 部作品 · 完整版 ${series.purchasedCount} · 预览版 ${series.previewCount}</p></div></article>`).join("");
  return renderShell(`
    <section class="topbar work-topbar"><div class="crumb-heading"><button class="back-button" title="返回作品库" data-action="close-series-view">${icon("back", 20)}</button><div><p class="section-kicker">作者作品库</p><h1>系列作品</h1></div></div></section>
    <section class="library-content"><div class="series-grid">${cards || '<div class="empty-state works-empty"><div class="empty-icon">' + icon("series", 26) + '</div><h2>还没有系列作品</h2><p>同步到的系列作品会显示在这里。</p></div>'}</div></section>`);
}

function renderEmptyWorks() {
  return `<div class="empty-state works-empty"><div class="empty-icon">${icon("image", 26)}</div><h2>没有符合条件的作品</h2><p>导入作品名称，或调整当前的搜索与筛选条件。</p><button class="primary-button" data-action="import-works">${icon("plus", 18)}<span>导入作品</span></button></div>`;
}

function modal(title, body, footer = "", variantClass = "") {
  return `<div class="modal-layer" role="presentation"><section class="modal ${variantClass}" role="dialog" aria-modal="true" aria-label="${escapeHtml(title)}"><header><h2>${escapeHtml(title)}</h2><button class="icon-button" data-action="close-modal" title="关闭">×</button></header><div class="modal-body">${body}</div>${footer ? `<footer class="modal-footer">${footer}</footer>` : ""}</section></div>`;
}

function showModal(markup) {
  document.body.insertAdjacentHTML("beforeend", markup);
  const layers = document.querySelectorAll(".modal-layer");
  const layer = layers[layers.length - 1];
  // 点遮罩关闭：配合 closeModal 只关最上层，嵌套弹窗不会互相顶掉
  layer?.addEventListener("click", (event) => { if (event.target.classList.contains("modal-layer")) closeModal(); });
  bindEvents();
}

function closeModal() {
  // 设置面板是自动保存的：趁表单还在 DOM 里，把还压在防抖里的改动先收下来写掉，关上不丢
  const settingsForm = document.querySelector("#settings-form");
  if (settingsForm && state.settingsSaveTimer) {
    window.clearTimeout(state.settingsSaveTimer);
    state.settingsSaveTimer = 0;
    try {
      const values = collectSettings(settingsForm);
      invoke("save_app_settings", { settings: values })
        .then(() => { state.searchSites = values.searchSites; })
        .catch((error) => toast(`设置没保存：${error}`, "error"));
    } catch (error) {
      // 输入到一半就关了窗，值本身不合法 —— 说一声，别让人以为存上了
      toast(`设置没保存：${error}`, "error");
    }
  }
  // 只关最上面那层：设置弹窗里再开确认框时，不能把设置一起关掉
  const layers = document.querySelectorAll(".modal-layer");
  layers[layers.length - 1]?.remove();
  state.pendingConfirmation = null;
  state.pendingMatch = null;
}

/** 匹配方式选择弹窗：真正要执行的动作先存进 state.pendingMatch，用户选完模式再执行。 */
function pickMatchMode(intent, title, note) {
  state.pendingMatch = intent;
  showModal(modal(title,
    `<p class="match-note">${note}</p>
     <div class="mode-choice">
       <button class="mode-choice-card" data-action="pick-match-mode" data-mode="character">
         <strong>匹配角色</strong>
         <span>从文件名里抽取角色名，和作品标题里的角色名取交集。适合"文件名只有作者名＋角色名、标题对不上"的情况。命中即列为候选，不卡相似度阈值。</span>
       </button>
       <button class="mode-choice-card" data-action="pick-match-mode" data-mode="title">
         <strong>匹配标题</strong>
         <span>按作品标题相似度匹配（原来的方式）。文件名和作品标题基本一致、只差一点后缀时用这个。</span>
       </button>
     </div>
     <p class="read-only-note">两种方式都需要您逐个确认，不会自动绑定。文件名里明确写了作者（例如"作者：AAA"）时，只会在这位作者的作品里匹配。</p>`,
    `<button class="quiet-button" data-action="close-modal">取消</button>`,
    "is-roomy"));
}

function confirmAction(title, message, label, action, keepOpen = false) {
  // keepOpen：设置弹窗里做二次确认时不关掉下面的设置弹窗
  if (!keepOpen) closeModal();
  state.pendingConfirmation = action;
  showModal(modal(title, `<p class="confirm-copy">${escapeHtml(message)}</p>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="danger-button" data-action="confirm-action">${escapeHtml(label)}</button>`));
}

async function runConfirmedAction() {
  const action = state.pendingConfirmation;
  state.pendingConfirmation = null;
  closeModal();
  if (action) await action();
}

async function chooseFile(extensions) {
  return open({ multiple: false, directory: false, filters: extensions ? [{ name: "文件", extensions }] : undefined });
}

function authorModal(author = {}) {
  showModal(modal(author.id ? "编辑作者" : "新增作者", `
    <form id="author-form" class="form-stack">
      <input type="hidden" name="id" value="${author.id || ""}">
      <input type="hidden" name="avatarManaged" value="${author.avatarManaged ? "true" : "false"}">
      <label>作者名称 <input name="name" required maxlength="80" value="${escapeHtml(author.name || "")}" placeholder="例如：某位作者"></label>
      <div class="form-field">作者别名
        <div class="alias-editor" data-author-alias>
          <div class="tag-editor-list">${aliasChips(author.aliases)}</div>
          <input id="alias-editor-input" placeholder="输入别名后按 Enter 添加" autocomplete="off">
        </div>
        <small>回车添加、点 × 删除；搜索作者时输入别名同样能搜到这位作者。</small>
      </div>
      <label class="is-homepage-field">Pixiv 作者主页 <input name="homepage" type="url" value="${escapeHtml(author.homepage || "")}" placeholder="https://www.pixiv.net/users/123456"><small>填写有效主页后，点击"同步作者信息"会自动获取作者名称和头像。保存时只保留到作者 ID，例如 https://www.pixiv.net/users/16208053。</small></label>
      <label>头像文件 <div class="path-input"><input name="avatarPath" value="${escapeHtml(author.avatarPath || "")}" readonly placeholder="尚未选择"><button type="button" class="quiet-button" data-action="pick-avatar">选择图片</button></div></label>
      <label>预览版文件夹 <div class="path-input"><input name="previewDir" value="${escapeHtml(author.previewDir || "")}" readonly placeholder="可在稍后绑定"><button type="button" class="quiet-button" data-action="pick-preview-dir">选择文件夹</button></div><small>在设置中配置默认目录并开启自动创建作者目录后，保存作者时会自动生成，无需手动选择。</small></label>
      <label>完整版文件夹 <div class="path-input"><input name="purchasedDir" value="${escapeHtml(author.purchasedDir || "")}" readonly placeholder="可在稍后绑定"><button type="button" class="quiet-button" data-action="pick-purchased-dir">选择文件夹</button></div><small>在设置中配置默认目录并开启自动创建作者目录后，保存作者时会自动生成，无需手动选择。</small></label>
      <label>备注 <textarea name="notes" rows="3" placeholder="可记录来源、说明等">${escapeHtml(author.notes || "")}</textarea></label>
      <div class="form-note"><small>关联相似度阈值已在全局设置中配置，无需为每个作者单独设置。</small></div>
    </form>`, `${author.id ? `<button class="danger-button" data-action="delete-author" data-author-id="${author.id}">删除作者</button>` : ""}<span class="footer-spacer"></span><button class="quiet-button" data-action="sync-author-profile">同步作者信息</button><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" form="author-form" type="submit">保存作者</button>`));
}

function importModal() {
  showModal(modal("导入作品", `
    <div class="import-tabs"><button class="filter-button is-active" data-tab="paste">粘贴文本</button><button class="filter-button" data-tab="file">Excel / CSV</button><button class="filter-button" data-tab="folder">文件夹</button></div>
    <form id="paste-import-form" class="form-stack import-panel" data-panel="paste">
      <label>固定开头 <input name="prefix" placeholder="例如：2025-" value="2025-"><small>只导入以该字符串开头的整行；行首日期会被提取。</small></label>
      <label>网页文本 <textarea name="text" rows="11" placeholder="每行一条作品名称，例如：\n2025-10-05（插画附+改编图文）～希儿&布洛妮娅.txt"></textarea></label>
    </form>
    <form id="file-import-form" class="form-stack import-panel is-hidden" data-panel="file">
      <label>作品名称列 <input name="column" type="number" min="1" value="1"><small>未选择时默认第一列。</small></label>
      <label>选择 Excel 或 CSV 文件 <div class="path-input"><input name="filePath" readonly placeholder="尚未选择文件"><button type="button" class="quiet-button" data-action="pick-import-file">选择文件</button></div></label>
    </form>
    <form id="folder-import-form" class="form-stack import-panel is-hidden" data-panel="folder">
      <label>选择作品文件夹 <div class="path-input"><input name="folderPath" readonly placeholder="尚未选择文件夹"><button type="button" class="quiet-button" data-action="pick-import-folder">选择文件夹</button></div><small>仅导入第一层中的 TXT 文件名和文件夹名；名称中的发布日期会自动提取。最小文件大小可在设置中调整。</small></label>
    </form>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="submit-import">预览导入结果</button>`));
}

/**
 * 写剪贴板：先走标准 API，失败再退回 execCommand。
 * WebView2 里 navigator.clipboard 偶尔会因为权限不给用（实测 NotAllowedError），
 * 兜底那条相当于老式复制，写完把原来的选区还回去，别打断用户接着复制。
 */
async function copyToClipboard(text) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* 落到下面走兜底 */ }
  const selection = window.getSelection();
  const ranges = selection && selection.rangeCount
    ? [...Array(selection.rangeCount)].map((_, index) => selection.getRangeAt(index).cloneRange())
    : [];
  try {
    const helper = document.createElement("textarea");
    helper.value = text;
    helper.setAttribute("readonly", "");
    helper.style.position = "fixed";
    helper.style.top = "-1000px";
    helper.style.opacity = "0";
    document.body.appendChild(helper);
    helper.select();
    const ok = document.execCommand("copy");
    helper.remove();
    if (selection && ranges.length) { selection.removeAllRanges(); ranges.forEach((range) => selection.addRange(range)); }
    return ok;
  } catch { return false; }
}

async function copyAndClose(text, label) {
  if (!text) { toast("没有可复制的内容", "info"); return; }
  const ok = await copyToClipboard(text);
  if (!ok) { toast("复制失败，可以手动选中后按 Ctrl+C", "error"); return; }
  toast(`已复制${label}`, "success");
  closeModal();
}

/**
 * 卡片上选中文字后右键：给的是「搜索」菜单 —— 把选中那截文字丢到设置里配好的网站上搜去。
 * 完整版是从别的网站弄来的，这功能就是为它准备的；没配网站时给个直达设置的入口，不留空菜单。
 * 文本要在弹菜单的那一刻就存下来 —— 点菜单按钮时浏览器已经把选区清掉了。
 */
function cardSearchMenu(work, selectedText) {
  state.copyPayload = { workId: work.id, text: selectedText };
  const preview = selectedText.length > 20 ? `${selectedText.slice(0, 20)}…` : selectedText;
  // 每个配好的网站一条：点一下就用系统浏览器打开它的搜索页，网站名高亮着，一眼看清去哪家
  // 要搜的文字已经在顶上一行亮着了，每行就不必再重复一遍，免得三行写满同样的字
  const search = state.searchSites.map((site) => `<button data-action="search-site" data-url="${escapeHtml(site.url)}">${icon("search", 18)}在 <span class="menu-site">${escapeHtml(site.name)}</span> 搜索选中文字</button>`).join("");
  // 菜单只留搜索这一件事（编辑标签走空白处右键），别再往里塞第二个用途
  showModal(modal("搜索选中文字", `<div class="menu-list">
    <p class="menu-selection">搜这段文字：<b>${escapeHtml(preview)}</b></p>
    ${search || `<button data-action="open-search-settings">${icon("plus", 18)}还没配搜索网站，点这里去添加</button>`}
  </div>`, '<button class="quiet-button" data-action="close-modal">关闭</button>'));
}

function workMenu(work) {
  // 绑定在 HTML / EPUB 上的作品才谈得上「打开阅读版」——外部带进来的电子书也算
  const bound = work.purchasedPath || work.previewPath || "";
  const reading = /\.(html?|epub)$/i.test(bound) ? `<button data-action="open-work-reading" data-work-id="${work.id}">${icon("image", 18)}打开阅读版</button>` : "";
  // 三个下载按钮都从 Pixiv 重抓最新正文，抓完都**把作品绑定到新文件上**（原来在哪一侧就还留在哪一侧）：
  // HTML / EPUB 出阅读版，txt 出纯文本（正文里的插图写成 `[插图 N：xxx_images/001.jpg]` 指引）
  const download = work.pixivNovelId ? `<button data-action="download-reading" data-work-id="${work.id}" data-format="html">${icon("file", 18)}重新下载 HTML 版并绑定</button><button data-action="download-reading" data-work-id="${work.id}" data-format="epub">${icon("file", 18)}重新下载 EPUB 版并绑定</button><button data-action="redownload-txt" data-work-id="${work.id}">${icon("file", 18)}重新下载 TXT 版并绑定</button>` : "";
  showModal(modal(work.title, `<div class="menu-list"><button data-action="open-work" data-work-id="${work.id}">${icon("arrow", 18)}打开${work.purchasedPath ? "完整版" : "预览版"}</button><button data-action="open-work-directory" data-work-id="${work.id}">${icon("folder", 18)}打开本地目录</button>${reading}${download}<button data-action="bind-work-file" data-work-id="${work.id}">${icon("folder", 18)}绑定完整版文件</button><button data-action="set-work-version" data-work-id="${work.id}" data-as-full="${work.purchasedPath ? "0" : "1"}">${icon(work.purchasedPath ? "file" : "check", 18)}${work.purchasedPath ? "设为预览版" : "设为完整版"}</button><button data-action="edit-tags" data-work-id="${work.id}">${icon("tag", 18)}编辑标签</button><button data-action="pick-collection" data-work-id="${work.id}">${icon("heart", 18)}${work.favorite ? "调整收藏夹…" : "收藏到…"}</button><button data-action="toggle-has-images" data-work-id="${work.id}">${icon("image", 18)}${work.hasImages ? "取消带图版" : "设为带图版"}</button><button class="menu-danger" data-action="delete-work" data-work-id="${work.id}">${icon("more", 18)}删除作品</button></div>`));
}

function editTagsModal(workId, tags = null) {
  const work = findWork(workId);
  if (!work) return;
  const values = tags || work.tags.split("|").filter(Boolean).map((tag) => tag.trim());
  showModal(modal("编辑标签", `<div class="tag-editor" data-work-id="${workId}"><div class="tag-editor-list">${values.map((tag, index) => `<span>${escapeHtml(tag)}<button title="删除标签" data-action="remove-tag" data-index="${index}">×</button></span>`).join("")}</div><input id="tag-editor-input" placeholder="输入标签后按 Enter 添加" autocomplete="off"></div>`, `<button class="quiet-button" data-action="open-work-directory" data-work-id="${workId}">${icon("folder", 16)}打开本地目录</button><span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="save-tags" data-work-id="${workId}">保存标签</button>`, "is-roomy"));
}

async function seriesModal(seriesId, seriesTitle) {
  const works = await invoke("list_series_works", { authorId: state.activeAuthor.id, seriesId });
  const rows = works.map((work) => `<button class="series-work-row" title="打开${work.purchasedPath ? "完整版" : "预览版"}" data-action="open-series-work" data-work-id="${work.id}"><span class="series-work-status ${work.purchasedPath ? "is-full" : "is-preview"}" title="${work.purchasedPath ? "完整版" : "预览版"}">${icon(work.purchasedPath ? "check" : "file", 15)}</span><span>${escapeHtml(work.title)}</span></button>`).join("");
  showModal(modal(seriesTitle, `<div class="series-work-list">${rows || '<p class="match-note">该系列暂时没有作品。</p>'}</div>`, '<button class="quiet-button" data-action="close-modal">关闭</button>'));
}

async function refreshSeriesView() {
  if (!state.seriesView) return;
  if (state.seriesView.kind === "detail") {
    state.seriesItems = await invoke("list_series_works", { authorId: state.activeAuthor.id, seriesId: state.seriesView.id });
  } else {
    state.seriesItems = await invoke("list_series", { authorId: state.activeAuthor.id });
  }
}

async function chooseSeriesForWorkLegacy(workId) {
  const series = await invoke("list_series", { authorId: state.activeAuthor.id });
  if (!series.length) {
    toast("当前作者还没有可加入的系列，请先同步含系列信息的作品", "info");
    return;
  }
  closeModal();
  const rows = series.map((item) => `<button class="series-choice-row" data-action="set-work-series" data-work-id="${workId}" data-series-id="${escapeHtml(item.id)}">${icon("series", 17)}<span>${escapeHtml(item.title)}</span><small>${item.workCount} 部作品</small></button>`).join("");
  showModal(modal("加入系列", `<div class="series-choice-list">${rows}</div>`, '<button class="quiet-button" data-action="close-modal">取消</button>'));
}

async function setWorkSeriesLegacy(workId, seriesId) {
  await invoke("set_work_series", { authorId: state.activeAuthor.id, workId, seriesId });
  closeModal();
  await refreshWorks();
  await refreshSeriesView();
  render();
  toast("作品已加入系列", "success");
}

async function leaveWorkSeries(workId) {
  await invoke("leave_work_series", { authorId: state.activeAuthor.id, workId });
  closeModal();
  await refreshWorks();
  await refreshSeriesView();
  render();
  toast("作品已退出系列", "success");
}

async function chooseSeriesForWorkLegacy2(workId) {
  const series = await invoke("list_series", { authorId: state.activeAuthor.id });
  if (!series.length) {
    toast("当前作者还没有可加入的系列，请先同步含系列信息的作品", "info");
    return;
  }
  closeModal();
  const options = series.map((item) => `<option value="${escapeHtml(item.id)}" data-max-order="${item.maxOrder || item.workCount || 0}">${escapeHtml(item.title)}</option>`).join("");
  showModal(modal("加入系列", `<form id="series-form" class="form-stack"><label>选择系列<select name="seriesId">${options}</select></label><label>系列序号<select name="seriesOrder" id="series-order"></select><small>已使用的序号不能重复。</small></label></form>`, '<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="set-work-series" data-work-id="' + workId + '">保存</button>'));
  const seriesSelect = document.querySelector("#series-form [name=seriesId]");
  const orderSelect = document.querySelector("#series-order");
  const populateOrders = () => {
    const maxOrder = Number(seriesSelect.selectedOptions[0]?.dataset.maxOrder || 0);
    orderSelect.innerHTML = Array.from({ length: Math.max(1, maxOrder + 1) }, (_, index) => `<option value="${index + 1}">${index + 1}</option>`).join("");
  };
  seriesSelect.onchange = populateOrders;
  populateOrders();
}

async function setWorkSeries(workId, seriesId, seriesOrder) {
  await invoke("set_work_series", { authorId: state.activeAuthor.id, workId, seriesId, seriesOrder });
  closeModal();
  await refreshWorks();
  await refreshSeriesView();
  render();
  toast("作品已加入系列", "success");
}

function seriesOrderOptions(maxOrder, selectedOrder) {
  const upper = Math.max(1, Number(maxOrder || 0) + 1, Number(selectedOrder || 0));
  return Array.from({ length: upper }, (_, index) => {
    const value = index + 1;
    return `<option value="${value}" ${value === Number(selectedOrder) ? "selected" : ""}>${value}</option>`;
  }).join("");
}

async function chooseSeriesForWork(workId) {
  const work = findWork(workId);
  if (!work) return;
  const series = await invoke("list_series", { authorId: state.activeAuthor.id });
  if (!series.length) {
    toast("当前作者还没有可加入的系列，请先同步作品", "info");
    return;
  }
  closeModal();
  if (work.seriesId) {
    const current = series.find((item) => item.id === work.seriesId);
    if (!current) throw new Error("当前作品所属系列不存在");
    const selectedOrder = work.seriesOrder || Math.max(1, Number(current.maxOrder || 0) + 1);
    showModal(modal("更改系列序号", `<form id="series-form" class="form-stack"><input type="hidden" name="seriesId" value="${escapeHtml(work.seriesId)}"><label>当前系列<input value="${escapeHtml(current.title)}" readonly></label><label>系列序号<select name="seriesOrder">${seriesOrderOptions(current.maxOrder, selectedOrder)}</select><small>不能与同一系列中的其他作品重复。</small></label></form>`, '<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="set-work-series" data-work-id="' + workId + '">保存</button>'));
    return;
  }
  const options = series.map((item) => `<option value="${escapeHtml(item.id)}" data-max-order="${item.maxOrder || 0}">${escapeHtml(item.title)}</option>`).join("");
  showModal(modal("加入系列", `<form id="series-form" class="form-stack"><label>选择系列<select name="seriesId" id="series-id">${options}</select></label><label>系列序号<select name="seriesOrder" id="series-order"></select><small>不能与同一系列中的其他作品重复。</small></label></form>`, '<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="set-work-series" data-work-id="' + workId + '">保存</button>'));
  const seriesSelect = document.querySelector("#series-id");
  const orderSelect = document.querySelector("#series-order");
  const populateOrders = () => {
    const maxOrder = Number(seriesSelect.selectedOptions[0]?.dataset.maxOrder || 0);
    orderSelect.innerHTML = seriesOrderOptions(maxOrder, maxOrder + 1);
  };
  seriesSelect.onchange = populateOrders;
  populateOrders();
}

async function bindEvents() {
  // 右下角「回到顶部 / 滚到底部」：滚动或窗口变化时刷新可用状态（监听只绑一次）
  if (!window.__scrollJumpBound) {
    window.__scrollJumpBound = true;
    window.addEventListener("scroll", updateScrollJump, { passive: true });
    window.addEventListener("resize", updateScrollJump);
  }
  updateScrollJump();
  app.querySelectorAll('[data-action="import-works"] span').forEach((label) => { label.textContent = "导入作品名称"; });
  app.querySelectorAll('[data-action="sync-pixiv"] span').forEach((label) => { label.textContent = "同步作品"; });
  const libraryActions = app.querySelector(".work-topbar .topbar-actions");
  if (libraryActions && state.activeAuthor && !state.seriesView && !state.bulkMode && !libraryActions.querySelector(".series-library-button")) {
    const button = document.createElement("button");
    button.className = "icon-text-button series-library-button";
    button.innerHTML = `${icon("series", 18)}<span>系列作品</span>`;
    button.addEventListener("click", openSeriesLibrary);
    libraryActions.prepend(button);
  }
  const bindingBar = app.querySelector(".binding-bar");
  if (bindingBar && state.activeAuthor && !app.querySelector(".library-summary")) {
    bindingBar.insertAdjacentHTML("afterend", `<div class="library-summary"><span>作品 <strong>${state.activeAuthor.workCount}</strong></span><span>完整版 <strong>${state.activeAuthor.purchasedCount}</strong></span><span>预览版 <strong>${Math.max(0, state.activeAuthor.workCount - state.activeAuthor.purchasedCount)}</strong></span><span>带图版 <strong>${state.activeAuthor.imagesCount || 0}</strong></span></div>`);
  }
  const authorSearch = app.querySelector("#author-search");
  if (authorSearch) {
    authorSearch.oncompositionstart = () => { authorSearch.dataset.composing = "true"; };
    authorSearch.oncompositionend = (event) => {
      delete authorSearch.dataset.composing;
      authorSearch.dataset.skipNextInput = "true";
      state.authorQuery = event.target.value;
      render();
      restoreSearchFocus("author-search");
    };
    authorSearch.oninput = (event) => {
      if (event.isComposing || authorSearch.dataset.composing) return;
      if (authorSearch.dataset.skipNextInput) { delete authorSearch.dataset.skipNextInput; return; }
      state.authorQuery = event.target.value;
      render();
      restoreSearchFocus("author-search");
    };
    authorSearch.onkeydown = (event) => {
      if (event.key !== "Enter" || event.isComposing || authorSearch.dataset.composing) return;
      event.preventDefault();
      state.authorQuery = event.currentTarget.value;
      render();
      restoreSearchFocus("author-search");
    };
  }
  const workSearch = app.querySelector("#work-search");
  if (workSearch) {
    workSearch.oncompositionstart = () => { workSearch.dataset.composing = "true"; };
    const commitWorkSearch = async (query) => {
      const inAllWorks = state.homeView === "allWorks" && !state.activeAuthor;
      const queryKey = inAllWorks ? "allWorksQuery" : "workQuery";
      state[queryKey] = query;
      if (inAllWorks) await refreshAllWorks(); else await refreshWorks();
      if (state[queryKey] !== query) return;
      render();
      restoreSearchFocus("work-search");
    };
    workSearch.oncompositionend = (event) => {
      delete workSearch.dataset.composing;
      workSearch.dataset.skipNextInput = "true";
      commitWorkSearch(event.target.value);
    };
    workSearch.oninput = async (event) => {
      if (event.isComposing || workSearch.dataset.composing) return;
      if (workSearch.dataset.skipNextInput) { delete workSearch.dataset.skipNextInput; return; }
      await commitWorkSearch(event.target.value);
    };
    workSearch.onkeydown = async (event) => {
      if (event.key !== "Enter" || event.isComposing || workSearch.dataset.composing) return;
      event.preventDefault();
      await commitWorkSearch(event.currentTarget.value);
    };
  }
  // 收藏夹与浏览历史的搜索框：跟作品库同一套路子，中文输入法期间不重绘
  const collectionSearch = app.querySelector("#collection-search");
  if (collectionSearch) {
    collectionSearch.oncompositionstart = () => { collectionSearch.dataset.composing = "true"; };
    const commitCollectionSearch = async (query) => {
      state.collectionQuery = query;
      await refreshCollectionWorks();
      if (state.collectionQuery !== query) return;
      render();
      restoreSearchFocus("collection-search");
    };
    collectionSearch.oncompositionend = (event) => {
      delete collectionSearch.dataset.composing;
      collectionSearch.dataset.skipNextInput = "true";
      commitCollectionSearch(event.target.value);
    };
    collectionSearch.oninput = async (event) => {
      if (event.isComposing || collectionSearch.dataset.composing) return;
      if (collectionSearch.dataset.skipNextInput) { delete collectionSearch.dataset.skipNextInput; return; }
      await commitCollectionSearch(event.target.value);
    };
    collectionSearch.onkeydown = async (event) => {
      if (event.key !== "Enter" || event.isComposing || collectionSearch.dataset.composing) return;
      event.preventDefault();
      await commitCollectionSearch(event.currentTarget.value);
    };
  }
  const collectionSort = app.querySelector("#collection-sort");
  if (collectionSort) collectionSort.onchange = async (event) => { state.collectionSort = event.target.value; await refreshCollectionWorks(); render(); };
  const historySearch = app.querySelector("#history-search");
  if (historySearch) {
    historySearch.oncompositionstart = () => { historySearch.dataset.composing = "true"; };
    const commitHistorySearch = async (query) => {
      state.historyQuery = query;
      await refreshHistory();
      if (state.historyQuery !== query) return;
      render();
      restoreSearchFocus("history-search");
    };
    historySearch.oncompositionend = (event) => {
      delete historySearch.dataset.composing;
      historySearch.dataset.skipNextInput = "true";
      commitHistorySearch(event.target.value);
    };
    historySearch.oninput = async (event) => {
      if (event.isComposing || historySearch.dataset.composing) return;
      if (historySearch.dataset.skipNextInput) { delete historySearch.dataset.skipNextInput; return; }
      await commitHistorySearch(event.target.value);
    };
    historySearch.onkeydown = async (event) => {
      if (event.key !== "Enter" || event.isComposing || historySearch.dataset.composing) return;
      event.preventDefault();
      await commitHistorySearch(event.currentTarget.value);
    };
  }
  const sortSelect = app.querySelector("#sort-select");
  if (sortSelect) sortSelect.onchange = async (event) => { state.sort = event.target.value; if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); };
  const searchField = app.querySelector("#search-field");
  if (searchField) searchField.onchange = async (event) => { state.searchField = event.target.value; if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); };

  document.querySelectorAll("[data-action]").forEach((element) => {
    if (element.dataset.bound) return;
    element.dataset.bound = "true";
    element.addEventListener("click", async (event) => {
    event.stopPropagation();
    const { action, authorId, workId, status, url } = element.dataset;
    try {
      if (action === "go-home") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "authors"; state.seriesView = null; state.seriesItems = []; state.authorQuery = ""; await refreshAuthors(); render(); }
      if (action === "go-all-works") { state.authorReturnTo = null; state.allWorksQuery = ""; state.activeAuthor = null; state.homeView = "allWorks"; state.seriesView = null; state.seriesItems = []; await refreshAllWorks(); render(); }
      // 作者作品库的返回箭头回到「所有作品」：保留原来那份搜索词，别让人回来发现搜索被清了（v0.3.68）
      if (action === "back-to-all-works") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "allWorks"; state.seriesView = null; state.seriesItems = []; await refreshAllWorks(); render(); }
      if (action === "open-author") { await openAuthorLibrary(Number(authorId)); return; }
      if (action === "help") { state.activeAuthor = null; state.homeView = "help"; state.seriesView = null; state.seriesItems = []; render(); }
      if (action === "open-external-url") { event.preventDefault(); await openExternalUrl(url); }      if (action === "new-author") authorModal();
      if (action === "edit-author") { const author = state.authors.find((item) => item.id === Number(authorId)) || state.activeAuthor; authorModal(author); }
      if (action === "sync-author-profile") await syncAuthorProfile();
      if (action === "sync-all-authors") await syncAllAuthors();
      if (action === "close-modal") closeModal();
      if (action === "status") { state.status = status; if (state.homeView === "collections" && !state.activeAuthor) await refreshCollectionWorks(); else if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); }
      if (action === "favorites-only") { if (state.homeView === "allWorks" && !state.activeAuthor) { state.allWorksFavoritesOnly = !state.allWorksFavoritesOnly; await refreshAllWorks(); } else { state.authorFavoritesOnly = !state.authorFavoritesOnly; await refreshWorks(); } render(); }
      if (action === "images-only") { if (state.homeView === "allWorks" && !state.activeAuthor) { state.allWorksImagesOnly = !state.allWorksImagesOnly; await refreshAllWorks(); } else { state.authorImagesOnly = !state.authorImagesOnly; await refreshWorks(); } render(); }
      if (action === "serial-latest") { state.serialLatestOnly = !state.serialLatestOnly; render(); }
      if (action === "toggle-author-starred") {
        const targetId = Number(authorId);
        const starred = await invoke("toggle_author_starred", { authorId: targetId });
        const author = state.authors.find((item) => item.id === targetId);
        if (author) author.starred = starred;
        if (state.activeAuthor && state.activeAuthor.id === targetId) state.activeAuthor.starred = starred;
        render();
        toast(starred ? `已将「${author?.name || "该作者"}」设为特别关注` : `已取消「${author?.name || "该作者"}」的特别关注`, "success");
      }
      if (action === "authors-starred-only") { state.authorsStarredOnly = !state.authorsStarredOnly; render(); }
      if (action === "scroll-top") { scrollLibraryTo("top"); return; }
      if (action === "scroll-bottom") { scrollLibraryTo("bottom"); return; }
      if (action === "import-works") importModal();
      if (action === "sync-pixiv") pixivSyncModal();
      if (action === "scan-preview") await scanPreview();
      if (action === "scan-purchased") pickMatchMode({ action: "scan-purchased" }, "关联完整版文件", "先选择用哪种方式在作者的作品里找对应作品。");
      if (action === "pick-match-mode") {
        const mode = element.dataset.mode === "character" ? "character" : "title";
        const intent = state.pendingMatch;
        state.pendingMatch = null;
        closeModal();
        if (intent?.action === "scan-purchased") await scanPurchased(mode);
        else if (intent?.action === "auto-group") await autoGroupPurchasedFiles(intent.authorId ?? null, intent.authorName || "", mode);
        return;
      }
      if (action === "work-menu") workMenu(findWork(Number(workId)));
      if (action === "open-series") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, state.seriesView?.returnTo || "works");
      if (action === "open-all-series") await openAllWorksSeries(Number(authorId), element.dataset.seriesId, element.dataset.seriesTitle);
      if (action === "open-series-library") await openSeriesLibrary();
      if (action === "open-series-card") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, "overview");
      if (action === "close-series-view") await closeSeriesView();
      if (action === "edit-tags") { closeModal(); editTagsModal(Number(workId)); }
      if (action === "copy-selected-text") await copyAndClose(state.copyPayload?.text, "选中的文字");
      if (action === "copy-work-title") await copyAndClose(findWork(Number(workId))?.title, "标题");
      if (action === "copy-work-author") await copyAndClose(findWork(Number(workId))?.authorName, "作者名");
      if (action === "copy-work-link") await copyAndClose(pixivNovelUrl(findWork(Number(workId))?.pixivNovelId), "Pixiv 链接");
      if (action === "join-series") await chooseSeriesForWork(Number(workId));
      if (action === "change-series-order") await chooseSeriesForWork(Number(workId));
      if (action === "set-work-series") {
        const form = document.querySelector("#series-form");
        const values = Object.fromEntries(new FormData(form).entries());
        await setWorkSeries(Number(workId), values.seriesId, Number(values.seriesOrder));
      }
      if (action === "leave-series") await leaveWorkSeries(Number(workId));
      if (action === "remove-tag") removeEditingTag(Number(element.dataset.index));
      if (action === "remove-author-alias") removeAuthorAlias(Number(element.dataset.index));
      if (action === "save-tags") await saveTags(Number(workId));
      if (action === "pick-collection") { closeModal(); await openCollectionPicker(Number(workId)); }
      if (action === "go-collections") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "collections"; state.seriesView = null; state.seriesItems = []; state.activeCollection = null; state.collectionQuery = ""; await refreshCollections(); render(); }
      if (action === "go-history") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "history"; state.seriesView = null; state.seriesItems = []; state.historyQuery = ""; await refreshHistory(); render(); }
      if (action === "open-collection") { const collection = state.collections.find((item) => item.id === Number(element.dataset.collectionId)); if (!collection) { toast("这个收藏夹已经不在了，刷新一下", "error"); return; } closeModal(); state.activeCollection = collection; state.collectionQuery = ""; await refreshCollectionWorks(); render(); }
      if (action === "back-to-collections") { state.activeCollection = null; state.collectionQuery = ""; state.collectionImagesOnly = false; await refreshCollections(); render(); }
      if (action === "collection-menu") { closeModal(); collectionMenu(element.dataset.collectionId); }
      if (action === "create-collection") { closeModal(); collectionNameModal("create"); }
      if (action === "rename-collection") {
        const collection = state.collections.find((item) => item.id === Number(element.dataset.collectionId));
        if (!collection) { toast("这个收藏夹已经不在了，刷新一下", "error"); return; }
        closeModal(); collectionNameModal("rename", collection.id, collection.name);
      }
      if (action === "delete-collection") { closeModal(); await deleteCollection(element.dataset.collectionId); }
      if (action === "submit-collection-name") { await submitCollectionName(element.dataset.mode, element.dataset.collectionId); }
      if (action === "picker-toggle") {
        const id = Number(element.dataset.collectionId);
        state.pickerSelected = state.pickerSelected.includes(id) ? state.pickerSelected.filter((item) => item !== id) : [...state.pickerSelected, id];
        refreshPickerDom();
      }
      if (action === "picker-create") {
        const input = document.querySelector("#picker-new-name");
        const name = (input?.value || "").trim();
        if (!name) { toast("先给收藏夹起个名字", "error"); input?.focus(); return; }
        const created = await invoke("create_collection", { name });
        await refreshCollections();
        state.pickerSelected = [...state.pickerSelected, created.id];
        refreshPickerDom();
      }
      if (action === "picker-save") {
        await invoke("set_work_collections", { workId: Number(state.pickerWorkId), collectionIds: state.pickerSelected });
        closeModal();
        await refreshAfterWorkFlagChange();
        toast(state.pickerSelected.length ? "已更新收藏夹" : "已从所有收藏夹移除", "success");
        state.pickerWorkId = null;
        state.pickerSelected = [];
        return;
      }
      if (action === "collection-images-only") { state.collectionImagesOnly = !state.collectionImagesOnly; await refreshCollectionWorks(); render(); }
      if (action === "open-history-work") { await invoke("open_work", { workId: Number(workId) }); await refreshHistory(); render(); }
      if (action === "remove-history") { await invoke("remove_history", { workId: Number(workId) }); await refreshHistory(); render(); }
      if (action === "clear-history") { await clearHistory(); }
      // 设置面板里清历史：二次确认弹窗叠在设置上面，别把设置一起关掉（keepOpen）
      if (action === "clear-history-settings") {
        confirmAction("清空浏览历史", "确定清空全部浏览记录吗？这不影响作品本身。", "清空", async () => {
          await invoke("clear_history");
          await refreshHistory();
          toast("浏览历史已清空", "success");
        }, true);
      }
      if (action === "toggle-has-images") { await invoke("toggle_has_images", { workId: Number(workId) }); await refreshAfterWorkFlagChange(); }
      if (action === "toggle-select") toggleWorkSelection(Number(workId));
      if (action === "bulk-mode") toggleBulkMode();
      if (action === "select-all") toggleSelectAll();
      if (action === "copy-selected-full") await copySelectedToFull();
      if (action === "set-images-selected") await setSelectedHasImages();
      if (action === "delete-work") await deleteWork(Number(workId));
      if (action === "delete-selected") await deleteSelectedWorks();
      if (action === "open-work") { await invoke("open_work", { workId: Number(workId) }); closeModal(); if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); }
      if (action === "open-work-directory") { await invoke("open_work_directory", { workId: Number(workId) }); if (element.closest(".menu-list")) closeModal(); }
      if (action === "open-work-url") {
        const url = pixivNovelUrl(findWork(Number(workId))?.pixivNovelId);
        if (!url) { toast("这个作品没有 Pixiv 作品 ID，先同步一次才能跳过去", "info"); return; }
        await openExternalUrl(url);
      }
      if (action === "open-work-reading") { await openWorkReading(Number(workId)); return; }
      if (action === "download-reading") { await downloadReadingVersion(Number(workId), element.dataset.format === "epub" ? "epub" : "html"); return; }
      if (action === "download-selected-reading") { await downloadSelectedReadings(element.dataset.format === "epub" ? "epub" : ""); return; }
      if (action === "backfill-images") { await downloadSelectedReadings(""); return; }
      if (action === "bind-work-file") await bindWork(Number(workId), false);
      if (action === "set-work-version") await setWorkVersion(Number(workId), element.dataset.asFull === "1");
      if (action === "pick-avatar") await pickPath("avatarPath", false, ["jpg", "jpeg", "png", "webp"]);
      if (action === "pick-preview-dir") await pickPath("previewDir", true);
      if (action === "pick-purchased-dir") await pickPath("purchasedDir", true);
      if (action === "pick-import-file") await pickPath("filePath", false, ["csv", "xlsx", "xls"]);
      if (action === "pick-import-folder") await pickPath("folderPath", true);
      if (action === "submit-import") await submitImport();
      if (action === "settings") await settingsModal();
      if (action === "open-update") openUpdateModal();
      if (action === "check-update") {
        const result = await checkForUpdate();
        // 手点检查时查到新版就直接把更新弹窗摆出来，省得用户再去找左下角
        if (result?.hasUpdate) openUpdateModal();
      }
      if (action === "start-update") { await startUpdateDownload(); return; }
      if (action === "ignore-update") {
        ignoreUpdateVersion(element.dataset.version || "");
        if (state.update) state.update.hasUpdate = false;
        closeModal();
        render();
        toast(`已忽略 v${element.dataset.version}，出新的还会提醒`, "info");
      }
      if (action === "open-release-page") {
        await openExternalUrl(state.update?.releaseUrl || RELEASE_PAGE_URL);
      }
      if (action === "open-release-mirror-page") {
        await openExternalUrl(state.update?.releaseMirrorUrl || `${DEFAULT_UPDATE_MIRROR}${RELEASE_PAGE_URL}`);
      }
      if (action === "reset-update-mirrors") {
        const list = await invoke("default_update_mirror_list");
        const box = document.querySelector('#settings-form [name="updateMirrors"]');
        if (box) {
          box.value = (list || []).join("\n");
          scheduleSettingsSave(document.querySelector("#settings-form"));
        }
      }
      if (action === "pick-pixiv-cookie") await importPixivCookie();
      if (action === "pick-default-preview") await pickPath("defaultPreviewDir", true);
      if (action === "pick-default-purchased") await pickPath("defaultPurchasedDir", true);
      if (action === "pick-auto-group-dir") await pickPath("autoGroupDir", true);
      if (action === "auto-group") pickMatchMode({ action: "auto-group", authorId: null, authorName: "" }, "完整版自动分组", "先选择用哪种方式把文件匹配到作品上，然后再开始分组。");
      if (action === "auto-group-by-author") autoGroupAuthorPicker();
      if (action === "add-search-site") addSearchSiteRow();
      if (action === "remove-search-site") {
        element.closest(".search-site-item")?.remove();
        // 删光了就把提示补回来，别留一片空白让人以为界面坏了
        const siteBox = document.querySelector("#search-site-editor");
        if (siteBox && !siteBox.querySelector(".search-site-row")) siteBox.innerHTML = `<p class="search-site-empty">还没配搜索网站，点下面的「添加网站」加一个。</p>`;
        // 删行不是表单输入，不会触发 input 事件，得自己催一下自动保存
        scheduleSettingsSave(document.querySelector("#settings-form"));
      }
      if (action === "search-site") await runSearchSite(element.dataset.url, state.copyPayload?.text);
      if (action === "open-search-settings") await openSearchSettings();
      if (action === "add-character") await addCharacter();
      if (action === "add-character-game") await addCharacterGame();
      if (action === "toggle-character") await toggleCharacter(Number(element.dataset.id));
      if (action === "delete-character") await deleteCharacter(Number(element.dataset.id));
      if (action === "rename-character-game") await renameCharacterGame(element.dataset.game, element);
      if (action === "delete-character-game") deleteCharacterGame(element.dataset.game);
      if (action === "confirm-auto-group-author") await confirmAutoGroupAuthor();
      if (action === "confirm-pixiv-sync") await syncPixivWorks();
      if (action === "cancel-pixiv-sync") await cancelPixivSync();
      // 批量下载阅读版没法中断，只能把进度浮层收起来；任务照跑，结束时照常弹提示
      if (action === "hide-images-progress") { state.syncTask = null; render(); return; }
      if (action === "delete-author") await deleteAuthor(Number(authorId));
      if (action === "export-backup") await exportBackup();
      if (action === "restore-backup") await restoreBackup();
      if (action === "clean-preview-versions") await cleanupPreviewVersions();
      if (action === "redownload-txt") await redownloadNovelTxt(Number(workId));
      if (action === "confirm-matches") await confirmMatches();
      if (action === "confirm-manual-group") await confirmManualGroup();
      if (action === "confirm-action") await runConfirmedAction();
    } catch (error) { toast(String(error), "error"); }
    });
  });

  document.querySelectorAll(".author-card").forEach((card) => {
    if (card.dataset.bound) return;
    card.dataset.bound = "true";
    card.addEventListener("mousedown", (event) => {
      if (event.button === 0) state.cardPressPoint = { x: event.clientX, y: event.clientY };
    });
    card.addEventListener("click", async (event) => {
    if (event.target.closest("button")) return;
    // 在作者名上拖选复制时别顺带把作者切走
    if (isCardDragClick(event, card)) return;
    state.activeAuthor = state.authors.find((author) => author.id === Number(card.dataset.authorId));
    state.homeView = "authors";
    state.authorFavoritesOnly = false;
    state.authorImagesOnly = false;
    await refreshWorks(); render();
    });
  });

  // 作者卡拖动排序：按住拖动按钮 0.2 秒把卡片「拿起来」——
  // 拿起那一刻克隆出一层跟手浮层（fixed，跟着指针走、微微倾斜、带大影子），
  // 原位置留成虚化的占位槽，指针划过谁就让谁平滑让位，松手落库。
  // 就是拖桌面图标那个手感：卡片是"被拎起来"的，不是原地换位置。
  document.querySelectorAll(".card-drag").forEach((handle) => {
    if (handle.dataset.bound) return;
    handle.dataset.bound = "true";
    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      const card = handle.closest(".author-card");
      const grid = card?.parentElement;
      if (!card || !grid) return;
      event.preventDefault();
      event.stopPropagation();
      const session = {
        card,
        grid,
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        orderBefore: [...grid.querySelectorAll(".author-card")].map((item) => Number(item.dataset.authorId)),
        armed: false,
        ghost: null,
        grabX: 0,
        grabY: 0,
        timer: 0,
      };
      session.timer = window.setTimeout(() => liftAuthorCard(), AUTHOR_DRAG_HOLD_MS);
      authorDragSession = session;
    });
  });
  if (!document.documentElement.dataset.authorDragBound) {
    document.documentElement.dataset.authorDragBound = "true";
    window.addEventListener("pointermove", (event) => {
      const session = authorDragSession;
      if (!session || event.pointerId !== session.pointerId) return;
      if (!session.armed) {
        // 长按还没到就动了 → 当成误触/滚动，放弃这次拖动
        if (Math.abs(event.clientX - session.startX) > 8 || Math.abs(event.clientY - session.startY) > 8) {
          window.clearTimeout(session.timer);
          authorDragSession = null;
        }
        return;
      }
      event.preventDefault();
      moveAuthorGhost(event.clientX, event.clientY);
      const grid = session.card.parentElement;
      if (!grid) return;
      const target = authorDragTarget(grid, session.card, event.clientX, event.clientY);
      // 已经在这个位置了就别白动 DOM —— 否则 FLIP 每帧算一堆 0 位移，卡片还会抖
      const nextSibling = session.card.nextElementSibling;
      if (target ? nextSibling === target : !nextSibling) return;
      reorderAuthorCards(grid, session.card, target);
    });
    window.addEventListener("pointerup", (event) => {
      if (!authorDragSession || event.pointerId !== authorDragSession.pointerId) return;
      finishAuthorDrag(authorDragSession.armed);
    });
    window.addEventListener("pointercancel", (event) => {
      if (!authorDragSession || event.pointerId !== authorDragSession.pointerId) return;
      finishAuthorDrag(false);
    });
  }

  document.querySelectorAll(".work-card").forEach((card) => {
    if (card.dataset.bound) return;
    card.dataset.bound = "true";
    card.addEventListener("mousedown", (event) => {
      // 右键按下这一刻选区还在（浏览器是 mousedown 之后、contextmenu 之前才清掉它的），
      // 先拍下来，右键菜单要靠它判断「用户是不是选中了文字」；左键按下则清掉旧快照
      state.cardSelectionText = event.button === 2 ? cardSelectedText(card) : "";
      // 记下按下位置；双击（detail ≥ 2）说明用户想选中标题里的词，把排队中的打开撤掉
      if (event.button !== 0) return;
      if (event.detail > 1) cancelPendingCardOpen();
      state.cardPressPoint = { x: event.clientX, y: event.clientY };
    });
    card.addEventListener("click", (event) => {
    if (event.target.closest("button")) return;
    if (state.suppressCardClick) return;
    // 拖动选中文字后浏览器还会补一个 click，这里吃掉它，好让人把标题复制走
    if (isCardDragClick(event, card)) return;
    if (state.bulkMode) { toggleWorkSelection(Number(card.dataset.workId)); return; }
    // 只有封面图和标题可以打开文件，卡片其他位置（日期、标签等）点击无反应
    if (!event.target.closest(".work-open")) return;
    queueCardOpen(Number(card.dataset.workId), card);
    });
    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      // 选中了文字（标题、标签、日期都行）再右键 → 给「搜索」菜单；
      // 没选中 → 老样子，直接开「编辑标签」，别在中间再垫一层操作菜单。
      // 右键点偏了导致选区被清时，用按下时的快照兜住。
      const selected = cardSelectedText(card) || state.cardSelectionText;
      state.cardSelectionText = "";
      const work = findWork(Number(card.dataset.workId));
      if (selected && work) { cardSearchMenu(work, selected); return; }
      if (card.classList.contains("is-read-only")) return;
      if (work) editTagsModal(work.id);
    });
  });

  const authorForm = document.querySelector("#author-form");
  if (authorForm && !authorForm.dataset.bound) {
    authorForm.dataset.bound = "true";
    authorForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      try {
        const form = new FormData(event.currentTarget);
        const author = Object.fromEntries(form.entries());
        author.id = author.id ? Number(author.id) : null;
        author.avatarManaged = author.avatarManaged === "true";
        author.aliases = authorAliases().join("|");
        const saved = await invoke("save_author", { author });
        await refreshAuthors();
        if (state.activeAuthor?.id === saved.id) state.activeAuthor = saved;
        closeModal();
        render();
      } catch (error) {
        toast(String(error), "error");
      }
    });
  }

  const settingsForm = document.querySelector("#settings-form");
  if (settingsForm && !settingsForm.dataset.bound) {
    const settings = await invoke("get_app_settings");
    document.querySelector("#settings-slot-pixiv")?.insertAdjacentHTML("beforeend", `<label class="delay-settings">Pixiv 抓取间隔 <div class="delay-input"><span>同步作品超过</span><input name="pixivDelayThreshold" type="number" min="1" step="1" value="${Number(settings.pixivDelayThreshold || 150)}"><span>部时，每部间隔</span><input name="pixivDelaySeconds" type="number" min="0" max="60" step="1" value="${Number(settings.pixivDelaySeconds ?? 1)}"><span>秒</span></div><small>超过阈值后，作品详情请求会按此间隔执行，降低连续抓取频率。默认超过 150 部时每部间隔 1 秒；填 0 秒可关闭间隔。</small></label>`);
    // 存量作品补角标：绑在 EPUB / HTML 上的作品，图片数可能在绑定之前就已经存在
    document.querySelector("#settings-slot-maintain")?.insertAdjacentHTML("beforeend", `<label>阅读版图片数 <button type="button" class="quiet-button" data-action="refresh-reading-image-counts">立即重算</button><small>扫描全库里绑定在 EPUB / HTML 上的作品，重新统计作品卡上的图片角标（数的是文件里的插图，不含封面）。绑定普通 txt 的作品不受影响。</small></label>`);
    document.querySelector("[data-action='refresh-reading-image-counts']")?.addEventListener("click", async (event) => {
      const button = event.currentTarget;
      button.disabled = true;
      try {
        const result = await invoke("refresh_reading_image_counts");
        const scanned = Number(result?.scannedCount || 0);
        const updated = Number(result?.updatedCount || 0);
        if (state.activeAuthor) await refreshWorks();
        else if (state.homeView === "allWorks") await refreshAllWorks();
        render();
        toast(scanned ? `已重算 ${scanned} 篇阅读版的图片数（其中 ${updated} 篇有变化）` : "没有作品绑定在 EPUB / HTML 上", "success");
      } catch (error) {
        toast(String(error), "error");
      } finally {
        button.disabled = false;
      }
    });
    settingsForm.dataset.bound = "true";
    // 自动保存：表单里任何一处改动（含槽位里后插的输入框）停手半秒就写库，不再有「保存设置」按钮
    settingsForm.addEventListener("input", (event) => {
      // 又在改网址了：上一条「已自动清洗」的说明已经过期，先撤掉
      event.target?.closest?.(".search-site-item")?.querySelector(".search-site-hint")?.remove();
      scheduleSettingsSave(settingsForm);
    });
    // 网址失焦时自动清洗（行是后加的，所以走委托）：改了就写回输入框并说明改了什么
    settingsForm.addEventListener("focusout", (event) => {
      const input = event.target?.closest?.(".search-site-url");
      if (!input) return;
      const before = input.value;
      cleanSearchUrlInput(input);
      if (input.value !== before) scheduleSettingsSave(settingsForm);
    });
    settingsForm.addEventListener("change", () => scheduleSettingsSave(settingsForm));
    // 表单里已经没有提交按钮，万一按回车触发隐式提交，就当一次「立刻保存」处理
    settingsForm.addEventListener("submit", (event) => { event.preventDefault(); flushSettingsSave(settingsForm); });
  }

  const tagInput = document.querySelector("#tag-editor-input");
  if (tagInput && !tagInput.dataset.bound) {
    tagInput.dataset.bound = "true";
    tagInput.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); addEditingTag(); } });
  }

  // 作者别名输入框：回车就地把别名加进去（不能重开弹窗，否则会丢掉表单里其它未保存的修改）
  const aliasInput = document.querySelector("#alias-editor-input");
  if (aliasInput && !aliasInput.dataset.bound) {
    aliasInput.dataset.bound = "true";
    aliasInput.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); addAuthorAlias(); } });
  }

  const tagEditor = document.querySelector(".tag-editor");
  if (tagEditor && !tagEditor.dataset.seriesBound) {
    tagEditor.dataset.seriesBound = "true";
    const work = findWork(Number(tagEditor.dataset.workId));
    const footer = document.querySelector(".modal-footer");
    if (work && footer) {
      footer.insertAdjacentHTML("afterbegin", work.seriesId
        ? `<button class="quiet-button" data-action="change-series-order" data-work-id="${work.id}">更改系列序号</button><button class="quiet-button" data-action="leave-series" data-work-id="${work.id}">退出系列</button>`
        : `<button class="quiet-button" data-action="join-series" data-work-id="${work.id}">加入系列</button>`);
      bindEvents();
    }
  }

  document.querySelectorAll("[data-tab]").forEach((button) => {
    if (button.dataset.bound) return;
    button.dataset.bound = "true";
    button.addEventListener("click", () => {
    document.querySelectorAll("[data-tab]").forEach((item) => item.classList.toggle("is-active", item === button));
    document.querySelectorAll(".import-panel").forEach((panel) => panel.classList.toggle("is-hidden", panel.dataset.panel !== button.dataset.tab));
    });
  });

  bindMarqueeSelection();
}

async function pickPath(field, directory, extensions) {
  const path = await open({ directory, multiple: false, filters: extensions ? [{ name: "文件", extensions }] : undefined });
  if (path) document.querySelector(`[name="${field}"]`).value = path;
}

async function syncAuthorProfile() {
  const form = document.querySelector("#author-form");
  const values = Object.fromEntries(new FormData(form).entries());
  if (!values.homepage) throw new Error("请先填写 Pixiv 作者主页。");
  // 同步会重开这个表单（后端会把名称/主页/头像回填），先把还没保存的别名记下来，避免被清掉
  const pendingAliases = authorAliases().join("|");
  const controls = document.querySelectorAll("#author-form input, #author-form textarea, #author-form button, .modal-footer button");
  controls.forEach((control) => { control.disabled = true; });
  const button = document.querySelector('[data-action="sync-author-profile"]');
  button?.classList.add("is-loading");
  const previousMarkup = button?.innerHTML;
  if (button) button.textContent = "正在同步...";
  try {
    const author = await invoke("sync_pixiv_author_profile", { authorId: values.id ? Number(values.id) : null, homepage: values.homepage });
    await refreshAuthors();
    closeModal();
    authorModal({ ...author, aliases: pendingAliases });
    toast("已获取作者名称和头像，请保存作者后创建目录", "success");
  } catch (error) {
    controls.forEach((control) => { control.disabled = false; });
    button?.classList.remove("is-loading");
    if (button && previousMarkup) button.innerHTML = previousMarkup;
    throw error;
  }
}

async function scanPreview() {
  if (!state.activeAuthor.previewDir) {
    const path = await open({ directory: true, multiple: false });
    if (!path) return;
    state.activeAuthor = await invoke("update_author_path", { authorId: state.activeAuthor.id, field: "preview", path });
  }
  const result = await invoke("scan_preview", { authorId: state.activeAuthor.id });
  await refreshWorks();
  if (result.createdCount || result.boundCount || result.ambiguousCount) {
    const details = [];
    if (result.createdCount) details.push(`新增作品 ${result.createdCount} 个`);
    if (result.boundCount) details.push(`新增关联 ${result.boundCount} 个`);
    if (result.ambiguousCount) details.push(`${result.ambiguousCount} 个同分候选未自动关联`);
    toast(details.join("；"), result.ambiguousCount ? "info" : "success");
  }
  else toast("未找到可匹配的预览版内容或封面，请检查名称与目录第一层文件", "info");
  await refreshActiveAuthor();
  render();
}

async function scanPurchased(mode = "title") {
  if (!state.activeAuthor.purchasedDir) {
    throw new Error("请先在作者设置中选择完整版文件夹");
  }
  const byCharacter = mode === "character";
  const result = await invoke("scan_purchased", { authorId: state.activeAuthor.id, mode });
  // 同步时下下来就已经绑好的文件，后端直接跳过、不会再列出来让人重绑一遍
  const skipped = result.skippedCount || 0;
  const skipNote = skipped ? `，另有 ${skipped} 个文件已关联过、已跳过` : "";
  const unknownGroups = result.unknownAuthorGroups || [];
  const unknownCount = unknownGroups.reduce((sum, group) => sum + group.files.length, 0);
  const unknownNote = unknownCount ? `，另有 ${unknownCount} 个文件的作者不在作者库里，已单独列出` : "";
  // 文件名写着库里另一位作者的文件：不属于这个目录，后端已跳过
  const otherNote = result.otherAuthorCount ? `，${result.otherAuthorCount} 个文件写着别的作者、已跳过` : "";
  if (result.selections.length) {
    // 按角色匹配时后端不做任何自动绑定，全部交给用户确认
    const head = byCharacter
      ? `按角色找到 ${result.selections.length} 个候选文件，请逐个确认`
      : `已自动绑定 ${result.boundCount} 个作品${skipNote}，${result.selections.length} 个完整版文件待您选择`;
    toast(`${head}${byCharacter ? unknownNote : ""}${otherNote}`, "info");
    showPurchasedSelections(result.selections, unknownGroups, mode);
  } else if (unknownCount) {
    toast(`${byCharacter ? "没有角色的名字能对上" : "没有新的完整版文件需要关联"}，另有 ${unknownCount} 个文件的作者不在作者库里${otherNote}`, "info");
    showPurchasedSelections([], unknownGroups, mode);
  } else if (result.boundCount === 0 && skipped) {
    toast(`没有新的完整版文件需要关联，${skipped} 个文件此前已关联${otherNote}`, "info");
  } else if (byCharacter) {
    toast(`没有角色的名字能对上，换个方式或用「匹配标题」试试${otherNote}`, "info");
  } else toast(`已自动绑定 ${result.boundCount} 个完整版作品${skipNote}${otherNote}`, "success");
  await refreshWorks(); await refreshActiveAuthor(); render();
}

// 自动分组：authorId 为空＝全库匹配；传入作者 id 时只在这位作者的作品里匹配
async function autoGroupPurchasedFiles(authorId = null, authorName = "", mode = "title") {
  const scope = authorId ? `「${authorName}」的作品` : "全部作者的作品";
  const byCharacter = mode === "character";
  try {
    const result = await invoke("auto_group_purchased_files", { authorId, mode });
    const unknownGroups = result.unknownAuthorGroups || [];
    const unknownCount = unknownGroups.reduce((sum, group) => sum + group.files.length, 0);
    const unknownNote = unknownCount ? `，${unknownCount} 个文件的作者不在作者库里` : "";
    if (result.manualSelections.length > 0) {
      const movedNote = byCharacter
        ? `按角色在${scope}中找到 ${result.manualSelections.length} 个文件需要您确认（按角色匹配不会自动移动文件）`
        : `已在${scope}中自动移动 ${result.autoMovedCount} 个文件，${result.manualSelections.length} 个文件需要手动选择`;
      toast(`${movedNote}${unknownNote}`, "info");
      showManualGroupSelections(result.manualSelections, unknownGroups, mode, Boolean(authorId));
    } else if (unknownCount) {
      toast(`没有需要分组的文件，${unknownCount} 个文件的作者不在作者库里`, "info");
      showManualGroupSelections([], unknownGroups, mode, Boolean(authorId));
    } else if (result.autoMovedCount > 0) {
      toast(`已在${scope}中自动移动 ${result.autoMovedCount} 个文件到对应作者文件夹`, "success");
    } else {
      toast(byCharacter ? "没有角色的名字能对上，换个方式或用「匹配标题」试试" : "没有找到需要分组的文件", "info");
    }
    await refreshAuthors();
    render();
  } catch (error) {
    toast(`自动分组失败: ${error}`, "error");
  }
}

// 指定作者自动分组：先选作者，再只在这位作者的作品里匹配文件名
function autoGroupAuthorPicker() {
  if (!state.authors.length) { toast("还没有作者，请先新增作者", "info"); return; }
  const options = state.authors.map((author) => {
    const usable = Boolean(author.purchasedDir);
    return `<option value="${author.id}" ${usable ? "" : "disabled"}>${escapeHtml(author.name)}（${author.workCount} 个作品${usable ? "" : " · 未绑定完整版文件夹"}）</option>`;
  }).join("");
  const usableCount = state.authors.filter((author) => author.purchasedDir).length;
  showModal(modal("指定作者自动分组",
    `<p class="match-note">只在选中的这位作者的作品里做文件名匹配，其他作者的作品一律不参与。适合"某位作者的书正好混在一起、想单独归档"的情况。</p>
     <label class="match-row"><span>选择作者</span><select id="auto-group-author">${options}</select></label>
     <p class="read-only-note">共 ${usableCount} 位作者已绑定完整版文件夹、可参与分组；未绑定的作者在列表里是灰的。分组范围仍是设置里的「完整版自动分组文件夹」。</p>`,
    `<button class="quiet-button" data-action="close-modal">取消</button>
     <button class="primary-button" data-action="confirm-auto-group-author" ${usableCount ? "" : "disabled"}>开始分组</button>`
  ));
}

async function confirmAutoGroupAuthor() {
  const select = document.querySelector("#auto-group-author");
  if (!select) return;
  const authorId = Number(select.value);
  const author = state.authors.find((item) => item.id === authorId);
  if (!author) { toast("请选择要分组的作者", "info"); return; }
  closeModal();
  pickMatchMode({ action: "auto-group", authorId, authorName: author.name }, "指定作者自动分组", `先选择用哪种方式在「${escapeHtml(author.name)}」的作品里匹配，然后再开始分组。`);
}

async function bindWork(workId, directory) {
  // 图片（封面、插图）不参与关联：能绑的只有文本、电子书和 HTML（目录不受此限制）
  const options = { directory, multiple: false };
  if (!directory) options.filters = [{ name: "作品文件", extensions: ["txt", "md", "html", "htm", "xhtml", "epub", "mobi", "azw", "azw3", "fb2", "lit", "pdf"] }];
  const path = await open(options);
  if (!path) return;
  await invoke("bind_work", { workId, path });
  closeModal(); await refreshWorks(); await refreshActiveAuthor(); render();
  toast("已绑定本地完整版内容", "success");
}

function toggleBulkMode() {
  state.bulkMode = !state.bulkMode;
  state.selectedWorkIds.clear();
  render();
}

function toggleWorkSelection(workId) {
  if (state.selectedWorkIds.has(workId)) state.selectedWorkIds.delete(workId);
  else state.selectedWorkIds.add(workId);
  render();
}

function toggleSelectAll() {
  const works = collapseSerialWorks(state.works);
  const areAllSelected = works.length > 0 && works.every((work) => state.selectedWorkIds.has(work.id));
  if (areAllSelected) state.selectedWorkIds.clear();
  else works.forEach((work) => state.selectedWorkIds.add(work.id));
  render();
}

function syncSelectionUI() {
  document.querySelectorAll(".selection-badge").forEach((badge) => {
    const selected = state.selectedWorkIds.has(Number(badge.dataset.workId));
    badge.classList.toggle("is-selected", selected);
    badge.innerHTML = selected ? icon("check", 16) : "";
    badge.title = selected ? "取消选择" : "选择作品";
  });
  const count = state.selectedWorkIds.size;
  document.querySelectorAll("[data-count-target]").forEach((button) => {
    const label = button.dataset.countLabel;
    button.textContent = `${label}（${count}）`;
    button.disabled = count === 0;
  });
}

function bindMarqueeSelection() {
  if (document.body.dataset.marqueeBound === "true") return;
  document.body.dataset.marqueeBound = "true";
  document.addEventListener("mousedown", (event) => {
    if (!state.bulkMode || event.button !== 0) return;
    if (event.target.closest("button, input, select, textarea, a, .modal-layer, .side-rail, .topbar, .library-tools, .binding-bar, .empty-state")) return;
    const scope = document.querySelector(".works-grid");
    if (!scope) return;
    event.preventDefault();
    const additive = event.ctrlKey || event.shiftKey;
    const baseSelection = new Set(state.selectedWorkIds);
    const startX = event.clientX;
    const startY = event.clientY;
    let rectEl = null;
    let moved = false;
    const onMove = (moveEvent) => {
      const x1 = Math.min(startX, moveEvent.clientX);
      const y1 = Math.min(startY, moveEvent.clientY);
      const x2 = Math.max(startX, moveEvent.clientX);
      const y2 = Math.max(startY, moveEvent.clientY);
      if (Math.abs(x2 - x1) > 4 || Math.abs(y2 - y1) > 4) {
        if (!rectEl) {
          rectEl = document.createElement("div");
          rectEl.className = "marquee-rect";
          document.body.appendChild(rectEl);
          document.body.classList.add("is-marquee");
        }
        moved = true;
        rectEl.style.left = `${x1}px`;
        rectEl.style.top = `${y1}px`;
        rectEl.style.width = `${x2 - x1}px`;
        rectEl.style.height = `${y2 - y1}px`;
        const next = new Set(additive ? baseSelection : []);
        scope.querySelectorAll(".work-card").forEach((card) => {
          const box = card.getBoundingClientRect();
          const hit = !(box.right < x1 || box.left > x2 || box.bottom < y1 || box.top > y2);
          if (hit) next.add(Number(card.dataset.workId));
        });
        state.selectedWorkIds = next;
        syncSelectionUI();
      }
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      rectEl?.remove();
      document.body.classList.remove("is-marquee");
      if (moved) {
        state.suppressCardClick = true;
        window.setTimeout(() => { state.suppressCardClick = false; }, 60);
        render();
      }
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  });
}

/**
 * 判断这次 click 是不是「拖动选中文字」留下的尾巴。
 *
 * 浏览器在 mousedown / mouseup 落在同一个元素上时一定会补发 click，不管中间拖了多远 ——
 * 所以想复制标题就得先把它和真正的单击区分开。两种情形都要放过：
 *   1. 按住拖过（起止坐标差 > 4px），哪怕是反向拖回来也算；
 *   2. 双击 / 三击选中了词或整段（此时坐标没动，但选区非空）。
 * 命中就把本次 click 吃掉，选区保留，Ctrl+C 照常可用。
 */
function isCardDragClick(event, container) {
  const press = state.cardPressPoint;
  state.cardPressPoint = null;
  if (press && (Math.abs(event.clientX - press.x) > 4 || Math.abs(event.clientY - press.y) > 4)) return true;
  return Boolean(cardSelectedText(container));
}

/**
 * 单击打开文件前等的这一小会儿：让双击的「第二下」来得及把它撤销。
 * 暂设为 0＝立即打开 —— 双击目前没有额外功能，不值得为此让每次打开都慢半拍。
 * 哪天要给标题加双击行为（比如双击复制标题），把这个值调回 300 就能生效。
 */
const CARD_OPEN_DELAY = 0;

function cancelPendingCardOpen() {
  if (!state.pendingCardOpen) return;
  window.clearTimeout(state.pendingCardOpen);
  state.pendingCardOpen = 0;
}

async function runCardOpen(workId, card) {
  if (!document.contains(card)) return; // 这期间列表被重绘过，就当这次点击不作数
  try {
    await invoke("open_work", { workId });
    if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks();
    render();
  } catch (error) { toast(String(error), "error"); }
}

/** 排队打开作品（延迟为 0 时就是直接打开） */
function queueCardOpen(workId, card) {
  cancelPendingCardOpen();
  if (CARD_OPEN_DELAY <= 0) { runCardOpen(workId, card); return; }
  state.pendingCardOpen = window.setTimeout(() => {
    state.pendingCardOpen = 0;
    runCardOpen(workId, card);
  }, CARD_OPEN_DELAY);
}

/** 这张卡片里当前选中的文字（选区折叠或落在别的元素上时返回空串） */
function cardSelectedText(card) {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed) return "";
  const text = String(selection).trim();
  if (!text) return "";
  const anchor = selection.anchorNode;
  if (anchor && card.contains(anchor)) return text;
  return "";
}

async function deleteWork(workId) {
  const work = state.works.find((item) => item.id === workId);
  if (!work) return;
  confirmAction("确认删除作品", `删除“${work.title}”只会移除软件记录和路径绑定，不会删除磁盘中的原始文件。`, "删除作品", async () => {
    await invoke("delete_work", { workId });
    await refreshWorks(); await refreshActiveAuthor(); render();
    toast("作品记录已删除，原始文件未受影响", "success");
  });
}

async function deleteSelectedWorks() {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) return;
  confirmAction("确认批量删除", `将删除 ${workIds.length} 条作品记录和路径绑定，不会删除磁盘中的原始文件。`, "删除已选作品", async () => {
    await invoke("delete_works", { workIds });
    state.bulkMode = false; state.selectedWorkIds.clear();
    await refreshWorks(); await refreshActiveAuthor(); render();
    toast(`已删除 ${workIds.length} 条作品记录，原始文件未受影响`, "success");
  });
}

async function copySelectedToFull() {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) return;
  const result = await invoke("copy_previews_to_purchased", { authorId: state.activeAuthor.id, workIds });
  state.bulkMode = false; state.selectedWorkIds.clear();
  await refreshWorks(); await refreshActiveAuthor(); render();
  const skipped = result.skippedCount ? `；${result.skippedCount} 条没有可用预览版，已跳过` : "";
  toast(`已把 ${result.copiedCount} 条搬到完整版目录并绑定 ${result.boundCount} 条完整版${skipped}`, result.skippedCount ? "info" : "success");
}

async function setSelectedHasImages() {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) return;
  await invoke("set_has_images", { workIds, hasImages: true });
  state.bulkMode = false; state.selectedWorkIds.clear();
  await refreshWorks(); render();
  toast(`已将 ${workIds.length} 个作品设为带图版`, "success");
}

// 图文小说：打开同步时生成的阅读版（HTML 交给浏览器，EPUB 交给系统默认阅读器）
async function openWorkReading(workId) {
  try {
    await invoke("open_work_reading", { workId });
    closeModal();
  } catch (error) {
    toast(String(error), "error");
  }
}

// 订阅批量下载阅读版的进度事件。浏览器预览里没有 Tauri 事件系统，
// 订阅失败就退化成「没有进度条」，不影响主流程（跑完照常弹结果提示）。
async function listenReadingProgress(handler) {
  try {
    return await listen("reading-download-progress", handler);
  } catch {
    return () => {};
  }
}

// 「重新下载 HTML / EPUB 版并绑定」：抓最新正文 → 补配图（已有的跳过）→ 生成指定格式的
// 阅读版（与正文同目录同名）→ 把作品绑定到它。格式由按钮决定，不看设置
// （预览版 / 完整版属性不变，只是「打开作品」从此打开这份阅读版）。
async function downloadReadingVersion(workId, format) {
  closeModal();
  const label = format === "epub" ? "EPUB" : "HTML";
  toast(`正在重新下载 ${label} 版（第一次会顺带把配图下到本地）…`, "info");
  try {
    const result = await invoke("download_reading_version", { workId, format });
    if (state.activeAuthor) await refreshWorks(); else await refreshAllWorks();
    await refreshActiveAuthor();
    render();
    const missing = result.missingCount ? `，${result.missingCount} 张配图没拿到` : "";
    const size = result.sizeBytes ? `：${formatMegabytes(result.sizeBytes)}` : "";
    const noImages = result.totalCount ? "" : "（这篇正文里没有配图）";
    toast(`已重新下载 ${label} 版并绑定${size}，含配图 ${result.savedCount} 张${missing}${noImages}`, missing ? "info" : "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

function formatMegabytes(bytes) {
  const value = Number(bytes) || 0;
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(0)} KB`;
  return `${value} B`;
}

// 批量：对勾选的作品「重新下载阅读版并绑定」，右下角带进度条。
// format 传空串＝跟随设置里选的格式（批量操作里的「补下配图（N）」走的就是这条），
// 传 "html" / "epub" 就是强制那种格式。
async function downloadSelectedReadings(format) {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) { toast("请先进入批量操作，勾选要处理的作品", "info"); return; }
  // 右下角浮层是共用的，别把正在跑的同步任务的进度顶掉
  if (state.syncTask && state.syncTask.kind !== "reading") {
    toast("同步任务正在进行，请等它结束或先终止，再批量下载阅读版", "info");
    return;
  }
  const label = format === "epub" ? "EPUB" : format === "html" ? "HTML" : "阅读版";
  state.syncTask = {
    kind: "reading",
    label: `正在重新下载${label}并绑定`,
    title: "正在准备…",
    current: 0,
    total: workIds.length,
    cancelAction: "hide-images-progress",
    cancelText: "隐藏进度",
  };
  render();
  const unlisten = await listenReadingProgress((event) => {
    const { total = 0, current = 0, title = "", done = false } = event.payload || {};
    // 收尾那一条不做 UI 更新，浮层由 invoke 返回后统一收起
    if (done || !state.syncTask) return;
    state.syncTask.total = total;
    state.syncTask.current = current;
    state.syncTask.title = title;
    updateSyncFloater();
  });
  let result;
  try {
    result = await invoke("download_reading_versions", { workIds, format });
  } catch (error) {
    unlisten();
    state.syncTask = null;
    render();
    toast(String(error), "error");
    return;
  }
  unlisten();
  state.syncTask = null;
  state.bulkMode = false;
  state.selectedWorkIds.clear();
  if (state.activeAuthor) await refreshWorks(); else await refreshAllWorks();
  await refreshActiveAuthor();
  render();
  const failed = result.failedCount ? `，${result.failedCount} 篇失败` : "";
  const skipped = result.skippedCount ? `，跳过 ${result.skippedCount} 篇（已绑定）` : "";
  const names = result.failedTitles.length ? `（${result.failedTitles.slice(0, 3).join("、")}${result.failedTitles.length > 3 ? "…" : ""}）` : "";
  const formatName = String(result.format || "html").toUpperCase();
  if (!result.exportedCount) {
    toast(`没有需要重新下载的篇目${skipped}${failed}${failed ? names : ""}`, failed ? "info" : "success");
    return;
  }
  const size = result.totalBytes ? `，共 ${formatMegabytes(result.totalBytes)}` : "";
  toast(`已重新下载 ${formatName} 版并绑定 ${result.exportedCount} 篇${size}、配图 ${result.imageCount} 张${skipped}${failed}${failed ? names : ""}`, failed ? "info" : "success");
}

// 手动纠正「完整版 / 预览版」判定：同步时按简介自动判断会出错，这里让用户手动改回来
async function setWorkVersion(workId, asFull) {
  closeModal();
  try {
    await invoke(asFull ? "mark_work_as_full" : "mark_work_as_preview", { workId });
  } catch (error) {
    toast(String(error), "error");
    return;
  }
  if (!state.activeAuthor) await refreshAllWorks(); else await refreshWorks();
  await refreshActiveAuthor();
  render();
  toast(asFull ? "已设为完整版" : "已设为预览版", "success");
}

/** 文件名里认出来的作者名徽标（例如"作者：AAA"） */
function authorTagHtml(authorName) {
  if (!authorName) return "";
  return `<span class="file-author" title="文件名里写明了作者，因此只在「${escapeHtml(authorName)}」的作品里匹配">作者：${escapeHtml(authorName)}</span>`;
}

/** 命中到的角色名徽标 */
function characterTagsHtml(names) {
  if (!names || !names.length) return "";
  return names.map((name) => `<span class="char-tag">${escapeHtml(name)}</span>`).join("");
}

/** 文件名里写了作者、但作者不在作者库里的文件：按作者名分组，组下面是文件名，纯展示不给下拉框 */
function unknownAuthorSection(groups) {
  if (!groups || !groups.length) return "";
  const rows = groups.map((group) => {
    const files = group.files.map((path) => {
      const fileName = path.split(/[\\/]/).pop();
      return `<li class="unknown-file" title="${escapeHtml(path)}">${escapeHtml(fileName)}</li>`;
    }).join("");
    return `<div class="unknown-group">
      <div class="unknown-author"><strong>${escapeHtml(group.author)}</strong><span class="unknown-count">${group.files.length} 个文件</span></div>
      <ul class="unknown-files">${files}</ul>
    </div>`;
  }).join("");
  return `<section class="unknown-section">
    <h3>作者不在作者库里</h3>
    <p class="read-only-note">这些文件名里写明了作者（例如"作者：AAA"），但这位作者不在您的作者库里，所以没有可以匹配的作品，这里只做展示。把这几位作者加进作者库后再来匹配，或者手工处理这些文件。</p>
    <div class="unknown-list">${rows}</div>
  </section>`;
}

function showPurchasedSelections(selections, unknownGroups = [], mode = "title") {
  // 结构与「自动分组」的选择弹窗对齐：宽弹窗 + 文件名超长时省略号（不再把弹窗撑宽）
  const byCharacter = mode === "character";
  const rows = selections.map((selection) => {
    const fileName = selection.path.split(/[\\/]/).pop();
    const options = [`<option value="">暂不绑定</option>`]
      .concat(selection.candidates.map((candidate) => {
        // 按角色匹配时括号里给命中的角色名，比相似度更能说明为什么是它
        const tail = byCharacter && candidate.matchedCharacters && candidate.matchedCharacters.length
          ? `角色：${candidate.matchedCharacters.join("、")}`
          : `${candidate.similarity}%`;
        return `<option value="${candidate.workId}">${escapeHtml(candidate.title)}（${escapeHtml(tail)}）</option>`;
      }))
      .join("");
    const hint = `<span class="group-file-hint">${selection.candidates.length} 个候选作品</span>`;
    return `<div class="group-item">
      <div class="group-file">${authorTagHtml(selection.authorName)}<span class="group-file-main"><span class="group-file-name" title="${escapeHtml(selection.path)}">${escapeHtml(fileName)}</span>${characterTagsHtml(selection.matchedCharacters)}</span>${hint}</div>
      <div class="group-options"><label class="match-row"><span>关联到作品</span><select data-purchased-path="${escapeHtml(selection.path)}">${options}</select></label></div>
    </div>`;
  }).join("");
  const note = byCharacter
    ? `按角色名匹配出来的候选：角色名对得上就会列在这里，不管标题像不像，所以请逐个确认。下拉框括号里是命中的角色名。`
    : `有多个候选作品时请手动选择；未达到相似度阈值的文件仅列出相似度最高的 3 个作品。`;
  const empty = selections.length ? "" : `<p class="match-note">没有需要确认的文件。</p>`;
  showModal(modal("选择要关联的作品",
    `<p class="match-note">${note}</p>${empty}<div class="match-list">${rows}</div>${unknownAuthorSection(unknownGroups)}`,
    `<button class="quiet-button" data-action="close-modal">稍后处理</button><button class="primary-button" data-action="confirm-matches" ${selections.length ? "" : "disabled"}>确认绑定</button>`,
    "is-wide"));
}

function showManualGroupSelections(selections, unknownGroups = [], mode = "title", preselect = false) {
  const byCharacter = mode === "character";
  const rows = selections.map((selection, groupIndex) => {
    const recommendedCount = selection.candidates.filter((candidate) => candidate.recommended).length;
    const conflictCount = selection.candidates.filter((candidate) => candidate.conflict).length;
    const options = selection.candidates.map((candidate) => {
      // 只有「指定作者自动分组」那条路（preselect=true）、且只有一个达标候选时才默认勾选；
      // 全库自动分组一律不预勾 —— 相似度只是个参考，勾错一下文件就挪到别的作品里了
      const checked = preselect && recommendedCount === 1 && candidate.recommended ? "checked" : "";
      const conflictTag = candidate.conflict ? '<span class="group-conflict">已有同名文件</span>' : "";
      const charTags = characterTagsHtml(candidate.matchedCharacters);
      const score = byCharacter && charTags
        ? ""
        : `<span class="group-sim ${candidate.recommended ? "is-strong" : ""}">${candidate.similarity}%</span>`;
      return `<label class="group-option">
        <input type="checkbox" data-work-id="${candidate.workId}" ${checked}>
        <span class="group-option-text"><strong>${escapeHtml(candidate.authorName)}</strong><em>${escapeHtml(candidate.workTitle)}</em></span>
        ${charTags}${conflictTag}${score}
      </label>`;
    }).join("");
    const hint = byCharacter
      ? `<span class="group-file-hint is-strong">${selection.candidates.length} 个作品的角色名对得上，勾选几个就复制几份</span>`
      : recommendedCount > 1
        ? `<span class="group-file-hint is-strong">${recommendedCount} 个作品达到关联阈值，勾选几个就复制几份</span>`
        : `<span class="group-file-hint">${selection.candidates.length} 个候选作品</span>`;
    const conflictBar = conflictCount
      ? `<div class="group-conflict-bar">
           <span class="group-conflict-note">${conflictCount} 个候选在作者目录里已有同名文件，处理方式（默认覆盖原文件）：</span>
           <label><input type="radio" name="conflict-action-${groupIndex}" value="skip"><span>跳过</span></label>
           <label><input type="radio" name="conflict-action-${groupIndex}" value="overwrite" checked><span>覆盖原文件</span></label>
           <label><input type="radio" name="conflict-action-${groupIndex}" value="keepBoth"><span>保留两份</span></label>
         </div>`
      : "";
    return `<div class="group-item" data-group-file data-file-path="${escapeHtml(selection.filePath)}">
      <div class="group-file">${selection.subDir ? `<span class="group-subdir" title="文件所在子文件夹：${escapeHtml(selection.subDir)}">${escapeHtml(selection.subDir)}/</span>` : ""}${authorTagHtml(selection.authorName)}<span class="group-file-main"><span class="group-file-name" title="${escapeHtml(selection.fileName)}">${escapeHtml(selection.fileName)}</span>${characterTagsHtml(selection.matchedCharacters)}</span>${hint}</div>
      <div class="group-options">${options}</div>
      ${conflictBar}
    </div>`;
  }).join("");
  const empty = selections.length ? "" : `<p class="match-note">没有需要分组的文件。</p>`;

  showModal(modal("选择要分组到的作品", 
    `<p class="match-note">勾选 1 个作品＝移动文件；勾选多个作品＝复制多份，每个作品都会各自拿到一份，文件名按各自作品标题命名。${preselect ? "" : "这里默认不勾选任何作品，相似度达标也只做个标记，请自己确认。"}<br>${byCharacter ? "按角色匹配：只要角色名对得上就会列为候选（忽略标题相似度），所以请逐个确认。" : `<span class="group-tag">推荐</span> 表示该候选相似度已达到关联阈值；`}<span class="group-conflict">已有同名文件</span> 表示该作品的完整版目录里已存在同名文件；文件名前的灰色文字是它所在的子文件夹。</p>
     ${empty}<div class="match-list">${rows}</div>${unknownAuthorSection(unknownGroups)}`,
    `<button class="quiet-button" data-action="close-modal">稍后处理</button>
     <button class="primary-button" data-action="confirm-manual-group" ${selections.length ? "" : "disabled"}>确认分组</button>`
  , "is-wide"));
}

async function confirmMatches() {
  const selections = [...document.querySelectorAll("[data-purchased-path]")].map((select) => ({ workId: Number(select.value), path: select.dataset.purchasedPath })).filter((item) => item.workId);
  let renamedCount = 0;
  for (const item of selections) {
    try {
      const newPath = await invoke("bind_work_with_rename", item);
      if (newPath !== item.path) renamedCount++;
    } catch (error) {
      console.error("绑定失败:", error);
      toast(`绑定失败: ${error}`, "error");
    }
  }
  closeModal(); await refreshWorks(); await refreshActiveAuthor(); render();
  if (renamedCount > 0) {
    toast(`已确认绑定 ${selections.length} 个完整版作品，其中 ${renamedCount} 个文件已重命名`, "success");
  } else {
    toast(`已确认绑定 ${selections.length} 个完整版作品`, "success");
  }
}

async function confirmManualGroup() {
  const choices = [...document.querySelectorAll("[data-group-file]")]
    .map((item) => ({
      filePath: item.dataset.filePath,
      workIds: [...item.querySelectorAll("input[type=checkbox]:checked")].map((box) => Number(box.dataset.workId)),
      conflictAction: item.querySelector('input[type="radio"]:checked')?.value || "overwrite",
    }))
    .filter((choice) => choice.workIds.length);
  
  if (choices.length === 0) {
    toast("没有勾选要分组的作品", "info");
    return;
  }
  
  try {
    const result = await invoke("confirm_manual_group", { choices });
    closeModal();
    await refreshAuthors();
    render();
    const dupNote = result.duplicatedCount ? `，其中 ${result.duplicatedCount} 个文件复制了多份` : "";
    const skipNote = result.skippedCount ? `，跳过 ${result.skippedCount} 个已存在同名文件的作品` : "";
    const failNote = result.failed?.length ? `，${result.failed.length} 个文件处理失败` : "";
    if (result.boundCount > 0) {
      toast(`已为 ${result.boundCount} 个作品绑定完整版文件${dupNote}${skipNote}${failNote}`, failNote ? "info" : "success");
    } else if (result.skippedCount > 0) {
      toast(`全部跳过：${result.skippedCount} 个作品已存在同名文件`, "info");
    } else {
      toast(`没有可绑定的作品${failNote}`, "error");
    }
    if (result.failed?.length) console.warn("分组失败的文件：", result.failed);
  } catch (error) {
    toast(`分组失败: ${error}`, "error");
  }
}

async function deleteAuthor(authorId) {
  confirmAction("确认删除作者", "删除作者只会移除软件记录和绑定关系，不会删除磁盘中的原始作品文件。", "删除作者", async () => {
    await invoke("delete_author", { authorId });
    state.activeAuthor = null; await refreshAuthors(); render();
    toast("作者记录已删除，原始文件未受影响", "success");
  });
}

async function exportBackup() {
  const path = await save({ defaultPath: "collection-library-backup.db", filters: [{ name: "数据库备份", extensions: ["db"] }] });
  if (!path) return;
  await invoke("export_backup", { path });
  toast("数据库备份已导出", "success");
}

async function restoreBackup() {
  const path = await open({ multiple: false, directory: false, filters: [{ name: "数据库备份", extensions: ["db"] }] });
  if (!path) return;
  confirmAction("确认恢复备份", "恢复会覆盖当前软件记录与绑定关系，但不会修改原始作品文件。", "恢复备份", async () => {
    await invoke("restore_backup", { path });
    state.activeAuthor = null; await refreshAuthors(); render();
    toast("已从备份恢复资料库", "success");
  });
}

/**
 * 作品卡三个点菜单里的「重新下载 TXT 版并绑定」：从 Pixiv 重抓正文，在作品所在目录落一份 txt，
 * 并把作品绑到这份 txt 上（跟 HTML / EPUB 两个下载入口同一路子，作品原来在哪一侧就还留在哪一侧）。
 */
async function redownloadNovelTxt(workId) {
  const work = findWork(workId);
  closeModal();
  toast(`正在从 Pixiv 重新下载《${work?.title || "这篇作品"}》的 TXT 版并绑定 …`, "info");
  try {
    const path = await invoke("redownload_novel_txt", { workId });
    if (state.activeAuthor) await refreshWorks();
    render();
    toast(`TXT 版已下载并绑定：${path}`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/**
 * 设置 →「清理多余预览版」：已有完整版的作品，预览版就没必要留了。
 * 分两步 —— 先只扫不删（apply=false）把数字报给用户，确认后才真动手。
 * 清理是「移进回收目录」而不是物理删除，所以敢让它一键跑；封面与配图会跟着搬到完整版目录。
 */
async function cleanupPreviewVersions() {
  try {
    const scan = await invoke("cleanup_redundant_previews", { apply: false });
    if (!scan.candidates) {
      toast("没有「同时挂着预览版和完整版」的作品，不用清理", "info");
      return;
    }
    const message = [
      `发现 ${scan.candidates} 篇作品同时挂着预览版与完整版。`,
      `其中 ${scan.ready} 篇的完整版文件确认在磁盘上，可以安全清掉预览版；另有 ${scan.skippedMissingFull} 篇完整版文件找不到，会跳过不动。`,
      "清出来的文件会送进 Windows 回收站（资源管理器里能还原），不是直接删除；封面和配图会跟着搬到完整版目录，避免断图和封面失效。",
    ].join("");
    confirmAction("清理多余预览版", message, "开始清理", async () => {
      const done = await invoke("cleanup_redundant_previews", { apply: true });
      state.activeAuthor = null;
      await refreshAuthors();
      render();
      const bits = [`清掉 ${done.cleaned} 篇预览版`];
      if (done.recordOnly) bits.push(`${done.recordOnly} 篇只剩库记录`);
      if (done.recycled) bits.push(`${done.recycled} 个文件/目录已送进回收站`);
      if (done.imagesMoved) bits.push(`搬走 ${done.imagesMoved} 个配图目录`);
      if (done.coversMoved) bits.push(`${done.coversMoved} 张封面跟着搬到完整版目录`);
      if (done.skippedMissingFull) bits.push(`跳过 ${done.skippedMissingFull} 篇（完整版文件找不到）`);
      if (done.recycleFailed) bits.push(`${done.recycleFailed} 个送回收站失败、原样留着`);
      toast(bits.join("，"), done.recycleFailed ? "info" : "success");
    }, true);
  } catch (error) {
    toast(String(error), "error");
  }
}

async function submitImport() {
  const pastePanel = document.querySelector('[data-panel="paste"]');
  const folderPanel = document.querySelector('[data-panel="folder"]');
  const usingPaste = !pastePanel.classList.contains("is-hidden");
  const usingFolder = !folderPanel.classList.contains("is-hidden");
  let lines = [];
  if (usingPaste) {
    const form = new FormData(document.querySelector("#paste-import-form"));
    const prefix = form.get("prefix").trim();
    lines = form.get("text").split(/\r?\n/).filter((line) => line.trim() && line.trim().startsWith(prefix));
  } else if (usingFolder) {
    const form = new FormData(document.querySelector("#folder-import-form"));
    const path = form.get("folderPath");
    if (!path) throw new Error("请先选择作品文件夹");
    const settings = await invoke("get_app_settings");
    const minimumSizeBytes = Number(settings.minimumFileSizeBytes || 0);
    lines = await invoke("read_import_folder", { path, minimumSizeBytes });
  } else {
    const form = new FormData(document.querySelector("#file-import-form"));
    const path = form.get("filePath");
    if (!path) throw new Error("请先选择 Excel 或 CSV 文件");
    lines = await invoke("read_import_file", { path, column: Number(form.get("column") || 1) });
  }
  const preview = await invoke("preview_import", { authorId: state.activeAuthor.id, lines });
  const body = `<p class="import-summary">可新增 <strong>${preview.newCount}</strong> 条；重复 <strong>${preview.duplicateCount}</strong> 条；跳过 <strong>${preview.invalidCount}</strong> 条。</p>${preview.duplicates.length ? `<div class="duplicate-list">${preview.duplicates.map((item) => `<span>${escapeHtml(item)}</span>`).join("")}</div>` : ""}`;
  closeModal();
  showModal(modal("确认导入", body, `<button class="quiet-button" data-action="close-modal">取消</button><button class="quiet-button" data-action="commit-import" data-overwrite="false">跳过重复项</button><button class="primary-button" data-action="commit-import" data-overwrite="true">覆盖已有记录</button>`));
  document.querySelectorAll('[data-action="commit-import"]').forEach((button) => button.addEventListener("click", async () => {
    const result = await invoke("commit_import", { authorId: state.activeAuthor.id, lines, overwrite: button.dataset.overwrite === "true" });
    closeModal(); await refreshWorks(); await refreshAuthors(); render();
    toast(`导入完成：新增 ${result.created} 条，更新 ${result.updated} 条，跳过 ${result.skipped} 条`, "success");
  }));
}

function isPixivNovelUrl(value) {
  try {
    const url = new URL(value.trim());
    return url.protocol === "https:" && url.hostname === "www.pixiv.net" && url.pathname === "/novel/show.php" && /^\d+$/.test(url.searchParams.get("id") || "");
  } catch {
    return false;
  }
}

function pixivSyncModal() {
  const author = state.activeAuthor;
  showModal(modal("作品同步", `
    <form id="pixiv-sync-form" class="form-stack">
      <div class="sync-intro"><span class="sync-intro-icon">${icon("sync", 22)}</span><div><strong>同步 Pixiv 小说</strong><p>${!author.previewDir ? "请先在作者设置中绑定预览版文件夹。" : !author.purchasedDir ? "请先在作者设置中绑定完整版文件夹。" : !author.homepage ? "填写单篇小说地址可直接同步；批量同步则需先填写作者主页。" : "将下载新小说正文、封面与标签到预览版目录。"}</p></div></div>
      <label>单篇小说地址 <input name="novelUrl" type="url" placeholder="https://www.pixiv.net/novel/show.php?id=28686066"><small>填写有效地址后只同步该作品，不更新上次成功同步时间，也不使用日期范围。</small></label>
      <div class="date-range"><label>开始日期 <input name="startDate" type="date"></label><label>结束日期 <input name="endDate" type="date"></label></div>
      <small>批量同步时，投稿时间以 Pixiv 原始投稿时间为准。不填日期时，仅检查上次成功同步后的新投稿；填写日期后，按指定投稿时间范围重新检查。已有作品会先按 Pixiv 小说 ID、再按关联相似度跳过。</small>
      <p class="read-only-note">点击开始后同步会在后台进行，窗口关闭也不影响；进度显示在页面右下角，可随时终止。</p>
    </form>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="confirm-pixiv-sync" ${author.homepage && author.previewDir && author.purchasedDir ? "" : "disabled"}>${icon("sync", 17)}开始同步</button>`));
  const novelUrlInput = document.querySelector('[name="novelUrl"]');
  const syncButton = document.querySelector('[data-action="confirm-pixiv-sync"]');
  novelUrlInput?.addEventListener("input", () => {
    const canSync = Boolean(author.previewDir && author.purchasedDir && (author.homepage || isPixivNovelUrl(novelUrlInput.value)));
    syncButton.disabled = !canSync;
  });
}

async function syncPixivWorks() {
  const form = document.querySelector("#pixiv-sync-form");
  if (!form) return;
  const { startDate = "", endDate = "", novelUrl = "" } = Object.fromEntries(new FormData(form).entries());
  const authorId = state.activeAuthor.id;
  const authorName = state.activeAuthor.name;
  // 同步放后台跑：先关掉弹窗，主界面右下角显示进度，用户可以继续浏览其他内容
  closeModal();
  state.syncTask = { authorId, label: `正在同步 ${authorName}`, title: "", current: 0, total: 0, cancelling: false };
  render();
  const unlisten = await listen("pixiv-sync-progress", (event) => {
    const { total = 0, current = 0, title = "" } = event.payload || {};
    if (!state.syncTask) return;
    state.syncTask.total = total;
    state.syncTask.current = current;
    state.syncTask.title = title;
    updateSyncFloater();
  });
  let result;
  try {
    result = await invoke("sync_pixiv_novels", { authorId, startDate, endDate, novelUrl });
  } catch (error) {
    unlisten();
    state.syncTask = null;
    render();
    toast(`同步失败：${error}`, "error");
    return;
  }
  unlisten();
  state.syncTask = null;
  await refreshAuthors();
  state.activeAuthor = state.authors.find((author) => author.id === authorId) || state.activeAuthor;
  await refreshWorks();
  render();
  const summary = `已下载 ${result.downloadedCount} 篇；已关联 ${result.reusedPreviewCount || 0} 篇已有预览版；已跳过 ${result.skippedExistingCount} 篇已有作品；日期筛除 ${result.skippedDateCount} 篇；大小筛除 ${result.skippedSizeCount || 0} 篇`;
  toast(result.cancelled ? `同步已终止；${summary}` : (result.failedCount ? `${summary}；${result.failedCount} 篇失败，将在下次同步时重试` : summary), result.cancelled || result.failedCount ? "info" : "success");
}

// 作者库的「同步所有作者」：逐位串行同步，全程后台运行，右下角为每位作者各显示一根进度条
async function syncAllAuthors() {
  if (state.syncTask) { toast("已有同步任务正在进行，请等待完成或先终止", "info"); return; }
  const targets = state.authors.filter((author) => author.homepage && author.previewDir && author.purchasedDir);
  if (!targets.length) {
    toast("没有可同步的作者：请先填写作者主页，并绑定预览版与完整版文件夹", "info");
    return;
  }
  state.syncTask = {
    authorId: null,
    label: `准备同步 ${targets.length} 位作者`,
    cancelling: false,
    authors: targets.map((author) => ({ authorId: author.id, name: author.name, status: "pending", current: 0, total: 0, title: "", note: "" })),
  };
  render();
  const unlisten = await listen("pixiv-sync-progress", (event) => {
    const { authorId = null, total = 0, current = 0, title = "" } = event.payload || {};
    const authors = state.syncTask?.authors;
    if (!authors) return;
    // 按 authorId 精确落到对应作者那一行；万一没带 authorId 就退回当前正在跑的那位
    const row = authors.find((item) => item.authorId === Number(authorId)) || authors.find((item) => item.status === "running");
    if (!row || row.status !== "running") return;
    row.total = total;
    row.current = current;
    row.title = title;
    updateSyncFloater();
  });

  let doneAuthors = 0;
  let downloaded = 0;
  let stopped = false;
  const failures = [];

  for (let index = 0; index < targets.length; index++) {
    const author = targets[index];
    // 用户点了「终止同步」：当前请求结束后不再继续后面的作者
    if (!state.syncTask || state.syncTask.cancelling) { stopped = true; break; }
    const row = state.syncTask.authors.find((item) => item.authorId === author.id);
    state.syncTask.authorId = author.id;
    state.syncTask.label = `同步 ${index + 1} / ${targets.length}：${author.name}`;
    if (row) { row.status = "running"; row.title = ""; row.current = 0; row.total = 0; }
    updateSyncFloater();
    try {
      const result = await invoke("sync_pixiv_novels", { authorId: author.id, startDate: "", endDate: "", novelUrl: "" });
      downloaded += result.downloadedCount || 0;
      if (row) {
        row.status = "done";
        row.note = result.cancelled
          ? "已终止"
          : `完成 · 下载 ${result.downloadedCount || 0} 篇${result.failedCount ? ` · ${result.failedCount} 篇失败` : ""}`;
      }
      if (result.cancelled) { stopped = true; updateSyncFloater(); break; }
      doneAuthors += 1;
      if (result.failedCount) failures.push(`${author.name}（${result.failedCount} 篇失败）`);
    } catch (error) {
      doneAuthors += 1;
      if (row) { row.status = "failed"; row.note = String(error); }
      failures.push(`${author.name}（${error}）`);
    }
    updateSyncFloater();
  }

  unlisten();
  state.syncTask = null;
  await refreshAuthors();
  if (state.activeAuthor) {
    state.activeAuthor = state.authors.find((author) => author.id === state.activeAuthor.id) || state.activeAuthor;
    await refreshWorks();
  }
  render();

  const head = stopped ? `批量同步已终止（完成 ${doneAuthors} / ${targets.length} 位作者）` : `已同步 ${doneAuthors} / ${targets.length} 位作者，共下载 ${downloaded} 篇`;
  if (failures.length) {
    const detail = failures.slice(0, 3).join("；");
    toast(`${head}；${failures.length} 位有失败：${detail}${failures.length > 3 ? " 等" : ""}`, "info");
  } else {
    toast(stopped ? `${head}，共下载 ${downloaded} 篇` : head, stopped ? "info" : "success");
  }
}

async function cancelPixivSync() {
  if (!state.syncTask || state.syncTask.cancelling) return;
  state.syncTask.cancelling = true;
  state.syncTask.label = "正在终止同步";
  updateSyncFloater();
  // 批量同步时可能还没轮到任何作者，此时靠 cancelling 标记停止后续循环
  if (state.syncTask.authorId) await invoke("cancel_pixiv_sync", { authorId: state.syncTask.authorId });
}

// 作者别名：就地增删（不像作品标签那样关掉重开弹窗，避免丢掉表单里其它未保存的修改）
function aliasChips(aliases) {
  return String(aliases || "").split("|").map((alias) => alias.trim()).filter(Boolean)
    .map((alias, index) => `<span>${escapeHtml(alias)}<button type="button" title="删除别名" data-action="remove-author-alias" data-index="${index}">×</button></span>`)
    .join("");
}

function authorAliases() {
  return [...document.querySelectorAll("[data-author-alias] .tag-editor-list > span")].map((item) => item.firstChild.textContent.trim()).filter(Boolean);
}

function addAuthorAlias() {
  const input = document.querySelector("#alias-editor-input");
  const value = input?.value.trim();
  if (!input || !value) return;
  input.value = "";
  const aliases = authorAliases();
  if (aliases.some((alias) => alias.toLowerCase() === value.toLowerCase())) return;
  document.querySelector("[data-author-alias] .tag-editor-list")?.insertAdjacentHTML("beforeend", aliasChips(value));
  input.focus();
}

function removeAuthorAlias(index) {
  const chips = [...document.querySelectorAll("[data-author-alias] .tag-editor-list > span")];
  chips[index]?.remove();
}

function editingTags() { return [...document.querySelectorAll(".tag-editor .tag-editor-list > span")].map((item) => item.firstChild.textContent.trim()); }
function addEditingTag() { const input = document.querySelector("#tag-editor-input"); const value = input?.value.trim(); if (!value) return; const workId = Number(document.querySelector(".tag-editor")?.dataset.workId); const tags = editingTags(); input.value = ""; closeModal(); editTagsModal(workId, [...tags, value]); }
function removeEditingTag(index) { const workId = Number(document.querySelector(".tag-editor")?.dataset.workId); const tags = editingTags(); tags.splice(index, 1); closeModal(); editTagsModal(workId, tags); }
async function saveTags(workId) { await invoke("update_work_tags", { workId, tags: editingTags() }); closeModal(); await refreshWorks(); render(); }

async function importPixivCookie() {
  const path = await open({ multiple: false, directory: false, filters: [{ name: "Cookie", extensions: ["json", "txt"] }] });
  if (!path) return;
  document.querySelector('[name="pixivCookie"]').value = await invoke("read_pixiv_cookie_file", { path });
}

/** 设置表单 → 提交给后端的对象；校验不过就抛错（自动保存时把错误写到页脚，不弹 toast 打断输入） */
function collectSettings(form) {
  const values = Object.fromEntries(new FormData(form).entries());
  values.autoCreateDirs = Boolean(form.querySelector('[name="autoCreateDirs"]')?.checked);
  const unitBytes = { KB: 1024, MB: 1024 ** 2, GB: 1024 ** 3 };
  const minimumFileSize = Number(values.minimumFileSize || 0);
  const minimumFileSizeUnit = values.minimumFileSizeUnit;
  if (!Number.isFinite(minimumFileSize) || minimumFileSize < 0 || !(minimumFileSizeUnit in unitBytes)) throw new Error("请输入有效的最小文件大小");
  values.minimumFileSizeBytes = Math.ceil(minimumFileSize * unitBytes[minimumFileSizeUnit]);
  if (!Number.isSafeInteger(values.minimumFileSizeBytes)) throw new Error("最小文件大小过大");
  values.pixivDelayThreshold = Number(values.pixivDelayThreshold || 150);
  values.pixivDelaySeconds = Number(values.pixivDelaySeconds ?? 1);
  if (!Number.isInteger(values.pixivDelayThreshold) || values.pixivDelayThreshold < 1) throw new Error("抓取数量阈值至少为 1");
  if (!Number.isInteger(values.pixivDelaySeconds) || values.pixivDelaySeconds < 0 || values.pixivDelaySeconds > 60) throw new Error("抓取间隔请输入 0 到 60 的整数秒数");
  values.similarityThreshold = Number(values.similarityThreshold || 70);
  values.minSimilarityThreshold = Number(values.minSimilarityThreshold || 30);
  values.matchTitleLength = Number(values.matchTitleLength || 0);
  if (!Number.isInteger(values.matchTitleLength) || values.matchTitleLength < 0) throw new Error("匹配作品名长度请输入 0 或正整数（0 表示匹配完整作品名）");
  values.imageQuality = values.imageQuality === "original" ? "original" : "1200";
  values.syncImageFormat = values.syncImageFormat === "epub" ? "epub" : "html";
  // 搜索网站：编辑器那两列是同一个控件的两半，收成数组后把原始字段摘掉，别夹带进设置
  values.searchSites = collectSearchSites(form);
  delete values.searchSiteName;
  delete values.searchSiteUrl;
  // 自动更新：勾选框 + 一行一个的镜像列表
  values.autoCheckUpdate = Boolean(form.querySelector('[name="autoCheckUpdate"]')?.checked);
  values.updateMirrors = String(values.updateMirrors || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  // 浏览历史：只是勾选框，FormData 拿不到未勾选的状态
  values.recordHistory = Boolean(form.querySelector('[name="recordHistory"]')?.checked);
  return values;
}

/** 把设置写进数据库，成功后刷新右键菜单用的缓存 */
async function persistSettings(form) {
  const values = collectSettings(form);
  await invoke("save_app_settings", { settings: values });
  // 右键菜单靠这份缓存，保存完立刻生效，不用重启
  state.searchSites = values.searchSites;
  return values;
}

/** 页脚那行状态：告诉用户「改动自己存了」，省得他到处找保存按钮 */
function setSettingsSaveState(text, kind = "idle") {
  const node = document.querySelector("#settings-save-state");
  if (!node) return;
  node.textContent = text;
  node.className = `settings-save-state is-${kind}`;
}

/** 改动后防抖保存：边打字边写库太吵，停手半秒再落盘 */
function scheduleSettingsSave(form, delay = 500) {
  if (!form) return;
  window.clearTimeout(state.settingsSaveTimer);
  setSettingsSaveState("改动待保存…", "busy");
  state.settingsSaveTimer = window.setTimeout(() => flushSettingsSave(form), delay);
}

/** 立刻落盘（防抖到期，或弹窗要关了）。失败只写页脚不弹 toast —— 输入中途的半成品不该打断人 */
async function flushSettingsSave(form) {
  window.clearTimeout(state.settingsSaveTimer);
  state.settingsSaveTimer = 0;
  if (!form || !form.isConnected) return;
  try {
    const values = await persistSettings(form);
    const time = new Date().toTimeString().slice(0, 8);
    setSettingsSaveState(`已自动保存 ${time}${values.searchSites.length ? `（${values.searchSites.length} 个搜索网站）` : ""}`, "done");
  } catch (error) {
    setSettingsSaveState(`没保存：${String(error)}`, "error");
  }
}

function minimumSizeParts(bytes) {
  const value = Number(bytes || 0);
  const units = [["GB", 1024 ** 3], ["MB", 1024 ** 2], ["KB", 1024]];
  const matched = units.find(([, size]) => value > 0 && value % size === 0) || ["KB", 1024];
  return { value: value / matched[1], unit: matched[0] };
}

async function settingsModal() {
  const settings = await invoke("get_app_settings");
  const minimumSize = minimumSizeParts(settings.minimumFileSizeBytes);
  showModal(modal("设置（自动保存）", `<div class="settings-copy"><form id="settings-form" class="settings-form">
  <section class="settings-group is-account">
    <h3 class="settings-group-head"><span class="settings-group-no">1</span>Pixiv 账号</h3>
    <div class="settings-fields is-single">
      <label>Pixiv Cookie <textarea name="pixivCookie" rows="3" placeholder="直接粘贴 Cookie，或从文件导入">${escapeHtml(settings.pixivCookie || "")}</textarea><button type="button" class="quiet-button" data-action="pick-pixiv-cookie">从文件导入</button><small>仅保存 Pixiv 接口需要的 PHPSESSID 到数据库。</small></label>
      <label>排除标签 <input name="excludedTags" value="${escapeHtml(settings.excludedTags || "")}" placeholder="标签A, 标签B"><small>用中英文逗号分隔。包含这些文字的 Pixiv 标签不会记录。</small></label>
      <div class="settings-slot" id="settings-slot-pixiv"></div>
    </div>
  </section>
  <section class="settings-group is-storage">
    <h3 class="settings-group-head"><span class="settings-group-no">2</span>文件与目录</h3>
    <div class="settings-fields is-single">
      <label>最小文件大小 <div class="size-input"><input name="minimumFileSize" type="number" min="0" step="0.1" value="${minimumSize.value}"><select name="minimumFileSizeUnit" aria-label="最小文件大小单位">${["KB", "MB", "GB"].map((unit) => `<option value="${unit}" ${minimumSize.unit === unit ? "selected" : ""}>${unit}</option>`).join("")}</select></div><small>导入文件夹和 Pixiv 同步时，会跳过小于此大小的文本内容；文件夹不受此限制。</small></label>
      <label>预览版文件夹默认目录 <div class="path-input"><input name="defaultPreviewDir" value="${escapeHtml(settings.defaultPreviewDir || "")}" readonly><button type="button" class="quiet-button" data-action="pick-default-preview">选择目录</button></div></label>
      <label>完整版文件夹默认目录 <div class="path-input"><input name="defaultPurchasedDir" value="${escapeHtml(settings.defaultPurchasedDir || "")}" readonly><button type="button" class="quiet-button" data-action="pick-default-purchased">选择目录</button></div></label>
      <label>完整版自动分组文件夹 <div class="path-input"><input name="autoGroupDir" value="${escapeHtml(settings.autoGroupDir || "")}" readonly><button type="button" class="quiet-button" data-action="pick-auto-group-dir">选择目录</button></div><small>将此文件夹（含其子文件夹）中的文件自动移动到对应作者的完整版文件夹中。同一本书匹配到多个作品时会列入手动选择，勾选几个作品就复制几份。</small></label>
      <label class="check-row"><input name="autoCreateDirs" type="checkbox" ${settings.autoCreateDirs ? "checked" : ""}><span>自动创建作者目录</span><small>新建或保存路径为空的作者时，在默认目录中创建以作者名称命名的文件夹。</small></label>
    </div>
  </section>
  <section class="settings-group is-match">
    <h3 class="settings-group-head"><span class="settings-group-no">3</span>匹配与关联</h3>
    <p class="settings-note">决定文件名怎么和作品对上号。「匹配标题」用相似度，「匹配角色」用下面的角色名列表。</p>
    <div class="settings-fields">
      <label>关联相似度阈值 <div class="threshold-input"><input name="similarityThreshold" type="number" min="0" max="100" value="${settings.similarityThreshold || 70}"><span>%</span></div><small>文件名与作品标题的相似度达到此值时自动关联。建议值：70-85。</small></label>
      <label>最小相似度阈值 <div class="threshold-input"><input name="minSimilarityThreshold" type="number" min="0" max="100" value="${settings.minSimilarityThreshold || 30}"><span>%</span></div><small>低于此值的匹配结果将被忽略。建议值：20-40。</small></label>
      <label>匹配作品名长度 <div class="threshold-input"><input name="matchTitleLength" type="number" min="0" step="1" value="${settings.matchTitleLength || 0}"><span>字</span></div><small>参与匹配时只取作品名的前 N 个字，超长作品名截取后再与文件名比对，能提高匹配率。填 0 或不填表示用完整作品名匹配。</small></label>
    </div>
    <div class="settings-fields is-single">
      <div class="form-field char-setting">
        <div class="char-setting-head"><span class="field-title">常见角色名列表</span><span class="settings-group-hint">「匹配角色」的依据</span></div>
        <small>作品标题和文件名里出现这些角色名就算候选。按游戏分组，游戏名只负责归类、不参与匹配。内置 8 款热门游戏的热门角色，可以自己增删；点角色名可以临时停用（划掉的那个不参与匹配），点 × 删除。</small>
        <div id="character-editor" class="char-editor"></div><div class="char-add-row"><input id="character-add-game" list="character-game-list" placeholder="游戏名" autocomplete="off"><datalist id="character-game-list"></datalist><input id="character-add-name" placeholder="角色名" autocomplete="off"><input id="character-add-aliases" placeholder="别名，用 | 分隔（可留空）" autocomplete="off"><button type="button" class="quiet-button" data-action="add-character">添加角色</button></div><div class="char-add-row"><input id="character-add-new-game" placeholder="新建一个空的游戏分组" autocomplete="off"><button type="button" class="quiet-button" data-action="add-character-game">新建分组</button></div>
      </div>
    </div>
  </section>
  <section class="settings-group is-download">
    <h3 class="settings-group-head"><span class="settings-group-no">4</span>下载与图文</h3>
    <div class="settings-fields is-single">
      <label>配图画质 <select name="imageQuality"><option value="1200" ${(settings.imageQuality || "1200") === "1200" ? "selected" : ""}>1200 宽（推荐，每张约 1 MB）</option><option value="original" ${settings.imageQuality === "original" ? "selected" : ""}>原始画质（作者上传的原图，可能每张几 MB）</option></select><small>图文小说同步时下载正文配图用这一档。原图更清晰，但一篇的配图可能要几十到上百 MB。</small></label>
      <label>同步作品时图文小说自动保存为 <select name="syncImageFormat"><option value="html" ${(settings.syncImageFormat || "html") === "html" ? "selected" : ""}>HTML（浏览器直接打开）</option><option value="epub" ${settings.syncImageFormat === "epub" ? "selected" : ""}>EPUB（电子书阅读器）</option></select><small>同步图文小说时只保留这一种阅读版：HTML 双击就能用浏览器看，EPUB 适合推到手机 / 阅读器。不会两种都存。</small></label>
    </div>
  </section>
  <section class="settings-group is-search">
    <h3 class="settings-group-head"><span class="settings-group-no">5</span>搜索网站</h3>
    <p class="settings-note">在作品卡上选中标题文字后右键，菜单里会多出这些网站的搜索入口，点一下就用系统浏览器打开它的搜索页。不填就没有这些入口。</p>
    <div class="settings-fields is-single">
      <div class="form-field search-site-setting">
        <div class="char-setting-head"><span class="field-title">网站列表</span><span class="settings-group-hint">右键菜单的搜索入口</span></div>
        <small>填网站名和它的搜索页网址。网址里<b>最后一个 <code>=</code> 后面就是搜索词的位置</b>，保存时会清掉那后面的内容——比如把 <code>https://xxx.com/search.php?kw=图</code> 填进来，之后搜索时就把「图」换成你选中的文字。可以填多个，右键菜单按这里的顺序列出。</small>
        <div id="search-site-editor" class="search-site-editor"></div>
        <div class="search-site-tools"><button type="button" class="quiet-button" data-action="add-search-site">添加网站</button></div>
      </div>
    </div>
  </section>
  <section class="settings-group is-maintain">
    <h3 class="settings-group-head"><span class="settings-group-no">6</span>维护与数据</h3>
    <div class="settings-fields is-single">
      <div class="form-field update-setting">
        <div class="char-setting-head"><span class="field-title">软件更新</span><span class="settings-group-hint">当前 v${APP_VERSION}</span></div>
        <small>查到新版本会把新版 exe 下载到程序所在的文件夹，下完自动重启到新版本；旧的那个文件在下次启动时移入回收站（可以还原）。你的作品文件和数据库都在旁边，不受影响。GitHub 直连不上时会自动换下面的加速镜像重试。</small>
        <div class="settings-button-row"><button type="button" class="quiet-button" data-action="check-update">检查更新</button><button type="button" class="quiet-button" data-action="open-release-page">打开发布页</button></div>
        <label class="check-row"><input name="autoCheckUpdate" type="checkbox" ${settings.autoCheckUpdate === false ? "" : "checked"}><span>启动时自动检查新版本</span><small>关掉之后程序不会主动联网查版本，想更新时点上面的「检查更新」。</small></label>
        <label>更新加速镜像 <textarea name="updateMirrors" rows="4" placeholder="https://ghproxy.net/">${escapeHtml((settings.updateMirrors || []).join("\n"))}</textarea><button type="button" class="quiet-button" data-action="reset-update-mirrors">恢复默认</button><small>一行一个。直连 GitHub 失败后按这里的顺序重试（地址会直接接在 <code>https://github.com/...</code> 前面）。镜像失效时自己换一个即可，不用等新版。</small></label>
      </div>
      <div class="form-field update-setting">
        <div class="char-setting-head"><span class="field-title">浏览历史</span><span class="settings-group-hint">最多 500 条</span></div>
        <small>打开作品或阅读版时自动记一笔，按时间倒序排在左侧栏的「浏览历史」里。</small>
        <label class="check-row"><input name="recordHistory" type="checkbox" ${settings.recordHistory === false ? "" : "checked"}><span>记录浏览历史</span><small>关掉之后不再新增记录，已有的记录留着，随时可以手动清空。</small></label>
        <div class="settings-button-row"><button type="button" class="quiet-button" data-action="clear-history-settings">清空浏览历史</button></div>
      </div>
    </div>
    <p class="settings-note">下面这些操作点下去立刻执行，跟自动保存无关。</p>
    <div class="settings-fields is-single">
      <div class="settings-slot" id="settings-slot-maintain"></div>
    </div>
    <div class="menu-list settings-actions">
      <button type="button" data-action="clean-preview-versions">${icon("file", 18)}清理多余预览版<span class="settings-action-hint">已经有完整版的作品，预览版就不必留了；先给你看数量再动手</span></button>
      <button type="button" data-action="export-backup">${icon("database", 18)}导出数据库备份<span class="settings-action-hint">保存一份数据库文件，出问题时可回滚</span></button>
      <button type="button" data-action="restore-backup">${icon("upload", 18)}从备份恢复<span class="settings-action-hint">用备份文件覆盖当前数据库，需重启</span></button>
    </div>
  </section>
</form>`, `<span id="settings-save-state" class="settings-save-state is-idle">改动会自动保存</span><span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">关闭</button>`, "is-wide"));
  bindCharacterEditorInputs();
  await renderCharacterEditor();
  renderSearchSiteEditor(settings.searchSites || []);
}

/** 搜索网站编辑器：一项一个网站（输入行 + 清洗后出现的说明），就地增删（重开弹窗会丢掉其它未保存的设置） */
function searchSiteRow(site = {}) {
  return `<div class="search-site-item">
    <div class="search-site-row">
      <input class="search-site-name" name="searchSiteName" value="${escapeHtml(site.name || "")}" placeholder="网站名，例如 书香" autocomplete="off">
      <input class="search-site-url" name="searchSiteUrl" value="${escapeHtml(site.url || "")}" placeholder="搜索页网址，搜索词位置留在最后一个 = 之后" autocomplete="off">
      <button type="button" class="icon-button search-site-remove" data-action="remove-search-site" title="删除这个网站">×</button>
    </div>
  </div>`;
}

/**
 * 常见的关键词参数名：Discuz 的 `srchtxt`（搜索表单字段）与 `kw`（结果页的高亮词），
 * 其它站常用 `q` / `wd` / `query` 之类。清洗时认出来，统一挪到网址最后一位。
 */
const SEARCH_KEYWORD_KEYS = ["srchtxt", "kw", "q", "wd", "word", "query", "keyword", "search", "text", "key"];

/**
 * 网址自动清洗（用户粘什么进来都往「能直接换词的搜索地址」上收，省得他自己改）：
 * 1. 扔掉 `searchid` —— 那是那一次搜索的结果缓存编号，留着换关键词结果也不会变，缓存过期还会失效；
 * 2. 关键词参数（Discuz 用 `srchtxt`）挪到最末一位，程序就按「最后一个 `=` 之后」的位置填搜索词；
 * 3. 其余参数**原样保留、原顺序**，像 `orderby=dateline&ascdesc=desc` 这种排序设置一个都不动，
 *    板块参数 `mod=forum` 之类也照旧；Discuz 缺 `searchsubmit=yes` 就补上（不补它不会真去搜）。
 * 认不出关键词位置（既不是结果页、也没有关键词参数）就退回老规矩：清掉最后一个 `=` 之后的内容。
 * 不像网址（不是 http/https 开头）原样返回，交给 collectSearchSites 去报错。
 */
function cleanSearchUrl(raw) {
  const text = String(raw || "").trim();
  if (!text || !/^https?:\/\//i.test(text)) return text;
  const hashAt = text.indexOf("#");
  const hash = hashAt >= 0 ? text.slice(hashAt) : "";
  const head = hashAt >= 0 ? text.slice(0, hashAt) : text;
  const markAt = head.indexOf("?");
  const path = markAt >= 0 ? head.slice(0, markAt) : head;
  const pieces = (markAt >= 0 ? head.slice(markAt + 1) : "").split("&").filter((piece) => piece.length);
  const isDiscuz = /search\.php$/i.test(path) || pieces.some((piece) => /^searchid=/i.test(piece));
  const kept = [];
  let keywordKey = "";
  let droppedSearchId = false;
  pieces.forEach((piece) => {
    const eq = piece.indexOf("=");
    const key = (eq >= 0 ? piece.slice(0, eq) : piece).trim().toLowerCase();
    if (key === "searchid") { droppedSearchId = true; return; }
    if (SEARCH_KEYWORD_KEYS.includes(key)) { keywordKey = key; return; }
    kept.push(piece);
  });
  if (!droppedSearchId && !keywordKey) {
    const index = text.lastIndexOf("=");
    return index >= 0 ? text.slice(0, index + 1) : text;
  }
  // Discuz 的搜索表单字段是 srchtxt（结果页那个 kw 只用来高亮，拿它搜不出结果）
  if (isDiscuz) keywordKey = "srchtxt";
  if (!keywordKey) keywordKey = "q";
  if (isDiscuz && !kept.some((piece) => /^searchsubmit=/i.test(piece))) kept.push("searchsubmit=yes");
  return `${path}?${kept.concat(`${keywordKey}=`).join("&")}${hash}`;
}

/** 清洗后在那一行下面留一句话：改动是程序做的，得让人看见改了什么、为什么 */
function searchUrlNote(before, after) {
  const text = /[?&]searchid=/i.test(String(before || ""))
    ? `已自动清掉 <code>searchid</code>（那一次搜索的缓存编号，换关键词结果也不会变，缓存过期还会失效），${keywordKeptText(after)}`
    : `已自动把关键词位置挪到最后一个 <code>=</code> 之后，${keywordKeptText(after)}`;
  return `<p class="search-site-hint">${text}</p>`;
}

/** 「排序、板块这些参数都留着」这句 —— 按清洗后的网址里到底还剩什么说，别空口保证 */
function keywordKeptText(url) {
  const query = String(url || "").split("?")[1] || "";
  const others = query.split("&").filter((piece) => piece && !SEARCH_KEYWORD_KEYS.some((key) => new RegExp(`^${key}=`, "i").test(piece)));
  return others.length ? "排序、板块这些参数都保留着。" : "";
}

/**
 * 把某一行网址的输入框就地清洗掉（不重渲染整行，否则光标会丢）。
 * 输入框里的值被程序改了，必须写回 DOM —— 存库读的就是 DOM 里的值。
 */
function cleanSearchUrlInput(input) {
  const item = input?.closest?.(".search-site-item");
  if (!item) return "";
  const before = input.value.trim();
  const after = cleanSearchUrl(before);
  if (!after || after === before) return after;
  input.value = after;
  item.querySelectorAll(".search-site-hint").forEach((node) => node.remove());
  item.insertAdjacentHTML("beforeend", searchUrlNote(before, after));
  return after;
}

function renderSearchSiteEditor(sites) {
  const box = document.querySelector("#search-site-editor");
  if (!box) return;
  // 老版本存下来的网址可能还带着 searchid：渲染时就清洗掉，并记下原始值好给一句说明
  const rows = sites.map((site) => {
    const url = cleanSearchUrl(site.url);
    return { name: site.name, url, before: url === site.url ? "" : site.url };
  });
  box.innerHTML = rows.length
    ? rows.map((site) => searchSiteRow(site)).join("")
    : `<p class="search-site-empty">还没配搜索网站，点下面的「添加网站」加一个。</p>`;
  const items = box.querySelectorAll(".search-site-item");
  rows.forEach((site, index) => {
    if (site.before) items[index]?.insertAdjacentHTML("beforeend", searchUrlNote(site.before, site.url));
  });
  // 行是刚造出来的，删除按钮还没挂事件（[data-action] 是逐个绑的），重新走一遍
  bindEvents();
  // 真改过值就静默落盘：不清掉 searchid，用户点搜索等于白点（换什么词结果都一样）
  if (rows.some((site) => site.before)) scheduleSettingsSave(document.querySelector("#settings-form"), 0);
}

function addSearchSiteRow() {
  const box = document.querySelector("#search-site-editor");
  if (!box) return;
  box.querySelector(".search-site-empty")?.remove();
  box.insertAdjacentHTML("beforeend", searchSiteRow());
  bindEvents();
  box.lastElementChild?.querySelector(".search-site-name")?.focus();
}

/** 提交前把编辑器里的网站收成数组：顺手清洗网址，空行忽略，填了一半的直接报错，别默默丢掉 */
function collectSearchSites(form) {
  const data = new FormData(form);
  const names = data.getAll("searchSiteName").map((value) => String(value));
  const urls = data.getAll("searchSiteUrl").map((value) => String(value));
  const sites = [];
  names.forEach((rawName, index) => {
    const name = rawName.trim();
    const url = cleanSearchUrl(urls[index] || "");
    if (!name && !url) return;
    if (!name) throw new Error(`第 ${index + 1} 个搜索网站还没填网站名`);
    if (!url) throw new Error(`搜索网站「${name}」还没填搜索页网址`);
    if (!/^https?:\/\//i.test(url)) throw new Error(`搜索网站「${name}」的网址需要以 http:// 或 https:// 开头`);
    if (!url.includes("=")) throw new Error(`搜索网站「${name}」的网址里要带上一个 = ，它后面就是搜索词的位置`);
    sites.push({ name, url });
  });
  return sites;
}

/** 用某个搜索网站搜选中的文字：网址由后端按同一套规则拼好并打开系统浏览器 */
async function runSearchSite(url, keyword) {
  const text = String(keyword || "").trim();
  if (!text) { toast("没有可搜索的文字", "info"); return; }
  const target = await invoke("open_search_site", { url, keyword: text });
  closeModal();
  toast(`已在浏览器打开搜索页：${target}`, "success");
}

/** 右键菜单里点「去添加」：直接开设置面板、滚到「搜索网站」那一块，光标落在输入框上 */
async function openSearchSettings() {
  closeModal();
  await settingsModal();
  const box = document.querySelector("#search-site-editor");
  if (!box) return;
  box.closest(".settings-group")?.scrollIntoView({ block: "start", behavior: "smooth" });
  if (box.querySelector(".search-site-row")) box.querySelector(".search-site-name")?.focus();
  else addSearchSiteRow();
}

/** 常见角色名列表编辑器：就地更新 DOM，绝不动设置弹窗本身（重开会丢未保存的设置） */
async function renderCharacterEditor() {
  const box = document.querySelector("#character-editor");
  if (!box) return;
  let groups = [];
  try {
    groups = await invoke("list_characters");
  } catch (error) {
    box.innerHTML = `<p class="read-only-note">角色列表读取失败：${escapeHtml(String(error))}</p>`;
    return;
  }
  state.characters = groups;
  const total = groups.reduce((sum, group) => sum + group.characters.length, 0);
  const gameList = document.querySelector("#character-game-list");
  if (gameList) gameList.innerHTML = groups.map((group) => `<option value="${escapeHtml(group.game)}"></option>`).join("");
  if (!groups.length) {
    box.innerHTML = `<p class="read-only-note">还没有角色名，先在下面新建一个游戏分组。</p>`;
    return;
  }
  box.dataset.total = String(total);
  box.innerHTML = groups.map((group, index) => {
    const enabledCount = group.characters.filter((entry) => entry.enabled).length;
    const chips = group.characters.map((entry) => {
      // 有别名时把别名收进 title，不额外占位；停用的划掉但仍占位，方便随时恢复
      const title = entry.aliases ? `别名：${entry.aliases}` : entry.name;
      return `<span class="char-chip ${entry.enabled ? "" : "is-off"}" data-action="toggle-character" data-id="${entry.id}" title="${escapeHtml(title)}（点击${entry.enabled ? "停用" : "启用"}）">${escapeHtml(entry.name)}<button type="button" class="char-chip-remove" data-action="delete-character" data-id="${entry.id}" title="删除这个角色">×</button></span>`;
    }).join("");
    const empty = group.characters.length ? "" : `<span class="char-empty">还没有角色，用上面的"添加角色"往里加</span>`;
    // 第一个分组默认展开，让人一眼看到这堆是能点、能删的角色名
    return `<details class="char-group" ${index === 0 ? "open" : ""}>
      <summary><strong>${escapeHtml(group.game)}</strong><span class="char-count">${enabledCount}/${group.characters.length}</span></summary>
      <div class="char-chips">${chips}${empty}</div>
      <div class="char-group-tools">
        <input class="char-game-name" value="${escapeHtml(group.game)}" aria-label="分组名">
        <button type="button" class="quiet-button" data-action="rename-character-game" data-game="${escapeHtml(group.game)}">重命名分组</button>
        <button type="button" class="quiet-button danger-text-button" data-action="delete-character-game" data-game="${escapeHtml(group.game)}">删除整个分组</button>
      </div>
    </details>`;
  }).join("");
  // 新生成的 chip / 分组按钮需要重新挂事件（元素是新造的，dataset.bound 会重新生效）
  bindEvents();
}
function bindCharacterEditorInputs() {
  const bind = (selector, run) => {
    const input = document.querySelector(selector);
    if (!input) return;
    input.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      run();
    });
  };
  bind("#character-add-game", addCharacter);
  bind("#character-add-name", addCharacter);
  bind("#character-add-aliases", addCharacter);
  bind("#character-add-new-game", addCharacterGame);
}

async function addCharacter() {
  const gameInput = document.querySelector("#character-add-game");
  const nameInput = document.querySelector("#character-add-name");
  const aliasInput = document.querySelector("#character-add-aliases");
  if (!gameInput || !nameInput) return;
  const game = gameInput.value.trim();
  const name = nameInput.value.trim();
  const aliases = aliasInput ? aliasInput.value.trim() : "";
  if (!game) { toast("请先填游戏名（可以从下拉里选已有的分组）", "info"); gameInput.focus(); return; }
  if (!name) { toast("请填角色名", "info"); nameInput.focus(); return; }
  try {
    await invoke("add_character", { game, name, aliases });
    nameInput.value = ""; if (aliasInput) aliasInput.value = "";
    await renderCharacterEditor();
    nameInput.focus();
    toast(`已添加「${name}」到「${game}」`, "success");
  } catch (error) {
    toast(`添加失败：${error}`, "error");
  }
}

async function addCharacterGame() {
  const input = document.querySelector("#character-add-new-game");
  if (!input) return;
  const game = input.value.trim();
  if (!game) { toast("请填游戏名", "info"); input.focus(); return; }
  try {
    await invoke("add_character_game", { game });
    input.value = "";
    await renderCharacterEditor();
    toast(`已新建分组「${game}」`, "success");
  } catch (error) {
    toast(`新建分组失败：${error}`, "error");
  }
}

async function toggleCharacter(id) {
  const entry = (state.characters || []).flatMap((group) => group.characters).find((item) => item.id === id);
  if (!entry) return;
  try {
    await invoke("update_character", { id, name: entry.name, aliases: entry.aliases, enabled: !entry.enabled });
    await renderCharacterEditor();
  } catch (error) {
    toast(`修改失败：${error}`, "error");
  }
}

async function deleteCharacter(id) {
  const entry = (state.characters || []).flatMap((group) => group.characters).find((item) => item.id === id);
  try {
    await invoke("delete_character", { id });
    await renderCharacterEditor();
    toast(`已删除「${entry ? entry.name : "该角色"}」`, "success");
  } catch (error) {
    toast(`删除失败：${error}`, "error");
  }
}

function deleteCharacterGame(game) {
  confirmAction("确认删除分组", `会删掉「${game}」分组里的所有角色名，作品本身不受影响。`, "删除分组", async () => {
    try {
      await invoke("delete_character_game", { game });
      await renderCharacterEditor();
      toast(`已删除分组「${game}」`, "success");
    } catch (error) {
      toast(`删除失败：${error}`, "error");
    }
  }, true);
}

async function renameCharacterGame(game, button) {
  const row = button.closest(".char-group-tools");
  const input = row ? row.querySelector(".char-game-name") : null;
  if (!input) return;
  const next = input.value.trim();
  if (!next) { toast("分组名不能为空", "info"); return; }
  if (next === game) return;
  try {
    await invoke("rename_character_game", { game, next });
    await renderCharacterEditor();
    toast(`分组「${game}」已改名为「${next}」`, "success");
  } catch (error) {
    toast(`重命名失败：${error}`, "error");
    input.value = game;
  }
}

/* ============================ 软件自动更新（v1.0.0） ============================
 * 便携版没有安装器，所以「更新」就是：问发布页要最新版本号 → 把新 exe 下到程序旁边
 * → 启动它 → 旧版本由新版本在启动时送进回收站。
 * 版本探测、镜像回退、下载、文件校验全在 Rust 侧，这里只负责把状态摆给用户看。
 */

/** 点了「忽略此版本」记在这里：同一个版本不再提示，出了新的照旧提示 */
const UPDATE_IGNORED_KEY = "pixiv_update_ignored_version";
/** 发布页与最常用的一条镜像：连 Rust 侧还没探测出结果时也能点「打开发布页」 */
const RELEASE_PAGE_URL = "https://github.com/fromzero1501/pixiv-novel-downloader/releases";
const DEFAULT_UPDATE_MIRROR = "https://ghproxy.net/";

function ignoredUpdateVersion() {
  try { return localStorage.getItem(UPDATE_IGNORED_KEY) || ""; } catch { return ""; }
}

function ignoreUpdateVersion(version) {
  // 隐私模式下 localStorage 会抛，忽略就好
  try { localStorage.setItem(UPDATE_IGNORED_KEY, version); } catch { /* 忽略 */ }
}

function hasUpdateReady() {
  return Boolean(state.update?.hasUpdate);
}

/** 手动点「检查更新」和启动时的静默检查共用；失败返回 null（静默模式下不打扰用户） */
async function checkForUpdate({ silent = false } = {}) {
  try {
    const result = await invoke("check_for_update");
    state.update = result;
    render();
    if (!result.hasUpdate) {
      state.updateNotes = null;
      if (!silent) toast(`已经是最新版（v${result.currentVersion}）`, "success");
      return result;
    }
    // 有新版才顺带去取更新说明（异步，不挡提示）；被忽略的版本也取，
    // 因为左下角角标还在，用户随时可能点开看
    void loadUpdateNotes(result.latestVersion);
    if (silent && ignoredUpdateVersion() === result.latestVersion) return result;
    toast(`发现新版本 v${result.latestVersion}，点左下角版本号查看`, "success");
    return result;
  } catch (error) {
    if (!silent) toast(`检查更新失败：${String(error)}`, "error");
    return null;
  }
}

/** 左侧栏底部的版本号：有新版时变成一个高亮按钮，平时就是一行灰字 */
function appVersionBadge() {
  if (hasUpdateReady()) {
    return `<button class="app-version is-update-ready" title="有新版本 v${escapeHtml(state.update.latestVersion)}，点这里更新" data-action="open-update">↑ v${escapeHtml(state.update.latestVersion)}</button>`;
  }
  return `<span class="app-version" title="当前版本">v${APP_VERSION}</span>`;
}

function humanSize(bytes) {
  if (!bytes) return "大小未知";
  const megabytes = bytes / 1024 / 1024;
  return megabytes >= 1 ? `${megabytes.toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function updateProgressPercent() {
  const progress = state.updateProgress;
  if (!progress?.total) return 0;
  return Math.min(100, Math.round((progress.received / progress.total) * 100));
}

/**
 * 更新说明只在「确实查到新版」时才去取一次，取回后填进更新弹窗，
 * 用户不用为了看更新内容再开一次浏览器。
 */
async function loadUpdateNotes(version) {
  if (!version) return;
  const cached = state.updateNotes;
  if (cached?.version === version && cached.state === "ready") return;
  if (cached?.version === version && cached.state === "loading") return;
  state.updateNotes = { version, state: "loading" };
  refreshUpdateNotesDom();
  try {
    const result = await invoke("fetch_release_notes", { version });
    // 期间又检查出别的版本了就别覆盖
    if (state.updateNotes?.version !== version) return;
    state.updateNotes = {
      version,
      state: "ready",
      title: result?.title || "",
      notes: result?.notes || "",
      source: result?.source || "",
    };
  } catch (error) {
    if (state.updateNotes?.version !== version) return;
    state.updateNotes = { version, state: "error", error: String(error) };
  }
  refreshUpdateNotesDom();
}

/** 更新说明那一块。取不到就老实说取不到，别留个空白框 */
function updateNotesSection() {
  const info = state.updateNotes;
  const latest = state.update?.latestVersion;
  if (!info || info.version !== latest) {
    return `<div class="update-notes is-loading">正在获取更新说明…</div>`;
  }
  if (info.state === "loading") {
    return `<div class="update-notes is-loading">正在获取更新说明…</div>`;
  }
  if (info.state === "ready") {
    return `<div class="update-notes">
        <p class="update-notes-title">${escapeHtml(info.title || `v${info.version}`)}</p>
        <pre class="update-notes-text">${escapeHtml(info.notes)}</pre>
        <p class="update-notes-source">更新说明来自发布页${info.source ? `（${escapeHtml(info.source)}）` : ""}</p>
      </div>`;
  }
  return `<div class="update-notes is-empty">没能取到更新说明，点下面的「打开发布页」可以在网页上看。</div>`;
}

/** 说明是异步到的，只替换这一块，不重开弹窗（重开会闪、也会丢掉下载进度） */
function refreshUpdateNotesDom() {
  const holder = document.querySelector("#update-notes-holder");
  if (!holder) return;
  holder.innerHTML = updateNotesSection();
}

function updateModal() {
  const info = state.update || {};
  const asset = info.asset;
  const downloading = state.updateDownloading;
  const progress = state.updateProgress || { received: 0, total: asset?.size || 0, source: "" };
  const missingAsset = info.hasUpdate && !asset;
  const body = `
    <div class="update-panel">
      <p class="update-versions">当前 <strong>v${escapeHtml(info.currentVersion || APP_VERSION)}</strong> → 最新 <strong>v${escapeHtml(info.latestVersion || "?")}</strong></p>
      <div id="update-notes-holder">${updateNotesSection()}</div>
      <p class="update-note">升级包会下载到程序所在的文件夹，下载完自动重启到新版本；<strong>旧版本文件在下次启动时移入回收站</strong>（可还原）。你的作品文件和数据库都在旁边，不受影响。</p>
      ${asset ? `<p class="update-source">安装包：${escapeHtml(asset.name)}（${humanSize(asset.size)}）<br>下载顺序：直连 GitHub${asset.urls.length > 1 ? ` → ${asset.urls.length - 1} 个加速镜像` : ""}，一个失败自动换下一个<br>版本号探测来源：${escapeHtml(info.source || "未知")}</p>` : ""}
      ${missingAsset ? `<div class="note-warn">这次发布里没有找到可自动下载的安装包，请点下面的「打开发布页」手动下载。</div>` : ""}
      <div class="update-progress ${downloading ? "" : "is-hidden"}" id="update-progress-box">
        <progress value="${progress.received || 0}" max="${progress.total || 1}"></progress>
        <span class="update-progress-label">${updateProgressPercent()}% · 正在从 ${escapeHtml(progress.source || "…")} 下载</span>
      </div>
    </div>`;
  const footer = info.hasUpdate
    ? `<button class="quiet-button" data-action="ignore-update" data-version="${escapeHtml(info.latestVersion)}">忽略此版本</button>
       <span class="footer-spacer"></span>
       <button class="quiet-button" data-action="open-release-mirror-page">镜像打开发布页</button>
       <button class="quiet-button" data-action="open-release-page">打开发布页</button>
       <button class="primary-button" data-action="start-update" ${asset && !downloading ? "" : "disabled"}>${downloading ? "正在下载…" : "立即更新"}</button>`
    : `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">关闭</button>`;
  return modal("软件更新", body, footer, "is-roomy");
}

function openUpdateModal() {
  // 兜底：不管从哪条路进来的，没取过更新说明就现取一份
  if (state.update?.latestVersion) void loadUpdateNotes(state.update.latestVersion);
  showModal(updateModal());
}

/** 下载中只改进度条和那行字，不重开弹窗（重开会闪、也会丢掉滚动位置） */
function refreshUpdateProgressDom() {
  const box = document.querySelector("#update-progress-box");
  if (!box) return;
  const progress = state.updateProgress || { received: 0, total: 0, source: "" };
  box.classList.remove("is-hidden");
  const bar = box.querySelector("progress");
  if (bar) {
    bar.value = progress.received || 0;
    if (progress.total) bar.max = progress.total;
  }
  const label = box.querySelector(".update-progress-label");
  if (label) label.textContent = `${updateProgressPercent()}% · 正在从 ${progress.source || "…"} 下载`;
}

async function startUpdateDownload() {
  const info = state.update;
  if (!info?.hasUpdate || state.updateDownloading) return;
  if (!info.asset) {
    toast("这次发布没有可自动下载的安装包，请点「打开发布页」手动下载", "error");
    return;
  }
  state.updateDownloading = true;
  state.updateProgress = { received: 0, total: info.asset.size || 0, source: "" };
  const button = document.querySelector('[data-action="start-update"]');
  if (button) {
    button.disabled = true;
    button.textContent = "正在下载…";
  }
  refreshUpdateProgressDom();
  let path;
  try {
    path = await invoke("download_update", {
      version: info.latestVersion,
      assetName: info.asset.name,
      urls: info.asset.urls,
    });
  } catch (error) {
    state.updateDownloading = false;
    state.updateProgress = null;
    if (button) {
      button.disabled = false;
      button.textContent = "立即更新";
    }
    document.querySelector("#update-progress-box")?.classList.add("is-hidden");
    toast(String(error), "error");
    return;
  }
  toast("下载完成，正在重启到新版本…", "success");
  // 等一下再重启：让这条提示露个面，也避开刚写完文件的那一下
  await new Promise((resolve) => window.setTimeout(resolve, 900));
  try {
    await invoke("apply_update", { path });
  } catch (error) {
    state.updateDownloading = false;
    toast(`启动新版本失败：${String(error)}`, "error");
    return;
  }
  state.updateDownloading = false;
  state.updateProgress = null;
}

/** 更新完启动时会发现上一个版本的 exe 还在旁边，送进回收站并告诉用户一声 */
async function cleanupOldPortableBuilds() {
  try {
    const result = await invoke("cleanup_old_portable_builds");
    if (result?.removed?.length) {
      toast(`已把旧版本文件移入回收站：${result.removed.join("、")}`, "success");
    }
  } catch (error) {
    // 清理失败不影响使用，静默就好
    console.log("清理旧版本失败:", error);
  }
}

/** 启动时的自动检查：设置里关掉了就完全不发请求 */
async function autoCheckUpdateOnStartup() {
  try {
    const settings = state.settings || {};
    if (settings.autoCheckUpdate === false) return;
    await checkForUpdate({ silent: true });
  } catch (error) {
    console.log("自动检查更新失败:", error);
  }
}

async function bootstrap() {
  try {
    await refreshAuthors();
    await loadSearchSites();
    render();

    // 启动时检查Pixiv Cookie有效性
    checkPixivCookieOnStartup();

    // 更新下载进度（下载中才有事件）
    try {
      await listen("update-download-progress", (event) => {
        state.updateProgress = event.payload || null;
        refreshUpdateProgressDom();
      });
    } catch (error) {
      // 浏览器预览环境没有事件通道，静默跳过
      console.log("更新事件通道不可用:", error);
    }

    // 上一版更新完留下的旧 exe，扫一遍送回收站。
    // 等两秒再扫：新版是被旧版拉起来的，旧进程可能还没退干净，那时删不掉。
    window.setTimeout(() => { cleanupOldPortableBuilds(); }, 2000);
    // 有没有新版本：设置里关掉了就完全不发请求
    autoCheckUpdateOnStartup();
  } catch (error) {
    app.innerHTML = `<div class="fatal-error"><h1>无法初始化资料库</h1><p>${escapeHtml(String(error))}</p></div>`;
  }
}

async function checkPixivCookieOnStartup() {
  try {
    const isValid = await invoke("check_pixiv_cookie");
    if (!isValid) {
      // Cookie无效或未设置，显示提示
      showCookieWarning();
    }
  } catch (error) {
    // 检查失败，可能是没有设置cookie，静默处理
    console.log("Cookie检查失败:", error);
  }
}

function showCookieWarning() {
  const hasSeenWarning = localStorage.getItem("pixiv_cookie_warning_seen");
  if (hasSeenWarning) return;
  
  showModal(modal("Pixiv Cookie 提示", 
    `<div class="cookie-warning">
      <p>您的 Pixiv Cookie 可能已失效或未设置。</p>
      <p>Cookie 失效会导致：</p>
      <ul>
        <li>无法同步敏感作品</li>
        <li>无法获取完整的作品列表</li>
        <li>同步功能可能失败</li>
      </ul>
      <p>建议您在设置中更新 Cookie。</p>
    </div>`,
    `<button class="quiet-button" data-action="close-cookie-warning">稍后提醒</button>
     <button class="primary-button" data-action="go-to-settings">前往设置</button>`
  ));
  
  // 手动绑定按钮事件（因为弹窗是在bindEvents之后创建的）
  const closeBtn = document.querySelector('[data-action="close-cookie-warning"]');
  const settingsBtn = document.querySelector('[data-action="go-to-settings"]');
  
  if (closeBtn) {
    closeBtn.addEventListener("click", () => {
      localStorage.setItem("pixiv_cookie_warning_seen", "true");
      closeModal();
    });
  }
  
  if (settingsBtn) {
    settingsBtn.addEventListener("click", async () => {
      localStorage.setItem("pixiv_cookie_warning_seen", "true");
      closeModal();
      await settingsModal();
    });
  }
}

bootstrap();
