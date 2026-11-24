//! The beat animated marks step to (the working mark on AI tabs, the dots
//! while Claude works): one timer for the whole app, a few steps a second,
//! running only while some view animates. Each step redraws just the views
//! that asked for it (a tab bar, a card, an AI view), all in one go, so a
//! window draws once per step; with nothing animating, nothing is drawn.

use std::time::Duration;

use gpui::{App, Context, EntityId, Global, prelude::*, svg};
use shed_emu::Rgb;

use crate::assets::icon;
use crate::theme::hsla;

/// How long a step lasts: four a second.
pub const STEP: Duration = Duration::from_millis(250);

/// Steps a turn of the working mark takes.
const TURN: u64 = 8;

/// Redraws a view; false once it is gone.
type Redraw = Box<dyn Fn(&mut App) -> bool>;

#[derive(Default)]
struct Pulse {
    /// The views it redraws each step: their entity, and how to redraw it
    /// (false once it is gone).
    views: Vec<(EntityId, Redraw)>,
    ticking: bool,
}

impl Global for Pulse {}

/// The step the beat is at: the same for every view drawn at a step. Told
/// by the clock, not kept in the global: a view reading a global is drawn
/// again whenever it changes, and the beat must redraw only those that ask.
pub fn frame() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let step = STEP.as_millis() as u64;
    (START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + step / 2) / step
}

/// Makes the view `cx` is for redraw at each step (`on`), or stop.
pub fn animate<T: 'static>(cx: &mut Context<T>, on: bool) {
    let id = cx.entity_id();
    let known = cx.try_global::<Pulse>().is_some_and(|p| p.views.iter().any(|(v, _)| *v == id));
    if on == known {
        return;
    }
    let weak = cx.entity().downgrade();
    let pulse = cx.default_global::<Pulse>();
    if on {
        pulse.views.push((id, Box::new(move |cx: &mut App| weak.update(cx, |_, cx| cx.notify()).is_ok())));
    } else {
        pulse.views.retain(|(v, _)| *v != id);
    }
    if pulse.views.is_empty() || pulse.ticking {
        return;
    }
    pulse.ticking = true;
    cx.spawn(async move |_, cx| {
        loop {
            cx.background_executor().timer(STEP).await;
            if !cx.update(step) {
                break;
            }
        }
    })
    .detach();
}

/// A step: every view asking for it is redrawn; whether any still asks.
fn step(cx: &mut App) -> bool {
    let views = std::mem::take(&mut cx.global_mut::<Pulse>().views);
    let mut alive: Vec<_> = views.into_iter().filter(|(_, redraw)| redraw(cx)).collect();
    let pulse = cx.global_mut::<Pulse>();
    // (Views that asked during the step come after.)
    alive.append(&mut pulse.views);
    pulse.views = alive;
    pulse.ticking = !pulse.views.is_empty();
    pulse.ticking
}

/// Whether any view steps with the beat (tests). (The timer itself stops
/// at its next step after the last view leaves.)
#[cfg(test)]
pub fn running(cx: &App) -> bool {
    cx.try_global::<Pulse>().is_some_and(|p| !p.views.is_empty())
}

/// The working mark: a ring of dots turning a step at each beat, in place
/// of an AI tab's icon while Claude is at it.
pub fn working_mark(frame: u64, size: gpui::Pixels, color: Rgb) -> gpui::Svg {
    let angle = (frame % TURN) as f32 * std::f32::consts::TAU / TURN as f32;
    svg().path(icon("working")).size(size).flex_none().text_color(hsla(color)).with_transformation(gpui::Transformation::rotate(gpui::radians(angle)))
}
