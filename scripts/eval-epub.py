# -*- coding: utf-8 -*-
"""EPUB 方案评估脚本（一次性测量，不参与软件运行）。

拿真实 Pixiv 小说的正文 + 正文配图，分别按「1200 宽」和「原图」两档量三件事：
下载耗时、EPUB 生成耗时、EPUB 体积。用来判断值不值得做 EPUB 导出。

用法：
    python scripts/eval-epub.py                # 用内置的两篇样例（4 图 / 19 图）
    python scripts/eval-epub.py 24319453 ...   # 指定小说 ID
"""

import io
import json
import os
import re
import sqlite3
import sys
import time
import urllib.request
import zipfile

DB_CANDIDATES = [
    r"D:\100 SoftWare\120 EnhaceSoftware\pixiv-novel-downloader\data\library.db",
    r"发布\藏集\PixivNovelDownloader\data\library.db",
]
UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)
QUALITY_KEYS = {"1200": "1200x1200", "original": "original"}
DEFAULT_NOVELS = ["24319453", "26410188"]


def read_cookie():
    for path in DB_CANDIDATES:
        if not os.path.isfile(path):
            continue
        try:
            conn = sqlite3.connect(path)
            row = conn.execute(
                "SELECT value FROM app_settings WHERE key='pixiv_cookie'"
            ).fetchone()
            conn.close()
        except sqlite3.Error:
            continue
        if row and row[0]:
            return row[0]
    return ""


def get_json(url, cookie):
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": UA,
            "Referer": "https://www.pixiv.net/",
            "Accept": "application/json",
            "Cookie": cookie,
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def get_bytes(url, cookie):
    request = urllib.request.Request(
        url,
        headers={"User-Agent": UA, "Referer": "https://www.pixiv.net/", "Cookie": cookie},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def image_refs(content):
    """按正文出现顺序取出去重后的图片 ID（uploadedimage / pixivimage 两类）。"""
    refs, seen = [], set()
    for match in re.finditer(r"\[(uploadedimage|pixivimage):(\d+)\]", content):
        token = match.group(0)
        if token in seen:
            continue
        seen.add(token)
        refs.append((match.group(1), match.group(2)))
    return refs


def collect_images(body, content, cookie, quality):
    """返回 [(文件名, 字节)]，顺序与正文一致。"""
    embedded = body.get("textEmbeddedImages") or {}
    out = []
    for index, (kind, image_id) in enumerate(image_refs(content), start=1):
        if kind == "uploadedimage":
            urls = (embedded.get(image_id) or {}).get("urls") or {}
            key = QUALITY_KEYS[quality]
            url = urls.get(key) or urls.get("1200x1200") or urls.get("original")
        else:
            try:
                detail = get_json(f"https://www.pixiv.net/ajax/illust/{image_id}", cookie)
                urls = (detail.get("body") or {}).get("urls") or {}
            except Exception:
                urls = {}
            url = urls.get("original" if quality == "original" else "regular") or urls.get(
                "original"
            )
        if not url:
            continue
        extension = re.sub(r"[?#].*$", "", url).rsplit(".", 1)[-1].lower()
        if extension not in ("jpg", "jpeg", "png", "gif", "webp"):
            extension = "jpg"
        if extension == "jpeg":
            extension = "jpg"
        out.append((f"{index:03}.{extension}", url))
    return out


def escape(value):
    return (
        value.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def body_xhtml(content, images):
    text = content
    for name, (file_name, _) in zip(image_refs(text), images):
        text = text.replace(f"[{name[0]}:{name[1]}]", f"[img:{file_name}]")

    parts, paragraph = [], []

    def flush():
        if paragraph:
            parts.append("<p>" + "<br/>".join(paragraph) + "</p>")
            paragraph.clear()

    for chunk in re.split(r"(\[[^\]]*\])", text):
        if not chunk:
            continue
        if chunk.startswith("[") and chunk.endswith("]"):
            inner = chunk[1:-1]
            if inner.startswith("img:"):
                flush()
                parts.append(f'<figure><img src="../images/{inner[4:]}" alt="插图"/></figure>')
                continue
            if inner.startswith("chapter:"):
                flush()
                parts.append(f"<h2>{escape(inner[8:].strip())}</h2>")
                continue
            if inner == "newpage":
                flush()
                parts.append('<hr class="page"/>')
                continue
            if inner.startswith("rb:") or inner.startswith("[rb:"):
                base, _, reading = inner.lstrip("[").removeprefix("rb:").partition(">")
                paragraph.append(
                    f"<ruby>{escape(base.strip())}<rt>{escape(reading.strip())}</rt></ruby>"
                )
                continue
        for line in chunk.split("\n"):
            if line.strip():
                paragraph.append(escape(line.strip()))
            else:
                flush()
    flush()
    return "".join(parts)


def build_epub(title, content, images, blobs, author, cover):
    # 图片本身已经是压缩格式（PNG/JPG），zip 里直存即可：80 MB 从 1.7s 降到 0.1s，体积完全一样。
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr(
            zipfile.ZipInfo("mimetype"), "application/epub+zip", zipfile.ZIP_STORED
        )
        archive.writestr(
            "META-INF/container.xml",
            '<?xml version="1.0" encoding="UTF-8"?><container version="1.0" '
            'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
            '<rootfile full-path="OEBPS/content.opf" '
            'media-type="application/oebps-package+xml"/></rootfiles></container>',
            zipfile.ZIP_DEFLATED,
        )
        archive.writestr(
            "OEBPS/style.css",
            "body{font-family:'Noto Serif CJK SC',serif;line-height:1.9;margin:1em;}"
            "h2{font-size:1.1em;border-left:3px solid #4a90d9;padding-left:.5em;}"
            "img{max-width:100%;}figure{text-align:center;margin:1.2em 0;}"
            "ruby rt{font-size:.6em;}hr.page{border:0;border-top:1px dashed #ccc;}",
            zipfile.ZIP_DEFLATED,
        )
        manifest = [
            '<item id="style" href="style.css" media-type="text/css"/>',
            '<item id="text" href="text/chapter1.xhtml" media-type="application/xhtml+xml"/>',
            '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>',
        ]
        spine, items = ['<itemref idref="text"/>'], []
        if cover:
            archive.writestr("OEBPS/images/cover.jpg", blobs[cover], zipfile.ZIP_STORED)
            manifest.append('<item id="cover" href="images/cover.jpg" media-type="image/jpeg"/>')
            manifest.append(
                '<item id="coverpage" href="text/cover.xhtml" media-type="application/xhtml+xml"/>'
            )
            spine.insert(0, '<itemref idref="coverpage"/>')
            archive.writestr(
                "OEBPS/text/cover.xhtml",
                '<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml">'
                '<head><title>封面</title></head><body><img src="../images/cover.jpg" alt="封面"/>'
                "</body></html>",
                zipfile.ZIP_DEFLATED,
            )
        for index, (file_name, _) in enumerate(images):
            archive.writestr(
                f"OEBPS/images/{file_name}", blobs[file_name], zipfile.ZIP_STORED
            )
            media = "image/png" if file_name.endswith(".png") else "image/jpeg"
            manifest.append(
                f'<item id="img{index}" href="images/{file_name}" media-type="{media}"/>'
            )
        body = body_xhtml(content, images)
        archive.writestr(
            "OEBPS/text/chapter1.xhtml",
            '<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml">'
            f"<head><title>{escape(title)}</title>"
            '<link rel="stylesheet" type="text/css" href="../style.css"/></head>'
            f"<body><h1>{escape(title)}</h1><p>{escape(author)}</p>{body}</body></html>",
            zipfile.ZIP_DEFLATED,
        )
        archive.writestr(
            "OEBPS/nav.xhtml",
            '<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml" '
            'xmlns:epub="http://www.idpf.org/2007/ops"><head><title>目录</title></head><body>'
            '<nav epub:type="toc"><ol><li><a href="text/chapter1.xhtml">正文</a></li></ol></nav>'
            "</body></html>",
            zipfile.ZIP_DEFLATED,
        )
        archive.writestr(
            "OEBPS/content.opf",
            '<?xml version="1.0" encoding="UTF-8"?><package xmlns="http://www.idpf.org/2007/opf" '
            'version="3.0" unique-identifier="bookid"><metadata '
            'xmlns:dc="http://purl.org/dc/elements/1.1/">'
            f'<dc:identifier id="bookid">pixiv-{abs(hash(title)) % 10**10}</dc:identifier>'
            f"<dc:title>{escape(title)}</dc:title><dc:language>zh-CN</dc:language>"
            f"<dc:creator>{escape(author)}</dc:creator></metadata>"
            f"<manifest>{''.join(manifest)}</manifest><spine>{''.join(spine)}</spine></package>",
            zipfile.ZIP_DEFLATED,
        )
    return buffer.getvalue()


def measure(novel_id, cookie, out_dir=None):
    started = time.time()
    detail = get_json(f"https://www.pixiv.net/ajax/novel/{novel_id}", cookie)
    body = detail.get("body") or {}
    content = body.get("content") or ""
    title = body.get("title") or novel_id
    author = body.get("userName") or ""
    print(f"\n=== 小说 {novel_id}：《{title[:40]}》 作者 {author} ===")
    print(f"  正文 {len(content)} 字符，图片引用 {len(image_refs(content))} 处")
    for quality in ("1200", "original"):
        targets = collect_images(body, content, cookie, quality)
        download_start = time.time()
        blobs, total = {}, 0
        for file_name, url in targets:
            data = get_bytes(url, cookie)
            blobs[file_name] = data
            total += len(data)
        download_time = time.time() - download_start
        cover_name = None
        cover_url = body.get("coverUrl") or ""
        if cover_url:
            cover_name = "cover.jpg"
            blobs[cover_name] = get_bytes(cover_url, cookie)
            total += len(blobs[cover_name])
        build_start = time.time()
        epub = build_epub(title, content, targets, blobs, author, cover_name)
        build_time = time.time() - build_start
        print(
            f"  [{quality}] 图片 {len(targets)} 张 + 封面："
            f"下载 {download_time:.1f}s（{total / 1024 / 1024:.1f} MB），"
            f"打包 {build_time:.2f}s，EPUB {len(epub) / 1024 / 1024:.1f} MB"
        )
        if out_dir and quality == "1200":
            os.makedirs(out_dir, exist_ok=True)
            sample = os.path.join(out_dir, f"样张-{novel_id}.epub")
            with open(sample, "wb") as handle:
                handle.write(epub)
            print(f"  样张已写出：{sample}")
    print(f"  （整篇一次运行总耗时 {time.time() - started:.1f}s）")


def main():
    cookie = read_cookie()
    if not cookie:
        print("没读到 Pixiv Cookie，无法请求 Pixiv 接口。")
        return
    args = sys.argv[1:]
    out_dir = None
    if "--out" in args:
        index = args.index("--out")
        out_dir = args[index + 1]
        del args[index : index + 2]
    novels = args or DEFAULT_NOVELS
    for novel_id in novels:
        try:
            measure(novel_id, cookie, out_dir)
        except Exception as error:  # 评估脚本，出错就跳过
            print(f"  小说 {novel_id} 处理失败：{error}")


if __name__ == "__main__":
    main()
