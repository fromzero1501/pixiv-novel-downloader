import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { HELP_DOC_CSS, HELP_DOC_HTML } from "./help-doc.js";
import "./styles.css";

const app = document.querySelector("#app");
// 版本号唯一手改源是 package.json 的 "version"：vite.config.js 把它注入成 __APP_VERSION__。
// 后面的兜底只在「没走 vite、直接拿源文件跑」时才会出现，正常情况下用不到。
const APP_VERSION = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "0.0.0";
/** 「所有作品」瀑布流一屏先画多少张，之后滚到底再追加这么多 */
const ALL_WORKS_PAGE = 60;
/**
 * 正文搜索一次最多要几篇命中（跟后端 TEXT_SEARCH_MAX_HITS 对齐）。
 * 搜「的」这种字会命中九成作品，全画出来既没意义又卡 —— 后端也是按这个数截断的。
 */
const TEXT_SEARCH_HIT_LIMIT = 200;
/**
 * 正文搜索的历史词存哪儿、最多留几条（v1.2.15）。**必须定义在 `state` 之前** ——
 * `state.textSearchHistory` 初始化时就要用它，而 `const` 有暂时性死区：
 * 放后面的话那行会抛 ReferenceError，被 `loadTextSearchHistory` 自己的 catch 吞掉，
 * 表现就是「软件重开一次，历史全没了」（这个坑真踩过）。
 *
 * 不进 settings.json —— 那是「设置」，这只是一次次搜索攒下来的词；也不进 view-state。
 */
const TEXT_SEARCH_HISTORY_KEY = "collection-library:text-search-history";
const TEXT_SEARCH_HISTORY_LIMIT = 12;
/** 瀑布流哨兵的观察器。放在模块级而不是 state 里 —— state 是数据，它是 DOM 对象 */
let loadMoreObserver = null;
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
  /**
   * 配图筛选（v1.2.13）：`all` 不限 / `has` 有图 / `none` 无图。三个列表页共用一份。
   * 原来是「仅看带图版」三个布尔（作者页 / 所有作品 / 收藏夹各存一个）——「无图」这一档
   * 布尔表达不了，而且同一件事存三份迟早对不齐（`status` 早就共用一份了）。
   */
  imagesFilter: "all",
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
  /**
   * 「所有作品」是瀑布流：一次只往 DOM 里塞 `allWorksShown` 张（v1.2.8）。
   * 库里一千四百多篇，一次铺完光卡片就够卡半天；数据本身还是整份拿回来的，
   * 所以各种筛选、排序的口径一点没变，只是「画多少张」受这个数字管。
   * 任何一次重新查询（改搜索 / 筛选 / 排序）都把它复位。
   */
  allWorksShown: ALL_WORKS_PAGE,
  /** 防抖用：正在追加下一页时不要再触发一次 */
  allWorksLoadingMore: false,
  /**
   * 正文搜索（v1.2.11）：后端不建索引，每次现扫绑定的本地正文，所以整页结果都放这儿。
   *
   * `textSearchPool` 是全库作品（用来给正文命中找 work 对象 —— 后端只回 workId），
   * 特意不跟 `allWorks` 共用：那是个列表页的状态，搜索不该顺手把它的分页数也重置了。
   * `textSearchMetaHits` 是「标题/简介/标签」那一档，先出来；`textSearchHits` 是正文扫描结果，后补上。
   */
  textSearchQuery: "",
  textSearchPool: [],
  textSearchMetaHits: [],
  textSearchHits: [],
  textSearchMeta: null,
  textSearchRunning: false,
  /**
   * 正文搜索的历史词（v1.2.15）。存 localStorage、**不进 view-state 序列化** ——
   * 那是「我刚才界面长什么样」，这是跨会话攒下来的常用词，两码事。
   */
  textSearchHistory: loadTextSearchHistory(),
  /**
   * 历史下拉是否展开。**只切 `hidden`、不触发全页 render** ——
   * 展开动作发生在输入框聚焦时，一 render 焦点和刚打的字就全没了。
   */
  textSearchHistoryOpen: false,
  /** 还没算过字数的作品数（后台补算进行中时非 0，字数档那一行会提示） */
  wordCountPending: 0,
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
  /** 收藏夹里默认按「什么时候收进来的」排，所以单独一个排序状态，不跟作者库共用 */
  collectionSort: "added_desc",
  /** 收藏夹选择器弹窗：正在改归属的作品 id + 已勾选的夹子 id */
  pickerWorkId: null,
  pickerSelected: [],
  /** 「浏览历史」页：`{ work, viewedAt, viewCount }` 列表 + 搜索词 */
  history: [],
  historyQuery: "",
  /**
   * 阅读状态筛选（v1.2.0）：`all` / `unread`（未读） / `reading`（在读，＝「继续读」清单）。
   * 三个列表页共用一份 —— 切页面还留着，比每个页面各存一份更好用。
   * v1.2.8 起显示在「高级筛选」面板里（工具栏那排按钮撤了）。
   */
  readFilter: "all",
  /** 作品详情弹窗正在展示的作品 id（null = 没开） */
  workDetailId: null,
  /** 详情弹窗里那篇作品当前所在的收藏夹 id（打开时查一次，改完刷新） */
  detailCollectionIds: [],
  /** 详情弹窗里「阅读版」文件的落盘路径（没有就空串，那一行不显示） */
  detailReadingPath: "",
  /** 详情弹窗的简介是不是展开着。默认折叠 —— 简介动辄上千字，全铺开把下面的内容顶没了 */
  detailSynopsisOpen: false,
  /** 刚刚在详情里加过标签 → 重画后把焦点还给标签输入框，方便连着敲下一个 */
  detailTagFocus: false,
  /** 批量「加入收藏夹」弹窗里勾中的收藏夹 id */
  bulkCollectionIds: [],
  /** 批量「移出收藏夹」弹窗里勾中的收藏夹 id */
  bulkRemoveCollectionIds: [],
  /**
   * 高级筛选（v1.2.7）：评分与字数在前端过滤（列表本来就是整份拉回来的），
   * 收藏夹过滤要走后端 —— 一篇作品在哪些夹子里是关系数据，前端手里没有。
   * 三个状态「作者作品库」和「所有作品」共用一份。
   */
  ratingFilter: "all",
  wordsFilter: "all",
  collectionFilter: 0,
  filterPanelOpen: false,
  /** 「待补完整版」工作台：全量缺口（含作者名，前端按作者归并） / 当前按处理状态筛的档 */
  missingFull: [],
  missingFullFilter: "todo",
  /** 待补完整版第二层：点进去的那位作者（null = 还在作者卡这一层） */
  missingFullAuthorId: null,
  /**
   * 待补完整版第二层的「批量标记」模式（v1.2.9）：作者名下有二十篇缺口时，
   * 一篇篇点「已找过·没有」是四十次点击。开了这个模式卡片上出现勾选框，
   * 顶上给一排「标为已找过·没有 / 标为不打算补 / 恢复未处理」。
   * 选中的 id 和批量操作共用一份 `selectedWorkIds`（切页会清）。
   */
  missingFullBulk: false,
  /** 「筛选模板」（v1.2.9）：存下来的筛选条件，侧栏点一下回到同一条件 */
  filterViews: [],
  /** 设置页「自动备份」那段列出来的备份文件 */
  backups: [],
};

const previewAuthors = [{ id: 1, name: "雾海档案", aliases: "雾海|档案屋", homepage: "https://www.pixiv.net/users/16208053", avatarPath: "D:\\头像\\雾海.png", notes: "", previewDir: "D:\\预览", purchasedDir: "D:\\已购", matchThreshold: 70, workCount: 48, purchasedCount: 19, imagesCount: 6, favoriteCount: 7, newCount: 3 }, { id: 2, name: "Mori", aliases: "", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 126, purchasedCount: 52, imagesCount: 14, favoriteCount: 16 }, { id: 3, name: "远野", aliases: "远野老师", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 33, purchasedCount: 8, imagesCount: 2, favoriteCount: 4 }];
// 第 1 篇故意绑 .epub：字数是 0（EPUB 读不出正文），卡片上该显示「EPUB」而不是字数 ——
// 预览数据必须和真机一个规矩，否则 v1.2.18 修的那个 bug 在这里根本复现不出来。
const previewWorks = [{ id: 1, title: "（插画附+改编图文）～希儿&布洛妮娅", releaseDate: "2025-10-05", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\希儿.epub", wordCount: 0, favorite: true }, { id: 2, title: "夏日短篇集", releaseDate: "2025-09-20", previewPath: "", coverPath: "", purchasedPath: "", wordCount: 4380, favorite: false }, { id: 3, title: "旧城的信", releaseDate: "2025-08-18", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\旧城的信.txt", wordCount: 20750, favorite: false }, { id: 4, title: "月色图文辑", releaseDate: "2025-07-09", previewPath: "", coverPath: "", purchasedPath: "", favorite: true }];

// 瀑布流（v1.2.8）在预览里没法验：就 4 篇，翻页永远触发不到。
// `window.__previewWorkScale = N` 把这 4 篇整体复制 N 份（id 与标题都错开），
// 专门用来喂「再加载」看行为；默认 1 份＝和原来完全一样。
/**
 * 预览作品池。`window.__previewWorkScale = N` 会把 4 篇原样复制 N 份（id 错开、
 * 标题加「·第 N 册」），用来验瀑布流翻页、批量操作这类「篇数太少跑不到」的路。
 * 复制出来的那份**缓存住**：不缓存的话每次都新建对象，mock 里改完的状态下一轮就没了
 * （批量标记会表现成「命令发出去了、一篇都没变」）。
 */
let previewWorkPoolCache = null;
function previewWorkPool() {
  const scale = Math.max(1, Math.trunc(Number(window.__previewWorkScale)) || 1);
  if (scale === 1) return previewWorks;
  if (previewWorkPoolCache && previewWorkPoolCache.scale === scale) return previewWorkPoolCache.list;
  const list = Array.from({ length: scale }, (_, copy) => previewWorks.map((work) => (copy === 0 ? work : { ...work, id: work.id + copy * previewWorks.length, title: `${work.title} ·第${copy + 1}册`, favorite: false }))).flat();
  previewWorkPoolCache = { scale, list };
  return list;
}

previewWorks.forEach((work, index) => {
  work.tags = ["Pixiv|小说", "短篇|日常", "小说|悬疑|长篇|都市|完结", "插画|图文"][index];
  work.pixivNovelId = ["26410188", "26521963", "26410189", ""][index];
  work.imageCount = [4, 0, 0, 0][index];
  // 个人元数据 / 简介 / 字数（v1.2.7）：给足各档取值，无头验证才测得到筛选边界
  work.rating = [5, 0, 4, 0][index];
  work.readState = [2, 0, 1, 0][index];
  work.synopsis = ["这是一个用于验证简介搜索的片段", "", "旧城的信：写信的人一直在等回音", ""][index];
  // 第 4 篇（唯一还缺完整版的另一篇）预置成「不打算补」：
  // 三个状态在预览里各有样本，验证「切档」时也能看出区别
  work.needFullState = [0, 0, 0, 2][index];
  // 处理时间（v1.2.8）：第 4 篇标成「3 天前」，正好验相对时间的渲染
  work.needFullMarkedAt = index === 3 ? new Date(Date.now() - 3 * 86400000).toISOString() : "";
  // 完整版不是 txt（EPUB / HTML…）才叫「阅读版」，卡片上显示格式名 ——
  // 判据和 Rust 侧 `populate_work_display_info` 的 `extension != "TXT"` 是同一条
  work.fileFormat = ["EPUB", "", "", ""][index];
  if (work.wordCount === undefined) work.wordCount = 0;
});

// 验证脚本要在页面加载后改这几篇的字段（比如把处理时间调成 40 天前看日期分支），
// 和 `__previewWorkScale` 一个用途：预览数据在模块作用域里，不挂出来就够不着。
window.__previewWorks = previewWorks;

/**
 * mock 里的「搜索范围 → 待匹配文字」，和 Rust 侧的 `search_match_clause` 保持同一套规则，
 * 否则无头验证会跟真机行为对不上。
 */
const mockSearchText = (work, field) => {
  const title = work.title || "";
  const synopsis = work.synopsis || "";
  const tags = work.tags || "";
  if (field === "tags") return tags;
  if (field === "title_synopsis") return `${title} ${synopsis}`;
  if (field === "title_synopsis_tags") return `${title} ${synopsis} ${tags}`;
  return title;
};
/**
 * 预览里那两档**要查库**的筛选（收藏夹 / 配图），规则跟 Rust 侧那几段 SQL 一模一样。
 *
 * 收藏夹：0 = 不限、-1 = 任意收藏夹、正数 = 指定夹子（真机是一段 EXISTS 子查询）。
 * 配图：`all` 不限 / `has` 有图 / `none` 无图。
 * 这两个条件在后端是拼进 SQL 的，mock 里不照着写一遍，无头验证就会「本地过、真机不过」。
 */
function mockFilterPass(work, collectionId, imagesFilter) {
  const picked = Number(collectionId || 0);
  const owned = work.collectionIds || [];
  if (picked === -1 && !owned.length) return false;
  if (picked > 0 && !owned.includes(picked)) return false;
  if (imagesFilter === "has" && !work.hasImages) return false;
  if (imagesFilter === "none" && work.hasImages) return false;
  return true;
}
// mock 的「问过 Pixiv 了，作者就是没写简介」名单（对齐后端的 works.synopsis_checked=1）
const mockNoSynopsis = new Set();
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

// 浏览器预览用的筛选模板（真数据在 filter_views 表里）。预置一条是为了让
// 「打开模板 → 条件写回 → 所有作品跟着筛」这条链在预览里开箱就能验到。
let previewFilterViewNextId = 1;
let previewFilterViews = [
  { id: 1, name: "未读长篇", payload: JSON.stringify({ readFilter: "unread", ratingFilter: "all", wordsFilter: "gte8w", collectionFilter: 0, allWorksQuery: "", searchField: "title", status: "all", allWorksFavoritesOnly: false, imagesFilter: "all", sort: "date_desc" }), createdAt: "2026-09-15T10:00:00+00:00" },
];
// 作者卡上的「上次同步」也要有值（真数据由 SQL 聚合，mock 里按相对日期造，标签才稳定）
previewAuthors[0].pixivLastSyncAt = previewDayAt(0, 8, 40);
previewAuthors[2].pixivLastSyncAt = previewDayAt(-2, 19, 10);
// 作品详情弹窗的个人字段（v1.2.0）：简介 / 阅读状态（0未读 1在读 2已读）/ 评分 / 笔记
previewWorks.forEach((work, index) => {
  work.synopsis = [
    // 1 号故意写成 Pixiv 那种 HTML 形态（`<br />` / `<strong>` / `<a href>` / `&amp;`）：
    // 抓回来的简介都长这样，界面必须净化后显示、外链要能点，就靠这条守
    "<br />本篇是布洛妮娅与希儿的日常向短篇，时间线接在主线之后，当作独立的甜品读也完全没问题。<br /><br />正文一共四章，配了 4 张插画，阅读版会把图一并打包进 EPUB。每一章都以一段没说完的对话收尾，作者说这是故意留的口子，让读者自己把后半句补上。<br /><br /><strong>四章里我最喜欢第三章：</strong>两个人在便利店门口站了很久，谁都没先开口，最后是雨先停了。那种「什么都没发生，但什么都变了」的感觉写得很稳。<br /><br />English/日本語版/한국어판：<a href=\"/jump.php?https%3A%2F%2Fallmylinks.com%2Fyunibobo\" target=\"_blank\">https://allmylinks.com/yunibobo</a><br />※ 完整版已在自己的平台放出 &amp; 感谢支持，转载前请先联系作者：<a href=\"https://example.com/contact\">点这里</a>",
    // 2 / 3 号留空，专门给「补抓简介」用：2 号当「作者没写简介」，3 号当「能补到」
    "",
    "",
    // 4 号没有 pixiv_novel_id（不是从 Pixiv 同步来的）→ 简介空、也从没问过 Pixiv，
    // 详情页该说「还没有简介」而不是「作者没写简介」，正好和 2 号形成两态对照
    "",
  ][index];
  work.readState = [2, 0, 1, 0][index];
  work.rating = [5, 0, 4, 0][index];
  work.note = ["重读第三遍了，插画加分。", "", "后半段有点赶，但氛围很好。", ""][index];
});
previewAuthors.forEach((author, index) => {
  author.newCount = [3, 0, 1][index];
  author.pixivLastSyncAt = ["2026-09-14T12:30:00+00:00", "", "2026-08-02T09:05:00+00:00"][index];
});

previewWorks.forEach((work, index) => {
  // 序号故意跳一号（1、3，缺 2）——「系列缺篇检测」在预览里也得验得到，
  // 全连着就永远显示「没有缺口」，那条路等于没验
  work.seriesOrder = index === 0 ? 1 : index === 1 ? 3 : 0;
  work.isNew = index === 0;
  // 作者归属（「所有作品」卡片要显示作者名、点作者名跳作品库 / 待补完整版按作者归堆）：
  // 前 3 篇归第 1 位作者，第 4 篇归第 2 位 —— 真数据由 SQL JOIN 出来
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
    return previewWorkPool().filter((work) => (!args.query || mockSearchText(work, args.searchField).includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite) && mockFilterPass(work, args.collectionId, args.imagesFilter));
  }
  if (command === "list_all_works") return previewWorkPool().filter((work) => (!args.query || mockSearchText(work, args.searchField).includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite) && mockFilterPass(work, args.collectionId, args.imagesFilter));
  if (command === "list_series_works") return previewWorkPool().filter((work) => work.seriesId === args.seriesId);
  if (command === "search_full_text") {
    // 预览里没有真的本地正文文件可扫，拿 synopsis 当「正文」占位 ——
    // 命中的篇目、片段截取、计数、截断标记这些**前端要处理的东西**都能照验。
    const needle = String(args.query || "").trim();
    const matched = [];
    let scannedCount = 0;
    if (needle) {
      for (const work of previewWorkPool()) {
        const text = String(work.synopsis || "");
        if (!text) continue;
        scannedCount += 1;
        const snippets = [];
        let hitCount = 0;
        let from = 0;
        for (;;) {
          const at = text.indexOf(needle, from);
          if (at < 0) break;
          hitCount += 1;
          if (snippets.length < 3) {
            snippets.push({
              before: text.slice(Math.max(0, at - 36), at).replace(/\s+/g, " ").trim(),
              hit: needle,
              after: text.slice(at + needle.length, at + needle.length + 36).replace(/\s+/g, " ").trim(),
            });
          }
          from = at + needle.length;
        }
        if (hitCount) matched.push({ workId: work.id, hitCount, snippets });
      }
    }
    matched.sort((left, right) => right.hitCount - left.hitCount || left.workId - right.workId);
    const limit = Number(args.limit) || 200;
    return {
      hits: matched.slice(0, limit),
      scannedCount,
      missing: [],
      missingCount: 0,
      elapsedMs: 8,
      truncated: matched.length > limit,
    };
  }
  // 系列序号故意留一个空号（1、3，缺 2），好让「缺篇检测」在预览里也验得到
  if (command === "list_series") return [{ id: "demo-series-1", title: "雾海档案短篇系列", workCount: 2, purchasedCount: 1, previewCount: 1, coverPath: "", maxOrder: 3, readCount: 1, gapOrders: [2] }];
  // 待补完整版工作台（v1.2.7）：mock 里「没有完整版」就等于 purchasedPath 为空。
  // authorName 按 authorId 现查，别统一写成第 1 位作者 —— 工作台第一层是按作者归堆的，
  // 名字全一样就看不出分组对不对了（真数据由 SQL JOIN 出来）
  if (command === "list_missing_full") return previewWorkPool().filter((work) => !work.purchasedPath).map((work) => ({ ...work, authorName: previewAuthors.find((author) => author.id === work.authorId)?.name || "未知作者" }));
  if (command === "set_works_need_full_state") {
    // 和真后端一样记下处理时间（0 = 恢复未处理，把时间清掉）。
    // 找的是 **pool** 而不是 previewWorks：开了数据倍率之后 id 是复制出来的，
    // 只认原始那 4 条的话，批量标记会「调了命令但一篇都没变」。
    args.workIds.forEach((id) => {
      const work = previewWorkPool().find((item) => item.id === id);
      if (!work) return;
      work.needFullState = args.state;
      work.needFullMarkedAt = args.state === 0 ? "" : new Date().toISOString();
    });
    return args.workIds.length;
  }
  // 批量补充（v1.2.7）
  if (command === "remove_works_from_collections") {
    let removed = 0;
    args.workIds.forEach((id) => {
      const work = previewWorks.find((item) => item.id === id);
      if (!work) return;
      const before = (work.collectionIds || []).length;
      work.collectionIds = (work.collectionIds || []).filter((collectionId) => !args.collectionIds.includes(collectionId));
      removed += before - work.collectionIds.length;
      work.favorite = work.collectionIds.length > 0;
    });
    return removed;
  }
  if (command === "set_works_rating") {
    args.workIds.forEach((id) => { const work = previewWorks.find((item) => item.id === id); if (work) work.rating = args.rating; });
    return args.workIds.length;
  }
  // 自动备份（v1.2.7）：浏览器预览里不落盘，只回一份假的记录
  if (command === "list_backups") return [
    { path: "D:\\备份\\library-auto-20260916-120000000.db", name: "library-auto-20260916-120000000.db", size: 1_835_008, createdAt: "2026-09-16 12:00" },
    { path: "D:\\备份\\library-auto-20260915-090000000.db", name: "library-auto-20260915-090000000.db", size: 1_792_000, createdAt: "2026-09-15 09:00" },
  ];
  if (command === "backup_database_now") return { path: "D:\\备份\\library-auto-20260916-235900000.db", name: "library-auto-20260916-235900000.db", size: 1_835_008, createdAt: "2026-09-16 23:59" };
  if (command === "auto_backup_if_due") return null;
  // 字数后台补算（v1.2.8）：浏览器预览里字数都是现成的，没什么可补的
  if (command === "refresh_word_counts") return { updated: 0, remaining: 0 };
  if (command === "set_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = args.seriesId; work.seriesTitle = "雾海档案短篇系列"; work.seriesOrder = args.seriesOrder; } return; }
  if (command === "leave_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = ""; work.seriesTitle = ""; work.seriesOrder = 0; } return; }
  if (command === "backfill_work_covers") return { fixedCount: 0, failedCount: 0, skippedCount: 0, failedTitles: [] };
  // 收藏夹：真数据在 collection_works 关联表里，mock 里挂在作品对象上（collectionIds）
  // 筛选模板（v1.2.9）：真数据在 filter_views 表里，这里存内存就够预览用
  if (command === "list_filter_views") return previewFilterViews.map((view) => ({ ...view }));
  if (command === "save_filter_view") {
    const record = { id: ++previewFilterViewNextId, name: args.name, payload: args.payload, createdAt: new Date().toISOString() };
    previewFilterViews.push(record);
    return { ...record };
  }
  if (command === "rename_filter_view") {
    const view = previewFilterViews.find((item) => item.id === args.id);
    if (!view) throw new Error("没找到这个筛选模板");
    view.name = args.name;
    return { ...view };
  }
  if (command === "update_filter_view") {
    const view = previewFilterViews.find((item) => item.id === args.id);
    if (!view) throw new Error("没找到这个筛选模板");
    view.payload = args.payload;
    return { ...view };
  }
  if (command === "delete_filter_view") {
    const index = previewFilterViews.findIndex((item) => item.id === args.id);
    if (index >= 0) previewFilterViews.splice(index, 1);
    return;
  }
  // 合集 EPUB：预览里不真写文件，报个账就完事（真机走 Rust 那套本地正文打包）
  if (command === "export_anthology_epub") {
    const works = args.workIds.map((id) => previewWorks.find((work) => work.id === id)).filter(Boolean);
    const skipped = works.filter((work) => !work.previewPath && !work.purchasedPath).map((work) => work.title);
    return { title: args.title, outputPath: args.path, chapters: works.length - skipped.length, imageCount: 0, sizeBytes: 1024 * 512 * (works.length - skipped.length), skipped };
  }
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
  // 浏览器预览里的假「阅读版」文件：第一篇故意给个和正文不同的路径，好看清详情页那一行
  if (command === "work_reading_path") return args.workId === 1 ? "D:\\已购\\希儿.html" : "";
  if (command === "set_work_collections") {
    const work = previewWorks.find((item) => item.id === args.workId);
    if (work) { work.collectionIds = [...args.collectionIds]; work.favorite = work.collectionIds.length > 0; }
    return;
  }
  if (command === "list_collection_works") return previewWorks.filter((work) => (work.collectionIds || []).includes(args.collectionId) && mockFilterPass(work, args.alsoInCollectionId, args.imagesFilter) && (!args.query || mockSearchText(work, args.searchField).includes(args.query)));
  // 浏览历史：mock 里存 workId，出口时再挂上作品对象（真机是一条 SQL JOIN 出来）
  if (command === "list_history") return previewHistory.map((entry) => ({ ...entry, work: previewWorks.find((work) => work.id === entry.workId) })).filter((entry) => entry.work && (!args.query || entry.work.title.includes(args.query)));
  if (command === "clear_history") { previewHistory.length = 0; return; }
  if (command === "remove_history") { const index = previewHistory.findIndex((entry) => entry.workId === args.workId); if (index >= 0) previewHistory.splice(index, 1); return; }
  if (command === "toggle_has_images") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.hasImages = !work.hasImages; return; }
  // 作品个人元数据（v1.2.0）：评分 / 阅读状态 / 笔记
  if (command === "set_work_meta") {
    const work = previewWorks.find((item) => item.id === args.workId);
    if (work) {
      if (args.readState !== null && args.readState !== undefined) work.readState = args.readState;
      if (args.rating !== null && args.rating !== undefined) work.rating = args.rating;
      if (args.note !== null && args.note !== undefined) work.note = String(args.note).trim();
    }
    return;
  }
  if (command === "set_works_read_state") {
    let changed = 0;
    for (const workId of args.workIds || []) { const work = previewWorks.find((item) => item.id === workId); if (work) { work.readState = args.readState; changed += 1; } }
    return changed;
  }
  if (command === "add_works_to_collections") {
    // 并集插入：不会把作品从别的收藏夹里踢出去
    for (const workId of args.workIds || []) {
      const work = previewWorks.find((item) => item.id === workId);
      if (!work) continue;
      const current = new Set(work.collectionIds || []);
      for (const collectionId of args.collectionIds || []) current.add(collectionId);
      work.collectionIds = [...current];
      work.favorite = work.collectionIds.length > 0;
    }
    return (args.workIds || []).length;
  }
  if (command === "update_works_tags") {
    const split = (raw) => String(raw || "").split("|").map((tag) => tag.trim()).filter(Boolean);
    const add = split(args.add);
    const remove = split(args.remove);
    let changed = 0;
    for (const workId of args.workIds || []) {
      const work = previewWorks.find((item) => item.id === workId);
      if (!work) continue;
      const before = split(work.tags);
      const tags = before.slice();
      for (const tag of add) if (!tags.some((existing) => existing.toLowerCase() === tag.toLowerCase())) tags.push(tag);
      const kept = tags.filter((tag) => !remove.some((tag2) => tag2.toLowerCase() === tag.toLowerCase()));
      if (kept.join("|") !== before.join("|")) { work.tags = kept.join("| "); changed += 1; }
    }
    return changed;
  }
  if (command === "synopsis_backfill_status") {
    const empty = previewWorks.filter((work) => work.pixivNovelId && !work.synopsis);
    const checked = empty.filter((work) => mockNoSynopsis.has(work.id)).length;
    return { pending: empty.length - checked, checkedNoSynopsis: checked };
  }
  if (command === "backfill_synopses") {
    const empty = previewWorks.filter((work) => work.pixivNovelId && !work.synopsis);
    const targets = args.recheck ? empty : empty.filter((work) => !mockNoSynopsis.has(work.id));
    let updated = 0;
    let noSynopsis = 0;
    targets.forEach((work) => {
      // 偶数 id 当「作者根本没写简介」：补抓永远补不出来，只能记下来别重复问。
      // 同时打上 synopsisChecked（对齐后端 works.synopsis_checked=1），
      // 详情页靠它把「作者没写」和「还没补抓」分开说。
      if (work.id % 2 === 0) { mockNoSynopsis.add(work.id); work.synopsisChecked = true; noSynopsis += 1; return; }
      work.synopsis = "（补抓到的简介示例）";
      updated += 1;
    });
    return { total: targets.length, updated, noSynopsis, failed: 0, cancelled: false, throttled: false };
  }
  if (command === "scan_work_files") {
    // mock 里只造「完整版目录下有一篇的文件被人挪走了」这一条失效
    const missing = previewWorks.filter((work) => work.purchasedPath && work.id === 3).map((work) => ({ workId: work.id, title: work.title, kind: "purchased", path: work.purchasedPath }));
    return { checked: previewWorks.filter((work) => work.purchasedPath || work.previewPath).length, missing, totalBytes: 187000000, totalFiles: 41, authors: [{ authorId: 1, authorName: "雾海档案", bytes: 121000000, fileCount: 22 }, { authorId: 2, authorName: "Mori", bytes: 66000000, fileCount: 19 }] };
  }
  if (command === "clear_missing_bindings") {
    const targets = previewWorks.filter((work) => work.purchasedPath && work.id === 3);
    targets.forEach((work) => { work.purchasedPath = ""; });
    return targets.length;
  }
  if (command === "export_work_list") {
    const count = args.scope === "collection" ? previewWorks.filter((work) => (work.collectionIds || []).includes(args.scopeId)).length : args.scope === "ids" ? (args.workIds || []).length : previewWorks.length;
    return { written: count, path: args.path };
  }
  if (command === "delete_work") { const index = previewWorks.findIndex((item) => item.id === args.workId); if (index >= 0) previewWorks.splice(index, 1); return; }
  if (command === "delete_works") { for (const workId of args.workIds) { const index = previewWorks.findIndex((item) => item.id === workId); if (index >= 0) previewWorks.splice(index, 1); } return; }
  if (command === "toggle_author_starred") { const author = previewAuthors.find((item) => item.id === args.authorId); if (author) author.starred = !author.starred; return Boolean(author?.starred); }
  if (command === "set_author_order") {
    const byId = new Map(previewAuthors.map((author) => [author.id, author]));
    const ordered = args.authorIds.map((id) => byId.get(id)).filter(Boolean);
    if (ordered.length === previewAuthors.length) previewAuthors.splice(0, previewAuthors.length, ...ordered);
    return;
  }
  // 搜索网站给了两条示例，方便在浏览器里直接看设置面板和右键菜单长什么样
  if (command === "get_app_settings") return { pixivCookie: "", excludedTags: "", defaultPreviewDir: "", defaultPurchasedDir: "", autoGroupDir: "", autoCreateDirs: false, minimumFileSizeBytes: 0, pixivDelayThreshold: 150, pixivDelaySeconds: 1, similarityThreshold: 70, minSimilarityThreshold: 30, matchTitleLength: 0, imageQuality: "1200", syncImageFormat: "html", autoCheckUpdate: true, recordHistory: true, autoBackupEnabled: true, autoBackupKeep: 7, updateMirrors: ["https://ghproxy.net/", "https://gh-proxy.com/", "https://ghfast.top/", "https://gh.xxooo.cf/"], searchSites: [{ name: "书香", url: "https://sxsy45.com/search.php?mod=forum&searchid=83552&orderby=dateline&ascdesc=desc&searchsubmit=yes&kw=%E5%9B%BE" }, { name: "示例站", url: "https://example.com/search?q=" }] };
  if (command === "save_app_settings") return args.settings;
  // 浏览器预览默认当作「已是最新版」；要看更新界面就把 window.__mockUpdateCheck 塞进来
  // 预览环境没有 Tauri 后端：给个默认「没填过 Cookie」，验证脚本可以塞 window.__mockCookieProbe 覆盖
  if (command === "check_pixiv_cookie") return window.__mockCookieProbe || { ok: false, status: "missing", message: "预览环境里没有 Pixiv Cookie。", userName: null, checkedAtMs: Date.now() };
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
  // 打开本地路径：预览里没有系统关联程序，当无副作用处理（别让它走到末尾那句
  // 「本地文件功能请在 Tauri 程序中使用」的报错上 —— 详情页的「打开」按钮点了会弹红条）
  if (command === "open_local_path") return;
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
  if (command === "open_work") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.isNew = false; if (work.readState === 0) work.readState = 1; } return; }
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
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5.6"/><path d="M12 7.7h.01"/>',
    download: '<path d="M12 4v12M7 11l5 5 5-5"/><path d="M5 20h14"/>',
    archive: '<rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8"/><path d="M10 12h4"/>',
    folderHeart: '<path d="M3 6.7A1.7 1.7 0 0 1 4.7 5H10l2 2h7.3A1.7 1.7 0 0 1 21 8.7v9.6a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 18.3Z"/><path d="M12 16.4c-1.8-1.2-2.8-2.4-2.8-3.6a1.5 1.5 0 0 1 2.8-.6 1.5 1.5 0 0 1 2.8.6c0 1.2-1 2.4-2.8 3.6Z"/>',
    filter: '<path d="M3.5 5.5h17l-6.6 7.7v5.6l-3.8 2.1v-7.7Z"/>',
    bookText: '<path d="M4 5.5A1.5 1.5 0 0 1 5.5 4H10a2 2 0 0 1 2 2 2 2 0 0 1 2-2h4.5A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5H12a2 2 0 0 0-2 2 2 2 0 0 0-2-2H5.5A1.5 1.5 0 0 1 4 18.5Z"/><path d="M12 6v14"/>',
  };
  // 名字以 Filled 结尾的图标用实心填充（如 starFilled）
  const fill = name.endsWith("Filled") ? "currentColor" : "none";
  return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="${fill}" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
};

const appLogo = (size = 44) => `<svg width="${size}" height="${size}" viewBox="0 0 100 100" aria-hidden="true"><rect width="100" height="100" rx="25" fill="#1595E8"/><path fill="#fff" d="M25 14h27c21 0 34 13 34 33S73 80 52 80H41v10H25V14Zm16 16v34h10c11 0 18-6 18-17s-7-17-18-17H41Z"/><path d="M68 76c5-3 10-3 14 0v13c-4-3-9-3-14 0-5-3-10-3-14 0V76c4-3 9-3 14 0Z" fill="#1595E8" stroke="#fff" stroke-width="3.5" stroke-linejoin="round"/><path d="M68 76v13" fill="none" stroke="#fff" stroke-width="3" stroke-linecap="round"/></svg>`;

function escapeHtml(value = "") {
  return String(value).replace(/[&<>'"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" }[char]));
}

/** 把 href 还原成能直接访问的地址。Pixiv 给站外链接套了一层跳转页：
 *  `/jump.php?https%3A%2F%2Fallmylinks.com%2Fyunibobo` —— 问号后面是 urlencode 过的真地址。 */
function pixivLinkTarget(href) {
  const value = String(href || "").trim();
  if (!value) return "";
  if (/^https?:\/\//i.test(value)) return value;
  const query = value.split("?").slice(1).join("?");
  if (query) {
    try {
      const decoded = decodeURIComponent(query);
      if (/^https?:\/\//i.test(decoded)) return decoded;
    } catch { /* urlencode 坏了就按站内链接处理 */ }
  }
  return value.startsWith("/") ? `https://www.pixiv.net${value}` : "";
}

/**
 * 把 Pixiv 简介切成「文字 / 链接」片段。
 * Pixiv 的简介是 HTML（`<br />`、`<strong>`、`<a href="…">`），直接转义会把标签原样露在界面上。
 * 净化放在**显示层**（库里仍存 Pixiv 原文）：可逆，已经补抓回来的老数据也能一起救，
 * 不用为了这个再重抓一遍。
 */
function synopsisSegments(raw) {
  const source = String(raw ?? "").trim();
  if (!source) return [];
  // 绝大多数简介是纯文本，没标签也没实体就别走解析，免得把 `<3` 这种当标签吃掉
  if (!/[<&]/.test(source)) return [{ text: source.replace(/\r/g, "").replace(/\u00a0/g, " ") }];
  // `<br>` / `</p>` 这类带换行语义的标签先变成真换行：textContent 不会替它们插换行
  const prepared = source
    .replace(/<\s*br\s*\/?\s*>/gi, "\n")
    .replace(/<\s*\/\s*(?:p|div|section|li|tr|h[1-6])\s*>/gi, "\n");
  const segments = [];
  const pushText = (value) => { if (value) segments.push({ text: value.replace(/\r/g, "").replace(/\u00a0/g, " ") }); };
  try {
    const holder = new DOMParser().parseFromString(`<body>${prepared}</body>`, "text/html").body;
    const walk = (node) => {
      if (node.nodeType === 3) { pushText(node.nodeValue); return; }
      if (node.nodeType !== 1) return;
      if (node.tagName === "A") {
        const label = (node.textContent || "").replace(/\s+/g, " ").trim();
        const url = pixivLinkTarget(node.getAttribute("href"));
        // 文字为空时直接拿地址当文字，别渲染出一个看不见的链接
        if (url) segments.push({ text: label || url, url });
        else pushText(label);
        return;
      }
      if (node.tagName === "IMG") return; // 纯文本视图里图片没有落脚点，跳过
      node.childNodes.forEach(walk);
    };
    holder.childNodes.forEach(walk);
  } catch {
    pushText(prepared.replace(/<[^>]*>/g, ""));
  }
  return segments;
}

/** 简介的纯文本（长度判断 / 提示语用），链接只留文字。 */
function synopsisPlainText(raw) {
  return synopsisSegments(raw)
    .map((segment) => segment.text)
    .join("")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** 简介的显示用 HTML：文本照常转义，链接渲染成可点的 `<a>`（点了交给系统浏览器）。 */
function synopsisHtml(raw) {
  return synopsisSegments(raw)
    .map((segment) => {
      const body = escapeHtml(segment.text);
      if (!segment.url) return body;
      return `<a class="detail-link" href="${escapeHtml(segment.url)}" title="${escapeHtml(`在浏览器中打开：${segment.url}`)}" rel="noreferrer noopener">${body}</a>`;
    })
    .join("")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .replace(/\n/g, "<br>")
    .replace(/^(?:<br>)+|(?:<br>)+$/g, "");
}

/**
 * 本地绝对路径 → WebView 能加载的 URL。
 *
 * 浏览器预览（没有 `__TAURI_INTERNALS__`）里 `convertFileSrc` 会直接抛
 * `Cannot read properties of undefined`，而它是在 render 途中调的 —— 一抛就把
 * 整页渲染打断，`bootstrap` 兜到 catch 里就只剩「无法初始化资料库」。
 * 以前预览数据里封面、头像都是空的，走不到这儿；现在头像有值了就得兜住。
 *
 * 兜底给一张 1×1 透明图而不是空串：`<img src="">` 会去请求当前页面、露出一块破图，
 * 而且「有图 / 没图」这两条渲染分支在预览里就断了一条（无头验证断言不到）。
 */
const PREVIEW_ASSET_FALLBACK = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

/**
 * 要一个「保存到哪儿」。**浏览器预览里没有系统对话框**，`save()` 会直接抛，
 * 而它是在导出流程的半路上调的 —— 一抛就把整段导出打断（原来是未捕获的拒绝，
 * 无头验证根本走不到导出后面那几步）。
 *
 * 所以：真机走对话框；预览里读 `window.__previewSavePath`（验证脚本塞个假路径进去，
 * 不塞就返回 null ＝ 用户点了取消，流程正常收场而不是报错）。
 */
async function askSavePath(options) {
  if (window.__TAURI_INTERNALS__) return save(options);
  return window.__previewSavePath ?? null;
}

function asset(path) {
  if (!path) return "";
  try {
    return convertFileSrc(path) || "";
  } catch {
    return PREVIEW_ASSET_FALLBACK;
  }
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
    imagesFilter: state.imagesFilter,
    collectionId: state.collectionFilter,
    sort: state.sort,
  });
}

async function refreshAllWorks() {
  state.allWorks = await invoke("list_all_works", {
    query: state.allWorksQuery,
    searchField: state.searchField,
    status: state.status,
    favoritesOnly: state.allWorksFavoritesOnly,
    imagesFilter: state.imagesFilter,
    collectionId: state.collectionFilter,
    sort: state.sort,
  });
  // 重新查过一遍就把瀑布流退回第一屏 —— 换了筛选条件还留着「已展开 600 张」很怪
  state.allWorksShown = ALL_WORKS_PAGE;
}

/**
 * 「所有作品」瀑布流：滚到底再画下一批。
 *
 * 用 IntersectionObserver 而不是滚轮事件，是因为列表本身就在窗口里滚、没有独立滚动容器；
 * 哨兵元素进了视野就说明到底了。追加之后会重画整个列表，哨兵是新节点，
 * `bindEvents()` 会重新挂上去 —— 如果那时哨兵仍在视野里（一屏放不下这么多张），
 * 会立刻再触发一次，于是连续滚到底就是平滑的无限加载。
 * 另外留一个「加载更多」按钮兜底：万一浏览器不支持/观察器没挂上，还能手点。
 */
async function loadMoreAllWorks() {
  if (state.homeView !== "allWorks" || state.activeAuthor) return;
  if (state.allWorksLoadingMore) return;
  if (state.allWorksShown >= visibleAllWorksCount()) return;
  state.allWorksLoadingMore = true;
  state.allWorksShown += ALL_WORKS_PAGE;
  render();
  state.allWorksLoadingMore = false;
}

/** 「所有作品」当前筛选条件下总共还剩多少张要画（筛选口径和 renderAllWorks 完全一致） */
function visibleAllWorksCount() {
  return collapseSerialWorks(state.allWorks).filter(matchesReadFilter).filter(matchesAdvancedFilters).length;
}

function ensureLoadMoreObserver() {
  const sentinel = document.querySelector('[data-role="all-works-sentinel"]');
  if (!sentinel) {
    loadMoreObserver?.disconnect();
    loadMoreObserver = null;
    return;
  }
  if (!loadMoreObserver) {
    loadMoreObserver = new IntersectionObserver(
      (entries) => { if (entries.some((entry) => entry.isIntersecting)) loadMoreAllWorks(); },
      // 提前 600px 就开始取下一批，滚到底时基本已经接上了
      { rootMargin: "600px 0px" },
    );
  }
  // 每次重画都是新节点，旧的自然失效；断开再观察当前这个
  loadMoreObserver.disconnect();
  loadMoreObserver.observe(sentinel);
}

/**
 * 字数后台补算（v1.2.8）。一次要读几 MB 正文，所以放在启动后慢慢跑、不挡界面。
 * 传 0 只问「还剩几篇」，用这个把进度显示出来；算完就不再调了。
 * 失败就当没这回事 —— 卡片的字数、字数档筛选顶多少点东西，不该弹错误。
 */
async function fillWordCountsInBackground() {
  try {
    for (;;) {
      const progress = await invoke("refresh_word_counts", { limit: 120 });
      const remaining = Number(progress?.remaining ?? 0);
      if (remaining !== state.wordCountPending) {
        state.wordCountPending = remaining;
        // 只在筛选面板正开着的时候重画，免得平白把用户正在看的列表刷一遍
        if (state.filterPanelOpen && state.homeView !== "missingFull") render();
      }
      if (!progress || remaining <= 0 || !Number(progress.updated)) return;
    }
  } catch (error) {
    console.log("字数补算跳过:", error);
  }
}

/**
 * 收藏 / 带图版这两个标记既影响作品卡，也影响作者卡上的统计（收藏数、带图版数），
 * 而「所有作品」与「作者作品库」用的是两份数据 —— 改完统一在这里补齐再重绘。
 *
 * 列表那半边和「详情弹窗里改完东西」的收尾是同一件事，直接复用
 * `refreshAfterDetailChange()`：它覆盖了全部列表页（作者库 / 系列 / 所有作品 /
 * 收藏夹 / 浏览历史 / 待补工作台 / 筛选模板），这里没必要再抄一份窄的。
 */
async function refreshAfterWorkFlagChange() {
  await refreshAfterDetailChange();
}

async function refreshCollections() {
  state.collections = await invoke("list_collections");
}

async function refreshCollectionWorks() {
  if (!state.activeCollection) return;
  state.collectionWorks = await invoke("list_collection_works", {
    collectionId: state.activeCollection.id,
    query: state.collectionQuery,
    searchField: state.searchField,
    status: state.status,
    imagesFilter: state.imagesFilter,
    alsoInCollectionId: state.collectionFilter,
    sort: state.collectionSort,
  });
}

/**
 * 重新拉当前正在看的那个列表。高级筛选面板里那几档要在三个列表页都生效，
 * 而三个页面的数据源不是同一个 —— 收藏夹视图用的是 `list_collection_works`。
 * 之前这里写成「不是所有作品就 refreshWorks()」，在收藏夹视图里会静默什么都不做。
 */
async function refreshActiveList() {
  if (state.homeView === "collections" && state.activeCollection) await refreshCollectionWorks();
  else if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks();
  else await refreshWorks();
}

async function refreshHistory() {
  state.history = await invoke("list_history", { query: state.historyQuery, limit: 0 });
}

/**
 * 「待补完整版」工作台的数据源。
 *
 * 一次性把全库「没绑完整版」的作品拉回来（`list_missing_full` 已按作者 + 日期排好），
 * 前端再切两层：第一层按作者归堆、第二层才是某位作者的具体作品。
 * 状态切换（未处理 / 已找过·没有 / 不打算补）全在本地筛，不用重查 —— 只有在
 * 标完状态想刷新计数时才再拉一次。
 */
async function refreshMissingFull() {
  state.missingFull = await invoke("list_missing_full");
}

function render() {
  app.innerHTML = state.activeAuthor
    ? (state.seriesView ? renderSeriesView() : renderWorks())
    : (state.homeView === "allWorks" ? renderAllWorks()
      : state.homeView === "collections" ? renderCollections()
      : state.homeView === "history" ? renderHistory()
      : state.homeView === "missingFull" ? renderMissingFull()
      : state.homeView === "filterViews" ? renderFilterViews()
      : state.homeView === "textSearch" ? renderTextSearch()
      : state.homeView === "help" ? renderHelp() : renderAuthors());
  document.body.classList.toggle("is-bulk", Boolean(state.bulkMode));
  mountHelpDocument();
  bindEvents();
  // 界面记忆的**唯一落盘点**：筛选、排序、搜索、切页最后都会走到这里，
  // 挂一处就够，加新筛选项不必再去追那十几处 data-action（详见「视图状态记忆」一节）
  scheduleViewStateSave();
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
  // 待补工作台和正文搜索结果也要算进来：那儿的卡片和别处长得一模一样，少了它点标题
  // 只会弹「没找到这篇作品」—— 明明就在眼前
  return [...state.works, ...state.seriesItems, ...state.allWorks, ...state.collectionWorks, ...state.missingFull, ...state.textSearchPool, ...state.history.map((entry) => entry.work)].find((work) => work.id === target);
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
        <button class="rail-button ${state.homeView === "missingFull" && !state.activeAuthor ? "is-active" : ""}" title="待补完整版" aria-label="待补完整版" data-action="go-missing-full">${icon("archive", 20)}</button>
        <button class="rail-button ${state.homeView === "filterViews" && !state.activeAuthor ? "is-active" : ""}" title="筛选模板" aria-label="筛选模板" data-action="go-filter-views">${icon("star", 20)}</button>
        <button class="rail-button ${state.homeView === "textSearch" && !state.activeAuthor ? "is-active" : ""}" title="正文搜索" aria-label="正文搜索" data-action="go-text-search">${icon("bookText", 20)}</button>
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
      <div class="sync-floater-foot-row">
        <span class="sync-floater-eta" id="sync-progress-eta"></span>
        ${floaterFootButton(task)}
      </div>
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
      <div class="sync-floater-foot-row">
        <button class="quiet-button" data-action="cancel-pixiv-sync" data-floater-foot ${task.cancelling ? "disabled" : ""}>${task.cancelling ? "正在终止…" : "终止同步"}</button>
      </div>
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
  // 抓取间隔拉长时把「还要等多久」报出来 —— 否则进度条半天不动，看着像卡死了
  const eta = box.querySelector("#sync-progress-eta");
  if (eta) eta.textContent = task.eta ? `剩余约 ${formatDuration(task.eta)}` : "";
  box.querySelector("#sync-progress-label").textContent = task.label;
}

/** 秒数 → 「1 分 20 秒」/「2 小时 05 分」 */
function formatDuration(seconds) {
  const total = Math.max(0, Math.round(Number(seconds) || 0));
  if (total < 60) return `${total} 秒`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} 分 ${String(total % 60).padStart(2, "0")} 秒`;
  return `${Math.floor(minutes / 60)} 小时 ${String(minutes % 60).padStart(2, "0")} 分`;
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
      <div class="author-avatar-wrap">
        ${authorAvatar(author)}
        ${author.newCount > 0 ? `<span class="author-new-badge" title="上次同步之后新收进来、还没点开看过的 ${author.newCount} 篇作品">${author.newCount > 99 ? "99+" : author.newCount}</span>` : ""}
      </div>
      <div class="author-card-body">
        <div class="author-card-title-row"><h2>${escapeHtml(author.name)}</h2><div class="author-card-actions"><button class="icon-button card-drag" title="长按拖动，调整作者顺序" aria-label="长按拖动调整顺序">${icon("grip", 17)}</button><button class="icon-button card-star${author.starred ? " is-on" : ""}" title="${author.starred ? "取消特别关注" : "设为特别关注"}" data-action="toggle-author-starred" data-author-id="${author.id}">${icon(author.starred ? "starFilled" : "star", 17)}</button><button class="icon-button card-edit" title="编辑作者" data-action="edit-author" data-author-id="${author.id}">${icon("more", 18)}</button></div></div>
        ${authorAliasList(author.aliases)}
        <dl class="author-stats"><div><dt>作品</dt><dd>${author.workCount}</dd></div><div><dt>完整版</dt><dd>${author.purchasedCount}</dd></div><div><dt>带图版</dt><dd>${author.imagesCount || 0}</dd></div><div><dt>收藏</dt><dd>${author.favoriteCount}</dd></div></dl>
        <p class="author-sync-note" title="${author.pixivLastSyncAt ? escapeHtml(author.pixivLastSyncAt) : "还没有同步记录"}">Pixiv ${escapeHtml(syncLabel(author.pixivLastSyncAt))}</p>
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

const READ_STATE_LABELS = ["未读", "在读", "已读"];

function readStateLabel(readState) {
  return READ_STATE_LABELS[Number(readState) || 0] || "未读";
}

/**
 * 阅读状态（v1.2.0）：0 未读 / 1 在读 / 2 已读。
 * 打开作品会自动推进到「在读」，只有「已读」是手动点的 —— 所以这里做成按钮，点一下循环切换。
 */
function workReadDot(work) {
  const readState = Number(work.readState) || 0;
  return `<button class="read-dot read-${readState}" title="阅读状态：${readStateLabel(readState)}（点击切换）" data-action="cycle-read-state" data-work-id="${work.id}"><span>${readStateLabel(readState)}</span></button>`;
}

/**
 * 封面左下角的一键「标为已读」（v1.2.9）。
 *
 * 为什么不是复用 `.read-dot`：那个是三档循环，从「已读」点一下会跳回「未读」——
 * 想「标完接着标下一篇」的人不敢按。这个只做一件事：没读过 → 已读，已读 → 未读。
 * 平时压在封面上会挡画面，所以只在**悬停**和**已读**两种状态露出来。
 */
function workReadToggle(work) {
  const read = Number(work.readState || 0) === 2;
  return `<button class="read-toggle ${read ? "is-read" : ""}" title="${read ? "标为未读" : "标为已读"}" data-action="toggle-read" data-work-id="${work.id}">${icon("check", 15)}<span>${read ? "已读" : "标为已读"}</span></button>`;
}

/** 卡片上的评分：没打过分就不显示，免得一排空星星白占地方。 */
function workRatingMark(work) {
  const rating = Number(work.rating) || 0;
  if (!rating) return "";
  return `<span class="work-rating" title="我的评分：${rating} 星">${icon("starFilled", 13)}<span>${rating}</span></span>`;
}

/** 标签最多显示 3 个，其余收成「+N」——完整标签在详情弹窗里看（卡片以前被这一整行撑得很臃肿）。 */
function workTags(work) {
  const tags = String(work.tags || "").split("|").map((tag) => tag.trim()).filter(Boolean);
  if (!tags.length) return "";
  const shown = tags.slice(0, 3);
  const rest = tags.length - shown.length;
  return `<div class="work-tags">${shown.map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag)}</span>`).join("")}${rest > 0 ? `<span class="work-tags-more" title="${escapeHtml(tags.slice(3).join("、"))}">+${rest}</span>` : ""}</div>`;
}

/**
 * 已读筛选（v1.2.0）：`all` / `unread` / `reading`。
 * 「reading（在读）」这一档其实就是「继续读」清单 —— 打开过但还没读完的书。
 *
 * v1.2.8 不再带 `unrated`（未评分）：搬进「高级筛选」面板后它和「评分」行的
 * 「未评分」是同一个判断（`rating === 0`），一个面板里放两个一模一样的按钮太蠢，
 * 所以只留评分行那一个 —— 用户要的「未评分可筛」照样在面板里。
 */
function matchesReadFilter(work) {
  const readState = Number(work.readState) || 0;
  if (state.readFilter === "unread") return readState === 0;
  if (state.readFilter === "reading") return readState === 1;
  if (state.readFilter === "read") return readState === 2;
  return true;
}

/**
 * 阅读状态档位。v1.2.8 起从工具栏并进「高级筛选」面板 ——
 * 工具栏那排按钮和「仅看收藏」、版本状态挤在一起，横着排到窗口外，
 * 而且它们本来就是「筛」，和面板里那几档是一类东西。
 *
 * v1.2.9 补回「已读」：卡片上有了「标为已读」的一键按钮，标完却筛不出
 * 已读的作品，这一档就成了半截功能（原来只给 未读 / 在读，是漏的）。
 */
const READ_FILTERS = [["all", "不限"], ["unread", "未读"], ["reading", "在读"], ["read", "已读"]];

/** 排序下拉（三处共用）：作者库 / 所有作品 / 收藏夹各用各的排序状态字段 */
/**
 * 搜索范围下拉。v1.2.7 起除了「标题」「标签」，中间插了两种更宽的：
 * 「标题 + 简介」和「标题 + 简介 + 标签」—— 库里的简介是整篇正文摘要，
 * 想按设定/情节找作品的时候比只搜标题有用得多。
 */
function searchFieldSelect(current) {
  const options = [["title", "标题"], ["title_synopsis", "标题 + 简介"], ["title_synopsis_tags", "标题 + 简介 + 标签"], ["tags", "标签"], ["body", "正文内容"]];
  return `<select class="sort-select search-mode-select" id="search-field" aria-label="搜索范围">${options.map(([value, label]) => `<option value="${value}" ${current === value ? "selected" : ""}>${label}</option>`).join("")}</select>`;
}

/** 搜索框的提示语跟着搜索范围走 */
function searchPlaceholder() {
  if (state.searchField === "tags") return "搜索标签";
  if (state.searchField === "title_synopsis") return "搜索标题或简介";
  if (state.searchField === "title_synopsis_tags") return "搜索标题、简介或标签";
  // 正文这一档要扫盘，不能边打边搜 —— 提示里说清楚要按回车
  if (state.searchField === "body") return "搜索正文，按回车";
  return "搜索作品名称";
}

/* ==================== 高级筛选面板（v1.2.7） ==================== */

/** 评分档位与字数档位：面板上「一行单选按钮」，选中的值直接存在 state 里 */
const RATING_FILTERS = [["all", "不限"], ["none", "未评分"], ["3", "3 星及以上"], ["4", "4 星及以上"], ["5", "5 星"]];
const WORDS_FILTERS = [["all", "不限"], ["lt5k", "5 千字以下"], ["5k-2w", "5 千 ~ 2 万字"], ["2w-8w", "2 万 ~ 8 万字"], ["gte8w", "8 万字以上"]];
/**
 * 版本状态与配图档位（v1.2.13 进面板）。
 * 版本这份**工具栏那排按钮读的是同一份** —— 一处定义、两处显示，加档位只改这里。
 */
const STATUS_FILTERS = [["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"]];
const IMAGES_FILTERS = [["all", "不限"], ["has", "有图"], ["none", "无图"]];

/**
 * 生效中的高级条件数 —— 挂在「高级筛选」按钮上，一眼看出列表是不是正在被筛。
 * 面板里每一行都要算进来，漏一行这个数就少报。
 */
function activeFilterCount() {
  return [
    state.readFilter !== "all",
    state.status !== "all",
    state.imagesFilter !== "all",
    state.ratingFilter !== "all",
    state.wordsFilter !== "all",
    state.collectionFilter !== 0,
  ].filter(Boolean).length;
}

function filterButton() {
  const count = activeFilterCount();
  return `<button class="icon-text-button filter-panel-button ${count ? "is-active" : ""} ${state.filterPanelOpen ? "is-open" : ""}" data-action="toggle-filter-panel">${icon("filter", 17)}<span>高级筛选</span>${count ? `<em class="filter-count">${count}</em>` : ""}</button>`;
}

/** 面板里的一行：标签 + 一排单选按钮 */
function filterPanelRow(label, items, active, action, attribute) {
  return `<div class="filter-panel-row"><span class="filter-panel-label">${label}</span><div class="filter-group">${items.map(([value, text]) => `<button class="filter-button ${String(active) === String(value) ? "is-active" : ""}" data-action="${action}" ${attribute}="${value}">${text}</button>`).join("")}</div></div>`;
}

/**
 * 高级筛选面板：已读 / 评分 / 字数 / 收藏夹。
 *
 * 面板是**内联展开**（不是弹窗）—— 点一下重画列表时 `filterPanelOpen` 还是 true，
 * 面板跟着一起重画但不会关掉；做成弹窗就得再写一套 refreshPickerDom 式的就地更新。
 */
function filterPanel() {
  if (!state.filterPanelOpen) return "";
  // 收藏夹那一行「不限 / 任意收藏」之后才是具体夹子。
  // 「任意收藏」用 -1 表示 —— 收藏夹主键都是正数，撞不上。
  const collections = [["0", "不限"], ["-1", "任意收藏"]].concat(state.collections.map((item) => [String(item.id), item.name]));
  // 字数还没全算完时说明一句：不解释的话，用户会以为「按字数筛」漏掉了作品
  const wordsPending = state.wordCountPending > 0
    ? `<p class="filter-panel-note">正在后台统计字数，还有 <strong>${state.wordCountPending}</strong> 篇没算完 —— 没算完的暂时进不了字数档。</p>`
    : "";
  return `<div class="filter-panel">
    ${filterPanelRow("阅读状态", READ_FILTERS, state.readFilter, "read-filter", "data-read-filter")}
    ${filterPanelRow("版本", STATUS_FILTERS, state.status, "status", "data-status")}
    ${filterPanelRow("配图", IMAGES_FILTERS, state.imagesFilter, "images-filter", "data-images-filter")}
    ${filterPanelRow("评分", RATING_FILTERS, state.ratingFilter, "rating-filter", "data-rating-filter")}
    ${filterPanelRow("字数", WORDS_FILTERS, state.wordsFilter, "words-filter", "data-words-filter")}
    ${filterPanelRow("收藏夹", collections, state.collectionFilter, "collection-filter", "data-collection-filter")}
    ${wordsPending}
    <div class="filter-panel-foot"><button class="quiet-button" data-action="clear-filters">清空筛选</button><button class="quiet-button" data-action="save-filter-view">${icon("star", 15)}存为筛选模板</button>${filterTemplateChips()}<span>这些条件都在本机筛，不会重新读文件。</span></div>
  </div>`;
}

/**
 * 面板里直接列出已存的筛选模板（v1.2.13）。
 *
 * 点一下就等于在「筛选模板」页点那一下 —— 复用 `open-filter-view`，不另造一套应用逻辑。
 * 一个都没存过时整块不出现，别让面板尾巴上挂一行空标签。
 */
function filterTemplateChips() {
  if (!state.filterViews.length) return "";
  const chips = state.filterViews
    .map((view) => `<button class="filter-template-chip" data-action="open-filter-view" data-view-id="${view.id}" title="套用这套条件：${escapeHtml(filterViewSummary(view))}">${icon("star", 12)}<span>${escapeHtml(view.name)}</span></button>`)
    .join("");
  return `<div class="filter-template-list"><span class="filter-template-label">已存模板</span>${chips}</div>`;
}

/* ============================== 筛选模板（v1.2.9） ============================== */

/**
 * 一个筛选模板里存哪些字段。**只存条件，不存结果** ——
 * 所以「未读」这类条件会随着阅读自然变少，跟收藏夹（手动往里放作品）完全不同。
 *
 * 前端加一档筛选项时记得往这儿补字段，否则新条件存不进模板：
 * 漏掉的字段会退回默认值，表现是「打开模板后条件少了一个」。
 */
const FILTER_VIEW_FIELDS = [
  ["readFilter", "all"],
  ["ratingFilter", "all"],
  ["wordsFilter", "all"],
  ["collectionFilter", 0],
  ["allWorksQuery", ""],
  ["searchField", "title"],
  ["status", "all"],
  ["imagesFilter", "all"],
  ["allWorksFavoritesOnly", false],
  ["sort", "date_desc"],
];

/**
 * 屏幕上现在是不是「某个作品列表页」，是哪一页。
 *
 * 判据必须跟 `render()` 的分发**保持一致**（`activeAuthor` 优先，`activeCollection`
 * 还得配上 `homeView === "collections"`）。光看 `state.activeCollection` 会被残留状态骗到
 * —— `go-filter-views` 这类切换不负责清它，去过收藏夹再切走，它就一直挂着；
 * 那种时候按它去存取字段名，就是「在别的页上读收藏夹的条件」。
 */
function currentWorksListPage() {
  if (state.activeAuthor) return "author";
  if (state.homeView === "collections" && state.activeCollection) return "collection";
  if (state.homeView === "allWorks") return "allWorks";
  // 模板页 / 作者库首页 / 历史 / 待补 / 正文搜索 / 帮助：这些页上没有筛选面板
  return null;
}

/**
 * 同一套条件在三个列表页上的「字段名对照」（模板字段 → 本页字段）。
 *
 * 模板按「所有作品」那套字段存（`allWorksQuery` / `allWorksFavoritesOnly` / `sort`），
 * 可筛选面板在**作者作品库**和**收藏夹**里也会出现 —— 那两页的搜索词、仅看收藏、排序
 * 各自另有字段（历史上就是分开存的状态）。所以在这两页上**存模板要按本页字段读、
 * 套模板要往本页字段写**；不走这张表就是「条件看着存下来了、列表一动不动」，
 * 或者更糟：就地套用却把人甩到「所有作品」去。
 */
function filterFieldSlots() {
  const page = currentWorksListPage();
  if (page === "author") {
    return { allWorksQuery: "workQuery", allWorksFavoritesOnly: "authorFavoritesOnly" };
  }
  if (page === "collection") {
    // 收藏夹没有「仅看收藏」开关（列出来的本来就都在夹子里），那一档不映射
    return { allWorksQuery: "collectionQuery", sort: "collectionSort" };
  }
  return {};
}

/** 当前这套筛选条件序列化成 JSON */
function currentFilterPayload() {
  const payload = {};
  const slots = filterFieldSlots();
  FILTER_VIEW_FIELDS.forEach(([key, fallback]) => {
    // 本页另有字段的，读本页那份 —— 否则在作者库里存模板会把「所有作品」的搜索词存进去
    const slot = slots[key] || key;
    payload[key] = state[slot] === undefined ? fallback : state[slot];
  });
  return JSON.stringify(payload);
}

function parseFilterPayload(raw) {
  try {
    const value = JSON.parse(raw || "{}");
    return value && typeof value === "object" ? value : {};
  } catch {
    return {};
  }
}

/**
 * 读一份存下来的模板条件，**带版本兼容**。
 *
 * v1.2.9 存的是布尔 `allWorksImagesOnly`（仅看带图版），v1.2.13 换成了三档的
 * `imagesFilter`。老模板里那个 `true` 得翻译过来，否则打开旧模板会**静悄悄地少一个条件**
 * —— 这类错不报错，只表现为「结果比当初多出一堆」，最难发现。
 * 三处读模板的地方（套用 / 摘要 / 计数）都走它，别各自 parseFilterPayload。
 */
function filterTemplatePayload(view) {
  const payload = parseFilterPayload(view.payload);
  if (payload.imagesFilter === undefined && payload.allWorksImagesOnly) payload.imagesFilter = "has";
  return payload;
}

/**
 * 套用模板 —— 面板里的 chip 和模板页那排「套用」按钮共用这一处。
 *
 * **在哪个页面上点的、条件就落在哪个页面上。** 面板在作者作品库和收藏夹里也有，
 * 在那儿点一下却被甩到「所有作品」就是丢上下文：刚才在看的这批作品没了，
 * 还得自己走回去。只有从「筛选模板」页点的才需要换页 —— 那一页本身不显示作品。
 *
 * 字段要按当前这页翻译过去（见 `filterFieldSlots()`）：模板存的是一套字段名，
 * 三个列表页却各有一套自己的搜索词 / 收藏开关 / 排序。
 */
async function applyFilterView(view) {
  const payload = filterTemplatePayload(view);
  const page = currentWorksListPage();
  const slots = filterFieldSlots();
  FILTER_VIEW_FIELDS.forEach(([key, fallback]) => {
    const value = payload[key] === undefined ? fallback : payload[key];
    state[slots[key] || key] = value;
  });
  if (page === "author") {
    // 就地重查这位作者。系列视图没地方放这套条件（它自己的工具栏上没有筛选面板），
    // 所以顺手退回作品列表 —— 那才是本页该有的样子。
    state.seriesView = null;
    state.seriesItems = [];
    await refreshWorks();
  } else if (page === "collection") {
    await refreshCollectionWorks();
  } else {
    // 「所有作品」页就地刷新；从「筛选模板」页点进来的才需要换页
    state.authorReturnTo = null;
    state.activeAuthor = null;
    state.activeCollection = null;
    state.seriesView = null;
    state.seriesItems = [];
    state.homeView = "allWorks";
    await refreshAllWorks();
  }
  render();
  toast(`已套用模板「${view.name}」`, "success");
}

/** 模板列表 / 面板 chip 上那句「条件摘要」—— 没条件就说没条件，别给一行空白 */
function filterViewSummary(view) {
  const payload = filterTemplatePayload(view);
  const parts = [];
  const readLabels = { unread: "未读", reading: "在读", read: "已读" };
  if (payload.readFilter && payload.readFilter !== "all") parts.push(readLabels[payload.readFilter] || payload.readFilter);
  if (payload.ratingFilter && payload.ratingFilter !== "all") {
    const found = RATING_FILTERS.find(([value]) => value === payload.ratingFilter);
    if (found) parts.push(found[1]);
  }
  if (payload.wordsFilter && payload.wordsFilter !== "all") {
    const found = WORDS_FILTERS.find(([value]) => value === payload.wordsFilter);
    if (found) parts.push(found[1]);
  }
  if (Number(payload.collectionFilter) === -1) {
    parts.push("任意收藏");
  } else if (Number(payload.collectionFilter) > 0) {
    const found = state.collections.find((item) => Number(item.id) === Number(payload.collectionFilter));
    parts.push(found ? `收藏夹「${found.name}」` : "某个收藏夹");
  }
  if (payload.status === "purchased") parts.push("完整版");
  if (payload.status === "unpurchased") parts.push("预览版");
  if (payload.allWorksFavoritesOnly) parts.push("仅看收藏");
  if (payload.imagesFilter === "has") parts.push("有图");
  if (payload.imagesFilter === "none") parts.push("无图");
  if (String(payload.allWorksQuery || "").trim()) parts.push(`搜索「${String(payload.allWorksQuery).trim()}」`);
  return parts.length ? parts.join(" · ") : "没有任何条件（＝全部作品）";
}

/**
 * 模板能命中多少篇。**就地拿 `state.allWorks` 现算** ——
 * 不为每个模板发一次查询：模板数量少则几个、多则几十个，
 * 每个都查库等于进一次页面打几十次 SQL，而所有作品本来就整份拉回来了。
 * 所以这个数只在「所有作品已加载」时准；没加载就先显示 `—`。
 */
function filterViewCount(view) {
  if (!state.allWorks.length) return null;
  const payload = filterTemplatePayload(view);
  const query = String(payload.allWorksQuery || "").trim().toLowerCase();
  return collapseSerialWorks(state.allWorks).filter((work) => {
    if (payload.status === "purchased" && !work.purchasedPath) return false;
    if (payload.status === "unpurchased" && work.purchasedPath) return false;
    if (payload.allWorksFavoritesOnly && !work.favorite) return false;
    if (payload.imagesFilter === "has" && !work.hasImages) return false;
    if (payload.imagesFilter === "none" && work.hasImages) return false;
    if (payload.readFilter && payload.readFilter !== "all") {
      const readState = Number(work.readState) || 0;
      const want = { unread: 0, reading: 1, read: 2 }[payload.readFilter];
      if (want !== undefined && readState !== want) return false;
    }
    if (payload.ratingFilter && payload.ratingFilter !== "all") {
      const rating = Number(work.rating || 0);
      if (payload.ratingFilter === "none") {
        if (rating !== 0) return false;
      } else if (rating < Number(payload.ratingFilter)) return false;
    }
    if (payload.wordsFilter && payload.wordsFilter !== "all") {
      const words = Number(work.wordCount || 0);
      if (words <= 0) return false;
      if (payload.wordsFilter === "lt5k" && words >= 5000) return false;
      if (payload.wordsFilter === "5k-2w" && (words < 5000 || words >= 20000)) return false;
      if (payload.wordsFilter === "2w-8w" && (words < 20000 || words >= 80000)) return false;
      if (payload.wordsFilter === "gte8w" && words < 80000) return false;
    }
    // 收藏夹那档要查库，前端算不了（「任意收藏」也一样：判不出它在哪个夹子里）——
    // 先不扣这一条，所以带收藏夹条件的模板这个数只是**上限**，摘要里会写明是哪个夹子
    if (query && !String(work.title || "").toLowerCase().includes(query)) return false;
    return true;
  }).length;
}

async function refreshFilterViews() {
  state.filterViews = await invoke("list_filter_views");
}

function renderFilterViews() {
  const cards = state.filterViews
    .map((view) => {
      const count = filterViewCount(view);
      return `<article class="filter-view-card" data-view-id="${view.id}">
        <button class="filter-view-open" data-action="open-filter-view" data-view-id="${view.id}" title="套用这个模板的条件">
          <div class="filter-view-copy">
            <h2>${escapeHtml(view.name)}</h2>
            <p>${escapeHtml(filterViewSummary(view))}</p>
            <span class="filter-view-count">${count === null ? "—" : `${count} 篇`}</span>
          </div>
        </button>
        <div class="filter-view-actions">
          <button class="quiet-button" data-action="update-filter-view" data-view-id="${view.id}" title="把这个模板的条件改写成当前这套">更新为当前条件</button>
          <button class="quiet-button" data-action="rename-filter-view" data-view-id="${view.id}">改名</button>
          <button class="quiet-button danger-text" data-action="delete-filter-view" data-view-id="${view.id}">删除</button>
        </div>
      </article>`;
    })
    .join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div><p class="section-kicker">条件收藏</p><h1>筛选模板</h1></div>
      <div class="topbar-actions"><button class="primary-button" data-action="save-filter-view">${icon("plus", 18)}<span>存为模板</span></button></div>
    </section>
    <section class="library-content">
      <div class="read-only-note">模板存的是<strong>条件</strong>，不是作品 —— 点开就按当初那套条件现算一遍，所以「未读」这类数字会随着阅读自然变少。想把作品真正攒起来，用「我的收藏」里的收藏夹。</div>
      <div class="filter-view-grid">${cards || `<div class="empty-state works-empty"><div class="empty-icon">${icon("star", 26)}</div><h2>还没有筛选模板</h2><p>在任意列表页打开「高级筛选」，把条件调好之后点「存为筛选模板」。</p></div>`}</div>
    </section>`);
}

/* ======================== 正文搜索（v1.2.11） ======================== */

/**
 * 库内正文检索。后端**不建索引**，每次现扫绑定的本地正文
 * （实测整库一百多兆、一千四百个文件约 0.2～0.4 秒），所以：
 * ① 只按回车 / 点按钮触发，绝不能挂在输入事件上；
 * ② 结果整体放在这一页上，不动「所有作品」那个列表 —— 免得顺手把它的瀑布流状态清了。
 *
 * 两段式：先出「标题 / 简介 / 标签」的命中（数据库，毫秒级），正文结果随后补上 ——
 * 这样那零点几秒不是在盯空白屏。
 */
/**
 * 读历史词。localStorage 里的东西一律当不可信处理：手改过、旧版本残留、存成别的类型都可能，
 * 所以逐项转字符串、丢掉空的、超出上限的截掉。
 * 必须是**函数声明**：`state` 初始化时就要调它（声明会提升）。
 */
function loadTextSearchHistory() {
  // 常量引用故意放在 try 外面：写错（比如又把定义挪到 state 后面去了）要当场炸出来，
  // 不能被下面的 catch 当成「localStorage 读不到」静默吃掉 —— 那个 bug 藏起来特别像「历史功能没做」
  const key = TEXT_SEARCH_HISTORY_KEY;
  let stored = null;
  try {
    stored = window.localStorage.getItem(key);
  } catch (error) {
    // 隐私模式 / 存储被禁用：读不到就当没有历史，不能连累启动
    console.log("读不到搜索历史:", error);
    return [];
  }
  let parsed = null;
  try {
    parsed = JSON.parse(stored || "[]");
  } catch (error) {
    // 存储被人手改坏了：当没有，不要连累启动
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed
    .map((item) => String(item == null ? "" : item).trim())
    .filter(Boolean)
    .slice(0, TEXT_SEARCH_HISTORY_LIMIT);
}

function saveTextSearchHistory(list) {
  try {
    window.localStorage.setItem(TEXT_SEARCH_HISTORY_KEY, JSON.stringify(list.slice(0, TEXT_SEARCH_HISTORY_LIMIT)));
  } catch (error) {
    // 隐私模式 / 存储被禁用：存不下历史不该连搜索本身都不能用
    console.log("搜索历史没存下:", error);
  }
}

/** 记一笔：已经有的提到最前（不重复堆同一条），挤掉最旧的 */
function rememberTextSearch(query) {
  const needle = String(query || "").trim();
  if (!needle) return;
  state.textSearchHistory = [needle, ...state.textSearchHistory.filter((item) => item !== needle)].slice(0, TEXT_SEARCH_HISTORY_LIMIT);
  saveTextSearchHistory(state.textSearchHistory);
}

function removeTextSearchHistory(query) {
  state.textSearchHistory = state.textSearchHistory.filter((item) => item !== query);
  saveTextSearchHistory(state.textSearchHistory);
}

function clearTextSearchHistory() {
  state.textSearchHistory = [];
  saveTextSearchHistory(state.textSearchHistory);
}

/**
 * 历史下拉。容器**永远输出**（哪怕没历史）—— 这样「删到一条不剩」时
 * 不用去 DOM 里补一个节点，只在 `refresh` 里换 innerHTML 就够了。
 *
 * 展开与否由 `state.textSearchHistoryOpen` 决定，直接写进 `hidden` 属性：
 * 展开这一下发生在输入框聚焦时，**绝不能走 render**【一 render 焦点和刚打的字全没了】。
 */
function textSearchHistoryPanel() {
  const items = state.textSearchHistory
    .map((query) => `<li><button type="button" class="text-search-history-item" data-action="use-text-search-history" data-query="${escapeHtml(query)}" title="${escapeHtml(query)}">${escapeHtml(query)}</button><button type="button" class="text-search-history-del" data-action="remove-text-search-history" data-query="${escapeHtml(query)}" title="删掉这条" aria-label="删掉这条">${icon("x", 13)}</button></li>`)
    .join("");
  const open = state.textSearchHistoryOpen && Boolean(items);
  return `<div class="text-search-history" id="text-search-history"${open ? "" : " hidden"}>
    ${items ? `<div class="text-search-history-head"><span>最近搜过</span><button type="button" class="link-button" data-action="clear-text-search-history">清空</button></div><ul>${items}</ul>` : ""}
  </div>`;
}

/**
 * 只切 `hidden`，不重画整页 —— 聚焦/失焦那两下用。
 * 删单条、清空走的是 render（那种场合用户视线就在下拉上，不会被抢焦点困扰）。
 */
function syncTextSearchHistoryPanel() {
  const panel = app.querySelector("#text-search-history");
  if (!panel) return;
  panel.hidden = !state.textSearchHistoryOpen || !state.textSearchHistory.length;
}

function renderTextSearch() {
  const pool = state.textSearchPool;
  const metaHits = state.textSearchMetaHits;
  const meta = state.textSearchMeta;
  const hits = state.textSearchHits;
  const metaCards = metaHits.map((work) => linkedWorkCard(work)).join("");
  const bodyCards = hits
    .map((hit) => {
      const work = pool.find((item) => item.id === hit.workId);
      return work ? linkedWorkCard(work, textHitSnippets(hit)) : "";
    })
    .join("");

  let bodySection = "";
  if (state.textSearchRunning) {
    bodySection = `<div class="text-search-progress">${icon("sync", 18)}<span>正在扫全库正文…（读本地文件，不联网）</span></div>`;
  } else if (meta) {
    const notes = [];
    if (meta.truncated) notes.push(`命中太多，只列了前 ${hits.length} 篇`);
    if (meta.missingCount) notes.push(`${meta.missingCount} 篇没有可读的正文（没绑阅读版、或者文件不在原处），已跳过`);
    bodySection = `
      <div class="text-search-section">
        <div class="text-search-heading">
          <h2>正文里命中 <em>${hits.length}</em> 篇</h2>
          <span class="text-search-stats">扫过 ${meta.scannedCount} 篇正文 · 用时 ${meta.elapsedMs} 毫秒</span>
        </div>
        ${notes.length ? `<div class="read-only-note">${notes.map((note) => escapeHtml(note)).join("　")}</div>` : ""}
        ${bodyCards
          ? `<div class="works-grid">${bodyCards}</div>`
          : `<div class="empty-state works-empty"><h2>正文里没搜到「${escapeHtml(state.textSearchQuery)}」</h2><p>换个词试试；如果是想找某个设定，把范围调成「标题 + 简介」可能更合适。</p></div>`}
      </div>`;
  }

  return renderShell(`
    <section class="topbar work-topbar">
      <div><p class="section-kicker">库内检索</p><h1>正文搜索</h1></div>
    </section>
    <section class="library-content">
      <div class="text-search-bar">
        <div class="text-search-field">
          <label class="search-field is-large"><span>${icon("search", 20)}</span><input id="text-search-input" type="search" placeholder="在本地正文里搜索，按回车" value="${escapeHtml(state.textSearchQuery)}" autocomplete="off"></label>
          ${textSearchHistoryPanel()}
        </div>
        <button class="primary-button" data-action="run-text-search">${icon("search", 18)}<span>搜索</span></button>
      </div>
      <div class="read-only-note">搜的是<strong>本地正文文件</strong>（txt / md / html / EPUB 内页），不联网、不建索引 —— 每次现扫一遍，所以按回车才跑，打字时不动。想找「那句台词在哪篇里」，用这一档。</div>
      ${!state.textSearchQuery
        ? `<div class="empty-state works-empty"><div class="empty-icon">${icon("bookText", 26)}</div><h2>输入正文里的几个字</h2><p>正文里的字都能搜到，比只搜标题精确得多；命中的前后文会直接列在卡片上。</p></div>`
        : `
        <div class="text-search-section">
          <div class="text-search-heading">
            <h2>标题 / 简介 / 标签里命中 <em>${metaHits.length}</em> 篇</h2>
            <span class="text-search-stats">数据库直接查，先看这个</span>
          </div>
          ${metaCards ? `<div class="works-grid">${metaCards}</div>` : `<div class="empty-state works-empty"><p>标题、简介、标签里都没有这个词。</p></div>`}
        </div>
        ${bodySection}`}
    </section>`);
}

/** 正文命中的片段：命中处前后各截一段，中间那几个字高亮 */
function textHitSnippets(hit) {
  const rows = (hit.snippets || [])
    .map((snippet) => `<p class="text-snippet">${snippet.before ? `<span>…${escapeHtml(snippet.before)}</span>` : ""}<mark>${escapeHtml(snippet.hit)}</mark>${snippet.after ? `<span>${escapeHtml(snippet.after)}…</span>` : ""}</p>`)
    .join("");
  return `<div class="text-hit-snippets">${rows}<span class="text-hit-count">正文里命中 ${hit.hitCount} 处</span></div>`;
}

/**
 * 从别处的搜索框跳进正文搜索页（搜索范围选了「正文内容」再按回车）。
 * 原词带过去、顺手就跑 —— 跳过来还要再按一次回车太别扭。
 */
async function openTextSearch(query) {
  state.authorReturnTo = null;
  state.activeAuthor = null;
  state.seriesView = null;
  state.seriesItems = [];
  state.homeView = "textSearch";
  await runTextSearch(query);
}

/**
 * 跑一次正文搜索。两段请求**一起发出去**，谁也不需要等谁：
 * 一段查数据库（快），一段让后端扫盘（慢），慢的先落地也无所谓。
 */
async function runTextSearch(query) {
  // 没传词就以**框里的值**为准，而不是 state。
  // 输入法上屏那一下，有的中文输入法在 Chromium 里只发 compositionend、不补 input 事件，
  // state 会停在旧值上；回车一直读的是 DOM 值所以看着正常，点搜索按钮却会拿着空 state 去搜
  // —— 顺手把框里的话也一起清掉。以框为准，两条路就同源了。
  const box = app.querySelector("#text-search-input");
  const raw = query == null ? (box ? box.value : state.textSearchQuery) : query;
  const needle = String(raw).trim();
  state.textSearchQuery = needle;
  state.textSearchMetaHits = [];
  state.textSearchHits = [];
  state.textSearchMeta = null;
  // 真跑了才记一笔，顺手把历史下拉收起来；空词不进历史（从别处跳进来也会走到这儿）
  state.textSearchHistoryOpen = false;
  rememberTextSearch(needle);
  if (!needle) {
    state.textSearchRunning = false;
    render();
    return;
  }
  state.textSearchRunning = true;
  render();

  const shared = { status: "all", sort: state.sort, collectionId: 0, favoritesOnly: false, imagesFilter: "all" };
  const metaRequest = invoke("list_all_works", { ...shared, query: needle, searchField: "title_synopsis_tags" });
  // 正文结果只回 workId，卡片要完整对象 —— 顺手把全库作品拉一份存着（只拉一次，之后复用）
  const poolRequest = state.textSearchPool.length
    ? Promise.resolve(state.textSearchPool)
    : invoke("list_all_works", { ...shared, query: "", searchField: "title" });
  // 先把失败接住：它可能比另一段先失败，那时还没人 await 它
  const bodyRequest = invoke("search_full_text", { query: needle, limit: TEXT_SEARCH_HIT_LIMIT })
    .then((result) => ({ result }))
    .catch((error) => ({ error: String(error) }));

  try {
    const [pool, metaHits] = await Promise.all([poolRequest, metaRequest]);
    if (state.textSearchQuery !== needle) return; // 词已经改了，这批作废
    if (pool.length) state.textSearchPool = pool;
    state.textSearchMetaHits = metaHits;
    render();
    restoreSearchFocus("text-search-input");
  } catch (error) {
    if (state.textSearchQuery !== needle) return;
    state.textSearchRunning = false;
    toast(`搜索失败：${error}`, "error");
    render();
    return;
  }

  const body = await bodyRequest;
  if (state.textSearchQuery !== needle) return;
  if (body.error) {
    state.textSearchHits = [];
    state.textSearchMeta = null;
    toast(`正文搜索失败：${body.error}`, "error");
  } else {
    state.textSearchHits = body.result.hits || [];
    state.textSearchMeta = body.result;
  }
  state.textSearchRunning = false;
  render();
  restoreSearchFocus("text-search-input");
}

/**
 * 评分 / 字数两个条件在前端过滤，不走 SQL —— 列表本来就是整份拉回来的
 * （作者作品库、所有作品都没有分页），多筛一遍不额外查库。
 * 收藏夹那条不在这里：它是关系数据，在 SQL 里用 EXISTS 子查询处理。
 */
function matchesAdvancedFilters(work) {
  if (state.ratingFilter !== "all") {
    const rating = Number(work.rating || 0);
    if (state.ratingFilter === "none") {
      if (rating !== 0) return false;
    } else if (rating < Number(state.ratingFilter)) {
      return false;
    }
  }
  if (state.wordsFilter !== "all") {
    const words = Number(work.wordCount || 0);
    // 字数为 0 表示「读不出字数」（绑的是 EPUB / HTML 阅读版就是这样）。
    // 那是「不知道」，不是「0 个字」，所以任何字数档位都不收它。
    if (words <= 0) return false;
    if (state.wordsFilter === "lt5k" && words >= 5000) return false;
    if (state.wordsFilter === "5k-2w" && (words < 5000 || words >= 20000)) return false;
    if (state.wordsFilter === "2w-8w" && (words < 20000 || words >= 80000)) return false;
    if (state.wordsFilter === "gte8w" && words < 80000) return false;
  }
  return true;
}

function sortSelect(current) {
  return `<select class="sort-select" id="sort-select" aria-label="排序"><option value="date_desc" ${current === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${current === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${current === "title_asc" ? "selected" : ""}>名称 A-Z</option><option value="words_desc" ${current === "words_desc" ? "selected" : ""}>字数从多到少</option><option value="rating_desc" ${current === "rating_desc" ? "selected" : ""}>评分从高到低</option></select>`;
}

function renderWorks() {
  const author = state.activeAuthor;
  const works = collapseSerialWorks(state.works).filter(matchesReadFilter).filter(matchesAdvancedFilters);
  const cards = works.map((work) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"} ${state.bulkMode ? "is-selecting" : ""}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${state.bulkMode ? `<button class="selection-badge ${state.selectedWorkIds.has(work.id) ? "is-selected" : ""}" title="${state.selectedWorkIds.has(work.id) ? "取消选择" : "选择作品"}" data-action="toggle-select" data-work-id="${work.id}">${state.selectedWorkIds.has(work.id) ? icon("check", 16) : ""}</button>` : `${workReadToggle(work)}${workMenuButton(work)}`}
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}${workReadDot(work)}${workRatingMark(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${workSeries(work)}${workTags(work)}</div>
    </article>`).join("");

  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="${state.authorReturnTo === "allWorks" ? "返回所有作品" : "返回作者库"}" data-action="${state.authorReturnTo === "allWorks" ? "back-to-all-works" : "go-home"}">${icon("back", 20)}</button><div><p class="section-kicker">作者作品库${author.homepage ? `<button class="homepage-link" title="打开作者主页：${escapeHtml(author.homepage)}" data-action="open-external-url" data-url="${escapeHtml(author.homepage)}">${icon("link", 13)}<span>作者主页</span></button>` : ""}</p><h1>${escapeHtml(author.name)}</h1></div></div>
      <div class="topbar-actions">${state.bulkMode ? `<strong class="bulk-count">已选 ${state.selectedWorkIds.size} 篇</strong>` : `<button class="icon-text-button" data-action="bulk-mode">${icon("more", 18)}<span>批量操作</span></button><button class="icon-text-button" data-action="edit-author" data-author-id="${author.id}">${icon("settings", 18)}<span>作者设置</span></button><button class="icon-text-button" data-action="import-works">${icon("plus", 18)}<span>导入作品</span></button><button class="primary-button" data-action="sync-pixiv">${icon("sync", 18)}<span>作品同步</span></button>`}</div>
    </section>
    ${state.bulkMode ? bulkBar(works) : ""}
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${searchPlaceholder()}" value="${escapeHtml(state.workQuery)}" autocomplete="off"></label>
        ${searchFieldSelect(state.searchField)}
        <div class="filter-group" role="group" aria-label="版本状态">${STATUS_FILTERS.map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.authorFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <button class="icon-text-button favorite-filter images-filter ${state.imagesFilter === "has" ? "is-active" : ""}" data-action="images-only" title="只看有配图的作品（等同高级筛选里的「配图 · 有图」）">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.works)}
        ${filterButton()}
        ${sortSelect(state.sort)}
      </div>
      ${filterPanel()}
      <div class="binding-bar"><div><strong>本地文件</strong><span>${author.previewDir ? "预览版目录已绑定" : "尚未绑定预览版目录"} · ${author.purchasedDir ? "完整版目录已绑定" : "尚未绑定完整版目录"}</span><em class="sync-status">Pixiv ${syncLabel(author.pixivLastSyncAt)}</em></div><div><button class="quiet-button" data-action="scan-preview">${icon("folder", 17)}关联预览版文件</button><button class="quiet-button" data-action="scan-purchased">${icon("upload", 17)}关联完整版文件</button></div></div>
      <div class="works-grid">${cards || renderEmptyWorks()}</div>
    </section>`);
}

function renderAllWorks() {
  const filtered = collapseSerialWorks(state.allWorks).filter(matchesReadFilter).filter(matchesAdvancedFilters);
  // 瀑布流：只画前 allWorksShown 张，剩下的滚到底再说（库里一千四百多篇，一次铺完很卡）
  const shown = filtered.slice(0, Math.max(ALL_WORKS_PAGE, state.allWorksShown));
  const cards = shown.map((work) => `
    <article class="work-card is-read-only ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${workReadToggle(work)}
      </div>
      <div class="work-copy">${work.authorName ? `<button class="work-author is-link" title="打开「${escapeHtml(work.authorName)}」的作品库" data-action="open-author" data-author-id="${work.authorId}">${escapeHtml(work.authorName)}</button>` : `<p class="work-author"></p>`}<div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}${workReadDot(work)}${workRatingMark(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${allWorkSeries(work)}${workTags(work)}</div>
    </article>`).join("");
  const remaining = filtered.length - shown.length;
  const footer = filtered.length
    ? `<div class="load-more-foot">
        <span class="load-more-count">已显示 <strong>${shown.length}</strong> / ${filtered.length} 篇</span>
        ${remaining > 0 ? `<button class="quiet-button" data-action="load-more-all-works">再加载 ${Math.min(ALL_WORKS_PAGE, remaining)} 篇</button>` : `<span class="load-more-done">到底了</span>`}
      </div>
      ${remaining > 0 ? '<div data-role="all-works-sentinel" class="load-more-sentinel" aria-hidden="true"></div>' : ""}`
    : "";
  return renderShell(`
    <section class="topbar work-topbar"><div><p class="section-kicker">全部作者</p><h1>所有作品</h1></div><div class="topbar-actions"><button class="icon-text-button" data-action="export-all">${icon("download", 18)}<span>导出清单</span></button></div></section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${searchPlaceholder()}" value="${escapeHtml(state.allWorksQuery)}" autocomplete="off"></label>
        ${searchFieldSelect(state.searchField)}
        <div class="filter-group" role="group" aria-label="版本状态">${STATUS_FILTERS.map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.allWorksFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <button class="icon-text-button favorite-filter images-filter ${state.imagesFilter === "has" ? "is-active" : ""}" data-action="images-only" title="只看有配图的作品（等同高级筛选里的「配图 · 有图」）">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.allWorks)}
        ${filterButton()}
        ${sortSelect(state.sort)}
      </div>
      ${filterPanel()}
      <div class="read-only-note">所有作品仅供搜索、筛选与打开查看。</div>
      <div class="works-grid">${cards || '<div class="empty-state works-empty"><h2>没有符合条件的作品</h2></div>'}</div>
      ${footer}
    </section>`);
}

/** 作品卡右下角的「更多操作」按钮（v1.2.1 起点它直接开详情弹窗） */
function workMenuButton(work) {
  return `<button class="work-menu" title="查看详情" data-action="work-menu" data-work-id="${work.id}">${icon("more", 18)}</button>`;
}

/**
 * 跨作者的作品卡（收藏夹 / 浏览历史 / 正文搜索用）：作者名可以点，直接落到那位作者的作品库。
 *
 * `extra` 是可选的附加块（正文搜索拿来挂命中片段），插在标签下面。
 * ⚠️ 别把本函数直接交给 `map()` —— 第二个参数会当成 `extra` 传进来（就是数组下标），
 * 于是每张卡上会莫名多出一个数字。调用处一律写 `.map((work) => linkedWorkCard(work))`。
 */
function linkedWorkCard(work, extra = "") {
  return `
    <article class="work-card is-read-only ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${workReadToggle(work)}${workMenuButton(work)}
      </div>
      <div class="work-copy">${work.authorName ? `<button class="work-author is-link" title="打开「${escapeHtml(work.authorName)}」的作品库" data-action="open-author" data-author-id="${work.authorId}">${escapeHtml(work.authorName)}</button>` : `<p class="work-author"></p>`}<div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}${workReadDot(work)}${workRatingMark(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${allWorkSeries(work)}${workTags(work)}${extra}</div>
    </article>`;
}

/* ============================ 作品详情弹窗（v1.2.0） ============================ */

/**
 * 作品详情弹窗：卡片上塞不下、或者只看一眼用不着的东西都放这儿 ——
 * 简介、完整标签、系列、我的记录（评分 / 阅读状态 / 笔记）、收藏夹、文件路径、Pixiv。
 *
 * 作品卡上的「三个点」现在**直接开这一页**，原来那个菜单的功能全部搬进来了。
 */
async function openWorkDetail(workId) {
  const work = findWork(workId);
  if (!work) { toast("没找到这篇作品，刷新一下", "error"); return; }
  state.workDetailId = workId;
  state.detailSynopsisOpen = false;
  state.detailTagFocus = false;
  // 阅读版路径拿不到（没生成过）不算错误，只是那一行不显示
  const [collectionIds, readingPath] = await Promise.all([
    invoke("work_collections", { workId }),
    invoke("work_reading_path", { workId }).catch(() => ""),
  ]);
  state.detailCollectionIds = collectionIds;
  state.detailReadingPath = String(readingPath || "");
  showModal(modal(work.title, `<div id="work-detail-body">${workDetailBody()}</div>`, detailFooter(), "is-wide"));
  bindDetailExtras();
}

/** 详情弹窗底部：左边删除（危险动作放角落），右边关闭 */
function detailFooter() {
  return `<button class="danger-button" data-action="detail-delete" data-work-id="${state.workDetailId}">${icon("more", 17)}删除作品</button><span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">关闭</button>`;
}

/**
 * 详情页要显示的作者名。
 * 作者自己的作品库那条查询是 `'' AS author_name`（那一页本来就知道是谁），
 * 所以从作者列表和当前作者两处兜底 —— 否则从作者页点进详情会显示「未知作者」。
 */
function workAuthorName(work) {
  return work.authorName
    || state.authors.find((author) => author.id === work.authorId)?.name
    || state.activeAuthor?.name
    || "";
}

/** 详情副标题里的格式信息：EPUB 这类报不出字数的，把配图数报出来 */
function workFormatLabels(work) {
  const parts = [];
  if (work.wordCount) parts.push(wordCountLabel(work.wordCount));
  else if (work.fileFormat) parts.push(String(work.fileFormat).toUpperCase());
  if (work.imageCount > 0) parts.push(`${work.imageCount} 张配图`);
  return parts;
}

/**
 * 重画详情弹窗内容（改完评分 / 阅读状态 / 标签 / 收藏夹之后）。
 * **绝不重开弹窗** —— 重开会闪屏、丢滚动位置，和收藏夹选择器里踩过的是同一个坑。
 */
function refreshDetailDom() {
  const holder = document.querySelector("#work-detail-body");
  if (!holder || !state.workDetailId) return;
  holder.innerHTML = workDetailBody();
  bindEvents();
  bindDetailExtras();
}

/** 笔记 / 标签是文本框，靠 `change`（失焦）和 Enter 落盘，不能用 data-action 走点击委托 */
function bindDetailExtras() {
  const textarea = document.querySelector("#work-detail-note");
  if (textarea) textarea.addEventListener("change", () => saveDetailNote());
  const tagInput = document.querySelector("#work-detail-tag-input");
  if (tagInput) {
    tagInput.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); addDetailTag(); } });
    // 连着加好几个标签时，重画详情会把焦点抢走 —— 还站在标签框里就还它焦点
    if (state.detailTagFocus) tagInput.focus();
  } else {
    state.detailTagFocus = false;
  }
}

/** 把笔记输入框里的内容收下来写库。返回是否有改动。 */
function captureDetailNote() {
  const textarea = document.querySelector("#work-detail-note");
  if (!textarea || !state.workDetailId) return false;
  const work = findWork(state.workDetailId);
  const value = textarea.value.trim().slice(0, 200);
  if (!work || (work.note || "") === value) return false;
  work.note = value;
  invoke("set_work_meta", { workId: state.workDetailId, readState: null, rating: null, note: value })
    .catch((error) => toast(`笔记没保存：${error}`, "error"));
  return true;
}

async function saveDetailNote() {
  const textarea = document.querySelector("#work-detail-note");
  if (!textarea || !state.workDetailId) return;
  const work = findWork(state.workDetailId);
  if (!work) return;
  const value = textarea.value.trim().slice(0, 200);
  if ((work.note || "") === value) return;
  try {
    await invoke("set_work_meta", { workId: state.workDetailId, readState: null, rating: null, note: value });
    work.note = value;
    toast("笔记已保存", "success");
  } catch (error) {
    toast(`笔记没保存：${error}`, "error");
  }
}

/** 改阅读状态 / 评分：先把还压在输入框里的笔记收掉，避免重画时丢掉 */
async function setDetailMeta(patch) {
  if (!state.workDetailId) return;
  captureDetailNote();
  const work = findWork(state.workDetailId);
  try {
    await invoke("set_work_meta", { workId: state.workDetailId, readState: null, rating: null, note: null, ...patch });
    if (work) Object.assign(work, patch);
    refreshDetailDom();
  } catch (error) {
    toast(String(error), "error");
  }
}

/** 卡片上的阅读状态点：点一下循环 未读 → 在读 → 已读 → 未读 */
async function cycleReadState(workId) {
  const work = findWork(workId);
  if (!work) return;
  const next = (Number(work.readState) + 1) % 3;
  try {
    await invoke("set_work_meta", { workId, readState: next, rating: null, note: null });
    work.readState = next;
    render();
  } catch (error) {
    toast(String(error), "error");
  }
}

/**
 * 卡片左下角那个一键「标为已读」（v1.2.9）：没读过 → 已读，已读 → 未读。
 *
 * 和三档循环的 `cycleReadState` 有意分开：循环那个从「已读」点一下会掉回「未读」，
 * 想连着标一批就不敢按。这条只做两个状态之间的一步切换。
 */
async function toggleWorkRead(workId) {
  const work = findWork(workId);
  if (!work) return;
  const read = Number(work.readState || 0) === 2;
  const next = read ? 0 : 2;
  try {
    await invoke("set_work_meta", { workId, readState: next, rating: null, note: null });
    work.readState = next;
    // 就地改完重画，不重查列表 —— 重查会把「所有作品」的瀑布流退回第一屏
    render();
    toast(read ? "已标为未读" : "已标为已读", "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

function detailPathRow(label, path, extra = "") {
  const value = String(path || "").trim();
  if (!value) return `<div class="detail-path"><dt>${label}</dt><dd class="is-empty">未绑定</dd></div>`;
  return `<div class="detail-path"><dt>${label}</dt><dd><span class="detail-path-text" title="${escapeHtml(value)}">${escapeHtml(value)}</span><span class="detail-path-actions"><button class="quiet-button" data-action="detail-open-path" data-path="${escapeHtml(value)}">打开</button><button class="quiet-button" data-action="detail-open-dir" data-path="${escapeHtml(value)}">所在目录</button>${extra}</span></dd></div>`;
}

/** 详情里的简介展开/收起。只是本地的显示状态，不落盘。 */
function toggleDetailSynopsis() {
  state.detailSynopsisOpen = !state.detailSynopsisOpen;
  refreshDetailDom();
}

/**
 * 详情里改标签（v1.2.2）：原来那个「编辑标签」弹窗整个并进来了 ——
 * 加 / 删都当场落库，不再有「保存标签」按钮（弹窗那套要先攒着改再一次性存，手滑关掉就白改）。
 */
function detailTagList(work) {
  return String(work?.tags || "").split("|").map((tag) => tag.trim()).filter(Boolean);
}

async function writeDetailTags(tags) {
  const work = findWork(state.workDetailId);
  if (!work) return;
  captureDetailNote();
  try {
    await invoke("update_work_tags", { workId: state.workDetailId, tags });
    work.tags = tags.join("|");
    await refreshAfterDetailChange();
    toast("标签已更新", "success");
  } catch (error) {
    toast(`标签没保存：${error}`, "error");
  }
}

async function addDetailTag() {
  const input = document.querySelector("#work-detail-tag-input");
  const work = findWork(state.workDetailId);
  if (!input || !work) return;
  const value = input.value.trim();
  if (!value) return;
  const tags = detailTagList(work);
  if (tags.includes(value)) { input.value = ""; toast("这个标签已经有了", "info"); return; }
  state.detailTagFocus = true;
  await writeDetailTags([...tags, value]);
}

async function removeDetailTag(index) {
  const work = findWork(state.workDetailId);
  if (!work) return;
  const tags = detailTagList(work);
  if (index < 0 || index >= tags.length) return;
  await writeDetailTags(tags.filter((_, position) => position !== index));
}

/**
 * 把「此刻屏幕上那个列表」重新拉一遍（只取数据，不重画）。
 *
 * 六个列表页各有各的数据源，漏掉一个就会出现「改完得先换个页面再回来才对」——
 * v1.2.9 用户报的「重新下载 EPUB 版并绑定后封面带图版角标不刷新」就是这么来的：
 * 当时只照顾了作者库和所有作品，站在系列页 / 收藏夹 / 浏览历史 / 待补工作台上都刷不到。
 */
async function refreshVisibleList() {
  if (state.activeAuthor) {
    await refreshWorks();
    // 系列页的作品列表来自 list_series_works，是另一份数据，不一起刷还是旧的
    await refreshSeriesView();
    return;
  }
  if (state.homeView === "allWorks") await refreshAllWorks();
  else if (state.homeView === "filterViews") await refreshAllWorks();
  else if (state.homeView === "collections") {
    await refreshCollections();
    if (state.activeCollection) await refreshCollectionWorks();
  } else if (state.homeView === "history") await refreshHistory();
  else if (state.homeView === "missingFull") await refreshMissingFull();
}

/**
 * 详情弹窗里做完「会影响卡片显示」的改动之后收尾：
 * 刷新当前这一页的列表 + 重画详情（**不关弹窗**）。
 */
async function refreshAfterDetailChange() {
  await refreshActiveAuthor();
  await refreshVisibleList();
  render();
  refreshDetailDom();
}

/** 详情里切「完整版 / 预览版」：不关弹窗，改完原地刷新（原来的菜单版是关掉弹窗再刷） */
async function detailSetVersion(workId, asFull) {
  captureDetailNote();
  try {
    await invoke(asFull ? "mark_work_as_full" : "mark_work_as_preview", { workId });
  } catch (error) {
    toast(String(error), "error");
    return;
  }
  await refreshAfterDetailChange();
  toast(asFull ? "已设为完整版" : "已设为预览版", "success");
}

/** 详情里绑定完整版文件 */
async function detailBindFile(workId) {
  captureDetailNote();
  const path = await open({
    directory: false,
    multiple: false,
    filters: [{ name: "作品文件", extensions: ["txt", "md", "html", "htm", "xhtml", "epub", "mobi", "azw", "azw3", "fb2", "lit", "pdf"] }],
  });
  if (!path) return;
  try {
    await invoke("bind_work", { workId, path });
  } catch (error) {
    toast(String(error), "error");
    return;
  }
  state.detailReadingPath = await invoke("work_reading_path", { workId }).catch(() => "");
  await refreshAfterDetailChange();
  toast("已绑定本地完整版内容", "success");
}

/** 详情里切带图版 */
async function detailToggleImages(workId) {
  captureDetailNote();
  await invoke("toggle_has_images", { workId });
  await refreshAfterDetailChange();
}

/** 详情里的「重新下载 HTML / EPUB / TXT 版并绑定」。跑的是原来菜单里那三个按钮的老逻辑。 */
async function detailDownload(workId, format) {
  if (format === "txt") { await redownloadNovelTxt(workId); return; }
  await downloadReadingVersion(workId, format === "epub" ? "epub" : "html");
}

/** 详情里的删除作品。详情可以从任意页面打开，所以用 findWork 找，别只看 state.works。 */
function detailDeleteWork(workId) {
  const work = findWork(workId);
  if (!work) return;
  confirmAction("确认删除作品", `删除“${work.title}”只会移除软件记录和路径绑定，不会删除磁盘中的原始文件。`, "删除作品", async () => {
    await invoke("delete_work", { workId });
    closeModal();
    state.workDetailId = null;
    await refreshAfterDetailChange();
    toast("作品记录已删除，原始文件未受影响", "success");
  });
}

function workDetailBody() {
  const work = findWork(state.workDetailId);
  if (!work) return '<p class="match-note">这篇作品已经不在了，关掉重新打开一次。</p>';
  const rating = Number(work.rating) || 0;
  const readState = Number(work.readState) || 0;
  const collectionNames = (state.detailCollectionIds || [])
    .map((id) => state.collections.find((item) => item.id === id)?.name)
    .filter(Boolean);
  const tags = String(work.tags || "").split("|").map((tag) => tag.trim()).filter(Boolean);
  const stars = [1, 2, 3, 4, 5].map((value) => `<button class="detail-star ${value <= rating ? "is-on" : ""}" title="给 ${value} 星" data-action="detail-rate" data-rating="${value}">${icon(value <= rating ? "starFilled" : "star", 19)}</button>`).join("");
  const seriesText = work.seriesId && work.seriesTitle
    ? `${escapeHtml(work.seriesTitle)}${work.seriesOrder > 0 ? ` · 第 ${work.seriesOrder} 篇` : ""}`
    : "未加入系列";
  const cover = work.coverPath
    ? `<img src="${asset(work.coverPath)}" alt="${escapeHtml(work.title)} 的封面" onerror="this.remove()">`
    : `<span class="detail-cover-placeholder">${icon("image", 30)}</span>`;
  const pixivUrl = work.pixivNovelId ? `https://www.pixiv.net/novel/show.php?id=${work.pixivNovelId}` : "";
  const authorName = workAuthorName(work);
  // 副标题：日期 · 完整版/预览版 · 字数（或格式）· 配图数
  const meta = [dateLabel(work.releaseDate), work.purchasedPath ? "完整版" : "预览版", ...workFormatLabels(work)];
  // 简介在 Pixiv 那边是 HTML（`<br />`、`<a>`…），这里取纯文本判长度、取净化后的 HTML 去显示
  const synopsis = synopsisPlainText(work.synopsis);
  // 只有长到会顶掉下面内容时才给「展开/收起」，短简介直接铺开
  const synopsisLong = synopsis.length > 140;
  const synopsisOpen = !synopsisLong || state.detailSynopsisOpen;
  const readingPath = String(state.detailReadingPath || "");
  // 简介有三种状态，后两种 `synopsis` 都是空串，只能靠 synopsisChecked 分开说：
  // 有内容 / 问过 Pixiv 且确认作者没写 / 还没问过（点了「补抓简介」能拉）。
  // 混成一句「这篇还没有简介」的话，作者明明没写的作品会让用户一直去点补抓 —— 白等还喂风控。
  const synopsisBody = synopsis
    ? `<p class="detail-synopsis ${synopsisOpen ? "" : "is-clamped"}">${synopsisHtml(work.synopsis)}</p>${synopsisLong ? `<button class="quiet-button detail-more" data-action="detail-toggle-synopsis">${state.detailSynopsisOpen ? "收起简介" : "展开全文"}</button>` : ""}`
    : work.synopsisChecked
      ? '<p class="match-note">作者没写简介 —— 这篇已经向 Pixiv 问过了，那边本来就没有简介内容。</p>'
      : '<p class="match-note">这篇还没有简介。同步过的作品可以在设置里用「补抓简介」拉一次。</p>';
  // 阅读版和正文指向同一个文件时就不必单列一行
  const showReadingRow = Boolean(readingPath) && readingPath !== work.purchasedPath && readingPath !== work.previewPath;

  return `
    <div class="detail-head">
      <div class="detail-cover">${cover}</div>
      <div class="detail-head-copy">
        <h3>${escapeHtml(work.title)}</h3>
        <p class="detail-author">${authorName ? `<button class="work-author is-link" data-action="detail-open-author" data-author-id="${work.authorId}">${escapeHtml(authorName)}</button>` : "未知作者"}${meta.map((item) => `<span class="detail-sep">·</span><span>${escapeHtml(item)}</span>`).join("")}</p>
        <div class="detail-quick">
          <button class="primary-button" data-action="detail-open-work" data-work-id="${work.id}">${icon("arrow", 18)}<span>打开${work.purchasedPath ? "完整版" : "预览版"}</span></button>
        </div>
      </div>
    </div>

    <section class="detail-block">
      <h4>简介</h4>
      ${synopsisBody}
    </section>

    <section class="detail-block">
      <h4>标签</h4>
      ${tags.length ? `<div class="detail-tags is-editable">${tags.map((tag, index) => `<span>${icon("tag", 12)}${escapeHtml(tag)}<button class="detail-tag-remove" title="删除标签「${escapeHtml(tag)}」" data-action="detail-remove-tag" data-index="${index}">${icon("x", 12)}</button></span>`).join("")}</div>` : '<p class="match-note">还没有标签。</p>'}
      <div class="detail-tag-add"><input id="work-detail-tag-input" type="text" maxlength="40" placeholder="输入标签后按 Enter 添加" autocomplete="off"></div>
    </section>

    <section class="detail-block">
      <h4>系列</h4>
      <p class="detail-plain">${seriesText}</p>
      ${state.activeAuthor ? `<button class="quiet-button" data-action="detail-series" data-work-id="${work.id}">${icon("series", 16)}调整系列</button>` : ""}
    </section>

    <section class="detail-block">
      <h4>我的记录</h4>
      <div class="detail-row">
        <span class="detail-label">阅读状态</span>
        <div class="filter-group" role="group" aria-label="阅读状态">${READ_STATE_LABELS.map((label, value) => `<button class="filter-button ${readState === value ? "is-active" : ""}" data-action="detail-read-state" data-read-state="${value}">${label}</button>`).join("")}</div>
      </div>
      <div class="detail-row">
        <span class="detail-label">我的评分</span>
        <div class="detail-stars">${stars}<button class="quiet-button detail-clear-rating ${rating ? "" : "is-hidden"}" data-action="detail-rate" data-rating="0">清除</button></div>
      </div>
      <div class="detail-row is-stacked">
        <span class="detail-label">一句话笔记</span>
        <textarea id="work-detail-note" maxlength="200" rows="3" placeholder="读完想说点什么？失焦即保存，最多 200 字">${escapeHtml(work.note || "")}</textarea>
      </div>
    </section>

    <section class="detail-block">
      <h4>收藏夹 <button class="quiet-button" data-action="detail-pick-collection" data-work-id="${work.id}">调整</button></h4>
      ${collectionNames.length ? `<div class="detail-tags">${collectionNames.map((name) => `<span>${icon("folderHeart", 12)}${escapeHtml(name)}</span>`).join("")}</div>` : '<p class="match-note">还没有收进任何收藏夹。</p>'}
    </section>

    <section class="detail-block">
      <h4>文件 <button class="quiet-button" data-action="detail-open-directory" data-work-id="${work.id}">${icon("folder", 16)}打开所在目录</button></h4>
      <dl class="detail-paths">
        ${detailPathRow("完整版", work.purchasedPath)}
        ${detailPathRow("预览版", work.previewPath)}
        ${showReadingRow ? `<div class="detail-path"><dt>阅读版</dt><dd><span class="detail-path-text" title="${escapeHtml(readingPath)}">${escapeHtml(readingPath)}</span><span class="detail-path-actions"><button class="quiet-button" data-action="detail-open-reading" data-work-id="${work.id}">打开阅读版</button><button class="quiet-button" data-action="detail-open-dir" data-path="${escapeHtml(readingPath)}">所在目录</button></span></dd></div>` : ""}
        ${detailPathRow("封面", work.coverPath)}
      </dl>
      <div class="detail-actions">
        <button class="quiet-button" data-action="detail-set-version" data-work-id="${work.id}" data-as-full="${work.purchasedPath ? "0" : "1"}">${icon(work.purchasedPath ? "file" : "check", 16)}${work.purchasedPath ? "设为预览版" : "设为完整版"}</button>
        <button class="quiet-button" data-action="detail-bind-file" data-work-id="${work.id}">${icon("folder", 16)}绑定完整版文件</button>
        <button class="quiet-button" data-action="detail-toggle-images" data-work-id="${work.id}">${icon("image", 16)}${work.hasImages ? "取消带图版" : "设为带图版"}</button>
      </div>
    </section>

    <section class="detail-block">
      <h4>Pixiv</h4>
      ${work.pixivNovelId ? `<p class="detail-plain">作品 ID：${escapeHtml(work.pixivNovelId)}</p>
      <div class="detail-actions">
        <button class="quiet-button" data-action="detail-open-pixiv" data-url="${escapeHtml(pixivUrl)}">${icon("link", 16)}在浏览器里打开原页</button>
        <button class="quiet-button" data-action="detail-download" data-work-id="${work.id}" data-format="html">${icon("file", 16)}重新下载 HTML 版并绑定</button>
        <button class="quiet-button" data-action="detail-download" data-work-id="${work.id}" data-format="epub">${icon("file", 16)}重新下载 EPUB 版并绑定</button>
        <button class="quiet-button" data-action="detail-download" data-work-id="${work.id}" data-format="txt">${icon("file", 16)}重新下载 TXT 版并绑定</button>
      </div>` : '<p class="match-note">这篇没有记录 Pixiv 作品 ID。</p>'}
    </section>`;
}

/**
 * 从详情弹窗开二次弹窗（收藏夹 / 标签 / 系列）时**不关详情弹窗** ——
 * 弹窗本来就能嵌套，关掉再开只会让人找不回来。
 */
async function openDetailPicker(workId) {
  state.pickerWorkId = workId;
  state.pickerSelected = await invoke("work_collections", { workId });
  showModal(modal("收藏到…", `<div id="collection-picker-body">${collectionPickerBody()}</div>`, '<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="picker-save">确定</button>'));
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
  const works = collapseSerialWorks(state.collectionWorks).filter(matchesReadFilter).filter(matchesAdvancedFilters);
  const cards = works.map((work) => linkedWorkCard(work)).join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="返回收藏夹列表" data-action="back-to-collections">${icon("back", 20)}</button><div><p class="section-kicker">收藏夹</p><h1>${escapeHtml(collection.name)}</h1></div></div>
      <div class="topbar-actions"><button class="icon-text-button" data-action="export-collection-epub" data-collection-id="${collection.id}" data-collection-name="${escapeHtml(collection.name)}" title="把这个收藏夹里的作品打成一整本 EPUB">${icon("download", 18)}<span>合成一本 EPUB</span></button><button class="icon-text-button" data-action="export-collection" data-collection-id="${collection.id}">${icon("download", 18)}<span>导出清单</span></button><button class="icon-text-button" data-action="rename-collection" data-collection-id="${collection.id}">${icon("settings", 18)}<span>重命名</span></button><button class="quiet-button" data-action="delete-collection" data-collection-id="${collection.id}">删除收藏夹</button></div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="collection-search" type="search" placeholder="${searchPlaceholder()}" value="${escapeHtml(state.collectionQuery)}" autocomplete="off"></label>
        ${searchFieldSelect(state.searchField)}
        <div class="filter-group" role="group" aria-label="版本状态">${STATUS_FILTERS.map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button images-filter ${state.imagesFilter === "has" ? "is-active" : ""}" data-action="images-only" title="只看有配图的作品（等同高级筛选里的「配图 · 有图」）">${icon("image", 17)}<span>仅看带图版</span></button>
        ${serialFilterButton(state.collectionWorks)}
        ${filterButton()}
        <select class="sort-select" id="collection-sort" aria-label="排序"><option value="added_desc" ${state.collectionSort === "added_desc" ? "selected" : ""}>最近收藏</option><option value="date_desc" ${state.collectionSort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.collectionSort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.collectionSort === "title_asc" ? "selected" : ""}>名称 A-Z</option><option value="words_desc" ${state.collectionSort === "words_desc" ? "selected" : ""}>字数从多到少</option><option value="rating_desc" ${state.collectionSort === "rating_desc" ? "selected" : ""}>评分从高到低</option></select>
      </div>
      ${filterPanel()}
      <div class="works-grid">${cards || `<div class="empty-state works-empty"><h2>${state.collectionQuery || state.status !== "all" || state.imagesFilter !== "all" ? "没有符合条件的作品" : "这个收藏夹还是空的"}</h2><p>在任意作品卡上点心形图标，就能把它收进来。</p></div>`}</div>
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

/* ---------------------------- 筛选模板的命名弹窗 ---------------------------- */

function filterViewNameModal(view = null) {
  const editing = Boolean(view);
  const summary = editing ? "" : `<p class="match-note">会把<strong>当前这套条件</strong>存下来：${escapeHtml(filterViewSummary({ payload: currentFilterPayload() }))}。它存的是条件不是作品，点开时按当时那套条件现算。</p>`;
  showModal(modal(editing ? "重命名筛选模板" : "存为筛选模板",
    `<div class="form-stack"><label>模板名字 <input id="filter-view-name-input" maxlength="30" value="${escapeHtml(view?.name || "")}" placeholder="例如：待读长篇、8 万字以上未读" autocomplete="off"></label></div>${summary}`,
    `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="${editing ? "submit-filter-view-rename" : "submit-filter-view"}" data-view-id="${view?.id || 0}">${editing ? "保存" : "存下来"}</button>`));
  window.requestAnimationFrame(() => document.querySelector("#filter-view-name-input")?.focus());
}

async function submitFilterViewName() {
  const input = document.querySelector("#filter-view-name-input");
  const name = (input?.value || "").trim();
  if (!name) { toast("先给模板起个名字", "error"); input?.focus(); return; }
  try {
    const created = await invoke("save_filter_view", { name, payload: currentFilterPayload() });
    closeModal();
    state.homeView = "filterViews";
    await refreshFilterViews();
    render();
    toast(`已存下筛选模板「${created?.name || name}」`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

async function submitFilterViewRename(viewId) {
  const input = document.querySelector("#filter-view-name-input");
  const name = (input?.value || "").trim();
  if (!name) { toast("模板名字不能为空", "error"); input?.focus(); return; }
  try {
    await invoke("rename_filter_view", { id: Number(viewId) || 0, name });
    closeModal();
    await refreshFilterViews();
    render();
    toast("筛选模板已改名", "success");
  } catch (error) {
    toast(String(error), "error");
  }
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

/* ==================== 待补完整版工作台（v1.2.7） ==================== */

/** 工作台上那排「处理状态」筛选项：值 / 文案 */
const MISSING_FULL_FILTERS = [["todo", "未处理"], ["searched", "已找过·没有"], ["skipped", "不打算补"], ["all", "全部"]];

function missingFullStateLabel(work) {
  if (Number(work.needFullState || 0) === 1) return "已找过·没有";
  if (Number(work.needFullState || 0) === 2) return "不打算补";
  return "未处理";
}

/**
 * 「什么时候处理的」拆成短文案 + 精确时间（v1.2.8）。
 *
 * 标过「已找过·没有」/「不打算补」都会记下时间，卡片上给个「3 天前」就够了，
 * 精确到分放在 title 里 —— 这功能的全部意义是**过俩月翻到一篇时想得起来找没找过**。
 * 超过 30 天就不再说「N 天前」了，直接给日期，那时候「37 天前」已经不好换算了。
 */
function missingFullMarked(value) {
  if (!value) return null;
  const time = new Date(value);
  if (Number.isNaN(time.getTime())) return null;
  const pad = (number) => String(number).padStart(2, "0");
  const absolute = `${time.getFullYear()}-${pad(time.getMonth() + 1)}-${pad(time.getDate())} ${pad(time.getHours())}:${pad(time.getMinutes())}`;
  const minutes = Math.floor((Date.now() - time.getTime()) / 60000);
  if (minutes < 1) return { label: "刚刚", absolute };
  if (minutes < 60) return { label: `${minutes} 分钟前`, absolute };
  if (minutes < 1440) return { label: `${Math.floor(minutes / 60)} 小时前`, absolute };
  if (minutes < 43200) return { label: `${Math.floor(minutes / 1440)} 天前`, absolute };
  return { label: absolute, absolute };
}

/**
 * 作者头像：作者库里有头像就用那张，没有再摆首字（v1.2.8 起待补工作台也用它）。
 * `author` 允许是 undefined —— 待补工作台手上只有 `work.authorId`，要先去查作者库，
 * 真查不到（作者被删了？）就按传进来的名字摆首字，别让整块渲染挂掉。
 */
function authorAvatar(author, fallbackName = "") {
  const url = asset(author?.avatarPath || "");
  const name = author?.name || fallbackName || "未知作者";
  return `<div class="author-avatar ${url ? "has-image" : ""}">${url ? `<img src="${url}" alt="${escapeHtml(name)} 的头像">` : `<span>${escapeHtml(initials(name))}</span>`}</div>`;
}

/** 按 id 去作者库捞人。`bootstrap()` 第一步就 `await refreshAuthors()`，所以一定捞得到。 */
function authorById(authorId) {
  return state.authors.find((item) => Number(item.id) === Number(authorId)) || null;
}

/**
 * 待补完整版工作台：把「只有预览版、还没有完整版」的作品按作者摊开。
 *
 * 它存在的全部理由就一条 —— **别让同一篇被反复找一遍**。
 * 每篇可以标「已找过·没有」或「不打算补」，默认视图只看未处理的；
 * 另外，在详情里绑定完整版之后它自己就会从这儿消失（`purchased_path` 一有值就不算缺口了）。
 */
function renderMissingFull() {
  const all = state.missingFull;
  const countBy = (list, value) => list.filter((work) => Number(work.needFullState || 0) === value).length;
  const pending = countBy(all, 0);
  const activeId = state.missingFullAuthorId;
  if (activeId === null) return renderMissingFullAuthors(all, pending);

  // ——— 第二层：某位作者的待补作品 ———
  const filter = state.missingFullFilter;
  const mine = all.filter((work) => Number(work.authorId) === Number(activeId));
  const authorName = mine[0]?.authorName || "未知作者";
  const visible = missingFullVisibleWorks();
  const minePending = countBy(mine, 0);
  const counts = { todo: minePending, searched: countBy(mine, 1), skipped: countBy(mine, 2), all: mine.length };
  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="返回作者列表" data-action="back-to-missing-authors">${icon("back", 20)}</button><div><p class="section-kicker">待补完整版</p><h1>${escapeHtml(authorName)}</h1></div></div>
      <div class="topbar-actions">${state.missingFullBulk ? `<strong class="bulk-count">已选 ${state.selectedWorkIds.size} 篇</strong>` : `<button class="icon-text-button" data-action="missing-bulk-enter" ${visible.length ? "" : "disabled"}>${icon("check", 18)}<span>批量标记</span></button>`}</div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <div class="filter-group" role="group" aria-label="处理状态">${MISSING_FULL_FILTERS.map(([value, label]) => `<button class="filter-button ${filter === value ? "is-active" : ""}" data-action="missing-full-filter" data-filter="${value}">${label} (${counts[value]})</button>`).join("")}</div>
      </div>
      ${state.missingFullBulk ? missingBulkBar(visible) : ""}
      <div class="read-only-note">这位作者有 <strong>${mine.length}</strong> 篇只有预览版，其中 <strong>${minePending}</strong> 篇还没处理。标过「已找过·没有」或「不打算补」的就不会再混在待办里。${state.missingFullBulk ? "（批量标记模式：点卡片勾选，也可以按住鼠标在列表上拖框选，然后点上面的按钮一次标一批。）" : ""}</div>
      <div class="works-grid">${visible.map(missingFullCard).join("") || `<div class="empty-state works-empty"><div class="empty-icon">${icon("archive", 26)}</div><h2>这个状态下没有作品</h2><p>切到上面别的状态看看。</p></div>`}</div>
    </section>`);
}

/**
 * 「待补完整版」第一层：按作者摊开。作品动辄上百篇、作者却只有几十位，
 * 先看作者一眼就知道该去补谁，直接铺作品列表会淹掉重点（v1.2.8 用户要求）。
 */
function renderMissingFullAuthors(all, pending) {
  const grouped = new Map();
  all.forEach((work) => {
    const key = Number(work.authorId) || 0;
    if (!grouped.has(key)) grouped.set(key, { id: key, name: work.authorName || "未知作者", total: 0, todo: 0 });
    const group = grouped.get(key);
    group.total += 1;
    if (Number(work.needFullState || 0) === 0) group.todo += 1;
  });
  // 待办多的排前面；一样多就按缺口总数，再按名字 —— 打开就该先看到最该动手的那位
  const authors = [...grouped.values()].sort((left, right) => right.todo - left.todo || right.total - left.total || left.name.localeCompare(right.name));
  const cards = authors
    .map(
      (author) => `
    <article class="missing-author-card" data-action="open-missing-author" data-author-id="${author.id}" tabindex="0">
      <div class="author-avatar-wrap">${authorAvatar(authorById(author.id), author.name)}</div>
      <div class="author-card-body">
        <div class="author-card-title-row"><h2 title="${escapeHtml(author.name)}">${escapeHtml(author.name)}</h2></div>
        <p class="missing-author-note">缺口 <strong>${author.total}</strong> 篇 · 未处理 <strong>${author.todo}</strong> 篇</p>
      </div>
      <span class="card-enter">${icon("arrow", 17)}</span>
    </article>`,
    )
    .join("");
  return renderShell(`
    <section class="topbar work-topbar">
      <div><p class="section-kicker">收藏体检</p><h1>待补完整版</h1></div>
    </section>
    <section class="library-content">
      <div class="read-only-note">这里列的是「只有预览版、还没有完整版」的作品所对应的作者：共 <strong>${authors.length}</strong> 位、缺 <strong>${all.length}</strong> 篇，其中 <strong>${pending}</strong> 篇还没处理。点作者进去看具体是哪些作品，标过「已找过·没有」或「不打算补」的就不会再混在待办里。</div>
      <div class="author-grid">${cards || `<div class="empty-state works-empty"><div class="empty-icon">${icon("archive", 26)}</div><h2>没有缺完整版的作品</h2><p>库里每一篇都绑好完整版了。</p></div>`}</div>
    </section>`);
}

/** 工作台上的一张卡：和普通作品卡一样，只是下面多了「标记」那一行 */
function missingFullCard(work) {
  const value = Number(work.needFullState || 0);
  const mark = (state_, label) => `<button class="chip-button" data-action="mark-missing-full" data-work-id="${work.id}" data-state="${state_}">${label}</button>`;
  const marked = missingFullMarked(work.needFullMarkedAt);
  // 批量标记模式下右下角让给勾选框（和作者作品库那套 selection-badge 用同一个样式）
  const selected = state.selectedWorkIds.has(work.id);
  const corner = state.missingFullBulk
    ? `<button class="selection-badge ${selected ? "is-selected" : ""}" title="${selected ? "取消选择" : "选择作品"}" data-action="missing-toggle-select" data-work-id="${work.id}">${selected ? icon("check", 16) : ""}</button>`
    : `${workReadToggle(work)}${workMenuButton(work)}`;
  return `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"} ${state.missingFullBulk ? "is-selecting" : ""}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${corner}
      </div>
      <div class="work-copy">
        <button class="work-author is-link" title="打开「${escapeHtml(work.authorName || "")}」的作品库" data-action="open-author" data-author-id="${work.authorId}">${escapeHtml(work.authorName || "")}</button>
        <div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}${workReadDot(work)}${workRatingMark(work)}</div>
        <h2 class="work-open" title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>
        ${workSeries(work)}${workTags(work)}
        <div class="missing-mark"><span class="missing-state">${missingFullStateLabel(work)}</span>${marked ? `<span class="missing-time" title="${escapeHtml(marked.absolute)}">${escapeHtml(marked.label)}处理</span>` : ""}${value === 0 ? `${mark(1, "已找过·没有")}${mark(2, "不打算补")}` : `<button class="chip-button" data-action="mark-missing-full" data-work-id="${work.id}" data-state="0">恢复未处理</button>`}</div>
      </div>
    </article>`;
}

/**
 * 待补工作台第二层当前**看得见**的那些作品（按作者 + 状态档筛过）。
 * 「全选本页」和批量标记后的收尾判断都走它，免得两处各写一份筛法。
 */
function missingFullVisibleWorks() {
  const activeId = state.missingFullAuthorId;
  if (activeId === null) return [];
  const filter = state.missingFullFilter;
  return state.missingFull
    .filter((work) => Number(work.authorId) === Number(activeId))
    .filter((work) => {
      if (filter === "all") return true;
      if (filter === "todo") return Number(work.needFullState || 0) === 0;
      if (filter === "searched") return Number(work.needFullState || 0) === 1;
      return Number(work.needFullState || 0) === 2;
    });
}

/**
 * 待补工作台第二层顶上的「批量标记」条（v1.2.9）。
 *
 * 真实用法是「这个作者的合集我找到了 → 他名下这二十篇一并标掉」，
 * 一篇篇点两个按钮就是四十次点击。勾选走的还是 `state.selectedWorkIds`。
 */
function missingBulkBar(works) {
  const count = state.selectedWorkIds.size;
  const disabled = count ? "" : "disabled";
  const allSelected = works.length > 0 && works.every((work) => state.selectedWorkIds.has(work.id));
  return `<section class="bulk-bar" aria-label="批量标记待补状态">
      <div class="bulk-bar-group is-lead">
        <strong class="bulk-count">已选 ${count} 篇</strong>
        <button class="quiet-button" data-action="missing-select-all" ${works.length ? "" : "disabled"}>${allSelected ? "取消全选" : "全选本页"}</button>
        <button class="quiet-button" data-action="missing-bulk-exit">退出批量</button>
      </div>
      <div class="bulk-bar-group"><span class="bulk-bar-label">处理状态</span><button class="bulk-button" data-action="missing-bulk-mark" data-state="1" ${disabled}>已找过·没有</button><button class="bulk-button" data-action="missing-bulk-mark" data-state="2" ${disabled}>不打算补</button><button class="bulk-button" data-action="missing-bulk-mark" data-state="0" ${disabled}>恢复未处理</button></div>
    </section>`;
}

function renderSeriesWorkCards(works) {
  return works.map((work, index) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        ${workBadges(work)}
        <div class="work-links">${workLinkBadge(work)}</div>
        ${workReadToggle(work)}${workMenuButton(work)}
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}${workReadDot(work)}${workRatingMark(work)}</div><h2 class="work-open" title="${escapeHtml(work.title)}"><span class="work-index">${work.seriesOrder || index + 1}.</span>${escapeHtml(work.title)}</h2>${workSeries(work)}${workTags(work)}</div>
    </article>`).join("");
}

/**
 * 系列缺哪几号（v1.2.9）。
 *
 * 只算序号 ≥ 1 的：序号 0 是「还没排进系列」，不是「第 0 篇」。
 * 序号唯一是 set_work_series 那边保证的（占位会被拒），所以这里只找空号、不查重。
 * 后端 `list_series` 也算了一份（`gapOrders`），那是给总览卡片用的 ——
 * 这一层手里有全部作品，就地算更省一次请求。
 */
function missingSeriesOrders(works) {
  const present = new Set(
    works.map((work) => Number(work.seriesOrder) || 0).filter((order) => order > 0),
  );
  if (!present.size) return [];
  const max = Math.max(...present);
  const missing = [];
  for (let order = 1; order <= max; order += 1) {
    if (!present.has(order)) missing.push(order);
  }
  return missing;
}

/** 「缺第 3、7 篇」那种一句人话；序号多了就折成「第 3 篇 等 6 处」 */
function seriesGapLabel(orders) {
  if (!orders.length) return "";
  if (orders.length > 3) {
    return `缺第 ${orders[0]} 篇 等 ${orders.length} 处`;
  }
  return `缺第 ${orders.join("、")} 篇`;
}

/**
 * 系列进度：「已读」和「在读」都算**读过**。
 *
 * 外部阅读器读完不会回调，打开一篇只会把它标成「在读」；如果只数「已读」，
 * 进度就永远停在 0（用户报的正是「这个已读一直是零」）。
 * 口径含「在读」之后，点一次「继续读」进度就 +1，不用再手动补标。
 */
function seriesReadCount(works) {
  return works.filter((work) => Number(work.readState || 0) > 0).length;
}

function renderSeriesView() {
  const author = state.activeAuthor;
  if (state.seriesView.kind === "detail") {
    const readCount = seriesReadCount(state.seriesItems);
    const total = state.seriesItems.length;
    // 「继续读」找的是完全没碰过的第一篇（readState = 0），读过的就不再回头
    const nextUnread = state.seriesItems.find((work) => Number(work.readState || 0) === 0);
    const gaps = missingSeriesOrders(state.seriesItems);
    return renderShell(`
      <section class="topbar work-topbar">
        <div class="crumb-heading"><button class="back-button" title="返回系列作品" data-action="close-series-view">${icon("back", 20)}</button><div><p class="section-kicker">系列作品</p><h1>${escapeHtml(state.seriesView.title)}</h1></div></div>
        <div class="topbar-actions">
          <span class="series-progress" title="系列里读过的篇数（已读 + 在读都算）">读过 ${readCount} / ${total}</span>
          ${gaps.length ? `<span class="series-gap" title="按序号算出来缺的篇目：第 ${gaps.join("、")} 篇">${icon("info", 15)}<span>${seriesGapLabel(gaps)}</span></span>` : ""}
          <button class="icon-text-button" data-action="export-series-epub" data-series-id="${escapeHtml(state.seriesView.id)}" data-series-title="${escapeHtml(state.seriesView.title)}" title="把这个系列里的作品打成一整本 EPUB">${icon("download", 18)}<span>合成一本 EPUB</span></button>
          <button class="primary-button" data-action="series-continue" title="${nextUnread ? `接着打开：${escapeHtml(nextUnread.title)}` : "这个系列都读过了"}" ${nextUnread ? "" : "disabled"}>${icon("arrow", 18)}<span>${readCount ? "继续读" : "从头读"}</span></button>
        </div>
      </section>
      <section class="library-content"><div class="works-grid">${renderSeriesWorkCards(state.seriesItems) || '<div class="empty-state works-empty"><h2>该系列没有作品</h2></div>'}</div></section>`);
  }
  const cards = state.seriesItems.map((series) => `<article class="series-card" data-action="open-series-card" data-series-id="${escapeHtml(series.id)}" data-series-title="${escapeHtml(series.title)}" tabindex="0">${series.coverPath ? `<img src="${asset(series.coverPath)}" alt="${escapeHtml(series.title)} 的封面">` : `<div class="series-card-placeholder">${icon("series", 30)}</div>`}<div class="series-card-copy"><h2>${escapeHtml(series.title)}</h2><p><strong>${series.workCount}</strong> 部作品 · 完整版 ${series.purchasedCount} · 预览版 ${series.previewCount}</p>${series.readCount ? `<p class="series-read">读过 ${series.readCount} / ${series.workCount}</p>` : ""}${(series.gapOrders || []).length ? `<p class="series-gap-line">${icon("info", 13)}<span>${seriesGapLabel(series.gapOrders)}</span></p>` : ""}</div></article>`).join("");
  return renderShell(`
    <section class="topbar work-topbar"><div class="crumb-heading"><button class="back-button" title="返回作品库" data-action="close-series-view">${icon("back", 20)}</button><div><p class="section-kicker">作者作品库</p><h1>系列作品</h1></div></div></section>
    <section class="library-content"><div class="series-grid">${cards || '<div class="empty-state works-empty"><div class="empty-icon">' + icon("series", 26) + '</div><h2>还没有系列作品</h2><p>同步到的系列作品会显示在这里。</p></div>'}</div></section>`);
}

/**
 * 系列连续读：接着打开这个系列里第一篇完全没碰过的作品（`read_state = 0`）。
 *
 * 打开之后把系列数据重拉一遍 —— `open_work` 会顺手把那篇标成「在读」，
 * 而「读过」的口径含「在读」，所以重画后进度 +1、按钮指向的「下一篇」也自动往前挪一格。
 * 系列里没有 `read_state = 0` 的篇时按钮是 disabled 的，这里再兜一次底。
 */
async function continueSeriesReading() {
  const view = state.seriesView;
  if (!view) return;
  const next = state.seriesItems.find((work) => Number(work.readState || 0) === 0);
  if (!next) {
    toast("这个系列已经全部读过了", "info");
    return;
  }
  const position = next.seriesOrder || state.seriesItems.indexOf(next) + 1;
  try {
    await invoke("open_work", { workId: next.id });
    await openSeriesDetail(view.id, view.title, view.returnTo);
    toast(`接着读第 ${position} 篇`, "info");
  } catch (error) {
    toast(String(error), "error");
  }
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
 * 卡片上选中文字后右键：给的是「搜索」菜单 —— 把选中那截文字丢到设置里配好的网站上搜去。
 * 完整版是从别的网站弄来的，这功能就是为它准备的；没配网站时给个直达设置的入口，不留空菜单。
 * 文本要在弹菜单的那一刻就存下来 —— 点菜单按钮时浏览器已经把选区清掉了。
 */
function cardSearchMenu(work, selectedText) {
  state.copyPayload = { text: selectedText };
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

// 原来这里的 workMenu（作品卡「三个点」菜单）已删除（v1.2.1）：
// 点三个点直接开作品详情弹窗，菜单里的每一项都搬进详情页对应区块了。
// 唯一没保留的是「打开阅读版」的旧判断条件（只在绑定文件是 html/epub 时才给）——
// 现在改由后端 work_reading_path 查真实存在的阅读版文件，比按扩展名猜更准。

// 原来这里的 editTagsModal（作品卡右键 →「编辑标签」弹窗）已删除（v1.2.2）：
// 加 / 删标签改成详情页「标签」块里就地做（addDetailTag / removeDetailTag）；
// 弹窗底部那个「打开本地目录」详情页「文件」块本来就有；「保存标签」按钮不再需要 ——
// 现在是改一下存一下，不会有「攒着改完忘了保存」这种事。

async function refreshSeriesView() {
  if (!state.seriesView) return;
  if (state.seriesView.kind === "detail") {
    state.seriesItems = await invoke("list_series_works", { authorId: state.activeAuthor.id, seriesId: state.seriesView.id });
  } else {
    state.seriesItems = await invoke("list_series", { authorId: state.activeAuthor.id });
  }
}

async function leaveWorkSeries(workId) {
  await invoke("leave_work_series", { authorId: state.activeAuthor.id, workId });
  closeModal();
  await refreshWorks();
  await refreshSeriesView();
  render();
  toast("作品已退出系列", "success");
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
    showModal(modal("更改系列序号", `<form id="series-form" class="form-stack"><input type="hidden" name="seriesId" value="${escapeHtml(work.seriesId)}"><label>当前系列<input value="${escapeHtml(current.title)}" readonly></label><label>系列序号<select name="seriesOrder">${seriesOrderOptions(current.maxOrder, selectedOrder)}</select><small>不能与同一系列中的其他作品重复。</small></label></form>`, '<button class="quiet-button" data-action="close-modal">取消</button><button class="danger-button" data-action="leave-series" data-work-id="' + workId + '">退出此系列</button><button class="primary-button" data-action="set-work-series" data-work-id="' + workId + '">保存</button>'));
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
  // 简介里的外链（Pixiv 简介本身是 HTML）：点了交给系统浏览器，
  // 别让 webview 自己导航过去 —— 在应用窗口里跳走会把整个界面顶掉。捕获阶段拦，监听只绑一次。
  if (!window.__detailLinksBound) {
    window.__detailLinksBound = true;
    document.addEventListener("click", (event) => {
      const link = event.target?.closest?.("a.detail-link");
      if (!link) return;
      event.preventDefault();
      openExternalUrl(link.getAttribute("href") || "").catch((error) => toast(String(error), "error"));
    }, true);
  }

  // 右下角「回到顶部 / 滚到底部」：滚动或窗口变化时刷新可用状态（监听只绑一次）
  if (!window.__scrollJumpBound) {
    window.__scrollJumpBound = true;
    window.addEventListener("scroll", updateScrollJump, { passive: true });
    window.addEventListener("resize", updateScrollJump);
  }
  updateScrollJump();
  // 「所有作品」瀑布流的哨兵：每次重画都是一个新节点，这里重新挂一遍
  ensureLoadMoreObserver();
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
    const commitWorkSearch = async (query, fromEnter = false) => {
      // 「正文内容」这一档不筛当前列表 —— 正文不在数据库里，得去正文搜索页扫盘。
      // 打字阶段只是把词记下来（避免每敲一个字扫一遍库），回车才跳过去。
      if (state.searchField === "body") {
        if (!fromEnter) {
          state[state.homeView === "allWorks" && !state.activeAuthor ? "allWorksQuery" : "workQuery"] = query;
          render();
          restoreSearchFocus("work-search");
          return;
        }
        await openTextSearch(query);
        return;
      }
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
      await commitWorkSearch(event.currentTarget.value, true);
    };
  }
  // 收藏夹与浏览历史的搜索框：跟作品库同一套路子，中文输入法期间不重绘
  const collectionSearch = app.querySelector("#collection-search");
  if (collectionSearch) {
    collectionSearch.oncompositionstart = () => { collectionSearch.dataset.composing = "true"; };
    const commitCollectionSearch = async (query, fromEnter = false) => {
      if (state.searchField === "body") {
        if (!fromEnter) {
          state.collectionQuery = query;
          render();
          restoreSearchFocus("collection-search");
          return;
        }
        await openTextSearch(query);
        return;
      }
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
      await commitCollectionSearch(event.currentTarget.value, true);
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
  const textSearchBox = app.querySelector("#text-search-input");
  if (textSearchBox) {
    textSearchBox.oncompositionstart = () => { textSearchBox.dataset.composing = "true"; };
    textSearchBox.oncompositionend = (event) => {
      delete textSearchBox.dataset.composing;
      // 上屏这一下必须自己把词记进来：部分中文输入法在 Chromium 里只发 compositionend、
      // 不补 input 事件，只删标记不记账的话 state 会一直空着（另外四个搜索框都是这么写的）。
      state.textSearchQuery = event.target.value;
    };
    // 打字时只记账、不搜：一次搜索要把全库正文读一遍，挂在输入事件上必然卡
    textSearchBox.oninput = (event) => {
      if (event.isComposing || textSearchBox.dataset.composing) return;
      state.textSearchQuery = event.target.value;
      // 每敲一下都重判：清空到没字了就重新把历史露出来
      state.textSearchHistoryOpen = !event.target.value.trim() && state.textSearchHistory.length > 0;
      syncTextSearchHistoryPanel();
    };
    // 历史下拉只在**空框**时展开 —— 框里有词时展开会盖住上一次的结果，那是用户更想看的东西
    textSearchBox.onfocus = () => {
      if (textSearchBox.value.trim()) return;
      state.textSearchHistoryOpen = true;
      syncTextSearchHistoryPanel();
    };
    textSearchBox.onblur = () => {
      state.textSearchHistoryOpen = false;
      syncTextSearchHistoryPanel();
    };
    textSearchBox.onkeydown = async (event) => {
      if (event.key === "Escape") {
        // 浮层开着时 Esc 的语义是「关掉它」，顺手挡掉 type=search 自带的清空
        event.preventDefault();
        state.textSearchHistoryOpen = false;
        syncTextSearchHistoryPanel();
        return;
      }
      if (event.key !== "Enter" || event.isComposing || textSearchBox.dataset.composing) return;
      event.preventDefault();
      await runTextSearch(event.currentTarget.value);
    };
  }
  // 点历史项时别让输入框先失焦 —— 一失焦 onblur 就把面板收走，click 根本落不到按钮上
  const textSearchHistoryPanelElement = app.querySelector("#text-search-history");
  if (textSearchHistoryPanelElement) textSearchHistoryPanelElement.onpointerdown = (event) => event.preventDefault();
  const sortSelect = app.querySelector("#sort-select");
  if (sortSelect) sortSelect.onchange = async (event) => { state.sort = event.target.value; if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); };
  const searchField = app.querySelector("#search-field");
  if (searchField) searchField.onchange = async (event) => {
    state.searchField = event.target.value;
    // 收藏夹视图也吃这个范围（v1.2.7 起），它的数据源跟另外两个列表不是同一个
    if (state.homeView === "collections" && state.activeCollection) await refreshCollectionWorks();
    else if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks();
    else await refreshWorks();
    render();
  };

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
      // 版本 / 配图 / 收藏夹都是后端 SQL 条件（作者库、所有作品、收藏夹三处各一份），改完要重查当前列表。
      // 重查哪一份交给 refreshActiveList()，这里别自己按 homeView 分支 —— 漏一种组合会静默不刷新
      if (action === "status") { state.status = status; await refreshActiveList(); render(); }
      if (action === "images-filter") { state.imagesFilter = element.dataset.imagesFilter || "all"; await refreshActiveList(); render(); }
      if (action === "favorites-only") { if (state.homeView === "allWorks" && !state.activeAuthor) { state.allWorksFavoritesOnly = !state.allWorksFavoritesOnly; await refreshAllWorks(); } else { state.authorFavoritesOnly = !state.authorFavoritesOnly; await refreshWorks(); } render(); }
      // 工具栏那个「仅看带图版」是面板「配图 · 有图」的快捷开关：同一个字段，两处显示，
      // 不会出现「工具栏亮着、面板说无图」这种自相矛盾的状态
      if (action === "images-only") { state.imagesFilter = state.imagesFilter === "has" ? "all" : "has"; await refreshActiveList(); render(); }
      if (action === "serial-latest") { state.serialLatestOnly = !state.serialLatestOnly; render(); }
      // 已读 / 评分筛选是纯前端过滤（数据都在列表里了），不用回后端重查
      if (action === "read-filter") { state.readFilter = element.dataset.readFilter; render(); }
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
      // 作品卡上的「三个点」现在直接开详情页，原来那个菜单的功能都搬进详情里了
      if (action === "work-menu") await openWorkDetail(Number(workId));
      if (action === "open-series") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, state.seriesView?.returnTo || "works");
      if (action === "open-all-series") await openAllWorksSeries(Number(authorId), element.dataset.seriesId, element.dataset.seriesTitle);
      if (action === "open-series-card") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, "overview");
      if (action === "close-series-view") await closeSeriesView();
      if (action === "set-work-series") {
        const form = document.querySelector("#series-form");
        const values = Object.fromEntries(new FormData(form).entries());
        await setWorkSeries(Number(workId), values.seriesId, Number(values.seriesOrder));
      }
      if (action === "leave-series") await leaveWorkSeries(Number(workId));
      if (action === "remove-author-alias") removeAuthorAlias(Number(element.dataset.index));
      if (action === "pick-collection") { closeModal(); await openCollectionPicker(Number(workId)); }
      // 作品详情弹窗（v1.2.0）
      if (action === "cycle-read-state") await cycleReadState(Number(workId));
      if (action === "detail-read-state") await setDetailMeta({ readState: Number(element.dataset.readState) });
      if (action === "detail-rate") await setDetailMeta({ rating: Number(element.dataset.rating) });
      if (action === "detail-remove-tag") await removeDetailTag(Number(element.dataset.index));
      if (action === "detail-pick-collection") await openDetailPicker(Number(workId));
      if (action === "detail-open-path") await invoke("open_local_path", { path: element.dataset.path, parent: false });
      if (action === "detail-open-dir") await invoke("open_local_path", { path: element.dataset.path, parent: true });
      if (action === "detail-open-author") { closeModal(); await openAuthorLibrary(Number(authorId)); }
      if (action === "detail-open-work") {
        // 打开就算「在读」。后端 open_work 只往上抬、**刻意不覆盖**手动标的「已读」，
        // 前端这份内存镜像得守同一条规矩，否则本来已读的作品点一下开会当场显示成「在读」
        const openedId = Number(workId);
        await invoke("open_work", { workId: openedId });
        const opened = findWork(openedId);
        if (opened) opened.readState = Math.max(Number(opened.readState) || 0, 1);
        refreshDetailDom();
      }
      if (action === "detail-open-reading") await openWorkReading(Number(workId));
      if (action === "detail-open-pixiv") await openExternalUrl(element.dataset.url);
      if (action === "detail-series") { closeModal(); await chooseSeriesForWork(Number(workId)); }
      // 原来「三个点」菜单里的功能，现在都在详情页里（v1.2.1）
      if (action === "detail-toggle-synopsis") toggleDetailSynopsis();
      if (action === "detail-open-directory") await invoke("open_work_directory", { workId: Number(workId) });
      if (action === "detail-set-version") await detailSetVersion(Number(workId), element.dataset.asFull === "1");
      if (action === "detail-bind-file") await detailBindFile(Number(workId));
      if (action === "detail-toggle-images") await detailToggleImages(Number(workId));
      if (action === "detail-download") await detailDownload(Number(workId), element.dataset.format);
      if (action === "detail-delete") detailDeleteWork(Number(workId));
      if (action === "go-collections") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "collections"; state.seriesView = null; state.seriesItems = []; state.activeCollection = null; state.collectionQuery = ""; await refreshCollections(); render(); }
      if (action === "go-history") { state.authorReturnTo = null; state.activeAuthor = null; state.homeView = "history"; state.seriesView = null; state.seriesItems = []; state.historyQuery = ""; await refreshHistory(); render(); }
      if (action === "go-missing-full") { state.authorReturnTo = null; state.activeAuthor = null; state.seriesView = null; state.seriesItems = []; state.homeView = "missingFull"; state.missingFullAuthorId = null; await refreshMissingFull(); render(); }
      // 高级筛选面板：开合、三档条件、清空
      if (action === "toggle-filter-panel") {
        // 收藏夹列表平时只有进「我的收藏」页才拉；面板里要列收藏夹，打开前先补齐
        if (!state.filterPanelOpen && !state.collections.length) await refreshCollections();
        state.filterPanelOpen = !state.filterPanelOpen;
        render();
      }
      if (action === "rating-filter") { state.ratingFilter = element.dataset.ratingFilter || "all"; render(); }
      if (action === "words-filter") { state.wordsFilter = element.dataset.wordsFilter || "all"; render(); }
      if (action === "collection-filter") {
        // 收藏夹是关系数据，前端筛不了 —— 改完要重新查库。-1 = 任意收藏夹
        const picked = Number(element.dataset.collectionFilter);
        state.collectionFilter = Number.isFinite(picked) ? picked : 0;
        await refreshActiveList();
        render();
      }
      if (action === "clear-filters") { state.readFilter = "all"; state.ratingFilter = "all"; state.wordsFilter = "all"; state.collectionFilter = 0; state.imagesFilter = "all"; state.status = "all"; await refreshActiveList(); render(); }
      if (action === "load-more-all-works") await loadMoreAllWorks();
      // 待补完整版工作台
      if (action === "open-missing-author") { state.missingFullAuthorId = Number(element.dataset.authorId) || null; state.missingFullFilter = "todo"; render(); }
      if (action === "back-to-missing-authors") { state.missingFullAuthorId = null; render(); }
      if (action === "missing-full-filter") { state.missingFullFilter = element.dataset.filter || "todo"; render(); }
      if (action === "mark-missing-full") {
        const workId = Number(element.dataset.workId);
        const nextState = Number(element.dataset.state) || 0;
        await invoke("set_works_need_full_state", { workIds: [workId], state: nextState });
        await refreshMissingFull();
        render();
        toast(nextState === 0 ? "已恢复成未处理" : nextState === 1 ? "标记为「已找过·没有」" : "标记为「不打算补」", "info");
      }
      // 待补完整版的批量标记（v1.2.9）：勾选走的是批量操作那套 selectedWorkIds
      if (action === "missing-bulk-enter") { state.missingFullBulk = true; state.selectedWorkIds.clear(); render(); }
      if (action === "missing-bulk-exit") { state.missingFullBulk = false; state.selectedWorkIds.clear(); render(); }
      if (action === "missing-toggle-select") { toggleWorkSelection(Number(element.dataset.workId)); }
      if (action === "missing-select-all") {
        const mine = missingFullVisibleWorks();
        const allSelected = mine.length > 0 && mine.every((work) => state.selectedWorkIds.has(work.id));
        if (allSelected) mine.forEach((work) => state.selectedWorkIds.delete(work.id));
        else mine.forEach((work) => state.selectedWorkIds.add(work.id));
        render();
      }
      if (action === "missing-bulk-mark") {
        const workIds = [...state.selectedWorkIds];
        if (!workIds.length) { toast("先勾选要标记的作品", "info"); return; }
        const nextState = Number(element.dataset.state) || 0;
        try {
          const changed = await invoke("set_works_need_full_state", { workIds, state: nextState });
          state.selectedWorkIds.clear();
          await refreshMissingFull();
          // 标完可能整批离开了当前这一档（比如「未处理」→「已找过」），
          // 列表空了就自动退回作者列表，别让用户对着空页面发愣
          if (!missingFullVisibleWorks().length) state.missingFullBulk = false;
          render();
          toast(`已标记 ${changed} 篇为「${nextState === 0 ? "未处理" : nextState === 1 ? "已找过·没有" : "不打算补"}」`, "success");
        } catch (error) {
          toast(String(error), "error");
        }
      }
      // 筛选模板（v1.2.9）
      if (action === "go-filter-views") { state.authorReturnTo = null; state.activeAuthor = null; state.seriesView = null; state.seriesItems = []; state.homeView = "filterViews"; await refreshAllWorks(); if (!state.collections.length) await refreshCollections(); await refreshFilterViews(); render(); }
      if (action === "go-text-search") { state.authorReturnTo = null; state.activeAuthor = null; state.seriesView = null; state.seriesItems = []; state.homeView = "textSearch"; render(); const box = document.querySelector("#text-search-input"); if (box) box.focus(); }
      if (action === "run-text-search") await runTextSearch();
      if (action === "use-text-search-history") {
        state.textSearchHistoryOpen = false;
        await runTextSearch(element.dataset.query);
      }
      if (action === "remove-text-search-history") {
        // 删完重画一遍：面板里少一行、剩下的位置全变，一并对齐（下拉这时本来就该开着）
        removeTextSearchHistory(element.dataset.query);
        render();
        restoreSearchFocus("text-search-input");
      }
      if (action === "clear-text-search-history") {
        clearTextSearchHistory();
        state.textSearchHistoryOpen = false;
        render();
      }
      if (action === "save-filter-view") { closeModal(); filterViewNameModal(); }
      if (action === "submit-filter-view") { await submitFilterViewName(); }
      if (action === "open-filter-view") {
        const view = state.filterViews.find((item) => item.id === Number(element.dataset.viewId));
        if (!view) { toast("这个模板已经不在了，刷新一下", "error"); return; }
        await applyFilterView(view);
      }
      if (action === "update-filter-view") {
        const view = state.filterViews.find((item) => item.id === Number(element.dataset.viewId));
        if (!view) { toast("这个模板已经不在了，刷新一下", "error"); return; }
        try {
          await invoke("update_filter_view", { id: view.id, payload: currentFilterPayload() });
          await refreshFilterViews();
          render();
          toast(`模板「${view.name}」已改成当前这条件`, "success");
        } catch (error) {
          toast(String(error), "error");
        }
      }
      if (action === "rename-filter-view") {
        const view = state.filterViews.find((item) => item.id === Number(element.dataset.viewId));
        if (!view) { toast("这个模板已经不在了，刷新一下", "error"); return; }
        closeModal();
        filterViewNameModal(view);
      }
      if (action === "submit-filter-view-rename") { await submitFilterViewRename(Number(element.dataset.viewId)); }
      if (action === "delete-filter-view") {
        const view = state.filterViews.find((item) => item.id === Number(element.dataset.viewId));
        if (!view) { toast("这个模板已经不在了，刷新一下", "error"); return; }
        closeModal();
        await confirmAction("删除筛选模板", `确定要删掉「${escapeHtml(view.name)}」吗？只会删掉这套条件，作品一篇都不会动。`, "删除", async () => {
          try {
            await invoke("delete_filter_view", { id: view.id });
            await refreshFilterViews();
            render();
            toast(`已删除模板「${view.name}」`, "success");
          } catch (error) {
            toast(String(error), "error");
          }
        });
      }
      // 一键标已读 / 未读（卡片左下角）
      if (action === "toggle-read") await toggleWorkRead(Number(element.dataset.workId));
      // 合集 EPUB
      if (action === "export-series-epub") {
        await exportAnthology(`series:${element.dataset.seriesId}`, element.dataset.seriesTitle || "系列合集");
      }
      if (action === "export-collection-epub") {
        await exportAnthology(`collection:${element.dataset.collectionId}`, element.dataset.collectionName || "收藏夹合集");
      }
      // 系列连续读
      if (action === "series-continue") await continueSeriesReading();      if (action === "open-collection") { const collection = state.collections.find((item) => item.id === Number(element.dataset.collectionId)); if (!collection) { toast("这个收藏夹已经不在了，刷新一下", "error"); return; } closeModal(); state.activeCollection = collection; state.collectionQuery = ""; await refreshCollectionWorks(); render(); }
      if (action === "back-to-collections") { state.activeCollection = null; state.collectionQuery = ""; await refreshCollections(); render(); }
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
      if (action === "delete-selected") await deleteSelectedWorks();
      // 批量操作扩展（v1.2.0）
      if (action === "bulk-read-state") await bulkSetReadState(Number(element.dataset.readState));
      if (action === "bulk-add-collection") bulkCollectionPicker();
      if (action === "bulk-collection-submit") await submitBulkCollection();
      if (action === "bulk-collection-toggle") {
        const id = Number(element.dataset.collectionId);
        state.bulkCollectionIds = state.bulkCollectionIds.includes(id) ? state.bulkCollectionIds.filter((item) => item !== id) : [...state.bulkCollectionIds, id];
        refreshBulkCollectionDom();
      }
      if (action === "bulk-collection-create") {
        const input = document.querySelector("#bulk-picker-new-name");
        const name = (input?.value || "").trim();
        if (!name) { toast("先给收藏夹起个名字", "error"); input?.focus(); return; }
        try {
          const created = await invoke("create_collection", { name });
          await refreshCollections();
          state.bulkCollectionIds = [...state.bulkCollectionIds, created.id];
          refreshBulkCollectionDom();
          toast(`已创建收藏夹「${created?.name || name}」`, "success");
        } catch (error) {
          toast(String(error), "error");
        }
      }
      if (action === "bulk-tag-modal") bulkTagModal();
      if (action === "bulk-tag-submit") await submitBulkTags();
      // 批量补充（v1.2.7）：移出收藏夹 + 设评分
      if (action === "bulk-remove-collection") await bulkRemoveCollectionPicker();
      if (action === "bulk-remove-submit") await submitBulkRemoveCollection();
      if (action === "bulk-remove-toggle") {
        const id = Number(element.dataset.collectionId);
        state.bulkRemoveCollectionIds = state.bulkRemoveCollectionIds.includes(id) ? state.bulkRemoveCollectionIds.filter((item) => item !== id) : [...state.bulkRemoveCollectionIds, id];
        refreshBulkRemoveCollectionDom();
      }
      if (action === "bulk-rating-modal") bulkRatingModal();
      if (action === "bulk-rating-submit") await submitBulkRating(element.dataset.rating);
      // 自动备份（v1.2.7）
      if (action === "backup-now") await backupNow();
      if (action === "restore-backup-file") await restoreBackupFile(element.dataset.path || "");
      if (action === "export-all") await runExport("all");
      if (action === "export-collection") await runExport("collection", Number(state.activeCollection?.id));
      if (action === "export-selected") await runExport("ids", null, [...state.selectedWorkIds]);
      // 维护工具（v1.2.0）
      if (action === "backfill-synopses") await backfillSynopses();
      if (action === "backfill-covers") await backfillCovers();
      if (action === "scan-work-files") await scanWorkFiles();
      if (action === "clear-missing-bindings") await clearMissingBindings();
      if (action === "open-work-url") {
        const url = pixivNovelUrl(findWork(Number(workId))?.pixivNovelId);
        if (!url) { toast("这个作品没有 Pixiv 作品 ID，先同步一次才能跳过去", "info"); return; }
        await openExternalUrl(url);
      }
      if (action === "download-selected-reading") { await downloadSelectedReadings(element.dataset.format === "epub" ? "epub" : ""); return; }
      if (action === "backfill-images") { await downloadSelectedReadings(""); return; }
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
    // 待补工作台的「批量标记」模式下整卡可点＝勾选（和批量操作一致）
    if (state.missingFullBulk) { toggleWorkSelection(Number(card.dataset.workId)); return; }
    // 只有封面图和标题可以打开文件，卡片其他位置（日期、标签等）点击无反应
    if (!event.target.closest(".work-open")) return;
    queueCardOpen(Number(card.dataset.workId), card);
    });
    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      const work = findWork(Number(card.dataset.workId));
      if (!work) return;
      // 选中了文字（标题、标签、日期都行）再右键 → 还是给「搜索」菜单：那是它唯一的入口，
      // 删掉就没法用了。没选中的那半边，v1.2.2 起和右下角「三个点」一样**直接开详情页** ——
      // 原来的「编辑标签」弹窗已经并进详情页的标签块，右键不必再分一层。
      // 右键点偏了导致选区被清时，用按下时的快照兜住。
      const selected = cardSelectedText(card) || state.cardSelectionText;
      state.cardSelectionText = "";
      if (selected) { cardSearchMenu(work, selected); return; }
      openWorkDetail(work.id);
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
    document.querySelector("#settings-slot-pixiv")?.insertAdjacentHTML("beforeend", `<label class="delay-settings">Pixiv 抓取间隔 <div class="delay-input"><span>同步作品超过</span><input name="pixivDelayThreshold" type="number" min="1" step="1" value="${Number(settings.pixivDelayThreshold || 150)}"><span>部时，每部间隔</span><input name="pixivDelaySeconds" type="number" min="0" max="60" step="1" value="${Number(settings.pixivDelaySeconds ?? 1)}"><span>秒</span></div><small>超过阈值后，作品详情请求会按此间隔执行，降低连续抓取频率 —— <b>「补抓作品简介」也吃这个设置</b>。默认超过 150 部时每部间隔 1 秒；填 0 秒可关闭间隔。抓简介时如果连续失败，间隔会自动翻倍（最多 60 秒），实在不行会提前停下。</small></label>`);
    // Cookie 体检（v1.2.15）：把框里**当前**的值直接送去测，不等自动保存那半秒 ——
    // 刚粘贴完就点「测试」，测到的必须是他刚粘贴的那份，而不是库里还存着的旧值
    document.querySelector("#settings-slot-pixiv")?.insertAdjacentHTML("beforeend", `<label>Cookie 体检 <div class="cookie-probe"><button type="button" class="quiet-button" data-action="test-pixiv-cookie">测试 Cookie</button><span class="cookie-probe-result" id="cookie-probe-result"></span></div><small>拿当前填的 Cookie 向 Pixiv 发一个只读请求，确认登录还有效 —— 有效就把登录的账号名带出来，方便确认换的是不是想要的那个号。上面换成新的 <b>PHPSESSID</b> 后点一下就能验，不用等同步跑到一半才失败；框里没填东西时测的是已保存的那份。失效只影响 R-18 作品和完整列表，不影响本地已有的文件。</small></label>`);
    document.querySelector('[data-action="test-pixiv-cookie"]')?.addEventListener("click", async (event) => {
      const button = event.currentTarget;
      const slot = document.querySelector("#cookie-probe-result");
      const cookieValue = document.querySelector('[name="pixivCookie"]')?.value || "";
      const paint = (className, text) => {
        if (!slot) return;
        slot.className = `cookie-probe-result ${className}`;
        slot.textContent = text;
      };
      button.disabled = true;
      paint("is-pending", "正在测试…");
      try {
        const probe = await invoke("check_pixiv_cookie", { cookie: cookieValue || null });
        paint(probe.ok ? "is-ok" : "is-bad", probe.message || (probe.ok ? "Cookie 有效。" : "Cookie 不可用。"));
        // 测出有效的当天就别再弹启动提醒了
        if (probe.ok) window.localStorage.removeItem(PIXIV_COOKIE_WARNING_DAY_KEY);
        toast(probe.ok ? "Cookie 有效" : probe.message, probe.ok ? "success" : "error");
      } catch (error) {
        paint("is-bad", String(error));
        toast(String(error), "error");
      } finally {
        button.disabled = false;
      }
    });
    // 存量作品补角标：绑在 EPUB / HTML 上的作品，图片数可能在绑定之前就已经存在
    document.querySelector("#settings-slot-maintain")?.insertAdjacentHTML("beforeend", `<label>阅读版图片数 <button type="button" class="quiet-button" data-action="refresh-reading-image-counts">立即重算</button><small>扫描全库里绑定在 EPUB / HTML 上的作品，重新统计作品卡上的图片角标（数的是文件里的插图，不含封面）。绑定普通 txt 的作品不受影响。</small></label>`);
    document.querySelector("[data-action='refresh-reading-image-counts']")?.addEventListener("click", async (event) => {
      const button = event.currentTarget;
      button.disabled = true;
      try {
        const result = await invoke("refresh_reading_image_counts");
        const scanned = Number(result?.scannedCount || 0);
        const updated = Number(result?.updatedCount || 0);
        // 图片数变了，封面上的带图版角标也要跟着变 —— 一律刷「当前这个列表」
        await refreshVisibleList();
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

  // 作者别名输入框：回车就地把别名加进去（不能重开弹窗，否则会丢掉表单里其它未保存的修改）
  const aliasInput = document.querySelector("#alias-editor-input");
  if (aliasInput && !aliasInput.dataset.bound) {
    aliasInput.dataset.bound = "true";
    aliasInput.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); addAuthorAlias(); } });
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

function toggleBulkMode() {
  state.bulkMode = !state.bulkMode;
  state.selectedWorkIds.clear();
  render();
}

/**
 * 批量操作条（v1.2.1）。
 * 原来是把十几个长得一模一样的按钮一股脑塞进顶栏、每个后面还挂个「（N）」，
 * 挤成两行完全看不出分类。现在拆成独立一条：左边只报一次「已选 N 篇」，
 * 其余按 版本 / 整理 / 文件 / 输出 分组、每组带个小标题，扫一眼就知道该点哪儿。
 */
function bulkBar(works) {
  const count = state.selectedWorkIds.size;
  const disabled = count ? "" : "disabled";
  const allSelected = works.length > 0 && works.every((work) => state.selectedWorkIds.has(work.id));
  const group = (label, buttons) => `<div class="bulk-bar-group"><span class="bulk-bar-label">${label}</span>${buttons}</div>`;
  return `
    <section class="bulk-bar" aria-label="批量操作">
      <div class="bulk-bar-group is-lead">
        <strong class="bulk-count">已选 ${count} 篇</strong>
        <button class="quiet-button" data-action="select-all" ${works.length ? "" : "disabled"}>${allSelected ? "取消全选" : "全选本页"}</button>
        <button class="quiet-button" data-action="bulk-mode">退出批量</button>
      </div>
      ${group("版本", `<button class="bulk-button" data-action="copy-selected-full" ${disabled}>设为完整版</button><button class="bulk-button" data-action="set-images-selected" ${disabled}>设为带图版</button>`)}
      ${group("整理", `<button class="bulk-button" data-action="bulk-read-state" data-read-state="2" ${disabled}>设已读</button><button class="bulk-button" data-action="bulk-read-state" data-read-state="0" ${disabled}>设未读</button><button class="bulk-button" data-action="bulk-rating-modal" ${disabled}>设评分</button><button class="bulk-button" data-action="bulk-add-collection" ${disabled}>加入收藏夹</button><button class="bulk-button" data-action="bulk-remove-collection" ${disabled}>移出收藏夹</button><button class="bulk-button" data-action="bulk-tag-modal" ${disabled}>改标签</button>`)}
      ${group("文件", `<button class="bulk-button" data-action="backfill-images" title="按设置里选的格式，给勾选的作品重新下载阅读版并绑定" ${disabled}>补下配图</button><button class="bulk-button" data-action="download-selected-reading" data-format="epub" ${disabled}>重新下载 EPUB 版</button>`)}
      ${group("输出", `<button class="bulk-button" data-action="export-selected" ${disabled}>导出清单</button>`)}
      <div class="bulk-bar-group is-danger"><button class="danger-button" data-action="delete-selected" ${disabled}>删除已选</button></div>
    </section>`;
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

// 框选时只更新卡片上的勾选角标和批量条上的计数 —— 整条重画会把正在拖的框打断。
// 批量按钮上的「（N）」在 v1.2.1 已经取消，换成批量条左边统一的「已选 N 篇」。
function syncSelectionUI() {
  document.querySelectorAll(".selection-badge").forEach((badge) => {
    const selected = state.selectedWorkIds.has(Number(badge.dataset.workId));
    badge.classList.toggle("is-selected", selected);
    badge.innerHTML = selected ? icon("check", 16) : "";
    badge.title = selected ? "取消选择" : "选择作品";
  });
  document.querySelectorAll(".bulk-count").forEach((counter) => {
    counter.textContent = `已选 ${state.selectedWorkIds.size} 篇`;
  });
}

// 框选只对「正在勾选」的列表生效，两处都算：作者作品库的批量操作、
// 待补工作台的批量标记（v1.2.9 用户报「待补那边不能像批量操作那样拖框选」）。
// 除了状态位，还要求这个网格里真的有勾选框 —— 免得上一页残留的批量状态
// 让别的列表莫名其妙也能框出一片选择。
function marqueeScope() {
  if (!state.bulkMode && !state.missingFullBulk) return null;
  const scope = document.querySelector(".works-grid");
  if (!scope || !scope.querySelector(".selection-badge")) return null;
  return scope;
}

function bindMarqueeSelection() {
  if (document.body.dataset.marqueeBound === "true") return;
  document.body.dataset.marqueeBound = "true";
  document.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    if (event.target.closest("button, input, select, textarea, a, .modal-layer, .side-rail, .topbar, .bulk-bar, .library-tools, .binding-bar, .empty-state, .read-only-note")) return;
    const scope = marqueeScope();
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
    // 打开＝阅读状态变「在读」，卡片上那颗圆点得跟着变。
    // 待补工作台 / 系列页 / 浏览历史各有各的数据源，走统一那份才不漏
    await refreshVisibleList();
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

/* ============================ 批量操作扩展（v1.2.0） ============================ */

/** 批量设阅读状态。批量条只在作者作品库出现，所以刷新 refreshWorks 就够。 */
async function bulkSetReadState(readState) {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) return;
  try {
    await invoke("set_works_read_state", { workIds, readState });
    state.bulkMode = false;
    state.selectedWorkIds.clear();
    await refreshWorks();
    render();
    toast(`已把 ${workIds.length} 个作品设为「${readStateLabel(readState)}」`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/**
 * 批量「加入收藏夹」弹窗的正文：勾选行 + 就地新建。
 * 和单篇那个收藏夹选择器同款 —— 勾上要有对号、能当场建新夹子，少这两样就没法用。
 */
function bulkCollectionBody() {
  const selected = new Set(state.bulkCollectionIds);
  const rows = state.collections.map((collection) => `
    <button class="picker-row ${selected.has(collection.id) ? "is-selected" : ""}" data-action="bulk-collection-toggle" data-collection-id="${collection.id}">
      <span class="picker-check">${selected.has(collection.id) ? icon("check", 15) : ""}</span>
      <span class="picker-name">${escapeHtml(collection.name)}</span>
      <small>${collection.workCount} 篇</small>
    </button>`).join("");
  return `
    <div class="picker-list">${rows || '<p class="match-note">还没有收藏夹，在下面建一个。</p>'}</div>
    <div class="picker-new">
      <input id="bulk-picker-new-name" placeholder="新建收藏夹，输入名字后回车" autocomplete="off">
      <button class="quiet-button" data-action="bulk-collection-create">${icon("plus", 16)}新建</button>
    </div>`;
}

/** 只换勾选区，不重开弹窗（重开会丢滚动位置） */
function refreshBulkCollectionDom() {
  const holder = document.querySelector("#bulk-collection-body");
  if (!holder) return;
  holder.innerHTML = bulkCollectionBody();
  bindEvents();
}

/** 批量加收藏夹：勾选式弹窗，确定时按「并集」插入（不会把作品从别的夹子里踢出来） */
async function bulkCollectionPicker() {
  if (!state.selectedWorkIds.size) return;
  // 收藏夹列表平时只有进「我的收藏」页才拉；批量条在作者库上，先补齐免得弹窗里是空的
  if (!state.collections.length) await refreshCollections();
  state.bulkCollectionIds = [];
  showModal(modal("加入收藏夹",
    `<p class="match-note">把选中的 ${state.selectedWorkIds.size} 篇作品放进下面勾选的收藏夹。已经在里面的不会重复添加。</p><div id="bulk-collection-body">${bulkCollectionBody()}</div>`,
    `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="bulk-collection-submit">确定</button>`));
}

async function submitBulkCollection() {
  const workIds = [...state.selectedWorkIds];
  const collectionIds = [...state.bulkCollectionIds];
  if (!collectionIds.length) { toast("先勾一个收藏夹", "error"); return; }
  try {
    await invoke("add_works_to_collections", { workIds, collectionIds });
    closeModal();
    state.bulkMode = false;
    state.selectedWorkIds.clear();
    state.bulkCollectionIds = [];
    await refreshCollections();
    await refreshWorks();
    render();
    toast(`已把 ${workIds.length} 篇作品加入 ${collectionIds.length} 个收藏夹`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/** 移出收藏夹的勾选区（和加入那边同构，只是没有任何「新建」入口） */
function bulkRemoveCollectionBody() {
  const selected = new Set(state.bulkRemoveCollectionIds);
  const rows = state.collections
    .map(
      (collection) => `
    <button class="picker-row ${selected.has(collection.id) ? "is-selected" : ""}" data-action="bulk-remove-toggle" data-collection-id="${collection.id}">
      <span class="picker-check">${selected.has(collection.id) ? icon("check", 15) : ""}</span>
      <span class="picker-name">${escapeHtml(collection.name)}</span>
      <small>${collection.workCount} 篇</small>
    </button>`,
    )
    .join("");
  return `<div class="picker-list">${rows || '<p class="match-note">还没有收藏夹。</p>'}</div>`;
}

function refreshBulkRemoveCollectionDom() {
  const holder = document.querySelector("#bulk-remove-body");
  if (!holder) return;
  holder.innerHTML = bulkRemoveCollectionBody();
  bindEvents();
}

/**
 * 批量移出收藏夹。和「加入」是两条独立的路 —— 加入是并集（只加不减），
 * 若把移出也做成「一次性替换」，用户勾一次就会把作品从别的夹子里踢出去。
 */
async function bulkRemoveCollectionPicker() {
  if (!state.selectedWorkIds.size) return;
  if (!state.collections.length) await refreshCollections();
  state.bulkRemoveCollectionIds = [];
  showModal(
    modal(
      "移出收藏夹",
      `<p class="match-note">把选中的 ${state.selectedWorkIds.size} 篇作品从下面勾选的收藏夹里移出去。作品本身不会删，只是不再属于这些夹子。</p><div id="bulk-remove-body">${bulkRemoveCollectionBody()}</div>`,
      `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="bulk-remove-submit">确定</button>`,
    ),
  );
}

async function submitBulkRemoveCollection() {
  const workIds = [...state.selectedWorkIds];
  const collectionIds = [...state.bulkRemoveCollectionIds];
  if (!collectionIds.length) {
    toast("先勾一个收藏夹", "error");
    return;
  }
  try {
    const removed = await invoke("remove_works_from_collections", { workIds, collectionIds });
    closeModal();
    state.bulkMode = false;
    state.selectedWorkIds.clear();
    state.bulkRemoveCollectionIds = [];
    await refreshCollections();
    await refreshWorks();
    render();
    toast(removed ? `已移出 ${removed} 条收藏记录` : "这些作品本来就不在勾选的收藏夹里", removed ? "success" : "info");
  } catch (error) {
    toast(String(error), "error");
  }
}

/** 批量设评分：点星即提交（不用再点确定），底下一颗「清除评分」 */
function bulkRatingModal() {
  if (!state.selectedWorkIds.size) return;
  const stars = [1, 2, 3, 4, 5]
    .map(
      (value) =>
        `<button class="rating-choice" data-action="bulk-rating-submit" data-rating="${value}">${[1, 2, 3, 4, 5].map((index) => icon(index <= value ? "starFilled" : "star", 18)).join("")}<span>${value} 星</span></button>`,
    )
    .join("");
  showModal(
    modal(
      "批量设评分",
      `<p class="match-note">给选中的 ${state.selectedWorkIds.size} 篇作品统一打分，点哪一档就是哪一档。</p><div class="rating-choices">${stars}</div>`,
      `<span class="footer-spacer"></span><button class="quiet-button" data-action="bulk-rating-submit" data-rating="0">清除评分</button><button class="quiet-button" data-action="close-modal">取消</button>`,
    ),
  );
}

async function submitBulkRating(rating) {
  const workIds = [...state.selectedWorkIds];
  if (!workIds.length) return;
  const value = Number(rating) || 0;
  try {
    await invoke("set_works_rating", { workIds, rating: value });
    closeModal();
    state.bulkMode = false;
    state.selectedWorkIds.clear();
    await refreshWorks();
    render();
    toast(value ? `已把 ${workIds.length} 篇设为 ${value} 星` : `已清除 ${workIds.length} 篇的评分`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/**
 * 批量改标签：一个弹窗里同时给「追加」和「移除」两个框。
 * 只做追加 / 移除，**不做整体替换** —— 整体替换手滑一次就毁一批标签，风险太大。
 */
function bulkTagModal() {
  if (!state.selectedWorkIds.size) return;
  showModal(modal("批量改标签",
    `<div class="form-stack"><label>追加标签 <input id="bulk-tag-add" type="text" placeholder="多个标签用空格、逗号或竖线分隔" autocomplete="off"></label><label>移除标签 <input id="bulk-tag-remove" type="text" placeholder="留空表示不删任何标签" autocomplete="off"></label></div><p class="match-note">两个框可以只填一个。追加时已有的同名标签会跳过；移除只摘掉匹配上的标签，其他原样保留。作用于选中的 ${state.selectedWorkIds.size} 篇作品。</p>`,
    `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="bulk-tag-submit">确定</button>`));
  window.requestAnimationFrame(() => document.querySelector("#bulk-tag-add")?.focus());
}

async function submitBulkTags() {
  const workIds = [...state.selectedWorkIds];
  const add = (document.querySelector("#bulk-tag-add")?.value || "").trim();
  const remove = (document.querySelector("#bulk-tag-remove")?.value || "").trim();
  if (!add && !remove) { toast("追加和移除至少填一个", "error"); return; }
  try {
    const changed = await invoke("update_works_tags", { workIds, add, remove });
    closeModal();
    state.bulkMode = false;
    state.selectedWorkIds.clear();
    await refreshWorks();
    render();
    toast(`已更新 ${changed} 篇作品的标签`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/**
 * 导出作品清单。`scope` = all / collection / ids。
 * 格式按用户选的扩展名判：.md / .markdown 走 Markdown，其余当 CSV（CSV 后端会写 UTF-8 BOM，Excel 打开不乱码）。
 */
async function runExport(scope, scopeId = null, workIds = null) {
  const label = scope === "collection" ? "收藏夹" : scope === "ids" ? "已选" : "全部作品";
  const path = await askSavePath({
    defaultPath: `作品清单-${label}.csv`,
    filters: [{ name: "CSV 表格", extensions: ["csv"] }, { name: "Markdown", extensions: ["md"] }],
  });
  if (!path) return;
  const lower = path.toLowerCase();
  const format = lower.endsWith(".md") || lower.endsWith(".markdown") ? "markdown" : "csv";
  try {
    const result = await invoke("export_work_list", { path, format, scope, scopeId, workIds });
    toast(`已导出 ${result.written} 篇作品`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
}

/* ======================= 合集 EPUB（v1.2.9） ======================= */

// 合集导出的进度事件。浏览器预览里没有 Tauri 事件系统，订阅失败就退化成没有进度条。
async function listenAnthologyProgress(handler) {
  try {
    return await listen("anthology-export-progress", handler);
  } catch {
    return () => {};
  }
}

/**
 * 把一个系列 / 一个收藏夹打成一整本 EPUB。
 *
 * `scopeKey` 形如 `<series|collection>:<id>` —— 作品 id 顺序**由前端定**：
 * 系列要按 `seriesOrder` 排、收藏夹要按收藏时间排，这两套口径前端手里现成，
 * 后端再排一次只会多一份要同步维护的排序规则。
 *
 * 正文和配图全部取自本地（正文 txt + 同名 `_images` 目录），不联网 ——
 * 想重新抓一遍正文用「重新下载 EPUB 版」，那是另一条路。
 */
async function exportAnthology(scopeKey, title) {
  const [kind, id] = String(scopeKey).split(":");
  let works = [];
  if (kind === "series") {
    // state.seriesItems 是后端原样给的整份（没被「只看最新」折过），顺序就是系列序号顺序
    works = state.seriesItems;
  } else if (kind === "collection") {
    works = state.collectionWorks;
  }
  const workIds = works.map((work) => work.id);
  if (!workIds.length) {
    toast("这批作品是空的，没什么可打包的", "info");
    return;
  }
  const path = await askSavePath({
    defaultPath: `${safeFileName(title)}.epub`,
    filters: [{ name: "EPUB 电子书", extensions: ["epub"] }],
  });
  if (!path) return;
  if (state.syncTask) {
    toast("有任务正在进行，等它结束再导出合集", "info");
    return;
  }
  state.syncTask = {
    kind: "anthology",
    label: "正在合成合集 EPUB",
    title: "正在准备…",
    current: 0,
    total: workIds.length,
    cancelAction: "hide-images-progress",
    cancelText: "隐藏进度",
  };
  render();
  const unlisten = await listenAnthologyProgress((event) => {
    const { total = 0, current = 0, title: label = "", done = false } = event.payload || {};
    if (done || !state.syncTask) return;
    state.syncTask.total = total;
    state.syncTask.current = current;
    state.syncTask.title = label;
    updateSyncFloater();
  });
  try {
    const result = await invoke("export_anthology_epub", { path, title, authorName: anthologyAuthor(works), workIds });
    unlisten();
    state.syncTask = null;
    render();
    const skipped = result.skipped?.length ? `，跳过 ${result.skipped.length} 篇（没有本地正文）` : "";
    const images = result.imageCount ? `，含配图 ${result.imageCount} 张` : "";
    toast(`已合成《${result.title}》：${result.chapters} 篇${images}，${formatMegabytes(result.sizeBytes)}${skipped}`, result.skipped?.length ? "info" : "success");
    if (result.skipped?.length) {
      const names = result.skipped.slice(0, 5).join("、");
      toast(`跳过的作品：${names}${result.skipped.length > 5 ? "…" : ""}`, "info");
    }
  } catch (error) {
    unlisten();
    state.syncTask = null;
    render();
    toast(String(error), "error");
  }
}

/** 合集作者名：同一个作者就写名字，混着两位以上写「多位作者」（一本电子书只能挂一个 creator） */
function anthologyAuthor(works) {
  const names = [...new Set(works.map((work) => String(work.authorName || "").trim()).filter(Boolean))];
  if (names.length === 1) return names[0];
  return names.length ? "多位作者" : "";
}

/** 名字里带不上路径分隔符，保存对话框会当成目录 */
function safeFileName(value) {
  const cleaned = String(value || "").replace(/[\\/:*?"<>|]/g, "_").trim();
  return cleaned || "合集";
}

/* ======================= 维护工具：补抓简介 / 文件体检（v1.2.0） ======================= */

/** 磁盘占用显示（到 GB 才换单位，几百 MB 时看 MB 更有感觉） */
function diskSize(bytes) {
  const value = Number(bytes) || 0;
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(2)} GB`;
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(0)} KB`;
  return `${value} B`;
}

/**
 * 补抓简介：把同步时读到、但没落库的 Pixiv 简介补回来。
 *
 * 两段式：默认只抓「还没问过 Pixiv」的那些（问过、确认作者没写简介的会跳过，
 * 再问也是空的，纯白等 + 白喂风控）；等全库都问过一遍之后，再点这个按钮就会
 * 问一句要不要「重新检查一遍」（`recheck = true`，连确认过没简介的也再问一次）。
 */
/**
 * 补齐失效封面：作品在预览版/完整版之间搬家、或目录被整理过之后，
 * 库里的 cover_path 可能指向已经不存在的文件，卡片就显示「暂无封面」。
 * 后端按 Pixiv 作品 ID 重新取直链、下载到正文旁边并更正路径；正文也不在的跳过。
 */
async function backfillCovers() {
  confirmAction(
    "补齐失效封面",
    "逐个核对作品的封面文件是否还在，指向已经找不到的会按 Pixiv 作品 ID 重新下载到正文旁边。封面还在的不动，正文也找不到的跳过。",
    "开始补齐",
    async () => {
      let result;
      try {
        result = await invoke("backfill_work_covers", { authorId: null });
      } catch (error) {
        toast(String(error), "error");
        return;
      }
      const fixed = Number(result?.fixedCount || 0);
      const failed = Number(result?.failedCount || 0);
      const skipped = Number(result?.skippedCount || 0);
      if (!fixed && !failed && !skipped) {
        toast("所有作品的封面都在，不用补", "success");
        return;
      }
      const parts = [];
      if (fixed) parts.push(`补回 ${fixed} 张`);
      if (failed) parts.push(`${failed} 张没取到`);
      if (skipped) parts.push(`${skipped} 篇正文不在、没地方放`);
      toast(`封面补齐完成：${parts.join("，")}`, failed ? "info" : "success");
      await refreshVisibleList();
      render();
    }
  );
}

async function backfillSynopses(recheck = false) {
  if (!recheck) {
    let status = { pending: 0, checkedNoSynopsis: 0 };
    try { status = await invoke("synopsis_backfill_status"); } catch { /* 查询失败就当没得抓，下面按 0 处理 */ }
    const pending = Number(status?.pending) || 0;
    const checked = Number(status?.checkedNoSynopsis) || 0;
    if (pending === 0) {
      if (checked > 0) {
        confirmAction("重新检查作品简介", `剩下 ${checked} 篇没有简介的作品，之前都问过 Pixiv 了 —— 那边作者就是没写。要再全部检查一遍吗？会按设置里的抓取间隔一篇篇请求。`, "重新检查一遍", () => runSynopsisBackfill(true));
      } else {
        toast("所有作品都已经有简介了", "info");
      }
      return;
    }
    confirmAction("补抓作品简介", `还有 ${pending} 篇作品没有简介。只给「记录了 Pixiv 作品 ID、但还没问过」的作品各发一次请求，已经有简介的和上次确认过「作者没写简介」的都会跳过。篇数超过设置里那个抓取阈值时会自动拉开请求间隔（防触发 Pixiv 风控），右下角浮层会显示预计剩余时间；连接着失败说明可能已被限流，会自动加大间隔、实在不行就提前停下，不会白跑。随时可以终止。`, "开始补抓", () => runSynopsisBackfill(false));
    return;
  }
  runSynopsisBackfill(true);
}

/** 真正的补抓流程：挂浮层 → 听进度 → 收尾报数。`recheck` 见 `backfillSynopses`。 */
async function runSynopsisBackfill(recheck) {
  state.syncTask = { authorId: 0, cancelAuthorId: 0, label: "正在补抓简介", title: "", current: 0, total: 0, eta: 0, cancelling: false };
  render();
  let unlisten = () => {};
  try {
    unlisten = await listen("synopsis-backfill-progress", (event) => {
      const { total = 0, current = 0, title = "", etaSeconds = 0 } = event.payload || {};
      if (!state.syncTask) return;
      state.syncTask.total = total;
      state.syncTask.current = current;
      if (title) state.syncTask.title = title;
      state.syncTask.eta = etaSeconds;
      updateSyncFloater();
    });
  } catch { unlisten = () => {}; }
  try {
    const result = await invoke("backfill_synopses", { authorId: 0, recheck });
    unlisten();
    state.syncTask = null;
    await refreshAuthors();
    // 简介刚补回来，站着的那个列表也要跟着刷新（待补工作台 / 系列页同样在用简介）
    await refreshVisibleList();
    render();
    const noSynopsis = Number(result.noSynopsis) || 0;
    const more = noSynopsis ? `，另有 ${noSynopsis} 篇作者没写简介` : "";
    if (result.throttled) toast(`疑似被 Pixiv 限流，已提前停止：更新 ${result.updated} 篇${more}，失败 ${result.failed} 篇。建议过一会儿再试，或把设置里的抓取间隔调大。`, "error");
    else if (result.cancelled) toast(`已终止：更新 ${result.updated} 篇${more}，还有 ${result.total - result.updated - noSynopsis - result.failed} 篇没抓`, "info");
    else if (result.updated === 0 && noSynopsis > 0) toast(`补抓完成：这 ${noSynopsis} 篇在 Pixiv 上作者都没写简介，没有能补的内容${result.failed ? `；另有 ${result.failed} 篇请求失败` : ""}。`, "info");
    else toast(`补抓完成：更新 ${result.updated} 篇${more}${result.failed ? `，失败 ${result.failed} 篇` : ""}`, "success");
  } catch (error) {
    unlisten();
    state.syncTask = null;
    render();
    toast(String(error), "error");
  }
}

/** 文件体检：核对绑定的文件在不在，并统计磁盘占用 */
async function scanWorkFiles() {
  toast("正在核对文件…", "info");
  let report;
  try {
    report = await invoke("scan_work_files");
  } catch (error) {
    toast(String(error), "error");
    return;
  }
  const missing = report.missing || [];
  const authors = report.authors || [];
  const kindLabel = (kind) => (kind === "purchased" ? "完整版" : "预览版");
  const missingRows = missing.map((item) => `
    <div class="scan-row">
      <div class="scan-row-main"><strong>${escapeHtml(item.title)}</strong><span class="scan-kind">${kindLabel(item.kind)}</span></div>
      <div class="scan-path" title="${escapeHtml(item.path)}">${escapeHtml(item.path)}</div>
    </div>`).join("");
  const authorRows = authors.slice(0, 12).map((item) => `
    <div class="scan-row is-compact"><div class="scan-row-main"><strong>${escapeHtml(item.authorName)}</strong><span class="scan-count">${item.fileCount} 个文件</span></div><div class="scan-size">${diskSize(item.bytes)}</div></div>`).join("");
  const body = `
    <div class="scan-summary">
      <div class="scan-stat"><span>核对作品</span><strong>${report.checked}</strong></div>
      <div class="scan-stat ${missing.length ? "is-warn" : ""}"><span>文件丢失</span><strong>${missing.length}</strong></div>
      <div class="scan-stat"><span>占用文件</span><strong>${report.totalFiles}</strong></div>
      <div class="scan-stat"><span>占用空间</span><strong>${diskSize(report.totalBytes)}</strong></div>
    </div>
    <section class="detail-block">
      <h4>文件丢失${missing.length ? `（${missing.length}）` : ""}</h4>
      ${missing.length ? `<p class="match-note">这些作品数据库里还绑着文件，但硬盘上已经找不到了（可能被你自己挪走或删了）。软件不会去动硬盘上的任何东西，这里只处理数据库里的绑定。</p><div class="scan-list">${missingRows}</div>` : '<p class="match-note">绑定的文件都还在，没有发现失效的关联。</p>'}
    </section>
    <section class="detail-block">
      <h4>磁盘占用（按作者）</h4>
      ${authorRows ? `<div class="scan-list">${authorRows}</div>` : '<p class="match-note">还没有可统计的文件。</p>'}
    </section>`;
  const footer = `<span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">关闭</button>${missing.length ? `<button class="danger-button" data-action="clear-missing-bindings">清理这 ${missing.length} 条失效绑定</button>` : ""}`;
  showModal(modal("文件体检", body, footer, "is-wide"));
}

/** 清掉失效绑定。后端会重新扫一遍再清（不信任前端那份可能过期的清单），只改数据库、不碰硬盘。 */
async function clearMissingBindings() {
  try {
    const cleared = await invoke("clear_missing_bindings");
    closeModal();
    // 失效绑定一清，凡是列作品的地方都可能少几条 —— 交给统一那份去判断该刷哪边
    await refreshCollections();
    await refreshVisibleList();
    render();
    toast(`已清理 ${cleared} 条失效绑定，硬盘上的文件一律没动`, "success");
  } catch (error) {
    toast(String(error), "error");
  }
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
    // 统一走详情那套收尾：作品详情能从作者库 / 系列 / 所有作品 / 收藏夹 / 浏览历史 /
    // 待补工作台任一处打开，只挑一路刷新会让别的页面停在旧值
    // （用户报的「重新下载 EPUB 版并绑定后封面带图版图标不刷新」就是它）
    await refreshAfterDetailChange();
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
  // 批量下完同样要点亮封面上的带图版角标，走统一收尾才能覆盖所有列表页
  await refreshAfterDetailChange();
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

/** 设置页「自动备份」那段列出来的备份文件（只展示最近 10 份，再多滚动也没意义） */
async function renderBackups() {
  const holder = document.querySelector("#backup-list");
  if (!holder) return;
  let entries = [];
  try {
    entries = await invoke("list_backups");
  } catch {
    entries = [];
  }
  state.backups = entries;
  if (!entries.length) {
    holder.innerHTML = '<p class="match-note">还没有自动备份。点下边的「立即备份一份」可以马上备一份。</p>';
    return;
  }
  holder.innerHTML = `<div class="backup-rows">${entries
    .slice(0, 10)
    .map(
      (entry) =>
        `<div class="backup-row"><span class="backup-time">${escapeHtml(entry.createdAt || "")}</span><span class="backup-size">${diskSize(entry.size)}</span><button class="quiet-button" data-action="restore-backup-file" data-path="${escapeHtml(entry.path)}">恢复这一份</button></div>`,
    )
    .join("")}</div>`;
  bindEvents();
}

/** 「立即备份一份」：走和后端自动备份同一条路（VACUUM INTO，拿到的是一致性快照） */
async function backupNow() {
  const button = document.querySelector('[data-action="backup-now"]');
  if (button) button.disabled = true;
  try {
    const entry = await invoke("backup_database_now");
    toast(`已备份一份（${humanSize(entry.size)}）`, "success");
    await renderBackups();
  } catch (error) {
    toast(String(error), "error");
  } finally {
    if (button) button.disabled = false;
  }
}

/** 从自动备份列表里直接恢复某一份 */
async function restoreBackupFile(path) {
  confirmAction("确认恢复备份", "恢复会用这一份备份覆盖当前数据库，当前记录会被替换掉。原始作品文件不受影响。", "恢复备份", async () => {
    await invoke("restore_backup", { path });
    state.activeAuthor = null;
    await refreshAuthors();
    render();
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
    // 和 HTML / EPUB 那两条一样走统一收尾，别只照顾作者作品库那一页
    await refreshAfterDetailChange();
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
  // cancelAuthorId 是给「整库补抓简介」这类没有具体作者的任务用的（后端用 0 当整库键）
  const targetId = state.syncTask.cancelAuthorId !== undefined ? state.syncTask.cancelAuthorId : state.syncTask.authorId;
  if (targetId || targetId === 0) await invoke("cancel_pixiv_sync", { authorId: targetId });
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
  // 自动备份：勾选框 + 保留份数（范围与后端一致：1 到 50）
  values.autoBackupEnabled = Boolean(form.querySelector('[name="autoBackupEnabled"]')?.checked);
  values.autoBackupKeep = Number(values.autoBackupKeep || 7);
  if (!Number.isInteger(values.autoBackupKeep) || values.autoBackupKeep < 1 || values.autoBackupKeep > 50) throw new Error("自动备份保留份数请输入 1 到 50 的整数");
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
      <div class="form-field update-setting">
        <div class="char-setting-head"><span class="field-title">自动备份</span><span class="settings-group-hint">每天首次启动备一份</span></div>
        <small>每天第一次打开软件时，自动把数据库备份到程序目录的 <code>data/backup/auto</code> 下，只保留最近几份、更早的自动清掉（<b>只删这个目录里自己生成的备份</b>，你手动导出的备份不会被动）。备份文件可以直接拿来恢复。</small>
        <label class="check-row"><input name="autoBackupEnabled" type="checkbox" ${settings.autoBackupEnabled === false ? "" : "checked"}><span>每天自动备份数据库</span></label>
        <label>保留最近 <div class="threshold-input"><input name="autoBackupKeep" type="number" min="1" max="50" value="${settings.autoBackupKeep || 7}"><span>份</span></div><small>1 到 50 份。</small></label>
        <div id="backup-list" class="backup-list"></div>
        <div class="settings-button-row"><button type="button" class="quiet-button" data-action="backup-now">${icon("database", 16)}立即备份一份</button></div>
      </div>
    </div>
    <p class="settings-note">下面这些操作点下去立刻执行，跟自动保存无关。</p>
    <div class="settings-fields is-single">
      <div class="settings-slot" id="settings-slot-maintain"></div>
    </div>
    <div class="menu-list settings-actions">
      <button type="button" data-action="backfill-synopses">${icon("info", 18)}补抓作品简介<span class="settings-action-hint">给同步过、但还没抓到简介的作品补一次（已经有简介的、上次确认过「作者没写简介」的都会跳过）</span></button>
      <button type="button" data-action="backfill-covers">${icon("image", 18)}补齐失效封面<span class="settings-action-hint">作品搬过家、目录被整理过之后，封面可能指向已经找不到的文件（卡片显示「暂无封面」）。这里按 Pixiv 作品 ID 重新取一次，下载到正文旁边</span></button>
      <button type="button" data-action="scan-work-files">${icon("search", 18)}检查文件是否还在<span class="settings-action-hint">逐个核对绑定的文件，列出「数据库里有记录、硬盘上已经没了」的作品，并统计磁盘占用</span></button>
      <button type="button" data-action="clean-preview-versions">${icon("file", 18)}清理多余预览版<span class="settings-action-hint">已经有完整版的作品，预览版就不必留了；先给你看数量再动手</span></button>
      <button type="button" data-action="export-backup">${icon("database", 18)}导出数据库备份<span class="settings-action-hint">保存一份数据库文件，出问题时可回滚</span></button>
      <button type="button" data-action="restore-backup">${icon("upload", 18)}从备份恢复<span class="settings-action-hint">用备份文件覆盖当前数据库，需重启</span></button>
    </div>
  </section>
</form>`, `<span id="settings-save-state" class="settings-save-state is-idle">改动会自动保存</span><span class="footer-spacer"></span><button class="quiet-button" data-action="close-modal">关闭</button>`, "is-wide"));
  bindCharacterEditorInputs();
  await renderCharacterEditor();
  renderSearchSiteEditor(settings.searchSites || []);
  await renderBackups();
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

/* ---------------------------------------------------------------------------
 * 全局快捷键 + 键盘导航（v1.2.12）
 *
 * 库里一千多篇，鼠标点到底太累。这里给一套只剩键盘也能转的路径：
 * 方向键在卡片网格里走、回车开详情、空格标已读。
 *
 * 三条底线，改这块之前先看一眼：
 *  1. **输入框里一律不抢键**（`isTypingTarget`）。空格和方向键在输入框里是打字的，
 *     抢掉就没法正常输入。
 *  2. **弹窗开着时不穿透**。除了 Esc，别的键不能越过弹窗去操作底下的列表 ——
 *     否则在详情里改笔记，手一抖空格就把列表里某篇标成已读了。
 *  3. 走的是已有的 `data-action` 按钮 / 已有函数，**不另写一套业务逻辑**。
 *     页面切换直接 `.click()` 侧栏按钮，全选直接点批量条上那个按钮。
 * ------------------------------------------------------------------------- */

/**
 * 快捷键说明表。**只有这一份** —— 按下 `?` 弹出来的说明就是照它画的，
 * 加一条快捷键往这儿补一行，说明里自动就有了，不会两边对不上。
 */
const SHORTCUTS = [
  { keys: "Ctrl + 1 ~ 7", text: "切到第 1~7 个导航页（作者库 / 所有作品 / 收藏 / 历史 / 待补 / 筛选模板 / 正文搜索）", scope: "全局" },
  { keys: "Ctrl + K", text: "跳到当前页的搜索框", scope: "全局" },
  { keys: "?", text: "显示这份快捷键说明", scope: "全局" },
  { keys: "Esc", text: "关掉最上面的弹窗；没弹窗时退出批量操作并清空选择", scope: "全局" },
  { keys: "↑ ↓ ← →", text: "在作品卡之间移动键盘焦点", scope: "列表页" },
  { keys: "Home / End", text: "跳到第一张 / 最后一张作品卡", scope: "列表页" },
  { keys: "Enter", text: "打开焦点作品的详情", scope: "列表页" },
  { keys: "空格", text: "把焦点作品切换成已读 / 未读（连着按可以一篇篇标下去）", scope: "列表页" },
  { keys: "B", text: "进入 / 退出批量操作", scope: "列表页" },
  { keys: "Ctrl + A", text: "批量操作下全选本页", scope: "批量模式" },
];

/** 侧栏那七个导航页对应的 data-action，顺序就是 Ctrl+1~7 的顺序 */
const NAV_SHORTCUT_ACTIONS = [
  "go-home",
  "go-all-works",
  "go-collections",
  "go-history",
  "go-missing-full",
  "go-filter-views",
  "go-text-search",
];

/** 焦点在输入框里时不能抢键 —— 打字的空格、方向键都得留给输入框自己 */
function isTypingTarget(node) {
  if (!node || !node.tagName) return false;
  const tag = node.tagName.toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select") return true;
  return Boolean(node.isContentEditable);
}

/**
 * 当前页面网格里的作品卡，按 DOM 顺序（就是屏幕上从左到右、从上到下的顺序）。
 * 收藏夹 / 历史 / 系列 / 待补 用的是同一个 `.works-grid` + `.work-card`，
 * 所以这一个选择器把六个列表页全盖住了。
 */
function visibleWorkCards() {
  return Array.from(document.querySelectorAll(".works-grid .work-card"));
}

function focusedWorkCard() {
  const active = document.activeElement;
  return active && active.closest ? active.closest(".work-card") : null;
}

/** 网格一行几张卡：数第一行有多少张的 `offsetTop` 跟第一张一样 */
function gridColumnCount(cards) {
  if (cards.length < 2) return 1;
  const firstTop = cards[0].offsetTop;
  let count = 0;
  for (const card of cards) {
    if (card.offsetTop !== firstTop) break;
    count += 1;
  }
  return Math.max(1, count);
}

/**
 * 把焦点放到某张卡上。
 * `preventScroll` 是关键：不关掉的话浏览器会先把卡片滚进视野中间，
 * 我再 `scrollIntoView({block:"nearest"})` 就白滚了 —— 表现为「按一下方向键，画面猛跳一下」。
 */
function focusWorkCard(card, scroll = true) {
  if (!card) return;
  card.focus({ preventScroll: true });
  if (scroll) card.scrollIntoView({ block: "nearest" });
}

function moveCardFocus(cards, step) {
  const current = focusedWorkCard();
  const index = current ? cards.indexOf(current) : -1;
  if (index < 0) {
    focusWorkCard(cards[step > 0 ? 0 : cards.length - 1]);
    return;
  }
  const next = index + step;
  if (next < 0 || next >= cards.length) return; // 到头就停住，别绕回另一头让人迷路
  focusWorkCard(cards[next]);
}

/**
 * 空格标已读。标完 `toggleWorkRead` 会重画整个列表，焦点跟着 DOM 一起没了 ——
 * 所以记下当前是第几张，画完再放回去。不这么做就只能标一篇，第二下空格没反应。
 */
async function keyboardToggleRead(card) {
  const index = visibleWorkCards().indexOf(card);
  await toggleWorkRead(Number(card.dataset.workId));
  const after = visibleWorkCards();
  const next = after[index] || after[after.length - 1];
  if (next) focusWorkCard(next, false);
}

function shortcutHelpBody() {
  const row = ({ keys, text, scope }) => `<div class="shortcut-row"><kbd>${escapeHtml(keys)}</kbd><div><p>${escapeHtml(text)}</p><small>${escapeHtml(scope)}</small></div></div>`;
  return `<p class="shortcut-note">在搜索框、文本框里打字时这些键都不会被抢走，放心输入。</p>
    <div class="shortcut-list">${SHORTCUTS.map(row).join("")}</div>`;
}

function showShortcutHelp() {
  showModal(modal(
    "键盘快捷键",
    shortcutHelpBody(),
    `<button class="quiet-button" data-action="close-modal">知道了</button>`,
    "is-roomy",
  ));
}

function handleGlobalShortcuts(event) {
  const key = event.key;

  // Esc 排在最前，而且不看焦点在哪 —— 关弹窗、退批量在输入框里按也得管用
  if (key === "Escape") {
    if (document.querySelector(".modal-layer")) { event.preventDefault(); closeModal(); return; }
    if (state.bulkMode) { event.preventDefault(); toggleBulkMode(); toast("已退出批量操作"); return; }
    return;
  }

  // 导航页切换：Ctrl+数字。webview 里没有标签页，不会跟浏览器抢这一组键
  if ((event.ctrlKey || event.metaKey) && !event.altKey && /^[1-7]$/.test(key)) {
    const nav = document.querySelector(`.rail-button[data-action="${NAV_SHORTCUT_ACTIONS[Number(key) - 1]}"]`);
    if (nav) { event.preventDefault(); nav.click(); }
    return;
  }

  // Ctrl+K：跳到本页搜索框。取页面里第一个 search 输入框，新加的页面自动就能用
  if ((event.ctrlKey || event.metaKey) && !event.altKey && key.toLowerCase() === "k") {
    const input = app.querySelector('input[type="search"]');
    if (input) { event.preventDefault(); input.focus(); input.select(); }
    return;
  }

  // Ctrl+A：只认批量条上那个「全选本页」按钮。点它而不是自己算 ——
  // 收藏夹页和作者页的「本页」定义不一样，自己算迟早对不上
  if ((event.ctrlKey || event.metaKey) && !event.altKey && key.toLowerCase() === "a" && !isTypingTarget(event.target)) {
    const button = document.querySelector('[data-action="select-all"], [data-action="missing-select-all"]');
    if (button && !button.disabled) { event.preventDefault(); button.click(); }
    return;
  }

  // 下面这些都要求「没按修饰键」。Ctrl+R 之类的是刷新/系统键，别碰
  if (event.ctrlKey || event.metaKey || event.altKey) return;
  if (isTypingTarget(event.target)) return;
  // 弹窗开着就只留 Esc 那条路，别的键不许穿透到底下的列表（详见本节开头的第 2 条）
  if (document.querySelector(".modal-layer")) return;

  if (key === "?") { event.preventDefault(); showShortcutHelp(); return; }

  const cards = visibleWorkCards();
  if (!cards.length) return;

  if (key === "ArrowRight" || key === "ArrowLeft" || key === "ArrowDown" || key === "ArrowUp") {
    event.preventDefault();
    const columns = gridColumnCount(cards);
    const step = key === "ArrowRight" ? 1 : key === "ArrowLeft" ? -1 : key === "ArrowDown" ? columns : -columns;
    moveCardFocus(cards, step);
    return;
  }
  if (key === "Home" || key === "End") {
    event.preventDefault();
    focusWorkCard(key === "Home" ? cards[0] : cards[cards.length - 1]);
    return;
  }
  if (key === "Enter") {
    const card = focusedWorkCard();
    if (!card) return;
    event.preventDefault();
    openWorkDetail(Number(card.dataset.workId));
    return;
  }
  if (key === " ") {
    const card = focusedWorkCard();
    if (!card) return;
    event.preventDefault();
    keyboardToggleRead(card);
    return;
  }
  if (key === "b" || key === "B") {
    // 「批量操作」按钮不在这一页就别开 —— 开了也只会给 body 挂个 is-bulk，看着像坏了
    const toggle = document.querySelector('[data-action="bulk-mode"]');
    if (!toggle) return;
    event.preventDefault();
    toggleBulkMode();
    toast(state.bulkMode ? "已进入批量操作" : "已退出批量操作");
  }
}

/* ---------------------------------------------------------------------------
 * 视图状态记忆（v1.2.12）
 *
 * 把「上次停在哪个页面、那页的搜索词/筛选/排序、滚到哪儿」记在 localStorage，
 * 下次打开接着看。一千多篇的库，每次启动都从作者库第一屏重新点一遍很烦。
 *
 * 几条规矩：
 *  - **只记界面状态，不碰 settings.json**。那是用户配置，有自己的保存时机和面板；
 *    这里丢了最多是回到默认页，不影响任何功能，所以放 localStorage 更合适。
 *  - **存的地方只有 `render()` 一处**（防抖）。筛选、排序、搜索、切页最后都会走
 *    一次 `render()`，把保存挂在这儿，加新筛选项时不用再去追那十几处 data-action。
 *  - 滚动位置实时收在 `scrollTops` 里按页面分桶。不能等切页之后再读 —— 那时 DOM
 *    已经换掉、scrollTop 归零了，读到的是新页面的 0。
 *  - **启动还原失败一律退回作者库**：记忆丢了是小事，打不开软件是大事。
 * ------------------------------------------------------------------------- */

const VIEW_STATE_KEY = "collection-library:view-state";
/** 超过这个天数的记忆不再还原 —— 一个月前的界面位置，多半也不是你现在想看的了 */
const VIEW_STATE_MAX_AGE_DAYS = 30;
/** localStorage 写入的防抖间隔：滚动时一帧一写太浪费，停下来再写 */
const VIEW_STATE_SAVE_DELAY = 250;

/**
 * 界面状态里 `FILTER_VIEW_FIELDS` 没盖住的那些字段。
 *
 * 它和筛选模板那份是**两件事**，别合并：筛选模板存的是「一套可以反复套用的条件」，
 * 这里存的是「我刚才界面长什么样」。所以作者库那个「仅看收藏」开关、各页的搜索词、
 * 收藏夹自己的排序都在这儿，而它们不该被塞进筛选模板。
 */
const VIEW_STATE_EXTRA_FIELDS = [
  ["authorQuery", ""],
  ["workQuery", ""],
  ["collectionQuery", ""],
  ["historyQuery", ""],
  ["textSearchQuery", ""],
  ["authorFavoritesOnly", false],
  ["serialLatestOnly", false],
  ["collectionSort", "added_desc"],
  ["missingFullFilter", "todo"],
  ["filterPanelOpen", false],
  ["allWorksShown", ALL_WORKS_PAGE],
];

/** 界面状态的全部字段：筛选模板那套 + 上面补的那几个，两边只在这里汇合一次 */
function viewStateFields() {
  return [...FILTER_VIEW_FIELDS, ...VIEW_STATE_EXTRA_FIELDS];
}

/** 从 localStorage 读回来的值一律过一遍类型 —— 手改过或有旧版本残留时别把 state 污染成字符串 */
function coerceViewStateValue(value, fallback) {
  if (typeof fallback === "number") {
    const number = Number(value);
    return Number.isFinite(number) ? number : fallback;
  }
  if (typeof fallback === "boolean") return Boolean(value);
  return value === undefined || value === null ? fallback : String(value);
}

/** 当前界面所在的「页面」桶：同一个 homeView 下，不同作者/收藏夹各有各的滚动位置 */
function viewStateBucket() {
  if (state.activeAuthor && state.seriesView) return `series:${state.seriesView.id ?? 0}`;
  if (state.activeAuthor) return `author:${state.activeAuthor.id}`;
  if (state.activeCollection) return `collection:${state.activeCollection.id}`;
  if (state.homeView === "missingFull" && state.missingFullAuthorId) return `missing:${state.missingFullAuthorId}`;
  return state.homeView;
}

function currentScrollTop() {
  const scroller = libraryScroller();
  return Math.max(0, Math.round(scroller.scrollTop || 0));
}

/** 各页面的滚动位置，模块级实时更新（切页前的那一次必须已经记下来，见本节开头第 3 条） */
const scrollTops = {};
let viewStateTimer = 0;

function readViewState() {
  try {
    const raw = window.localStorage.getItem(VIEW_STATE_KEY);
    if (!raw) return null;
    const value = JSON.parse(raw);
    return value && typeof value === "object" ? value : null;
  } catch {
    // 存的内容坏了就当没有，别让它拦住启动
    return null;
  }
}

function saveViewState() {
  try {
    const fields = {};
    viewStateFields().forEach(([key, fallback]) => {
      fields[key] = state[key] === undefined ? fallback : state[key];
    });
    window.localStorage.setItem(VIEW_STATE_KEY, JSON.stringify({
      version: 1,
      savedAt: Date.now(),
      homeView: state.homeView,
      activeAuthorId: state.activeAuthor?.id ?? null,
      activeCollectionId: state.activeCollection?.id ?? null,
      missingFullAuthorId: state.missingFullAuthorId ?? null,
      authorReturnTo: state.authorReturnTo === "allWorks" ? "allWorks" : null,
      fields,
      scrollTops,
    }));
  } catch (error) {
    // 隐私模式 / 配额满了都可能写不进去。这只是界面记忆，安静跳过就好
    console.debug("视图状态没记住:", error);
  }
}

function scheduleViewStateSave() {
  window.clearTimeout(viewStateTimer);
  viewStateTimer = window.setTimeout(saveViewState, VIEW_STATE_SAVE_DELAY);
}

/**
 * 滚动时先更新 `scrollTops`，再排队落盘。
 * 分两步的原因：落盘有防抖，而切页是同步发生的 —— 等防抖到点时页面已经换了，
 * 那时再读 scrollTop 读到的是新页的 0，上一页的位置就永远存不进去。
 */
function trackScrollPosition() {
  scrollTops[viewStateBucket()] = currentScrollTop();
  scheduleViewStateSave();
}

/**
 * 还原滚动位置。等两帧再滚 —— 刚 `innerHTML` 完，瀑布流那些卡片还没完成布局，
 * 直接设 scrollTop 会被后面撑开的高度顶回去。
 */
function restoreScrollTop(top) {
  const value = Math.max(0, Number(top) || 0);
  if (!value) return;
  const scroller = libraryScroller();
  window.requestAnimationFrame(() => {
    window.requestAnimationFrame(() => { scroller.scrollTop = value; });
  });
}

/**
 * 启动时接着上次那页看。返回 true 表示它已经画完了，调用方不用再 `render()` 一次。
 * 任何一步抛错都由调用方兜住退回作者库，这里只管尽力还原。
 */
async function restoreViewState() {
  const saved = readViewState();
  if (!saved) return false;
  const age = Date.now() - Number(saved.savedAt || 0);
  if (!Number.isFinite(age) || age > VIEW_STATE_MAX_AGE_DAYS * 24 * 60 * 60 * 1000) return false;

  const fields = saved.fields && typeof saved.fields === "object" ? saved.fields : {};
  viewStateFields().forEach(([key, fallback]) => {
    if (fields[key] !== undefined) state[key] = coerceViewStateValue(fields[key], fallback);
  });
  Object.assign(scrollTops, saved.scrollTops && typeof saved.scrollTops === "object" ? saved.scrollTops : {});
  state.authorReturnTo = saved.authorReturnTo === "allWorks" ? "allWorks" : null;
  // 瀑布流深度跟着回到离开时那样，否则滚动位置落在一片空白上
  if (!(state.allWorksShown >= ALL_WORKS_PAGE)) state.allWorksShown = ALL_WORKS_PAGE;

  // 先把页面回到最朴素的作者库，下面按记下来的页面逐条还原
  state.homeView = "authors";
  state.activeAuthor = null;
  state.activeCollection = null;
  state.missingFullAuthorId = null;
  state.seriesView = null;
  state.seriesItems = [];

  const view = String(saved.homeView || "authors");
  if (view === "allWorks") {
    state.homeView = "allWorks";
    await refreshAllWorks();
  } else if (view === "collections") {
    state.homeView = "collections";
    await refreshCollections();
    const id = Number(saved.activeCollectionId || 0);
    const collection = id ? state.collections.find((item) => Number(item.id) === id) : null;
    if (collection) {
      state.activeCollection = collection;
      await refreshCollectionWorks();
    }
  } else if (view === "history") {
    state.homeView = "history";
    await refreshHistory();
  } else if (view === "missingFull") {
    state.homeView = "missingFull";
    state.missingFullAuthorId = Number(saved.missingFullAuthorId || 0) || null;
    await refreshMissingFull();
  } else if (view === "filterViews") {
    state.homeView = "filterViews";
    await refreshAllWorks();
    if (!state.collections.length) await refreshCollections();
    await refreshFilterViews();
  } else if (view === "textSearch") {
    state.homeView = "textSearch";
  } else if (view === "help") {
    state.homeView = "help";
  } else {
    // 作者作品库：作者得从刚拉回来的列表里找对象，光有 id 不够（卡片要用到 workCount 那些）
    const id = Number(saved.activeAuthorId || 0);
    const author = id ? state.authors.find((item) => Number(item.id) === id) : null;
    if (author) {
      state.activeAuthor = author;
      await refreshWorks();
    }
  }
  render();
  restoreScrollTop(scrollTops[viewStateBucket()] || 0);
  // 正文搜索：把词放回框里，并重跑那次搜索（后端现扫本地文件，几百毫秒，不挡界面）。
  // 只放词不重跑的话框里有字、下面却写着「搜一下试试」，看着像坏了
  if (state.homeView === "textSearch" && state.textSearchQuery.trim()) {
    await runTextSearch(state.textSearchQuery);
  }
  return true;
}

/** 装快捷键与视图记忆的两组监听。放在 bootstrap 最前面 —— 拉数据失败也不该连键都用不了 */
function installGlobalShortcuts() {
  document.addEventListener("keydown", handleGlobalShortcuts);
  window.addEventListener("scroll", trackScrollPosition, { passive: true });
  // 关窗那一下可能还压在防抖里，抢在进程退出前写掉
  window.addEventListener("beforeunload", saveViewState);
  // 手机上切后台/被系统冻结时不发 beforeunload，补一个可见性变化
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") saveViewState();
  });
}

async function bootstrap() {
  installGlobalShortcuts();
  try {
    await refreshAuthors();
    await loadSearchSites();
    // 接着上次那页看；还原不了（或压根没记过）就照老样子落在作者库
    let restored = false;
    try {
      restored = await restoreViewState();
    } catch (error) {
      console.log("视图状态还原失败，回作者库:", error);
      state.homeView = "authors";
      state.activeAuthor = null;
    }
    if (!restored) render();

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

    // 每天第一次启动时自动备份数据库（v1.2.7）。
    // 失败**不能拦住启动** —— 备份只是保险，不该因为盘满/权限问题就打不开软件。
    try {
      const entry = await invoke("auto_backup_if_due");
      if (entry) toast(`已自动备份数据库（${humanSize(entry.size)}）`, "success");
    } catch (error) {
      console.log("自动备份跳过:", error);
    }

    // 字数后台补算（v1.2.8）：不 await、失败吞掉，界面该干嘛干嘛。
    // 全库要读 98 MB 正文，几千毫秒起步，绝不能挡在启动流程里。
    fillWordCountsInBackground();

    // 上一版更新完留下的旧 exe，扫一遍送回收站。
    // 等两秒再扫：新版是被旧版拉起来的，旧进程可能还没退干净，那时删不掉。
    window.setTimeout(() => { cleanupOldPortableBuilds(); }, 2000);
    // 有没有新版本：设置里关掉了就完全不发请求
    autoCheckUpdateOnStartup();
  } catch (error) {
    app.innerHTML = `<div class="fatal-error"><h1>无法初始化资料库</h1><p>${escapeHtml(String(error))}</p></div>`;
  }
}

/** 失效提醒按天记一次，而不是「看过就永远不再提」——换了一份 Cookie 又坏掉时，旧的写法再也不会提醒 */
const PIXIV_COOKIE_WARNING_DAY_KEY = "pixiv_cookie_warning_day";

/**
 * 启动自检 Cookie。v1.2.15 起后端回的是结构体而不是 bool，这里按 `status` 分情况处理。
 *
 * 只在**确认失效**（invalid）时提示：没填过 Cookie 的新用户不弹 —— 那是「还没设置」，
 * 不是「坏了」，一进软件就被拦一下很烦。
 */
async function checkPixivCookieOnStartup() {
  try {
    const probe = await invoke("check_pixiv_cookie", { cookie: null });
    if (probe?.status !== "invalid") return;
    const today = new Date().toISOString().slice(0, 10);
    if (window.localStorage.getItem(PIXIV_COOKIE_WARNING_DAY_KEY) === today) return;
    window.localStorage.setItem(PIXIV_COOKIE_WARNING_DAY_KEY, today);
    showCookieWarning(probe.message);
  } catch (error) {
    // 探测本身失败（数据库没就绪、或者后端还没编进来）不该拦住启动
    console.log("Cookie 自检失败:", error);
  }
}

function showCookieWarning(message) {
  showModal(modal("Pixiv Cookie 已失效",
    `<div class="cookie-warning">
      <p>${escapeHtml(message || "您的 Pixiv Cookie 可能已失效或未设置。")}</p>
      <p>Cookie 失效会导致：</p>
      <ul>
        <li>无法同步敏感作品</li>
        <li>无法获取完整的作品列表</li>
        <li>同步功能可能失败</li>
      </ul>
      <p>建议您在设置里更新 Cookie，再点一下「测试 Cookie」确认。</p>
    </div>`,
    `<button class="quiet-button" data-action="close-cookie-warning">稍后提醒</button>
     <button class="primary-button" data-action="go-to-settings">前往设置</button>`
  ));

  // 手动绑定按钮事件（因为弹窗是在bindEvents之后创建的）
  document.querySelector('[data-action="close-cookie-warning"]')?.addEventListener("click", () => closeModal());
  document.querySelector('[data-action="go-to-settings"]')?.addEventListener("click", async () => {
    closeModal();
    await settingsModal();
  });
}

bootstrap();
