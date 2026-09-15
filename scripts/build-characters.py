# 生成「常见角色名」内置数据：从各游戏中文 wiki 拉全量角色名单（不判性别）
# 热度排序由 build-character-heat.py 完成
# 输出 characters-raw.json：{游戏: [名字, ...]}
import json
import os
import re
import time
import urllib.error
import urllib.parse
import urllib.request

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", ".workbuddy", "research", "characters-raw.json")
BA_NAMES = os.path.join(HERE, "..", ".workbuddy", "research", "bluearchive-names.json")
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

# 游戏 -> (wiki 根地址, 角色分类名)
SITES = [
    ("碧蓝航线", "https://wiki.biligame.com/blhx", "舰娘"),
    ("明日方舟", "https://prts.wiki", "干员"),
    ("原神", "https://wiki.biligame.com/ys", "角色"),
    ("崩坏：星穹铁道", "https://wiki.biligame.com/sr", "角色"),
    ("绝区零", "https://wiki.biligame.com/zzz", "角色"),
    ("崩坏3", "https://wiki.biligame.com/bh3", "角色"),
    ("鸣潮", "https://wiki.biligame.com/wutheringwaves", "共鸣者"),
]


def api(base, params, retries=4):
    url = base + "/api.php?" + urllib.parse.urlencode(params)
    for attempt in range(retries):
        req = urllib.request.Request(url)
        req.add_header("User-Agent", UA)
        req.add_header("Referer", base + "/")
        req.add_header("Accept", "application/json, text/javascript, */*; q=0.01")
        req.add_header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        # PRTS 的 WAF 会拦掉"不像浏览器"的请求，这两个头是必需项
        req.add_header("X-Requested-With", "XMLHttpRequest")
        req.add_header("Connection", "keep-alive")
        try:
            with OPENER.open(req, timeout=30) as resp:
                time.sleep(1.0)
                return json.loads(resp.read().decode("utf-8", "replace"))
        except urllib.error.HTTPError as e:
            if e.code in (403, 429, 500, 502, 503):
                time.sleep(3 + attempt * 4)
                continue
            return None
        except Exception:
            time.sleep(2 + attempt * 2)
    return None


def category_members(base, cat):
    out, cont, idle = [], None, 0
    while idle < 3:
        params = {"action": "query", "list": "categorymembers", "cmtitle": "Category:" + cat,
                  "cmlimit": "200", "format": "json"}
        if cont:
            params.update(cont)
        d = api(base, params)
        if not d:
            idle += 1
            continue
        batch = [m["title"] for m in d.get("query", {}).get("categorymembers", [])]
        out += batch
        cont = d.get("continue")
        if not cont or not batch:
            break
    return out


SKIP_PREFIX = ("模板:", "Template:", "分类:", "Category:", "文件:", "File:", "帮助:", "Help:",
               "模块:", "Module:", "Widget:", "MediaWiki:", "用户:", "User:", "沙盒:")
SKIP_EXACT = {"首页", "角色一览", "干员一览", "舰娘列表", "导航", "公告", "更新公告", "关于我们",
              "角色需要内容", "角色筛选", "共鸣者", "角色", "学生", "教师"}
# 玩家化身 / 双性别选项 / 非具体角色，不作为可匹配的「角色名」
SKIP_ROLE = {"旅行者", "开拓者", "漂泊者", "星", "穹", "绳匠", "主角", "主人公", "老师", "先生",
             "指挥官", "博士", "御主", "藤丸立香", "空", "荧", "男漂泊者", "女漂泊者", "佩洛伊斯",
             "代理", "校长", "主人"}
# 蔚蓝档案名单里的学校/组织名，不是角色
SCHOOL_RE = re.compile(r"(学园|學園|高中|学校|學校|大学|大學|学院|學院|联合|聯合|委员会|委員會|部$|会$|會$|企业|企業|集团|集團|学院都市)")


def clean_names(title):
    """wiki 标题 -> [候选名, ...]（含引号形态的别名）"""
    if not title or title.startswith(SKIP_PREFIX):
        return []
    t = title.strip()
    if t in SKIP_EXACT:
        return []
    if "/" in t:            # 「共鸣者/爱弥斯」-> 爱弥斯
        t = t.split("/")[-1].strip()
    t = re.sub(r"[（(][^）)]{1,14}[）)]$", "", t).strip()   # 「阿米娅(近卫)」-> 阿米娅
    if not t or len(t) > 16:
        return []
    if SCHOOL_RE.search(t):
        return []
    names = []
    stripped = t.strip("「」『』\"' ")
    if stripped and stripped != t:
        names += [stripped, t]
    else:
        names.append(t)
    return [n for n in names if n and n not in SKIP_ROLE]


print("=" * 72)
print("拉各游戏角色全名单（不判性别）")
print("=" * 72)
result = {}
for game, base, cat in SITES:
    members = category_members(base, cat)
    names = []
    for m in members:
        for n in clean_names(m):
            if n not in names:
                names.append(n)
    result[game] = names
    print(f"  {game:<13} Category:{cat:<8} {len(members):>4} 项 → 角色名 {len(names):>4} 个")

# 蔚蓝档案：中文 wiki 的 API 不可用，名单取自萌娘百科「蔚蓝档案/学生」页面
if os.path.isfile(BA_NAMES):
    raw = json.load(open(BA_NAMES, encoding="utf-8"))
    names = []
    for t in raw:
        for n in clean_names(t):
            if n not in names:
                names.append(n)
    result["蔚蓝档案"] = names
    print(f"  {'蔚蓝档案':<13} 萌娘百科学生名单 → 角色名 {len(names):>4} 个")

print("\n" + "=" * 72)
total = sum(len(v) for v in result.values())
print(f"候选角色名合计 {total} 个（{len(result)} 款游戏）")
for game, names in result.items():
    print(f"  {game:<14} {len(names):>4}  {names[:10]}")

with open(OUT, "w", encoding="utf-8") as f:
    json.dump(result, f, ensure_ascii=False, indent=1)
print("\n已写入:", os.path.abspath(OUT))
