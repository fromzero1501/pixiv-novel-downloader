# 角色热度排序：
#   1) 复用已抓的 pixiv 标签频次表（character-freq.json）
#   2) 标签做「简繁归一 + 剥(游戏名)括号」，并到简体中文名上
#   3) 与 wiki 全名单匹配，排序取热门
# 需在装有 zhconv 的 venv 里运行
# 输出 character-rank.json
import json
import os
import re

from zhconv import convert

HERE = os.path.dirname(os.path.abspath(__file__))
RES = os.path.join(HERE, "..", ".workbuddy", "research")
RAW = os.path.join(RES, "characters-raw.json")
FREQ = os.path.join(RES, "character-freq.json")
OUT = os.path.join(RES, "character-rank.json")

raw = json.load(open(RAW, encoding="utf-8"))
freq = json.load(open(FREQ, encoding="utf-8"))

PAREN = re.compile(r"^(.{1,20})[（(]([^）)]{1,25})[)）]$")
# 通用/题材标签兜底黑名单（正常情况下匹配不上候选名，这里只防意外同名）
BLACK = {"中文", "同人", "漫画", "小说", "小説", "游戏", "遊戲", "女の子", "少女", "美少女",
         "巨乳", "爆乳", "纯爱", "百合", "调教", "露出", "中文/中国语"}


def norm_tag(tag):
    """标签 -> 简体中文归一名（可能返回 None）"""
    t = tag
    m = PAREN.match(tag)
    if m:
        t = m.group(1)
    t = convert(t, "zh-cn").strip()
    if not t or t in BLACK or len(t) > 16:
        return None
    return t


print("=" * 74)
print("按 pixiv 共现热度排序各游戏角色")
print("=" * 74)

rank = {}
for game, names in raw.items():
    counter = {}
    for tag, c in freq.get(game, {}).items():
        # 原始标签
        counter[tag] = counter.get(tag, 0) + c
        # 归一化后的简体名（含剥括号）
        n = norm_tag(tag)
        if n:
            counter[n] = counter.get(n, 0) + c
    scored = sorted(((n, counter.get(n, 0)) for n in names), key=lambda x: (-x[1], x[0]))
    hot = [(n, c) for n, c in scored if c > 0]
    warm = [(n, c) for n, c in scored if c == 0]
    rank[game] = {"all": scored, "hot": hot, "warmCount": len(warm)}
    print(f"\n### {game}  候选 {len(names)} → 有热度 {len(hot)}、无热度 {len(warm)}")
    print("   前 30：", [f"{n}×{c}" for n, c in hot[:30]])

json.dump(rank, open(OUT, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print("\n已写入:", OUT)

# 汇总：热度阈值建议
print("\n" + "=" * 74)
print("各游戏热度分布（用于决定收录条数）")
print("=" * 74)
for game, v in rank.items():
    h = [c for _, c in v["hot"]]
    for th in (30, 10, 5, 3, 2, 1):
        n = sum(1 for c in h if c >= th)
        print(f"  {game:<13} 热度≥{th:<3} {n:>4} 个", end="")
    print()
