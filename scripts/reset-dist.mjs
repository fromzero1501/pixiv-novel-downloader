// 构建前把旧的 dist 目录「移走」而不是删除：
// vite 默认会清空输出目录，而本机环境对批量删除有配额保护（会话内删除过多时会拒绝），
// 移动目录不受影响。旧目录挪到 .workbuddy/dist-old 下（必须同盘，跨盘 rename 会失败），
// 确认新版本没问题后可以整个删掉。
import { existsSync, mkdirSync, readdirSync, renameSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dist = path.join(root, "dist");
if (!existsSync(dist)) {
  process.exit(0);
}
const graveyard = path.join(root, ".workbuddy", "dist-old");
mkdirSync(graveyard, { recursive: true });
const target = path.join(graveyard, `dist-${Date.now()}`);
renameSync(dist, target);
const stale = readdirSync(graveyard).length;
console.log(`旧 dist 已移走：${target}${stale > 3 ? `（回收目录里已有 ${stale} 份，可手动清空 .workbuddy/dist-old）` : ""}`);
