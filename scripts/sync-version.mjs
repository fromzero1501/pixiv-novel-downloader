// 版本号唯一手改源 = package.json（`"version"`）。
//
// 其余引用点全部由这个脚本在 dev / build 之前自动同步（挂在 npm 的 predev / prebuild 上），
// 改版本时**只改 package.json 一处**就够了：
//   src-tauri/Cargo.toml       —— Rust 侧的 env!("CARGO_PKG_VERSION")（UA、更新检查）靠它
//   src-tauri/tauri.conf.json  —— 打包产物与更新检查看到的版本
//   src-tauri/Cargo.lock       —— cargo 自己也会更新，但先写上省得 git status 一直脏
//   package-lock.json          —— 同上，npm 自己会同步
//
// 前端（src/main.js 的 APP_VERSION）不走这里：vite.config.js 直接把 package.json
// 注入成编译期常量 __APP_VERSION__，改完 package.json 重新构建即可。
//
// 幂等：已经一致的文件不动、不重写（避免无谓的 mtime 变化惊动 cargo / tauri 的增量构建）。
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const version = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8")).version;
if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`[version] package.json 里的版本号看着不像版本号：${JSON.stringify(version)}`);
  process.exit(1);
}

// 正则都锚得很死：只替换该文件里那**一处**目标，绝不误伤依赖项同名字段。
// 顺序有讲究 —— 同一个文件的多条改动按顺序生效（package-lock.json 要改两处）。
const edits = [
  {
    file: "src-tauri/Cargo.toml",
    pattern: /^version = "[^"]*"$/m,
    replace: () => `version = "${version}"`,
  },
  {
    file: "src-tauri/tauri.conf.json",
    pattern: /^(\s*"version": ")[^"]*(")/m,
    replace: (_match, head, tail) => `${head}${version}${tail}`,
  },
  {
    file: "src-tauri/Cargo.lock",
    pattern: /(name = "collection-library"\r?\nversion = ")[^"]*(")/,
    replace: (_match, head, tail) => `${head}${version}${tail}`,
  },
  {
    file: "package-lock.json",
    pattern: /^(\s*"version": ")[^"]*(")/m,
    replace: (_match, head, tail) => `${head}${version}${tail}`,
  },
  {
    // packages[""] 里那处：必须连缩进一起锚（6 空格），
    // 否则会先命中文件顶部那个 2 空格缩进的同名 "name"/"version"，那一处就改不到了。
    file: "package-lock.json",
    pattern: /^(      "name": "local-collection-library",\r?\n      "version": ")[^"]*(")/m,
    replace: (_match, head, tail) => `${head}${version}${tail}`,
  },
];

let touched = 0;
for (const edit of edits) {
  const file = path.join(root, edit.file);
  const text = readFileSync(file, "utf8");
  if (!edit.pattern.test(text)) {
    console.error(`[version] ${edit.file} 里没找到该改的版本字段，跳过`);
    continue;
  }
  const next = text.replace(edit.pattern, edit.replace);
  if (next === text) continue;
  writeFileSync(file, next);
  touched += 1;
  console.log(`[version] ${edit.file} → ${version}`);
}

console.log(touched ? `[version] 已同步 ${touched} 处到 ${version}` : `[version] 各处已是 ${version}`);
