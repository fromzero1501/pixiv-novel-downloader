// 便携版打包后的归档脚本：把裸 EXE 复制成发布命名，放进发布归档目录。
// 用 Node 写而不是 .bat，是为了正确处理含中文的路径（cmd 默认 GBK 会乱码）。
// 用户要求（2026-09-14）：**只产出到发布归档目录**，项目根目录不再留副本。
// 用户要求（2026-10-05）：**压缩包版只在发布时才有** —— 平时（`npm run portable` / 自用升级）
//   只出裸 exe；只有 `npm run release --publish` 会设 `PORTABLE_WITH_ZIP=1`，那时才多打一个 zip。
//   zip 解压出来是一层 `Pixiv小说下载管理器/` 文件夹，exe 在里面，数据首次运行落在同级 data/
//   （数据目录规则见 lib.rs 的 resolve_data_dir_in：exe 所在文件夹名 == 软件名时不再套同名层）。
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

const baseName = `PixivNovelDownloader-v${version}.exe`;
const destinations = [join(root, "发布", "藏集", "PixivNovelDownloader", baseName)];

for (const dest of destinations) {
  mkdirSync(dirname(dest), { recursive: true });
  copyFileSync(src, dest);
  const mb = (statSync(dest).size / 1024 / 1024).toFixed(1);
  console.log(`[portable] ${dest}  (${mb} MB)`);
}

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
  const zipPath = join(root, "发布", "藏集", "PixivNovelDownloader", `${ZIP_FOLDER}-v${version}.zip`);
  const count = writeZip(zipPath, [{ name: `${ZIP_FOLDER}/${baseName}`, source: src }]);
  console.log(`[portable] ${zipPath}  (${count} 个文件)`);
  console.log(`[portable] zip 解压即用：${ZIP_FOLDER}/ 里放 exe，数据落同级 data/。`);
} else {
  console.log("[portable] 本次只出裸 exe（压缩包版只在发布时产）");
}

console.log(`[portable] v${version} portable build ready - double-click to run, no install needed.`);
