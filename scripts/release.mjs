// 一条命令跑完发版：升版本号 → 打包便携版 → 提交 → 推送。
//
// 用法（在项目根目录）：
//   npm run release -- "提交说明"          升末位（patch），说明可省
//   npm run release -- "说明" --minor      升中间位
//   npm run release -- "说明" --major      升首位
//   npm run release -- --tag               额外打 tag + 建 GitHub Release
//   npm run release -- --dry-run           只预览会做什么：不改文件、不构建、不提交、不推送
//
// 几条约定（和项目原有流程对齐）：
//   - 版本号唯一手改源仍是 package.json；其余 5 处由 sync-version.mjs 在打包前自动同步，
//     所以这里只改 package.json 一处。
//   - 中途任何一步失败 → 立刻停，并把 package.json 的版本号还原，
//     不留「升了号却没打包 / 没提交」的半成品状态。
//   - tag / Release 默认**不做**，只有显式加 --tag 才做（与「发版要单独说」的约定一致）。
//   - 帮助文档不碰。
import { execFileSync, execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pkgPath = path.join(root, "package.json");

const argv = process.argv.slice(2);
const flags = new Set(argv.filter((a) => a.startsWith("--")));
const message = argv.find((a) => !a.startsWith("--")) || "";
const dryRun = flags.has("--dry-run");
const withTag = flags.has("--tag");
const kind = flags.has("--major") ? "major" : flags.has("--minor") ? "minor" : "patch";

const log = (line = "") => console.log(line);
const fail = (reason) => {
  console.error(`\n[release] 失败：${reason}`);
  process.exit(1);
};

// 会改动状态的命令：dry-run 时只打印不执行
function run(cmd, args) {
  log(`\n$ ${[cmd, ...args].join(" ")}`);
  if (dryRun) return "";
  return execFileSync(cmd, args, { cwd: root, stdio: "inherit", encoding: "utf8" }) || "";
}

// 需要 shell 的命令（npm 是 .cmd，Windows 下必须过 shell）
function shell(cmd) {
  log(`\n$ ${cmd}`);
  if (dryRun) return;
  execSync(cmd, { cwd: root, stdio: "inherit" });
}

// 只读命令：dry-run 时也照跑，好让预览看到真实的分支名 / git status
function read(cmd, args) {
  try {
    return (execFileSync(cmd, args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }) || "").trim();
  } catch {
    return "";
  }
}

// ── 1. 算新版本号 ────────────────────────────────────────────────
const original = readFileSync(pkgPath, "utf8");
const current = JSON.parse(original).version;
const matched = /^(\d+)\.(\d+)\.(\d+)$/.exec(current);
if (!matched) fail(`package.json 的版本号不是 x.y.z 形式：${JSON.stringify(current)}`);

const nums = [Number(matched[1]), Number(matched[2]), Number(matched[3])];
if (kind === "major") {
  nums[0] += 1;
  nums[1] = 0;
  nums[2] = 0;
} else if (kind === "minor") {
  nums[1] += 1;
  nums[2] = 0;
} else {
  nums[2] += 1;
}
const next = nums.join(".");
const commitMessage = message || `release: v${next}`;
const branch = read("git", ["rev-parse", "--abbrev-ref", "HEAD"]) || "master";

log(`[release] ${current} → ${next}   (${kind})`);
log(`[release] 提交说明：${commitMessage}`);
log(`[release] 目标分支：${branch}`);
if (withTag) log(`[release] 会额外打 tag v${next} 并建 Release`);
if (dryRun) {
  log("[release] --dry-run：只预览，不改文件、不构建、不提交、不推送");
  log("\n[release] 当前 git status（这些会被 git add -A 收走）：");
  log(read("git", ["status", "--short"]) || "  (干净)");
}

// ── 2. 写回 package.json ────────────────────────────────────────
let bumped = false;
const restoreVersion = () => {
  if (bumped && !dryRun) writeFileSync(pkgPath, original);
};

try {
  if (!dryRun) {
    const updated = original.replace(/("version"\s*:\s*")[^"]*(")/, `$1${next}$2`);
    if (updated === original) fail("没能在 package.json 里定位到 version 字段");
    writeFileSync(pkgPath, updated);
    bumped = true;
  }

  // ── 3. 打包（内部先把新版本同步到其余 5 处，再编译、再把 exe 归档到发布目录）──
  shell("npm run portable");

  // ── 4. 暂存 → 提交 → 推送 ───────────────────────────────────
  run("git", ["add", "-A"]);
  run("git", ["commit", "-m", commitMessage]);
  run("git", ["push", "origin", branch]);

  // ── 5. 可选：tag + Release（默认不做）────────────────────────
  if (withTag) {
    const exe = path.join(root, "发布", "藏集", "PixivNovelDownloader", `PixivNovelDownloader-v${next}.exe`);
    run("git", ["tag", `v${next}`]);
    run("git", ["push", "origin", `v${next}`]);
    run("gh", ["release", "create", `v${next}`, exe, "--title", `v${next}`, "--notes", `v${next}`]);
  }
} catch (error) {
  restoreVersion();
  fail(`${error.message || error}${bumped ? "（package.json 的版本号已还原）" : ""}`);
}

log(`\n[release] 完成：v${next}${dryRun ? "（dry-run，什么都没改）" : ""}`);
