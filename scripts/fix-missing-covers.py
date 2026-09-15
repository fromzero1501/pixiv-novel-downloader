"""一次性修复：把失效的封面重新落到作品正文旁边，并更正库里的 cover_path。

背景：早期同步模型下，作品的封面 jpg 只存在于「预览版」目录，库里 cover_path 也指向那里。
作品升级为完整版后正文搬到了完整版目录，封面却在后续的清理/整理中被删掉，
于是 cover_path 变成死路径 —— 界面上这些作品的封面就全挂了。

本脚本对每篇 cover_path 失效的作品：
1. 先看正文旁边是否已有同名封面（`{正文名}.jpg/.png/.jpeg/.webp`）→ 有就直接改指过去；
2. 没有就按 pixiv_novel_id 调 Pixiv 接口取封面直链、下载到正文旁边（与正文同名），再改 cover_path。

默认只预演；加 --apply 才真正下载并写库。执行前自动备份数据库到 .workbuddy/backup/。
用法：
    <python> scripts/fix-missing-covers.py             # 预演
    <python> scripts/fix-missing-covers.py --limit 8 --apply   # 小批试跑
    <python> scripts/fix-missing-covers.py --apply     # 全量修复
"""

import argparse
import datetime
import json
import os
import shutil
import sqlite3
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor

# 重定向到管道时 stdout 默认块缓冲，长跑过程中看不到任何进度；强制按行刷新。
sys.stdout.reconfigure(line_buffering=True)

TARGETS = [
    (r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db", "运行目录"),
    (r"D:\500 工作\Program\收藏记录软件\发布\藏集\PixivNovelDownloader\data\library.db", "发布归档"),
]
BACKUP_DIR = r"D:\500 工作\Program\收藏记录软件\.workbuddy\backup"
IMAGE_EXTS = (".jpg", ".jpeg", ".png", ".webp")
UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/124.0 Safari/537.36"
)


def system_proxy():
    """当前要用的代理地址。

    Python 的 urllib 只认 http_proxy / https_proxy 环境变量，**不读** Windows
    「Internet 选项」里的代理设置。所以开着代理软件时脚本会直连 pixiv，然后被连接
    重置（表现为 SSL UNEXPECTED_EOF 或 WinError 10060，每次白等 30 秒）。
    这里把系统代理补上，跟软件（reqwest 自动读系统代理）的行为对齐。
    """
    for key in ("https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"):
        value = os.environ.get(key)
        if value:
            return value.strip()
    if sys.platform != "win32":
        return ""
    try:
        import winreg

        path = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings"
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path) as key:
            enabled = winreg.QueryValueEx(key, "ProxyEnable")[0]
            server = winreg.QueryValueEx(key, "ProxyServer")[0]
    except OSError:
        return ""
    if not enabled or not server:
        return ""
    # 可能是 "http=127.0.0.1:7890;https=127.0.0.1:7890" 这种按协议分开的写法
    if "=" in server:
        parts = dict(item.split("=", 1) for item in server.split(";") if "=" in item)
        server = parts.get("https") or parts.get("http") or ""
    return f"http://{server}" if server else ""


def build_opener():
    proxy = system_proxy()
    handlers = []
    if proxy:
        handlers.append(urllib.request.ProxyHandler({"http": proxy, "https": proxy}))
        print(f"  代理：{proxy}")
    else:
        print("  ! 没检测到系统代理；pixiv 直连在国内通常会被重置，可能大面积失败")
    return urllib.request.build_opener(*handlers)


OPENER = None


def http_get(url, cookie, timeout=30, retries=5):
    """Pixiv 对 ajax 接口限流很敏感，429 时按 4→8→16… 秒退避重试。"""
    global OPENER
    if OPENER is None:
        OPENER = build_opener()
    delay = 4.0
    for attempt in range(retries):
        request = urllib.request.Request(url)
        if cookie:
            request.add_header("Cookie", cookie)
        request.add_header("Referer", "https://www.pixiv.net/")
        request.add_header("User-Agent", UA)
        try:
            with OPENER.open(request, timeout=timeout) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            if error.code == 429 and attempt < retries - 1:
                print(f"    · 被限流（429），等 {delay:.0f}s 后重试")
                time.sleep(delay)
                delay *= 2
                continue
            raise


def sibling_cover(text_path):
    """正文旁边已有的同名封面（按扩展名依次找）。"""
    if not text_path:
        return None
    stem = os.path.splitext(text_path)[0]
    for ext in IMAGE_EXTS:
        candidate = stem + ext
        if os.path.isfile(candidate) and os.path.getsize(candidate) > 0:
            return candidate
    return None


def target_cover_path(text_path):
    return os.path.splitext(text_path)[0] + ".jpg"


def fetch_cover_url(cookie, novel_id):
    payload = json.loads(
        http_get(f"https://www.pixiv.net/ajax/novel/{novel_id}?time=0", cookie)
    )
    if payload.get("error"):
        raise RuntimeError("接口返回错误")
    return payload["body"].get("coverUrl", "")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--apply", action="store_true", help="真正下载并写库")
    parser.add_argument("--limit", type=int, default=0, help="本库最多处理多少篇")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument(
        "--delay", type=float, default=1.0, help="每篇之间的等待秒数（防风控）"
    )
    args = parser.parse_args()

    # 在这里就把 opener 建好：省得多线程里各建一次、日志重复打印
    global OPENER
    OPENER = build_opener()

    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    os.makedirs(BACKUP_DIR, exist_ok=True)
    print("模式：" + ("正式修复（会联网下载封面并写库）" if args.apply else "预演（不改动任何东西）"))

    total_fixed = total_local = total_failed = 0
    for path, label in TARGETS:
        print("=" * 74)
        print(f"[{label}] {path}")
        if not os.path.isfile(path):
            print("  跳过：数据库不存在")
            continue

        conn = sqlite3.connect(path, timeout=30)
        cookie = (
            conn.execute(
                "SELECT value FROM app_settings WHERE key='pixiv_cookie'"
            ).fetchone()
            or [""]
        )[0].strip()
        rows = conn.execute(
            "SELECT id, title, preview_path, purchased_path, cover_path, pixiv_novel_id "
            "FROM works WHERE pixiv_novel_id <> ''"
        ).fetchall()

        pending = []
        local_hits = []
        for work_id, title, preview, purchased, cover, novel_id in rows:
            if cover.strip() and os.path.isfile(cover):
                continue
            # 正文优先取完整版，其次预览版
            text = purchased if purchased and os.path.isfile(purchased) else preview
            if not text or not os.path.isfile(text):
                continue
            local = sibling_cover(text)
            if local:
                local_hits.append((work_id, local))
            else:
                pending.append((work_id, title, text, novel_id))
        if args.limit:
            pending = pending[: args.limit]

        print(f"  作品 {len(rows)} 篇；封面失效需处理 {len(pending) + len(local_hits)} 篇")
        print(f"    其中正文旁边本来就有封面、只改库 {len(local_hits)} 篇")
        print(f"    需要联网下载 {len(pending)} 篇")
        if not pending and not local_hits:
            conn.close()
            continue
        if not cookie:
            print("  ! 库里没有 Pixiv Cookie，无法下载封面")
        if not args.apply:
            print("  预演：例如")
            for work_id, title, text, novel_id in pending[:5]:
                print(f"    #{work_id} {title[:24]} -> {target_cover_path(text)}")
            conn.close()
            continue

        backup = os.path.join(BACKUP_DIR, f"{label}-library-{stamp}.db")
        shutil.copy2(path, backup)
        print(f"  备份 -> {backup}")

        updates = [(work_id, cover_path) for work_id, cover_path in local_hits]
        fixed = len(local_hits)
        failed = 0
        started = time.time()

        def handle(item):
            work_id, title, text, novel_id = item
            target = target_cover_path(text)
            try:
                if not os.path.isfile(target):
                    url = fetch_cover_url(cookie, novel_id)
                    if not url:
                        raise RuntimeError("没有封面直链")
                    data = http_get(url, cookie)
                    if not data:
                        raise RuntimeError("封面是空文件")
                    tmp = target + ".part"
                    with open(tmp, "wb") as handle_file:
                        handle_file.write(data)
                    os.replace(tmp, target)
                return work_id, target, None
            except Exception as error:  # noqa: BLE001 - 逐篇容错，继续跑
                return work_id, title, str(error)
            finally:
                time.sleep(args.delay)

        done = 0
        with ThreadPoolExecutor(max_workers=args.workers) as pool:
            for work_id, result, error in pool.map(handle, pending):
                done += 1
                if error:
                    failed += 1
                    if failed <= 5:
                        print(f"    ! #{work_id} 失败：{error}")
                else:
                    fixed += 1
                    updates.append((work_id, result))
                if done % 25 == 0 or done == len(pending):
                    # 逐批落库：中途被中断也不会丢掉已经下好的封面
                    conn.executemany(
                        "UPDATE works SET cover_path=?2 WHERE id=?1",
                        [(i, p) for i, p in updates],
                    )
                    conn.commit()
                    print(
                        f"    进度 {done}/{len(pending)}，已修 {fixed}，失败 {failed}"
                        f"（{time.time() - started:.0f}s）"
                    )

        conn.executemany(
            "UPDATE works SET cover_path=?2 WHERE id=?1", [(i, p) for i, p in updates]
        )
        conn.commit()
        conn.close()
        print(f"  完成：修复 {fixed} 篇（含本地直改 {len(local_hits)}），失败 {failed} 篇")
        total_fixed += fixed
        total_local += len(local_hits)
        total_failed += failed

    print("=" * 74)
    print(f"合计：修复 {total_fixed} 篇（本地直改 {total_local}），失败 {total_failed} 篇")
    if not args.apply:
        print("\n以上是预演结果。确认后加 --apply 再跑一次。")


if __name__ == "__main__":
    sys.exit(main())
