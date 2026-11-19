//! What the Changes panel and the AI tab's review share, so they look and
//! behave alike: diff rows drawn (laid out by `shed_git::layout`, unified or
//! side by side), the colour of Git's status letters, and changed files by
//! folder (`folder_rows`, from there too).

use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, prelude::*, px};
use shed_git::{Hunk, LineKind};
pub use shed_git::layout::{DiffRow, FolderRow, diff_layout, folder_rows};
use shed_model::Rgb;

use crate::theme::{UiColors, hsla, hsla_alpha};

/// A diff row's height, at zoom 1.
pub const DIFF_ROW: f32 = 20.0;

/// A diff row drawn: `base` is the row's element (its id and height set by
/// the caller), `actions` what goes at the right of a hunk's header.
pub fn render_diff_row(base: gpui::Stateful<Div>, row: &DiffRow, hunks: &[Hunk], ui: &UiColors, accent: Rgb, actions: Option<AnyElement>) -> AnyElement {
    let (red, green) = (Rgb::hex(0xE5534B), Rgb::hex(0x57AB5A));
    let num = |n: Option<usize>| div().flex_none().w(px(40.0)).text_color(hsla(ui.text_dim)).child(n.map(|n| n.to_string()).unwrap_or_default());
    let cell = |line: Option<&shed_git::Line>| {
        let (bg, sign) = match line.map(|l| l.kind) {
            Some(LineKind::Added) => (Some(green), "+"),
            Some(LineKind::Removed) => (Some(red), "-"),
            Some(LineKind::Context) => (None, " "),
            None => (Some(ui.divider), " "),
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .when_some(bg, |d, c| d.bg(hsla_alpha(c, 0.15)))
            .child(num(line.and_then(|l| l.old.or(l.new))))
            .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(sign))
            .child(div().whitespace_nowrap().child(line.map(|l| l.text.clone()).unwrap_or_default()))
    };
    match row {
        DiffRow::Hunk(h) => base
            .gap_2()
            .px_2()
            .bg(hsla_alpha(accent, 0.08))
            .child(div().flex_1().text_color(hsla(ui.text_dim)).child(hunks[*h].header()))
            .children(actions)
            .into_any_element(),
        DiffRow::Line(h, l) => {
            let line = &hunks[*h].lines[*l];
            let (bg, sign) = match line.kind {
                LineKind::Added => (Some(green), "+"),
                LineKind::Removed => (Some(red), "-"),
                LineKind::Context => (None, " "),
            };
            base.when_some(bg, |d, c| d.bg(hsla_alpha(c, 0.15)))
                .child(num(line.old))
                .child(num(line.new))
                .child(div().flex_none().w(px(12.0)).text_color(hsla(ui.text_dim)).child(sign))
                .child(div().whitespace_nowrap().child(line.text.clone()))
                .into_any_element()
        }
        DiffRow::Pair(a, b) => {
            let left = a.map(|(h, l)| &hunks[h].lines[l]);
            let right = b.map(|(h, l)| &hunks[h].lines[l]);
            base.child(cell(left)).child(div().flex_none().w(px(1.0)).h_full().bg(hsla(ui.divider))).child(cell(right)).into_any_element()
        }
    }
}

/// The colour of Git's letter for a change: added green, deleted or in
/// conflict red, renamed or copied blue, else (changed) amber.
pub fn status_color(code: char, ui: &UiColors) -> Rgb {
    match code {
        'A' | '?' => Rgb::hex(0x57AB5A),
        'D' | 'U' => ui.danger,
        'R' | 'C' => Rgb::hex(0x539BF5),
        _ => Rgb::hex(0xC69026),
    }
}
