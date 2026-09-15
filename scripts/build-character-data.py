# 由热度排名生成最终内置角色数据（打包进程序）
# 规则：只收「在 pixiv 上确实有同人热度」的角色（热度≥1）；
#       排除游戏名本身、单字名（误伤太大）、带 LV/版本号的变体标签。
# 输出 src-tauri/characters.json
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
RES = os.path.join(HERE, "..", ".workbuddy", "research")
RANK = os.path.join(RES, "character-rank.json")
OUT = os.path.join(HERE, "..", "src-tauri", "characters.json")

rank = json.load(open(RANK, encoding="utf-8"))

GAME_WORDS = {"碧蓝航线", "明日方舟", "原神", "绝区零", "崩坏3", "崩坏3rd", "鸣潮", "蔚蓝档案",
              "碧蓝档案", "崩坏星穹铁道", "崩壊:スターレイル", "星穹铁道", "明日方舟终末地",
              "崩坏学园", "崩坏", "崩壊", "原神学園", "原神BL"}
BAD = re.compile(r"(LV\.?\d|#\d|版$|^第|集$|篇$|Ver\.?\d|ver\.?\d|\d+周年)")

result = {}
for game, v in rank.items():
    picked = []
    for name, heat in v["hot"]:
        if heat < 1:
            continue
        if name in GAME_WORDS:
            continue
        if len(name) < 2 or len(name) > 14:
            continue
        if BAD.search(name):
            continue
        # 纯英文/数字的组合照收（Z23、U-552、Mon3tr 是真实舰船/干员名）
        picked.append({"name": name, "heat": heat})
    result[game] = picked

total = sum(len(v) for v in result.values())
print(f"内置角色 {total} 个（{len(result)} 款游戏）\n")
for game, items in result.items():
    names = [x["name"] for x in items]
    print(f"{game}（{len(items)}）")
    print("   ", "、".join(names))
    print()

with open(OUT, "w", encoding="utf-8") as f:
    json.dump(result, f, ensure_ascii=False, indent=1)
size = os.path.getsize(OUT)
print(f"已写入 {os.path.abspath(OUT)}（{size/1024:.1f} KB）")
