import { readFileSync } from "node:fs";
import { defineConfig } from "vite";

// 版本号唯一手改源就是 package.json —— 这里读出来注入成编译期常量 __APP_VERSION__，
// 前端 src/main.js 的 APP_VERSION 直接用它，不用再往源码里写死一个。
// （Cargo.toml / tauri.conf.json / lock 文件由 scripts/sync-version.mjs 同步，挂在 predev/prebuild）
const { version } = JSON.parse(readFileSync(new URL("./package.json", import.meta.url), "utf8"));

export default defineConfig({
  clearScreen: false,
  define: {
    __APP_VERSION__: JSON.stringify(version),
  },
  server: {
    port: 1420,
    strictPort: true,
  },
});
