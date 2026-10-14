//! PDF 生成探针（v1.3.6 开发用，不参与发布）。
//!
//! 验证四件事：krilla 写出的 PDF 能被打开 / 系统黑体的**子集嵌入**（体积别失控）/
//! 中文排版与自动分页 / JPEG 与 PNG 图片嵌入。
//!
//! 跑法：cargo run --manifest-path src-tauri/Cargo.toml --example pdf_probe
//! 产物：.workbuddy/probe/out/pdf-probe.pdf
//!
//! 只读本地素材（系统字体 / data 下头像 jpg / public 帮助页 png），不联网。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use krilla::geom::{Point, Size, Transform};
use krilla::image::Image;
use krilla::page::PageSettings;
use krilla::text::{Font, TextDirection};
use krilla::Document;

const PAGE_W: f32 = 595.28; // A4 宽（pt）
const PAGE_H: f32 = 841.89; // A4 高（pt）
const MARGIN_X: f32 = 58.0;
const MARGIN_TOP: f32 = 62.0;
const MARGIN_BOTTOM: f32 = 58.0;
const BODY_SIZE: f32 = 12.0;
const BODY_LINE: f32 = 21.0;
const TITLE_SIZE: f32 = 19.0;
const TITLE_LINE: f32 = 30.0;

enum Row {
    Text {
        text: String,
        size: f32,
        indent: f32,
        height: f32,
    },
    Picture {
        image: Image,
        width: f32,
        height: f32,
    },
}

struct Pager {
    pages: Vec<Vec<Row>>,
    cursor: f32,
}

impl Pager {
    fn new() -> Self {
        Self {
            pages: vec![Vec::new()],
            cursor: MARGIN_TOP,
        }
    }

    fn ensure_room(&mut self, height: f32) {
        if self.cursor + height > PAGE_H - MARGIN_BOTTOM {
            self.pages.push(Vec::new());
            self.cursor = MARGIN_TOP;
        }
    }

    fn push(&mut self, row: Row, height: f32) {
        self.ensure_room(height);
        self.cursor += height;
        self.pages.last_mut().unwrap().push(row);
    }

    fn gap(&mut self, height: f32) {
        self.ensure_room(height);
        self.cursor += height;
    }
}

fn main() {
    let started = Instant::now();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();

    // 字体：系统黑体（simhei.ttf 是独立 TTF，可直接子集嵌入）
    let font_path = PathBuf::from("C:/Windows/Fonts/simhei.ttf");
    let font_data: Arc<Vec<u8>> = Arc::new(fs::read(&font_path).expect("读取 simhei.ttf 失败"));
    let face = ttf_parser::Face::parse(&font_data, 0).expect("解析 simhei.ttf 失败");
    let upm = face.units_per_em() as f32;
    let ascent = face.ascender() as f32 / upm;
    let krilla_font = Font::new(font_data.clone().into(), 0).expect("krilla 加载字体失败");

    let advance = |ch: char, size: f32| -> f32 {
        face.glyph_index(ch)
            .and_then(|id| face.glyph_hor_advance(id))
            .map(|units| units as f32 / upm * size)
            .unwrap_or(size * 0.5)
    };

    let max_w = PAGE_W - MARGIN_X * 2.0;
    let mut pager = Pager::new();
    let mut used_texts = std::collections::BTreeSet::new();

    // 标题
    pager.push(
        Row::Text {
            text: "PDF 生成探针 · 中文排版测试".to_string(),
            size: TITLE_SIZE,
            indent: 0.0,
            height: TITLE_LINE,
        },
        TITLE_LINE + 10.0,
    );

    // 两张测试图：jpg（原样直嵌）与 png（帮助页素材）
    let jpg_path = root.join("data/avatars/pixiv-author-1.jpg");
    let png_path = fs::read_dir(root.join("public/help/images"))
        .ok()
        .and_then(|entries| {
            entries.flatten().map(|entry| entry.path()).find(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or_else(|| root.join("public/help/images/missing.png"));

    let load_image = |path: &PathBuf| -> Option<Image> {
        let bytes = fs::read(path).ok()?;
        let is_png = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("png"))
            .unwrap_or(false);
        let result = if is_png {
            Image::from_png(Arc::new(bytes).into(), true)
        } else {
            Image::from_jpeg(Arc::new(bytes).into(), true)
        };
        match result {
            Ok(image) => Some(image),
            Err(error) => {
                eprintln!("图片构造失败（{}）：{error}", path.display());
                None
            }
        }
    };

    let paragraph = "这是一段用来验证中文排版的中文文本，包含常见的标点符号（逗号、句号、问号？感叹号！顿号、分号；冒号：以及“引号”）和英文字母 ABC abc 12345，看看混排效果。句子要足够长，才能触发自动换行，也顺便检查每一行的行距是否均匀、段落首行缩进是否只有一个字的宽度、分页时行不会重叠或者被截断。";

    for index in 0..14 {
        if index == 4 || index == 9 {
            let path = if index == 4 { &jpg_path } else { &png_path };
            if let Some(image) = load_image(path) {
                let (pw, ph) = image.size();
                let width = (max_w * 0.72).min(pw as f32);
                let height = if pw == 0 {
                    20.0
                } else {
                    width * ph as f32 / pw as f32
                };
                pager.gap(14.0);
                pager.push(Row::Picture { image, width, height }, height);
                pager.gap(14.0);
            } else {
                eprintln!("跳过图片（读不到）：{}", path.display());
            }
        }

        // 段落换行：首行缩进两个字，续行不缩进；逐字累加宽度断行
        let first_indent = BODY_SIZE * 2.0;
        let mut line = String::new();
        let mut width = 0.0;
        let mut current_indent = first_indent;
        for ch in paragraph.chars() {
            let ch_width = advance(ch, BODY_SIZE);
            let avail = max_w - current_indent;
            if width + ch_width > avail && !line.is_empty() {
                for c in line.chars() {
                    used_texts.insert(c);
                }
                pager.push(
                    Row::Text {
                        text: std::mem::take(&mut line),
                        size: BODY_SIZE,
                        indent: current_indent,
                        height: BODY_LINE,
                    },
                    BODY_LINE,
                );
                width = 0.0;
                current_indent = 0.0;
            }
            line.push(ch);
            width += ch_width;
        }
        if !line.is_empty() {
            for c in line.chars() {
                used_texts.insert(c);
            }
            pager.push(
                Row::Text {
                    text: line,
                    size: BODY_SIZE,
                    indent: current_indent,
                    height: BODY_LINE,
                },
                BODY_LINE,
            );
        }
        pager.gap(8.0); // 段间距
    }

    // 渲染
    let mut document = Document::new();
    for rows in &pager.pages {
        let mut page = document.start_page_with(PageSettings::from_wh(PAGE_W, PAGE_H).unwrap());
        let mut surface = page.surface();
        let mut cursor = MARGIN_TOP;
        for row in rows {
            match row {
                Row::Text {
                    text,
                    size,
                    indent,
                    height,
                } => {
                    // krilla 的坐标是**左上原点、y 向下**（surface.rs 的约定），
                    // cursor＝行顶距页顶，baseline 再往下走一个 ascent。
                    let baseline = cursor + size * ascent;
                    surface.draw_text(
                        Point::from_xy(MARGIN_X + indent, baseline),
                        krilla_font.clone(),
                        *size,
                        text,
                        false,
                        TextDirection::Auto,
                    );
                    cursor += height;
                }
                Row::Picture {
                    image,
                    width,
                    height,
                } => {
                    // 左上原点：图片顶边就在 cursor 处
                    let y = cursor;
                    let x = MARGIN_X + (max_w - width) / 2.0;
                    surface.push_transform(&Transform::from_translate(x, y));
                    surface.draw_image(
                        image.clone(),
                        Size::from_wh(*width, *height).expect("图片尺寸为正数"),
                    );
                    surface.pop();
                    cursor += height;
                }
            }
        }
        surface.finish();
        page.finish();
    }
    let pdf = document.finish().unwrap();

    let out_dir = root.join(".workbuddy/probe/out");
    fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("pdf-probe.pdf");
    fs::write(&out, &pdf).unwrap();

    println!("页数：{}", pager.pages.len());
    println!(
        "PDF 大小：{} 字节（{:.1} KB）",
        pdf.len(),
        pdf.len() as f64 / 1024.0
    );
    println!("正文用字：{} 个不同字符", used_texts.len());
    println!("耗时：{:.2?}", started.elapsed());
    println!("输出：{}", out.display());
}
