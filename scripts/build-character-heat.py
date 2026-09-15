# 统计各游戏角色在 pixiv 的共现热度：
#   1) 搜插画区 + 小说区，累计所有标签频次
#   2) 把日文/罗马音标签译回中文（查 pixiv 标签词典），并到中文名上
#   3) 与 wiki 全名单匹配，算出每个角色的热度分，排序
# 输出 character-heat.json（含原始频次表，可复用）
import json
import os
import re
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
RES = os.path.join(HERE, "..", ".workbuddy", "research")
RAW = os.path.join(RES, "characters-raw.json")
FREQ = os.path.join(RES, "character-freq.json")
OUT = os.path.join(RES, "character-heat.json")

DB = r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db"
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({"http": "http://127.0.0.1:7897",
                                                                 "https": "http://127.0.0.1:7897"}))
CKE = ""
if os.path.isfile(DB):
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    row = conn.execute("SELECT value FROM app_settings WHERE key='pixiv_cookie'").fetchone()
    conn.close()
    CKE = row[0] if row else ""

# 游戏 -> 搜索用标签（中文 + 日文/别名）
GAMES = {
    "碧蓝航线": ["碧蓝航线", "アズールレーン"],
    "明日方舟": ["明日方舟", "アークナイツ"],
    "原神": ["原神"],
    "崩坏：星穹铁道": ["崩坏星穹铁道", "崩壊:スターレイル"],
    "绝区零": ["绝区零", "ゼンレスゾーンゼロ"],
    "崩坏3": ["崩坏3rd", "崩壊3rd"],
    "鸣潮": ["鸣潮", "鳴潮"],
    "蔚蓝档案": ["蔚蓝档案", "ブルーアーカイブ"],
}


def pixiv(url, retries=3):
    for attempt in range(retries):
        req = urllib.request.Request(url)
        req.add_header("User-Agent", UA)
        req.add_header("Referer", "https://www.pixiv.net/")
        req.add_header("Accept-Language", "zh-CN,zh;q=0.9,ja;q=0.8")
        if CKE:
            req.add_header("Cookie", CKE)
        try:
            with OPENER.open(req, timeout=25) as resp:
                time.sleep(0.35)
                return json.loads(resp.read().decode("utf-8", "replace"))
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503):
                time.sleep(2 + attempt * 3)
                continue
            return None
        except Exception:
            time.sleep(1.2)
    return None


def collect_tags(game, words, illust_pages=3, novel_pages=2):
    counter = Counter()
    total = 0
    for w in words:
        q = urllib.parse.quote(w)
        for kind, pages in (("artworks", illust_pages), ("novels", novel_pages)):
            for p in range(1, pages + 1):
                url = (f"https://www.pixiv.net/ajax/search/{kind}/{q}?word={q}"
                       f"&order=popular_d&mode=all&p={p}&s_mode=s_tag_full&type=all&lang=zh")
                d = pixiv(url)
                if not d:
                    continue
                body = d.get("body", {})
                items = body.get("illustManga", {}).get("data", []) if kind == "artworks" \
                    else body.get("novel", {}).get("data", [])
                for it in items:
                    total += 1
                    for t in it.get("tags", []):
                        counter[t] += 1
    return counter, total


if os.path.isfile(FREQ):
    print("复用已抓取的频次表:", FREQ)
    freq = json.load(open(FREQ, encoding="utf-8"))
    freq = {g: Counter(v) for g, v in freq.items()}
else:
    print("=" * 72)
    print("第一步：抓各游戏在 pixiv 的标签频次")
    print("=" * 72)
    freq = {}
    for game, words in GAMES.items():
        t0 = time.time()
        counter, total = collect_tags(game, words)
        freq[game] = counter
        print(f"  {game:<13} 作品 {total:>4} 篇  去重标签 {len(counter):>4} 个  "
              f"{int(time.time()-t0)}s  高频: {[t for t,_ in counter.most_common(6)]}")
    json.dump({g: dict(c) for g, c in freq.items()},
              open(FREQ, "w", encoding="utf-8"), ensure_ascii=False, indent=1)

# 第二步：把日文/罗马音标签译回中文（只查「非纯中文」且出现过的标签）
CJK = re.compile(r"[\u4e00-\u9fff]")
cache_path = os.path.join(RES, "tag-translation-cache.json")
cache = json.load(open(cache_path, encoding="utf-8")) if os.path.isfile(cache_path) else {}


def translate(tag):
    """标签 -> 中文写法（拿不到就返回 None）"""
    if tag in cache:
        return cache[tag]
    q = urllib.parse.quote(tag)
    d = pixiv("https://www.pixiv.net/ajax/search/tags/" + q)
    zh = None
    if d:
        body = d.get("body", {}) or {}
        tr = (body.get("tagTranslation") or {}).get(tag) or {}
        zh = (tr.get("zh") or "").strip() or None
    cache[tag] = zh
    return zh


need = set()
for game, counter in freq.items():
    for tag, c in counter.items():
        if c >= 2 and not CJK.search(tag):
            need.add(tag)
need = sorted(need)
print(f"\n需要译回中文的日文/罗马音标签：{len(need)} 个"
      f"（缓存已有 {len(cache)}）")
todo = [t for t in need if t not in cache]
print(f"实际要请求：{len(todo)} 个")
for i, tag in enumerate(todo, 1):
    translate(tag)
    if i % 25 == 0:
        print(f"   … {i}/{len(todo)}")
        json.dump(cache, open(cache_path, "w", encoding="utf-8"), ensure_ascii=False)
json.dump(cache, open(cache_path, "w", encoding="utf-8"), ensure_ascii=False)

# 第三步：合并频次（中文名 += 其日文写法的频次）
print("\n" + "=" * 72)
print("第三步：与 wiki 全名单匹配，算热度")
print("=" * 72)
raw = json.load(open(RAW, encoding="utf-8"))
strip_paren = re.compile(r"^(.)[（(]([^）)]{1,25})[)）]$")


def merged_counter(game):
    counter = Counter()
    for tag, c in freq[game].items():
        counter[tag] += c
        m = strip_paren.match(tag)
        if m:
            counter[m.group(1)] += c
        if not CJK.search(tag):
            zh = cache.get(tag)
            if zh:
                counter[zh] += c
    return counter


heat = {}
for game, names in raw.items():
    counter = merged_counter(game)
    scored = [(n, counter.get(n, 0)) for n in names]
    scored.sort(key=lambda x: (-x[1], x[0]))
    hot = [n for n, c in scored if c > 0]
    heat[game] = {"top": scored[:60], "hotCount": len(hot), "total": len(names)}
    print(f"\n  {game}：候选 {len(names)} 个，有热度 {len(hot)} 个")
    print("     热度前 25：", [f"{n}×{c}" for n, c in scored[:25]])

json.dump({g: {"top": v["top"], "hotCount": v["hotCount"], "total": v["total"]}
           for g, v in heat.items()}, open(OUT, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print("\n已写入:", OUT)
