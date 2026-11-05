// 便携版打包后的归档脚本：把裸 EXE 复制成发布命名，放进发布归档目录。
// 用 Node 写而不是 .bat，是为了正确处理含中文的路径（cmd 默认 GBK 会乱码）。
// 用户要求（2026-09-14）：**只产出到发布归档目录**，项目根目录不再留副本。
// 用户要求（2026-10-05）：**压缩包版只在发布时才有** —— 平时（`npm run portable` / 自用升级）
//   只出裸 exe；只有 `npm run release --publish` 会设 `PORTABLE_WITH_ZIP=1`，那时才多打一个 zip。
//   zip 解压出来是一层 `Pixiv小说下载管理器/` 文件夹，exe 在里面，数据首次运行落在同级 data/
//   （数据目录规则见 lib.rs 的 resolve_data_dir_in：exe 所在文件夹名 == 软件名时不再套同名层）。
// 用户要求（2026-11-05）：**发布出来的 exe 不带版本号** —— 固定叫 `PixivNovelDownloader.exe`，
//   桌面快捷方式指向它一次就长期有效，升级只覆盖这个文件、不用重钉。版本号只留在 tag / Release 标题 / zip 名里。
//   连带一条必须先做的事：发布目录里可能正跑着旧版（映像锁着 exe，覆盖会失败），
//   所以每次出包前先按**完整路径**关掉这个目录下正在运行的进程，再等文件句柄释放。
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));

const src = join(root, "src-tauri", "target", "release", "collection-library.exe");
if (!existsSync(src)) {
  console.error(`[portable] build output not found: ${src}`);
  process.exit(1);
}

/** 发布目录里那个固定的程序名；发布产物、zip 内条目、桌面快捷方式全部指向它。 */
const PROGRAM_NAME = "PixivNovelDownloader.exe";
const releaseDir = join(root, "发布", "藏集", "PixivNovelDownloader");
const destinations = [join(releaseDir, PROGRAM_NAME)];

/**
 * 关掉发布目录里正在运行的旧版。
 * 只按**完整路径**认（`$_.Path` 就在这个目录下），不按进程名 —— 别的地方装过同名程序也杀不到它。
 * 正在跑的那个 exe 被映像锁着，Windows 不允许覆盖，不先关掉这一步就写不进去。
 */
function closeRunningCopies(directory) {
  if (process.platform !== "win32" || !existsSync(directory)) return 0;
  const quoted = directory.replace(/'/g, "''");
  const script = [
    `$dir = '${quoted}'`,
    "$targets = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($dir, [StringComparison]::OrdinalIgnoreCase) -and $_.Path.ToLowerInvariant().EndsWith('.exe') })",
    "foreach ($t in $targets) { Stop-Process -Id $t.Id -Force -ErrorAction SilentlyContinue }",
    "'KILLED=' + $targets.Count",
  ].join("; ");
  let out = "";
  try {
    out = execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    // 关不掉不是致命错误：下面复制时会重试，真不行再报错，让用户自己关。
    console.warn(`[portable] 关旧版进程失败：${error.message}`);
    return 0;
  }
  const killed = Number((/KILLED=(\d+)/.exec(out) || [])[1] || 0);
  if (killed > 0) console.log(`[portable] 已关掉 ${killed} 个正在运行的旧版进程`);
  return killed;
}

/** 等一下再复制：进程被 Stop-Process 干掉之后，映像句柄不是同一刻放开的。 */
function sleepSync(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function copyWithRetry(source, target, attempts = 20, waitMs = 300) {
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      copyFileSync(source, target);
      return attempt;
    } catch (error) {
      if (attempt === attempts) throw error;
      if (attempt === 1) console.log(`[portable] 目标还占着，重试覆盖（最多 ${attempts} 次 × ${waitMs} ms）`);
      sleepSync(waitMs);
    }
  }
  return 0;
}

closeRunningCopies(releaseDir);

for (const dest of destinations) {
  mkdirSync(dirname(dest), { recursive: true });
  copyWithRetry(src, dest);
  const mb = (statSync(dest).size / 1024 / 1024).toFixed(1);
  console.log(`[portable] ${dest}  (${mb} MB)`);
}
const ZIP_PROGRAM_NAME = PROGRAM_NAME;

// ── zip 分发包（只在发布时产）─────────────────────────────────────────────────
const ZIP_FOLDER = "Pixiv小说下载管理器";

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[i] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i += 1) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function dosDateTime(date) {
  return {
    time: ((date.getHours() << 11) | (date.getMinutes() << 5) | (date.getSeconds() >> 1)) & 0xffff,
    date: (((date.getFullYear() - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate()) & 0xffff,
  };
}

/**
 * 极简 zip 写盘：只支持 stored（不压缩）。
 * exe 内部本来就是压缩过的，再套一层 deflate 几乎不减小、还多一份依赖，所以**不引第三方库**，
 * 直接照 ZIP 规范手写 local header + central directory。文件名按 UTF-8 写并置 bit 11，
 * 这样中文文件夹名在任何解压器里都不会乱码。
 */
function writeZip(target, entries) {
  const { time, date } = dosDateTime(new Date());
  const chunks = [];
  const centrals = [];
  let offset = 0;
  for (const entry of entries) {
    const data = readFileSync(entry.source);
    const nameBytes = Buffer.from(entry.name, "utf8");
    const crc = crc32(data);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6);
    local.writeUInt16LE(0, 8);
    local.writeUInt16LE(time, 10);
    local.writeUInt16LE(date, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(time, 12);
    central.writeUInt16LE(date, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt16LE(0, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE(0x20, 38); // 外部属性：普通文件
    central.writeUInt32LE(offset, 42);

    chunks.push(local, nameBytes, data);
    centrals.push(central, nameBytes);
    offset += local.length + nameBytes.length + data.length;
  }

  const centralBuf = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralBuf.length, 12);
  end.writeUInt32LE(offset, 16);
  end.writeUInt16LE(0, 20);

  writeFileSync(target, Buffer.concat([...chunks, centralBuf, end]));
  return entries.length;
}

if (process.env.PORTABLE_WITH_ZIP === "1") {
  // ⚠️ zip 的**文件名必须是 ASCII**：`gh release create` 上传时会把文件名里的非 ASCII
  // 字符一律替换成 `.`（实测 `Pixiv小说下载管理器-v1.2.38.zip` 在 Release 页面上变成
  // `Pixiv.-v1.2.38.zip`）。压缩包**内部**那一层目录名仍然是中文（ZIP 的 UTF-8 名不受影响），
  // 用户解压出来照样是 `Pixiv小说下载管理器/`。
  // zip 名**保留版本号**：它是给新用户手动下载的，下载页上看得出是哪一版；里面的 exe 仍是固定名。
  const zipPath = join(releaseDir, `PixivNovelDownloader-v${version}.zip`);
  const count = writeZip(zipPath, [{ name: `${ZIP_FOLDER}/${ZIP_PROGRAM_NAME}`, source: src }]);
  console.log(`[portable] ${zipPath}  (${count} 个文件)`);
  console.log(`[portable] zip 解压即用：${ZIP_FOLDER}/${ZIP_PROGRAM_NAME}，数据落同级 data/；升级只覆盖这个文件。`);
} else {
  console.log("[portable] 本次只出裸 exe（压缩包版只在发布时产）");
}

console.log(`[portable] v${version} portable build ready - double-click to run, no install needed.`);
