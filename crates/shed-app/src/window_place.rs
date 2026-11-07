//! Windows open where they were left: size, position, maximized or full
//! screen. A window's place is sent to the keeper once it has stopped
//! changing for a moment, and kept with its bench (or, for the Shed window,
//! on its own), so hiding, closing and quitting all bring it back the same.

use std::time::Duration;

use gpui::{App, Bounds, Context, Pixels, Size, Subscription, Task, Window, WindowBounds, point, px, size};
use shed_model::{BenchId, WindowPlace, WindowState};
use shed_proto::ClientMsg;

use crate::keeper_conn::KeeperConn;

/// How long a window must keep still before its place is recorded; also
/// skips the in-between sizes of maximizing on X11.
const SETTLE: Duration = Duration::from_millis(500);

/// Smaller than this is a place left by mistake (or by a bug), not a choice.
const MIN: Size<Pixels> = Size { width: px(320.0), height: px(200.0) };

/// Follows one window's place and tells the keeper when it changes.
pub struct PlaceTracker {
    bench: Option<BenchId>,
    conn: KeeperConn,
    /// The window's bounds when last neither maximized nor full screen: what
    /// it goes back to.
    normal: Bounds<Pixels>,
    sent: Option<WindowPlace>,
    settling: Option<Task<()>>,
}

/// A view whose window's place is followed.
pub trait Placed: 'static + Sized {
    fn tracker(&mut self) -> &mut PlaceTracker;
}

impl PlaceTracker {
    /// For a window just opened with `opened` (from [`open_bounds`]).
    pub fn new(bench: Option<BenchId>, conn: KeeperConn, opened: WindowBounds, sent: Option<WindowPlace>) -> Self {
        Self { bench, conn, normal: opened.get_bounds(), sent, settling: None }
    }

    /// Records the window's place whenever it has settled after a change.
    pub fn follow<V: Placed>(window: &mut Window, cx: &mut Context<V>) -> Subscription {
        cx.observe_window_bounds(window, |view, window, cx| {
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(SETTLE).await;
                this.update_in(cx, |view, window, _| view.tracker().record(window)).ok();
            });
            view.tracker().settling = Some(task);
        })
    }

    /// Sends the window's place now if it changed: before the window goes.
    pub fn record(&mut self, window: &Window) {
        self.settling = None;
        let state = if window.is_fullscreen() {
            WindowState::Fullscreen
        } else if window.is_maximized() {
            WindowState::Maximized
        } else {
            WindowState::Normal
        };
        if state == WindowState::Normal {
            self.normal = window.bounds();
        }
        let b = self.normal;
        let place = WindowPlace {
            x: b.origin.x.into(),
            y: b.origin.y.into(),
            width: b.size.width.into(),
            height: b.size.height.into(),
            state,
        };
        if self.sent != Some(place) {
            self.sent = Some(place);
            self.conn.send_msg(ClientMsg::SetWindow { bench: self.bench, place });
        }
    }
}

/// Where to open a window last at `place`: there, unless no display shows
/// that spot any more (then centered, at its size); `default` sized and
/// centered if it has no place yet.
pub fn open_bounds(place: Option<WindowPlace>, default: Size<Pixels>, cx: &App) -> WindowBounds {
    let Some(p) = place else { return WindowBounds::Windowed(Bounds::centered(None, default, cx)) };
    let at = Bounds::new(point(px(p.x), px(p.y)), size(px(p.width).max(MIN.width), px(p.height).max(MIN.height)));
    // Enough of it on a display to grab its title bar.
    let grip = Bounds::new(at.origin, size(at.size.width.min(px(160.0)), px(40.0)));
    let shown = cx.displays().iter().any(|d| d.bounds().intersects(&grip));
    let bounds = if shown { at } else { Bounds::centered(None, at.size, cx) };
    match p.state {
        WindowState::Normal => WindowBounds::Windowed(bounds),
        WindowState::Maximized => WindowBounds::Maximized(bounds),
        WindowState::Fullscreen => WindowBounds::Fullscreen(bounds),
    }
}
