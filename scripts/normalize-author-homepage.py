# 把作者主页统一收敛到「https://www.pixiv.net/users/<id>」，
# 与 Rust 端 normalize_author_homepage / db() 里的一次性迁移规则完全一致。
#
# 用法：
#   python scripts/normalize-author-homepage.py            # 处理默认两套库
#   python scripts/normalize-author-homepage.py 路径...    # 处理指定库
#
# 安全性：跑前把每个库备份到 .workbuddy/backup/<标签>-library-<时间戳>.db；
#        只改 homepage 文本，作者 ID 保留，同步/主页链接都不受影响。
import sqlite3
import os
import shutil
import sys
import datetime

BACKUP_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), ".workbuddy", "backup")

DIGITS = "0123456789"

DEFAULT_TARGETS = [
    (r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db", "运行目录"),
    (r"D:\500 工作\Program\收藏记录软件\发布\藏集\PixivNovelDownloader\data\library.db", "发布归档"),
]


def normalize(homepage: str) -> str:
    s = (homepage or "").strip()
    if not s:
        return ""
    if "/users/" in s:
        prefix, _, suffix = s.partition("/users/")
        digits = ""
        for ch in suffix:
            if ch in DIGITS:
                digits += ch
            else:
                break
        if digits:
            return f"{prefix.rstrip('/')}/users/{digits}"
    if "id=" in s:
        _, _, suffix = s.partition("id=")
        digits = ""
        for ch in suffix:
            if ch in DIGITS:
                digits += ch
            else:
                break
        if digits:
            return f"https://www.pixiv.net/users/{digits}"
    return s


def main():
    args = sys.argv[1:]
    targets = [(path, None) for path in args] if args else DEFAULT_TARGETS
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    os.makedirs(BACKUP_DIR, exist_ok=True)

    for path, label in targets:
        print("=" * 72)
        print(path)
        if not os.path.exists(path):
            print("  跳过：文件不存在")
            continue
        label = label or os.path.splitext(os.path.basename(os.path.dirname(path)))[0] or "库"
        backup = os.path.join(BACKUP_DIR, f"{label}-library-{stamp}.db")
        shutil.copy2(path, backup)
        print(f"  已备份 → {backup}")

        conn = sqlite3.connect(path, timeout=15)
        conn.execute("BEGIN IMMEDIATE")
        rows = conn.execute("SELECT id, name, homepage FROM authors WHERE homepage <> ''").fetchall()
        changed = []
        conflict = set()
        for author_id, name, homepage in rows:
            normalized = normalize(homepage)
            if normalized == homepage:
                continue
            if normalized:
                dup = conn.execute(
                    "SELECT name FROM authors WHERE homepage = ?1 AND id <> ?2", (normalized, author_id)
                ).fetchone()
                if dup:
                    conflict.add(f"{name} → {normalized}（与“{dup[0]}”重复）")
            conn.execute("UPDATE authors SET homepage = ?1 WHERE id = ?2", (normalized, author_id))
            changed.append((name, homepage, normalized))
        conn.commit()
        total = conn.execute("SELECT COUNT(*) FROM authors").fetchone()[0]
        blank = conn.execute("SELECT COUNT(*) FROM authors WHERE homepage = ''").fetchone()[0]
        conn.close()

        print(f"  作者共 {total} 位，其中已填主页 {len(rows)} 位、未填 {blank} 位")
        print(f"  已清洗 {len(changed)} 条：")
        for name, before, after in changed:
            print(f"    {name}：{before}  →  {after}")
        if not changed:
            print("    （没有需要清洗的主页，已经是 /users/<id> 格式）")
        for item in sorted(conflict):
            print(f"  ⚠ 清洗后与已有作者主页重复：{item}")


if __name__ == "__main__":
    main()
