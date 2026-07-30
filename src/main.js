import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

const app = document.querySelector("#app");
const APP_VERSION = "0.3.21";
const state = {
  authors: [],
  activeAuthor: null,
  works: [],
  authorQuery: "",
  workQuery: "",
  searchField: "title",
  status: "all",
  authorFavoritesOnly: false,
  allWorksFavoritesOnly: false,
  sort: "date_desc",
  bulkMode: false,
  selectedWorkIds: new Set(),
  pendingConfirmation: null,
  seriesView: null,
  seriesItems: [],
  homeView: "authors",
  allWorks: [],
};

const previewAuthors = [{ id: 1, name: "雾海档案", homepage: "", avatarPath: "", notes: "", previewDir: "D:\\预览", purchasedDir: "D:\\已购", matchThreshold: 70, workCount: 48, purchasedCount: 19, favoriteCount: 7 }, { id: 2, name: "Mori", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 126, purchasedCount: 52, favoriteCount: 16 }, { id: 3, name: "远野", homepage: "", avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, workCount: 33, purchasedCount: 8, favoriteCount: 4 }];
const previewWorks = [{ id: 1, title: "（插画附+改编图文）～希儿&布洛妮娅", releaseDate: "2025-10-05", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\希儿.txt", wordCount: 12680, favorite: true }, { id: 2, title: "夏日短篇集", releaseDate: "2025-09-20", previewPath: "", coverPath: "", purchasedPath: "", wordCount: 4380, favorite: false }, { id: 3, title: "旧城的信", releaseDate: "2025-08-18", previewPath: "", coverPath: "", purchasedPath: "D:\\已购\\旧城的信", wordCount: 20750, favorite: false }, { id: 4, title: "月色图文辑", releaseDate: "2025-07-09", previewPath: "", coverPath: "", purchasedPath: "", favorite: true }];

previewWorks.forEach((work, index) => { work.tags = ["Pixiv|小说", "短篇|日常", "小说", "插画|图文"][index]; work.pixivNovelId = ""; });
previewWorks.forEach((work, index) => { work.seriesId = index < 2 ? "demo-series-1" : ""; work.seriesTitle = index < 2 ? "雾海档案短篇系列" : ""; });

previewWorks.forEach((work, index) => {
  work.seriesOrder = index < 2 ? index + 1 : 0;
  work.isNew = index === 0;
  work.authorName = index < 3 ? "雾海档案" : "Mori";
  work.authorId = index < 3 ? 1 : 2;
});

async function invoke(command, args = {}) {
  if (window.__TAURI_INTERNALS__) return tauriInvoke(command, args);
  if (command === "list_authors") return previewAuthors;
  if (command === "list_works") {
    return previewWorks.filter((work) => (!args.query || work.title.includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite));
  }
  if (command === "list_all_works") return previewWorks.filter((work) => (!args.query || (args.searchField === "tags" ? work.tags : work.title).includes(args.query)) && (args.status === "all" || (args.status === "purchased") === Boolean(work.purchasedPath)) && (!args.favoritesOnly || work.favorite));
  if (command === "list_series_works") return previewWorks.filter((work) => work.seriesId === args.seriesId);
  if (command === "list_series") return [{ id: "demo-series-1", title: "雾海档案短篇系列", workCount: 2, purchasedCount: 1, previewCount: 1, coverPath: "", maxOrder: 2 }];
  if (command === "set_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = args.seriesId; work.seriesTitle = "雾海档案短篇系列"; work.seriesOrder = args.seriesOrder; } return; }
  if (command === "leave_work_series") { const work = previewWorks.find((item) => item.id === args.workId); if (work) { work.seriesId = ""; work.seriesTitle = ""; work.seriesOrder = 0; } return; }
  if (command === "toggle_favorite") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.favorite = !work.favorite; return; }
  if (command === "delete_work") { const index = previewWorks.findIndex((item) => item.id === args.workId); if (index >= 0) previewWorks.splice(index, 1); return; }
  if (command === "delete_works") { for (const workId of args.workIds) { const index = previewWorks.findIndex((item) => item.id === workId); if (index >= 0) previewWorks.splice(index, 1); } return; }
  if (command === "set_match_threshold") { const author = previewAuthors.find((item) => item.id === args.authorId); if (author) author.matchThreshold = args.threshold; return author; }
  if (command === "get_app_settings") return { pixivCookie: "", excludedTags: "", defaultPreviewDir: "", defaultPurchasedDir: "", autoCreateDirs: false, minimumFileSizeBytes: 0, pixivDelayThreshold: 150, pixivDelaySeconds: 1 };
  if (command === "save_app_settings") return args.settings;
  if (command === "read_pixiv_cookie_file") return "";
  if (command === "sync_pixiv_author_profile") return { id: args.authorId ?? null, name: "Pixiv 作者", homepage: args.homepage, avatarPath: "", notes: "", previewDir: "", purchasedDir: "", matchThreshold: 70, pixivLastSyncAt: "", avatarManaged: false };
  if (command === "update_work_tags") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.tags = args.tags.join("|"); return; }
  if (command === "copy_previews_to_purchased") return { copiedCount: args.workIds.length, boundCount: args.workIds.length, skippedCount: 0 };
  if (command === "sync_pixiv_novels") return { downloadedCount: 0, reusedPreviewCount: 0, skippedExistingCount: 0, skippedDateCount: 0, skippedSizeCount: 0, failedCount: 0, lastSyncAt: new Date().toISOString() };
  if (command === "open_work") { const work = previewWorks.find((item) => item.id === args.workId); if (work) work.isNew = false; return; }
  if (command === "open_external_url") { window.open(args.url, "_blank", "noopener,noreferrer"); return; }
  if (command === "open_help_document") { window.open("./help.html", "_blank", "noopener,noreferrer"); return; }
  throw new Error("普通浏览器预览仅展示界面；本地文件功能请在 Tauri 程序中使用。");
}

const icon = (name, size = 18) => {
  const paths = {
    plus: '<path d="M12 5v14M5 12h14"/>',
    search: '<circle cx="11" cy="11" r="6"/><path d="m16 16 4 4"/>',
    settings: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.04 2.04-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.02 1.56V20h-2.88v-.09A1.7 1.7 0 0 0 10.9 18.4a1.7 1.7 0 0 0-1.88.34l-.06.06-2.04-2.04.06-.06A1.7 1.7 0 0 0 7.32 14.8 1.7 1.7 0 0 0 5.76 13.8H5.7v-2.88h.06a1.7 1.7 0 0 0 1.56-1.02A1.7 1.7 0 0 0 6.98 8l-.06-.06L8.96 5.9l.06.06a1.7 1.7 0 0 0 1.88.34 1.7 1.7 0 0 0 1.02-1.56V4.7h2.88v.06a1.7 1.7 0 0 0 1.02 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2.04 2.04-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1.02h.06v2.88H21a1.7 1.7 0 0 0-1.6 1.2Z"/>',
    gear: '<polygon points="21 16 12 21 3 16 3 8 12 3 21 8 21 16"/><circle cx="12" cy="12" r="3"/>',
    heart: '<path d="M20.8 8.6c0 5.1-8.8 10.5-8.8 10.5S3.2 13.7 3.2 8.6A4.4 4.4 0 0 1 12 8a4.4 4.4 0 0 1 8.8.6Z"/>',
    check: '<path d="m5 12 4 4L19 6"/>',
    x: '<path d="m7 7 10 10M17 7 7 17"/>',
    folder: '<path d="M3 6.7A1.7 1.7 0 0 1 4.7 5H10l2 2h7.3A1.7 1.7 0 0 1 21 8.7v9.6a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 18.3Z"/>',
    upload: '<path d="M12 16V4M7 9l5-5 5 5"/><path d="M5 20h14"/>',
    more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
    arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
    back: '<path d="M19 12H5M11 18l-6-6 6-6"/>',
    database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v7c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12v7c0 1.7 3.6 3 8 3s8-1.3 8-3v-7"/>',
    image: '<rect x="3" y="4" width="18" height="16" rx="1"/><circle cx="8.5" cy="9" r="1.5"/><path d="m21 15-4.5-4.5L7 20"/>',
    sync: '<path d="M20 7v5h-5"/><path d="M4 17v-5h5"/><path d="M6.1 9a7 7 0 0 1 11.8-2L20 9M4 15l2.1 2A7 7 0 0 0 17.9 15"/>',
    tag: '<path d="M20 13.5 13.5 20a2.1 2.1 0 0 1-3 0L4 13.5V4h9.5L20 10.5a2.1 2.1 0 0 1 0 3Z"/><circle cx="8.5" cy="8.5" r="1"/>',
    series: '<rect x="4" y="5" width="16" height="14" rx="1"/><path d="M8 3v4M16 3v4M8 11h8M8 15h5"/>',
    file: '<path d="M7 3h7l4 4v14H7z"/><path d="M14 3v5h5M10 13h5M10 16h5"/>',
    help: '<circle cx="12" cy="12" r="9"/><path d="M9.8 9a2.3 2.3 0 1 1 3.8 1.7c-.9.7-1.6 1.2-1.6 2.5"/><path d="M12 16h.01"/>',
  };
  return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
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
    sort: state.sort,
  });
}

async function refreshAllWorks() {
  state.allWorks = await invoke("list_all_works", {
    query: state.workQuery,
    searchField: state.searchField,
    status: state.status,
    favoritesOnly: state.allWorksFavoritesOnly,
    sort: state.sort,
  });
}

function render() {
  app.innerHTML = state.activeAuthor
    ? (state.seriesView ? renderSeriesView() : renderWorks())
    : (state.homeView === "allWorks" ? renderAllWorks() : state.homeView === "help" ? renderHelp() : renderAuthors());
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
  return [...state.works, ...state.seriesItems, ...state.allWorks].find((work) => work.id === workId);
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
        <button class="rail-button ${state.homeView === "help" && !state.activeAuthor ? "is-active" : ""}" title="帮助" aria-label="帮助" data-action="help">${icon("help", 20)}</button>
      </nav>
      <div class="rail-footer">
        <span class="app-version" title="当前版本">v${APP_VERSION}</span>
        <button class="rail-button rail-settings-button" title="设置" aria-label="设置" data-action="settings">${icon("gear", 25)}<span>设置</span></button>
      </div>
    </aside>
    <main class="workspace">${content}</main>
  </div>`;
}

function renderHelp() {
  return renderShell(`
    <section class="topbar help-topbar">
      <div><p class="section-kicker">使用指南与项目资料</p><h1>帮助</h1></div>
    </section>
    <section class="help-content">
      <div class="help-link-bar">
        <span>项目主页</span>
        <a href="https://github.com/fromzero1501/pixiv-novel-downloader" data-action="open-external-url" data-url="https://github.com/fromzero1501/pixiv-novel-downloader">fromzero1501/pixiv-novel-downloader</a>
      </div>
      <div class="help-link-bar">
        <span>使用指南</span>
        <a href="help.html" data-action="open-help-document">在浏览器中打开完整帮助文档</a>
      </div>
    </section>`);
}

async function openExternalUrl(url) {
  if (!/^https?:\/\//i.test(url || "")) throw new Error("仅支持在浏览器中打开 HTTP 或 HTTPS 链接");
  await invoke("open_external_url", { url });
}

async function openHelpDocument() {
  await invoke("open_help_document");
}

function renderAuthors() {
  const query = state.authorQuery.trim().toLowerCase();
  const authors = state.authors.filter((author) => author.name.toLowerCase().includes(query));
  const cards = authors.map((author) => `
    <article class="author-card" data-author-id="${author.id}" tabindex="0">
      <div class="author-avatar ${author.avatarPath ? "has-image" : ""}">
        ${author.avatarPath ? `<img src="${asset(author.avatarPath)}" alt="${escapeHtml(author.name)} 的头像">` : `<span>${escapeHtml(initials(author.name))}</span>`}
      </div>
      <div class="author-card-body">
        <div class="author-card-title-row"><h2>${escapeHtml(author.name)}</h2><button class="icon-button card-edit" title="编辑作者" data-action="edit-author" data-author-id="${author.id}">${icon("more", 18)}</button></div>
        <dl class="author-stats"><div><dt>作品</dt><dd>${author.workCount}</dd></div><div><dt>完整版</dt><dd>${author.purchasedCount}</dd></div><div><dt>收藏</dt><dd>${author.favoriteCount}</dd></div></dl>
      </div>
      <div class="card-enter">${icon("arrow", 18)}</div>
    </article>`).join("");

  return renderShell(`
    <section class="topbar">
      <div><p class="section-kicker">私人作品档案</p><h1>作者库</h1></div>
      <div class="topbar-actions"><button class="icon-text-button" data-action="settings">${icon("database", 18)}<span>备份与恢复</span></button><button class="primary-button" data-action="new-author">${icon("plus", 18)}<span>新增作者</span></button></div>
    </section>
    <section class="authors-content">
      <label class="search-field"><span>${icon("search", 19)}</span><input id="author-search" type="search" placeholder="搜索作者" value="${escapeHtml(state.authorQuery)}" autocomplete="off"></label>
      <div class="section-row"><p>${authors.length ? `共 ${authors.length} 位作者` : ""}</p></div>
      <div class="author-grid">${cards || renderEmptyAuthors()}</div>
    </section>`);
}

function renderEmptyAuthors() {
  return `<div class="empty-state"><div class="empty-icon">${icon("image", 26)}</div><h2>还没有作者</h2><p>建立第一位作者后，即可导入作品并绑定本地文件。</p><button class="primary-button" data-action="new-author">${icon("plus", 18)}<span>新增作者</span></button></div>`;
}

function workCover(work) {
  if (work.coverPath) return `<img src="${asset(work.coverPath)}" alt="${escapeHtml(work.title)} 的封面">`;
  return `<div class="cover-placeholder"><span>${icon("image", 28)}</span><small>暂无封面</small></div>`;
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

function renderWorks() {
  const author = state.activeAuthor;
  const cards = state.works.map((work) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"} ${state.bulkMode ? "is-selecting" : ""}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        <div class="work-badges">
          <span class="status-badge ${work.purchasedPath ? "owned" : "unowned"}" title="${work.purchasedPath ? "完整版已绑定" : "预览版"}">${icon(work.purchasedPath ? "check" : "x", 17)}</span>
          <button class="favorite-badge ${work.favorite ? "is-favorite" : ""}" title="${work.favorite ? "已收藏，点击取消" : "未收藏，点击收藏"}" data-action="toggle-favorite" data-work-id="${work.id}">${icon("heart", 17)}</button>
        </div>
        ${state.bulkMode ? `<button class="selection-badge ${state.selectedWorkIds.has(work.id) ? "is-selected" : ""}" title="${state.selectedWorkIds.has(work.id) ? "取消选择" : "选择作品"}" data-action="toggle-select" data-work-id="${work.id}">${state.selectedWorkIds.has(work.id) ? icon("check", 16) : ""}</button>` : `<button class="work-menu" title="更多操作" data-action="work-menu" data-work-id="${work.id}">${icon("more", 18)}</button>`}
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${workSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`).join("");

  return renderShell(`
    <section class="topbar work-topbar">
      <div class="crumb-heading"><button class="back-button" title="返回作者库" data-action="go-home">${icon("back", 20)}</button><div><p class="section-kicker">作者作品库</p><h1>${escapeHtml(author.name)}</h1></div></div>
      <div class="topbar-actions">${state.bulkMode ? `<button class="quiet-button" data-action="bulk-mode">取消</button><button class="quiet-button" data-action="select-all" ${state.works.length ? "" : "disabled"}>${state.works.length && state.works.every((work) => state.selectedWorkIds.has(work.id)) ? "取消全选" : "全选"}</button><button class="quiet-button" data-action="copy-selected-full" ${state.selectedWorkIds.size ? "" : "disabled"}>设为完整版（${state.selectedWorkIds.size}）</button><button class="danger-button" data-action="delete-selected" ${state.selectedWorkIds.size ? "" : "disabled"}>删除已选（${state.selectedWorkIds.size}）</button>` : `<button class="icon-text-button" data-action="bulk-mode">${icon("more", 18)}<span>批量操作</span></button><button class="icon-text-button" data-action="edit-author" data-author-id="${author.id}">${icon("settings", 18)}<span>作者设置</span></button><button class="icon-text-button" data-action="import-works">${icon("plus", 18)}<span>导入作品</span></button><button class="primary-button" data-action="sync-pixiv">${icon("sync", 18)}<span>作品同步</span></button>`}</div>
    </section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${state.searchField === "tags" ? "搜索标签" : "搜索作品名称"}" value="${escapeHtml(state.workQuery)}" autocomplete="off"></label>
        <select class="sort-select search-mode-select" id="search-field" aria-label="搜索范围"><option value="title" ${state.searchField === "title" ? "selected" : ""}>标题</option><option value="tags" ${state.searchField === "tags" ? "selected" : ""}>标签</option></select>
        <div class="filter-group" role="group" aria-label="版本状态">${[ ["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"] ].map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.authorFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <select class="sort-select" id="sort-select" aria-label="排序"><option value="date_desc" ${state.sort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.sort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.sort === "title_asc" ? "selected" : ""}>名称 A-Z</option></select>
      </div>
      <div class="binding-bar"><div><strong>本地文件</strong><span>${author.previewDir ? "预览版目录已绑定" : "尚未绑定预览版目录"} · ${author.purchasedDir ? "完整版目录已绑定" : "尚未绑定完整版目录"}</span><em class="sync-status">Pixiv ${syncLabel(author.pixivLastSyncAt)}</em></div><div><button class="quiet-button" data-action="scan-preview">${icon("folder", 17)}关联预览版文件</button><button class="quiet-button" data-action="scan-purchased">${icon("upload", 17)}关联完整版文件</button></div></div>
      <div class="works-grid">${cards || renderEmptyWorks()}</div>
    </section>`);
}

function renderAllWorks() {
  const cards = state.allWorks.map((work) => `
    <article class="work-card is-read-only ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}
        ${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        <div class="work-badges"><span class="status-badge ${work.purchasedPath ? "owned" : "unowned"}" title="${work.purchasedPath ? "完整版已绑定" : "预览版"}">${icon(work.purchasedPath ? "check" : "x", 17)}</span></div>
      </div>
      <div class="work-copy"><p class="work-author">${escapeHtml(work.authorName || "")}</p><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 title="${escapeHtml(work.title)}">${escapeHtml(work.title)}</h2>${allWorkSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
    </article>`).join("");
  return renderShell(`
    <section class="topbar work-topbar"><div><p class="section-kicker">全部作者</p><h1>所有作品</h1></div></section>
    <section class="library-content">
      <div class="library-tools">
        <label class="search-field"><span>${icon("search", 19)}</span><input id="work-search" type="search" placeholder="${state.searchField === "tags" ? "搜索标签" : "搜索作品名称"}" value="${escapeHtml(state.workQuery)}" autocomplete="off"></label>
        <select class="sort-select search-mode-select" id="search-field" aria-label="搜索范围"><option value="title" ${state.searchField === "title" ? "selected" : ""}>标题</option><option value="tags" ${state.searchField === "tags" ? "selected" : ""}>标签</option></select>
        <div class="filter-group" role="group" aria-label="版本状态">${[ ["all", "全部"], ["purchased", "完整版"], ["unpurchased", "预览版"] ].map(([value, label]) => `<button class="filter-button ${state.status === value ? "is-active" : ""}" data-action="status" data-status="${value}">${label}</button>`).join("")}</div>
        <button class="icon-text-button favorite-filter ${state.allWorksFavoritesOnly ? "is-active" : ""}" data-action="favorites-only">${icon("heart", 17)}<span>仅看收藏</span></button>
        <select class="sort-select" id="sort-select" aria-label="排序"><option value="date_desc" ${state.sort === "date_desc" ? "selected" : ""}>日期从新到旧</option><option value="date_asc" ${state.sort === "date_asc" ? "selected" : ""}>日期从旧到新</option><option value="title_asc" ${state.sort === "title_asc" ? "selected" : ""}>名称 A-Z</option></select>
      </div>
      <div class="read-only-note">所有作品仅供搜索、筛选与打开查看。</div>
      <div class="works-grid">${cards || '<div class="empty-state works-empty"><h2>没有符合条件的作品</h2></div>'}</div>
    </section>`);
}

function renderSeriesWorkCards(works) {
  return works.map((work, index) => `
    <article class="work-card ${work.purchasedPath ? "is-purchased" : "is-unpurchased"}" data-work-id="${work.id}" tabindex="0">
      <div class="work-cover">${workCover(work)}${work.isNew ? '<span class="new-badge">NEW</span>' : ""}
        <div class="work-badges">
          <span class="status-badge ${work.purchasedPath ? "owned" : "unowned"}" title="${work.purchasedPath ? "完整版已绑定" : "预览版"}">${icon(work.purchasedPath ? "check" : "x", 17)}</span>
          <button class="favorite-badge ${work.favorite ? "is-favorite" : ""}" title="${work.favorite ? "已收藏，点击取消" : "未收藏，点击收藏"}" data-action="toggle-favorite" data-work-id="${work.id}">${icon("heart", 17)}</button>
        </div>
        <button class="work-menu" title="更多操作" data-action="work-menu" data-work-id="${work.id}">${icon("more", 18)}</button>
      </div>
      <div class="work-copy"><div class="work-meta"><p class="work-date">${dateLabel(work.releaseDate)}</p>${workContentMeta(work)}</div><h2 title="${escapeHtml(work.title)}"><span class="work-index">${work.seriesOrder || index + 1}.</span>${escapeHtml(work.title)}</h2>${workSeries(work)}${work.tags ? `<div class="work-tags">${work.tags.split("|").filter(Boolean).map((tag) => `<span>${icon("tag", 12)}${escapeHtml(tag.trim())}</span>`).join("")}</div>` : ""}</div>
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

function modal(title, body, footer = "") {
  return `<div class="modal-layer" role="presentation"><section class="modal" role="dialog" aria-modal="true" aria-label="${escapeHtml(title)}"><header><h2>${escapeHtml(title)}</h2><button class="icon-button" data-action="close-modal" title="关闭">×</button></header><div class="modal-body">${body}</div>${footer ? `<footer class="modal-footer">${footer}</footer>` : ""}</section></div>`;
}

function showModal(markup) {
  document.body.insertAdjacentHTML("beforeend", markup);
  document.querySelector(".modal-layer")?.addEventListener("click", (event) => { if (event.target.classList.contains("modal-layer")) closeModal(); });
  bindEvents();
}

function closeModal() {
  document.querySelector(".modal-layer")?.remove();
  state.pendingConfirmation = null;
}

function confirmAction(title, message, label, action) {
  closeModal();
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
      <label>关联相似度 <input name="matchThreshold" type="number" min="1" max="100" step="1" value="${author.matchThreshold || 70}"><small>用于关联预览版、封面和完整版文件。名称不完全一致时，达到此值才会作为自动关联候选。</small></label>
      <label>Pixiv 作者主页 <input name="homepage" type="url" value="${escapeHtml(author.homepage || "")}" placeholder="https://www.pixiv.net/users/123456"><small>填写有效主页后，点击“同步作者信息”会自动获取作者名称和头像。</small></label>
      <label>头像文件 <div class="path-input"><input name="avatarPath" value="${escapeHtml(author.avatarPath || "")}" readonly placeholder="尚未选择"><button type="button" class="quiet-button" data-action="pick-avatar">选择图片</button></div></label>
      <label>预览版文件夹 <div class="path-input"><input name="previewDir" value="${escapeHtml(author.previewDir || "")}" readonly placeholder="可在稍后绑定"><button type="button" class="quiet-button" data-action="pick-preview-dir">选择文件夹</button></div><small>在设置中配置默认目录并开启自动创建作者目录后，保存作者时会自动生成，无需手动选择。</small></label>
      <label>完整版文件夹 <div class="path-input"><input name="purchasedDir" value="${escapeHtml(author.purchasedDir || "")}" readonly placeholder="可在稍后绑定"><button type="button" class="quiet-button" data-action="pick-purchased-dir">选择文件夹</button></div><small>在设置中配置默认目录并开启自动创建作者目录后，保存作者时会自动生成，无需手动选择。</small></label>
      <label>备注 <textarea name="notes" rows="3" placeholder="可记录来源、说明等">${escapeHtml(author.notes || "")}</textarea></label>
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

function workMenu(work) {
  showModal(modal(work.title, `<div class="menu-list"><button data-action="open-work" data-work-id="${work.id}">${icon("arrow", 18)}打开${work.purchasedPath ? "完整版" : "预览版"}</button><button data-action="bind-work-file" data-work-id="${work.id}">${icon("folder", 18)}绑定完整版文件</button><button data-action="edit-tags" data-work-id="${work.id}">${icon("tag", 18)}编辑标签</button><button data-action="toggle-favorite" data-work-id="${work.id}">${icon("heart", 18)}${work.favorite ? "取消收藏" : "收藏"}</button><button class="menu-danger" data-action="delete-work" data-work-id="${work.id}">${icon("more", 18)}删除作品</button></div>`));
}

function editTagsModal(workId, tags = null) {
  const work = findWork(workId);
  if (!work) return;
  const values = tags || work.tags.split("|").filter(Boolean).map((tag) => tag.trim());
  showModal(modal("编辑标签", `<div class="tag-editor" data-work-id="${workId}"><div class="tag-editor-list">${values.map((tag, index) => `<span>${escapeHtml(tag)}<button title="删除标签" data-action="remove-tag" data-index="${index}">×</button></span>`).join("")}</div><input id="tag-editor-input" placeholder="输入标签后按 Enter 添加" autocomplete="off"></div>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" data-action="save-tags" data-work-id="${workId}">保存标签</button>`));
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
    bindingBar.insertAdjacentHTML("afterend", `<div class="library-summary"><span>作品 <strong>${state.activeAuthor.workCount}</strong></span><span>完整版 <strong>${state.activeAuthor.purchasedCount}</strong></span><span>预览版 <strong>${Math.max(0, state.activeAuthor.workCount - state.activeAuthor.purchasedCount)}</strong></span></div>`);
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
      state.workQuery = query;
      if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks();
      if (state.workQuery !== query) return;
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
      if (action === "go-home") { state.activeAuthor = null; state.homeView = "authors"; state.seriesView = null; state.seriesItems = []; state.authorQuery = ""; await refreshAuthors(); render(); }
      if (action === "go-all-works") { state.activeAuthor = null; state.homeView = "allWorks"; state.seriesView = null; state.seriesItems = []; state.workQuery = ""; await refreshAllWorks(); render(); }
      if (action === "help") { state.activeAuthor = null; state.homeView = "help"; state.seriesView = null; state.seriesItems = []; render(); }
      if (action === "open-external-url") { event.preventDefault(); await openExternalUrl(url); }
      if (action === "open-help-document") { event.preventDefault(); await openHelpDocument(); }
      if (action === "new-author") authorModal();
      if (action === "edit-author") { const author = state.authors.find((item) => item.id === Number(authorId)) || state.activeAuthor; authorModal(author); }
      if (action === "sync-author-profile") await syncAuthorProfile();
      if (action === "close-modal") closeModal();
      if (action === "status") { state.status = status; if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); }
      if (action === "favorites-only") { if (state.homeView === "allWorks" && !state.activeAuthor) { state.allWorksFavoritesOnly = !state.allWorksFavoritesOnly; await refreshAllWorks(); } else { state.authorFavoritesOnly = !state.authorFavoritesOnly; await refreshWorks(); } render(); }
      if (action === "import-works") importModal();
      if (action === "sync-pixiv") pixivSyncModal();
      if (action === "scan-preview") await scanPreview();
      if (action === "scan-purchased") await scanPurchased();
      if (action === "work-menu") workMenu(findWork(Number(workId)));
      if (action === "open-series") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, state.seriesView?.returnTo || "works");
      if (action === "open-all-series") await openAllWorksSeries(Number(authorId), element.dataset.seriesId, element.dataset.seriesTitle);
      if (action === "open-series-library") await openSeriesLibrary();
      if (action === "open-series-card") await openSeriesDetail(element.dataset.seriesId, element.dataset.seriesTitle, "overview");
      if (action === "close-series-view") await closeSeriesView();
      if (action === "edit-tags") { closeModal(); editTagsModal(Number(workId)); }
      if (action === "join-series") await chooseSeriesForWork(Number(workId));
      if (action === "change-series-order") await chooseSeriesForWork(Number(workId));
      if (action === "set-work-series") {
        const form = document.querySelector("#series-form");
        const values = Object.fromEntries(new FormData(form).entries());
        await setWorkSeries(Number(workId), values.seriesId, Number(values.seriesOrder));
      }
      if (action === "leave-series") await leaveWorkSeries(Number(workId));
      if (action === "remove-tag") removeEditingTag(Number(element.dataset.index));
      if (action === "save-tags") await saveTags(Number(workId));
      if (action === "toggle-favorite") { await invoke("toggle_favorite", { workId: Number(workId) }); closeModal(); await refreshWorks(); await refreshAuthors(); render(); }
      if (action === "toggle-select") toggleWorkSelection(Number(workId));
      if (action === "bulk-mode") toggleBulkMode();
      if (action === "select-all") toggleSelectAll();
      if (action === "copy-selected-full") await copySelectedToFull();
      if (action === "delete-work") await deleteWork(Number(workId));
      if (action === "delete-selected") await deleteSelectedWorks();
      if (action === "open-work") { await invoke("open_work", { workId: Number(workId) }); closeModal(); if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks(); render(); }
      if (action === "bind-work-file") await bindWork(Number(workId), false);
      if (action === "pick-avatar") await pickPath("avatarPath", false, ["jpg", "jpeg", "png", "webp"]);
      if (action === "pick-preview-dir") await pickPath("previewDir", true);
      if (action === "pick-purchased-dir") await pickPath("purchasedDir", true);
      if (action === "pick-import-file") await pickPath("filePath", false, ["csv", "xlsx", "xls"]);
      if (action === "pick-import-folder") await pickPath("folderPath", true);
      if (action === "submit-import") await submitImport();
      if (action === "settings") await settingsModal();
      if (action === "pick-pixiv-cookie") await importPixivCookie();
      if (action === "pick-default-preview") await pickPath("defaultPreviewDir", true);
      if (action === "pick-default-purchased") await pickPath("defaultPurchasedDir", true);
      if (action === "confirm-pixiv-sync") await syncPixivWorks();
      if (action === "cancel-pixiv-sync") await cancelPixivSync();
      if (action === "delete-author") await deleteAuthor(Number(authorId));
      if (action === "export-backup") await exportBackup();
      if (action === "restore-backup") await restoreBackup();
      if (action === "confirm-matches") await confirmMatches();
      if (action === "confirm-action") await runConfirmedAction();
    } catch (error) { toast(String(error), "error"); }
    });
  });

  document.querySelectorAll(".author-card").forEach((card) => {
    if (card.dataset.bound) return;
    card.dataset.bound = "true";
    card.addEventListener("click", async (event) => {
    if (event.target.closest("button")) return;
    state.activeAuthor = state.authors.find((author) => author.id === Number(card.dataset.authorId));
    state.homeView = "authors";
    state.authorFavoritesOnly = false;
    await refreshWorks(); render();
    });
  });

  document.querySelectorAll(".work-card").forEach((card) => {
    if (card.dataset.bound) return;
    card.dataset.bound = "true";
    card.addEventListener("click", async (event) => {
    if (event.target.closest("button")) return;
    if (state.bulkMode) { toggleWorkSelection(Number(card.dataset.workId)); return; }
    try {
      await invoke("open_work", { workId: Number(card.dataset.workId) });
      if (state.homeView === "allWorks" && !state.activeAuthor) await refreshAllWorks(); else await refreshWorks();
      render();
    } catch (error) { toast(String(error), "error"); }
    });
    card.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      if (card.classList.contains("is-read-only")) return;
      editTagsModal(Number(card.dataset.workId));
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
        const threshold = Number(author.matchThreshold || 70);
        if (!Number.isInteger(threshold) || threshold < 1 || threshold > 100) throw new Error("关联相似度请输入 1 到 100 之间的整数");
        author.id = author.id ? Number(author.id) : null;
        author.avatarManaged = author.avatarManaged === "true";
        const saved = await invoke("save_author", { author });
        const result = await invoke("set_match_threshold", { authorId: saved.id, threshold });
        await refreshAuthors();
        if (state.activeAuthor?.id === result.id) state.activeAuthor = result;
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
    settingsForm.insertAdjacentHTML("beforeend", `<label class="delay-settings">Pixiv 抓取间隔 <div class="delay-input"><span>同步作品超过</span><input name="pixivDelayThreshold" type="number" min="1" step="1" value="${Number(settings.pixivDelayThreshold || 150)}"><span>部时，每部间隔</span><input name="pixivDelaySeconds" type="number" min="0" max="60" step="1" value="${Number(settings.pixivDelaySeconds ?? 1)}"><span>秒</span></div><small>超过阈值后，作品详情请求会按此间隔执行，降低连续抓取频率。默认超过 150 部时每部间隔 1 秒；填 0 秒可关闭间隔。</small></label>`);
    settingsForm.dataset.bound = "true";
    settingsForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      const values = Object.fromEntries(new FormData(event.currentTarget).entries());
      values.autoCreateDirs = Boolean(document.querySelector('[name="autoCreateDirs"]')?.checked);
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
      await invoke("save_app_settings", { settings: values });
      closeModal();
      toast("软件设置已保存", "success");
    });
  }

  const tagInput = document.querySelector("#tag-editor-input");
  if (tagInput && !tagInput.dataset.bound) {
    tagInput.dataset.bound = "true";
    tagInput.addEventListener("keydown", (event) => { if (event.key === "Enter") { event.preventDefault(); addEditingTag(); } });
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
}

async function pickPath(field, directory, extensions) {
  const path = await open({ directory, multiple: false, filters: extensions ? [{ name: "文件", extensions }] : undefined });
  if (path) document.querySelector(`[name="${field}"]`).value = path;
}

async function syncAuthorProfile() {
  const form = document.querySelector("#author-form");
  const values = Object.fromEntries(new FormData(form).entries());
  if (!values.homepage) throw new Error("请先填写 Pixiv 作者主页。");
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
    authorModal(author);
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

async function scanPurchased() {
  if (!state.activeAuthor.purchasedDir) {
    throw new Error("请先在作者设置中选择完整版文件夹");
  }
  const result = await invoke("scan_purchased", { authorId: state.activeAuthor.id });
  if (result.selections.length) {
    toast(`已自动绑定 ${result.boundCount} 个作品，${result.selections.length} 个完整版文件待您选择`, "info");
    showPurchasedSelections(result.selections);
  } else toast(`已自动绑定 ${result.boundCount} 个完整版作品`, "success");
  await refreshWorks(); await refreshActiveAuthor(); render();
}

async function bindWork(workId, directory) {
  const path = await open({ directory, multiple: false });
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
  const areAllSelected = state.works.length > 0 && state.works.every((work) => state.selectedWorkIds.has(work.id));
  if (areAllSelected) state.selectedWorkIds.clear();
  else state.works.forEach((work) => state.selectedWorkIds.add(work.id));
  render();
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
  toast(`已复制 ${result.copiedCount} 条预览版并绑定 ${result.boundCount} 条完整版${skipped}`, result.skippedCount ? "info" : "success");
}

function showPurchasedSelections(selections) {
  const rows = selections.map((selection) => `<label class="match-row"><span>完整版文件：${escapeHtml(selection.path.split(/[\\/]/).pop())}</span><select data-purchased-path="${escapeHtml(selection.path)}"><option value="">暂不绑定</option>${selection.candidates.map((candidate) => `<option value="${candidate.workId}">${escapeHtml(candidate.title)}（${candidate.similarity}%）</option>`).join("")}</select></label>`).join("");
  showModal(modal("选择要关联的作品", `<p class="match-note">有多个候选作品时请手动选择；未达到相似度阈值的文件仅列出相似度最高的 3 个作品。</p><div class="match-list">${rows}</div>`, `<button class="quiet-button" data-action="close-modal">稍后处理</button><button class="primary-button" data-action="confirm-matches">确认绑定</button>`));
}

async function confirmMatches() {
  const selections = [...document.querySelectorAll("[data-purchased-path]")].map((select) => ({ workId: Number(select.value), path: select.dataset.purchasedPath })).filter((item) => item.workId);
  for (const item of selections) await invoke("bind_work", item);
  closeModal(); await refreshWorks(); await refreshActiveAuthor(); render();
  toast(`已确认绑定 ${selections.length} 个完整版作品`, "success");
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
      <div class="sync-progress is-hidden" id="sync-progress"><div><strong id="sync-progress-label">准备同步</strong><span id="sync-progress-count">0 / 0</span></div><progress id="sync-progress-bar" value="0" max="1"></progress><p id="sync-progress-title"></p></div>
    </form>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="danger-button" data-action="cancel-pixiv-sync" disabled>终止同步</button><button class="primary-button" data-action="confirm-pixiv-sync" ${author.homepage && author.previewDir && author.purchasedDir ? "" : "disabled"}>${icon("sync", 17)}开始同步</button>`));
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
  const button = document.querySelector('[data-action="confirm-pixiv-sync"]');
  const cancelButton = document.querySelector('[data-action="cancel-pixiv-sync"]');
  const { startDate = "", endDate = "", novelUrl = "" } = Object.fromEntries(new FormData(form).entries());
  button.disabled = true;
  button.textContent = "正在同步...";
  cancelButton.disabled = false;
  const progress = document.querySelector("#sync-progress");
  progress.classList.remove("is-hidden");
  const unlisten = await listen("pixiv-sync-progress", (event) => {
    const { total, current, title } = event.payload;
    document.querySelector("#sync-progress-bar").max = Math.max(total, 1);
    document.querySelector("#sync-progress-bar").value = current;
    document.querySelector("#sync-progress-count").textContent = `${current} / ${total}`;
    document.querySelector("#sync-progress-title").textContent = title || "正在读取作品列表...";
  });
  let result;
  try {
    result = await invoke("sync_pixiv_novels", { authorId: state.activeAuthor.id, startDate, endDate, novelUrl });
  } catch (error) {
    button.disabled = false;
    button.innerHTML = `${icon("sync", 17)}开始同步`;
    cancelButton.disabled = true;
    progress.classList.add("is-hidden");
    throw error;
  } finally {
    unlisten();
  }
  await refreshAuthors();
  state.activeAuthor = state.authors.find((author) => author.id === state.activeAuthor.id) || state.activeAuthor;
  await refreshWorks();
  closeModal();
  render();
  const summary = `已下载 ${result.downloadedCount} 篇；已关联 ${result.reusedPreviewCount || 0} 篇已有预览版；已跳过 ${result.skippedExistingCount} 篇已有作品；日期筛除 ${result.skippedDateCount} 篇；大小筛除 ${result.skippedSizeCount || 0} 篇`;
  toast(result.cancelled ? `同步已终止；${summary}` : (result.failedCount ? `${summary}；${result.failedCount} 篇失败，将在下次同步时重试` : summary), result.cancelled || result.failedCount ? "info" : "success");
}

async function cancelPixivSync() {
  const button = document.querySelector('[data-action="cancel-pixiv-sync"]');
  if (button) { button.disabled = true; button.textContent = "正在终止..."; }
  await invoke("cancel_pixiv_sync", { authorId: state.activeAuthor.id });
  const label = document.querySelector("#sync-progress-label");
  if (label) label.textContent = "将在当前请求结束后终止";
}

function editingTags() { return [...document.querySelectorAll(".tag-editor-list > span")].map((item) => item.firstChild.textContent.trim()); }
function addEditingTag() { const input = document.querySelector("#tag-editor-input"); const value = input?.value.trim(); if (!value) return; const workId = Number(document.querySelector(".tag-editor")?.dataset.workId); const tags = editingTags(); input.value = ""; closeModal(); editTagsModal(workId, [...tags, value]); }
function removeEditingTag(index) { const workId = Number(document.querySelector(".tag-editor")?.dataset.workId); const tags = editingTags(); tags.splice(index, 1); closeModal(); editTagsModal(workId, tags); }
async function saveTags(workId) { await invoke("update_work_tags", { workId, tags: editingTags() }); closeModal(); await refreshWorks(); render(); }

async function importPixivCookie() {
  const path = await open({ multiple: false, directory: false, filters: [{ name: "Cookie", extensions: ["json", "txt"] }] });
  if (!path) return;
  document.querySelector('[name="pixivCookie"]').value = await invoke("read_pixiv_cookie_file", { path });
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
  showModal(modal("设置", `<div class="settings-copy"><form id="settings-form" class="form-stack"><label>Pixiv Cookie <textarea name="pixivCookie" rows="3" placeholder="直接粘贴 Cookie，或从文件导入">${escapeHtml(settings.pixivCookie || "")}</textarea><button type="button" class="quiet-button" data-action="pick-pixiv-cookie">从文件导入</button><small>仅保存 Pixiv 接口需要的 PHPSESSID 到数据库。</small></label><label>排除标签 <input name="excludedTags" value="${escapeHtml(settings.excludedTags || "")}" placeholder="标签A, 标签B"><small>用中英文逗号分隔。包含这些文字的 Pixiv 标签不会记录。</small></label><label>最小文件大小 <div class="size-input"><input name="minimumFileSize" type="number" min="0" step="0.1" value="${minimumSize.value}"><select name="minimumFileSizeUnit" aria-label="最小文件大小单位">${["KB", "MB", "GB"].map((unit) => `<option value="${unit}" ${minimumSize.unit === unit ? "selected" : ""}>${unit}</option>`).join("")}</select></div><small>导入文件夹和 Pixiv 同步时，会跳过小于此大小的文本内容；文件夹不受此限制。</small></label><label>预览版文件夹默认目录 <div class="path-input"><input name="defaultPreviewDir" value="${escapeHtml(settings.defaultPreviewDir || "")}" readonly><button type="button" class="quiet-button" data-action="pick-default-preview">选择目录</button></div></label><label>完整版文件夹默认目录 <div class="path-input"><input name="defaultPurchasedDir" value="${escapeHtml(settings.defaultPurchasedDir || "")}" readonly><button type="button" class="quiet-button" data-action="pick-default-purchased">选择目录</button></div></label><label class="check-row"><input name="autoCreateDirs" type="checkbox" ${settings.autoCreateDirs ? "checked" : ""}><span>自动创建作者目录</span><small>新建或保存路径为空的作者时，在默认目录中创建以作者名称命名的文件夹。</small></label></form><div class="menu-list settings-actions"><button data-action="export-backup">${icon("database", 18)}导出数据库备份</button><button data-action="restore-backup">${icon("upload", 18)}从备份恢复</button></div></div>`, `<button class="quiet-button" data-action="close-modal">取消</button><button class="primary-button" form="settings-form" type="submit">保存设置</button>`));
}

async function bootstrap() {
  try { await refreshAuthors(); render(); } catch (error) { app.innerHTML = `<div class="fatal-error"><h1>无法初始化资料库</h1><p>${escapeHtml(String(error))}</p></div>`; }
}

bootstrap();
