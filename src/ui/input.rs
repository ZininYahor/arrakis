//! What the controls do, apart from how they look: the view draws its
//! sliders, switches and time fields around these, and the mouse, the
//! keyboard and VoiceOver work through them.

use std::time::Duration;

use egui::{
    Event, Id, InputState, Key, PointerButton, Rangef, Rect, Response, Sense, Ui, WidgetInfo,
    WidgetType,
};

use super::detent::{self, Detents, Hold, Snap, Snapped};
use super::thumper;

/// A push button. `latched` makes it a toggle that is on or off, such as
/// Red Now.
pub fn button(ui: &mut Ui, rect: Rect, id: Id, label: &str, latched: Option<bool>) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| match latched {
        Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, label),
        None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
    });
    response
}

/// A check box or a switch. The whole of `rect` takes the click.
pub fn toggle(ui: &mut Ui, rect: Rect, id: Id, label: &str, on: bool) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, label));
    response
}

/// Keeps the arrow keys for a focused control that uses them, instead of
/// letting egui move the focus with them.
fn keep_arrows(ui: &Ui, response: &Response) {
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
    }
}

/// What a slider did this frame.
pub struct Slider {
    pub response: Response,
    /// The new position, 0 at the left to 1 at the right, when it moved.
    pub moved_to: Option<f32>,
    /// True while the primary button is down on the slider, from the frame
    /// it goes down, so the real screen previews the look from then on: a
    /// press that snaps moves the knob in that frame (see `still_press`).
    pub dragging: bool,
    /// The frame the primary button comes up after holding the slider,
    /// however the press ended, which ends the preview.
    pub let_go: bool,
    /// The pointer brought the value onto a detent or over one, and a tick
    /// is due on the trackpad. The keys never tick.
    pub tick: bool,
    /// A beat of the thumper is due on the trackpad: the pointer has held
    /// the value on the slider's thumper stop until a beat came (see
    /// `thumper_hold` below). The keys never thump.
    pub thump: bool,
}

impl Slider {
    /// Where the thumb is this frame, from 0 to 1.
    pub fn shown(&self, value: f32) -> f32 {
        self.moved_to.unwrap_or(value)
    }

    /// Tells a screen reader `label: spoken`, with the slider at `value`.
    /// The view calls it once it knows the look the frame's move leaves, so
    /// what is heard never lags the knob. The readout goes in the label,
    /// since a screen reader would otherwise only hear the 0 to 1 position,
    /// and may say more than the view shows, such as the name of the stop
    /// the value sits on.
    pub fn describe(&self, value: f32, label: &str, spoken: &str) {
        let enabled = self.response.enabled();
        self.response.widget_info(|| {
            WidgetInfo::slider(enabled, f64::from(value), format!("{label}: {spoken}"))
        });
    }
}

/// The arrow keys that move a slider toward its left end, and toward its
/// right end.
const LEFTWARD: [Key; 2] = [Key::ArrowLeft, Key::ArrowDown];
const RIGHTWARD: [Key; 2] = [Key::ArrowRight, Key::ArrowUp];

/// Whether one of `keys` went down this frame, with ⌥ held or not as `alt`
/// says, or either way when it is `None`.
fn pressed(input: &InputState, keys: [Key; 2], alt: Option<bool>) -> bool {
    input.events.iter().any(|event| {
        matches!(
            event,
            Event::Key { key, pressed: true, modifiers, .. }
                if keys.contains(key) && alt.is_none_or(|held| modifiers.alt == held)
        )
    })
}

/// A horizontal slider over `rect`, whose thumb's center travels across
/// `travel`. `value` is the position from 0 to 1; `step` is how far one
/// arrow key press moves it. A slider with `detents` snaps to them under the
/// pointer, stops on them under the keys, and jumps between its stops with
/// ⌥ and an arrow; on one of them, `thumper`, a hold of the pointer beats
/// (see `thumper_hold` below). Only the primary button moves a slider, as
/// in macOS: another, such as a click with two fingers, leaves it alone.
/// Describe it with [`Slider::describe`].
#[allow(clippy::too_many_arguments)]
pub fn slider(
    ui: &mut Ui,
    rect: Rect,
    travel: Rangef,
    id: Id,
    value: f32,
    step: f32,
    detents: Option<Detents>,
    thumper: Option<f32>,
) -> Slider {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    keep_arrows(ui, &response);
    let position = |x: f32| ((x - travel.min) / travel.span()).clamp(0.0, 1.0);
    let snap = detents.map(|detents| detents.along(travel.span()));
    let pressing =
        response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_down());
    // The press's own frame: the primary button went down on the slider,
    // and egui has yet to tell a click from a drag.
    let pressed_now = pressing && ui.input(|input| input.pointer.primary_pressed());
    let dragged = response.dragged_by(PointerButton::Primary);
    let drag_started = response.drag_started_by(PointerButton::Primary);
    // A key that sets the value takes the frame: the pointer moves nothing
    // in it, so no tick plays for a detent the knob is never shown on, and
    // the pointer's move is worked out again in the next frame.
    let keyed = keys(ui, &response, value, step, detents);
    let mut moved_to = None;
    let mut tick = false;
    if keyed.is_none()
        && let Some(pointer) = response.interact_pointer_pos()
        && (dragged || response.clicked() || drag_started)
    {
        let raw = position(pointer.x);
        moved_to = Some(raw);
        if let Some(snap) = snap {
            // Apart from a press that snaps in its own frame (see
            // `still_press`), the pointer moves the value once egui tells a
            // click from a drag: on the click, or as the drag starts, which
            // may be a few points from where the press landed. A click lands
            // where it is let go, as egui drops where it landed with the
            // button.
            let press = (drag_started || response.clicked())
                .then(|| landing(ui, id, snap, value, position, Some(raw)))
                .flatten();
            let snapped = commit(ui, id, snapping(ui, id, snap, value, press, raw));
            moved_to = Some(snapped.to);
            tick = snapped.tick;
        }
    } else if keyed.is_none()
        && pressed_now
        && let Some(snap) = snap
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(snapped) = still_press(ui, id, snap, value, position, position(pointer.x))
    {
        moved_to = Some(snapped.to);
        tick = snapped.tick;
    }
    // Whether this frame lets go of a press that held the slider, however
    // it ended: a click, a drag, or a press dragged off.
    let mut let_go = false;
    ui.data_mut(|data| {
        if pressing {
            data.insert_temp(id.with(IN_HAND), true);
        } else {
            // The press has ended, or never began: the next one starts
            // afresh.
            data.remove::<Hold>(id.with(HOLD));
            data.remove::<f32>(id.with(PRESSED_ON));
            let_go = data.remove_temp::<bool>(id.with(IN_HAND)).is_some();
        }
    });
    let by_pointer = moved_to;
    if let Some(to) = keyed {
        moved_to = Some(to);
        if pressing && !dragged && snap.is_some() {
            // A key moves the value under a press that egui has yet to call
            // a click or a drag. The press still lands as it would have on
            // the value it began on.
            ui.data_mut(|data| {
                data.get_temp_mut_or(id.with(PRESSED_ON), value);
            });
        }
    }
    let thump = match (snap, thumper) {
        (Some(snap), Some(at)) => {
            // The pointer holds the value on the stop: the drag left it there
            // this frame, or a press that egui has yet to call a click or a
            // drag landed on it. Such a press may not have moved the value
            // (a press on the knob at the stop), but it holds the stop from
            // its own frame. The keys set the value on their own: they never
            // start a hold, and a hold they set ends.
            let held = pressing
                && keyed.is_none()
                && ui.input(|input| input.focused)
                && match by_pointer {
                    Some(to) => to == at,
                    None => landing(ui, id, snap, value, position, None)
                        .is_some_and(|landed| landed.to == at),
                };
            thumper_hold(ui, id, held)
        }
        _ => false,
    };
    Slider {
        dragging: pressing,
        let_go,
        response,
        moved_to,
        tick,
        thump,
    }
}

/// Where the keys set a slider at `value` this frame, if it has the
/// keyboard and one of them went down: an arrow, or a page, steps by
/// `step` (four for a page), stopping on the `detents` it passes; ⌥ with an
/// arrow jumps to the next stop, on a slider with them; Home and End go to
/// the ends.
fn keys(
    ui: &Ui,
    response: &Response,
    value: f32,
    step: f32,
    detents: Option<Detents>,
) -> Option<f32> {
    if !response.has_focus() {
        return None;
    }
    // A slider with stops leaves ⌥ with an arrow to them; one without steps
    // with it as with a plain arrow.
    let plain = detents.map(|_| false);
    ui.input(|input| {
        let mut moved_to = None;
        let mut delta = 0.0;
        if pressed(input, LEFTWARD, plain) {
            delta -= step;
        }
        if pressed(input, RIGHTWARD, plain) {
            delta += step;
        }
        if input.key_pressed(Key::PageDown) {
            delta -= step * 4.0;
        }
        if input.key_pressed(Key::PageUp) {
            delta += step * 4.0;
        }
        if delta != 0.0 {
            moved_to = Some(match detents {
                Some(detents) => detents.step(value, delta),
                None => (value + delta).clamp(0.0, 1.0),
            });
        }
        let jump = detents.and_then(|detents| {
            if pressed(input, LEFTWARD, Some(true)) {
                detents.jump(value, false)
            } else if pressed(input, RIGHTWARD, Some(true)) {
                detents.jump(value, true)
            } else {
                None
            }
        });
        if jump.is_some() {
            moved_to = jump;
        }
        if input.key_pressed(Key::Home) {
            moved_to = Some(0.0);
        }
        if input.key_pressed(Key::End) {
            moved_to = Some(1.0);
        }
        moved_to
    })
}

/// Where a slider with a thumper keeps its [`Beating`], under its id, while
/// the hold lasts.
const THUMPER: &str = "thumper";

/// A hold on a thumper stop: when it began, on egui's clock, and how many of
/// its beats have come.
#[derive(Clone, Copy, Debug)]
struct Beating {
    began: f64,
    played: u32,
    /// The last of egui's passes that found the hold held.
    seen: u64,
}

/// The thumper, an easter egg for the owner and whoever finds it, through
/// one frame: whether a thump is due on the trackpad.
///
/// Hold the Temperature slider on its Desert stop (between Incandescent
/// and Ember) with the pointer down, and the trackpad beats like a thumper,
/// the device that calls a sandworm: nothing for 1.5 s, then two thumps
/// 180 ms apart, and another pair each second ([`thumper`] keeps the time).
/// It stops when the pointer goes up, when the value leaves the stop, on a
/// key that sets the value, and on a frame that finds the window unfocused;
/// the next hold waits the 1.5 s again. The primary button alone holds it.
/// The keys and VoiceOver never start it. Nothing in the window, its help or
/// the README tells of it: only this comment.
///
/// `held` is whether the pointer holds the value on the stop this frame,
/// the window focused. A hold begins at the first frame that finds it held,
/// and at no earlier time:
/// - for a press that lands on the stop, that is the press's own frame,
///   whose time is egui's `press_start_time`. A press that catches the stop
///   from elsewhere snaps the value onto it in that frame too (see
///   [`still_press`]), so the snap, its tick and the hold's start share it;
/// - for a drag, it is the frame where the detent caught the value;
/// - after a hold ends, with the pointer still down (a key, the value off
///   the stop, an unfocused frame), it is the frame where the hold is found
///   again. The press that began the ended hold is never its start.
///
/// A hold lasts while egui's passes find it held one after another, and the
/// [`Beating`] in memory is touched on those passes alone: one that a pass
/// missed has ended, and is never read as the hold again.
///
/// A finger held still draws no frames, so while a hold lasts, armed or
/// beating, this asks egui for one at the next beat, and at no other time:
/// with no hold, it costs nothing.
fn thumper_hold(ui: &Ui, id: Id, held: bool) -> bool {
    if !held {
        return false;
    }
    let (now, early) = ui.input(|input| (input.time, f64::from(input.predicted_dt)));
    let pass = ui.ctx().cumulative_pass_nr();
    let fresh = Beating {
        began: now,
        played: 0,
        seen: pass,
    };
    let beat = ui.data_mut(|data| {
        let hold = data.get_temp_mut_or(id.with(THUMPER), fresh);
        if hold.seen + 1 != pass {
            *hold = fresh;
        }
        let beat = thumper::beat(hold.began, now, hold.played);
        hold.played = beat.played;
        hold.seen = pass;
        beat
    });
    // egui wakes a frame's predicted length early, so that the frame is
    // done by the time asked for; the beat must have come when the frame
    // starts, so ask that much later. The clock puts the next beat after
    // now; a time it cannot wait for asks for no frame rather than for one
    // at once.
    if let Ok(wait) = Duration::try_from_secs_f64(beat.next - now + early) {
        ui.ctx().request_repaint_after(wait);
    }
    beat.due
}

/// Where a drag keeps its [`Hold`], under the slider's id. It lasts while
/// the pointer is down on the slider.
const HOLD: &str = "detent hold";
/// Where a slider notes, under its id, that the primary button is down on
/// it, so that the frame it comes up in lets go of the preview.
const IN_HAND: &str = "in hand";
/// Where a slider keeps the time of its last tick, in seconds, under its id.
/// It outlasts the drag, so that quick clicks keep to the limit too.
const LAST_TICK: &str = "last detent tick";
/// Where a slider with detents keeps the value that a press began on, under
/// its id, once a key has moved the value under the press, or the detent
/// that a press caught in its own frame (see [`still_press`]). It lasts
/// while the pointer is down on the slider.
const PRESSED_ON: &str = "value pressed on";

/// Where the press under way leaves the value of a slider with the detents
/// `snap`, `value` being the value now: from where egui saw it land, along
/// `position`, or from `fallback` once egui has dropped that, as it does
/// when the button goes up. `None` with neither.
///
/// The press keeps the detent the value was on as the press began: a key
/// that has moved the value under the press since makes no keep (see
/// [`PRESSED_ON`]), so it never makes the pointer's hold either.
fn landing(
    ui: &Ui,
    id: Id,
    snap: Snap,
    value: f32,
    position: impl Fn(f32) -> f32,
    fallback: Option<f32>,
) -> Option<Snapped> {
    let at = ui
        .input(|input| input.pointer.press_origin())
        .map(|origin| position(origin.x))
        .or(fallback)?;
    let began_on = ui
        .data(|data| data.get_temp::<f32>(id.with(PRESSED_ON)))
        .unwrap_or(value);
    Some(snap.press(began_on, at))
}

/// A press on a slider with the detents `snap`, in the press's own frame,
/// the value being at `value` and the pointer now at `raw`. A press that
/// lands in a detent's catch with the value elsewhere moves the value onto
/// the detent at once, with its tick, rather than waiting for egui to call
/// it a click or a drag: on the release, or 0.8 s on for a still press.
///
/// `None` leaves the press to that later frame, as before:
/// - a press clear of every detent;
/// - a press on the knob at its detent, or within that detent's release,
///   which keeps the value there with no tick;
/// - a press whose frame's move let the detent go again: never shown on
///   it, it never ticks (see [`snapping`]).
///
/// From then on the press keeps the detent it caught, as a press keeps the
/// one the value was on as it began (see [`PRESSED_ON`]).
fn still_press(
    ui: &Ui,
    id: Id,
    snap: Snap,
    value: f32,
    position: impl Fn(f32) -> f32,
    raw: f32,
) -> Option<Snapped> {
    let caught = landing(ui, id, snap, value, position, None).filter(|landed| landed.tick)?;
    let snapped = snapping(ui, id, snap, value, Some(caught), raw);
    if snapped.hold.detent != caught.hold.detent {
        return None;
    }
    ui.data_mut(|data| data.insert_temp(id.with(PRESSED_ON), caught.to));
    Some(commit(ui, id, snapped))
}

/// The pointer at `raw` on a slider with detents, the value being at
/// `value`: where the value goes, and whether a tick is due. `press` is
/// where a press leaves the value (see [`landing`]), on the frame that sees
/// it. Changes nothing: [`commit`] keeps the result.
fn snapping(ui: &Ui, id: Id, snap: Snap, value: f32, press: Option<Snapped>, raw: f32) -> Snapped {
    let held = ui.data(|data| data.get_temp::<Hold>(id.with(HOLD)));
    match (press, held) {
        (None, Some(hold)) => snap.drag(hold, raw),
        // A press, or a drag that lost its hold, which lands where the
        // pointer is: from where it landed to where the pointer is now, with
        // one tick at most. The press's own tick counts only if the value is
        // still on the detent it caught: one caught and let go within the
        // frame is never shown.
        (pressed, _) => {
            let pressed = pressed.unwrap_or_else(|| snap.press(value, raw));
            let moved = snap.drag(pressed.hold, raw);
            let shown = moved.hold.detent == pressed.hold.detent;
            Snapped {
                tick: (pressed.tick && shown) || moved.tick,
                ..moved
            }
        }
    }
}

/// Keeps `snapped`'s hold for the drag's next frame, and lets its tick play
/// unless one played less than [`detent::TICK_GAP`] before.
fn commit(ui: &Ui, id: Id, snapped: Snapped) -> Snapped {
    ui.data_mut(|data| data.insert_temp(id.with(HOLD), snapped.hold));
    let now = ui.input(|input| input.time);
    let last = ui.data(|data| data.get_temp::<f64>(id.with(LAST_TICK)));
    let tick = snapped.tick && detent::may_tick(last, now);
    if tick {
        ui.data_mut(|data| data.insert_temp(id.with(LAST_TICK), now));
    }
    Snapped { tick, ..snapped }
}

/// What a spinner did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpinEdit {
    /// The arrows, keys or wheel moved `segment` by `by` steps.
    Step { segment: usize, by: i32 },
    /// Digits typed into `segment` make `value`; the caller clamps it.
    Typed { segment: usize, value: u16 },
}

/// A spin box through one frame. It shows `segments` (one, or hour and
/// minute); its arrows, the wheel and the arrow keys step the selected
/// one, and digits typed into it replace it.
///
/// The view makes it over the field, lays the segments out, passes their
/// places to [`Spinner::input`], draws, adds its arrow buttons with
/// [`Spinner::arrow`], and ends with [`Spinner::finish`].
pub struct Spinner {
    id: Id,
    pub response: Response,
    segments: usize,
    selected: usize,
    /// The segment typed into, its value so far, and how many digits.
    typed: Option<(usize, u16, usize)>,
    typed_selected: usize,
    step: i32,
    edit: Option<SpinEdit>,
}

impl Spinner {
    pub fn new(ui: &mut Ui, field: Rect, id: Id, label: &str, segments: &[String]) -> Self {
        let enabled = ui.is_enabled();
        let selected = ui
            .data(|data| data.get_temp(id.with("segment")))
            .unwrap_or(0);
        let typed = ui.data(|data| data.get_temp(id.with("typed"))).flatten();
        let response = ui.interact(field, id, Sense::click());
        let value_text = segments.join(":");
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::DragValue,
                enabled,
                format!("{label}: {value_text}"),
            )
        });
        keep_arrows(ui, &response);
        Self {
            id,
            response,
            segments: segments.len(),
            selected,
            typed,
            typed_selected: selected,
            step: 0,
            edit: None,
        }
    }

    /// Whether segment `index` shows selected: a focused spinner selects
    /// its segment, as an edit box selects its text.
    pub fn is_selected(&self, index: usize) -> bool {
        index == self.selected && self.response.has_focus()
    }

    /// Clicks on the segments laid out at `places`, digits, the arrow keys
    /// and the wheel. After `max_digits` digits, typing moves on to the
    /// next segment.
    pub fn input(&mut self, ui: &Ui, places: &[Rect], max_digits: usize) {
        let enabled = ui.is_enabled();
        let last = self.segments.saturating_sub(1);
        if self.response.clicked()
            && let Some(pointer) = self.response.interact_pointer_pos()
        {
            self.selected = places
                .iter()
                .position(|place| pointer.x < place.max.x + 2.0)
                .unwrap_or(last);
            self.response.request_focus();
        }

        if self.response.has_focus() && enabled {
            let digits: Vec<u16> = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::Text(text) => Some(
                            text.chars()
                                .filter_map(|c| c.to_digit(10))
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .map(|digit| digit as u16)
                    .collect()
            });
            for digit in digits {
                let (value, count) = match self.typed {
                    Some((segment, value, count)) if segment == self.selected => {
                        (value * 10 + digit, count + 1)
                    }
                    _ => (digit, 1),
                };
                self.edit = Some(SpinEdit::Typed {
                    segment: self.selected,
                    value,
                });
                if count >= max_digits {
                    self.typed = None;
                    self.selected = (self.selected + 1).min(last);
                } else {
                    self.typed = Some((self.selected, value, count));
                }
            }
        }
        self.typed_selected = self.selected;

        if self.response.has_focus() {
            ui.input(|input| {
                if input.key_pressed(Key::ArrowUp) {
                    self.step += 1;
                }
                if input.key_pressed(Key::ArrowDown) {
                    self.step -= 1;
                }
                if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::Colon) {
                    self.selected = (self.selected + 1).min(last);
                }
                if input.key_pressed(Key::ArrowLeft) {
                    self.selected = self.selected.saturating_sub(1);
                }
            });
        }
        if self.response.hovered() && enabled {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll > 0.5 {
                self.step += 1;
            } else if scroll < -0.5 {
                self.step -= 1;
            }
        }
    }

    /// One arrow button at `rect`, "up" or "down": a click steps the
    /// selected segment by `delta`, and holding it repeats.
    pub fn arrow(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        name: &str,
        label: &str,
        delta: i32,
    ) -> Response {
        let enabled = ui.is_enabled();
        let arrow = ui.interact(rect, self.id.with(name), Sense::click());
        arrow.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Button, enabled, format!("{label} {name}"))
        });
        if arrow.clicked() {
            self.step += delta;
            self.response.request_focus();
        }
        if arrow.is_pointer_button_down_on()
            && enabled
            && auto_repeat(ui, self.id.with(name).with("repeat"))
        {
            self.step += delta;
        }
        arrow
    }

    /// Keeps the selection and the digits typed so far for the next frame,
    /// and returns what changed.
    pub fn finish(mut self, ui: &Ui) -> Option<SpinEdit> {
        // Stepping, moving to another segment or leaving starts a new number.
        if self.step != 0 || self.selected != self.typed_selected || !self.response.has_focus() {
            self.typed = None;
        }
        ui.data_mut(|data| {
            data.insert_temp(self.id.with("segment"), self.selected);
            data.insert_temp(self.id.with("typed"), self.typed);
        });
        if self.step != 0 && ui.is_enabled() {
            self.edit = Some(SpinEdit::Step {
                segment: self.selected,
                by: self.step,
            });
        }
        self.edit
    }
}

/// While an arrow button is held: true after a 400 ms pause, then every
/// 60 ms, like the Windows spin control.
fn auto_repeat(ui: &Ui, id: Id) -> bool {
    let now = ui.input(|input| input.time);
    let (started, last) = ui
        .data(|data| data.get_temp::<(f64, f64)>(id))
        .unwrap_or((now, now));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(60));
    let fire = now - started > 0.4 && now - last > 0.06;
    ui.data_mut(|data| data.insert_temp(id, (started, if fire { now } else { last })));
    fire
}

/// Forget the hold timers of spinners' buttons no longer held.
pub fn release_repeats(ui: &Ui, ids: &[Id]) {
    if !ui.input(|input| input.pointer.any_down()) {
        ui.data_mut(|data| {
            for id in ids {
                for name in ["up", "down"] {
                    data.remove::<(f64, f64)>(id.with(name).with("repeat"));
                }
            }
        });
    }
}
