"""一次性清理脚本：把历史遗留的「预览版 + 完整版各一份」收敛成单份。

软件早期版本同步时会把正文复制到完整版目录（两边各一份）。从 v0.3.44 起改成了移动，
一个作品只占一边。这个脚本用来把旧数据也理干净：

- 作品同时绑定了预览版和完整版路径、且两份文件都存在、大小一致 → 删掉预览版目录里那份，
  并把库里的 preview_path 清空（作品变成纯完整版）。
- 两份大小不一致（用户自己改过其中一份）→ 只报告，不动任何文件。

默认只做预演（不会动任何东西），确认无误后加 --apply 才真正执行。

安全约定（重要）：
- 执行前把每个库备份到 .workbuddy/backup/（只备份数据库，不含文件本体）。
- 待清理的文件**不会被物理删除**，而是挪进 `.workbuddy/trash/<时间戳>/<库名>/`。
  确认没问题后自己删掉回收目录即可。加 --purge 才直接抹除、跳过回收目录。

教训：曾被这个脚本直接 os.remove 掉的「预览版目录封面 jpg」，其实是完整版作品
cover_path 指向的文件（历史遗留的错位引用），删完 800 多篇作品的封面集体挂掉。
凡是要删还被数据库引用的文件，一律先挪不删。

用法：
    <python> scripts/dedupe-single-side.py                     # 预演，只报告
    <python> scripts/dedupe-single-side.py --apply              # 移入回收目录（安全）
    <python> scripts/dedupe-single-side.py --apply --purge      # 直接删除（危险）
"""

import datetime
import os
import shutil
import sqlite3
import sys

TARGETS = [
    (r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db", "运行目录"),
    (r"D:\500 工作\Program\收藏记录软件\发布\藏集\PixivNovelDownloader\data\library.db", "发布归档"),
]
BACKUP_DIR = r"D:\500 工作\Program\收藏记录软件\.workbuddy\backup"
TRASH_DIR = r"D:\500 工作\Program\收藏记录软件\.workbuddy\trash"
STAMP = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
APPLY = "--apply" in sys.argv
PURGE = "--purge" in sys.argv


def move_to_trash(target, trash_root, work_id):
    """把待清理的文件/目录挪进回收目录，返回释放的字节数（目录记 0）。

    直接用 os.remove / shutil.rmtree 会让文件绕过回收站、彻底消失；
    这里改成 shutil.move，删错了还能从 .workbuddy/trash 里捞回来。
    """
    if not os.path.exists(target):
        return 0
    size = os.path.getsize(target) if os.path.isfile(target) else 0
    if PURGE:
        if os.path.isdir(target):
            shutil.rmtree(target)
        else:
            os.remove(target)
        return size
    name = os.path.basename(target.rstrip("\\/")) or "unnamed"
    dest = os.path.join(trash_root, f"{work_id}__{name}")
    counter = 1
    while os.path.exists(dest):
        dest = os.path.join(trash_root, f"{work_id}__{counter}__{name}")
        counter += 1
    shutil.move(target, dest)
    return size


def siblings(text_path):
    stem, folder = os.path.splitext(text_path)
    return [
        os.path.join(os.path.dirname(text_path), f"{os.path.basename(stem)}_images"),
        f"{stem}.html",
        f"{stem}.jpg",
        f"{stem}.epub",
    ]


def main():
    os.makedirs(BACKUP_DIR, exist_ok=True)
    if APPLY and not PURGE:
        os.makedirs(TRASH_DIR, exist_ok=True)
    mode = "预演（不动任何文件）"
    if APPLY:
        mode = "正式执行（待清理文件直接删除）" if PURGE else "正式执行（待清理文件移入回收目录）"
    print("模式：" + mode)
    for path, label in TARGETS:
        print("=" * 72)
        print(f"[{label}] {path}")
        if not os.path.isfile(path):
            print("  跳过：数据库不存在")
            continue
        if APPLY:
            bak = os.path.join(BACKUP_DIR, f"{label}-library-{STAMP}.db")
            shutil.copy2(path, bak)
            print(f"  备份 -> {bak}")
        if APPLY:
            trash_root = os.path.join(TRASH_DIR, STAMP, label)
            if not PURGE:
                os.makedirs(trash_root, exist_ok=True)
                print(f"  回收目录 -> {trash_root}")
        conn = sqlite3.connect(path, timeout=15)
        rows = conn.execute(
            "SELECT id, preview_path, purchased_path FROM works "
            "WHERE preview_path <> '' AND purchased_path <> ''"
        ).fetchall()
        removed = warned = 0
        freed = 0
        for work_id, preview, purchased in rows:
            if not (os.path.isfile(preview) and os.path.isfile(purchased)):
                continue
            if os.path.normcase(os.path.abspath(preview)) == os.path.normcase(
                os.path.abspath(purchased)
            ):
                continue
            if os.path.getsize(preview) != os.path.getsize(purchased):
                warned += 1
                print(f"  ! 大小不一致，保留两份：{os.path.basename(preview)}")
                continue
            targets = [preview] + [
                asset for asset in siblings(preview) if os.path.exists(asset)
            ]
            if not APPLY:
                removed += 1
                freed += os.path.getsize(preview)
                continue
            try:
                for target in targets:
                    freed += move_to_trash(target, trash_root, work_id)
                conn.execute(
                    "UPDATE works SET preview_path='' WHERE id=?", (work_id,)
                )
                removed += 1
            except OSError as error:
                print(f"  ! 处理失败：{error}")
        if APPLY:
            conn.commit()
        conn.close()
        if APPLY and PURGE:
            action = "已删除"
        elif APPLY:
            action = "已移入回收目录"
        else:
            action = "可清理"
        print(
            f"  两边都绑定 {len(rows)} 篇；{action} {removed} 篇"
            f"（约 {freed / 1024 / 1024:.1f} MB）；大小不一致跳过 {warned} 篇"
        )
    if not APPLY:
        print("\n以上是预演结果。确认无误后加 --apply 再跑一次（默认移入回收目录）。")
    elif not PURGE:
        print(f"\n文件都挪到 {os.path.join(TRASH_DIR, STAMP)}，确认没问题后再删这个目录。")


if __name__ == "__main__":
    main()
