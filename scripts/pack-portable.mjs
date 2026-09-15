// 便携版打包后的归档脚本：把裸 EXE 复制成发布命名，放进发布归档目录。
// 用 Node 写而不是 .bat，是为了正确处理含中文的路径（cmd 默认 GBK 会乱码）。
// 用户要求（2026-09-14）：**只产出到发布归档目录**，项目根目录不再留副本。
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));

const src = join(root, "src-tauri", "target", "release", "collection-library.exe");
if (!existsSync(src)) {
  console.error(`[portable] build output not found: ${src}`);
  process.exit(1);
}

const baseName = `PixivNovelDownloader-v${version}.exe`;
const destinations = [join(root, "发布", "藏集", "PixivNovelDownloader", baseName)];

for (const dest of destinations) {
  mkdirSync(dirname(dest), { recursive: true });
  copyFileSync(src, dest);
  const mb = (statSync(dest).size / 1024 / 1024).toFixed(1);
  console.log(`[portable] ${dest}  (${mb} MB)`);
}

console.log(`[portable] v${version} portable build ready - double-click to run, no install needed.`);
