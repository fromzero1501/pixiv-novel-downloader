"""一次性补标脚本：把「标题前 15 字含『插画』或『图文』」的作品标为带图版。

规则与 Rust 端 `title_indicates_images` 完全一致，只做 0 -> 1，绝不会清掉已有标记。
运行前会先把每个库备份到 .workbuddy/backup/。
重新导入本地作品后如果还要补标，直接再跑一次即可（已标过的会被 WHERE has_images=0 跳过）。

用法：<python> scripts/backfill-has-images.py
"""
import sqlite3, os, shutil, datetime, sys

TARGETS = [
    (r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db", "运行目录"),
    (r"D:\500 工作\Program\收藏记录软件\发布\藏集\PixivNovelDownloader\data\library.db", "发布归档"),
]
BACKUP_DIR = r"D:\500 工作\Program\收藏记录软件\.workbuddy\backup"
STAMP = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")

# 与 Rust 端 title_indicates_images 同规则：标题前 15 个字符含「插画」或「图文」
WHERE = (
    "has_images = 0 AND ("
    "instr(substr(title, 1, 15), '插画') > 0 OR "
    "instr(substr(title, 1, 15), '图文') > 0)"
)

os.makedirs(BACKUP_DIR, exist_ok=True)

for path, label in TARGETS:
    print("=" * 72)
    print(f"[{label}] {path}")
    if not os.path.exists(path):
        print("  跳过：文件不存在")
        continue

    bak = os.path.join(BACKUP_DIR, f"{label}-library-{STAMP}.db")
    shutil.copy2(path, bak)
    print(f"  备份 -> {bak}")

    con = sqlite3.connect(path, timeout=15)
    try:
        cur = con.cursor()
        cur.execute(f"SELECT COUNT(*) FROM works WHERE {WHERE}")
        pending = cur.fetchone()[0]
        cur.execute("BEGIN IMMEDIATE")
        cur.execute(f"UPDATE works SET has_images = 1 WHERE {WHERE}")
        changed = cur.rowcount
        con.commit()
        cur.execute("SELECT COUNT(*) FROM works WHERE has_images = 1")
        total_flagged = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM works")
        total = cur.fetchone()[0]
        print(f"  命中待补标 {pending} 条，实际更新 {changed} 行")
        print(f"  现在带图版共 {total_flagged} / {total} 个作品")
    except Exception as e:
        con.rollback()
        print("  失败：", e)
        sys.exit(1)
    finally:
        con.close()

print("=" * 72)
print("完成")
