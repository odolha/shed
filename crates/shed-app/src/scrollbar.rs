//! Scrollbars, which GPUI does not draw: a thumb on the right edge of
//! whatever scrolls (lists, scrollable boxes, the editor, terminals). Drag
//! the thumb, or press the track to bring the thumb there and drag on.
//!
//! It is painted directly (one small quad, two when hovered), and listens
//! to the mouse at the window, so a drag goes on outside the bar.
//!
//! Scrolling with the wheel also brings up two small buttons above the
//! bar's foot, **Top** and **End**, for a few seconds.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, DispatchPhase, IntoElement, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, ScrollHandle, ScrollWheelEvent, SharedString, Styled, TextAlign, TextRun, TransformationMatrix,
    UniformListScrollHandle, Window, canvas, point, px, size,
};

use crate::theme::{UiColors, hsla_alpha};

/// How wide the bar is, and the thumb's shortest length.
pub const WIDTH: f32 = 10.0;
const MIN_THUMB: f32 = 28.0;

/// Where the content is: scrolled `offset` of at most `max`, showing
/// `viewport` (all in the same unit: pixels or lines).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub offset: f32,
    pub max: f32,
    pub viewport: f32,
}

/// A custom source's metrics, read when painting.
pub type ReadMetrics = Rc<dyn Fn(&App) -> Option<Metrics>>;
/// A custom source's way to scroll to an offset.
pub type SetOffset = Rc<dyn Fn(f32, &mut Window, &mut App)>;

/// What a scrollbar reads and moves.
#[derive(Clone)]
pub enum Source {
    /// A scrollable box (`overflow_y_scroll` with `track_scroll`).
    Handle(ScrollHandle),
    /// GPUI's `list`.
    List(ListState),
    /// Anything else: its metrics, and a way to scroll it to an offset.
    Custom { metrics: ReadMetrics, set: SetOffset },
}

impl Source {
    pub fn uniform(handle: &UniformListScrollHandle) -> Self {
        Source::Handle(handle.0.borrow().base_handle.clone())
    }

    fn metrics(&self, cx: &App) -> Option<Metrics> {
        match self {
            Source::Handle(h) => {
                let max = f32::from(h.max_offset().y);
                Some(Metrics { offset: -f32::from(h.offset().y), max, viewport: f32::from(h.bounds().size.height) })
            }
            Source::List(l) => Some(Metrics {
                offset: -f32::from(l.scroll_px_offset_for_scrollbar().y),
                max: f32::from(l.max_offset_for_scrollbar().y),
                viewport: f32::from(l.viewport_bounds().size.height),
            }),
            Source::Custom { metrics, .. } => metrics(cx),
        }
    }

    fn set(&self, offset: f32, window: &mut Window, cx: &mut App) {
        match self {
            Source::Handle(h) => {
                let x = h.offset().x;
                h.set_offset(point(x, px(-offset)));
            }
            Source::List(l) => l.set_offset_from_scrollbar(point(px(0.0), px(-offset))),
            Source::Custom { set, .. } => set(offset, window, cx),
        }
    }

    fn drag(&self, started: bool) {
        if let Source::List(l) = self {
            if started { l.scrollbar_drag_started() } else { l.scrollbar_drag_ended() }
        }
    }
}

/// A scrollbar's own state, kept by its view across frames.
#[derive(Clone, Default)]
pub struct State {
    /// While dragging: where in the thumb it was taken.
    grab: Rc<Cell<Option<f32>>>,
    hovered: Rc<Cell<bool>>,
    /// The offset at the last paint, to see the wheel move this content.
    last_offset: Rc<Cell<Option<f32>>>,
    /// Until when the Top and End buttons show.
    jumps_until: Rc<Cell<Option<Instant>>>,
}

/// How long the Top and End buttons stay after the wheel stops.
const JUMPS_FOR: Duration = Duration::from_millis(2500);

thread_local! {
    /// When the wheel last turned anywhere: a bar whose content moved just
    /// then was scrolled by it (not by new output or a following list).
    static LAST_WHEEL: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn wheel_just_turned() -> bool {
    LAST_WHEEL.with(|w| w.get().is_some_and(|t| t.elapsed() < Duration::from_millis(400)))
}

/// The thumb's place on a track `track` long: its top and length.
fn thumb(m: Metrics, track: f32) -> (f32, f32) {
    let len = (track * m.viewport / (m.viewport + m.max)).clamp(MIN_THUMB.min(track), track);
    let top = if m.max > 0.0 { (track - len) * (m.offset / m.max).clamp(0.0, 1.0) } else { 0.0 };
    (top, len)
}

/// The offset that puts the thumb's top at `top`.
fn offset_for(m: Metrics, track: f32, len: f32, top: f32) -> f32 {
    let room = (track - len).max(1.0);
    (top / room).clamp(0.0, 1.0) * m.max
}

/// A vertical scrollbar filling its bounds (put it at the right edge of
/// what scrolls: `absolute().top_0().right_0().bottom_0()`). Shows only
/// when there is something to scroll; with `quiet`, only while the mouse is
/// over it, it is dragged or the content is not at its end (terminals).
pub fn vertical(source: Source, state: &State, ui: &UiColors, quiet: bool) -> impl IntoElement + Styled {
    vertical_with_blocks(source, state, ui, quiet, None)
}

/// Goes to the previous (`true`) or next block: message, command.
pub type Step = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A scrollbar whose content has blocks (messages, commands): its buttons
/// also go to the previous and next one.
pub fn vertical_with_blocks(source: Source, state: &State, ui: &UiColors, quiet: bool, step: Option<Step>) -> impl IntoElement + Styled {
    let state = state.clone();
    let ui = ui.clone();
    let color = ui.text;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, cx: &mut App| {
            let Some(m) = source.metrics(cx).filter(|m| m.max > 0.5 && m.viewport > 0.0) else { return };
            let track = f32::from(bounds.size.height);
            let (top, len) = thumb(m, track);
            let dragging = state.grab.get().is_some();
            let at_end = m.offset >= m.max - 0.5;
            let show = !quiet || dragging || state.hovered.get() || !at_end;
            if show {
                let strong = dragging || state.hovered.get();
                let x = bounds.origin.x + px(if strong { 2.0 } else { 3.0 });
                let w = bounds.size.width - px(if strong { 4.0 } else { 6.0 });
                let thumb_bounds = Bounds::new(point(x, bounds.origin.y + px(top)), size(w, px(len)));
                window.paint_quad(PaintQuad::new(thumb_bounds).corner_radii(w / 2.0).background(hsla_alpha(color, if strong { 0.55 } else { 0.3 })));
            }

            // The wheel moved this content: the jump buttons come up.
            let moved = state.last_offset.replace(Some(m.offset)).is_some_and(|o| (o - m.offset).abs() > 0.5);
            if moved && wheel_just_turned() && !dragging {
                let fresh = state.jumps_until.get().is_none_or(|t| t <= Instant::now());
                state.jumps_until.set(Some(Instant::now() + JUMPS_FOR));
                if fresh {
                    // Paint once more when they are due to go.
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(JUMPS_FOR + Duration::from_millis(50)).await;
                        cx.update(|cx| cx.refresh_windows());
                    })
                    .detach();
                }
            }
            window.on_mouse_event(|_: &ScrollWheelEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    LAST_WHEEL.with(|w| w.set(Some(Instant::now())));
                }
            });
            if state.jumps_until.get().is_some_and(|t| t > Instant::now()) {
                paint_jumps(&source, m, bounds, &ui, &state, step.as_ref(), window, cx);
            }

            let (s, st) = (source.clone(), state.clone());
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !bounds.contains(&e.position) {
                    return;
                }
                let y = f32::from(e.position.y - bounds.origin.y);
                // On the thumb: take it there; on the track: bring it here.
                let grab = if y >= top && y <= top + len {
                    y - top
                } else {
                    let to = offset_for(m, track, len, y - len / 2.0);
                    s.set(to, window, cx);
                    st.last_offset.set(Some(to));
                    len / 2.0
                };
                st.grab.set(Some(grab));
                s.drag(true);
                cx.stop_propagation();
                window.refresh();
            });
            let (s, st) = (source.clone(), state.clone());
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                match st.grab.get() {
                    Some(grab) if e.pressed_button == Some(MouseButton::Left) => {
                        let y = f32::from(e.position.y - bounds.origin.y);
                        let to = offset_for(m, track, len, y - grab);
                        s.set(to, window, cx);
                        st.last_offset.set(Some(to));
                        cx.stop_propagation();
                        window.refresh();
                    }
                    _ => {
                        let over = bounds.contains(&e.position);
                        if st.hovered.replace(over) != over {
                            window.refresh();
                        }
                    }
                }
            });
            let (s, st) = (source.clone(), state.clone());
            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble && st.grab.take().is_some() {
                    s.drag(false);
                    window.refresh();
                }
            });
        },
    )
    .w(px(WIDTH))
}

/// The Top and End buttons, left of the bar near its foot: each a small
/// pill with an arrow and a word; pressing one goes there.
#[allow(clippy::too_many_arguments)]
fn paint_jumps(source: &Source, m: Metrics, bar: Bounds<Pixels>, ui: &UiColors, state: &State, step: Option<&Step>, window: &mut Window, cx: &mut App) {
    let color = ui.text;
    // What a button does: go to an offset, or a block up or down.
    #[derive(Clone, Copy)]
    enum Go {
        To(f32),
        Block(bool),
    }
    let mut buttons: Vec<(&'static str, &'static str, Go)> = Vec::new();
    if m.offset > 1.0 {
        buttons.push(("arrow-up", "Top", Go::To(0.0)));
    }
    if step.is_some() {
        if m.offset > 1.0 {
            buttons.push(("arrow-up", "Prev", Go::Block(true)));
        }
        if m.offset < m.max - 1.0 {
            buttons.push(("arrow-down", "Next", Go::Block(false)));
        }
    }
    if m.offset < m.max - 1.0 {
        buttons.push(("arrow-down", "End", Go::To(m.max)));
    }
    let (h, w, gap) = (px(24.0), px(62.0), px(6.0));
    let right = bar.origin.x - px(6.0);
    let mut y = bar.origin.y + bar.size.height - px(12.0) - h * buttons.len() as f32 - gap * (buttons.len().saturating_sub(1)) as f32;
    let font = window.text_style().font();
    for (glyph, label, go) in buttons {
        let b = Bounds::new(point(right - w, y), size(w, h));
        y += h + gap;
        let ground = PaintQuad::new(b)
            .corner_radii(h / 2.0)
            .background(crate::theme::hsla(ui.tab_active))
            .border_widths(px(1.0))
            .border_color(crate::theme::hsla(ui.divider));
        window.paint_quad(ground);
        let icon_size = px(12.0);
        let icon = Bounds::new(point(b.origin.x + px(9.0), b.origin.y + (h - icon_size) / 2.0), size(icon_size, icon_size));
        let _ = window.paint_svg(icon, crate::assets::icon(glyph), None, TransformationMatrix::unit(), hsla_alpha(color, 0.9), cx);
        let run = TextRun { len: label.len(), font: font.clone(), color: hsla_alpha(color, 0.9), background_color: None, underline: None, strikethrough: None, letter_spacing: None };
        let line = window.text_system().shape_line(SharedString::from(label), px(12.0), &[run]);
        let _ = line.paint(point(b.origin.x + px(25.0), b.origin.y + (h - px(16.0)) / 2.0), px(16.0), TextAlign::Left, None, window, cx);

        let (s, st, step) = (source.clone(), state.clone(), step.cloned());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !b.contains(&e.position) {
                return;
            }
            match go {
                Go::To(to) => {
                    s.set(to, window, cx);
                    // Moved by the button, not the wheel: they need not come back.
                    st.last_offset.set(Some(to));
                    st.jumps_until.set(None);
                }
                // Block by block: the buttons stay for the next press.
                Go::Block(up) => {
                    if let Some(step) = &step {
                        step(up, window, cx);
                    }
                    st.jumps_until.set(Some(Instant::now() + JUMPS_FOR));
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(JUMPS_FOR + Duration::from_millis(50)).await;
                        cx.update(|cx| cx.refresh_windows());
                    })
                    .detach();
                }
            }
            cx.stop_propagation();
            window.refresh();
        });
    }
}

/// `content` (which fills its height) with a scrollbar over its right
/// edge; size the box returned as `content` was sized.
pub fn with_bar(content: impl IntoElement, source: Source, state: &State, ui: &UiColors) -> gpui::Div {
    use gpui::ParentElement;
    gpui::div()
        .relative()
        .min_h_0()
        .flex()
        .flex_col()
        .child(content)
        .child(vertical(source, state, ui, false).absolute().top_0().right_0().bottom_0())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, ListAlignment, Modifiers, ParentElement, Render, div, list};

    struct Rows {
        list: ListState,
        bar: State,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = list(self.list.clone(), |i, _, _| div().h(px(20.0)).child(format!("row {i}")).into_any_element()).flex_1();
            div().w(px(300.0)).h(px(200.0)).flex().flex_col().child(with_bar(rows, Source::List(self.list.clone()), &self.bar, &UiColors::default()).size_full())
        }
    }

    struct Tree {
        scroll: UniformListScrollHandle,
        bar: State,
    }

    impl Render for Tree {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = gpui::uniform_list("rows", 100, |range, _, _| range.map(|i| div().h(px(20.0)).child(format!("row {i}")).into_any_element()).collect())
                .track_scroll(&self.scroll)
                .flex_1();
            div().w(px(300.0)).h(px(200.0)).flex().flex_col().child(with_bar(rows, Source::uniform(&self.scroll), &self.bar, &UiColors::default()).size_full())
        }
    }

    #[gpui::test]
    fn the_mouse_scrolls_a_uniform_list_by_its_bar(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Tree { scroll: UniformListScrollHandle::new(), bar: State::default() });
        cx.run_until_parked();
        let at = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, _| -f32::from(v.scroll.0.borrow().base_handle.offset().y));
        let x = px(300.0 - WIDTH / 2.0);
        cx.simulate_mouse_down(point(x, px(199.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(at(cx) > 1500.0, "{}", at(cx));
        cx.simulate_mouse_move(point(x, px(100.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!((700.0..1100.0).contains(&at(cx)), "{}", at(cx));
        cx.simulate_mouse_up(point(x, px(100.0)), MouseButton::Left, Modifiers::none());
    }

    /// Turning the wheel brings up Top and End; End goes to the end.
    #[gpui::test]
    fn the_wheel_brings_up_top_and_end(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Rows { list: ListState::new(100, ListAlignment::Top, px(100.0)).measure_all(), bar: State::default() });
        cx.run_until_parked();
        let at = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, _| -f32::from(v.list.scroll_px_offset_for_scrollbar().y));
        cx.simulate_event(ScrollWheelEvent { position: point(px(100.0), px(100.0)), delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-300.0))), ..Default::default() });
        cx.run_until_parked();
        let scrolled = at(cx);
        assert!(scrolled > 100.0 && scrolled < 1700.0, "{scrolled}");
        assert!(view.read_with(cx, |v, _| v.bar.jumps_until.get().is_some()), "the buttons are up");
        // End: the lower button, left of the bar near its foot.
        cx.simulate_mouse_down(point(px(250.0), px(176.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(at(cx) > 1790.0, "{}", at(cx));
        assert!(view.read_with(cx, |v, _| v.bar.jumps_until.get().is_none()), "they go once used");
    }

    /// Pressing the track and dragging the thumb scroll a list, with the
    /// mouse alone (no wheel).
    #[gpui::test]
    fn the_mouse_scrolls_a_list_by_its_bar(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Rows { list: ListState::new(100, ListAlignment::Top, px(100.0)).measure_all(), bar: State::default() });
        cx.run_until_parked();
        let at = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |v, _| -f32::from(v.list.scroll_px_offset_for_scrollbar().y));
        assert_eq!(at(cx), 0.0);
        let x = px(300.0 - WIDTH / 2.0);
        // The track's bottom: the thumb goes there, the list to its end.
        cx.simulate_mouse_down(point(x, px(199.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(at(cx) > 1500.0, "{}", at(cx));
        // Dragged back up, still holding it.
        cx.simulate_mouse_move(point(x, px(0.0)), MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(at(cx) < 100.0, "{}", at(cx));
        cx.simulate_mouse_up(point(x, px(0.0)), MouseButton::Left, Modifiers::none());
        // Moving without the button does nothing.
        cx.simulate_mouse_move(point(x, px(150.0)), None, Modifiers::none());
        cx.run_until_parked();
        assert!(at(cx) < 100.0);
    }

    #[test]
    fn the_thumb_follows_the_content() {
        let m = Metrics { offset: 0.0, max: 300.0, viewport: 100.0 };
        // A quarter of the content shows: a quarter of the track.
        assert_eq!(thumb(m, 200.0), (0.0, 50.0));
        assert_eq!(thumb(Metrics { offset: 300.0, ..m }, 200.0), (150.0, 50.0));
        assert_eq!(thumb(Metrics { offset: 150.0, ..m }, 200.0), (75.0, 50.0));
        // Long content: the thumb keeps a length to take hold of.
        assert_eq!(thumb(Metrics { offset: 0.0, max: 100_000.0, viewport: 100.0 }, 200.0).1, MIN_THUMB);
        // And back: the thumb's place gives the offset.
        assert_eq!(offset_for(m, 200.0, 50.0, 75.0), 150.0);
        assert_eq!(offset_for(m, 200.0, 50.0, -20.0), 0.0);
        assert_eq!(offset_for(m, 200.0, 50.0, 500.0), 300.0);
    }
}
