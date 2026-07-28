# Pixiv小说下载管理器

Windows 本地作品管理软件，用于按作者管理 Pixiv 小说预览版与完整版文件，记录购买状态、封面、标签、系列和收藏。

## 功能

- 作者库、所有作品与系列作品浏览
- 导入作品名称、关联预览版/完整版文件和封面
- 基于相似度的文件匹配、批量操作、标签与收藏管理
- 从 Pixiv 作者主页同步小说正文、封面、投稿日期、标签和系列信息
- 同步按原始投稿时间筛选，支持增量同步、进度、终止和抓取限速
- 便携式 SQLite 数据库，数据保存在程序目录旁的 `data` 文件夹

## 开发环境

- Windows 11
- Node.js 20 或更新版本
- Rust stable（MSVC 工具链）
- Microsoft Edge WebView2 Runtime（Windows 11 通常已安装）

## 安装与运行

```powershell
npm install
npm run tauri dev
```

## 构建免安装程序

```powershell
npm run tauri -- build --no-bundle
```

生成文件位于 `src-tauri/target/release/collection-library.exe`。发布前可将其复制并重命名为产品名称。

## 数据与隐私

- Pixiv Cookie、作者数据和本地作品路径仅保存在本机 SQLite 数据库，不应提交到 Git。
- `.gitignore` 已排除构建缓存、依赖、数据库、发布文件和本地调试产物。
- 发布版 `v0.3.11` 使用 Tauri + WebView2；正常 Windows 11 环境可直接运行。
