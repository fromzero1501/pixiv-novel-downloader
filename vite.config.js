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
    // 绑死 IPv4 回环，别用默认的 "localhost"：Node 17+ 解析 localhost 会优先给 ::1，
    // vite 于是只监听 [::1]，而 Tauri 的 WebView2 那边不保证也走 IPv6 ——
    // 两边对不上就是「窗口打开一片白」。devUrl 里也同步写成 127.0.0.1，杜绝歧义。
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      // 必须排掉 src-tauri：那里光 target 就是两万个文件、十几 G（编译一次涨一批），
      // 而 vite 的监听**不读 .gitignore**。不排的话 chokidar 要把这堆东西反复扫，
      // dev 服务器会卡到不响应请求 —— 表现就是打开软件白屏（2026-09-17 踩过）。
      // Tauri 官方模板同样有这一条。其余几个也都是不该被监听的本地大目录。
      ignored: ["**/src-tauri/**", "**/.workbuddy/**", "**/发布/**", "**/epub-eval/**"],
    },
  },
});
