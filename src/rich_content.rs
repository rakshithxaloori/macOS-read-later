//! Saved HTML is parsed as data. No scripts, publisher resources, or browser run here.
use crate::reader_style::ReaderFont;
use crate::theme::{Appearance, palette};
use egui::{Color32, FontFamily, FontId, Layout, RichText, TextureHandle, Ui, Vec2};
use scraper::{ElementRef, Html, Selector};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub superscript: bool,
    pub subscript: bool,
    pub href: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParagraphKind {
    Prose,
    Heading(u8),
    Quote,
    Code,
    Caption,
    ListItem { marker: String, depth: usize },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub content: Vec<Block>,
    pub colspan: usize,
    pub rowspan: usize,
    pub header: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph {
        kind: ParagraphKind,
        runs: Vec<Inline>,
        anchor: Option<String>,
    },
    Image {
        source: String,
        alt: String,
    },
    Table {
        caption: Vec<Inline>,
        rows: Vec<Vec<Cell>>,
    },
    Rule,
    Details {
        summary: Vec<Inline>,
        content: Vec<Block>,
    },
}
#[derive(Clone, Debug)]
pub struct RichDocument {
    pub image_layouts: HashMap<String, String>,
    pub image_links: HashMap<String, String>,
    pub blocks: Vec<Block>,
    /// Digest of the original saved bytes, never of a flattened presentation.
    pub source_sha256: String,
}
impl RichDocument {
    pub fn parse(saved_html: &str) -> Self {
        let html = Html::parse_fragment(saved_html);
        let blocks = parse_children(html.root_element(), 0);
        let image_layouts = html
            .select(&selector("img[data-layout]"))
            .filter_map(|e| {
                Some((
                    e.value().attr("src")?.to_owned(),
                    e.value().attr("data-layout")?.to_owned(),
                ))
            })
            .collect();
        let image_links = html
            .select(&selector("a[href] img"))
            .filter_map(|image| {
                let source = image.value().attr("src")?;
                let parent = image
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .find(|e| e.value().name() == "a")?;
                Some((source.to_owned(), parent.value().attr("href")?.to_owned()))
            })
            .collect();
        Self {
            image_layouts,
            image_links,
            blocks,
            source_sha256: format!("{:x}", Sha256::digest(saved_html.as_bytes())),
        }
    }
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        text_blocks(&self.blocks, &mut out);
        out
    }
    pub fn image_sources(&self) -> Vec<&str> {
        fn collect<'a>(blocks: &'a [Block], out: &mut Vec<&'a str>) {
            for block in blocks {
                match block {
                    Block::Image { source, .. } => out.push(source),
                    Block::Table { rows, .. } => {
                        for row in rows {
                            for cell in row {
                                collect(&cell.content, out);
                            }
                        }
                    }
                    Block::Details { content, .. } => collect(content, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        collect(&self.blocks, &mut out);
        out
    }
}
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}
fn runs(element: ElementRef<'_>) -> Vec<Inline> {
    fn visit(element: ElementRef<'_>, style: InlineStyle, out: &mut Vec<Inline>) {
        let mut next = style;
        match element.value().name() {
            "strong" | "b" => next.strong = true,
            "em" | "i" => next.emphasis = true,
            "u" => next.underline = true,
            "s" | "del" => next.strike = true,
            "code" | "pre" => next.code = true,
            "sup" => next.superscript = true,
            "sub" => next.subscript = true,
            "a" => next.href = element.value().attr("href").map(str::to_owned),
            "br" => {
                out.push(Inline {
                    text: "\n".into(),
                    style: next,
                });
                return;
            }
            "script" | "style" | "iframe" | "img" => return,
            _ => {}
        }
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                let value = if next.code {
                    text.to_string()
                } else {
                    collapse_whitespace(text)
                };
                if !value.is_empty() {
                    out.push(Inline {
                        text: value,
                        style: next.clone(),
                    });
                }
            } else if let Some(child) = ElementRef::wrap(child) {
                visit(child, next.clone(), out);
            }
        }
    }
    let mut out = Vec::new();
    visit(element, InlineStyle::default(), &mut out);
    out
}
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(ch);
            space = false;
        }
    }
    out
}
fn paragraph(element: ElementRef<'_>, kind: ParagraphKind) -> Block {
    Block::Paragraph {
        kind,
        runs: runs(element),
        anchor: element.value().attr("id").map(str::to_owned),
    }
}
fn flow_blocks(element: ElementRef<'_>, kind: ParagraphKind) -> Vec<Block> {
    fn visit(
        element: ElementRef<'_>,
        mut style: InlineStyle,
        pending: &mut Vec<Inline>,
        out: &mut Vec<Block>,
        kind: &ParagraphKind,
        anchor: &mut Option<String>,
    ) {
        match element.value().name() {
            "strong" | "b" => style.strong = true,
            "em" | "i" => style.emphasis = true,
            "u" => style.underline = true,
            "s" | "del" => style.strike = true,
            "code" | "pre" => style.code = true,
            "sup" => style.superscript = true,
            "sub" => style.subscript = true,
            "a" => style.href = element.value().attr("href").map(str::to_owned),
            "br" => {
                pending.push(Inline {
                    text: "\n".into(),
                    style,
                });
                return;
            }
            "script" | "style" | "iframe" => return,
            "img" => {
                if !pending.is_empty() {
                    out.push(Block::Paragraph {
                        kind: kind.clone(),
                        runs: std::mem::take(pending),
                        anchor: anchor.take(),
                    });
                }
                out.push(image_block(element));
                return;
            }
            _ => {}
        }
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                let text = if style.code {
                    text.to_string()
                } else {
                    collapse_whitespace(text)
                };
                if !text.is_empty() {
                    pending.push(Inline {
                        text,
                        style: style.clone(),
                    });
                }
            } else if let Some(child) = ElementRef::wrap(child) {
                visit(child, style.clone(), pending, out, kind, anchor);
            }
        }
    }
    let mut pending = Vec::new();
    let mut out = Vec::new();
    let mut anchor = element.value().attr("id").map(str::to_owned);
    visit(
        element,
        InlineStyle::default(),
        &mut pending,
        &mut out,
        &kind,
        &mut anchor,
    );
    if pending.iter().any(|run| !run.text.trim().is_empty()) {
        out.push(Block::Paragraph {
            kind,
            runs: pending,
            anchor,
        });
    }
    out
}
fn parse_children(element: ElementRef<'_>, depth: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut loose = Vec::<Inline>::new();
    for child in element.children() {
        if let Some(text) = child.value().as_text() {
            if !text.trim().is_empty() {
                loose.push(Inline {
                    text: collapse_whitespace(text),
                    style: InlineStyle::default(),
                });
            }
            continue;
        }
        let Some(e) = ElementRef::wrap(child) else {
            continue;
        };
        if matches!(
            e.value().name(),
            "script" | "style" | "iframe" | "nav" | "form"
        ) {
            continue;
        }
        let inline = matches!(
            e.value().name(),
            "span" | "strong" | "b" | "em" | "i" | "u" | "s" | "a" | "code" | "sup" | "sub" | "br"
        );
        if inline {
            if e.select(&selector("img")).next().is_some() {
                if !loose.is_empty() {
                    blocks.push(Block::Paragraph {
                        kind: ParagraphKind::Prose,
                        runs: std::mem::take(&mut loose),
                        anchor: None,
                    });
                }
                blocks.extend(flow_blocks(e, ParagraphKind::Prose));
            } else {
                loose.extend(runs(e));
            }
            continue;
        }

        if !loose.is_empty() {
            blocks.push(Block::Paragraph {
                kind: ParagraphKind::Prose,
                runs: std::mem::take(&mut loose),
                anchor: None,
            });
        }
        match e.value().name() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => blocks.push(paragraph(
                e,
                ParagraphKind::Heading(e.value().name().as_bytes()[1] - b'0'),
            )),
            "p" => blocks.extend(flow_blocks(e, ParagraphKind::Prose)),
            "blockquote" => {
                let nested = parse_children(e, depth);
                if nested.is_empty() {
                    blocks.push(paragraph(e, ParagraphKind::Quote));
                } else {
                    for block in nested {
                        blocks.push(match block {
                            Block::Paragraph { runs, anchor, .. } => Block::Paragraph {
                                kind: ParagraphKind::Quote,
                                runs,
                                anchor,
                            },
                            other => other,
                        });
                    }
                }
            }
            "pre" => blocks.push(paragraph(e, ParagraphKind::Code)),
            "figcaption" | "caption" => blocks.push(paragraph(e, ParagraphKind::Caption)),
            "img" => blocks.push(image_block(e)),
            "hr" => blocks.push(Block::Rule),
            "ul" | "ol" => {
                let start = e
                    .value()
                    .attr("start")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1);
                for (index, item) in (start..).zip(
                    e.children()
                        .filter_map(ElementRef::wrap)
                        .filter(|e| e.value().name() == "li"),
                ) {
                    let marker = if e.value().name() == "ol" {
                        format!("{index}.")
                    } else {
                        "•".into()
                    };
                    // Descendant lists must not be duplicated inside their parent item.
                    let mut inline_runs = Vec::new();
                    for child in item.children() {
                        if let Some(t) = child.value().as_text() {
                            inline_runs.push(Inline {
                                text: collapse_whitespace(t),
                                style: InlineStyle::default(),
                            });
                        } else if let Some(c) = ElementRef::wrap(child)
                            && !matches!(c.value().name(), "ul" | "ol")
                        {
                            inline_runs.extend(runs(c));
                        }
                    }
                    blocks.push(Block::Paragraph {
                        kind: ParagraphKind::ListItem { marker, depth },
                        runs: inline_runs,
                        anchor: item.value().attr("id").map(str::to_owned),
                    });
                    for list in item
                        .children()
                        .filter_map(ElementRef::wrap)
                        .filter(|e| matches!(e.value().name(), "ul" | "ol"))
                    {
                        blocks.extend(parse_children_of_list(list, depth + 1));
                    }
                }
            }
            "table" => {
                let caption = e
                    .select(&selector("caption"))
                    .next()
                    .map(runs)
                    .unwrap_or_default();
                let rows = e
                    .select(&selector("tr"))
                    .filter(|row| {
                        row.ancestors()
                            .filter_map(ElementRef::wrap)
                            .find(|a| a.value().name() == "table")
                            .map(|a| a.id())
                            == Some(e.id())
                    })
                    .map(|row| {
                        row.children()
                            .filter_map(ElementRef::wrap)
                            .filter(|c| matches!(c.value().name(), "td" | "th"))
                            .map(|cell| {
                                let mut content = parse_children(cell, depth);
                                if content.is_empty() {
                                    content.push(paragraph(cell, ParagraphKind::Prose));
                                }
                                Cell {
                                    content,
                                    colspan: span(cell, "colspan"),
                                    rowspan: span(cell, "rowspan"),
                                    header: cell.value().name() == "th",
                                }
                            })
                            .collect()
                    })
                    .collect();
                blocks.push(Block::Table { caption, rows });
            }
            "details" => {
                let summary = e
                    .select(&selector("summary"))
                    .next()
                    .map(runs)
                    .unwrap_or_default();
                let mut content = Vec::new();
                for c in e
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|c| c.value().name() != "summary")
                {
                    let nested = parse_children(c, depth);
                    if nested.is_empty() {
                        content.push(paragraph(c, ParagraphKind::Prose));
                    } else {
                        content.extend(nested);
                    }
                }
                blocks.push(Block::Details { summary, content });
            }
            _ => blocks.extend(parse_children(e, depth)),
        }
    }
    if !loose.is_empty() {
        blocks.push(Block::Paragraph {
            kind: ParagraphKind::Prose,
            runs: loose,
            anchor: None,
        });
    }
    blocks
}
fn parse_children_of_list(list: ElementRef<'_>, depth: usize) -> Vec<Block> {
    // Parse a temporary wrapper, allowing one shared list implementation.
    let html = Html::parse_fragment(&list.html());
    parse_children(html.root_element(), depth)
}
fn span(cell: ElementRef<'_>, name: &str) -> usize {
    cell.value()
        .attr(name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .clamp(1, 1000)
}
fn image_block(e: ElementRef<'_>) -> Block {
    Block::Image {
        source: e.value().attr("src").unwrap_or_default().to_owned(),
        alt: e.value().attr("alt").unwrap_or_default().to_owned(),
    }
}
fn text_blocks(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Paragraph { runs, .. } => {
                for r in runs {
                    out.push_str(&r.text);
                }
                out.push('\n');
            }
            Block::Table { caption, rows } => {
                for r in caption {
                    out.push_str(&r.text);
                }
                out.push('\n');
                for row in rows {
                    for cell in row {
                        text_blocks(&cell.content, out);
                    }
                }
            }
            Block::Details { summary, content } => {
                for r in summary {
                    out.push_str(&r.text);
                }
                out.push('\n');
                text_blocks(content, out);
            }
            Block::Image { .. } | Block::Rule => {}
        }
    }
}

/// PDFKit may read table columns in a different order from HTML rows. Prose must
/// remain exact and ordered; each complete cell must occur exactly once in its table.
pub fn verify_rendered_text(document: &RichDocument, rendered: &str) -> Result<(), String> {
    fn norm(value: &str) -> String {
        value
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    enum Part {
        Prose(String),
        Table(Vec<String>),
    }
    fn collect(blocks: &[Block], parts: &mut Vec<Part>) {
        for block in blocks {
            match block {
                Block::Paragraph { kind, runs, .. } => {
                    let mut text = String::new();
                    if let ParagraphKind::ListItem { marker, .. } = kind {
                        text.push_str(marker);
                    }
                    for run in runs {
                        text.push_str(&run.text);
                    }
                    let text = norm(&text);
                    if !text.is_empty() {
                        parts.push(Part::Prose(text));
                    }
                }
                Block::Table { caption, rows } => {
                    let caption =
                        norm(&caption.iter().map(|r| r.text.as_str()).collect::<String>());
                    if !caption.is_empty() {
                        parts.push(Part::Prose(caption));
                    }
                    let mut cells = Vec::new();
                    for row in rows {
                        for cell in row {
                            let mut text = String::new();
                            text_blocks(&cell.content, &mut text);
                            let value = norm(&text);
                            if !value.is_empty() {
                                cells.push(value);
                            }
                        }
                    }
                    parts.push(Part::Table(cells));
                }
                Block::Details { summary, content } => {
                    let summary =
                        norm(&summary.iter().map(|r| r.text.as_str()).collect::<String>());
                    if !summary.is_empty() {
                        parts.push(Part::Prose(summary));
                    }
                    collect(content, parts);
                }
                _ => {}
            }
        }
    }
    let mut parts = Vec::new();
    collect(&document.blocks, &mut parts);
    let actual = norm(rendered);
    let mut cursor = 0;
    for part in parts {
        match part {
            Part::Prose(expected) => {
                if !actual[cursor..].starts_with(&expected) {
                    return Err(format!("PDF prose differs at normalized offset {cursor}"));
                }
                cursor += expected.len();
            }
            Part::Table(mut cells) => {
                let length = cells.iter().map(String::len).sum::<usize>();
                let Some(region) = actual.get(cursor..cursor + length) else {
                    return Err("PDF table is incomplete".into());
                };
                let mut rest = region.to_owned();
                cells.sort_by_key(|cell| std::cmp::Reverse(cell.len()));
                for cell in cells {
                    let Some(offset) = rest.find(&cell) else {
                        return Err("PDF table cell is missing or split".into());
                    };
                    rest.replace_range(offset..offset + cell.len(), "");
                }
                if !rest.is_empty() {
                    return Err("PDF table contains extra text".into());
                }
                cursor += length;
            }
        }
    }
    if cursor != actual.len() {
        return Err("PDF contains duplicated or unexpected prose".into());
    }
    Ok(())
}

/// Apply allocation and dimension bounds before raster decoding.
fn decode_raster(bytes: &[u8]) -> Result<image::DynamicImage, image::ImageError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16000);
    limits.max_image_height = Some(16000);
    limits.max_alloc = Some(256_000_000);
    reader.limits(limits);
    reader.decode()
}

/// Raster bytes are supplied by the database owner, never fetched during display.
#[derive(Default)]
pub struct RichReader {
    pub document: Option<RichDocument>,
    textures: HashMap<String, TextureHandle>,
    pub image_errors: HashMap<String, String>,
}
impl RichReader {
    pub fn set_html(&mut self, html: &str) {
        self.document = Some(RichDocument::parse(html));
        self.textures.clear();
        self.image_errors.clear();
    }
    pub fn cache_image(
        &mut self,
        ctx: &egui::Context,
        source: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let image = decode_raster(bytes).map_err(|e| e.to_string())?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        if size[0] > 16000 || size[1] > 16000 || size[0].saturating_mul(size[1]) > 64_000_000 {
            return Err("Saved image exceeds decoder limits".into());
        }
        let texture = ctx.load_texture(
            source,
            egui::ColorImage::from_rgba_unmultiplied(size, &image),
            egui::TextureOptions::LINEAR,
        );
        self.textures.insert(source.into(), texture);
        Ok(())
    }
    pub fn show(&self, ui: &mut Ui, size: f32, font: ReaderFont) {
        if let Some(doc) = &self.document {
            show_blocks(ui, &doc.blocks, size, font, &self.textures);
        }
    }
}
fn show_runs(ui: &mut Ui, runs: &[Inline], size: f32, family: ReaderFont) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let mut links = Vec::new();
    let mut chars = 0;
    for run in runs {
        let font = FontId::new(
            if run.style.superscript || run.style.subscript {
                size * 0.72
            } else {
                size
            },
            if run.style.code {
                FontFamily::Monospace
            } else {
                FontFamily::Name(family.family().into())
            },
        );
        let mut format = egui::TextFormat {
            font_id: font,
            color: palette(ui.ctx()).ink,
            italics: run.style.emphasis,
            extra_letter_spacing: if run.style.strong { 0.18 } else { 0.0 },
            ..Default::default()
        };
        if run.style.underline || run.style.href.is_some() {
            format.underline = egui::Stroke::new(0.5, palette(ui.ctx()).green);
        }
        if run.style.strike {
            format.strikethrough = egui::Stroke::new(0.7, format.color);
        }
        let count = run.text.chars().count();
        if let Some(href) = &run.style.href {
            links.push((chars..chars + count, href));
        }
        chars += count;
        job.append(&run.text, 0.0, format);
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let response = ui.add(egui::Label::new(galley.clone()).selectable(true));
    if let Some(pos) = response.hover_pos() {
        let cursor = galley.cursor_from_pos(pos - response.rect.min);
        if let Some((_, href)) = links
            .iter()
            .find(|(range, _)| range.contains(&cursor.index.0))
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            response.clone().on_hover_text(href.as_str());
            if response.clicked()
                && url::Url::parse(href)
                    .is_ok_and(|u| matches!(u.scheme(), "https" | "http" | "mailto"))
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(href.as_str()));
            }
        }
    }
}
fn show_blocks(
    ui: &mut Ui,
    blocks: &[Block],
    size: f32,
    font: ReaderFont,
    textures: &HashMap<String, TextureHandle>,
) {
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Paragraph { kind, runs, .. } => {
                let point_size = match kind {
                    ParagraphKind::Heading(1) => 32.0,
                    ParagraphKind::Heading(2) => 27.0,
                    ParagraphKind::Heading(_) => 23.0,
                    ParagraphKind::Caption => 12.0,
                    ParagraphKind::Code => size * 0.85,
                    _ => size,
                };
                match kind {
                    ParagraphKind::ListItem { marker, depth } => {
                        ui.horizontal_top(|ui| {
                            ui.add_space((*depth as f32) * 16.0);
                            ui.label(
                                RichText::new(marker).font(FontId::new(
                                    size,
                                    FontFamily::Name(font.family().into()),
                                )),
                            );
                            show_runs(ui, runs, point_size, font);
                        });
                    }
                    ParagraphKind::Quote => {
                        ui.horizontal_top(|ui| {
                            ui.add_space(18.0);
                            show_runs(ui, runs, point_size, font);
                        });
                    }
                    ParagraphKind::Code => {
                        egui::Frame::new()
                            .fill(palette(ui.ctx()).sidebar)
                            .inner_margin(12.0)
                            .show(ui, |ui| show_runs(ui, runs, point_size, font));
                    }
                    _ => show_runs(ui, runs, point_size, font),
                }
                ui.add_space(if matches!(kind, ParagraphKind::Caption) {
                    10.0
                } else {
                    size * 0.65
                });
            }
            Block::Image { source, alt } => {
                if let Some(texture) = textures.get(source) {
                    let natural = texture.size_vec2();
                    let width = natural.x.min(ui.available_width());
                    let height = natural.y * width / natural.x;
                    ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::new(width, height)))
                        .on_hover_text(alt);
                } else {
                    ui.label(
                        RichText::new(format!("Saved image unavailable: {alt}"))
                            .color(palette(ui.ctx()).warning),
                    );
                }
                ui.add_space(10.0);
            }
            Block::Table { caption, rows } => {
                if !caption.is_empty() {
                    show_runs(ui, caption, 12.0, font);
                }
                let columns = rows
                    .iter()
                    .map(|row| row.iter().map(|cell| cell.colspan).sum::<usize>())
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let width = ui.available_width();
                let col_width = width / (columns as f32);
                // Merged cells retain their source span and receive the corresponding width.
                for row in rows {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for cell in row {
                            let cell_width = col_width * (cell.colspan as f32);
                            egui::Frame::new()
                                .fill(if cell.header {
                                    palette(ui.ctx()).selected
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(egui::Stroke::new(0.5, palette(ui.ctx()).divider))
                                .inner_margin(5.0)
                                .show(ui, |ui| {
                                    ui.allocate_ui_with_layout(
                                        Vec2::new((cell_width - 10.0).max(20.0), 0.0),
                                        Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            show_blocks(
                                                ui,
                                                &cell.content,
                                                size * 0.75,
                                                font,
                                                textures,
                                            )
                                        },
                                    );
                                });
                        }
                    });
                }
                ui.add_space(16.0);
            }
            Block::Rule => {
                ui.separator();
            }
            Block::Details { summary, content } => {
                let label = summary.iter().map(|r| r.text.as_str()).collect::<String>();
                egui::CollapsingHeader::new(
                    RichText::new(label)
                        .font(FontId::new(size, FontFamily::Name(font.family().into()))),
                )
                .id_salt(index)
                .show(ui, |ui| show_blocks(ui, content, size, font, textures));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_image_keeps_its_position_between_prose() {
        let document = RichDocument::parse(
            "<p>Before <a href='https://example.org/full'><img src='/api/media/x'></a> after.</p>",
        );
        assert!(matches!(&document.blocks[0],Block::Paragraph{runs,..}if runs[0].text=="Before "));
        assert!(matches!(&document.blocks[1],Block::Image{source,..}if source=="/api/media/x"));
        assert!(matches!(&document.blocks[2],Block::Paragraph{runs,..}if runs[0].text==" after."));
        assert_eq!(
            document.image_links.get("/api/media/x").map(String::as_str),
            Some("https://example.org/full")
        );
    }
    #[test]
    fn linked_cached_images_keep_their_layout() {
        let document = RichDocument::parse(
            "<a href='https://example.org/image'><img src='/api/media/linked' data-layout='data'></a>",
        );
        assert_eq!(document.image_sources(), vec!["/api/media/linked"]);
        assert_eq!(
            document
                .image_layouts
                .get("/api/media/linked")
                .map(String::as_str),
            Some("data")
        );
    }
    #[test]
    fn pdf_verifier_accepts_visual_cell_order_but_rejects_missing_and_duplicate_text() {
        let document = RichDocument::parse(
            "<p>Opening prose.</p><table><tr><td>Alpha cell</td><td>Beta cell</td></tr><tr><td>Gamma cell</td><td>Delta cell</td></tr></table><p>Final ending.</p>",
        );
        assert!(
            verify_rendered_text(
                &document,
                "Opening prose. Alpha cell Gamma cell Beta cell Delta cell Final ending."
            )
            .is_ok()
        );
        assert!(
            verify_rendered_text(
                &document,
                "Opening prose. Alpha cell Gamma cell Beta cell Final ending."
            )
            .is_err()
        );
        assert!(verify_rendered_text(&document,"Opening prose. Alpha cell Gamma cell Beta cell Delta cell Final ending. Final ending.").is_err());
    }
    #[test]
    fn rich_structures_and_unicode_survive() {
        let html = "<h2>Résumé</h2><p>A <strong>bold</strong> and <em>italic</em> sentence <a href='https://example.org'>source</a><sup>1</sup>.</p><figure><img src='/api/media/123' alt='Chart'><figcaption>Source chart</figcaption></figure><table><caption>Results</caption><tr><th colspan='2'>Group</th></tr><tr><td>α</td><td>β</td></tr></table><pre>let x = 1;\n  x + 2</pre><ol><li>First<ul><li>Nested</li></ul></li><li>Last</li></ol><p id='note'>Footnote text</p><script>bad()</script>";
        let doc = RichDocument::parse(html);
        assert_eq!(doc.image_sources(), vec!["/api/media/123"]);
        let text = doc.plain_text();
        for fragment in [
            "Résumé",
            "bold",
            "italic",
            "Results",
            "α",
            "β",
            "let x = 1;\n  x + 2",
            "Nested",
            "Footnote text",
        ] {
            assert!(text.contains(fragment), "missing {fragment}: {text}");
        }
        assert!(!text.contains("bad()"));
        assert_eq!(text.matches("Nested").count(), 1);
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b,Block::Table{rows,..} if rows[0][0].colspan==2))
        );
    }
    #[test]
    fn reader_appearance_changes_only_its_color_table() {
        let document = RichDocument::parse(
            "<h2>Heading</h2><p><strong>Bold</strong> <a href='https://example.org'>Link</a></p><table><tr><td>Cell</td></tr></table>",
        );
        let baseline = reader_rtf(&document, &HashMap::new(), 19.0, ReaderFont::Georgia).unwrap();
        let light = themed_reader_rtf(
            &document,
            &HashMap::new(),
            19.0,
            ReaderFont::Georgia,
            Appearance::Light,
        )
        .unwrap();
        let dark = themed_reader_rtf(
            &document,
            &HashMap::new(),
            19.0,
            ReaderFont::Georgia,
            Appearance::Dark,
        )
        .unwrap();
        assert_eq!(light, baseline);
        let light = String::from_utf8(light).unwrap();
        let dark = String::from_utf8(dark).unwrap();
        let table = |rtf: &str| {
            let start = rtf.find("{\\colortbl;").unwrap();
            let end = start + rtf[start..].find('}').unwrap() + 1;
            rtf[start..end].to_owned()
        };
        assert_ne!(table(&light), table(&dark));
        assert_eq!(dark.replacen(&table(&dark), &table(&light), 1), light);
        assert_eq!(
            reader_rtf(&document, &HashMap::new(), 19.0, ReaderFont::Georgia).unwrap(),
            baseline
        );
    }

    #[test]
    fn reader_marks_rules_for_native_drawing_without_changing_production_rtf() {
        let document = RichDocument::parse("<p>Before</p><div><hr></div><p>After</p>");
        assert!(document.blocks.contains(&Block::Rule));
        let production =
            String::from_utf8(native_rtf(&document, &HashMap::new(), 19., false).unwrap()).unwrap();
        assert!(production.contains(RTF_RULE));
        let reader = String::from_utf8(
            reader_rtf(&document, &HashMap::new(), 19., ReaderFont::Georgia).unwrap(),
        )
        .unwrap();
        assert!(!reader.contains(RTF_RULE));
        assert_eq!(
            reader
                .matches(&rule_marker(&document.source_sha256))
                .count(),
            1
        );
    }

    #[test]
    fn reader_font_changes_only_its_generated_font_entry() {
        let document = RichDocument::parse(
            "<h2>Heading Georgia</h2><p><strong>Bold</strong> <em>Italic</em> <a href='https://example.org/font'>Link</a> <code>Menlo code</code></p><table><tr><td>Cell</td></tr></table>",
        );
        let baseline =
            String::from_utf8(native_rtf(&document, &HashMap::new(), 19., false).unwrap()).unwrap();
        for font in ReaderFont::ALL {
            let rtf = String::from_utf8(reader_rtf(&document, &HashMap::new(), 19., font).unwrap())
                .unwrap();
            assert_eq!(
                rtf.replacen(
                    &format!("{{\\f0 {};}}", font.family()),
                    "{\\f0 Georgia;}",
                    1
                ),
                baseline
            );
            assert!(rtf.contains("{\\f1 Menlo;}"));
            assert!(rtf.contains("{\\f2 Helvetica;}"));
        }
    }
}

fn image_marker(document_sha256: &str, source: &str) -> String {
    format!(
        "RLIMAGE{document_sha256}{:x}",
        Sha256::digest(source.as_bytes())
    )
}

/// Generate native AppKit-readable rich text without invoking the platform HTML importer.
/// All content is serialized from the parsed structure and cached raster bytes.
pub fn native_rtf(
    document: &RichDocument,
    images: &HashMap<String, Vec<u8>>,
    body_size: f32,
    strict_images: bool,
) -> Result<Vec<u8>, String> {
    fn escaped(text: &str) -> String {
        let mut out = String::new();
        for unit in text.encode_utf16() {
            match unit {
                92 => out.push_str("\\\\"),
                123 => out.push_str("\\{"),
                125 => out.push_str("\\}"),
                10 => out.push_str("\\line "),
                9 => out.push_str("\\tab "),
                32..=126 => out.push(char::from_u32(unit as u32).expect("ASCII")),
                _ => out.push_str(&format!("\\u{}?", unit as i16)),
            }
        }
        out
    }
    fn inline(out: &mut String, runs: &[Inline]) {
        for r in runs {
            out.push('{');
            if r.style.strong {
                out.push_str("\\b ");
            }
            if r.style.emphasis {
                out.push_str("\\i ");
            }
            if r.style.underline {
                out.push_str("\\ul ");
            }
            if r.style.strike {
                out.push_str("\\strike ");
            }
            if r.style.code {
                out.push_str("\\f1 ");
            }
            if r.style.superscript {
                out.push_str("\\super ");
            }
            if r.style.subscript {
                out.push_str("\\sub ");
            }
            if let Some(href) = &r.style.href {
                if href.starts_with('#')
                    || url::Url::parse(href)
                        .is_ok_and(|u| matches!(u.scheme(), "https" | "http" | "mailto"))
                {
                    out.push_str("{\\field{\\*\\fldinst HYPERLINK \"");
                    out.push_str(&escaped(href));
                    out.push_str("\"}{\\fldrslt\\cf2\\ul ");
                    out.push_str(&escaped(&r.text));
                    out.push_str("}}");
                } else {
                    out.push_str(&escaped(&r.text));
                }
            } else {
                out.push_str(&escaped(&r.text));
            }
            out.push('}');
        }
    }
    fn blocks(
        out: &mut String,
        content: &[Block],
        images: &HashMap<String, Vec<u8>>,
        half_points: u32,
        strict: bool,
        in_table: bool,
        source_token: &str,
    ) -> Result<(), String> {
        for block in content {
            match block {
                Block::Paragraph { kind, runs, anchor } => {
                    out.push_str("{\\pard");
                    if in_table {
                        out.push_str("\\intbl");
                    }
                    out.push_str(&format!(
                        "\\f0\\fs{half_points}\\cf1\\sl{}\\slmult1\\sa{} ",
                        444,
                        half_points * 5
                    ));
                    match kind {
                        ParagraphKind::Heading(level) => out.push_str(&format!(
                            "\\b\\fs{}\\keepn ",
                            match level {
                                1 => 64,
                                2 => 54,
                                _ => 46,
                            }
                        )),
                        ParagraphKind::Code => out.push_str("\\f1\\fs28\\sl300\\slmult0 "),
                        ParagraphKind::Quote => out.push_str("\\li360\\i "),
                        ParagraphKind::Caption => out.push_str("\\f2\\fs24\\cf3 "),
                        ParagraphKind::ListItem { marker, depth } => {
                            out.push_str(&format!("\\li{}\\fi-240 ", (*depth + 1) * 360));
                            out.push_str(&escaped(marker));
                            out.push_str("\\tab ");
                        }
                        _ => {}
                    }
                    if let Some(anchor) = anchor {
                        out.push_str("{\\*\\bkmkstart ");
                        out.push_str(&escaped(anchor));
                        out.push('}');
                    }
                    inline(out, runs);
                    if let Some(anchor) = anchor {
                        out.push_str("{\\*\\bkmkend ");
                        out.push_str(&escaped(anchor));
                        out.push('}');
                    }
                    out.push_str("\\par}\n");
                }
                Block::Image { source, alt } => {
                    let bytes = images.get(source).or_else(|| {
                        source
                            .strip_prefix("/api/media/")
                            .and_then(|id| images.get(id))
                    });
                    if let Some(bytes) = bytes {
                        decode_raster(bytes)
                            .map_err(|e| format!("Saved raster could not decode: {e}"))?;
                        out.push_str("{\\pard\\qc ");
                        out.push_str(&image_marker(source_token, source));
                        out.push_str("\\par}\n");
                    } else if strict {
                        return Err(format!("Saved image is missing: {source}"));
                    } else {
                        out.push_str("{\\pard\\f2\\fs24\\cf3 ");
                        out.push_str(&escaped(&if alt.trim().is_empty() {
                            "Saved image unavailable".to_owned()
                        } else {
                            format!("Saved image unavailable: {alt}")
                        }));
                        out.push_str("\\par}\n");
                    }
                }
                Block::Table { caption, rows } => {
                    if !caption.is_empty() {
                        out.push_str("{\\pard\\f2\\fs24 ");
                        inline(out, caption);
                        out.push_str("\\par}\n");
                    }
                    let columns = rows
                        .iter()
                        .map(|r| r.iter().map(|c| c.colspan).sum::<usize>())
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    let colwidth = 9360 / columns;
                    let mut active_spans = HashMap::<usize, usize>::new();
                    for row in rows {
                        out.push_str("{\\trowd\\trgaph100\\trleft0 ");
                        let mut col = 0;
                        let mut occupied = Vec::new();
                        for cell in row {
                            while active_spans
                                .get(&col)
                                .is_some_and(|remaining| *remaining > 0)
                            {
                                out.push_str(&format!("\\clvmrg\\cellx{} ", (col + 1) * colwidth));
                                occupied.push(None);
                                col += 1;
                            }
                            for span_index in 0..cell.colspan {
                                if cell.colspan > 1 {
                                    out.push_str(if span_index == 0 {
                                        "\\clmgf"
                                    } else {
                                        "\\clmrg"
                                    });
                                }
                                if cell.rowspan > 1 {
                                    out.push_str("\\clvmgf");
                                    active_spans.insert(col, cell.rowspan);
                                }
                                out.push_str("\\clbrdrb\\brdrs\\brdrw5");
                                out.push_str(&format!("\\cellx{} ", (col + 1) * colwidth));
                                occupied.push(if span_index == 0 { Some(cell) } else { None });
                                col += 1;
                            }
                        }
                        for cell in occupied {
                            if let Some(cell) = cell {
                                if cell.header {
                                    out.push_str("\\b ");
                                }
                                blocks(
                                    out,
                                    &cell.content,
                                    images,
                                    (half_points * 3 / 4).max(18),
                                    strict,
                                    true,
                                    source_token,
                                )?;
                            }
                            out.push_str("\\intbl\\cell ");
                        }
                        out.push_str("\\intbl\\row}\n");
                        for remaining in active_spans.values_mut() {
                            *remaining = remaining.saturating_sub(1);
                        }
                    }
                }
                Block::Rule => {
                    out.push_str(RTF_RULE);
                    out.push('\n');
                }
                Block::Details { summary, content } => {
                    out.push_str("{\\pard\\b ");
                    inline(out, summary);
                    out.push_str("\\par}\n");
                    blocks(
                        out,
                        content,
                        images,
                        half_points,
                        strict,
                        in_table,
                        source_token,
                    )?;
                }
            }
        }
        Ok(())
    }
    let mut out = String::from(
        "{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1{\\fonttbl{\\f0 Georgia;}{\\f1 Menlo;}{\\f2 Helvetica;}}{\\colortbl;\\red44\\green48\\blue44;\\red49\\green84\\blue68;\\red133\\green135\\blue127;}\\viewkind4\n",
    );
    blocks(
        &mut out,
        &document.blocks,
        images,
        (body_size * 2.0).round() as u32,
        strict_images,
        false,
        &document.source_sha256,
    )?;
    out.push('}');
    Ok(out.into_bytes())
}

/// Reading preferences affect only this generated font-table entry. Magazine
/// rendering continues to use `native_rtf` and its independent typography.
pub fn reader_rtf(
    document: &RichDocument,
    images: &HashMap<String, Vec<u8>>,
    body_size: f32,
    font: ReaderFont,
) -> Result<Vec<u8>, String> {
    let rtf = String::from_utf8(native_rtf(document, images, body_size, false)?)
        .map_err(|error| error.to_string())?;
    Ok(rtf
        .replacen(
            "{\\f0 Georgia;}",
            &format!("{{\\f0 {};}}", font.family()),
            1,
        )
        // AppKit's RTF reader ignores paragraph borders, so the reader draws
        // horizontal rules as native text blocks at these private markers.
        .replace(
            RTF_RULE,
            &format!(
                "{{\\pard\\fs2 {}\\par}}",
                rule_marker(&document.source_sha256)
            ),
        )
        .into_bytes())
}
const RTF_RULE: &str = "{\\pard\\brdrb\\brdrs\\brdrw10\\par}";
fn rule_marker(document_sha256: &str) -> String {
    format!("RLRULE{document_sha256}")
}

/// Reader appearance changes only the color table, leaving production RTF untouched.
fn themed_reader_rtf(
    document: &RichDocument,
    images: &HashMap<String, Vec<u8>>,
    body_size: f32,
    font: ReaderFont,
    appearance: Appearance,
) -> Result<Vec<u8>, String> {
    let rtf = String::from_utf8(reader_rtf(document, images, body_size, font)?)
        .map_err(|error| error.to_string())?;
    let colors = appearance.palette();
    let color_table = |colors: crate::theme::Palette| {
        let mut table = String::from("{\\colortbl;");
        for color in [colors.ink, colors.green, colors.muted] {
            table.push_str(&format!(
                "\\red{}\\green{}\\blue{};",
                color.r(),
                color.g(),
                color.b()
            ));
        }
        table.push('}');
        table
    };
    Ok(rtf
        .replacen(
            &color_table(Appearance::Light.palette()),
            &color_table(colors),
            1,
        )
        .into_bytes())
}

#[cfg(target_os = "macos")]
pub mod native {
    use super::{Appearance, ReaderFont, RichDocument, native_rtf, reader_rtf, themed_reader_rtf};
    use objc2::{
        AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, ClassBuilder, ProtocolObject, Sel},
        sel,
    };
    use objc2_app_kit::{
        NSAccessibility, NSAppearance, NSAppearanceCustomization, NSAttachmentAttributeName,
        NSAttributedStringAppKitDocumentFormats, NSAttributedStringAttachmentConveniences,
        NSBackgroundColorAttributeName, NSBezelStyle, NSButton, NSColor, NSControlSize, NSFont,
        NSForegroundColorAttributeName, NSImage, NSLinkAttributeName, NSObjectNSAccessibility,
        NSParagraphStyle, NSParagraphStyleAttributeName, NSPopUpButton, NSScrollView,
        NSTextAttachment, NSTextBlockDimension, NSTextBlockValueType, NSTextDelegate,
        NSTextTableBlock, NSTextView, NSTextViewDelegate, NSView,
    };
    use objc2_foundation::{
        MainThreadMarker, NSArray, NSAttributedString, NSData, NSDictionary,
        NSMutableAttributedString, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize,
        NSString, NSURL,
    };
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        ffi::{CString, c_void},
        ptr::NonNull,
        rc::Rc,
        sync::Arc,
    };
    // Winit requires its own view to remain NSWindow.contentView. AccessKit
    // supplies that view's virtual accessibility tree, so merge the native
    // reader into that tree instead of inserting a replacement content view.
    thread_local! {
        static NATIVE_SCROLLS: RefCell<HashMap<usize, Retained<NSScrollView>>> = RefCell::new(HashMap::new());
        static NATIVE_PICKERS: RefCell<HashMap<usize, Retained<NSPopUpButton>>> = RefCell::new(HashMap::new());
    }
    fn native_scroll(view: &AnyObject) -> Option<Retained<NSScrollView>> {
        NATIVE_SCROLLS.with(|views| {
            views
                .borrow()
                .get(&(view as *const AnyObject as usize))
                .cloned()
        })
    }
    fn native_children(view: &AnyObject) -> Vec<Retained<NSView>> {
        let mut children = Vec::new();
        if let Some(scroll) = native_scroll(view).filter(|scroll| !scroll.isHidden()) {
            let view: &NSView = &scroll;
            children.push(view.retain());
        }
        NATIVE_PICKERS.with(|pickers| {
            if let Some(picker) = pickers.borrow().get(&(view as *const AnyObject as usize))
                && !picker.isHidden()
            {
                let view: &NSView = picker;
                children.push(view.retain());
            }
        });
        children
    }
    extern "C-unwind" fn composite_children(view: &AnyObject, _: Sel) -> *mut NSArray<AnyObject> {
        let superclass = view
            .class()
            .superclass()
            .expect("Composite view superclass");
        let original: Option<Retained<NSArray<AnyObject>>> =
            unsafe { msg_send![super(view, superclass), accessibilityChildren] };
        let mut children = original
            .map(|children| children.to_vec())
            .unwrap_or_default();
        for child in native_children(view) {
            // Upcast the retained native view to the heterogeneous AX array.
            children.push(child.into_super().into_super().into_super());
        }
        Retained::autorelease_return(NSArray::from_retained_slice(&children))
    }
    extern "C-unwind" fn composite_hit_test(
        view: &AnyObject,
        _: Sel,
        point: NSPoint,
    ) -> *mut AnyObject {
        for child in native_children(view) {
            let Some(window) = child.window() else {
                continue;
            };
            let local = child.convertPoint_fromView(window.convertPointFromScreen(point), None);
            let bounds = child.bounds();
            if local.x >= bounds.origin.x
                && local.y >= bounds.origin.y
                && local.x < bounds.origin.x + bounds.size.width
                && local.y < bounds.origin.y + bounds.size.height
                && let Some(found) = child.accessibilityHitTest(point)
            {
                return Retained::autorelease_return(found);
            }
        }
        let superclass = view
            .class()
            .superclass()
            .expect("Composite view superclass");
        let result: Option<Retained<AnyObject>> =
            unsafe { msg_send![super(view, superclass), accessibilityHitTest:point] };
        result
            .map(Retained::autorelease_return)
            .unwrap_or(std::ptr::null_mut())
    }
    extern "C-unwind" fn composite_focus(view: &AnyObject, _: Sel) -> *mut AnyObject {
        for child in native_children(view) {
            if let Some(responder) = child.window().and_then(|window| window.firstResponder())
                && let Some(focused) = responder.downcast_ref::<NSView>()
                && focused.isDescendantOf(&child)
            {
                return Retained::autorelease_return(responder.into_super().into_super());
            }
        }
        let superclass = view
            .class()
            .superclass()
            .expect("Composite view superclass");
        let result: Option<Retained<AnyObject>> =
            unsafe { msg_send![super(view, superclass), accessibilityFocusedUIElement] };
        result
            .map(Retained::autorelease_return)
            .unwrap_or(std::ptr::null_mut())
    }
    fn attach_accessibility(parent: &NSView, scroll: &NSScrollView) -> Result<(), String> {
        if native_scroll(parent).is_some() {
            return Err("Native reader is already attached to this view".into());
        }
        let original = parent.class();
        // Dropping the reader removes its registry entry, but the no-ivar
        // subclass remains installed. Reuse it: wrapping it again would make
        // the superclass callback recurse into another composite callback.
        if original
            .name()
            .to_bytes()
            .starts_with(b"ReadingListComposite_")
        {
            NATIVE_SCROLLS.with(|views| {
                views
                    .borrow_mut()
                    .insert(parent as *const NSView as usize, scroll.retain());
            });
            return Ok(());
        }
        let name = CString::new(format!(
            "ReadingListComposite_{}",
            original.name().to_string_lossy()
        ))
        .map_err(|_| "Invalid native view class name")?;
        let class = if let Some(class) = AnyClass::get(&name) {
            class
        } else {
            let mut builder = ClassBuilder::new(&name, original)
                .ok_or("Native accessibility class already exists")?;
            // No ivars are added: Winit and AccessKit retain their exact layout.
            unsafe {
                builder.add_method(
                    sel!(accessibilityChildren),
                    composite_children as extern "C-unwind" fn(_, _) -> _,
                );
                builder.add_method(
                    sel!(accessibilityHitTest:),
                    composite_hit_test as extern "C-unwind" fn(_, _, _) -> _,
                );
                builder.add_method(
                    sel!(accessibilityFocusedUIElement),
                    composite_focus as extern "C-unwind" fn(_, _) -> _,
                );
            }
            builder.register()
        };
        if class.instance_size() != original.instance_size() {
            return Err("Native accessibility must preserve Winit view layout".into());
        }
        NATIVE_SCROLLS.with(|views| {
            views
                .borrow_mut()
                .insert(parent as *const NSView as usize, scroll.retain());
        });
        // Both class layouts match, and the new class inherits all original
        // Winit/AccessKit behavior. This follows AccessKit's own subclass model.
        unsafe {
            objc2::ffi::object_setClass((parent as *const NSView).cast_mut().cast(), class);
        }
        Ok(())
    }
    /// Import text/table styles through RTF, then insert raster attachments directly.
    /// AppKit ignores standard RTF pict groups, so image preservation is explicit.
    pub fn attributed_document(
        document: &RichDocument,
        images: &HashMap<String, Vec<u8>>,
        body_size: f32,
        strict_images: bool,
        max_image_width: f64,
    ) -> Result<Retained<NSAttributedString>, String> {
        let rtf = native_rtf(document, images, body_size, strict_images)?;
        attributed_from_rtf(document, images, &rtf, max_image_width, 1_000_000.0)
    }
    pub fn attributed_from_rtf(
        document: &RichDocument,
        images: &HashMap<String, Vec<u8>>,
        rtf: &[u8],
        max_image_width: f64,
        max_image_height: f64,
    ) -> Result<Retained<NSAttributedString>, String> {
        attributed_from_rtf_with_image_limits(
            document,
            images,
            rtf,
            max_image_width,
            max_image_height,
            max_image_height,
        )
    }
    pub fn attributed_from_rtf_with_image_limits(
        document: &RichDocument,
        images: &HashMap<String, Vec<u8>>,
        rtf: &[u8],
        max_image_width: f64,
        illustration_height: f64,
        data_height: f64,
    ) -> Result<Retained<NSAttributedString>, String> {
        let data = NSData::with_bytes(rtf);
        let decoded = unsafe {
            NSAttributedString::initWithRTF_documentAttributes(
                NSAttributedString::alloc(),
                &data,
                None,
            )
        }
        .ok_or("Native rich-text decoder rejected saved content")?;
        let output = NSMutableAttributedString::initWithAttributedString(
            NSMutableAttributedString::alloc(),
            &decoded,
        );
        for source in document.image_sources() {
            let Some(bytes) = images.get(source).or_else(|| {
                source
                    .strip_prefix("/api/media/")
                    .and_then(|id| images.get(id))
            }) else {
                continue;
            };
            let raster = super::decode_raster(bytes).map_err(|e| e.to_string())?;
            let marker = NSString::from_str(&super::image_marker(&document.source_sha256, source));
            let range = output.string().rangeOfString(&marker);
            if range.location == usize::MAX {
                return Err("Native decoder lost a saved image anchor".into());
            }
            let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes))
                .ok_or("AppKit could not decode a cached image")?;
            let max_image_height = if document
                .image_layouts
                .get(source)
                .is_some_and(|layout| layout == "data")
            {
                data_height
            } else {
                illustration_height
            };
            let scale = (max_image_width / f64::from(raster.width()))
                .min(max_image_height / f64::from(raster.height()))
                .min(1.0);
            let width = f64::from(raster.width()) * scale;
            let height = f64::from(raster.height()) * scale;
            image.setSize(NSSize::new(width, height));
            let attachment = NSTextAttachment::new();
            attachment.setImage(Some(&image));
            attachment.setBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, height),
            ));
            let replacement = NSAttributedString::attributedStringWithAttachment(&attachment);
            output.replaceCharactersInRange_withAttributedString(range, &replacement);
            if let Some(href) = document.image_links.get(source).filter(|href| {
                url::Url::parse(href).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            }) {
                unsafe {
                    output.addAttribute_value_range(
                        NSLinkAttributeName,
                        &NSString::from_str(href),
                        NSRange::new(range.location, 1),
                    );
                }
            }
        }
        Ok(output.into_super())
    }

    pub fn anchor_ranges(document: &RichDocument, text: &NSString) -> HashMap<String, NSRange> {
        fn collect(blocks: &[super::Block], out: &mut Vec<(String, String)>) {
            for block in blocks {
                match block {
                    super::Block::Paragraph {
                        anchor: Some(anchor),
                        runs,
                        ..
                    } => out.push((
                        anchor.clone(),
                        runs.iter().map(|r| r.text.as_str()).collect::<String>(),
                    )),
                    super::Block::Table { rows, .. } => {
                        for row in rows {
                            for cell in row {
                                collect(&cell.content, out);
                            }
                        }
                    }
                    super::Block::Details { content, .. } => collect(content, out),
                    _ => {}
                }
            }
        }
        let mut anchors = Vec::new();
        collect(&document.blocks, &mut anchors);
        let mut ranges = HashMap::new();
        for (anchor, value) in anchors {
            if !value.is_empty() {
                let needle = NSString::from_str(&value);
                let range = text.rangeOfString(&needle);
                if range.location != usize::MAX {
                    ranges.insert(anchor, range);
                }
            }
        }
        ranges
    }
    pub struct ReaderDelegateIvars {
        anchors: RefCell<HashMap<String, NSRange>>,
    }
    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind=MainThreadOnly]
        #[ivars=ReaderDelegateIvars]
        #[name = "ReadingListRichTextDelegate"]
        pub struct ReaderDelegate;
        unsafe impl NSObjectProtocol for ReaderDelegate {}
        unsafe impl NSTextDelegate for ReaderDelegate {}
        unsafe impl NSTextViewDelegate for ReaderDelegate {
            #[unsafe(method(textView:clickedOnLink:atIndex:))]
            fn clicked(&self, text: &NSTextView, link: &AnyObject, _index: usize) -> bool {
                self.follow(text, link)
            }
        }
    );
    impl ReaderDelegate {
        fn follow(&self, text: &NSTextView, link: &AnyObject) -> bool {
            let value = if let Some(value) = link.downcast_ref::<NSString>() {
                value.to_string()
            } else if let Some(value) = link.downcast_ref::<NSURL>() {
                value.relativeString().to_string()
            } else {
                return false;
            };
            let Some(anchor) = value.strip_prefix('#') else {
                return false;
            };
            if let Some(range) = self.ivars().anchors.borrow().get(anchor) {
                text.scrollRangeToVisible(*range);
                text.setSelectedRange(NSRange::new(range.location, 0));
            }
            true
        }
    }
    pub fn install_reader_delegate(
        text: &NSTextView,
        document: &RichDocument,
    ) -> Result<Retained<ReaderDelegate>, String> {
        let mtm = MainThreadMarker::new().ok_or("Reader delegate requires main thread")?;
        let delegate = ReaderDelegate::alloc(mtm).set_ivars(ReaderDelegateIvars {
            anchors: RefCell::new(anchor_ranges(document, &text.string())),
        });
        let delegate: Retained<ReaderDelegate> = unsafe { msg_send![super(delegate), init] };
        text.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Ok(delegate)
    }
    #[derive(Clone, Debug)]
    pub struct ReaderHeader {
        pub article_id: crate::domain::ArticleId,
        pub publication: String,
        pub title: String,
        pub metadata: String,
        pub read: bool,
        pub archived: bool,
        pub notice: Option<ReaderNotice>,
        pub fetch_info: Option<String>,
    }
    #[derive(Clone, Debug)]
    pub enum ReaderNotice {
        Info(String),
        Warning(String),
        Recovery {
            title: String,
            summary: String,
            details: String,
        },
    }
    #[derive(Clone, Debug)]
    pub enum ReaderActionKind {
        ToggleRead(bool),
        Archive(bool),
        Delete,
        Refetch,
        Details { title: String, message: String },
    }
    #[derive(Clone, Debug)]
    pub struct ReaderAction {
        pub article_id: crate::domain::ArticleId,
        pub kind: ReaderActionKind,
    }
    struct ReaderBridge {
        header: RefCell<Option<ReaderHeader>>,
        actions: RefCell<Vec<ReaderAction>>,
        wake: RefCell<Option<Arc<dyn Fn() + Send + Sync>>>,
        font_choice: Cell<Option<ReaderFont>>,
    }
    struct HeaderTargetIvars {
        bridge: Rc<ReaderBridge>,
    }
    define_class!(
        #[unsafe(super(NSObject))] #[thread_kind=MainThreadOnly] #[ivars=HeaderTargetIvars] #[name="ReadingListReaderButtonTarget"]
        struct HeaderTarget;
        impl HeaderTarget{
            #[unsafe(method(readerAction:))]fn reader_action(&self,sender:&NSButton){let bridge=&self.ivars().bridge;if let Some(header)=bridge.header.borrow().as_ref(){let kind=match sender.tag(){0=>ReaderActionKind::ToggleRead(!header.read),1=>ReaderActionKind::Archive(!header.archived),2=>ReaderActionKind::Delete,3=>ReaderActionKind::Refetch,4=>match &header.notice { Some(ReaderNotice::Recovery {title,details,..})=>ReaderActionKind::Details {title:title.clone(),message:details.clone()},_=>return },_=>return};let mut actions=bridge.actions.borrow_mut();if actions.len()<32{actions.push(ReaderAction{article_id:header.article_id.clone(),kind});if let Some(wake)=bridge.wake.borrow().as_ref(){wake();}}}}
            #[unsafe(method(readerFontChoice:))]
            fn reader_font_choice(&self, sender: &NSPopUpButton) {
                let font = match sender.indexOfSelectedItem() {
                    1 => ReaderFont::Georgia,
                    2 => ReaderFont::Baskerville,
                    4 => ReaderFont::AvenirNext,
                    5 => ReaderFont::HelveticaNeue,
                    _ => return,
                };
                let bridge = &self.ivars().bridge;
                bridge.font_choice.set(Some(font));
                if let Some(wake) = bridge.wake.borrow().as_ref() { wake(); }
            }
        }
    );
    fn font_picker(mtm: MainThreadMarker, target: &HeaderTarget) -> Retained<NSPopUpButton> {
        let font_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(134.0, 28.0)),
            false,
        );
        for label in [
            "Classic",
            "Georgia",
            "Baskerville",
            "Modern",
            "Avenir Next",
            "Helvetica Neue",
        ] {
            font_picker.addItemWithTitle(&NSString::from_str(label));
        }
        if let Some(menu) = font_picker.menu() {
            menu.setAutoenablesItems(false);
        }
        for index in [0, 3] {
            if let Some(item) = font_picker.itemAtIndex(index) {
                item.setEnabled(false);
            }
        }
        font_picker.selectItemAtIndex(1);
        font_picker.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        font_picker.setControlSize(NSControlSize::Small);
        font_picker.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        font_picker.setContentTintColor(Some(&native_color(Appearance::Light.palette().ink)));
        unsafe {
            font_picker.setTarget(Some(target));
            font_picker.setAction(Some(sel!(readerFontChoice:)));
        }
        font_picker.setAccessibilityLabel(Some(&NSString::from_str("Reader font")));
        font_picker.setToolTip(Some(&NSString::from_str(
            "Choose a classic or modern reading font",
        )));
        font_picker.setHidden(true);
        font_picker
    }
    struct ReaderDocumentIvars {
        appearance: Cell<Appearance>,
        divider: Cell<f64>,
        inset: Cell<f64>,
    }
    define_class!(
        #[unsafe(super(NSView))] #[thread_kind=MainThreadOnly] #[ivars=ReaderDocumentIvars] #[name="ReadingListReaderScrollDocument"]
        struct ReaderDocumentView;
        impl ReaderDocumentView{
            #[unsafe(method(isFlipped))]fn is_flipped(&self)->bool{true}
            #[unsafe(method(drawRect:))]fn draw_rect(&self,_dirty:NSRect){let colors=self.ivars().appearance.get().palette();native_color(colors.reader).setFill();objc2_app_kit::NSRectFill(self.bounds());let y=self.ivars().divider.get();if y>0.0{native_color(colors.divider).setFill();let inset=self.ivars().inset.get();objc2_app_kit::NSRectFill(NSRect::new(NSPoint::new(inset,y),NSSize::new((self.bounds().size.width-inset*2.0).max(1.0),1.0)));}}
        }
    );
    fn configure_text(text: &NSTextView) {
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(true);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setMinSize(NSSize::new(0.0, 0.0));
        text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        text.setTextContainerInset(NSSize::new(0.0, 0.0));
        configure_text_appearance(text, Appearance::Light);
    }
    /// The reader body's attributed text, with screen typography and native blocks.
    pub fn reader_document(
        document: &RichDocument,
        images: &HashMap<String, Vec<u8>>,
        body_size: f32,
        font: ReaderFont,
        appearance: Appearance,
    ) -> Result<Retained<NSAttributedString>, String> {
        let half_points = (body_size * 2.0).round() as u32;
        let mut rtf = String::from_utf8(themed_reader_rtf(
            document, images, body_size, font, appearance,
        )?)
        .map_err(|e| e.to_string())?
        // 1.6 line height for screen reading; magazine RTF keeps its own spacing.
        .replace("\\sl444\\slmult1", "\\sl384\\slmult1")
        .replace(
            &format!("\\sa{}", half_points * 5),
            &format!("\\sa{}", (body_size * 1.3 * 20.0).round() as u32),
        )
        .replace("\\f2\\fs24\\cf3", "\\f2\\fs26\\cf3\\sl360\\slmult1\\sa160")
        // Quotes are set upright beside a native bar, as publishers style them.
        .replace("\\li360\\i ", "\\li360 ")
        // Images otherwise butt against the following paragraph.
        .replace(
            "{\\pard\\qc ",
            &format!(
                "{{\\pard\\qc\\sa{} ",
                (body_size * 0.8 * 20.0).round() as u32
            ),
        );
        // Keep the heading hierarchy: a section heading (h3-h6) must not
        // match the size of a top-level one.
        for (heading, scale, before) in [(64, 1.6, 1.5), (54, 1.35, 1.4), (46, 1.15, 1.25)] {
            rtf = rtf.replace(
                &format!("\\b\\fs{heading}\\keepn"),
                &format!(
                    "\\b\\fs{}\\sl300\\slmult1\\sb{}\\sa{}\\keepn",
                    (body_size * 2.0 * scale).round() as u32,
                    (body_size * before * 20.0).round() as u32,
                    (body_size * 0.6 * 20.0).round() as u32
                ),
            );
        }
        reader_attributed_from_rtf(document, images, rtf.as_bytes(), appearance)
    }
    /// AppKit decodes RTF colors as calibrated RGB. Restore exact shared sRGB
    /// tokens for the reader after import; production/PDF decoding stays independent.
    fn reader_attributed_from_rtf(
        document: &RichDocument,
        images: &HashMap<String, Vec<u8>>,
        rtf: &[u8],
        appearance: Appearance,
    ) -> Result<Retained<NSAttributedString>, String> {
        use objc2_app_kit::NSColorSpace;
        let decoded = attributed_from_rtf(document, images, rtf, 780.0, 1_000_000.0)?;
        let output = NSMutableAttributedString::initWithAttributedString(
            NSMutableAttributedString::alloc(),
            &decoded,
        );
        let colors = appearance.palette();
        let mut index = 0;
        while index < output.length() {
            let mut range = NSRange::new(index, 1);
            if let Some(value) = unsafe {
                output.attribute_atIndex_effectiveRange(
                    NSForegroundColorAttributeName,
                    index,
                    &mut range,
                )
            } && let Some(color) = value.downcast_ref::<NSColor>()
                && let Some(rgb) = color.colorUsingColorSpace(&NSColorSpace::genericRGBColorSpace())
            {
                for token in [colors.ink, colors.green, colors.muted] {
                    if [
                        rgb.redComponent(),
                        rgb.greenComponent(),
                        rgb.blueComponent(),
                    ]
                    .into_iter()
                    .zip([token.r(), token.g(), token.b()])
                    .all(|(actual, expected)| {
                        (actual - f64::from(expected) / 255.0).abs() < 0.5 / 255.0
                    }) {
                        unsafe {
                            output.addAttribute_value_range(
                                NSForegroundColorAttributeName,
                                &native_color(token),
                                range,
                            );
                        }
                        break;
                    }
                }
            }
            index = range.location + range.length.max(1);
        }
        reader_text_blocks(
            &output,
            &super::rule_marker(&document.source_sha256),
            colors,
        );
        Ok(output.into_super())
    }
    /// Draw rules and quote bars as native text blocks; RTF import cannot express them.
    fn reader_text_blocks(
        output: &NSMutableAttributedString,
        rule_marker: &str,
        colors: crate::theme::Palette,
    ) {
        use objc2_app_kit::{NSMutableParagraphStyle, NSTextBlock, NSTextBlockLayer};
        use objc2_foundation::{NSMutableCopying, NSRectEdge};
        let absolute = NSTextBlockValueType::AbsoluteValueType;
        let marker = NSString::from_str(rule_marker);
        loop {
            let range = output.string().rangeOfString(&marker);
            // NSNotFound is NSIntegerMax, so test the empty range instead.
            if range.length == 0 {
                break;
            }
            output.replaceCharactersInRange_withString(range, &NSString::from_str("\u{a0}"));
            let paragraph = output
                .string()
                .paragraphRangeForRange(NSRange::new(range.location, 1));
            let block = NSTextBlock::new();
            block.setValue_type_forDimension(
                100.0,
                NSTextBlockValueType::PercentageValueType,
                NSTextBlockDimension::Width,
            );
            block.setWidth_type_forLayer_edge(
                1.0,
                absolute,
                NSTextBlockLayer::Border,
                NSRectEdge::MaxY,
            );
            block.setBorderColor(Some(&native_color(colors.divider)));
            block.setWidth_type_forLayer_edge(
                4.0,
                absolute,
                NSTextBlockLayer::Margin,
                NSRectEdge::MinY,
            );
            block.setWidth_type_forLayer_edge(
                28.0,
                absolute,
                NSTextBlockLayer::Margin,
                NSRectEdge::MaxY,
            );
            let style = NSMutableParagraphStyle::new();
            style.setTextBlocks(&NSArray::from_retained_slice(&[block]));
            unsafe {
                output.addAttribute_value_range(NSParagraphStyleAttributeName, &style, paragraph);
            }
        }
        // Quote paragraphs are the only ones with an equal 18 pt head and first-line
        // indent; list items hang their marker. Consecutive quotes share one bar.
        let text = output.string();
        let mut index = 0;
        let mut bar: Option<Retained<NSTextBlock>> = None;
        while index < text.length() {
            let paragraph = text.paragraphRangeForRange(NSRange::new(index, 0));
            let style = unsafe {
                output.attribute_atIndex_effectiveRange(
                    NSParagraphStyleAttributeName,
                    paragraph.location,
                    std::ptr::null_mut(),
                )
            };
            let style = style
                .as_deref()
                .and_then(|value| value.downcast_ref::<NSParagraphStyle>())
                .filter(|style| {
                    style.textBlocks().is_empty()
                        && (style.headIndent() - 18.0).abs() < 0.01
                        && (style.firstLineHeadIndent() - 18.0).abs() < 0.01
                });
            if let Some(style) = style {
                let block = bar
                    .get_or_insert_with(|| {
                        let block = NSTextBlock::new();
                        block.setValue_type_forDimension(
                            100.0,
                            NSTextBlockValueType::PercentageValueType,
                            NSTextBlockDimension::Width,
                        );
                        block.setWidth_type_forLayer_edge(
                            3.0,
                            absolute,
                            NSTextBlockLayer::Border,
                            NSRectEdge::MinX,
                        );
                        block.setBorderColor(Some(&native_color(colors.accent)));
                        block.setWidth_type_forLayer_edge(
                            18.0,
                            absolute,
                            NSTextBlockLayer::Padding,
                            NSRectEdge::MinX,
                        );
                        block
                    })
                    .clone();
                let quoted: Retained<NSMutableParagraphStyle> = style.mutableCopy();
                quoted.setHeadIndent(0.0);
                quoted.setFirstLineHeadIndent(0.0);
                quoted.setTextBlocks(&NSArray::from_retained_slice(&[block]));
                unsafe {
                    output.addAttribute_value_range(
                        NSParagraphStyleAttributeName,
                        &quoted,
                        paragraph,
                    );
                }
            } else {
                bar = None;
            }
            index = paragraph.location + paragraph.length.max(1);
        }
    }
    fn native_color(color: egui::Color32) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(color.r()) / 255.0,
            f64::from(color.g()) / 255.0,
            f64::from(color.b()) / 255.0,
            1.0,
        )
    }
    fn configure_text_appearance(text: &NSTextView, appearance: Appearance) {
        let colors = appearance.palette();
        text.setBackgroundColor(&native_color(colors.reader));
        text.setInsertionPointColor(Some(&native_color(colors.ink)));
        let green = native_color(colors.green);
        let values: [&AnyObject; 1] = [&*green];
        let attributes =
            NSDictionary::from_slices(&[unsafe { NSForegroundColorAttributeName }], &values);
        let ink = native_color(colors.ink);
        let selected = native_color(colors.selected);
        let selection_values: [&AnyObject; 2] = [&*ink, &*selected];
        let selection = NSDictionary::from_slices(
            &[unsafe { NSForegroundColorAttributeName }, unsafe {
                NSBackgroundColorAttributeName
            }],
            &selection_values,
        );
        unsafe {
            text.setLinkTextAttributes(Some(&attributes));
            text.setSelectedTextAttributes(&selection);
        }
    }
    fn layout_text(text: &NSTextView, width: f64) -> f64 {
        text.setFrameSize(NSSize::new(width, 1.0));
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(false);
            container.setLineFragmentPadding(0.0);
            container.setContainerSize(NSSize::new(width, f64::MAX));
            if let Some(manager) = unsafe { text.layoutManager() } {
                manager.ensureLayoutForTextContainer(&container);
                let height = manager.usedRectForTextContainer(&container).size.height + 2.0;
                text.setFrameSize(NSSize::new(width, height));
                return height;
            }
        }
        2.0
    }
    fn replace_text(
        text: &NSTextView,
        attributed: &NSAttributedString,
        preserve_selection: bool,
    ) -> Result<(), String> {
        let selection = (preserve_selection && text.string() == attributed.string())
            .then(|| text.selectedRange());
        unsafe { text.textStorage() }
            .ok_or("Native reader text storage unavailable")?
            .setAttributedString(attributed);
        if let Some(range) = selection {
            let start = range.location.min(attributed.length());
            text.setSelectedRange(NSRange::new(
                start,
                range.length.min(attributed.length() - start),
            ));
        }
        Ok(())
    }

    /// Focused AppKit appearance check. It opens no window and touches no library data.
    pub fn verify_reader_appearance() -> Result<serde_json::Value, String> {
        use objc2_app_kit::{NSApplication, NSColorSpace};
        let mtm = MainThreadMarker::new().ok_or("Reader theme proof requires the main thread")?;
        let _application = NSApplication::sharedApplication(mtm);
        let document = RichDocument::parse(
            "<h2>HeadingToken</h2><p>BodyToken <strong>BoldToken</strong> <a href='https://example.org/source'>LinkToken</a></p><figcaption>CaptionToken</figcaption><table><tr><td>CellToken</td></tr></table>",
        );
        let images = HashMap::new();
        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(780.0, 900.0)),
        );
        configure_text(&text_view);
        let check_color =
            |label: &str, color: &NSColor, expected: egui::Color32| -> Result<(), String> {
                let rgb = color
                    .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
                    .ok_or_else(|| format!("{label} has no RGB color"))?;
                // Reader colors are explicitly restored to the palette's sRGB space.
                for (actual, channel) in [
                    rgb.redComponent(),
                    rgb.greenComponent(),
                    rgb.blueComponent(),
                ]
                .into_iter()
                .zip([expected.r(), expected.g(), expected.b()])
                {
                    if (actual - f64::from(channel) / 255.0).abs() > 1.0 / 255.0 {
                        return Err(format!(
                            "{label} channel {actual:.6} differs from expected {}",
                            f64::from(channel) / 255.0
                        ));
                    }
                }
                Ok(())
            };
        let mut baseline = None;
        let selection = NSRange::new(2, 5);
        for appearance in [Appearance::Light, Appearance::Dark, Appearance::Light] {
            let colors = appearance.palette();
            configure_text_appearance(&text_view, appearance);
            let rtf = themed_reader_rtf(&document, &images, 19.0, ReaderFont::Georgia, appearance)?;
            let attributed = reader_attributed_from_rtf(&document, &images, &rtf, appearance)?;
            let text = attributed.string();
            if let Some(original) = &baseline {
                if original != &text.to_string() {
                    return Err("Appearance changed reader text".into());
                }
            } else {
                baseline = Some(text.to_string());
            }
            for (needle, expected) in [
                ("HeadingToken", colors.ink),
                ("BodyToken", colors.ink),
                ("BoldToken", colors.ink),
                ("LinkToken", colors.green),
                ("CaptionToken", colors.muted),
                ("CellToken", colors.ink),
            ] {
                let range = text.rangeOfString(&NSString::from_str(needle));
                if range.location == usize::MAX {
                    return Err(format!("Appearance lost {needle}"));
                }
                let value = unsafe {
                    attributed.attribute_atIndex_effectiveRange(
                        NSForegroundColorAttributeName,
                        range.location,
                        std::ptr::null_mut(),
                    )
                }
                .ok_or_else(|| format!("Missing foreground for {needle}"))?;
                let color = value
                    .downcast_ref::<NSColor>()
                    .ok_or("Invalid foreground color")?;
                check_color(needle, color, expected)?;
            }
            let preserve = !text_view.string().is_empty();
            replace_text(&text_view, &attributed, preserve)?;
            if preserve && text_view.selectedRange() != selection {
                return Err("Appearance changed reader selection".into());
            }
            text_view.setSelectedRange(selection);
            check_color(
                "Reader background",
                &text_view.backgroundColor(),
                colors.reader,
            )?;
            check_color(
                "Insertion point",
                &text_view.insertionPointColor(),
                colors.ink,
            )?;
            let links = text_view
                .linkTextAttributes()
                .ok_or("Link attributes are absent")?;
            let value = links
                .objectForKey(unsafe { NSForegroundColorAttributeName })
                .ok_or("Link foreground is absent")?;
            let link = value
                .downcast_ref::<NSColor>()
                .ok_or("Invalid link foreground")?;
            check_color("Link foreground override", link, colors.green)?;
            let attributes = text_view.selectedTextAttributes();
            let value = attributes
                .objectForKey(unsafe { NSBackgroundColorAttributeName })
                .ok_or("Selection background is absent")?;
            let selected = value
                .downcast_ref::<NSColor>()
                .ok_or("Invalid selection background")?;
            check_color("Selection background", selected, colors.selected)?;
        }
        Ok(serde_json::json!({
            "native_reader_theme": true,
            "appearance_cycle": ["light", "dark", "light"],
            "text_and_selection_preserved": true,
            "foreground_link_caption_background_insertion_selection": true,
        }))
    }

    /// Main-thread regression against AppKit's actual decoded font attributes.
    /// Called explicitly by a developer proof binary; it opens no window.
    pub fn verify_reader_fonts() -> Result<serde_json::Value, String> {
        use objc2_app_kit::{NSApplication, NSFontAttributeName, NSFontManager, NSFontTraitMask};
        let mtm = MainThreadMarker::new().ok_or("Reader font proof requires the main thread")?;
        let _application = NSApplication::sharedApplication(mtm);
        let manager = NSFontManager::sharedFontManager(mtm);
        let bridge = Rc::new(ReaderBridge {
            header: RefCell::new(None),
            actions: RefCell::new(Vec::new()),
            wake: RefCell::new(None),
            font_choice: Cell::new(None),
        });
        let target = HeaderTarget::alloc(mtm).set_ivars(HeaderTargetIvars {
            bridge: bridge.clone(),
        });
        let target: Retained<HeaderTarget> = unsafe { msg_send![super(target), init] };
        let picker = font_picker(mtm, &target);
        for index in [0, 3] {
            if picker
                .itemAtIndex(index)
                .is_none_or(|item| item.isEnabled())
            {
                return Err("Reader font menu section must be disabled".into());
            }
        }
        for (index, expected) in [1, 2, 4, 5].into_iter().zip(ReaderFont::ALL) {
            picker.selectItemAtIndex(index);
            let _: () = unsafe { msg_send![&*target, readerFontChoice: &*picker] };
            if bridge.font_choice.take() != Some(expected) || !bridge.actions.borrow().is_empty() {
                return Err(
                    "Reader font picker requires an article or mutates article actions".into(),
                );
            }
        }
        let document = RichDocument::parse(
            "<h1>TitleToken</h1><h2>HeadingToken</h2><p>PlainToken <strong>BoldToken</strong> <em>ItalicToken</em> <strong><em>BothToken</em></strong> <a href='https://example.org/source'>ExternalToken</a> <a href='#note'>FootnoteToken</a> <code>CodeToken</code></p><table><tr><td>CellToken</td></tr></table><p id='note'>EndingToken</p><figcaption>MetadataToken</figcaption>",
        );
        let images = HashMap::new();
        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(780., 900.)),
        );
        configure_text(&text_view);
        let normalize = |family: &str| {
            family
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        };
        let mut baseline = None;
        let mut reports = Vec::new();
        for family in ReaderFont::ALL {
            let rtf = reader_rtf(&document, &images, 19., family)?;
            let attributed = attributed_from_rtf(&document, &images, &rtf, 780., 1_000_000.)?;
            let text = attributed.string();
            let index = |needle: &str| -> Result<usize, String> {
                let range = text.rangeOfString(&NSString::from_str(needle));
                if range.location == usize::MAX {
                    Err(format!("Font change lost {needle}"))
                } else {
                    Ok(range.location)
                }
            };
            let font_at = |needle: &str| -> Result<Retained<NSFont>, String> {
                unsafe {
                    attributed.attribute_atIndex_effectiveRange(
                        NSFontAttributeName,
                        index(needle)?,
                        std::ptr::null_mut(),
                    )
                }
                .ok_or_else(|| format!("Missing native font for {needle}"))?
                .downcast::<NSFont>()
                .map_err(|_| format!("Invalid native font for {needle}"))
            };
            let mut observed = serde_json::Map::new();
            for (needle, bold, italic) in [
                ("TitleToken", true, false),
                ("HeadingToken", true, false),
                ("PlainToken", false, false),
                ("BoldToken", true, false),
                ("ItalicToken", false, true),
                ("BothToken", true, true),
                ("CellToken", false, false),
                ("EndingToken", false, false),
            ] {
                let font = font_at(needle)?;
                let actual = font
                    .familyName()
                    .ok_or("Native font has no family")?
                    .to_string();
                if normalize(&actual) != normalize(family.family()) {
                    return Err(format!("{} decoded {needle} as {actual}", family.family()));
                }
                let traits = manager.traitsOfFont(&font);
                if traits.contains(NSFontTraitMask::BoldFontMask) != bold
                    || traits.contains(NSFontTraitMask::ItalicFontMask) != italic
                {
                    return Err(format!(
                        "{} lost bold/italic traits for {needle}: {}",
                        family.family(),
                        font.fontName()
                    ));
                }
                observed.insert(needle.into(),serde_json::json!({"family":actual,"name":font.fontName().to_string(),"bold":bold,"italic":italic}));
            }
            if normalize(
                &font_at("CodeToken")?
                    .familyName()
                    .ok_or("Code font missing")?
                    .to_string(),
            ) != "menlo"
            {
                return Err("Reader font change replaced Menlo code".into());
            }
            if normalize(
                &font_at("MetadataToken")?
                    .familyName()
                    .ok_or("Metadata font missing")?
                    .to_string(),
            ) != "helvetica"
            {
                return Err("Reader font change replaced system metadata font".into());
            }
            let mut links = Vec::new();
            for needle in ["ExternalToken", "FootnoteToken"] {
                let value = unsafe {
                    attributed.attribute_atIndex_effectiveRange(
                        NSLinkAttributeName,
                        index(needle)?,
                        std::ptr::null_mut(),
                    )
                }
                .ok_or_else(|| format!("Reader font change lost {needle} link"))?;
                let description: Retained<NSString> = unsafe { msg_send![&*value, description] };
                links.push((index(needle)?, description.to_string()));
            }
            let current = (text.to_string(), links);
            if baseline
                .as_ref()
                .is_some_and(|expected| expected != &current)
            {
                return Err("Reader font change altered text or link targets".into());
            }
            let prior_selection = text_view.selectedRange();
            replace_text(&text_view, &attributed, baseline.is_some())?;
            if baseline.is_some() && text_view.selectedRange() != prior_selection {
                return Err("Reader font change reset native selection".into());
            }
            text_view.setSelectedRange(NSRange::new(index("PlainToken")?, 10));
            baseline = Some(current);
            reports.push(serde_json::json!({"requested_family":family.family(),"observed_fonts":observed,"code_family":"Menlo","metadata_family":"Helvetica","text_and_links_unchanged":true,"selection_preserved":true}));
        }
        let recovery_id = crate::domain::ArticleId("synthetic-recovery-proof".into());
        *bridge.header.borrow_mut() = Some(ReaderHeader {
            article_id: recovery_id.clone(),
            publication: "Synthetic publication".into(),
            title: "A retained reading copy".into(),
            metadata: "Synthetic metadata".into(),
            read: false,
            archived: false,
            fetch_info: Some("Fetched today".into()),
            notice: Some(ReaderNotice::Recovery {
                title: "Your reading copy is still available".into(),
                summary: "You can keep reading the retained copy.".into(),
                details: "Synthetic exact failure details".into(),
            }),
        });
        let details_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Details"),
                Some(&target),
                Some(sel!(readerAction:)),
                mtm,
            )
        };
        details_button.setTag(4);
        let selected = text_view.selectedRange();
        let _: () = unsafe { msg_send![&*target, readerAction: &*details_button] };
        let actions = std::mem::take(&mut *bridge.actions.borrow_mut());
        if !matches!(actions.as_slice(), [ReaderAction { article_id, kind: ReaderActionKind::Details { title, message } }]
            if article_id == &recovery_id && title == "Your reading copy is still available" && message == "Synthetic exact failure details")
        {
            return Err(
                "Native recovery Details did not preserve its exact article/title/message".into(),
            );
        }
        if text_view.selectedRange() != selected {
            return Err("Native recovery Details changed text selection".into());
        }
        if let Some(header) = bridge.header.borrow_mut().as_mut() {
            header.notice = Some(ReaderNotice::Info("Fetched today".into()));
        }
        let _: () = unsafe { msg_send![&*target, readerAction: &*details_button] };
        if !bridge.actions.borrow().is_empty() {
            return Err("Native Details routed an action without a recovery notice".into());
        }
        Ok(
            serde_json::json!({"families":reports,"runtime":"Rust + AppKit font attributes","windows_opened":0,"native_picker":{"choices":4,"disabled_section_headers":2,"works_without_article":true,"article_actions_unchanged":true},"recovery_details":{"exact_article_title_message":true,"selection_preserved":true,"inactive_without_recovery":true}}),
        )
    }
    /// AppKit owns continuous document selection, links, accessibility, and body scrolling.
    /// The surrounding navigation and reader toolbar remain egui.
    pub struct NativeRichView {
        appearance: Appearance,
        parent: Retained<NSView>,
        scroll: Retained<NSScrollView>,
        text: Retained<NSTextView>,
        loaded_sha256: Option<String>,
        delegate: Retained<ReaderDelegate>,
        document_view: Retained<ReaderDocumentView>,
        header_text: Retained<NSTextView>,
        notice_text: Retained<NSTextView>,
        notice_title: Retained<NSTextView>,
        fetch_text: Retained<NSTextView>,
        buttons: Vec<Retained<NSButton>>,
        font_picker: Retained<NSPopUpButton>,
        _button_target: Retained<HeaderTarget>,
        bridge: Rc<ReaderBridge>,
    }
    impl NativeRichView {
        /// # Safety
        /// `ns_view` must be the live NSView pointer supplied by the main eframe window's
        /// AppKit raw-window-handle. Call and destroy this host on the main UI thread.
        pub unsafe fn attach(ns_view: NonNull<c_void>) -> Result<Self, String> {
            let mtm =
                MainThreadMarker::new().ok_or("Native rich view must attach on main thread")?;
            let parent = unsafe { Retained::<NSView>::retain(ns_view.as_ptr().cast()) }
                .ok_or("Missing AppKit view")?;
            let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(780.0, 900.0));
            let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), bounds);
            let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
            configure_text(&text);
            let document_view = ReaderDocumentView::alloc(mtm).set_ivars(ReaderDocumentIvars {
                appearance: Cell::new(Appearance::Light),
                divider: Cell::new(0.0),
                inset: Cell::new(76.0),
            });
            let document_view: Retained<ReaderDocumentView> =
                unsafe { msg_send![super(document_view),initWithFrame:bounds] };
            let header_text = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
            configure_text(&header_text);
            let notice_text = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
            configure_text(&notice_text);
            let notice_title = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
            configure_text(&notice_title);
            let fetch_text = NSTextView::initWithFrame(NSTextView::alloc(mtm), bounds);
            configure_text(&fetch_text);
            document_view.addSubview(&header_text);
            document_view.addSubview(&notice_text);
            document_view.addSubview(&notice_title);
            document_view.addSubview(&fetch_text);
            document_view.addSubview(&text);
            let bridge = Rc::new(ReaderBridge {
                header: RefCell::new(None),
                actions: RefCell::new(Vec::new()),
                wake: RefCell::new(None),
                font_choice: Cell::new(None),
            });
            let target = HeaderTarget::alloc(mtm).set_ivars(HeaderTargetIvars {
                bridge: bridge.clone(),
            });
            let target: Retained<HeaderTarget> = unsafe { msg_send![super(target), init] };
            let font_picker = font_picker(mtm, &target);
            let mut buttons = Vec::new();
            for (index, label) in ["Mark as read", "Archive", "Delete", "Re-fetch", "Details"]
                .iter()
                .enumerate()
            {
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str(label),
                        Some(&target),
                        Some(objc2::sel!(readerAction:)),
                        mtm,
                    )
                };
                button.setTag(index as isize);
                // Read, archive, delete and re-fetch live in the egui toolbar.
                // Details stays inline and appears only with a recovery notice.
                button.setHidden(true);
                button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
                button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
                document_view.addSubview(&button);
                buttons.push(button);
            }
            scroll.setHasVerticalScroller(true);
            scroll.setHasHorizontalScroller(false);
            scroll.setAutohidesScrollers(true);
            scroll.setDocumentView(Some(&document_view));
            let delegate = ReaderDelegate::alloc(mtm).set_ivars(ReaderDelegateIvars {
                anchors: RefCell::new(HashMap::new()),
            });
            let delegate: Retained<ReaderDelegate> = unsafe { msg_send![super(delegate), init] };
            text.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            let window = parent.window().ok_or("Native reader window missing")?;
            let content = window
                .contentView()
                .ok_or("Native window content missing")?;
            if !std::ptr::eq(&*content, &*parent) {
                return Err("Native reader must attach to the original Winit content view".into());
            }
            attach_accessibility(&parent, &scroll)?;
            parent.addSubview(&scroll);
            parent.addSubview(&font_picker);
            NATIVE_PICKERS.with(|pickers| {
                pickers
                    .borrow_mut()
                    .insert(Retained::as_ptr(&parent) as usize, font_picker.clone());
            });
            if !window
                .contentView()
                .is_some_and(|content| std::ptr::eq(&*content, &*parent))
                || !scroll.isDescendantOf(&parent)
            {
                return Err("Native reader attachment changed the Winit content view".into());
            }
            window.makeFirstResponder(Some(&parent));
            scroll.setHidden(true);
            let view = Self {
                appearance: Appearance::Light,
                parent,
                scroll,
                text,
                loaded_sha256: None,
                delegate,
                document_view,
                header_text,
                notice_text,
                notice_title,
                fetch_text,
                buttons,
                font_picker,
                _button_target: target,
                bridge,
            };
            view.apply_appearance();
            Ok(view)
        }
        /// Returns whether header/body content must be refreshed from the already loaded copy.
        pub fn set_appearance(&mut self, appearance: Appearance) -> bool {
            if self.appearance == appearance {
                return false;
            }
            self.appearance = appearance;
            self.apply_appearance();
            true
        }
        fn apply_appearance(&self) {
            let colors = self.appearance.palette();
            let appearance_name = NSString::from_str(match self.appearance {
                Appearance::Light => "NSAppearanceNameAqua",
                Appearance::Dark => "NSAppearanceNameDarkAqua",
            });
            let appearance = NSAppearance::appearanceNamed(&appearance_name);
            self.scroll.setAppearance(appearance.as_deref());
            self.font_picker.setAppearance(appearance.as_deref());
            self.font_picker
                .setContentTintColor(Some(&native_color(colors.ink)));
            self.scroll.setBackgroundColor(&native_color(colors.reader));
            self.document_view.ivars().appearance.set(self.appearance);
            self.document_view.setNeedsDisplay(true);
            for text in [
                &self.text,
                &self.header_text,
                &self.notice_text,
                &self.notice_title,
                &self.fetch_text,
            ] {
                configure_text_appearance(text, self.appearance);
            }
            for (index, button) in self.buttons.iter().enumerate() {
                button.setContentTintColor(Some(&native_color(if index == 2 {
                    colors.danger
                } else {
                    colors.green
                })));
            }
        }
        pub fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
            *self.bridge.wake.borrow_mut() = Some(wake);
        }
        pub fn take_actions(&self) -> Vec<ReaderAction> {
            std::mem::take(&mut *self.bridge.actions.borrow_mut())
        }
        pub fn take_font_choice(&self) -> Option<ReaderFont> {
            self.bridge.font_choice.take()
        }
        /// Native popup menus draw above the native reader, unlike egui popup layers.
        /// This control belongs to the toolbar and remains independent of article loading.
        pub fn set_font_picker_rect(
            &self,
            rect: Option<egui::Rect>,
            viewport_size: egui::Vec2,
            font: ReaderFont,
        ) {
            let index = match font {
                ReaderFont::Georgia => 1,
                ReaderFont::Baskerville => 2,
                ReaderFont::AvenirNext => 4,
                ReaderFont::HelveticaNeue => 5,
            };
            if self.font_picker.indexOfSelectedItem() != index {
                self.font_picker.selectItemAtIndex(index);
            }
            let Some(rect) = rect else {
                if !self.font_picker.isHidden() {
                    self.focus_controls();
                    self.font_picker.setHidden(true);
                }
                return;
            };
            let bounds = self.parent.bounds();
            let scale = bounds.size.width / f64::from(viewport_size.x.max(1.0));
            let y = if self.parent.isFlipped() {
                f64::from(rect.min.y) * scale
            } else {
                bounds.size.height - f64::from(rect.max.y) * scale
            };
            self.font_picker.setFrame(NSRect::new(
                NSPoint::new(f64::from(rect.min.x) * scale, y),
                NSSize::new(
                    f64::from(rect.width()) * scale,
                    f64::from(rect.height()) * scale,
                ),
            ));
            self.font_picker.setHidden(false);
        }
        pub fn set_header(&mut self, header: ReaderHeader, font: ReaderFont) -> Result<(), String> {
            let document = RichDocument {
                image_layouts: HashMap::new(),
                image_links: HashMap::new(),
                source_sha256: String::new(),
                blocks: vec![
                    super::Block::Paragraph {
                        kind: super::ParagraphKind::Caption,
                        runs: vec![super::Inline {
                            text: header.publication.to_uppercase(),
                            style: super::InlineStyle::default(),
                        }],
                        anchor: None,
                    },
                    super::Block::Paragraph {
                        kind: super::ParagraphKind::Heading(1),
                        runs: vec![super::Inline {
                            text: header.title.clone(),
                            style: super::InlineStyle::default(),
                        }],
                        anchor: None,
                    },
                    super::Block::Paragraph {
                        kind: super::ParagraphKind::Caption,
                        runs: vec![super::Inline {
                            text: header.metadata.clone(),
                            style: super::InlineStyle::default(),
                        }],
                        anchor: None,
                    },
                ],
            };
            let rtf = String::from_utf8(themed_reader_rtf(
                &document,
                &HashMap::new(),
                11.0,
                font,
                self.appearance,
            )?)
            .map_err(|e| e.to_string())?
            .replace("\\b\\fs64", "\\b0\\fs80")
            .replace("\\sl444\\slmult1", "\\sl270\\slmult1")
            .replace("\\sa110", "\\sa360")
            .replace("\\f2\\fs24", "\\f2\\fs22");
            let attributed = reader_attributed_from_rtf(
                &document,
                &HashMap::new(),
                rtf.as_bytes(),
                self.appearance,
            )?;
            let same_article = self
                .bridge
                .header
                .borrow()
                .as_ref()
                .is_some_and(|previous| previous.article_id.0 == header.article_id.0);
            if !same_article {
                self.text.setSelectedRange(NSRange::new(0, 0));
                let clip = self.scroll.contentView();
                clip.scrollToPoint(NSPoint::new(0.0, 0.0));
                self.scroll.reflectScrolledClipView(&clip);
            }
            replace_text(&self.header_text, &attributed, same_article)?;
            self.buttons[0].setTitle(&NSString::from_str(if header.read {
                "Mark as unread"
            } else {
                "Mark as read"
            }));
            self.buttons[1].setTitle(&NSString::from_str(if header.archived {
                "Restore"
            } else {
                "Archive"
            }));
            let (title, notice, warning, recovery) = match &header.notice {
                Some(ReaderNotice::Info(message)) => ("", message.as_str(), false, false),
                Some(ReaderNotice::Warning(message)) => ("", message.as_str(), true, false),
                Some(ReaderNotice::Recovery { title, summary, .. }) => {
                    (title.as_str(), summary.as_str(), true, true)
                }
                None => ("", "", false, false),
            };
            self.notice_title.setString(&NSString::from_str(title));
            self.notice_title
                .setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
            self.notice_title
                .setTextColor(Some(&native_color(self.appearance.palette().ink)));
            self.notice_text.setString(&NSString::from_str(notice));
            self.notice_text
                .setFont(Some(&NSFont::systemFontOfSize(if recovery {
                    12.0
                } else {
                    11.0
                })));
            let colors = self.appearance.palette();
            self.notice_text
                .setTextColor(Some(&native_color(if recovery {
                    colors.ink
                } else if warning {
                    colors.warning
                } else {
                    colors.muted
                })));
            self.buttons[4].setHidden(!recovery);
            self.fetch_text.setString(&NSString::from_str(
                header.fetch_info.as_deref().unwrap_or_default(),
            ));
            self.fetch_text
                .setFont(Some(&NSFont::systemFontOfSize(11.)));
            self.fetch_text
                .setTextColor(Some(&native_color(colors.muted)));
            *self.bridge.header.borrow_mut() = Some(header);
            Ok(())
        }
        pub fn set_document(
            &mut self,
            document: &RichDocument,
            images: &HashMap<String, Vec<u8>>,
            body_size: f32,
            font: ReaderFont,
        ) -> Result<(), String> {
            let attributed = reader_document(document, images, body_size, font, self.appearance)?;
            let mut character = 0;
            while character < attributed.length() {
                let mut range = NSRange::new(character, 1);
                if let Some(value) = unsafe {
                    attributed.attribute_atIndex_effectiveRange(
                        NSParagraphStyleAttributeName,
                        character,
                        &mut range,
                    )
                } && let Some(style) = value.downcast_ref::<NSParagraphStyle>()
                {
                    for block in style.textBlocks().iter() {
                        if let Some(cell) = block.downcast_ref::<NSTextTableBlock>() {
                            let table = cell.table();
                            table.setValue_type_forDimension(
                                100.0,
                                NSTextBlockValueType::PercentageValueType,
                                NSTextBlockDimension::Width,
                            );
                            cell.setValue_type_forDimension(
                                100.0 * cell.columnSpan() as f64
                                    / table.numberOfColumns().max(1) as f64,
                                NSTextBlockValueType::PercentageValueType,
                                NSTextBlockDimension::Width,
                            );
                        }
                    }
                }
                character = range.location + range.length.max(1);
            }
            let same_document = self.loaded_sha256.as_deref() == Some(&document.source_sha256);
            let scroll_origin = self.scroll.contentView().bounds().origin;
            replace_text(&self.text, &attributed, same_document)?;
            *self.delegate.ivars().anchors.borrow_mut() =
                anchor_ranges(document, &attributed.string());
            if !same_document {
                let clip = self.scroll.contentView();
                clip.scrollToPoint(NSPoint::new(0.0, 0.0));
                self.scroll.reflectScrolledClipView(&clip);
            } else {
                let clip = self.scroll.contentView();
                clip.scrollToPoint(scroll_origin);
                self.scroll.reflectScrolledClipView(&clip);
            }
            self.loaded_sha256 = Some(document.source_sha256.clone());
            Ok(())
        }
        /// Position the body in egui points. Coordinates are converted to the native
        /// view's logical points and orientation, including egui zoom and Retina scale.
        pub fn set_rect(&self, rect: egui::Rect, viewport_size: egui::Vec2) {
            let bounds = self.parent.bounds();
            let scale = bounds.size.width / f64::from(viewport_size.x.max(1.0));
            let x = f64::from(rect.min.x) * scale;
            let width = f64::from(rect.width()) * scale;
            let height = f64::from(rect.height()) * scale;
            let y = if self.parent.isFlipped() {
                f64::from(rect.min.y) * scale
            } else {
                bounds.size.height - f64::from(rect.max.y) * scale
            };
            self.scroll
                .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
            let minimum_inset = if width > 700.0 { 76.0 } else { 28.0 };
            let inset = ((width - 780.0) / 2.0).max(minimum_inset);
            let content_width = (width - inset * 2.0).max(80.0);
            self.document_view.ivars().inset.set(inset);
            if let Some(storage) = unsafe { self.text.textStorage() } {
                for (index, unit) in storage.string().to_string().encode_utf16().enumerate() {
                    if unit == 0xfffc
                        && let Some(object) = unsafe {
                            storage.attribute_atIndex_effectiveRange(
                                NSAttachmentAttributeName,
                                index,
                                std::ptr::null_mut(),
                            )
                        }
                        && let Some(attachment) = object.downcast_ref::<NSTextAttachment>()
                    {
                        let bounds = attachment.bounds();
                        let original = attachment
                            .image()
                            .map(|image| image.size())
                            .unwrap_or(bounds.size);
                        let available = (width - inset * 2.0).max(80.0);
                        let factor = (available / original.width.max(1.0)).min(1.0);
                        attachment.setBounds(NSRect::new(
                            bounds.origin,
                            NSSize::new(original.width * factor, original.height * factor),
                        ));
                    }
                }
            }
            let header_height = layout_text(&self.header_text, content_width);
            self.header_text.setFrameOrigin(NSPoint::new(inset, 38.0));
            let mut y = 38.0 + header_height + 24.0;
            if !self.notice_title.string().is_empty() {
                let title_height = layout_text(&self.notice_title, content_width);
                self.notice_title.setFrameOrigin(NSPoint::new(inset, y));
                self.notice_title.setHidden(false);
                y += title_height + 6.;
            } else {
                self.notice_title.setHidden(true);
            }
            if !self.notice_text.string().is_empty() {
                let notice_height = layout_text(&self.notice_text, content_width);
                self.notice_text.setFrameOrigin(NSPoint::new(inset, y));
                self.notice_text.setHidden(false);
                y += notice_height + 8.;
            } else {
                self.notice_text.setHidden(true);
            }
            if !self.buttons[4].isHidden() {
                let details = &self.buttons[4];
                details.sizeToFit();
                details.setFrame(NSRect::new(
                    NSPoint::new(inset, y),
                    NSSize::new(details.frame().size.width.max(60.) + 8., 30.),
                ));
                y += 40.;
            }
            if !self.fetch_text.string().is_empty() {
                let fetch_height = layout_text(&self.fetch_text, content_width);
                self.fetch_text.setFrameOrigin(NSPoint::new(inset, y));
                self.fetch_text.setHidden(false);
                y += fetch_height + 12.;
            } else {
                self.fetch_text.setHidden(true);
            }
            self.document_view.ivars().divider.set(y);
            y += 25.0;
            let body_height = layout_text(&self.text, content_width);
            self.text.setFrameOrigin(NSPoint::new(inset, y));
            self.document_view
                .setFrameSize(NSSize::new(width, (y + body_height + 38.0).max(height)));
            self.document_view.setNeedsDisplay(true);
        }
        pub fn set_visible(&self, visible: bool) {
            // Hiding NSTextView may first move its responder to NSWindow.
            // Hand control to Winit before AppKit performs that transition,
            // including subsequent frames where the reader is already hidden.
            let picker_focused = !self.font_picker.isHidden()
                && self
                    .parent
                    .window()
                    .and_then(|window| window.firstResponder())
                    .and_then(|responder| responder.downcast::<NSView>().ok())
                    .is_some_and(|view| view.isDescendantOf(&self.font_picker));
            if !visible && !picker_focused {
                self.focus_controls();
            }
            if self.scroll.isHidden() == !visible {
                return;
            }
            self.scroll.setHidden(!visible);
        }
        /// Give egui controls their native keyboard event destination. Merely
        /// setting an egui focus ID does not resign an AppKit text responder.
        pub fn focus_controls(&self) {
            if let Some(window) = self.parent.window()
                && !window.firstResponder().is_some_and(|responder| {
                    std::ptr::eq(
                        Retained::as_ptr(&responder).cast::<c_void>(),
                        Retained::as_ptr(&self.parent).cast::<c_void>(),
                    )
                })
            {
                window.makeFirstResponder(Some(&self.parent));
            }
        }
        fn focused_text(&self) -> Retained<NSTextView> {
            self.parent
                .window()
                .and_then(|window| window.firstResponder())
                .and_then(|responder| responder.downcast::<NSTextView>().ok())
                .filter(|text| text.isDescendantOf(&self.scroll))
                .unwrap_or_else(|| self.text.clone())
        }
        pub fn select_all(&self) {
            unsafe {
                self.focused_text().selectAll(None);
            }
        }
        pub fn copy(&self) {
            unsafe {
                self.focused_text().copy(None);
            }
        }
        pub fn selected_text(&self) -> String {
            let text = self.focused_text();
            let range = text.selectedRange();
            text.string().substringWithRange(range).to_string()
        }
    }
    impl Drop for NativeRichView {
        fn drop(&mut self) {
            NATIVE_SCROLLS.with(|views| {
                views
                    .borrow_mut()
                    .remove(&(Retained::as_ptr(&self.parent) as usize));
            });
            NATIVE_PICKERS.with(|pickers| {
                pickers
                    .borrow_mut()
                    .remove(&(Retained::as_ptr(&self.parent) as usize));
            });
            self.font_picker.removeFromSuperview();
            self.scroll.removeFromSuperview();
        }
    }
}
