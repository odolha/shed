//! An AI tab's review of what Claude changed (`shed-review`): every change
//! by how far it went, its diff, Viewed marks and explanations.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use shed_model::TabId;
use shed_proto::{AiChange, ClientMsg};
use shed_review::{Found, Layer, Scope};

use crate::ai::{AiView, Panel};
use crate::app::App;
use crate::text::{fit, pad, wrap};
use crate::theme::Colors;

pub struct Review {
    pub scope: Scope,
    pub found: Option<Found>,
    pub sel: usize,
    pub diff_scroll: usize,
    /// Changes being explained, by key.
    pub explaining: Vec<String>,
}

impl Review {
    pub fn scroll_diff(&mut self, by: isize) {
        self.diff_scroll = self.diff_scroll.saturating_add_signed(by);
    }
}

/// Where a change stands, as a heading.
fn layer_title(layer: &Layer, found: &Found, repo: usize) -> String {
    let name = if found.repos.len() > 1 { format!("{} · ", found.repos[repo].name) } else { String::new() };
    match layer {
        Layer::Unstaged { .. } => format!("{name}Not staged"),
        Layer::Staged => format!("{name}Staged"),
        Layer::Committed { hash, pushed } => {
            let subject = found.repos[repo].commits.iter().find(|c| c.hash == *hash).map(|c| c.subject.clone()).unwrap_or_default();
            format!("{name}{} {} {subject}", if *pushed { "Pushed" } else { "Committed" }, &hash[..hash.len().min(8)])
        }
    }
}

impl App {
    pub(crate) fn ai_review(&mut self, tab: TabId) {
        let Some(view) = self.ais.get_mut(&tab) else { return };
        let scope = match &view.panel {
            Panel::Review(r) => r.scope,
            _ => Scope::Turn,
        };
        view.panel = Panel::Review(Review { scope, found: None, sel: 0, diff_scroll: 0, explaining: Vec::new() });
        self.review_load(tab);
    }

    fn review_load(&mut self, tab: TabId) {
        let Some(ai) = self.tree.tab(tab).and_then(|t| t.ai_info()).cloned() else { return };
        let Some(AiView { panel: Panel::Review(r), entries, .. }) = self.ais.get_mut(&tab) else { return };
        r.found = None;
        let since = shed_review::review_since(entries, r.scope);
        let dirs = shed_review::dirs(&ai);
        self.spawn(move || {
            let found = shed_review::read(dirs, since);
            move |app: &mut App| {
                if let Some(AiView { panel: Panel::Review(r), .. }) = app.ais.get_mut(&tab) {
                    r.sel = r.sel.min(found.changes.len().saturating_sub(1));
                    r.found = Some(found);
                }
            }
        });
    }

    pub(crate) fn review_key(&mut self, tab: TabId, ev: KeyEvent) {
        let Some(ai) = self.tree.tab(tab).and_then(|t| t.ai_info()).cloned() else { return };
        let Some(AiView { panel: panel @ Panel::Review(_), .. }) = self.ais.get_mut(&tab) else { return };
        let Panel::Review(r) = panel else { return };
        let n = r.found.as_ref().map_or(0, |f| f.changes.len());
        match ev.code {
            KeyCode::Esc | KeyCode::Char('q') => *panel = Panel::Conversation,
            KeyCode::Up | KeyCode::Char('k') => {
                r.sel = r.sel.saturating_sub(1);
                r.diff_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                r.sel = (r.sel + 1).min(n.saturating_sub(1));
                r.diff_scroll = 0;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => r.scroll_diff(10),
            KeyCode::PageUp => r.scroll_diff(-10),
            KeyCode::Char('t') => {
                r.scope = if r.scope == Scope::Turn { Scope::Session } else { Scope::Turn };
                r.sel = 0;
                self.review_load(tab);
            }
            KeyCode::Char('g') | KeyCode::Char('r') => self.review_load(tab),
            KeyCode::Char('v') | KeyCode::Enter => {
                let Some(found) = &r.found else { return };
                let Some(c) = found.changes.get(r.sel) else { return };
                let key = found.key(c);
                let viewed = !found.is_viewed(c, &ai.viewed);
                let hash = c.hash.clone();
                // Viewed: on to the next one.
                if viewed {
                    r.sel = (r.sel + 1).min(n.saturating_sub(1));
                    r.diff_scroll = 0;
                }
                self.send(ClientMsg::AiChange { tab, change: AiChange::Viewed { key, hash, viewed } });
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                let Some(found) = &r.found else { return };
                let Some(session) = ai.session.clone() else {
                    self.note("explaining needs a session with Claude");
                    return;
                };
                let which: Vec<usize> = if ev.code == KeyCode::Char('E') {
                    (0..n).filter(|i| found.explanation(&found.changes[*i], &ai.explained).is_none_or(|(_, current)| !current)).collect()
                } else {
                    vec![r.sel]
                };
                if which.is_empty() {
                    return;
                }
                let request = found.explain_request(&which);
                r.explaining.extend(request.keys());
                let (folder, model) = (ai.folder.clone(), ai.model.clone());
                self.note("asking Claude to explain…");
                self.spawn(move || {
                    let answer = request.ask(&folder, &session, model.as_deref());
                    move |app: &mut App| {
                        if let Some(AiView { panel: Panel::Review(r), .. }) = app.ais.get_mut(&tab) {
                            r.explaining.retain(|k| !request.keys().contains(k));
                        }
                        match answer {
                            Ok(a) => app.send(ClientMsg::AiChange { tab, change: AiChange::Explained(request.answered(a)) }),
                            Err(e) => app.error(format!("could not explain: {e}")),
                        }
                    }
                });
            }
            _ => {}
        }
    }

    pub(crate) fn render_review(&mut self, f: &mut Frame, area: Rect, tab: TabId) {
        let c = self.colors.clone();
        let Some(ai) = self.tree.tab(tab).and_then(|t| t.ai_info()).cloned() else { return };
        let Some(AiView { panel: Panel::Review(r), .. }) = self.ais.get_mut(&tab) else { return };
        let scope = if r.scope == Scope::Turn { "this turn" } else { "this session" };
        let head = Line::from(vec![
            Span::styled(format!(" Review: {scope} "), c.chip(c.accent)),
            Span::styled("  ↑↓ change · v viewed · e explain (E all) · t turn/session · r reread · PgUp/PgDn diff · Esc back", c.dim()),
        ]);
        f.render_widget(Paragraph::new(head), Rect { height: 1, ..area });
        let body = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
        let Some(found) = &r.found else {
            f.render_widget(Paragraph::new(Line::styled("  reading the repositories…", c.dim())), body);
            return;
        };
        if found.changes.is_empty() {
            let why = if found.repos.is_empty() { "no Git repository in the tab's folders" } else { "nothing changed" };
            f.render_widget(Paragraph::new(Line::styled(format!("  {why}"), c.dim())), body);
            return;
        }
        let list_w = (body.width / 3).clamp(24, 50).min(body.width);
        let list = Rect { width: list_w, ..body };
        let diff = Rect { x: body.x + list_w, width: body.width - list_w, ..body };
        // The list, with a heading where the layer changes.
        let mut lines: Vec<Line> = Vec::new();
        let mut sel_line = 0;
        let mut last: Option<(usize, Layer)> = None;
        for (i, ch) in found.changes.iter().enumerate() {
            if last.as_ref() != Some(&(ch.repo, ch.layer.clone())) {
                lines.push(Line::styled(fit(&layer_title(&ch.layer, found, ch.repo), list_w as usize), c.accent().add_modifier(Modifier::BOLD)));
                last = Some((ch.repo, ch.layer.clone()));
            }
            let viewed = found.is_viewed(ch, &ai.viewed);
            let explained = found.explanation(ch, &ai.explained).is_some();
            let mark = if viewed { "✓" } else { " " };
            let note = if r.explaining.contains(&found.key(ch)) { "…" } else if explained { "¶" } else { " " };
            let text = pad(&format!("{mark}{note}{} {}", ch.code, ch.path), list_w as usize - 1);
            let style = if i == r.sel { c.selected() } else if viewed { c.dim() } else { Style::new() };
            if i == r.sel {
                sel_line = lines.len();
            }
            lines.push(Line::styled(text, style));
        }
        let h = list.height as usize;
        let top = sel_line.saturating_sub(h.saturating_sub(2));
        f.render_widget(Paragraph::new(lines.into_iter().skip(top).take(h).collect::<Vec<_>>()), list);
        // The selected change.
        let Some(ch) = found.changes.get(r.sel) else { return };
        let block = Block::default().borders(Borders::LEFT).border_style(c.fg(c.divider));
        let inner = block.inner(diff);
        f.render_widget(block, diff);
        let w = inner.width as usize;
        let mut lines: Vec<Line> = vec![Line::styled(fit(&ch.path, w), Style::new().add_modifier(Modifier::BOLD))];
        if let Some((text, current)) = found.explanation(ch, &ai.explained) {
            let stale = if current { "" } else { " (of an earlier version)" };
            lines.push(Line::styled(format!("Explanation{stale}"), c.accent()));
            lines.extend(crate::md::lines(&text, w, &c));
            lines.push(Line::default());
        }
        if let Some((old, new)) = &ch.pictures {
            let d = |p: &Option<shed_project::picture::PictureData>| p.as_ref().map_or("none".to_string(), |p| p.describe());
            lines.push(Line::raw(format!("Picture: before {}, now {}", d(old), d(new))));
        }
        hunk_lines(&ch.hunks, None, w, &c, &mut lines);
        r.diff_scroll = r.diff_scroll.min(lines.len().saturating_sub(1));
        f.render_widget(Paragraph::new(lines.into_iter().skip(r.diff_scroll).take(inner.height as usize).collect::<Vec<_>>()), inner);
    }
}

/// Hunks as lines: a heading each, then removed and added lines in colour;
/// the hunk `selected` is marked in the margin.
pub fn hunk_lines(hunks: &[shed_git::Hunk], selected: Option<usize>, w: usize, c: &Colors, out: &mut Vec<Line<'static>>) {
    for (n, h) in hunks.iter().enumerate() {
        let mark = if selected == Some(n) { "▌" } else { " " };
        out.push(Line::from(vec![Span::styled(mark, c.accent()), Span::styled(h.header(), c.fg(c.ansi[6]))]));
        for l in &h.lines {
            let (sign, style) = match l.kind {
                shed_git::LineKind::Added => ('+', c.added()),
                shed_git::LineKind::Removed => ('-', c.removed()),
                shed_git::LineKind::Context => (' ', Style::new()),
            };
            let num = l.new.or(l.old).map_or(String::new(), |n| n.to_string());
            out.push(Line::from(vec![
                Span::styled(mark, c.accent()),
                Span::styled(format!("{num:>5} "), c.dim()),
                Span::styled(format!("{sign}{}", fit(&l.text.replace('\t', "    "), w.saturating_sub(8))), style),
            ]));
        }
    }
    if hunks.is_empty() {
        out.extend(wrap(vec![Span::styled("no differences in text", c.dim())], w, vec![], vec![]));
    }
}
