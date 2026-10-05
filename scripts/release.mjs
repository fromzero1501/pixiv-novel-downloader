// 一条命令跑完发版：升版本号 → 打包便携版 → 提交 → 推送。
//
// 用法（在项目根目录）：
//   npm run release -- "提交说明"          升末位（patch），说明可省
//   npm run release -- "说明" --minor      升中间位
//   npm run release -- "说明" --major      升首位
//   npm run release -- "说明" --publish    发布：多打 tag、建 Release、传 exe
//   npm run release -- --dry-run           只预览会做什么：不改文件、不构建、不提交、不推送
//
// 几条约定（和项目原有流程对齐）：
//   - 版本号唯一手改源仍是 package.json；其余 5 处由 sync-version.mjs 在打包前自动同步，
//     所以这里只改 package.json 一处。
//   - 中途任何一步失败 → 立刻停，并把 package.json 的版本号还原，
//     不留「升了号却没打包 / 没提交」的半成品状态。
//   - 默认只做「升号 → 打包 → 提交 → 推送」四步，不碰 tag / Release。
//     明确要「发布」时再加 --publish：多打 tag、建 Release、传 exe。
//     （自动更新只认 releases/latest，漏建 Release 就等于更新链路断了，所以真正对外发布时必须带 --publish；忘了会在末尾打出补建命令。）
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
// 默认只升号→打包→提交→推送；明确说「发布」时加 --publish 才打 tag + 建 Release。
// （--release 保留为同义别名；旧的 --no-release 也接受，它本来就等于现在的默认行为。）
const withRelease = flags.has("--publish") || flags.has("--release");
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
log(`[release] 会打包便携 exe：发布/藏集/PixivNovelDownloader/PixivNovelDownloader-v${next}.exe`);
if (withRelease) {
  log(`[release] --publish：会打 tag v${next}、建 Release 并上传 exe`);
} else {
  log(`[release] 不动 tag / Release：只升号 → 打包 → 提交 → 推送`);
}
if (dryRun) {
  log("[release] --dry-run：只预览，不改文件、不构建、不提交、不推送");
  log("\n[release] 当前 git status（这些会被 git add -A 收走）：");
  log(read("git", ["status", "--short"]) || "  (干净)");
}

// ── 2. 写回 package.json ────────────────────────────────────────
let bumped = false;
let committed = false;
// 只在**还没提交**时还原版本号：提交之后若在推 tag / 建 Release 上失败，
// 还原会让工作区里留一个「版本号倒退」的脏文件，比不还原更糟。
const restoreVersion = () => {
  if (bumped && !committed && !dryRun) writeFileSync(pkgPath, original);
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
  committed = true;
  run("git", ["push", "origin", branch]);

  // ── 5. tag + Release（只有 --publish 才做）─────────────
  //  用提交说明当 Release 正文：应用内「更新说明」读的就是这一段。
  if (withRelease) {
    const exe = path.join(root, "发布", "藏集", "PixivNovelDownloader", `PixivNovelDownloader-v${next}.exe`);
    run("git", ["tag", `v${next}`]);
    run("git", ["push", "origin", `v${next}`]);
    run("gh", ["release", "create", `v${next}`, exe, "--title", `v${next}`, "--notes", commitMessage]);
  }
} catch (error) {
  restoreVersion();
  fail(`${error.message || error}${bumped && !committed ? "（package.json 的版本号已还原）" : ""}`);
}

log(`\n[release] 完成：v${next}${dryRun ? "（dry-run，什么都没改）" : ""}`);
if (!dryRun && !withRelease) {
  const exeRel = `发布/藏集/PixivNovelDownloader/PixivNovelDownloader-v${next}.exe`;
  log(`[release] 本次没建 Release（自动更新看不到 v${next}）。要对外发布时执行：`);
  log(`  git tag v${next} && git push origin v${next}`);
  log(`  gh release create v${next} "${exeRel}" --title v${next} --notes "${commitMessage}"`);
}
