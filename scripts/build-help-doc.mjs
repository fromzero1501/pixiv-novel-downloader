// 把 docs/index.html（GitHub Pages 上的那份图文帮助）编译成应用内帮助页能直接用的东西。
//
// 用户要求（2026-09-15）：**帮助内容只维护一份**，不再另做外置文档。所以：
//   docs/index.html  = 唯一内容源（Pages 直接用它）
//   src/help-doc.js  = 由它生成，供应用内「帮助」页（Shadow DOM）渲染
//   public/help/     = 由它生成的配图，应用内按 help/images/xxx.png 取
//
// 生成物不要手改、也不要提交：改内容请改 docs/index.html，然后重新跑一次构建
// （`npm run build` / `npm run portable` 会自动调用本脚本）。
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(root, "docs", "index.html");
const outputJs = join(root, "src", "help-doc.js");
const imageSourceDir = join(root, "docs", "images");
const imageTargetDir = join(root, "public", "help", "images");

function fail(message) {
  console.error(`[help-doc] ${message}`);
  process.exit(1);
}

if (!existsSync(source)) fail(`找不到帮助页源文件：${source}`);

const page = readFileSync(source, "utf8");

// --- 取 <style> 和 <body> 里的正文 -----------------------------------------
const styleMatch = page.match(/<style>([\s\S]*?)<\/style>/i);
if (!styleMatch) fail("docs/index.html 里找不到 <style> 块");

const bodyMatch = page.match(/<body>([\s\S]*)<\/body>/i);
if (!bodyMatch) fail("docs/index.html 里找不到 <body> 块");

// 正文连 .wrap 一起搬（.wrap 负责 900px 居中栏宽，去掉它正文会摊满整个工作区）
const inner = bodyMatch[1].trim();
if (inner.length < 5000 || !inner.includes('class="wrap"') || !inner.includes("<section")) {
  fail("正文结构不像预期（没取到 .wrap），请检查 docs/index.html");
}

// --- CSS 改写 --------------------------------------------------------------
// 页面里 :root / body / html 这几个选择器进了 Shadow DOM 就没有对应元素了：
//   :root → :host（自定义属性照样向下继承）
//   body  → :host（字体、字号、颜色靠继承进子树）
//   html  → 丢掉（平滑滚动与 scroll-padding 由外层页面负责）
// 背景色也去掉，让应用自己的底色透上来，免得工作区中间糊一块灰。
let css = styleMatch[1];
if (!/:root\s*\{/.test(css)) fail("CSS 里找不到 :root，改写规则需要同步更新");

css = css
  .replace(/html\s*\{[^}]*\}/, "")
  .replace(/:root\s*\{/, ":host {")
  .replace(/^[ \t]*body\s*\{[^}]*\}[ \t]*$/m, "")
  .replace(/\n{3,}/g, "\n\n");

// 上面把 body 整块删掉了，这里按源文件的属性补一份到 :host 上（不抄 background）
const hostBlock = `  :host {
    display: block;
    color: var(--ink);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "Microsoft YaHei", "PingFang SC", "Hiragino Sans GB", sans-serif;
    font-size: 15.5px;
    line-height: 1.75;
    -webkit-font-smoothing: antialiased;
  }
  /* 应用里没有顶栏吸附，锚点跳转留一点余量就够 */
  [id] { scroll-margin-top: 24px; }
`;
css = hostBlock + css;

// --- HTML 改写 -------------------------------------------------------------
// 1. 版本号写成占位符，运行时用 APP_VERSION 替换（页眉徽标 + 页脚两处）
// 2. 配图路径加 help/ 前缀（生成物在 public/help/images/ 下）
// 3. 图片懒加载：正文很长，没必要一打开就解码全部截图
const referenced = new Set();
let html = inner
  .replace(/v1\.0\.0/g, "__APP_VERSION__")
  .replace(/src="images\/([^"]+)"/g, (_, name) => {
    referenced.add(name);
    return `src="help/images/${name}"`;
  })
  .replace(/<img /g, '<img loading="lazy" decoding="async" ');

if (!referenced.size) fail("正文里没有解析到任何配图，请检查 docs/index.html 的图片路径");

// --- 输出 JS 模块 ----------------------------------------------------------
const banner = `// 本文件由 scripts/build-help-doc.mjs 从 docs/index.html 自动生成，请勿手改。\n// 改帮助内容请改 docs/index.html，然后重新构建（npm run build / npm run portable）。\n`;
const module =
  banner +
  `export const HELP_DOC_CSS = ${JSON.stringify(css)};\n\n` +
  `export const HELP_DOC_HTML = ${JSON.stringify(html)};\n`;
writeFileSync(outputJs, module, "utf8");

// --- 同步配图 --------------------------------------------------------------
mkdirSync(imageTargetDir, { recursive: true });
let copied = 0;
for (const name of referenced) {
  const from = join(imageSourceDir, name);
  if (!existsSync(from)) fail(`正文引用了 ${name}，但 ${imageSourceDir} 里没有这个文件`);
  const to = join(imageTargetDir, name);
  const same =
    existsSync(to) && statSync(to).size === statSync(from).size;
  if (!same) {
    copyFileSync(from, to);
    copied += 1;
  }
}

const kb = (statSync(outputJs).size / 1024).toFixed(1);
console.log(
  `[help-doc] 已生成 ${outputJs.split(/[\\/]/).pop()}（${kb} KB，${referenced.size} 张配图，新复制 ${copied} 张）`
);
