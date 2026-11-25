//! A Markdown file's View: the file drawn as a document (headings, lists,
//! tables, coloured code, pictures, diagrams, math, footnotes) from the text
//! in its editor, so unsaved edits show and changes on disk follow. Text is
//! selected and copied as in the AI tab's answers (`md_draw`).
//!
//! The document is a column of rows (a paragraph, a list, a table, a
//! diagram), each a view of its own (`RowView`), in a scrolling page
//! (`Page`). A row is drawn again only when something in it changes (the
//! link under the mouse, the selection, a diagram drawn), never because the
//! window is: frames stay as cheap as the editor's, however long the file.
//! The page holds only the rows in view (spaces as tall as the others stand
//! for them), so scrolling draws those alone; it learns every row's height
//! by laying them all out once, when the rows or the width change. (GPUI's
//! `list` would draw the rows in view at every frame of the window.)
//! Diagrams and math are drawn off the UI thread (`shed_markdown::render`)
//! and kept by their source, colours, size and the screen's scale; pictures
//! are read off it and kept by path.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    RenderImage, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, div, img, point, prelude::*, px,
    svg,
};
use shed_markdown::Block;
use shed_markdown::render::Colors;
use shed_model::Rgb;

use crate::ai_text::{self, Copy, Kind, Piece, Point, Selection};
use crate::assets::icon;
use crate::md_draw::{self, CodeColors, PieceHost, PieceSource, TextSel};
use crate::settings::Settings;
use crate::text_view::TextView;
use crate::theme::{UiColors, current_ui, hsla, hsla_alpha};

/// The widest a document is set (at zoom 1), for lines that read well;
/// diagrams are drawn no wider.
const MAX_WIDTH: f32 = 900.0;
/// Space around the document.
const PAD: f32 = 24.0;

/// Whether a file is Markdown (by its name).
pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown" | "mkd" | "mdx"))
}

pub enum MdEvent {
    /// A link to a file of the project (a path, and an anchor in it).
    OpenFile(PathBuf, Option<String>),
}

/// The document as drawn: its pieces, and the colours of those that are code.
struct Doc {
    pieces: Vec<Piece>,
    code: Vec<Option<CodeColors>>,
}

impl PieceSource for Doc {
    fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    fn code_colors(&self, j: usize) -> Option<&CodeColors> {
        self.code.get(j).and_then(Option::as_ref)
    }
}

/// A diagram or math, drawn.
enum Drawn {
    Picture(Arc<RenderImage>, (f32, f32)),
    /// It could not be: why, briefly.
    Failed(String),
}

/// What a diagram or math is drawn for: the widest it may be and the
/// screen's scale (in hundredths).
type Fit = (u32, u32);

#[derive(Default)]
struct Visual {
    have: Option<(Fit, Drawn)>,
    drawing: Option<(Fit, Task<()>)>,
}

/// A picture of the file's, being read or read (`None`: it cannot be).
enum Pic {
    Reading { _task: Task<()> },
    Read(Option<crate::picture::Picture>),
}

pub struct MarkdownView {
    path: PathBuf,
    source: Entity<TextView>,
    accent: Rgb,
    settings: Settings,
    /// Pictures are read from this disk (not for another workspace's file).
    local: bool,
    focus: FocusHandle,
    doc: Rc<Doc>,
    /// The editor's text version the document was read from.
    version: Option<u64>,
    /// The rows: pieces drawn together (a paragraph, a list, a table).
    groups: Vec<Range<usize>>,
    /// Where `#anchors` go: the piece.
    anchors: Vec<(String, usize)>,
    page: Entity<Page>,
    sel: TextSel,
    copied: Option<(SharedString, Task<()>)>,
    /// Diagrams and math, by what they are drawn from; and the piece each
    /// was last drawn at (its row is drawn again when it is done).
    visuals: HashMap<u64, Visual>,
    visual_at: HashMap<u64, usize>,
    pictures: HashMap<PathBuf, Pic>,
    /// Code blocks' colours, by language and text.
    code_cache: HashMap<(String, u64), Option<CodeColors>>,
    coloring: Option<Task<()>>,
    /// Pixels per point, as last drawn.
    scale: f32,
    /// An anchor to go to once the document is read.
    go_to: Option<String>,
    /// The rows' views, made anew when the document is read again.
    rows: Vec<Entity<RowView>>,
    _subs: Vec<Subscription>,
}

/// The scrolling page the rows are in (with its scrollbar): drawn again as
/// it scrolls, with the rows in view.
pub struct Page {
    rows: Vec<Entity<RowView>>,
    scroll: ScrollHandle,
    bar: crate::scrollbar::State,
    /// Each row's height, as last laid out.
    heights: Vec<Option<f32>>,
    /// The rows the page held at the last frame (children after the space above).
    held: Range<usize>,
    /// Hold every row at the next frame, to learn their heights.
    measure: bool,
    width: f32,
    /// A row to bring to the top once the heights are known.
    show: Option<usize>,
}

/// A row's height until it is laid out.
const ROW_GUESS: f32 = 48.0;

impl Page {
    fn new() -> Self {
        Page { rows: Vec::new(), scroll: ScrollHandle::new(), bar: crate::scrollbar::State::default(), heights: Vec::new(), held: 0..0, measure: true, width: 0.0, show: None }
    }

    /// New rows (the document read again): their heights are learnt anew.
    fn set_rows(&mut self, rows: Vec<Entity<RowView>>) {
        self.heights = vec![None; rows.len()];
        self.rows = rows;
        self.held = 0..0;
        self.measure = true;
    }

    /// Where row `ix` starts.
    fn top_of(&self, ix: usize) -> f32 {
        self.heights[..ix.min(self.heights.len())].iter().map(|h| h.unwrap_or(ROW_GUESS)).sum()
    }

    /// The row at `y` from the top of the document.
    fn row_at(&self, y: f32) -> usize {
        let mut top = 0.0;
        for (ix, h) in self.heights.iter().enumerate() {
            top += h.unwrap_or(ROW_GUESS);
            if top > y {
                return ix;
            }
        }
        self.heights.len().saturating_sub(1)
    }

    /// Takes the heights of the rows held at the last frame; a row above
    /// the view that changed moves the view with it, so what shows stays.
    fn learn_heights(&mut self) {
        let offset = -f32::from(self.scroll.offset().y);
        let top_row = self.row_at(offset);
        let mut moved = 0.0;
        for (k, ix) in self.held.clone().enumerate() {
            let Some(bounds) = self.scroll.bounds_for_item(k + 1) else { continue };
            let Some(slot) = self.heights.get_mut(ix) else { continue };
            let h = f32::from(bounds.size.height);
            if ix < top_row {
                moved += h - slot.unwrap_or(ROW_GUESS);
            }
            *slot = Some(h);
        }
        if moved.abs() > 0.5 {
            let at = self.scroll.offset();
            self.scroll.set_offset(point(at.x, at.y - px(moved)));
        }
    }
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = current_ui(cx);
        let width = f32::from(self.scroll.bounds().size.width);
        if (width - self.width).abs() > 0.5 {
            self.width = width;
            self.measure = true;
        }
        self.learn_heights();
        let n = self.rows.len();
        let known = self.heights.iter().all(Option::is_some);
        if known && let Some(ix) = self.show.take() {
            self.scroll.set_offset(point(px(0.0), -px(self.top_of(ix))));
        }
        let held = if std::mem::take(&mut self.measure) || !known {
            // Every row, once; the next frame holds those in view.
            cx.on_next_frame(window, |_, _, cx| cx.notify());
            0..n
        } else {
            let offset = -f32::from(self.scroll.offset().y);
            let view = f32::from(self.scroll.bounds().size.height).max(200.0);
            self.row_at(offset)..(self.row_at(offset + view) + 1).min(n)
        };
        let (above, below) = (self.top_of(held.start), self.top_of(n) - self.top_of(held.end));
        self.held = held.clone();
        let rows = div()
            .id("md-page")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(div().flex_none().h(px(above)))
            .children(self.rows[held].iter().cloned())
            .child(div().flex_none().h(px(below)));
        crate::scrollbar::with_bar(rows, crate::scrollbar::Source::Handle(self.scroll.clone()), &self.bar, &ui).size_full()
    }
}

/// A row of the document (`MarkdownView::groups`), drawn by the view.
pub struct RowView {
    md: WeakEntity<MarkdownView>,
    ix: usize,
}

impl Render for RowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ix, scale) = (self.ix, window.scale_factor());
        match self.md.upgrade() {
            Some(md) => md.update(cx, |md, cx| {
                md.scale = scale;
                md.render_group(ix, cx)
            }),
            None => div().into_any_element(),
        }
    }
}

impl EventEmitter<MdEvent> for MarkdownView {}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

fn hash_of(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn rgb(c: Rgb) -> [u8; 3] {
    [c.r, c.g, c.b]
}

/// Pieces drawn together, in order: a list (with code inside its items), a
/// table, the footnotes; anything else alone.
fn group(pieces: &[Piece]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut j = 0;
    while j < pieces.len() {
        let start = j;
        j += 1;
        match &pieces[start].kind {
            Kind::Item { .. } => {
                while j < pieces.len() && matches!(pieces[j].kind, Kind::Item { .. } | Kind::Code { depth: 1.., .. }) {
                    j += 1;
                }
            }
            Kind::Cell { .. } => {
                while j < pieces.len() && matches!(pieces[j].kind, Kind::Cell { row, col, .. } if row > 0 || col > 0) {
                    j += 1;
                }
            }
            Kind::Footnote { .. } => {
                while j < pieces.len() && matches!(pieces[j].kind, Kind::Footnote { .. }) {
                    j += 1;
                }
            }
            _ => {}
        }
        out.push(start..j);
    }
    out
}

/// Where each block's pieces start (a table has a piece per cell).
fn block_starts(blocks: &[Block]) -> Vec<usize> {
    let mut at = 0;
    blocks
        .iter()
        .map(|b| {
            let start = at;
            at += match b {
                Block::Table { header, rows, .. } => header.len() + rows.iter().map(Vec::len).sum::<usize>(),
                _ => 1,
            };
            start
        })
        .collect()
}

/// `%20` and the like in a link, as the characters.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// A link's target in the project: the file (relative to `dir`, the
/// linking file's folder) and an anchor in it; `None` for one on the web.
pub fn resolve_link(dir: &Path, url: &str) -> Option<(PathBuf, Option<String>)> {
    if url.contains("://") || url.starts_with("mailto:") || url.starts_with('#') {
        return None;
    }
    let (path, anchor) = match url.split_once('#') {
        Some((p, a)) => (p, Some(a.to_string())),
        None => (url, None),
    };
    let path = percent_decode(path.split('?').next().unwrap_or(path));
    let path = Path::new(&path);
    Some((normalize(&if path.is_absolute() { path.to_path_buf() } else { dir.join(path) }), anchor))
}

/// A path without `.` and `..` in it (as written, not following links).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

impl MarkdownView {
    pub fn new(path: PathBuf, source: Entity<TextView>, accent: Rgb, settings: Settings, local: bool, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            // Edits and reloads: read again when next drawn.
            cx.observe(&source, |this: &mut Self, source, cx| {
                if this.version != Some(source.read(cx).buffer.version()) {
                    cx.notify();
                }
            }),
            // Its pictures leave the GPU's atlas with it.
            cx.on_release(|this: &mut Self, cx| {
                for v in this.visuals.values_mut() {
                    if let Some((_, Drawn::Picture(image, _))) = v.have.take() {
                        cx.drop_image(image, None);
                    }
                }
            }),
        ];
        MarkdownView {
            path,
            source,
            accent,
            settings,
            local,
            focus: cx.focus_handle(),
            doc: Rc::new(Doc { pieces: Vec::new(), code: Vec::new() }),
            version: None,
            groups: Vec::new(),
            anchors: Vec::new(),
            page: cx.new(|_| Page::new()),
            sel: TextSel::default(),
            copied: None,
            visuals: HashMap::new(),
            visual_at: HashMap::new(),
            pictures: HashMap::new(),
            code_cache: HashMap::new(),
            coloring: None,
            scale: 1.0,
            go_to: None,
            rows: Vec::new(),
            _subs: subs,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// New colours, font or zoom.
    pub fn apply(&mut self, settings: &Settings, cx: &mut Context<Self>) {
        self.settings = settings.clone();
        self.code_cache.clear();
        self.version = None;
        cx.notify();
    }

    /// A file changed on disk: a picture of the document's is read again.
    pub fn file_changed(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.pictures.remove(path).is_some() {
            cx.notify();
        }
    }

    /// Shows the heading (or footnote) an anchor names, now or once read.
    pub fn go_to_anchor(&mut self, anchor: &str, cx: &mut Context<Self>) {
        self.go_to = Some(anchor.to_string());
        cx.notify();
    }

    fn z(&self) -> f32 {
        self.settings.font.size / crate::theme::BASE_FONT_SIZE
    }

    fn scroll(&self, cx: &gpui::App) -> ScrollHandle {
        self.page.read(cx).scroll.clone()
    }

    /// Reads the editor's text again if it changed since.
    fn read_source(&mut self, cx: &mut Context<Self>) {
        let (version, text) = {
            let b = &self.source.read(cx).buffer;
            let v = b.version();
            if self.version == Some(v) {
                return;
            }
            (v, b.text())
        };
        self.version = Some(version);
        let blocks = shed_markdown::parse_doc(&text);
        let starts = block_starts(&blocks);
        let anchors = shed_markdown::anchors(&blocks).into_iter().filter_map(|(a, b)| starts.get(b).map(|p| (a, *p))).collect();
        let pieces = ai_text::pieces(blocks);
        // Code keeps its colours while the new ones are found.
        let code = pieces
            .iter()
            .map(|p| match &p.kind {
                Kind::Code { lang, .. } => self.code_cache.get(&(lang.clone(), hash_of(&p.text.text))).cloned().flatten(),
                _ => None,
            })
            .collect();
        self.groups = group(&pieces);
        self.anchors = anchors;
        self.doc = Rc::new(Doc { pieces, code });
        let md = cx.entity().downgrade();
        self.rows = (0..self.groups.len()).map(|ix| cx.new(|_| RowView { md: md.clone(), ix })).collect();
        let rows = self.rows.clone();
        self.page.update(cx, |page, cx| {
            page.set_rows(rows);
            cx.notify();
        });
        if let Some(sel) = self.sel.selection
            && sel.ordered().1.piece >= self.doc.pieces.len()
        {
            self.sel.selection = None;
        }
        self.sel.hover_link = None;
        self.color_code(cx);
    }

    /// Colours the code blocks not coloured yet, off the UI thread.
    fn color_code(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, String)> = self
            .doc
            .pieces
            .iter()
            .filter_map(|p| match &p.kind {
                Kind::Code { lang, .. } if !lang.is_empty() && !self.code_cache.contains_key(&(lang.clone(), hash_of(&p.text.text))) => Some((lang.clone(), p.text.text.clone())),
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let palette = self.settings.palette.clone();
        self.coloring = Some(cx.spawn(async move |this, cx| {
            let colored = cx
                .background_executor()
                .spawn(async move { wanted.into_iter().map(|(lang, code)| ((lang.clone(), hash_of(&code)), md_draw::code_colors(&lang, &code, &palette))).collect::<Vec<_>>() })
                .await;
            this.update(cx, |this, cx| {
                this.code_cache.extend(colored);
                let code = this
                    .doc
                    .pieces
                    .iter()
                    .map(|p| match &p.kind {
                        Kind::Code { lang, .. } => this.code_cache.get(&(lang.clone(), hash_of(&p.text.text))).cloned().flatten(),
                        _ => None,
                    })
                    .collect();
                this.doc = Rc::new(Doc { pieces: this.doc.pieces.clone(), code });
                cx.notify();
            })
            .ok();
        }));
    }

    /// The colours diagrams and math are drawn in.
    fn colors(&self, ui: &UiColors) -> Colors {
        Colors { text: rgb(ui.text), ground: rgb(self.settings.palette.background), accent: rgb(ui.ink(self.accent)) }
    }

    /// Goes to an anchor if one waits and the document has it.
    fn follow_anchor(&mut self, cx: &mut Context<Self>) {
        let Some(anchor) = self.go_to.take() else { return };
        let anchor = anchor.trim_start_matches('#').to_lowercase();
        if let Some(group) = self.anchors.iter().find(|(a, _)| a.to_lowercase() == anchor).and_then(|(_, p)| self.group_of(*p)) {
            self.page.update(cx, |page, cx| {
                page.show = Some(group);
                cx.notify();
            });
        }
    }

    fn group_of(&self, piece: usize) -> Option<usize> {
        let g = self.groups.partition_point(|g| g.end <= piece);
        (g < self.groups.len()).then_some(g)
    }

    /// Draws again the rows that hold these pieces.
    fn redraw_pieces(&self, pieces: impl IntoIterator<Item = Range<usize>>, cx: &mut Context<Self>) {
        let mut rows: Vec<usize> = Vec::new();
        for r in pieces.into_iter().filter(|r| r.start < r.end) {
            let (Some(first), Some(last)) = (self.group_of(r.start), self.group_of(r.end - 1)) else { continue };
            rows.extend(first..=last);
        }
        rows.sort_unstable();
        rows.dedup();
        for ix in rows {
            if let Some(row) = self.rows.get(ix) {
                row.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// Draws again the row that holds piece `j`.
    fn redraw_piece(&self, j: usize, cx: &mut Context<Self>) {
        self.redraw_pieces(std::iter::once(j..j + 1), cx);
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.doc.pieces.len().checked_sub(1) else { return };
        let end = self.doc.pieces[last].text.text.len();
        self.sel.selection = Some(Selection { anchor: Point { entry: 0, piece: 0, offset: 0 }, head: Point { entry: 0, piece: last, offset: end } });
        cx.notify();
    }

    fn selected_copy(&self) -> Option<Copy> {
        let sel = self.sel.selection.filter(|s| !s.is_empty())?;
        let (a, b) = sel.ordered();
        let mut copy = if a.piece == b.piece { Copy::within_one() } else { Copy::default() };
        for (j, piece) in self.doc.pieces.iter().enumerate().take(b.piece + 1).skip(a.piece) {
            if let Some(range) = sel.range_in(0, j, piece.text.text.len()) {
                copy.add(piece, range);
            }
        }
        Some(copy)
    }

    /// Lets go of the selection.
    fn unselect(&mut self, cx: &mut Context<Self>) {
        if self.sel.selection.is_some() {
            let before = self.sel.clone();
            self.sel.selection = None;
            self.text_sel_changed(&before, cx);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let sec = ks.modifiers.secondary();
        let scroll = self.scroll(cx);
        let line = 40.0 * self.z();
        let page = (f32::from(scroll.bounds().size.height) - line).max(line);
        let by = |dy: f32| {
            let at = scroll.offset();
            scroll.set_offset(point(at.x, at.y - px(dy)));
        };
        match ks.key.as_str() {
            "c" if sec && !ks.modifiers.shift => match self.selected_copy() {
                Some(copy) => cx.write_to_clipboard(ai_text::formatted_item(copy.plain, &copy.markdown)),
                None => return,
            },
            "a" if sec => self.select_all(cx),
            "escape" if self.sel.selection.is_some() => self.unselect(cx),
            "up" => by(-line),
            "down" => by(line),
            "pageup" => by(-page),
            "pagedown" | "space" => by(page),
            "home" => scroll.set_offset(point(px(0.0), px(0.0))),
            "end" => scroll.scroll_to_bottom(),
            _ => return,
        }
        self.page.update(cx, |_, cx| cx.notify());
        cx.stop_propagation();
    }

    /// Shows "Copied" on the button `key` (`copy-code-<entry>-<piece>`) for a moment.
    fn copied(&mut self, key: SharedString, cx: &mut Context<Self>) {
        let piece = key.rsplit('-').next().and_then(|j| j.parse::<usize>().ok());
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                if let Some(j) = piece {
                    this.redraw_piece(j, cx);
                }
            })
            .ok();
        });
        self.copied = Some((key, task));
        if let Some(j) = piece {
            self.redraw_piece(j, cx);
        }
    }

    /// A diagram or math, drawn (starting to draw it if needed).
    fn render_drawing(&mut self, j: usize, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let piece = &self.doc.pieces[j];
        let source = piece.text.text.clone();
        let kind = match &piece.kind {
            Kind::Diagram(k) => Some(*k),
            _ => None,
        };
        let colors = self.colors(ui);
        let z = self.z();
        let font_px = 14.0 * z;
        let key = hash_of((kind, &source, colors, font_px.to_bits()));
        let fit: Fit = ((MAX_WIDTH * z).round() as u32, (self.scale * 100.0).round() as u32);
        self.visual_at.insert(key, j);
        let visual = self.visuals.entry(key).or_default();
        let wanted = visual.have.as_ref().is_none_or(|(f, _)| *f != fit);
        if wanted && visual.drawing.as_ref().is_none_or(|(f, _)| *f != fit) {
            let (max_width, scale) = (fit.0 as f32, fit.1 as f32 / 100.0);
            let src = source.clone();
            let task = cx.spawn(async move |this, cx| {
                let made = cx
                    .background_executor()
                    .spawn(async move {
                        let what = match kind {
                            Some(k) => shed_markdown::render::Source::Diagram(k, &src),
                            None => shed_markdown::render::Source::Math(&src),
                        };
                        shed_markdown::render::picture(what, &colors, font_px, max_width, scale)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let drawn = match made {
                        Ok(r) => {
                            // GPUI's pictures are BGRA.
                            let mut bgra = r.rgba;
                            for p in bgra.chunks_exact_mut(4) {
                                p.swap(0, 2);
                            }
                            match image::RgbaImage::from_raw(r.width, r.height, bgra) {
                                Some(buffer) => Drawn::Picture(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])), r.size),
                                None => Drawn::Failed("could not be drawn".into()),
                            }
                        }
                        Err(e) => Drawn::Failed(e),
                    };
                    let Some(v) = this.visuals.get_mut(&key) else { return };
                    v.drawing = None;
                    if let Some((_, Drawn::Picture(old, _))) = v.have.replace((fit, drawn)) {
                        cx.drop_image(old, None);
                    }
                    if let Some(j) = this.visual_at.get(&key).copied() {
                        this.redraw_piece(j, cx);
                    }
                    this.page.update(cx, |page, cx| {
                        page.measure = true;
                        cx.notify();
                    });
                })
                .ok();
            });
            visual.drawing = Some((fit, task));
        }
        let have = visual.have.as_ref().map(|(_, d)| match d {
            Drawn::Picture(image, size) => Ok((image.clone(), *size)),
            Drawn::Failed(why) => Err(why.clone()),
        });
        match have {
            Some(Ok((image, (w, h)))) => {
                // At most its own size, narrower with the window.
                let picture = div().w_full().max_w(px(w)).aspect_ratio(w / h.max(1.0)).child(img(image).size_full());
                Some(
                    div()
                        .flex()
                        .justify_center()
                        .when(kind.is_some(), |d| d.p_2().rounded_md().border_1().border_color(hsla(ui.divider)))
                        .child(picture)
                        .into_any_element(),
                )
            }
            // The source, as a code block, with why it is not drawn.
            Some(Err(why)) => {
                let why = format!("{} not drawn: {why}", if kind.is_some() { "Diagram" } else { "Math" });
                let (doc, mono) = (self.doc.clone(), self.settings.font.family.clone());
                let code = md_draw::render_code(self, 0, j, &doc, ui, &mono, cx);
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(code)
                        .child(div().flex().items_center().gap_1().text_size(px(11.5 * z)).text_color(hsla(ui.danger)).child(svg().path(icon("alert")).size(px(12.0 * z)).text_color(hsla(ui.danger))).child(why))
                        .into_any_element(),
                )
            }
            None => Some(div().py_2().text_size(px(11.5 * z)).text_color(hsla(ui.text_dim)).child(if kind.is_some() { "Drawing the diagram…" } else { "Setting the math…" }).into_any_element()),
        }
    }

    /// A picture of the file's (read if needed), fitted to the width.
    fn render_picture(&mut self, j: usize, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Kind::Image { url, link } = &self.doc.pieces[j].kind else { return None };
        let (url, link, alt) = (url.clone(), link.clone(), self.doc.pieces[j].text.text.clone());
        let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let path = resolve_link(&dir, &url).map(|(p, _)| p);
        let z = self.z();
        let missing = |why: String| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .py_1()
                .text_size(px(12.0 * z))
                .text_color(hsla(ui.text_dim))
                .child(svg().path(icon("file")).size(px(13.0 * z)).text_color(hsla(ui.text_dim)))
                .child(if alt.is_empty() { why } else { format!("{alt} ({why})") })
                .into_any_element()
        };
        let Some(path) = path.filter(|_| self.local) else { return Some(missing(format!("{url} is not shown here"))) };
        let state = self.pictures.entry(path.clone()).or_insert_with(|| {
            let read_path = path.clone();
            Pic::Reading {
                _task: cx.spawn(async move |this, cx| {
                    let reading = read_path.clone();
                    let picture = cx.background_executor().spawn(async move { crate::picture::read(&reading) }).await;
                    this.update(cx, |this, cx| {
                        this.pictures.insert(read_path, Pic::Read(picture));
                        this.redraw_piece(j, cx);
                        this.page.update(cx, |page, cx| {
                            page.measure = true;
                            cx.notify();
                        });
                    })
                    .ok();
                }),
            }
        });
        let picture = match state {
            Pic::Reading { .. } => return Some(div().h(px(24.0)).into_any_element()),
            Pic::Read(None) => return Some(missing(format!("{url} cannot be read"))),
            Pic::Read(Some(p)) => p.clone(),
        };
        let el = match picture.size {
            Some((w, h)) if w > 0 && h > 0 => div().w_full().max_w(px(w as f32)).aspect_ratio(w as f32 / h as f32).child(img(picture.image.clone()).size_full()),
            _ => div().w_full().child(img(picture.image.clone()).max_w_full()),
        };
        let mut holder = div().id(("md-picture", j)).flex().child(el);
        if let Some(link) = link {
            holder = holder.cursor_pointer().on_click(cx.listener(move |this, _, window, cx| this.open_link(&link, window, cx)));
        }
        Some(holder.into_any_element())
    }

    fn render_group(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(test)]
        tests::DRAWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let ui = current_ui(cx);
        let Some(range) = self.groups.get(ix).cloned() else { return div().into_any_element() };
        let doc = self.doc.clone();
        let mono = self.settings.font.family.clone();
        let first = range.start;
        let body = md_draw::render_range(self, 0, &doc, range, &ui, &mono, cx);
        let z = self.z();
        // Headings stand apart from what is above them.
        let heading = matches!(doc.pieces.get(first).map(|p| &p.kind), Some(Kind::Heading(_))) && ix > 0;
        div()
            .w_full()
            .flex()
            .justify_center()
            .px(px(PAD))
            .pb(px(10.0 * z))
            .when(heading, |d| d.pt(px(10.0 * z)))
            .when(ix == 0, |d| d.pt(px(PAD)))
            .when(ix + 1 == self.groups.len(), |d| d.pb(px(PAD * 2.0)))
            .child(div().w_full().min_w_0().max_w(px(MAX_WIDTH * z)).child(body))
            .into_any_element()
    }
}

impl PieceHost for MarkdownView {
    fn text_sel(&self) -> &TextSel {
        &self.sel
    }

    fn text_sel_mut(&mut self) -> &mut TextSel {
        &mut self.sel
    }

    fn accent(&self) -> Rgb {
        self.accent
    }

    fn zoom(&self) -> f32 {
        self.z()
    }

    /// Only the rows whose selection or hovered link changed are drawn
    /// again: a selection's ends, and what it took in or let go of.
    fn text_sel_changed(&mut self, before: &TextSel, cx: &mut Context<Self>) {
        let mut pieces: Vec<Range<usize>> = Vec::new();
        for (_, j, _) in [&before.hover_link, &self.sel.hover_link].into_iter().flatten() {
            pieces.push(*j..*j + 1);
        }
        if before.selection != self.sel.selection {
            let span = |s: Option<Selection>| {
                s.filter(|s| !s.is_empty()).map(|s| {
                    let (a, b) = s.ordered();
                    a.piece..b.piece + 1
                })
            };
            match (span(before.selection), span(self.sel.selection)) {
                (Some(a), Some(b)) => {
                    for j in [a.start, a.end - 1, b.start, b.end - 1] {
                        pieces.push(j..j + 1);
                    }
                    pieces.push(a.start.min(b.start)..a.start.max(b.start));
                    pieces.push(a.end.min(b.end)..a.end.max(b.end));
                }
                (Some(r), None) | (None, Some(r)) => pieces.push(r),
                (None, None) => {}
            }
        }
        self.redraw_pieces(pieces, cx);
    }

    /// `#anchor` scrolls to it, a file of the project opens there, anything
    /// else goes to the browser.
    fn open_link(&mut self, url: &str, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(anchor) = url.strip_prefix('#') {
            self.go_to = Some(percent_decode(anchor));
            self.follow_anchor(cx);
            return;
        }
        let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        match resolve_link(&dir, url) {
            Some((path, anchor)) => cx.emit(MdEvent::OpenFile(path, anchor)),
            None => crate::links::open_external(url, cx),
        }
    }

    fn copy_button(&self, key: SharedString, item: ClipboardItem, ui: &UiColors, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let done = self.copied.as_ref().is_some_and(|(k, _)| *k == key);
        div()
            .id(key.clone())
            .flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(11.0 * self.z()))
            .text_color(hsla(ui.text_dim))
            .hover(|s| s.bg(hsla(ui.tab_hover)).text_color(hsla(ui.text)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(item.clone());
                this.copied(key.clone(), cx);
            }))
            .child(svg().path(icon("copy")).size(px(12.0)).text_color(hsla(ui.text_dim)))
            .child(if done { "Copied" } else { "Copy" })
    }

    fn heading_size(&self, level: u8) -> f32 {
        match level {
            1 => 26.0,
            2 => 21.0,
            3 => 17.5,
            4 => 15.5,
            _ => 14.0,
        }
    }

    fn heading_rule(&self, level: u8) -> bool {
        level <= 2
    }

    fn render_visual(&mut self, _: usize, j: usize, ui: &UiColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.doc.pieces.get(j).map(|p| &p.kind) {
            Some(Kind::Diagram(_) | Kind::Math) => self.render_drawing(j, ui, cx),
            Some(Kind::Image { .. }) => self.render_picture(j, ui, cx),
            _ => None,
        }
    }
}

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        self.read_source(cx);
        self.follow_anchor(cx);
        let ui = current_ui(cx);
        let empty = self.groups.is_empty();
        div()
            .key_context("MarkdownView")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
                // A click beside the text lets go of the selection.
                this.unselect(cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if std::mem::take(&mut this.sel.selecting) && this.sel.dragged {
                    // X11 convention: selecting copies to the primary selection.
                    #[cfg(target_os = "linux")]
                    if let Some(copy) = this.selected_copy() {
                        cx.write_to_primary(ClipboardItem::new_string(copy.plain));
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = &cx;
                }
            }))
            // On the window's own ground (the theme's background, as the
            // editor): a ground of its own would be another window-sized layer.
            .size_full()
            .text_color(hsla(ui.text))
            .text_size(px(14.0 * self.z()))
            .when(empty, |d| d.flex().items_center().justify_center().text_color(hsla_alpha(ui.text_dim, 0.8)).child("Nothing to show yet: the file is empty"))
            .when(!empty, |d| d.child(self.page.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows drawn, for the perf test.
    pub static DRAWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn open<'a>(text: &str, cx: &'a mut gpui::TestAppContext) -> (Entity<MarkdownView>, Entity<TextView>, &'a mut gpui::VisualTestContext) {
        cx.update(|cx| {
            crate::theme::set_ui_scale(1.0);
            cx.set_global(crate::prompt::CurrentUi(UiColors::default()));
        });
        let settings = Settings::default();
        let text = text.to_string();
        let mut editor = None;
        let (view, cx) = cx.add_window_view(|_, cx| {
            let source = cx.new(|cx| TextView::new(&text, crate::text_view::TextOptions::default(), &settings, cx));
            editor = Some(source.clone());
            MarkdownView::new(PathBuf::from("/nowhere/README.md"), source, Rgb::hex(0xFF8A57), settings.clone(), true, cx)
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(700.0)));
        cx.run_until_parked();
        (view, editor.unwrap(), cx)
    }

    /// What a piece of each kind came out as.
    fn drawn_kinds(view: &Entity<MarkdownView>, cx: &mut gpui::VisualTestContext) -> Vec<String> {
        view.update(cx, |v, _| {
            v.doc
                .pieces
                .iter()
                .filter(|p| matches!(p.kind, Kind::Diagram(_) | Kind::Math))
                .map(|p| {
                    let kind = match p.kind {
                        Kind::Diagram(d) => d.lang().to_string(),
                        _ => "math".into(),
                    };
                    let key = v.visual_at.iter().find(|(_, j)| v.doc.pieces[**j].text.text == p.text.text).map(|(k, _)| *k);
                    match key.and_then(|k| v.visuals.get(&k)).and_then(|vis| vis.have.as_ref()) {
                        Some((_, Drawn::Picture(_, (w, h)))) => format!("{kind}: {w}x{h}"),
                        Some((_, Drawn::Failed(why))) => format!("{kind} failed: {why}"),
                        None => format!("{kind}: not drawn"),
                    }
                })
                .collect()
        })
    }

    /// A document drawn: diagrams and math off the UI thread, a broken
    /// diagram as its source with why; edits in the editor show at once.
    #[gpui::test]
    fn a_document_is_drawn_and_follows_its_editor(cx: &mut gpui::TestAppContext) {
        let filler = "More words.\n\n".repeat(30);
        let md = format!("# Title\n\n```dot\ndigraph {{ a -> b }}\n```\n\n```mermaid\nflowchart TD\n  A -->\n  ]]] B{{\n```\n\n$$\nx^2\n$$\n\n{filler}## Status\n\nDone.\n\n{filler}");
        let (view, editor, cx) = open(&md, cx);
        // Drawn on the background executor, then shown.
        for _ in 0..3 {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        }
        let kinds = drawn_kinds(&view, cx);
        assert!(kinds[0].starts_with("dot: "), "{kinds:?}");
        assert!(kinds[1].starts_with("mermaid failed: ") && !kinds[1].contains('\n'), "{kinds:?}");
        assert!(kinds[2].starts_with("math: "), "{kinds:?}");
        // The broken one stays a code block of its source.
        view.update(cx, |v, _| {
            let j = v.doc.pieces.iter().position(|p| matches!(p.kind, Kind::Diagram(shed_markdown::DiagramKind::Mermaid))).unwrap();
            assert!(v.doc.pieces[j].text.text.starts_with("flowchart TD"));
            assert_eq!(v.groups.len(), 66);
        });
        // A link to a heading scrolls to it.
        cx.update(|window, cx| view.update(cx, |v, cx| v.open_link("#status", window, cx)));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let (top, at) = view.update(cx, |v, cx| {
            let page = v.page.read(cx);
            (page.top_of(34), -f32::from(page.scroll.offset().y))
        });
        assert!(top > 0.0 && (top - at).abs() < 1.0, "the heading at {top}, the view at {at}");
        // Unsaved edits in the editor show in the View.
        editor.update(cx, |e, cx| {
            e.buffer.set_cursor(0, false);
            e.buffer.insert("Intro **bold**\n\n");
            cx.notify();
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        view.update(cx, |v, _| {
            assert!(matches!(&v.doc.pieces[0], Piece { kind: Kind::Para, text } if text.text == "Intro bold"));
            assert_eq!(v.groups.len(), 67);
        });
        // Selecting everything and copying gives the Markdown back.
        view.update(cx, |v, cx| v.select_all(cx));
        let copy = view.update(cx, |v, _| v.selected_copy()).unwrap();
        assert!(copy.markdown.starts_with("Intro **bold**\n\n# "), "{}", copy.markdown);
        assert!(copy.markdown.contains("```dot\ndigraph { a -> b }\n```"), "{}", copy.markdown);
        assert!(copy.markdown.contains("$$\nx^2\n$$"), "{}", copy.markdown);
    }

    /// SHED_PERF_DOC=<file.md> cargo test -p shed-app --bins perf_long_document -- --ignored --nocapture
    #[gpui::test]
    #[ignore]
    fn perf_long_document(cx: &mut gpui::TestAppContext) {
        let path = std::env::var("SHED_PERF_DOC").expect("SHED_PERF_DOC=<file.md>");
        let text = std::fs::read_to_string(&path).unwrap();
        let (view, _, cx) = open(&text, cx);
        let timed = |cx: &mut gpui::VisualTestContext, label: &str, n: u32, poke: &dyn Fn(&mut gpui::VisualTestContext)| {
            DRAWN.store(0, std::sync::atomic::Ordering::Relaxed);
            let started = std::time::Instant::now();
            for _ in 0..n {
                poke(cx);
                cx.run_until_parked();
            }
            println!("{label}: {:?} per frame, {} rows drawn per frame", started.elapsed() / n, DRAWN.load(std::sync::atomic::Ordering::Relaxed) as u32 / n);
        };
        timed(cx, "first frames", 3, &|cx| cx.update(|window, _| window.refresh()));
        let rows = view.update(cx, |v, _| v.rows.len());
        println!("{rows} rows");
        timed(cx, "full redraw (window refreshed)", 10, &|cx| cx.update(|window, _| window.refresh()));
        let page = view.update(cx, |v, _| v.page.clone());
        timed(cx, "scrolled (the page notified)", 30, &|cx| page.update(cx, |p, cx| {
            let at = p.scroll.offset();
            p.scroll.set_offset(point(at.x, at.y - px(40.0)));
            cx.notify();
        }));
        timed(cx, "hover on a link (one row notified)", 30, &|cx| view.update(cx, |v, cx| v.redraw_piece(3, cx)));
        timed(cx, "the document changed (the view notified)", 3, &|cx| view.update(cx, |_, cx| cx.notify()));
    }

    #[test]
    fn links_resolve_against_the_files_folder() {
        let dir = Path::new("/p/docs");
        assert_eq!(resolve_link(dir, "guide.md"), Some((PathBuf::from("/p/docs/guide.md"), None)));
        assert_eq!(resolve_link(dir, "../README.md#quick-start"), Some((PathBuf::from("/p/README.md"), Some("quick-start".into()))));
        assert_eq!(resolve_link(dir, "./img/a.png"), Some((PathBuf::from("/p/docs/img/a.png"), None)));
        assert_eq!(resolve_link(dir, "my%20notes.md"), Some((PathBuf::from("/p/docs/my notes.md"), None)));
        assert_eq!(resolve_link(dir, "/etc/hosts"), Some((PathBuf::from("/etc/hosts"), None)));
        assert_eq!(resolve_link(dir, "https://x.org/a.md"), None);
        assert_eq!(resolve_link(dir, "#top"), None);
        assert!(is_markdown(Path::new("README.md")) && is_markdown(Path::new("a.MARKDOWN")) && !is_markdown(Path::new("a.rs")));
    }

    #[test]
    fn lists_tables_and_footnotes_are_rows_of_their_own() {
        let pieces = ai_text::pieces(shed_markdown::parse_doc("# T\n\n- a\n- b\n\n  ```sh\n  ls\n  ```\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\n| p |\n|---|\n| q |\n\nText[^1].\n\n[^1]: Note.\n"));
        let groups = group(&pieces);
        let kinds: Vec<&Kind> = groups.iter().map(|g| &pieces[g.start].kind).collect();
        assert!(matches!(kinds[..], [Kind::Heading(1), Kind::Item { .. }, Kind::Cell { .. }, Kind::Cell { .. }, Kind::Para, Kind::Footnote { .. }]), "{kinds:?}");
        // The list holds its code; each table its cells.
        assert_eq!(groups[1].len(), 3);
        assert_eq!(groups[2].len(), 4);
        assert_eq!(groups[3].len(), 2);
        assert_eq!(groups.last().unwrap().end, pieces.len());
    }

    #[test]
    fn anchors_point_at_their_pieces() {
        let blocks = shed_markdown::parse_doc("| a | b |\n|---|---|\n| 1 | 2 |\n\n## After the table\n");
        let starts = block_starts(&blocks);
        let (name, block) = &shed_markdown::anchors(&blocks)[0];
        assert_eq!(name, "after-the-table");
        let pieces = ai_text::pieces(blocks.clone());
        assert!(matches!(pieces[starts[*block]].kind, Kind::Heading(2)));
    }
}
