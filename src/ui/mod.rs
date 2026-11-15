//! The settings window: a picture of the night look, the state, the
//! Temperature and Brightness sliders, the schedule, Red Now, and what the
//! displays show. [`view`] draws them; this module holds the window and
//! what the controls mean. Controls push [`Action`]s, which the app
//! applies after the frame.

#[cfg(target_os = "macos")]
pub mod appkit;
/// Only AppKit animates a closing window, keeps its last frame after it, and
/// starts a helper process with a window.
#[cfg(not(target_os = "macos"))]
pub mod appkit {
    pub(super) fn skip_close_animation(_frame: &eframe::Frame) {}
    pub fn release_closed_window() {}
    pub(crate) fn skip_autofill_helper() {}
}
pub mod cover;
pub mod detent;
#[cfg(test)]
pub(crate) mod headless;
pub mod input;
pub mod paint;
#[cfg(test)]
pub(crate) mod testing;
pub mod thumper;
pub mod view;

use std::sync::LazyLock;

use egui::{Key, Modifiers, PointerButton, Response, Ui};
use fastframe_shell::Held;

use crate::app::App;
use crate::color::{self, Look, Tint};
use crate::model::Action;
use crate::schedule::{ClockTime, MINUTES_PER_DAY};
use crate::settings::FADE_MAX_MINUTES;
use detent::Detents;
use input::{Slider, SpinEdit};

/// One press of an arrow key on the Temperature slider.
const REDNESS_STEP: f32 = 0.02;
/// One press on the Brightness slider: five percent.
const BRIGHTNESS_STEP: f32 = 5.0 / BRIGHTNESS_SPAN;
const BRIGHTNESS_SPAN: f32 = (color::MAX_BRIGHTNESS - color::MIN_BRIGHTNESS) as f32;
/// The minute field of a time steps by this much, as on a kitchen timer:
/// nobody turns red at 21:03.
const MINUTE_STEP: i32 = 5;
const FADE_STEP: i32 = 5;

/// The widest texture, in pixels, that the window tells egui the GPU takes.
///
/// eframe hands egui the painter's limit, 16384 on a Mac with either painter,
/// and egui makes the font atlas that wide: 16384 x 64 x 4 B = 4 MiB, paid
/// three times on glow (in the heap, in the first upload's copy that the
/// allocator keeps, and as a GL texture; the Metal painter's count is unread)
/// for glyphs that fill about 8% of it. At this cap the atlas is 0.5 MiB at
/// first and grows by rows with the glyphs, and 12 MB leave the open
/// window's footprint.
///
/// 2048 and not the painter's limit, which is the 4 MiB above. Not 1024 either,
/// though it is the least egui takes (`TextureAtlas::new` asserts it): it
/// ties at rest and halves the atlas after a drag of the Temperature slider
/// (1 MiB against 2 MiB), but 2048 is egui's own default, which the headless
/// tests and benches already run at. A drag fills 0.06 of it; egui builds
/// the atlas again past 0.8. The window's keyboard zoom is off
/// (`theme::install`), which is what keeps it there. The cover's pictures
/// stay under the cap: a debug build panics in `load_texture` above it.
const MAX_TEXTURE_SIDE: usize = 2048;

/// Caps the texture side that eframe tells egui, in a pass's input, at
/// [`MAX_TEXTURE_SIDE`]. Only the first input carries the painter's limit
/// (`RawInput::take`), so the cap is applied to what is there: `map` leaves
/// a later `None` alone and never raises a limit that is already below the
/// cap. Every window of this crate calls it from its `raw_input_hook`: this
/// one, and the demo's Scale plate.
pub(crate) fn cap_texture_side(raw_input: &mut egui::RawInput) {
    raw_input.max_texture_side = raw_input
        .max_texture_side
        .map(|side| side.min(MAX_TEXTURE_SIDE));
}

/// The window around the app while it is open.
pub struct Window {
    app: Held<App>,
    #[cfg(feature = "demo")]
    shot: Option<crate::demo::Shot>,
}

impl Window {
    pub fn new(app: Held<App>) -> Self {
        Self {
            app,
            #[cfg(feature = "demo")]
            shot: None,
        }
    }

    #[cfg(feature = "demo")]
    pub fn with_shot(mut self, shot: Option<crate::demo::Shot>) -> Self {
        self.shot = shot;
        self
    }

    /// The app, for what a drive reads of it.
    #[cfg(feature = "demo")]
    pub fn app(&self) -> &App {
        &self.app
    }
}

impl eframe::App for Window {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // The window's last pass: see `appkit::skip_close_animation` for why
        // the animation goes here and not as the window is made.
        if closes(ctx) {
            appkit::skip_close_animation(frame);
        }
        self.app.background_frame(Some(ctx));
        #[cfg(feature = "demo")]
        if let Some(shot) = &mut self.shot
            && shot.frame(ctx, Some(&mut self.app))
        {
            self.app.actions.push(Action::Quit);
            self.app.apply_actions(Some(ctx));
        }
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        // AppKit puts the traffic lights back in its own layout passes.
        let ctx = ui.ctx().clone();
        // The zoom is off (`theme::install`), so a point is a point.
        fastframe_titlebar::align_traffic_lights(frame, &ctx, crate::theme::TITLE_BAR);
        draw(&mut self.app, ui);
        self.app.apply_actions(Some(&ctx));
    }

    /// The last word on the input of every pass, before egui sees it (eframe
    /// runs it from `prepare_raw_input`, for a pass that draws and for one
    /// that only ticks the logic): the cap on the texture side, and in a
    /// demo build what a shot needs.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        cap_texture_side(raw_input);
        // A shot pictures the window in use, whatever app has the keyboard
        // while it is taken. egui's `Response::has_focus` reads
        // `RawInput::focused`, which winit sets from the real window, so the
        // hook sets that and each viewport's `focused` too. Outside a shot
        // the OS decides.
        #[cfg(feature = "demo")]
        if self.shot.is_some() {
            raw_input.focused = true;
            for viewport in raw_input.viewports.values_mut() {
                viewport.focused = Some(true);
            }
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

/// Whether this pass closes the window: eframe drops it after a pass whose
/// input asks for a close, since nothing here cancels one. Every close comes
/// this way. Esc, Cmd+W, the menu's Quit and the driver send
/// `ViewportCommand::Close`, and the close button asks through winit.
///
/// That pass may be a full one or, while the window is minimized or covered,
/// one that runs `logic` alone (eframe's `update_logic_only`): both give
/// `logic` the pass's viewport input.
pub(crate) fn closes(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.viewport().close_requested())
}

/// The whole window.
pub fn draw(app: &mut App, ui: &mut Ui) {
    keyboard(app, ui);
    view::draw(app, ui);
}

/// Esc, Return with nothing focused, and Cmd+W close the window; Cmd+Q
/// quits.
fn keyboard(app: &mut App, ui: &Ui) {
    let nothing_focused = ui.memory(|memory| memory.focused()).is_none();
    ui.input_mut(|input| {
        if input.consume_key(Modifiers::NONE, Key::Escape)
            || input.consume_key(Modifiers::COMMAND, Key::W)
            || (nothing_focused && input.consume_key(Modifiers::NONE, Key::Enter))
        {
            app.actions.push(Action::HideWindow);
        }
        if input.consume_key(Modifiers::COMMAND, Key::Q) {
            app.actions.push(Action::Quit);
        }
    });
}

/// What a picture of the night look draws through: seen through the tint
/// the real displays have now, it shows the night look itself at noon,
/// during a fade and while red.
fn preview_tint(app: &App, night: Look) -> Tint {
    night.tint().through(app.status.tint)
}

/// How far through the red hours the clock is: 0 at From, 1 at To, and 0
/// outside them. None while the schedule is off or From is To.
fn night_progress(app: &App) -> Option<f32> {
    let settings = &app.settings;
    if !settings.schedule_on {
        return None;
    }
    let day = f64::from(MINUTES_PER_DAY);
    let start = f64::from(settings.start.minutes());
    let length = (f64::from(settings.end.minutes()) - start).rem_euclid(day);
    if length == 0.0 {
        return None;
    }
    let elapsed = (app.minute_of_day() - start).rem_euclid(day);
    Some(if elapsed < length {
        (elapsed / length) as f32
    } else {
        0.0
    })
}

/// A named stop along the Temperature track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    pub kelvin: u16,
    pub name: &'static str,
    /// Whether the name under the track sits midway between its neighbors'
    /// names, in place of centered on the stop's tick, so the two gaps are
    /// equal. Only a stop inside the track, between two stops that are not
    /// midway themselves, can.
    pub midway: bool,
}

impl Stop {
    /// Where the stop sits on the slider, from 0 to 1.
    pub fn position(&self) -> f32 {
        color::redness_of(self.kelvin)
    }
}

/// The stops along the track, from the left end to the right: the ends,
/// and the three detents between them, each with a tick. All five are
/// named under the track; f.lux's other names stay off it.
pub const STOPS: [Stop; 5] = [
    Stop {
        kelvin: color::DAY_KELVIN,
        name: "Daylight",
        midway: false,
    },
    Stop {
        kelvin: 2700,
        name: "Incandescent",
        midway: false,
    },
    // Halfway between Incandescent and Ember on the scale, where the owner
    // asked for it, and its name midway between theirs.
    Stop {
        kelvin: DESERT_KELVIN,
        name: "Desert",
        midway: true,
    },
    Stop {
        kelvin: 1200,
        name: "Ember",
        midway: false,
    },
    Stop {
        kelvin: color::PURE_RED_KELVIN,
        name: "Pure red",
        midway: false,
    },
];

/// The stop a temperature sits on: exactly 6500 K, 2700 K, Desert's,
/// 1200 K, or pure red.
pub fn stop_at(kelvin: u16) -> Option<&'static Stop> {
    let kelvin = kelvin.max(color::PURE_RED_KELVIN);
    STOPS.iter().find(|stop| stop.kelvin == kelvin)
}

/// The Desert stop's temperature, a detent of the Temperature slider and
/// the one where its thumper beats. It is the scale's midpoint between
/// Incandescent and Ember (position 0.4966), on the scale's 10 K grid, and
/// the one source: the stop table, the detents and the thumper take it from
/// here.
pub const DESERT_KELVIN: u16 = 1760;

/// The stops with a detent inside the track: all but the ends.
fn detent_stops() -> &'static [Stop] {
    &STOPS[1..STOPS.len() - 1]
}

/// Where the Temperature slider's detents sit, from the left.
static DETENT_POSITIONS: LazyLock<[f32; STOPS.len() - 2]> =
    LazyLock::new(|| std::array::from_fn(|index| detent_stops()[index].position()));

/// The Temperature slider's detents, worked out once: its stops inside the
/// track, pure red's zone at the right end, which is the scale's own, and
/// 860 K next to it, which the keys step to and from. This is the one copy
/// of each.
static DETENTS: LazyLock<Detents<'static>> = LazyLock::new(|| Detents {
    inner: &*DETENT_POSITIONS,
    end_zone: color::pure_red_from(),
    short_of_end: color::short_of_pure_red(),
});

/// Where the Temperature slider's thumper beats (see `ui::input`): on the
/// Desert stop, the same position as that detent's. This is the one copy;
/// the detent math leaves it out.
static THUMPER: LazyLock<Option<f32>> =
    LazyLock::new(|| stop_at(DESERT_KELVIN).map(Stop::position));

/// The Temperature slider's position for `look`, and its readout: the
/// kelvin, which every stop shows alike, or "Pure red".
fn temperature(look: Look) -> (f32, String) {
    let readout = if look.is_pure_red() {
        "Pure red".to_owned()
    } else {
        format!("{} K", look.kelvin)
    };
    (color::redness_of(look.kelvin), readout)
}

/// What a screen reader hears after "Temperature:" for `look`, whose readout
/// is `readout`. On a named stop it says the kelvin and the name; pure red's
/// readout already is one.
fn temperature_spoken(look: Look, readout: &str) -> String {
    match stop_at(look.kelvin) {
        Some(stop) if !look.is_pure_red() => format!("{} K, {}", look.kelvin, stop.name),
        _ => readout.to_owned(),
    }
}

/// The Brightness slider's position for `look`, and its readout.
fn brightness(look: Look) -> (f32, String) {
    (
        f32::from(look.brightness - color::MIN_BRIGHTNESS) / BRIGHTNESS_SPAN,
        format!("{}%", look.brightness),
    )
}

/// `night` with the Temperature slider's move this frame, if it moved.
fn with_temperature(mut night: Look, temperature: &Slider) -> Look {
    if let Some(redness) = temperature.moved_to {
        night.kelvin = color::kelvin_at_redness(redness);
    }
    night
}

/// `night` with the Brightness slider's move this frame, if it moved.
fn with_brightness(mut night: Look, brightness: &Slider) -> Look {
    if let Some(position) = brightness.moved_to {
        night.brightness = color::MIN_BRIGHTNESS + (position * BRIGHTNESS_SPAN).round() as u8;
    }
    night
}

/// The night look the two sliders leave this frame, from `moved`, the night
/// with both sliders' moves (see [`with_temperature`] and
/// [`with_brightness`]). Pushes it while it changes, and in every frame the
/// primary button is down on a slider, so the real screen follows the knob
/// from the press's own frame; the let-go ends that preview. Pushes a tick
/// when the Temperature slider's detent asks for one, and a thump when its
/// thumper does.
fn night_from(app: &mut App, moved: Look, temperature: &Slider, brightness: &Slider) -> Look {
    let dragging = temperature.dragging || brightness.dragging;
    if moved != app.settings.night || dragging {
        app.actions.push(Action::SetNight {
            look: moved,
            dragging,
        });
    }
    if temperature.tick {
        app.actions.push(Action::Detent);
    }
    if temperature.thump {
        app.actions.push(Action::Thump);
    }
    if temperature.let_go || brightness.let_go {
        app.actions.push(Action::EndPreview);
    }
    moved.clamped()
}

/// Whether a button took a click this frame, from the pointer, the keys or
/// VoiceOver. The pointer's primary button going down on it also asks the
/// trackpad for a press, in that frame, so the finger feels it before it
/// lifts; the click comes as the button comes up. A press dragged off
/// presses with no click. A click whose press and release share one frame
/// presses in that frame. The keys and VoiceOver never press.
///
/// The buttons that use it: the "Turn red automatically" switch and Red Now.
/// Close does not, by the owner's choice of 2026-10-23, and neither do the
/// Schedule's time fields (From, To and Fade, their parts and their arrows),
/// for now: the owner left the time selection out.
fn clicked(app: &mut App, response: &Response) -> bool {
    // egui says no button is down on a widget in a frame that lets one go,
    // so a press let go in its own frame shows as the click.
    let went_down = response.ctx.input(|input| input.pointer.primary_pressed());
    if went_down
        && (response.is_pointer_button_down_on() || response.clicked_by(PointerButton::Primary))
    {
        app.actions.push(Action::Press);
    }
    response.clicked()
}

/// A time after a spinner edit. A typed hour or minute past its range is
/// taken as the largest it can be.
fn edit_time(time: ClockTime, edit: SpinEdit) -> ClockTime {
    match edit {
        SpinEdit::Step { segment, by } => step_time(time, segment, by),
        SpinEdit::Typed { segment: 0, value } => ClockTime::new(value.min(23), time.minute()),
        SpinEdit::Typed { value, .. } => ClockTime::new(time.hour(), value.min(59)),
    }
}

fn time_segments(time: ClockTime) -> [String; 2] {
    [
        format!("{:02}", time.hour()),
        format!("{:02}", time.minute()),
    ]
}

/// One step of a time spinner: hours wrap around the day; minutes move in
/// fives and wrap within the hour, as in the Windows time picker.
fn step_time(time: ClockTime, segment: usize, step: i32) -> ClockTime {
    let total = i32::from(time.minutes());
    if segment == 0 {
        return ClockTime::from_minutes(total + 60 * step);
    }
    let hour = i32::from(time.hour()) * 60;
    let minute = i32::from(time.minute());
    let fives = if step > 0 {
        minute / MINUTE_STEP
    } else {
        (minute + MINUTE_STEP - 1) / MINUTE_STEP
    };
    ClockTime::from_minutes(hour + ((fives + step) * MINUTE_STEP).rem_euclid(60))
}

fn edit_start(app: &mut App, edit: Option<SpinEdit>) {
    if let Some(edit) = edit {
        app.actions
            .push(Action::SetStart(edit_time(app.settings.start, edit)));
    }
}

fn edit_end(app: &mut App, edit: Option<SpinEdit>) {
    if let Some(edit) = edit {
        app.actions
            .push(Action::SetEnd(edit_time(app.settings.end, edit)));
    }
}

/// The Fade spinner's edit: steps of five minutes, up to two hours.
fn edit_fade(app: &mut App, edit: Option<SpinEdit>) {
    let fade = app.settings.fade_minutes;
    let minutes = match edit {
        Some(SpinEdit::Step { by, .. }) => (i32::from(fade) / FADE_STEP + by) * FADE_STEP,
        Some(SpinEdit::Typed { value, .. }) => i32::from(value),
        None => return,
    };
    app.actions.push(Action::SetFade(
        minutes.clamp(0, i32::from(FADE_MAX_MINUTES)) as u16,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumper_beats_on_the_desert_detent() {
        let desert = detent_stops()
            .iter()
            .position(|stop| stop.kelvin == DESERT_KELVIN)
            .expect("Desert among the detents");
        // The same f32: the thumper takes a value the snap and the keys put
        // there, and compares them exactly.
        let thumper = THUMPER.expect("a thumper");
        assert_eq!(thumper.to_bits(), DETENT_POSITIONS[desert].to_bits());
        assert_eq!(color::kelvin_at_redness(thumper), DESERT_KELVIN);
    }

    #[test]
    fn hours_wrap_around_the_day() {
        assert_eq!(
            step_time(ClockTime::new(23, 30), 0, 1),
            ClockTime::new(0, 30)
        );
        assert_eq!(
            step_time(ClockTime::new(0, 30), 0, -1),
            ClockTime::new(23, 30)
        );
    }

    #[test]
    fn typed_times_stay_on_the_clock() {
        let time = ClockTime::new(21, 30);
        let typed = |segment, value| edit_time(time, SpinEdit::Typed { segment, value });
        assert_eq!(typed(0, 7), ClockTime::new(7, 30));
        assert_eq!(typed(0, 95), ClockTime::new(23, 30));
        assert_eq!(typed(1, 45), ClockTime::new(21, 45));
        assert_eq!(typed(1, 99), ClockTime::new(21, 59));
    }

    #[test]
    fn minutes_move_in_fives_within_the_hour() {
        assert_eq!(
            step_time(ClockTime::new(21, 3), 1, 1),
            ClockTime::new(21, 5)
        );
        assert_eq!(
            step_time(ClockTime::new(21, 3), 1, -1),
            ClockTime::new(21, 0)
        );
        assert_eq!(
            step_time(ClockTime::new(21, 0), 1, -1),
            ClockTime::new(21, 55)
        );
        assert_eq!(
            step_time(ClockTime::new(21, 55), 1, 1),
            ClockTime::new(21, 0)
        );
    }

    /// Lays the window out for the night look `night` and lists what its
    /// controls tell a screen reader, with the shapes it draws.
    fn laid_out(night: Look) -> (Vec<egui::Shape>, Vec<String>) {
        let mut output = testing::Screen::new(night)
            .with_screen_reader()
            .output(vec![]);
        let labels = testing::labels(&mut output);
        (testing::shapes(&mut output), labels)
    }

    /// The window's 14 controls, and nothing else, tell a screen reader
    /// something: no theme picker, and no "Color" or "Schedule" header, which
    /// the window drew as plain text and so never told it of.
    #[test]
    fn the_window_exposes_its_controls() {
        let mut labels = laid_out(Look::default()).1;
        let mut want: Vec<String> = [
            // The default look sits on the Ember stop, which the label names.
            "Temperature: 1200 K, Ember",
            "Brightness: 100%",
            "Turn red automatically",
            "From: 21:00",
            "To: 07:00",
            "Fade minutes: 20",
            "From up",
            "From down",
            "To up",
            "To down",
            "Fade minutes up",
            "Fade minutes down",
            "Red Now",
            "Close",
        ]
        .map(str::to_owned)
        .into();
        labels.sort();
        want.sort();
        assert_eq!(labels, want);
    }

    #[test]
    fn on_the_desert_stop_the_readout_says_the_kelvin_and_the_label_adds_the_name() {
        let (shapes, labels) = laid_out(Look {
            kelvin: DESERT_KELVIN,
            brightness: 100,
        });
        let want = format!("Temperature: {DESERT_KELVIN} K, Desert");
        assert!(labels.contains(&want), "{want} missing from {labels:?}");
        // The readout, in 14, shows the kelvin as it does on every other
        // stop, and the card's title says it in 28; the name is written
        // once, under the strip, in 11.5.
        let sizes_of = |text: &str| -> Vec<f32> {
            let mut sizes: Vec<f32> = testing::texts(&shapes)
                .into_iter()
                .filter(|shape| shape.galley.text() == text)
                .map(|shape| shape.galley.job.sections[0].format.font_id.size)
                .collect();
            sizes.sort_by(f32::total_cmp);
            sizes
        };
        assert_eq!(sizes_of("Desert"), [11.5]);
        assert_eq!(sizes_of(&format!("{DESERT_KELVIN} K")), [14.0, 28.0]);
    }

    /// A shot pictures the window focused, whatever app has the keyboard
    /// while it is taken. A widget's `has_focus`
    /// reads the window's focus from `RawInput::focused`, which winit sets
    /// from the real window, so the hook must set it too: with only the
    /// viewport's info, the `focused` scene shows no knob and no focus ring
    /// unless the window really has the keyboard.
    ///
    /// This test guarantees it: `Window::raw_input_hook` turns
    /// `RawInput::focused` on while a shot is taken, even when winit reported
    /// the window unfocused.
    #[cfg(feature = "demo")]
    #[test]
    fn a_shot_makes_the_window_focused_to_its_widgets() {
        let waker = fastframe_shell::Waker::default();
        fastframe_shell::Shell::new(crate::app::test_app(|| 12.0 * 60.0), &waker)
            .run(|lease| {
                let ctx = egui::Context::default();
                let path = std::env::temp_dir().join("arrakis-never-saved.png");
                let shot = crate::demo::Shot::new(path, crate::demo::Scene::Focused);
                let mut window = Window::new(lease.take(&ctx)).with_shot(Some(shot));
                // As winit hands it over while another app has the keyboard.
                let mut raw = egui::RawInput {
                    focused: false,
                    ..Default::default()
                };
                eframe::App::raw_input_hook(&mut window, &ctx, &mut raw);
                assert!(raw.focused, "the shot's window is not focused to egui");
                Ok::<(), ()>(())
            })
            .unwrap();
    }

    /// A window on a test app whose clock reads `clock`, in a context with
    /// the theme installed as the app installs it, for `check`.
    fn with_window(clock: fn() -> f64, check: impl FnOnce(&mut Window, &egui::Context)) {
        let waker = fastframe_shell::Waker::default();
        let mut check = Some(check);
        fastframe_shell::Shell::new(crate::app::test_app(clock), &waker)
            .run(|lease| {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx);
                let mut window = Window::new(lease.take(&ctx));
                if let Some(check) = check.take() {
                    check(&mut window, &ctx);
                }
                Ok::<(), ()>(())
            })
            .unwrap();
    }

    /// Pixels to the point in every pass here: a Retina Mac's.
    const SCALE: f32 = 2.0;
    /// The limit this Mac's painter reports, which eframe hands egui: 16384
    /// from glow's GL and from the Metal painter's device alike.
    const PAINTER_LIMIT: usize = 16384;

    /// What winit hands eframe for a pass: the window's size, at `SCALE`
    /// pixels to the point, and the painter's limit, which eframe puts in the
    /// first input only. `testing::run` builds an input of its own with
    /// neither the scale nor the limit; the one line the two share, the
    /// screen's rect, is not worth a builder of its own.
    fn input(limit: Option<usize>) -> egui::RawInput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                crate::theme::WINDOW,
            )),
            max_texture_side: limit,
            ..Default::default()
        };
        let viewport = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        viewport.native_pixels_per_point = Some(SCALE);
        viewport.focused = Some(true);
        input
    }

    /// One pass as eframe runs it, but for `Window::logic`'s close animation,
    /// which needs a frame: the hook on the input, then in egui's pass the
    /// poll of the engine (`App::background_frame`, so the status line reads
    /// what the engine sent), the window's drawing and the actions it pushed,
    /// as `Window::ui` does them (a press on a control moves nothing until
    /// they are applied). The textures it sets leave with it, as no renderer
    /// is here to take them: their images are returned, a full upload or a
    /// patch each.
    fn pass(
        window: &mut Window,
        ctx: &egui::Context,
        mut input: egui::RawInput,
    ) -> Vec<egui::epaint::ImageDelta> {
        eframe::App::raw_input_hook(window, ctx, &mut input);
        let mut output = ctx.run_ui(input, |ui| {
            window.app.background_frame(Some(ui.ctx()));
            draw(&mut window.app, ui);
            window.app.apply_actions(Some(ctx));
        });
        let set = std::mem::take(&mut output.textures_delta.set);
        output.textures_delta.clear();
        set.into_values().flatten().collect()
    }

    /// The font atlas's size, as the context has it after a pass.
    fn atlas(ctx: &egui::Context) -> [usize; 2] {
        ctx.fonts(|fonts| fonts.font_image_size())
    }

    /// eframe hands egui the painter's limit, and egui makes the font atlas
    /// that wide: 4 MiB at the 16384 this Mac's painter reports, for glyphs
    /// that fill about 8% of it. The hook caps the limit, so the atlas is made
    /// at the cap, whatever the figure is: this test does not pin 2048, which
    /// is not the owner's choice (`MAX_TEXTURE_SIDE` says why it is the cap),
    /// only that the cap is in force and below the painter's limit.
    #[test]
    fn the_atlas_is_made_at_the_cap_and_not_as_wide_as_the_painter_says() {
        const { assert!(MAX_TEXTURE_SIDE < PAINTER_LIMIT) };
        with_window(
            || 12.0 * 60.0,
            |window, ctx| {
                pass(window, ctx, input(Some(PAINTER_LIMIT)));
                assert_eq!(atlas(ctx)[0], MAX_TEXTURE_SIDE);
            },
        );
        // The control: a pass that does not go through the hook is given the
        // painter's limit whole, which is what the cap is for.
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut output = ctx.run_ui(input(Some(PAINTER_LIMIT)), |_| {});
        output.textures_delta.clear();
        assert_eq!(atlas(&ctx)[0], PAINTER_LIMIT);
    }

    /// The cap never raises a limit: a GL that takes less than the cap keeps
    /// its own. Only the first input carries the limit, and the hook must
    /// not invent one for a later input that carries none: egui keeps the
    /// last value it was given, and a changed one would make the atlas again.
    #[test]
    fn a_limit_below_the_cap_stays_and_a_later_input_without_one_is_left_alone() {
        // The smallest side egui's atlas takes (`TextureAtlas::new`).
        const SMALLEST: usize = 1024;
        const { assert!(SMALLEST < MAX_TEXTURE_SIDE) };
        with_window(
            || 12.0 * 60.0,
            |window, ctx| {
                let mut first = input(Some(SMALLEST));
                eframe::App::raw_input_hook(window, ctx, &mut first);
                assert_eq!(first.max_texture_side, Some(SMALLEST));
                pass(window, ctx, input(Some(SMALLEST)));
                assert_eq!(atlas(ctx)[0], SMALLEST);
                // The next pass, as winit hands it: no limit.
                let mut later = input(None);
                eframe::App::raw_input_hook(window, ctx, &mut later);
                assert_eq!(later.max_texture_side, None);
                pass(window, ctx, input(None));
                assert_eq!(atlas(ctx)[0], SMALLEST, "the atlas was made again");
            },
        );
    }

    /// A key event for `shortcut`, as winit hands it to eframe.
    fn pressed(shortcut: egui::KeyboardShortcut) -> egui::Event {
        egui::Event::Key {
            key: shortcut.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: shortcut.modifiers,
        }
    }

    /// egui zooms its whole interface with Cmd+Plus, Cmd+= (the same key
    /// without Shift), Cmd+Minus and Cmd+0, unless the context says not to.
    /// The window is a fixed size, so a zoom only crops it, and the steps
    /// from 1 to 5 made the font atlas again 7 times, the first past zoom
    /// 2.6: `theme::install` turns it off. Here the context is the one the
    /// app sets up, and each shortcut goes through the window's own pass,
    /// which is the same for a window that is reopened, since it gets a new
    /// context.
    #[test]
    fn the_keyboard_zoom_is_off() {
        use egui::gui_zoom::kb_shortcuts::{ZOOM_IN, ZOOM_IN_SECONDARY, ZOOM_OUT, ZOOM_RESET};
        with_window(
            || 12.0 * 60.0,
            |window, ctx| {
                // A new zoom takes effect in the pass after the one that
                // sets it, and the pass that consumes a key sets it: the
                // zoom is read after two more.
                let settle = |window: &mut Window| {
                    for _ in 0..2 {
                        pass(window, ctx, input(None));
                    }
                };
                let mut moved = vec![];
                // Cmd+0 brings a zoom back to 1.0, so it starts from another
                // one. The app puts none there; the test does.
                for (name, shortcut, start) in [
                    ("Cmd+Plus", ZOOM_IN, 1.0),
                    ("Cmd+=", ZOOM_IN_SECONDARY, 1.0),
                    ("Cmd+Minus", ZOOM_OUT, 1.0),
                    ("Cmd+0", ZOOM_RESET, 1.5),
                ] {
                    ctx.set_zoom_factor(start);
                    settle(window);
                    assert_eq!(ctx.zoom_factor(), start, "{name}: the zoom was not set");
                    let mut raw = input(None);
                    raw.events.push(pressed(shortcut));
                    pass(window, ctx, raw);
                    settle(window);
                    if ctx.zoom_factor() != start {
                        moved.push(format!("{name}: {start} to {}", ctx.zoom_factor()));
                    }
                }
                assert!(moved.is_empty(), "the zoom moved: {moved:?}");
            },
        );
    }

    /// A drag of the pointer over the whole Temperature slider, out to the
    /// pure red end and back, at 2 pixels to the point, once for each of
    /// `frames` frames each way: the readout's digits in two sizes, at a new
    /// place for each new value, are the glyphs the window makes while it is
    /// in use. The last frame lets go.
    ///
    /// The pointer starts at the Day end, as `--demo-drive drag` does.
    fn drag_the_temperature_slider(
        window: &mut Window,
        ctx: &egui::Context,
        frames: usize,
        mut each_frame: impl FnMut(&mut Window, &egui::Context),
    ) {
        let (day, red) = crate::ui::view::TRACK;
        let at = |x| egui::pos2(x, crate::ui::view::TEMPERATURE_Y);
        let mut limit = Some(PAINTER_LIMIT);
        // egui hit-tests a press against the frame before, so two frames
        // draw before the pointer comes (`drive::FRAMES_BEFORE_PRESS`).
        for _ in 0..2 {
            pass(window, ctx, input(limit.take()));
            each_frame(window, ctx);
        }
        for step in 0..=2 * frames {
            let out = step.min(2 * frames - step) as f32 / frames as f32;
            let pos = at(day + (red - day) * out);
            let mut raw = input(limit.take());
            raw.events.push(egui::Event::PointerMoved(pos));
            if step == 0 {
                raw.events.push(testing::button(pos, true));
            }
            pass(window, ctx, raw);
            each_frame(window, ctx);
        }
        let mut raw = input(None);
        raw.events.push(testing::button(at(day), false));
        pass(window, ctx, raw);
        each_frame(window, ctx);
    }

    /// A drag of the Temperature slider across its whole range and back, in
    /// the night window at `SCALE` pixels to the point. At the cap the
    /// glyphs of a whole drag fill the atlas to 0.06 (2048 x 256). egui makes
    /// it again, and sends it again, from the frame that ends it more than 0.8
    /// full: the fill has to stay under half of that. And the width is the
    /// cap, not the painter's limit.
    #[test]
    fn a_drag_over_the_whole_temperature_slider_fills_the_atlas_far_from_the_line_where_egui_builds_it_again()
     {
        use crate::color::{DAY_KELVIN, PURE_RED_KELVIN};
        // The fill past which egui builds the atlas again: a private value
        // of epaint 0.36.2 (`text/fonts.rs`, `font_atlas_almost_full`).
        const EGUI_REBUILDS_ABOVE: f32 = 0.8;
        // Under half of the line, not under the line: the drag types no
        // digits into a time field, and the status line reads whatever the
        // engine has sent by each pass, which varies from run to run. Those
        // glyphs come on top of the measured 0.063, and a fill that reached
        // the line would have left the margin long before.
        const FILL_BELOW: f32 = EGUI_REBUILDS_ABOVE / 2.0;
        // About 2 points a frame: the readout takes 566 values over the
        // 372 points of the track.
        const FRAMES: usize = 187;
        let mut fills = vec![];
        let mut lowest = u16::MAX;
        // The window at half past eleven at night, as the `night` shot is.
        with_window(
            || 23.0 * 60.0 + 30.0,
            |window, ctx| {
                drag_the_temperature_slider(window, ctx, FRAMES, |window, ctx| {
                    let fill = ctx.fonts(|fonts| fonts.font_atlas_fill_ratio());
                    let [width, height] = atlas(ctx);
                    let frame = fills.len();
                    assert!(
                        fill < FILL_BELOW,
                        "frame {frame}: fill {fill} in {width}x{height}"
                    );
                    assert_eq!(width, MAX_TEXTURE_SIDE, "frame {frame}");
                    fills.push(fill);
                    lowest = lowest.min(window.app.settings.night.kelvin);
                });
                // The drag came back to the Day end, where it started.
                assert_eq!(window.app.settings.night.kelvin, DAY_KELVIN);
            },
        );
        // And it went out to the pure red end first: across the whole range,
        // and its glyphs are in the atlas, or this proves nothing.
        assert_eq!(lowest, PURE_RED_KELVIN);
        assert!(fills.last() > fills.first(), "the drag made no new glyphs");
    }

    /// Every texture a frame loads has its largest side within the cap, and
    /// the cover is among them, not only the font atlas: the cover is the
    /// largest picture the app loads, 136 points wide, so 272 px at `SCALE`.
    /// `cover::draw` has no clamp on the scale, and `prebuilt::MAX_SCALE`
    /// bounds only the thread that builds ahead, so a real scale over about 15
    /// would pass the cap. None is near; this proves the path, not a bound.
    /// In a debug build `load_texture`'s `debug_assert!` may panic before the
    /// assertions here are reached, and that fails the test as well.
    #[test]
    fn every_texture_a_frame_loads_is_within_the_cap_and_the_cover_is_among_them() {
        let cover = (view::COVER_RECT.width() * SCALE).round() as usize;
        let mut sizes = vec![];
        with_window(
            || 23.0 * 60.0 + 30.0,
            |window, ctx| {
                // The first frame loads the textures; the next ones patch them.
                for frame in 0..4 {
                    let limit = (frame == 0).then_some(PAINTER_LIMIT);
                    for delta in pass(window, ctx, input(limit)) {
                        sizes.push(delta.image.size());
                    }
                }
            },
        );
        assert!(
            sizes
                .iter()
                .all(|&[width, height]| width.max(height) <= MAX_TEXTURE_SIDE),
            "a texture is over the cap: {sizes:?}"
        );
        assert!(
            sizes.contains(&[cover, cover]),
            "the cover was not loaded: {sizes:?}"
        );
    }

    /// What a screen reader hears after "Temperature:" at `kelvin`.
    fn spoken(kelvin: u16) -> String {
        let look = Look {
            kelvin,
            brightness: 100,
        };
        let (_, readout) = temperature(look);
        temperature_spoken(look, &readout)
    }

    #[test]
    fn a_named_stop_adds_its_name_to_what_a_screen_reader_hears() {
        assert_eq!(spoken(6500), "6500 K, Daylight");
        assert_eq!(spoken(2700), "2700 K, Incandescent");
        assert_eq!(spoken(1200), "1200 K, Ember");
        assert_eq!(spoken(DESERT_KELVIN), format!("{DESERT_KELVIN} K, Desert"));
        for kelvin in [DESERT_KELVIN - 10, DESERT_KELVIN + 10] {
            assert_eq!(spoken(kelvin), format!("{kelvin} K"));
        }
        // At pure red the readout already is the name.
        assert_eq!(spoken(850), "Pure red");
        assert_eq!(spoken(800), "Pure red");
        // Between the stops it says what the slider shows.
        assert_eq!(spoken(1500), "1500 K");
        assert_eq!(spoken(1210), "1210 K");
    }

    #[test]
    fn a_value_exactly_on_a_stop_is_on_it() {
        let name = |kelvin| stop_at(kelvin).map(|stop| stop.name);
        assert_eq!(name(6500), Some("Daylight"));
        assert_eq!(name(2700), Some("Incandescent"));
        assert_eq!(name(1200), Some("Ember"));
        assert_eq!(name(DESERT_KELVIN), Some("Desert"));
        assert_eq!(name(850), Some("Pure red"));
        // Below the scale is pure red too, as `Look::is_pure_red` says.
        assert_eq!(name(800), Some("Pure red"));
        // Near is not on: the lit stop follows 10 K steps.
        for kelvin in [
            1190,
            1210,
            2690,
            2710,
            6490,
            1500,
            3000,
            DESERT_KELVIN - 10,
            DESERT_KELVIN + 10,
        ] {
            assert_eq!(name(kelvin), None, "{kelvin} K");
        }
    }

    /// The stops a window lights, which is how its strip shows the stop the
    /// value sits on: the names it draws in ink. The names are caption text:
    /// the readout and the card say "Pure red" too, in larger type.
    fn lit_stops(shapes: &[egui::Shape]) -> Vec<String> {
        let palette = crate::theme::Palette::DARK;
        testing::texts(shapes)
            .into_iter()
            .filter(|text| text.galley.job.sections[0].format.font_id.size == 11.5)
            .filter(|text| STOPS.iter().any(|stop| stop.name == text.galley.text()))
            .filter(|text| testing::ink(text) == palette.text)
            .map(|text| text.galley.text().to_owned())
            .collect()
    }

    /// A pass's input whose root viewport has `viewport` as its input.
    fn root_input(viewport: egui::ViewportInfo) -> egui::RawInput {
        egui::RawInput {
            viewports: std::iter::once((egui::ViewportId::ROOT, viewport)).collect(),
            ..Default::default()
        }
    }

    /// What `closes` says in one full pass of `ctx` whose root viewport has
    /// `viewport` as its input.
    fn closes_in_pass(ctx: &egui::Context, viewport: egui::ViewportInfo) -> bool {
        let mut closes_here = None;
        let mut output = ctx.run_ui(root_input(viewport), |ui| {
            closes_here = Some(closes(ui.ctx()));
        });
        // No renderer here to take the font atlas.
        output.textures_delta.clear();
        closes_here.expect("the pass ran")
    }

    /// What `closes` says in a pass of `ctx` that runs `logic` alone, as
    /// eframe runs it while the window is minimized or covered.
    fn closes_in_logic_only_pass(ctx: &egui::Context, viewport: egui::ViewportInfo) -> bool {
        let mut closes_here = None;
        let _ = ctx.run_logic(&root_input(viewport), |ctx| {
            closes_here = Some(closes(ctx));
        });
        closes_here.expect("the logic ran")
    }

    #[test]
    fn a_pass_closes_only_when_its_input_asks_for_a_close() {
        let ctx = egui::Context::default();
        let close = egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..Default::default()
        };
        assert!(closes_in_pass(&ctx, close), "a pass asked to close");
        assert!(
            !closes_in_pass(&ctx, egui::ViewportInfo::default()),
            "a pass with default input"
        );
    }

    #[test]
    fn a_minimized_window_closes_in_a_pass_that_runs_logic_alone() {
        // While the window is minimized or covered, eframe runs `logic` with
        // no ui pass (`update_logic_only`), and still drops the window after
        // one that asks for a close: the menu bar's Quit can close it there.
        let ctx = egui::Context::default();
        assert!(
            !closes_in_pass(&ctx, egui::ViewportInfo::default()),
            "the last full pass asked for nothing"
        );
        let minimized = egui::ViewportInfo {
            minimized: Some(true),
            ..Default::default()
        };
        assert!(
            !closes_in_logic_only_pass(&ctx, minimized.clone()),
            "a minimized window that stays"
        );
        let closing = egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..minimized
        };
        assert!(
            closes_in_logic_only_pass(&ctx, closing),
            "a minimized window asked to close, as its logic sees it"
        );
    }

    #[test]
    fn other_viewport_input_is_not_a_close() {
        let ctx = egui::Context::default();
        let busy = egui::ViewportInfo {
            title: Some("Arrakis".to_owned()),
            native_pixels_per_point: Some(2.0),
            focused: Some(true),
            minimized: Some(false),
            ..Default::default()
        };
        assert!(
            !closes_in_pass(&ctx, busy),
            "a focused window with its title, scale and state set, and no close"
        );
    }

    #[test]
    fn the_lit_stop_and_the_screen_reader_name_the_same_stop() {
        // The scale has a point every 10 K, so a file that says 1205 K puts
        // the strip's edge at 1200 K's place. The look is still 1205 K, which
        // is on no stop: the view must not light Ember while the screen
        // reader says 1205 K.
        let desert = format!("Temperature: {DESERT_KELVIN} K, Desert");
        let beside_desert = format!("Temperature: {} K", DESERT_KELVIN + 5);
        for (kelvin, lit, label) in [
            (1200, Some("Ember"), "Temperature: 1200 K, Ember"),
            (1205, None, "Temperature: 1205 K"),
            (1195, None, "Temperature: 1195 K"),
            (
                2700,
                Some("Incandescent"),
                "Temperature: 2700 K, Incandescent",
            ),
            (2705, None, "Temperature: 2705 K"),
            (6500, Some("Daylight"), "Temperature: 6500 K, Daylight"),
            (850, Some("Pure red"), "Temperature: Pure red"),
            (855, None, "Temperature: 855 K"),
            // Desert lights its name, and the label names it. 5 K beside it
            // is on no stop, as 1205 K is on none.
            (DESERT_KELVIN, Some("Desert"), desert.as_str()),
            (DESERT_KELVIN + 5, None, beside_desert.as_str()),
        ] {
            let (shapes, labels) = laid_out(Look {
                kelvin,
                brightness: 100,
            });
            assert_eq!(
                lit_stops(&shapes),
                lit.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                "{kelvin} K lit"
            );
            assert!(
                labels.iter().any(|spoken| spoken == label),
                "{kelvin} K: {label} missing from {labels:?}"
            );
        }
    }

    #[test]
    fn the_owner_named_the_stop_desert_and_put_it_at_1760_k() {
        // The owner's choices of 2026-10-23: its place, and its name
        // ("Arrakis" is the app's). The other tests read the stop from the
        // table, so this one is the only place the literals live.
        assert_eq!(DESERT_KELVIN, 1760);
        let names: Vec<&str> = STOPS.iter().map(|stop| stop.name).collect();
        assert_eq!(
            names,
            ["Daylight", "Incandescent", "Desert", "Ember", "Pure red"]
        );
    }

    #[test]
    fn the_stops_sit_where_the_spec_puts_them() {
        let positions: Vec<f32> = STOPS.iter().map(Stop::position).collect();
        assert_eq!(positions[0], 0.0);
        assert_eq!(positions[4], 1.0);
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{positions:?} do not rise"
        );
        // On the 372-point travel from x 24, the ticks of 2700 K, Desert
        // and 1200 K sit at x 137.8, 208.7 and 279.8.
        for (position, x) in [
            (positions[1], 137.8),
            (positions[2], 208.7),
            (positions[3], 279.8),
        ] {
            let at = 24.0 + 372.0 * position;
            assert!((at - x).abs() < 0.05, "a stop at {at}, not {x}");
        }
    }

    #[test]
    fn desert_is_the_grid_kelvin_nearest_the_midpoint_between_its_neighbors() {
        let (incandescent, ember) = (STOPS[1].position(), STOPS[3].position());
        let midpoint = (incandescent + ember) / 2.0;
        // The kelvins of the scale's 10 K grid that read back as themselves,
        // which is what a detent needs to hold the value exactly.
        let nearest = (STOPS[3].kelvin..=STOPS[1].kelvin)
            .step_by(10)
            .filter(|&kelvin| color::kelvin_at_redness(color::redness_of(kelvin)) == kelvin)
            .min_by(|&a, &b| {
                let off = |kelvin| (color::redness_of(kelvin) - midpoint).abs();
                off(a).total_cmp(&off(b))
            });
        assert_eq!(nearest, Some(DESERT_KELVIN));
    }

    /// A slider that has moved to `moved_to`, made through one frame.
    fn slider_moved_to(moved_to: Option<f32>) -> Slider {
        let mut slider = None;
        testing::frame(&testing::context(), vec![], |ui| {
            slider = Some(input::slider(
                ui,
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 16.0)),
                egui::Rangef::new(0.0, 100.0),
                egui::Id::new("test slider"),
                0.5,
                0.1,
                None,
                None,
            ));
        });
        let mut slider = slider.expect("a frame ran");
        slider.moved_to = moved_to;
        slider
    }

    #[test]
    fn only_a_moved_slider_changes_the_look() {
        let night = Look {
            kelvin: 1500,
            brightness: 60,
        };
        assert_eq!(with_temperature(night, &slider_moved_to(None)), night);
        let to = |redness| with_temperature(night, &slider_moved_to(Some(redness)));
        // Only the temperature moves.
        assert_eq!(
            to(color::redness_of(2700)),
            Look {
                kelvin: 2700,
                brightness: 60,
            }
        );
        assert_eq!(to(1.0).kelvin, color::PURE_RED_KELVIN);
        assert_eq!(to(0.0).kelvin, color::DAY_KELVIN);
    }
}
