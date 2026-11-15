//! The window, laid out like a music player: the night look as the track
//! now playing, with the night itself as its progress bar, then the
//! settings, and a player bar at the foot with Red Now and Close.

use egui::{
    Align2, Color32, Id, Pos2, Rangef, Rect, Response, Stroke, StrokeKind, Ui, UiBuilder, pos2,
    vec2,
};

use crate::app::App;
use crate::color::{self, Look};
use crate::model::Action;
use crate::theme::{self, Cover, Palette};
use crate::ui::detent::Detents;
use crate::ui::input::{self, SpinEdit};
use crate::ui::{self as shared, paint};

const LEFT: f32 = theme::MARGIN;
const RIGHT: f32 = theme::WINDOW.x - theme::MARGIN;
const CARD: Rect = Rect {
    min: pos2(LEFT, theme::TITLE_BAR + 8.0),
    max: pos2(RIGHT, theme::TITLE_BAR + 8.0 + COVER + 24.0),
};
const COVER: f32 = 136.0;
/// Where the cover sits in the window. `--demo-diff` names it too, and
/// `cover::prebuilt` builds the cover's images for its size before a window
/// opens.
pub(crate) const COVER_RECT: Rect = Rect {
    min: pos2(CARD.min.x + 12.0, CARD.min.y + 12.0),
    max: pos2(CARD.min.x + 12.0 + COVER, CARD.min.y + 12.0 + COVER),
};
/// The window's palette: it has one design.
const PALETTE: Palette = Palette::DARK;
/// The scene the cover is drawn in, the window palette's. `cover::prebuilt`
/// builds the cover's images for it before a window opens.
pub(crate) const COVER_SCENE: Cover = PALETTE.cover;
/// The card's text column: from beside the cover to where the track ends.
const COLUMN_LEFT: f32 = CARD.min.x + 12.0 + COVER + 16.0;
const COLUMN_RIGHT: f32 = CARD.max.x - 14.0;
const COLUMN: f32 = COLUMN_RIGHT - COLUMN_LEFT;
const TEMPERATURE_TOP: f32 = 238.0;
const BRIGHTNESS_TOP: f32 = 310.0;
/// A slider's strip is centered this far below its row's top.
const STRIP_BELOW_TOP: f32 = 28.0;
/// The ends of both strips in x, in points: from the Day end to the far one.
/// `--demo-drive drag` and the benchmarks aim the pointer along them.
pub(crate) const TRACK: (f32, f32) = (LEFT, RIGHT);
/// The Temperature and Brightness strips' center lines, in y, for the same.
#[cfg(any(test, feature = "demo"))]
pub(crate) const TEMPERATURE_Y: f32 = TEMPERATURE_TOP + STRIP_BELOW_TOP;
#[cfg(feature = "demo")]
pub(crate) const BRIGHTNESS_Y: f32 = BRIGHTNESS_TOP + STRIP_BELOW_TOP;
const AUTO_Y: f32 = 392.0;
const FIELD_LABEL_Y: f32 = 426.0;
const FIELD_Y: f32 = 456.0;
const COLUMN_GAP: f32 = 12.0;

/// The whole window.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let palette = PALETTE;
    let window = Rect::from_min_size(Pos2::ZERO, theme::WINDOW);
    ui.painter().rect_filled(window, 0, palette.window);
    let bar = Rect::from_min_size(Pos2::ZERO, vec2(theme::WINDOW.x, theme::TITLE_BAR));
    fastframe_titlebar::drag(ui, bar);

    let night = color_section(app, ui, &palette);
    schedule_section(app, ui, &palette);
    player_bar(app, ui, &palette);
    now_playing(app, ui, night, &palette);
}

/// The accent hairline around what has the keyboard.
fn focus_ring(ui: &Ui, rect: Rect, radius: f32, palette: &Palette) {
    ui.painter().rect_stroke(
        rect.expand(2.0),
        (radius + 2.0).round() as u8,
        Stroke::new(1.0, palette.accent),
        StrokeKind::Outside,
    );
}

/// The night look as the track now playing: its cover, the look as the
/// title, the state under it, and the night's progress.
fn now_playing(app: &App, ui: &Ui, night: Look, palette: &Palette) {
    let tint = shared::preview_tint(app, night);
    let t = |color: Color32| tint.apply(color);
    let painter = ui.painter();
    // A wash of the warm of the cover's low sky, as the player tints its
    // backdrop.
    painter.rect_filled(
        CARD,
        theme::RADIUS,
        paint::mix(palette.surface, t(palette.wash), theme::CARD_WASH),
    );
    let cover = COVER_RECT;
    let (redness, _) = shared::temperature(night);
    shared::cover::draw(ui, cover, redness, &COVER_SCENE, tint);

    let x = COLUMN_LEFT;
    let title = if night.is_pure_red() {
        "Pure red".to_owned()
    } else {
        format!("{} K", night.kelvin)
    };
    let painter = ui.painter();
    // A line that is too long for the column ends in an ellipsis.
    paint::text_fit(
        painter,
        pos2(x, cover.min.y + 26.0),
        Align2::LEFT_CENTER,
        &title,
        theme::bold(28.0),
        palette.text,
        COLUMN,
    );
    paint::text_fit(
        painter,
        pos2(x, cover.min.y + 54.0),
        Align2::LEFT_CENTER,
        &app.status_line(),
        theme::medium(14.0),
        palette.secondary,
        COLUMN,
    );
    paint::text_fit(
        painter,
        pos2(x, cover.min.y + 74.0),
        Align2::LEFT_CENTER,
        &app.detail_line(),
        theme::regular(12.0),
        palette.faint,
        COLUMN,
    );

    // The night as the track: From to To, filled as far as the clock is.
    // The lines and the track sit centered against the cover.
    let track = Rect::from_min_max(
        pos2(x, cover.min.y + 99.0),
        pos2(COLUMN_RIGHT, cover.min.y + 102.0),
    );
    painter.rect_filled(track, 2, palette.track);
    let progress = if app.red_now {
        Some(1.0)
    } else {
        shared::night_progress(app)
    };
    if let Some(progress) = progress {
        painter.rect_filled(
            Rect::from_min_max(
                track.min,
                pos2(track.min.x + track.width() * progress, track.max.y),
            ),
            2,
            palette.text,
        );
    }
    let (start, end) = if app.settings.schedule_on {
        (app.settings.start.to_string(), app.settings.end.to_string())
    } else {
        // The state line above already says the schedule is off.
        (String::new(), String::new())
    };
    for (pos, anchor, label) in [
        (
            pos2(track.min.x, track.max.y + 13.0),
            Align2::LEFT_CENTER,
            start,
        ),
        (
            pos2(track.max.x, track.max.y + 13.0),
            Align2::RIGHT_CENTER,
            end,
        ),
    ] {
        paint::text(
            painter,
            pos,
            anchor,
            &label,
            theme::regular(11.5),
            palette.faint,
        );
    }
    ui.interact(CARD, Id::new("now-playing"), egui::Sense::hover())
        .on_hover_text("Your night look. The bar is tonight, from From to To.");
}

fn color_section(app: &mut App, ui: &mut Ui, palette: &Palette) -> Look {
    let night = app.settings.night;
    let live = app.status.tint;
    // The Temperature strip shows what white becomes at each position,
    // drawn so that the screen as it is now shows it.
    let scale = |position| color::scale_tint(position).through(live).light();
    let temperature_row = Row {
        top: TEMPERATURE_TOP,
        label: TEMPERATURE,
        value: shared::temperature(night).0,
        step: shared::REDNESS_STEP,
        names: Names::Stops(&shared::STOPS),
        light: Light::Scale(&scale),
        detents: Some(*shared::DETENTS),
        thumper: *shared::THUMPER,
    };
    let temperature = slider_input(ui, &temperature_row, palette);
    // The one temperature of this frame: the look as the Temperature slider
    // leaves it. The readout, what a screen reader hears, the lit stop, the
    // Brightness strip and the night all take it, so they never disagree,
    // even in the frame a detent catches the value. The night this returns
    // is what the card's title and cover draw.
    let moved = shared::with_temperature(night, &temperature);
    let (position, readout) = shared::temperature(moved);
    let spoken = shared::temperature_spoken(moved, &readout);
    slider_value(
        ui,
        &temperature_row,
        &temperature,
        position,
        &readout,
        &spoken,
        palette,
    );
    slider_strip(
        ui,
        &temperature_row,
        &temperature,
        shared::stop_at(moved.kelvin),
        palette,
    );
    // The Brightness strip shows the chosen light at full brightness, as
    // far as the screen goes, in the look this frame's Temperature leaves:
    // it follows a drag.
    let full = Look {
        brightness: color::MAX_BRIGHTNESS,
        ..moved
    };
    let brightness_row = Row {
        top: BRIGHTNESS_TOP,
        label: "Brightness",
        value: shared::brightness(night).0,
        step: shared::BRIGHTNESS_STEP,
        names: Names::Ends("Dim", "Full"),
        light: Light::Flat(shared::preview_tint(app, full).light()),
        detents: None,
        thumper: None,
    };
    let brightness = slider_input(ui, &brightness_row, palette);
    let moved = shared::with_brightness(moved, &brightness);
    let (position, readout) = shared::brightness(moved);
    slider_value(
        ui,
        &brightness_row,
        &brightness,
        position,
        &readout,
        &readout,
        palette,
    );
    slider_strip(ui, &brightness_row, &brightness, None, palette);
    shared::night_from(app, moved, &temperature, &brightness)
}

/// The Temperature slider's label, which also names its widget.
pub const TEMPERATURE: &str = "Temperature";

/// What a slider row names under its strip.
#[derive(Clone, Copy)]
enum Names<'a> {
    /// The two ends, and nothing between.
    Ends(&'a str, &'a str),
    /// A name for each stop, with a tick at every one inside the track. The
    /// stop the value sits on is lit.
    Stops(&'a [shared::Stop]),
}

/// The light a slider's strip shows, before the veil.
#[derive(Clone, Copy)]
enum Light<'a> {
    /// One color along the whole strip.
    Flat(Color32),
    /// The color at a position from 0 to 1.
    Scale(&'a dyn Fn(f32) -> Color32),
}

/// One row of a slider: what it says, and the light its strip shows.
struct Row<'a> {
    top: f32,
    label: &'a str,
    /// The position the frame starts with, from 0 to 1.
    value: f32,
    step: f32,
    names: Names<'a>,
    light: Light<'a>,
    /// Where the pointer snaps and the keys stop, if anywhere.
    detents: Option<Detents<'a>>,
    /// The detent where a pointer's hold beats as a thumper, if any.
    thumper: Option<f32>,
}

impl Row<'_> {
    /// The y of the strip's center line.
    fn y(&self) -> f32 {
        self.top + STRIP_BELOW_TOP
    }
}

/// The label of a row, and its slider: the part that takes the pointer and
/// the keys. Once the frame knows the look the slider leaves, give its value
/// with [`slider_value`] and draw its strip and names with [`slider_strip`].
fn slider_input(ui: &mut Ui, row: &Row, palette: &Palette) -> input::Slider {
    paint::text(
        ui.painter(),
        pos2(LEFT, row.top + 6.0),
        Align2::LEFT_CENTER,
        row.label,
        theme::medium(14.0),
        palette.text,
    );
    let y = row.y();
    let area = Rect::from_min_max(pos2(LEFT - 6.0, y - 8.0), pos2(RIGHT + 6.0, y + 8.0));
    input::slider(
        ui,
        area,
        Rangef::new(TRACK.0, TRACK.1),
        Id::new(row.label),
        row.value,
        row.step,
        row.detents,
        row.thumper,
    )
}

/// A row's value in the look the frame leaves: `readout` at the right, and
/// `spoken` after the label for a screen reader, with the slider at
/// `position`.
fn slider_value(
    ui: &Ui,
    row: &Row,
    slider: &input::Slider,
    position: f32,
    readout: &str,
    spoken: &str,
    palette: &Palette,
) {
    paint::text(
        ui.painter(),
        pos2(RIGHT, row.top + 6.0),
        Align2::RIGHT_CENTER,
        readout,
        theme::semibold(14.0),
        palette.text,
    );
    slider.describe(position, row.label, spoken);
}

/// A row's strip of light, lit up to the value, and the names of its stops
/// under it, with `lit` in ink. While hovered, focused or in hand, the
/// slider shows its ringed knob on the value; focus adds the accent ring.
fn slider_strip(
    ui: &Ui,
    row: &Row,
    slider: &input::Slider,
    lit: Option<&shared::Stop>,
    palette: &Palette,
) {
    let y = row.y();
    let shown = slider.shown(row.value);
    let active = slider.response.hovered() || slider.response.has_focus() || slider.dragging;
    let painter = ui.painter();
    let half = theme::SLIDER_TRACK / 2.0;
    let strip = Rect::from_min_max(pos2(LEFT, y - half), pos2(RIGHT, y + half));
    let veil = |color| palette.veiled(color);
    match row.light {
        Light::Flat(color) => paint::flat(painter, strip, shown, veil, color),
        Light::Scale(at) => paint::spectrum(painter, strip, shown, veil, at),
    }
    names_under(ui, y, row.names, lit, palette);
    // The knob goes last, over the tick that may sit under it.
    if active {
        let center = pos2(LEFT + (RIGHT - LEFT) * shown, y);
        knob(ui, center, slider.response.has_focus(), palette);
    }
}

/// A ringed knob centered at `center`; the ring parts the knob from a strip
/// that is bright through red. The accent focus ring goes round it when
/// `focused`.
fn knob(ui: &Ui, center: Pos2, focused: bool, palette: &Palette) {
    let radius = theme::SLIDER_KNOB / 2.0;
    let painter = ui.painter();
    painter.circle_filled(center, radius, palette.on_accent);
    painter.circle_filled(center, radius - theme::KNOB_RING, palette.knob);
    if focused {
        focus_ring(
            ui,
            Rect::from_center_size(center, vec2(theme::SLIDER_KNOB, theme::SLIDER_KNOB)),
            radius,
            palette,
        );
    }
}

/// The names under the strip whose center line is at `y`, and the ticks
/// of the stops inside it. The stop `lit` is in ink, with its tick; the
/// others are dim. A name sits centered on its tick, the ends flush with
/// the margins, except the name of a [`shared::Stop::midway`] stop, which
/// sits midway between its neighbors' names.
fn names_under(ui: &Ui, y: f32, names: Names, lit: Option<&shared::Stop>, palette: &Palette) {
    let painter = ui.painter();
    let name_y = y + theme::SLIDER_NAME_GAP;
    let font = theme::regular(11.5);
    match names {
        Names::Ends(left, right) => {
            for (x, anchor, name) in [
                (LEFT, Align2::LEFT_CENTER, left),
                (RIGHT, Align2::RIGHT_CENTER, right),
            ] {
                paint::text(
                    painter,
                    pos2(x, name_y),
                    anchor,
                    name,
                    font.clone(),
                    palette.dim,
                );
            }
        }
        Names::Stops(stops) => {
            let last = stops.len() - 1;
            let ink_of = |stop: &shared::Stop| {
                if lit == Some(stop) {
                    palette.text
                } else {
                    palette.dim
                }
            };
            // The ticks first, on whole device pixels: the x each name is
            // centered on, or flush from at an end.
            let ticks: Vec<f32> = stops
                .iter()
                .enumerate()
                .map(|(index, stop)| match index {
                    0 => LEFT,
                    index if index == last => RIGHT,
                    _ => {
                        let at = LEFT + (RIGHT - LEFT) * stop.position();
                        let top = y + theme::SLIDER_TICK_GAP;
                        let ink = ink_of(stop);
                        paint::tick(painter, at, top, theme::SLIDER_TICK_HEIGHT, ink)
                    }
                })
                .collect();
            // The x span each name covers there, to place a midway name by.
            let span = |index: usize| {
                let x = ticks[index];
                let width = paint::measure(painter, stops[index].name, font.clone()).x;
                match index {
                    0 => (x, x + width),
                    index if index == last => (x - width, x),
                    _ => (x - width / 2.0, x + width / 2.0),
                }
            };
            for (index, stop) in stops.iter().enumerate() {
                let (x, anchor) = if index == 0 {
                    (LEFT, Align2::LEFT_CENTER)
                } else if index == last {
                    (RIGHT, Align2::RIGHT_CENTER)
                } else if stop.midway {
                    let (_, before) = span(index - 1);
                    let (after, _) = span(index + 1);
                    // To the nearest whole point: the two gaps differ by at
                    // most 1.
                    (((before + after) / 2.0).round(), Align2::CENTER_CENTER)
                } else {
                    (ticks[index], Align2::CENTER_CENTER)
                };
                paint::text(
                    painter,
                    pos2(x, name_y),
                    anchor,
                    stop.name,
                    font.clone(),
                    ink_of(stop),
                );
            }
        }
    }
}

/// A pill switch at the right edge, in the accent when on.
fn switch(
    ui: &mut Ui,
    center_y: f32,
    id: &str,
    label: &str,
    on: bool,
    palette: &Palette,
) -> Response {
    let rect = Rect::from_min_size(
        pos2(RIGHT - theme::SWITCH.x, center_y - theme::SWITCH.y / 2.0),
        theme::SWITCH,
    );
    let id = Id::new(id);
    let response = input::toggle(ui, rect.expand(4.0), id, label, on);
    let t = ui.ctx().animate_bool_with_time(id.with("on"), on, 0.12);
    let radius = theme::SWITCH.y / 2.0;
    ui.painter().rect_filled(
        rect,
        radius as u8,
        paint::mix(palette.surface_active, palette.accent, t),
    );
    let knob_x = rect.min.x + 11.0 + (rect.width() - 22.0) * t;
    // White on the gold is one red through the tint: a dark ring keeps
    // the knob apart from the track when on.
    ui.painter().circle(
        pos2(knob_x, rect.center().y),
        8.0,
        palette.knob,
        Stroke::new(1.5, palette.on_accent.gamma_multiply(t)),
    );
    if response.has_focus() {
        focus_ring(ui, rect, radius, palette);
    }
    response
}

fn schedule_section(app: &mut App, ui: &mut Ui, palette: &Palette) {
    let on = app.settings.schedule_on;
    paint::text(
        ui.painter(),
        pos2(LEFT, AUTO_Y),
        Align2::LEFT_CENTER,
        "Turn red automatically",
        theme::medium(14.0),
        palette.text,
    );
    let auto = switch(
        ui,
        AUTO_Y,
        "schedule-on",
        "Turn red automatically",
        on,
        palette,
    );
    if shared::clicked(app, &auto) {
        app.actions.push(Action::SetSchedule(!on));
    }

    let mut fields = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(
        pos2(0.0, FIELD_LABEL_Y - 12.0),
        pos2(theme::WINDOW.x, FIELD_Y + theme::FIELD_HEIGHT / 2.0 + 4.0),
    )));
    if !on {
        fields.disable();
        // `disable` halves the opacity of everything this `Ui` paints, on top
        // of the quiet inks `time_field` picks for a disabled field itself
        // (`dim`, `outline` and a fill halfway to the window), which would
        // fade them twice. The fields stay disabled, and the opacity goes back.
        fields.set_opacity(1.0);
    }
    let ui = &mut fields;
    let width = (RIGHT - LEFT - 2.0 * COLUMN_GAP) / 3.0;
    let column = |index: usize| LEFT + (width + COLUMN_GAP) * index as f32;
    let edit = time_field(
        ui,
        column(0),
        width,
        "from",
        "From",
        &shared::time_segments(app.settings.start),
        None,
        2,
        palette,
    );
    shared::edit_start(app, edit);
    let edit = time_field(
        ui,
        column(1),
        width,
        "to",
        "To",
        &shared::time_segments(app.settings.end),
        None,
        2,
        palette,
    );
    shared::edit_end(app, edit);
    let edit = time_field(
        ui,
        column(2),
        width,
        "fade",
        "Fade minutes",
        &[app.settings.fade_minutes.to_string()],
        Some("min"),
        3,
        palette,
    );
    shared::edit_fade(app, edit);
    input::release_repeats(ui, &[Id::new("from"), Id::new("to"), Id::new("fade")]);
}

/// A column with its name over a text field of segments and a stepper at
/// its right. The field takes clicks on its parts, digits and the arrow
/// keys; the stepper steps the part selected.
#[allow(clippy::too_many_arguments)]
fn time_field(
    ui: &mut Ui,
    left: f32,
    width: f32,
    id: &str,
    label: &str,
    segments: &[String],
    unit: Option<&str>,
    max_digits: usize,
    palette: &Palette,
) -> Option<SpinEdit> {
    let enabled = ui.is_enabled();
    let (ink, quiet) = if enabled {
        (palette.text, palette.secondary)
    } else {
        (palette.dim, palette.dim)
    };
    let name = if label == "Fade minutes" {
        "Fade"
    } else {
        label
    };
    paint::text(
        ui.painter(),
        pos2(left, FIELD_LABEL_Y),
        Align2::LEFT_CENTER,
        name,
        theme::regular(12.5),
        quiet,
    );
    let field = Rect::from_min_max(
        pos2(left, FIELD_Y - theme::FIELD_HEIGHT / 2.0),
        pos2(
            left + width - theme::STEPPER.x - 4.0,
            FIELD_Y + theme::FIELD_HEIGHT / 2.0,
        ),
    );
    let id = Id::new(id);
    let mut spinner = input::Spinner::new(ui, field, id, label, segments);
    // Each part of the time stands in a cell of its own, and the cells are
    // the parts' places for a click. No box is drawn round the field: its
    // rect is still where it hovers, takes a click and rings the focus. A
    // cell is as wide as its count of digits times the widest digit, so it
    // and the cells after it keep their places while a held stepper changes
    // the value (the face's digits differ in width).
    let font = theme::semibold(15.0);
    let colon = paint::measure(ui.painter(), ":", font.clone()).x;
    let digit = paint::widest_digit(ui.painter(), &font);
    let mut x = field.min.x;
    let mut cells = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            x += colon + theme::COLON_GAP;
        }
        let digits = segment.chars().count() as f32 * digit;
        let cell = Rect::from_min_size(
            pos2(x, field.center().y - theme::CELL_HEIGHT / 2.0),
            vec2(digits + 2.0 * theme::CELL_PAD, theme::CELL_HEIGHT),
        );
        cells.push(cell);
        x = cell.max.x;
    }
    spinner.input(ui, &cells, max_digits);

    let painter = ui.painter();
    let (fill, edge) = if !enabled {
        (
            paint::mix(palette.surface, palette.window, 0.5),
            palette.outline,
        )
    } else if spinner.response.hovered() {
        (palette.surface_hover, palette.field_stroke)
    } else {
        (palette.surface, palette.field_stroke)
    };
    // The digits, the colon and the unit sit a point above the field's
    // middle line.
    let line = field.center().y - theme::CELL_LIFT;
    for (index, (segment, cell)) in segments.iter().zip(&cells).enumerate() {
        if index > 0 {
            let before = cells[index - 1].max.x;
            paint::text(
                painter,
                pos2((before + cell.min.x) / 2.0, line),
                Align2::CENTER_CENTER,
                ":",
                font.clone(),
                quiet,
            );
        }
        let cell_fill = if enabled && spinner.is_selected(index) {
            palette.selection
        } else {
            fill
        };
        painter.rect(
            *cell,
            theme::CELL_RADIUS,
            cell_fill,
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
        paint::text(
            painter,
            pos2(cell.center().x, line),
            Align2::CENTER_CENTER,
            segment,
            font.clone(),
            ink,
        );
    }
    if let (Some(unit), Some(last)) = (unit, cells.last()) {
        paint::text(
            painter,
            pos2(last.max.x + theme::UNIT_GAP, line),
            Align2::LEFT_CENTER,
            unit,
            theme::regular(13.0),
            quiet,
        );
    }
    if spinner.response.has_focus() {
        focus_ring(ui, field, 6.0, palette);
    }

    // The stepper: one pill, split into up and down.
    let stepper = Rect::from_min_size(pos2(field.max.x + 4.0, field.min.y), theme::STEPPER);
    let up = Rect::from_min_max(stepper.min, pos2(stepper.max.x, stepper.center().y));
    let down = Rect::from_min_max(pos2(stepper.min.x, stepper.center().y), stepper.max);
    let up = spinner.arrow(ui, up, "up", label, 1);
    let down = spinner.arrow(ui, down, "down", label, -1);
    let painter = ui.painter();
    painter.rect(
        stepper,
        theme::STEPPER_RADIUS,
        if enabled { palette.surface } else { fill },
        Stroke::new(1.0, edge),
        StrokeKind::Inside,
    );
    for (half, is_up) in [(&up, true), (&down, false)] {
        let pressed = half.is_pointer_button_down_on() && enabled;
        if pressed || (half.hovered() && enabled) {
            painter.with_clip_rect(half.rect).rect_filled(
                stepper.shrink(1.0),
                theme::STEPPER_RADIUS - 1,
                if pressed {
                    palette.surface_active
                } else {
                    palette.surface_hover
                },
            );
        }
        let center = half.rect.center() + vec2(0.0, if is_up { 1.0 } else { -1.0 });
        paint::chevron(painter, center, 7.0, 3.5, is_up, Stroke::new(1.4, ink));
    }
    if up.has_focus() || down.has_focus() {
        focus_ring(ui, stepper, f32::from(theme::STEPPER_RADIUS), palette);
    }
    spinner.finish(ui)
}

/// The player bar: Red Now at the left, Close at the right.
fn player_bar(app: &mut App, ui: &mut Ui, palette: &Palette) {
    let bar = Rect::from_min_max(
        pos2(0.0, theme::WINDOW.y - theme::BAR_HEIGHT),
        pos2(theme::WINDOW.x, theme::WINDOW.y),
    );
    ui.painter().rect_filled(bar, 0, palette.panel);
    paint::hairline(
        ui.painter(),
        bar.min.x,
        bar.max.x,
        bar.min.y,
        palette.outline,
    );

    // Both are a round key with its name beside it, and the hit area is the
    // key, its name and a little past the name.
    let middle = bar.center().y;
    let hit_width = |name: f32| theme::KEY + theme::KEY_GAP + name + theme::KEY_TAIL;
    let red_font = theme::medium(13.0);
    let red_name = paint::measure(ui.painter(), "Red Now", red_font.clone()).x;
    let red_now_rect = Rect::from_min_size(
        pos2(LEFT, middle - theme::KEY / 2.0),
        vec2(hit_width(red_name), theme::KEY),
    );
    let red_now = input::button(
        ui,
        red_now_rect,
        Id::new("red-now"),
        "Red Now",
        Some(app.red_now),
    );
    // The pointer anywhere in the hit area lights the key, and a press keeps
    // the pointer there. Off, the key takes the time fields' edge, as its
    // fill alone barely parts it from the bar through the tint.
    let key = pos2(LEFT + theme::KEY / 2.0, middle);
    let (fill, edge, glyph) = if app.red_now {
        (palette.text, Stroke::NONE, palette.window)
    } else {
        let fill = if red_now.hovered() {
            palette.surface_hover
        } else {
            palette.surface
        };
        (fill, Stroke::new(1.0, palette.field_stroke), palette.text)
    };
    ui.painter().circle(key, theme::KEY / 2.0, fill, edge);
    paint::sundown(ui.painter(), key, glyph);
    paint::text(
        ui.painter(),
        pos2(key.x + theme::KEY / 2.0 + theme::KEY_GAP, middle),
        Align2::LEFT_CENTER,
        "Red Now",
        red_font,
        palette.text,
    );
    if red_now.has_focus() {
        focus_ring(ui, red_now_rect, theme::KEY / 2.0, palette);
    }
    if shared::clicked(app, &red_now) {
        app.actions.push(Action::RedNow(!app.red_now));
    }

    let close_font = theme::semibold(13.0);
    let close_name = paint::measure(ui.painter(), "Close", close_font.clone()).x;
    let close_width = hit_width(close_name);
    let close_rect = Rect::from_min_size(
        pos2(RIGHT - close_width, middle - theme::KEY / 2.0),
        vec2(close_width, theme::KEY),
    );
    let close = input::button(ui, close_rect, Id::new("close"), "Close", None);
    let key = pos2(RIGHT - theme::KEY / 2.0, middle);
    ui.painter().circle_filled(
        key,
        theme::KEY / 2.0,
        if close.hovered() {
            palette.accent_hover
        } else {
            palette.accent
        },
    );
    let cross = Stroke::new(theme::CROSS_STROKE, palette.on_accent);
    let reach = theme::CROSS / 2.0;
    for (from, to) in [
        (vec2(-reach, -reach), vec2(reach, reach)),
        (vec2(-reach, reach), vec2(reach, -reach)),
    ] {
        ui.painter().line_segment([key + from, key + to], cross);
    }
    paint::text(
        ui.painter(),
        pos2(key.x - theme::KEY / 2.0 - theme::KEY_GAP, middle),
        Align2::RIGHT_CENTER,
        "Close",
        close_font,
        palette.text,
    );
    if close.has_focus() {
        focus_ring(ui, close_rect, theme::KEY / 2.0, palette);
    }
    // Close plays no press: the owner dropped it (2026-10-23).
    if close.clicked() {
        app.actions.push(Action::HideWindow);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::test_app;
    use crate::color;
    use crate::schedule::ClockTime;
    use crate::theme::in_a_frame;
    use egui::{FontId, Painter};

    fn noon() -> f64 {
        12.0 * 60.0
    }

    /// Runs `lay_out` in a frame with the window's own fonts.
    fn with_fonts<T>(lay_out: impl FnOnce(&Painter) -> T) -> T {
        in_a_frame(|ui| lay_out(ui.painter()))
    }

    /// Every line of the card that has fixed copy, with its font. The font
    /// test in `theme` reads it too.
    pub(crate) fn fixed_lines() -> Vec<(String, FontId)> {
        let mut app = test_app(noon);
        let mut lines = vec![];
        let mut states = |app: &mut crate::app::App, line: &str| {
            assert_eq!(app.status_line(), line);
            lines.push((line.to_owned(), theme::medium(14.0)));
        };
        // At noon: the day, inside the night, just out of it, and never.
        for (start, end, line) in [
            ((23, 30), (7, 0), "Turns red at 23:30"),
            ((11, 0), (23, 30), "Red until 23:30"),
            ((7, 0), (11, 50), "Fading back to normal"),
            ((9, 0), (9, 0), "From and To match"),
        ] {
            app.settings.start = ClockTime::new(start.0, start.1);
            app.settings.end = ClockTime::new(end.0, end.1);
            states(&mut app, line);
        }
        app.settings.schedule_on = false;
        states(&mut app, "Schedule is off");
        app.red_now = true;
        states(&mut app, "Red Now is on");

        app.status.displays = 3;
        let displays = app.displays_line();
        let shown = [
            "Now: 3400 K, 60%",
            "Now: pure red, 60%",
            "Now: normal colors",
            "Now: as set",
        ];
        for shown in shown {
            lines.push((format!("{shown} · {displays}"), theme::regular(12.0)));
        }
        app.status.failed = true;
        // The failed state's fallback, which carries no count.
        let failed = app.detail_line();
        assert_eq!(failed, "Could not change the colors");
        lines.push((failed, theme::regular(12.0)));

        lines.push(("Pure red".to_owned(), theme::bold(28.0)));
        let wide = format!("{} K", color::DAY_KELVIN);
        lines.push((wide, theme::bold(28.0)));
        lines
    }

    /// Every text that the window draws, with the font it is laid out in, at
    /// looks that put the Temperature on each of its stops and the
    /// Brightness at each end of its scale: the sliders' labels, the stops'
    /// names under the Temperature, the ends' names under the Brightness and
    /// both readouts. The strings are the ones the view draws, not copies of
    /// them, so a renamed stop reaches the font test in `theme`. (There are
    /// no section headers to draw: `no_header_text_is_drawn_at_any_look`.)
    pub(crate) fn drawn_lines() -> Vec<(String, FontId)> {
        use crate::ui::testing::{Screen, texts};

        let mut lines: Vec<(String, FontId)> = Vec::new();
        for stop in shared::STOPS {
            for brightness in [color::MIN_BRIGHTNESS, color::MAX_BRIGHTNESS] {
                let look = Look {
                    kelvin: stop.kelvin,
                    brightness,
                };
                let mut screen = Screen::new(look);
                let shapes = screen.at_rest();
                let mut here: Vec<(String, FontId)> = Vec::new();
                for text in texts(&shapes) {
                    let job = &text.galley.job;
                    for section in &job.sections {
                        let range = section.byte_range.start.0..section.byte_range.end.0;
                        let line = (job.text[range].to_owned(), section.format.font_id.clone());
                        if !here.contains(&line) {
                            here.push(line);
                        }
                    }
                }
                // The frame draws what this look says, in the fonts the rows
                // use: the names of the stops under the Temperature, the ends
                // under the Brightness, the readouts and the labels.
                let drawn = |line: &str, font: FontId| here.contains(&(line.to_owned(), font));
                let at = format!("{} K, {brightness}%", stop.kelvin);
                let mut wanted: Vec<(String, FontId)> = shared::STOPS
                    .iter()
                    .map(|named| (named.name.to_owned(), theme::regular(11.5)))
                    .collect();
                wanted.extend(["Dim", "Full"].map(|end| (end.to_owned(), theme::regular(11.5))));
                wanted.extend([
                    (shared::temperature(look).1, theme::semibold(14.0)),
                    (shared::brightness(look).1, theme::semibold(14.0)),
                    (TEMPERATURE.to_owned(), theme::medium(14.0)),
                    ("Brightness".to_owned(), theme::medium(14.0)),
                ]);
                for (line, font) in wanted {
                    assert!(drawn(&line, font), "{at}: {line:?} is not drawn");
                }
                for line in here {
                    if !lines.contains(&line) {
                        lines.push(line);
                    }
                }
            }
        }
        lines
    }

    #[test]
    fn the_cards_fixed_lines_fit_its_column() {
        let widths = with_fonts(|painter| {
            fixed_lines()
                .into_iter()
                .map(|(line, font)| {
                    let width = paint::measure(painter, &line, font).x;
                    (line, width)
                })
                .collect::<Vec<_>>()
        });
        let report: Vec<_> = widths
            .iter()
            .map(|(line, width)| {
                let verdict = if *width > COLUMN { "TOO WIDE" } else { "fits" };
                format!("{width:6.1} {verdict:8} {line}")
            })
            .collect();
        assert!(
            widths.iter().all(|(_, width)| *width <= COLUMN),
            "the column is {COLUMN} wide:\n{}",
            report.join("\n")
        );
    }

    #[test]
    fn a_long_line_ends_in_an_ellipsis_on_one_row() {
        let long =
            "Could not change the colors of the display on the left, which refused the table";
        let font = theme::regular(12.0);
        let (cut, whole) = with_fonts(|painter| {
            (
                paint::fit(painter, long, font.clone(), Color32::WHITE, COLUMN),
                paint::fit(painter, "Now: normal colors", font, Color32::WHITE, COLUMN),
            )
        });
        assert!(cut.elided);
        assert_eq!(cut.rows.len(), 1);
        assert!(cut.size().x <= COLUMN, "{} of {COLUMN}", cut.size().x);
        assert!(
            cut.rows[0].text().ends_with('…'),
            "{:?}",
            cut.rows[0].text()
        );
        assert!(!whole.elided);
        assert_eq!(whole.rows[0].text(), "Now: normal colors");
    }

    /// The window's rows through whole frames of `ui::draw` on a `test_app`:
    /// where they sit now that the "Color" and "Schedule" headers are gone,
    /// and that nothing around them moved. Positions are read off what is
    /// painted.
    mod layout {
        use super::*;
        use crate::ui::testing::{Screen, circles, middle, rects, texts};
        use egui::Shape;
        use egui::epaint::{CircleShape, TextShape};

        /// The window at the night look `night`, with the screen reader on.
        pub(super) fn screen_at(night: Look) -> Screen {
            Screen::new(night).with_screen_reader()
        }

        /// A node of the screen reader's tree that has bounds: a control, or
        /// the drag strip, which has no label.
        struct Node {
            /// The id of the widget, as egui names its node.
            id: u64,
            label: Option<String>,
            bounds: Rect,
        }

        impl Screen {
            /// Every node of the screen reader's tree that has bounds.
            fn nodes(&mut self) -> Vec<Node> {
                let update = self
                    .output(vec![])
                    .platform_output
                    .accesskit_update
                    .expect("accesskit tree");
                update
                    .nodes
                    .iter()
                    .filter_map(|(id, node)| {
                        let bounds = node.bounds()?;
                        Some(Node {
                            id: id.0,
                            label: node.label().map(str::to_owned),
                            bounds: Rect::from_min_max(
                                pos2(bounds.x0 as f32, bounds.y0 as f32),
                                pos2(bounds.x1 as f32, bounds.y1 as f32),
                            ),
                        })
                    })
                    .collect()
            }

            /// The bounds of the control whose label starts with `label`.
            pub(super) fn control(&mut self, label: &str) -> Rect {
                let found: Vec<Rect> = self
                    .nodes()
                    .into_iter()
                    .filter(|node| node.label.as_deref().is_some_and(|n| n.starts_with(label)))
                    .map(|node| node.bounds)
                    .collect();
                assert_eq!(found.len(), 1, "controls labeled {label:?}");
                found[0]
            }
        }

        /// The one text shape that reads `text`.
        pub(super) fn text_of<'a>(shapes: &'a [Shape], text: &str) -> &'a TextShape {
            let found: Vec<&TextShape> = texts(shapes)
                .into_iter()
                .filter(|shape| shape.galley.text() == text)
                .collect();
            assert_eq!(found.len(), 1, "{text:?} painted {} times", found.len());
            found[0]
        }

        /// Where the one text that reads `text` is centered.
        fn center_of(shapes: &[Shape], text: &str) -> Pos2 {
            middle(text_of(shapes, text))
        }

        /// The window as `view::draw` grounds it.
        pub(super) fn window() -> Rect {
            Rect::from_min_size(Pos2::ZERO, theme::WINDOW)
        }

        /// The player bar's ground.
        pub(super) fn bar() -> Rect {
            Rect::from_min_max(
                pos2(0.0, theme::WINDOW.y - theme::BAR_HEIGHT),
                pos2(theme::WINDOW.x, theme::WINDOW.y),
            )
        }

        /// The three time fields and their steppers, which are as tall as the
        /// fields, from the screen reader's bounds: a field draws no box of
        /// its own, only a cell for each part of its time, so its rect is not
        /// painted. A field's node is named for its value, "From: 21:00", and
        /// its stepper is the union of the "up" and "down" halves.
        pub(super) fn fields(screen: &mut Screen) -> Vec<Rect> {
            let mut found = Vec::new();
            for name in ["From", "To", "Fade minutes"] {
                found.push(screen.control(&format!("{name}: ")));
                found.push(
                    screen
                        .control(&format!("{name} up"))
                        .union(screen.control(&format!("{name} down"))),
                );
            }
            found
        }

        /// The two keys of the bar, Red Now's and then Close's: the two discs
        /// whose centers lie in it.
        pub(super) fn keys(shapes: &[Shape]) -> [CircleShape; 2] {
            let mut found: Vec<CircleShape> = circles(shapes)
                .into_iter()
                .filter(|circle| bar().contains(circle.center))
                .cloned()
                .collect();
            found.sort_by(|a, b| a.center.x.total_cmp(&b.center.x));
            assert_eq!(found.len(), 2, "keys: {found:?}");
            [found[0], found[1]]
        }

        /// The switch of "Turn red automatically".
        fn switch_of(shapes: &[Shape]) -> Rect {
            let found: Vec<Rect> = rects(shapes)
                .into_iter()
                .filter(|shape| shape.rect.size() == theme::SWITCH)
                .map(|shape| shape.rect)
                .collect();
            assert_eq!(found.len(), 1, "switches: {found:?}");
            found[0]
        }

        /// What is painted in the open band of y between `from` and `to`,
        /// apart from the window's own ground.
        fn painted_between(shapes: &[Shape], from: f32, to: f32) -> Vec<Rect> {
            shapes
                .iter()
                .map(Shape::visual_bounding_rect)
                .filter(|bounds| {
                    bounds.is_finite()
                        && *bounds != window()
                        && bounds.max.y > from
                        && bounds.min.y < to
                })
                .collect()
        }

        // The window's height.

        #[test]
        fn everything_fits_in_the_window_at_every_look() {
            // 850 K is pure red, 6500 K daylight, and the default 1200 K the
            // Ember stop: the card's text and the maw differ at each.
            for kelvin in [Look::default().kelvin, 850, 6500] {
                let night = Look {
                    kelvin,
                    ..Look::default()
                };
                let shapes = screen_at(night).at_rest();
                // The window's ground is the first thing painted, and it is
                // exactly the window.
                assert_eq!(rects(&shapes)[0].rect, window(), "at {kelvin} K");
                for shape in &shapes {
                    let bounds = shape.visual_bounding_rect();
                    if !bounds.is_finite() {
                        continue;
                    }
                    assert!(
                        window().contains_rect(bounds),
                        "at {kelvin} K a shape reaches {bounds:?}, outside {:?}: {shape:?}",
                        window()
                    );
                }
            }
        }

        #[test]
        fn the_player_bar_still_ends_the_window_and_centers_its_buttons() {
            let shapes = screen_at(Look::default()).at_rest();
            let grounds: Vec<Rect> = rects(&shapes)
                .into_iter()
                .filter(|shape| shape.fill == Palette::DARK.panel)
                .map(|shape| shape.rect)
                .collect();
            assert_eq!(grounds, [bar()]);
            assert_eq!(bar().max.y, theme::WINDOW.y);
            assert_eq!(bar().height(), theme::BAR_HEIGHT);
            // Each button is a round key with its name beside it: the two
            // circles in the bar, Red Now's at the left, and the names (the
            // bar's own ground would be the smallest rectangle round a name).
            let keys = keys(&shapes);
            for (label, key) in [("Red Now", &keys[0]), ("Close", &keys[1])] {
                let center = center_of(&shapes, label);
                // The key's edge, if it has one, is stroked centered on it.
                let reach = key.radius + key.stroke.width / 2.0;
                let rim = Rect::from_center_size(key.center, vec2(2.0 * reach, 2.0 * reach));
                assert!(bar().contains_rect(rim), "{label}: {rim:?}");
                for middle_y in [center.y, key.center.y] {
                    assert!(
                        (middle_y - bar().center().y).abs() < 0.5,
                        "{label} at {middle_y}, the bar's middle is {}",
                        bar().center().y
                    );
                }
            }
            // The names stand on the keys' inner sides: Red Now's name to
            // the right of its key, Close's to the left of its.
            assert!(center_of(&shapes, "Red Now").x > keys[0].center.x);
            assert!(center_of(&shapes, "Close").x < keys[1].center.x);
        }

        #[test]
        fn the_fields_keep_their_air_above_the_bar() {
            let fields = fields(&mut screen_at(Look::default()));
            // Three fields, and a stepper at the right of each.
            assert_eq!(fields.len(), 6, "{fields:?}");
            for field in fields {
                // 31 points before the headers went, and 31 after: the 60
                // points cut came off the rows above, not off this air. The
                // numbers: fields centered at 456, so down to 471, against a
                // bar from 502.
                assert_eq!(bar().min.y - field.max.y, 31.0, "{field:?}");
                assert_eq!(field.max.y, 456.0 + 15.0);
            }
        }

        // No header text is drawn.

        #[test]
        fn no_header_text_is_drawn_at_any_look() {
            let desert = Look {
                kelvin: shared::DESERT_KELVIN,
                brightness: 100,
            };
            let mut schedule_off = test_app(noon);
            schedule_off.settings.schedule_on = false;
            let screens = [
                ("the default look", Screen::new(Look::default())),
                ("the Desert look", Screen::new(desert)),
                ("the schedule off", Screen::of(schedule_off)),
            ];
            for (name, mut screen) in screens {
                let shapes = screen.at_rest();
                for text in texts(&shapes) {
                    let line = text.galley.text();
                    assert!(
                        line != "Color" && line != "Schedule",
                        "{line:?} painted with {name}"
                    );
                    // Bold 16 was the headers' font, and the demo's scale
                    // plate title's, which is not in the window.
                    assert!(
                        text.galley
                            .job
                            .sections
                            .iter()
                            .all(|section| section.format.font_id != theme::bold(16.0)),
                        "{line:?} in the headers' font with {name}"
                    );
                }
            }
        }

        #[test]
        fn nothing_is_painted_where_the_headers_were() {
            let shapes = screen_at(Look::default()).at_rest();
            // Where "Color" was: between the card and the Temperature label.
            let label = text_of(&shapes, "Temperature").visual_bounding_rect();
            let gap = painted_between(&shapes, CARD.max.y, label.min.y);
            assert!(
                gap.is_empty(),
                "painted between the card and Temperature: {gap:?}"
            );
            // Where "Schedule" was: between the Brightness names and the
            // switch row. Only the row's own text and switch are painted
            // below the names.
            let names = ["Dim", "Full"].map(|name| text_of(&shapes, name).visual_bounding_rect());
            let names_bottom = names[0].max.y.max(names[1].max.y);
            let switch = switch_of(&shapes);
            let gap = painted_between(&shapes, names_bottom, switch.min.y);
            assert!(
                gap.is_empty(),
                "painted between Brightness and the switch: {gap:?}"
            );
            // The same bands hold no text by its center, which is how a
            // header would sit in them.
            let centers: Vec<f32> = texts(&shapes).into_iter().map(|t| middle(t).y).collect();
            let brightness_names = center_of(&shapes, "Dim").y;
            for y in centers {
                assert!(
                    !(CARD.max.y < y && y < TEMPERATURE_TOP)
                        && !(brightness_names < y && y < AUTO_Y),
                    "a text centered at {y}"
                );
            }
        }

        // The controls keep their order and labels.

        #[test]
        fn the_controls_keep_their_order_down_the_window() {
            let mut screen = screen_at(Look::default());
            let shapes = screen.at_rest();
            let at = |text: &str| center_of(&shapes, text);
            let (temperature, brightness) = (at("Temperature"), at("Brightness"));
            let auto = at("Turn red automatically");
            let (from, to, fade) = (at("From"), at("To"), at("Fade"));
            let (red_now, close) = (at("Red Now"), at("Close"));
            assert!(
                temperature.y < brightness.y,
                "{temperature:?} {brightness:?}"
            );
            assert!(brightness.y < auto.y, "{brightness:?} {auto:?}");
            // The three columns' names share a row, below the switch's.
            assert!(auto.y < from.y, "{auto:?} {from:?}");
            assert_eq!((from.y, to.y), (fade.y, fade.y));
            assert!(from.x < to.x && to.x < fade.x, "{from:?} {to:?} {fade:?}");
            // The buttons share a row, below the fields.
            let fields_bottom = fields(&mut screen)
                .iter()
                .map(|rect| rect.max.y)
                .fold(0.0, f32::max);
            assert!(fields_bottom < red_now.y, "{fields_bottom} {red_now:?}");
            assert_eq!(red_now.y, close.y);
            assert!(red_now.x < close.x, "{red_now:?} {close:?}");
        }

        /// The rows' positions: the numbers are written out here, so that a
        /// change of the layout fails here.
        #[test]
        fn the_rows_sit_where_the_spec_puts_them() {
            let mut screen = screen_at(Look::default());
            let shapes = screen.at_rest();
            // The labels are 6 below their rows' tops, 238 and 310.
            assert_eq!(center_of(&shapes, "Temperature").y, 244.0);
            assert_eq!(center_of(&shapes, "Brightness").y, 316.0);
            // The strips' center lines are 266 and 338, 28 below the tops.
            assert_eq!(screen.control("Temperature").center().y, 266.0);
            assert_eq!(screen.control("Brightness").center().y, 338.0);
            // The switch row.
            assert_eq!(center_of(&shapes, "Turn red automatically").y, 392.0);
            assert_eq!(switch_of(&shapes).center().y, 392.0);
            // The fields' names and the fields, 30 tall.
            for name in ["From", "To", "Fade"] {
                assert_eq!(center_of(&shapes, name).y, 426.0, "{name}");
            }
            for field in fields(&mut screen) {
                assert_eq!(
                    (field.min.y, field.center().y, field.max.y),
                    (441.0, 456.0, 471.0)
                );
            }
        }

        /// The groups have no header, so the room between them parts them: a
        /// gap only a little wider than the gaps inside a group, as the owner
        /// chose from the renders. A layout that parts the groups more, or
        /// less, fails here.
        #[test]
        fn the_groups_are_parted_by_a_gap_a_little_wider_than_the_gaps_within_one() {
            /// The least a gap between groups is wider than the gap within
            /// the slider group, in points.
            const LEAST: f32 = 2.0;
            let shapes = screen_at(Look::default()).at_rest();
            let y = |text: &str| center_of(&shapes, text).y;
            // Each gap runs from one line of text to the next. Within the
            // slider group: from the Temperature names to the Brightness
            // label.
            let within = y("Brightness") - y(shared::STOPS[0].name);
            assert!(within > 0.0, "{within}");
            for (gap, what) in [
                (
                    y("Turn red automatically") - y("Dim"),
                    "from the Brightness names to the switch row",
                ),
                (
                    y("From") - y("Turn red automatically"),
                    "from the switch row to the field names",
                ),
            ] {
                let wider = gap - within;
                assert!(
                    wider >= LEAST,
                    "the gap {what} is {gap}, only {wider} wider than the {within} within the slider group"
                );
                // "A little": by no more than a quarter of that gap.
                assert!(
                    wider <= within / 4.0,
                    "the gap {what} is {gap}, {wider} wider than the {within} within the slider group"
                );
            }
        }

        // The drag strip and the traffic lights are unaffected.

        #[test]
        fn no_control_reaches_into_the_drag_strip() {
            let mut screen = screen_at(Look::default());
            let strip = Rect::from_min_size(Pos2::ZERO, vec2(theme::WINDOW.x, theme::TITLE_BAR));
            let nodes = screen.nodes();
            // The drag strip is the one node with bounds and no label, and
            // it is the title bar; the 14 controls all start below it.
            let (strips, controls): (Vec<_>, Vec<_>) =
                nodes.into_iter().partition(|node| node.label.is_none());
            assert_eq!(
                strips
                    .into_iter()
                    .map(|node| node.bounds)
                    .collect::<Vec<_>>(),
                [strip]
            );
            assert_eq!(controls.len(), 14);
            for Node { label, bounds, .. } in controls {
                assert!(
                    bounds.min.y >= theme::TITLE_BAR,
                    "{label:?} starts at {}, inside the strip",
                    bounds.min.y
                );
            }
            // The Temperature slider's hit area, 8 above its center line,
            // stays below the card.
            let temperature = screen.control("Temperature");
            assert!(
                temperature.min.y >= CARD.max.y,
                "{temperature:?} and {CARD:?}"
            );
            assert!(CARD.max.y <= temperature.center().y - 8.0);
        }

        #[test]
        fn a_press_in_the_strip_changes_nothing() {
            let mut screen = screen_at(Look::default());
            let before = screen.app.settings.clone();
            assert!(screen.app.actions.is_empty());
            for at in [pos2(210.0, 22.0), pos2(210.0, theme::TITLE_BAR - 1.0)] {
                screen.press(at);
                screen.release(at);
                assert_eq!(screen.app.settings, before, "after a press at {at:?}");
                assert_eq!(screen.app.actions, [], "after a press at {at:?}");
            }
            // The same presses do reach a control: Close hides the window.
            let close = screen.control("Close").center();
            screen.press(close);
            screen.release(close);
            assert_eq!(screen.app.actions, [Action::HideWindow]);
        }

        #[test]
        fn a_press_inside_the_strip_is_taken_by_the_drag_strip() {
            // `fastframe_titlebar::drag` is the first interaction of
            // `view::draw`. Its `StartDrag` needs AppKit, so what shows here
            // is that the strip's widget has the press in the strip, and no
            // control has it; `no_control_reaches_into_the_drag_strip` keeps
            // any control from overlapping the strip.
            let mut screen = screen_at(Look::default());
            // The window's 14 controls by the ids of their widgets: two
            // sliders and the switch, three fields with an up and a down
            // arrow each, and two buttons. The screen reader has a node for
            // each, named by the widget's id, so that an id renamed in the
            // view, or a control left off this list, fails here instead of
            // passing the check below because no widget has the id.
            let fields = ["from", "to", "fade"].map(Id::new);
            let arrows = fields.map(|field| [field.with("up"), field.with("down")]);
            let close = Id::new("close");
            let controls: Vec<Id> = [TEMPERATURE, "Brightness", "schedule-on", "red-now"]
                .map(Id::new)
                .into_iter()
                .chain([close])
                .chain(fields)
                .chain(arrows.into_iter().flatten())
                .collect();
            let mut named: Vec<u64> = controls.iter().map(Id::value).collect();
            let mut told: Vec<u64> = screen
                .nodes()
                .into_iter()
                .filter(|node| node.label.is_some())
                .map(|node| node.id)
                .collect();
            named.sort();
            told.sort();
            assert_eq!(named, told, "the controls' ids and the screen reader's");
            let strip = screen.strip();
            let bar = Rect::from_min_size(Pos2::ZERO, vec2(theme::WINDOW.x, theme::TITLE_BAR));
            assert_eq!(screen.ctx.read_response(strip).map(|r| r.rect), Some(bar));
            let inside = pos2(60.0, 40.0);
            assert!(bar.contains(inside));
            screen.press(inside);
            assert!(screen.pressed(strip));
            for id in &controls {
                assert!(
                    screen.ctx.read_response(*id).is_some(),
                    "{id:?} is no widget of the window"
                );
                assert!(!screen.pressed(*id), "{id:?} has the press in the strip");
            }
            screen.release(inside);
            // And a press on a control is that control's, not the strip's.
            let on_close = screen.control("Close").center();
            screen.press(on_close);
            assert!(screen.pressed(close));
            assert!(!screen.pressed(strip));
        }
    }

    /// The time fields, their steppers and the player bar's two keys, through
    /// whole frames of the window with the screen reader on. Rects come from
    /// the screen reader's bounds and from the frame's shapes, sizes from
    /// `theme`'s constants and colors from `Palette`; the owner's three picks
    /// stand as numbers once, in
    /// `the_owners_picks_are_a_24_point_cell_a_32_point_key_and_semibold_15`.
    mod controls {
        use super::layout::{bar, fields, keys, screen_at, text_of, window};
        use super::*;
        use crate::ui::testing::{
            Screen, drawn, ink, key_down, middle, paths, rects, segments, texts,
        };
        use egui::epaint::{CircleShape, ColorMode, PathShape, RectShape, TextShape};
        use egui::{CornerRadius, Event, Key, Modifiers, Shape};

        /// A time field of the schedule at the default settings.
        struct Field {
            /// The name over it.
            name: &'static str,
            /// What its parts show: From 21:00, To 07:00, Fade 20 minutes.
            parts: &'static [&'static str],
            unit: Option<&'static str>,
        }

        impl Field {
            /// The name the screen reader gives the field's steppers.
            fn name_label(&self) -> &'static str {
                if self.name == "Fade" {
                    "Fade minutes"
                } else {
                    self.name
                }
            }
        }

        const FIELDS: [Field; 3] = [
            Field {
                name: "From",
                parts: &["21", "00"],
                unit: None,
            },
            Field {
                name: "To",
                parts: &["07", "00"],
                unit: None,
            },
            Field {
                name: "Fade",
                parts: &["20"],
                unit: Some("min"),
            },
        ];

        // The scenes.

        fn at_night() -> Screen {
            screen_at(Look::default())
        }

        fn by_day() -> Screen {
            screen_at(Look {
                kelvin: color::DAY_KELVIN,
                ..Look::default()
            })
        }

        /// The schedule is off: its fields are disabled.
        fn schedule_off() -> Screen {
            let mut app = test_app(noon);
            app.settings.schedule_on = false;
            Screen::of(app).with_screen_reader()
        }

        /// Red Now is on.
        fn red_now_on() -> Screen {
            let mut app = test_app(noon);
            app.red_now = true;
            Screen::of(app).with_screen_reader()
        }

        // Small helpers.

        fn close(a: f32, b: f32) -> bool {
            (a - b).abs() < 0.01
        }

        fn at(a: Pos2, b: Pos2) -> bool {
            close(a.x, b.x) && close(a.y, b.y)
        }

        fn near(a: Rect, b: Rect) -> bool {
            at(a.min, b.min) && at(a.max, b.max)
        }

        /// A disabled cell's or stepper's fill: halfway from `surface` to
        /// `window`. The view gives the fields their full opacity back after
        /// `disable`, so no shape of a disabled field is faded on top.
        fn disabled() -> Color32 {
            paint::mix(PALETTE.surface, PALETTE.window, 0.5)
        }

        fn font_of(text: &TextShape) -> egui::FontId {
            text.galley.job.sections[0].format.font_id.clone()
        }

        fn width_of(text: &str, font: egui::FontId) -> f32 {
            with_fonts(|painter| paint::measure(painter, text, font).x)
        }

        fn shapes_of(frame: &[(Rect, Shape)]) -> Vec<Shape> {
            frame.iter().map(|(_, shape)| shape.clone()).collect()
        }

        /// The texts of `shapes` whose middle lies in `rect`.
        fn texts_in(shapes: &[Shape], rect: Rect) -> Vec<&TextShape> {
            texts(shapes)
                .into_iter()
                .filter(|text| rect.contains(middle(text)))
                .collect()
        }

        /// The pointer presses and lets go at `at`, then leaves the window, and
        /// a frame at rest follows: what the window looks like after a click.
        fn click(screen: &mut Screen, at: Pos2) -> Vec<Shape> {
            screen.press(at);
            screen.release(at);
            screen.frame(vec![Event::PointerGone]);
            screen.at_rest()
        }

        /// A frame with the pointer moved to `at` and resting there, with the
        /// clip rectangle of each shape.
        fn hovering(screen: &mut Screen, at: Pos2) -> Vec<(Rect, Shape)> {
            screen.frame(vec![Event::PointerMoved(at)]);
            screen.at_rest_clipped()
        }

        // The cells and the steppers.

        /// The cells painted for the field at `field`, left to right: the rects
        /// on its middle line that are shorter than the field and start in it.
        fn cells_of(shapes: &[Shape], field: Rect) -> Vec<RectShape> {
            let mut found: Vec<RectShape> = rects(shapes)
                .into_iter()
                .filter(|shape| {
                    close(shape.rect.center().y, field.center().y)
                        && shape.rect.height() < field.height()
                        && shape.rect.min.x >= field.min.x - 0.01
                        && shape.rect.min.x < field.max.x
                })
                .cloned()
                .collect();
            found.sort_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x));
            found
        }

        /// A cell's fill, its edge and the color its digits are drawn in.
        type CellLook = (Color32, Stroke, Color32);

        fn cell(fill: Color32, edge: Color32, digits: Color32) -> CellLook {
            (fill, Stroke::new(1.0, edge), digits)
        }

        /// How each cell of the field at `field` looks.
        fn looks_of(shapes: &[Shape], field: Rect) -> Vec<CellLook> {
            cells_of(shapes, field)
                .iter()
                .map(|cell| {
                    let digits = texts_in(shapes, cell.rect);
                    assert_eq!(digits.len(), 1, "texts in the cell at {:?}", cell.rect);
                    (cell.fill, cell.stroke, drawn(digits[0]))
                })
                .collect()
        }

        /// The stepper's pill: the one rect of `stepper`'s size and place.
        fn pill(shapes: &[Shape], stepper: Rect) -> RectShape {
            let found: Vec<&RectShape> = rects(shapes)
                .into_iter()
                .filter(|shape| near(shape.rect, stepper))
                .collect();
            assert_eq!(found.len(), 1, "pills at {stepper:?}: {found:?}");
            found[0].clone()
        }

        /// Whether `path` is a chevron of `stepper`: three points, all inside it.
        fn is_chevron(path: &PathShape, stepper: Rect) -> bool {
            path.points.len() == 3 && path.points.iter().all(|p| stepper.contains(*p))
        }

        /// The chevrons inside `stepper`, top to bottom.
        fn chevrons(shapes: &[Shape], stepper: Rect) -> Vec<&PathShape> {
            let mut found: Vec<&PathShape> = paths(shapes)
                .into_iter()
                .filter(|path| is_chevron(path, stepper))
                .collect();
            found.sort_by(|a, b| a.points[1].y.total_cmp(&b.points[1].y));
            found
        }

        /// The lit halves of a frame: the rect that fills a stepper inset by 1
        /// point, with the clip rectangle it was painted under.
        fn lit(frame: &[(Rect, Shape)]) -> Vec<(RectShape, Rect)> {
            let inset = theme::STEPPER - vec2(2.0, 2.0);
            frame
                .iter()
                .filter_map(|(clip, shape)| match shape {
                    Shape::Rect(lit)
                        if close(lit.rect.width(), inset.x)
                            && close(lit.rect.height(), inset.y) =>
                    {
                        Some((lit.clone(), *clip))
                    }
                    _ => None,
                })
                .collect()
        }

        // The keys.

        /// Whether `point` lies inside the disc of `key`.
        fn in_key(point: &Pos2, key: &CircleShape) -> bool {
            point.distance(key.center) < key.radius
        }

        /// What is drawn over a key: the polygons and the line segments whose
        /// points all lie inside its circle.
        #[allow(clippy::type_complexity)]
        fn over<'a>(
            shapes: &'a [Shape],
            key: &CircleShape,
        ) -> (Vec<&'a PathShape>, Vec<([Pos2; 2], Stroke)>) {
            (
                paths(shapes)
                    .into_iter()
                    .filter(|path| path.points.iter().all(|point| in_key(point, key)))
                    .collect(),
                segments(shapes)
                    .into_iter()
                    .filter(|(points, _)| points.iter().all(|point| in_key(point, key)))
                    .collect(),
            )
        }

        /// Where the pointer is when a key is read.
        #[derive(Clone, Copy, Debug)]
        enum Pointer {
            /// Not in the window.
            Away,
            OverKey,
            /// Over the name alone, which the key's circle does not hold.
            OverName,
            PressedOnKey,
            PressedOnName,
            /// Pressed on the key and dragged off the hit area.
            PressedThenOff,
        }

        impl Pointer {
            const ALL: [Pointer; 6] = [
                Pointer::Away,
                Pointer::OverKey,
                Pointer::OverName,
                Pointer::PressedOnKey,
                Pointer::PressedOnName,
                Pointer::PressedThenOff,
            ];

            /// Whether the key lights: the pointer is in the hit area.
            fn lights(self) -> bool {
                !matches!(self, Pointer::Away | Pointer::PressedThenOff)
            }
        }

        /// A frame of `screen` with the pointer as `pointer` has it, for the
        /// key centered at `key` whose name's middle is at `name`.
        fn with_pointer(
            screen: &mut Screen,
            pointer: Pointer,
            key: Pos2,
            name: Pos2,
        ) -> Vec<Shape> {
            match pointer {
                Pointer::Away => screen.at_rest(),
                Pointer::OverKey => screen.hover(key),
                Pointer::OverName => screen.hover(name),
                Pointer::PressedOnKey => {
                    screen.press(key);
                    screen.at_rest()
                }
                Pointer::PressedOnName => {
                    screen.press(name);
                    screen.at_rest()
                }
                Pointer::PressedThenOff => {
                    screen.press(key);
                    screen.hover(bar().center())
                }
            }
        }

        /// The name `label` of a key, read from `shapes`: its text, in `font`
        /// and `text`, and its middle on the bar's middle line.
        fn name_of<'a>(shapes: &'a [Shape], label: &str, font: egui::FontId) -> &'a TextShape {
            let name = text_of(shapes, label);
            assert_eq!(ink(name), PALETTE.text, "{label}'s ink");
            assert_eq!(font_of(name), font, "{label}'s font");
            assert!(
                close(middle(name).y, bar().center().y),
                "{label} is centered at {}, the bar's middle is {}",
                middle(name).y,
                bar().center().y
            );
            name
        }

        // Each part of a time stands in its cell.

        #[test]
        fn each_part_of_a_time_stands_in_its_cell() {
            let colon = width_of(":", theme::semibold(15.0));
            // The face's digits differ in width, and a cell is as wide as its
            // count of digits times the widest of them.
            let widest = ('0'..='9')
                .map(|digit| width_of(&digit.to_string(), theme::semibold(15.0)))
                .fold(0.0, f32::max);
            for (scene, mut screen) in [
                ("night", at_night()),
                ("day", by_day()),
                ("off", schedule_off()),
            ] {
                let bounds = fields(&mut screen);
                let shapes = screen.at_rest();
                let line = bounds[0].center().y;
                let colons: Vec<&TextShape> = texts(&shapes)
                    .into_iter()
                    .filter(|text| text.galley.text() == ":")
                    .collect();
                assert_eq!(colons.len(), 2, "{scene}: a colon in From and in To");
                let units: Vec<&TextShape> = texts(&shapes)
                    .into_iter()
                    .filter(|text| {
                        text.galley.text() == "min"
                            && close(middle(text).y, line - theme::CELL_LIFT)
                    })
                    .collect();
                assert_eq!(units.len(), 1, "{scene}: Fade's unit");
                // From and To two cells each, Fade one: none other is 24 tall
                // on the middle line.
                let on_the_line = rects(&shapes)
                    .into_iter()
                    .filter(|shape| {
                        close(shape.rect.height(), theme::CELL_HEIGHT)
                            && close(shape.rect.center().y, line)
                    })
                    .count();
                assert_eq!(on_the_line, 5, "{scene}: cells on the middle line");

                for (index, field) in FIELDS.iter().enumerate() {
                    let rect = bounds[2 * index];
                    let who = format!("{scene}, {}", field.name);
                    let cells = cells_of(&shapes, rect);
                    assert_eq!(cells.len(), field.parts.len(), "{who}: {cells:?}");
                    // The first cell starts at the field's left, and each
                    // next one the colon's width plus 4 after the one before.
                    let mut left = rect.min.x;
                    for (part, (digits, cell)) in field.parts.iter().zip(&cells).enumerate() {
                        if part > 0 {
                            left += colon + theme::COLON_GAP;
                        }
                        // The width is the count of digits times the widest,
                        // to a few thousandths: a laid-out digit is rounded
                        // to the layout's grid (10.125 for an 8), where the
                        // cell's width reads the glyph's own advance.
                        let width = digits.chars().count() as f32 * widest + 2.0 * theme::CELL_PAD;
                        assert!(
                            (cell.rect.width() - width).abs() < 0.02,
                            "{who}, part {part}: {} wide, not {width}",
                            cell.rect.width()
                        );
                        let expected = Rect::from_min_size(
                            pos2(left, line - theme::CELL_HEIGHT / 2.0),
                            vec2(cell.rect.width(), theme::CELL_HEIGHT),
                        );
                        left += cell.rect.width();
                        assert!(
                            near(cell.rect, expected),
                            "{who}, part {part}: {:?}, not {expected:?}",
                            cell.rect
                        );
                        assert_eq!(cell.corner_radius, CornerRadius::same(theme::CELL_RADIUS));
                        assert_eq!(cell.stroke.width, 1.0, "{who}: a 1-point edge");
                        assert_eq!(cell.stroke_kind, StrokeKind::Inside, "{who}: inside");
                        // Its digits are semibold 15, centered in it, a
                        // point above the middle line.
                        let inside = texts_in(&shapes, cell.rect);
                        assert_eq!(inside.len(), 1, "{who}, part {part}: {inside:?}");
                        assert_eq!(inside[0].galley.text(), *digits);
                        assert_eq!(font_of(inside[0]), theme::semibold(15.0));
                        let want = pos2(cell.rect.center().x, line - theme::CELL_LIFT);
                        assert!(
                            at(middle(inside[0]), want),
                            "{who}, part {part}: digits at {:?}, not {want:?}",
                            middle(inside[0])
                        );
                    }
                    // A colon between two cells, centered in the gap, in the
                    // digits' font.
                    for pair in cells.windows(2) {
                        let between = Rect::from_min_max(
                            pos2(pair[0].rect.max.x, rect.min.y),
                            pos2(pair[1].rect.min.x, rect.max.y),
                        );
                        let found: Vec<&&TextShape> = colons
                            .iter()
                            .filter(|colon| between.contains(middle(colon)))
                            .collect();
                        assert_eq!(found.len(), 1, "{who}: a colon between the cells");
                        let want = pos2(between.center().x, line - theme::CELL_LIFT);
                        assert!(
                            at(middle(found[0]), want),
                            "{who}: colon at {:?}",
                            middle(found[0])
                        );
                        assert_eq!(font_of(found[0]), theme::semibold(15.0));
                    }
                    // Fade's unit starts 5 points after its cell, in regular
                    // 13, a point above the middle line.
                    if let (Some(unit), Some(last)) = (field.unit, cells.last()) {
                        let found = units[0];
                        assert_eq!(found.galley.text(), unit);
                        assert_eq!(font_of(found), theme::regular(13.0));
                        assert!(
                            close(found.pos.x, last.rect.max.x + theme::UNIT_GAP),
                            "{who}: the unit starts at {}",
                            found.pos.x
                        );
                        assert!(close(middle(found).y, line - theme::CELL_LIFT));
                    }
                }
                // The colons and the unit stand on the window: no cell holds
                // them, and the window's ground is the only rect that does.
                for stray in colons.iter().chain(units.iter()) {
                    let holding: Vec<&RectShape> = rects(&shapes)
                        .into_iter()
                        .filter(|shape| shape.rect.contains(middle(stray)))
                        .collect();
                    assert_eq!(
                        holding.len(),
                        1,
                        "{scene}: {:?} stands in {holding:?}",
                        stray.galley.text()
                    );
                    assert_eq!(holding[0].rect, window(), "{scene}: the window's ground");
                    assert_eq!(holding[0].fill, PALETTE.window);
                }
            }
        }

        #[test]
        fn a_field_draws_no_box_and_its_stepper_is_a_pill() {
            for (scene, mut screen) in [("night", at_night()), ("off", schedule_off())] {
                let bounds = fields(&mut screen);
                let shapes = screen.at_rest();
                let painted = rects(&shapes);
                for pair in bounds.chunks(2) {
                    let (field, stepper) = (pair[0], pair[1]);
                    // Nothing round the field but the window's ground.
                    for shape in &painted {
                        assert!(!near(shape.rect, field), "{scene}: a box {:?}", shape.rect);
                        assert!(
                            shape.rect == window() || !shape.rect.contains_rect(field),
                            "{scene}: {:?} is painted round {field:?}",
                            shape.rect
                        );
                    }
                    // Today's place, a pill of radius 9.
                    let pill = pill(&shapes, stepper);
                    assert_eq!(
                        pill.corner_radius,
                        CornerRadius::same(theme::STEPPER_RADIUS)
                    );
                    assert_eq!(pill.stroke.width, 1.0);
                    assert_eq!(pill.stroke_kind, StrokeKind::Inside);
                    assert_eq!(pill.rect.size(), theme::STEPPER);
                }
                // The rects as tall as a field, in the fields' row, are the
                // three steppers.
                let row = bounds[0].center().y;
                let tall: Vec<Rect> = painted
                    .iter()
                    .filter(|shape| {
                        close(shape.rect.height(), theme::FIELD_HEIGHT)
                            && close(shape.rect.center().y, row)
                    })
                    .map(|shape| shape.rect)
                    .collect();
                let steppers = [bounds[1], bounds[3], bounds[5]];
                assert_eq!(tall.len(), 3, "{scene}: {tall:?}");
                for stepper in steppers {
                    assert!(
                        tall.iter().any(|rect| near(*rect, stepper)),
                        "{scene}: {stepper:?}"
                    );
                }
            }
        }

        #[test]
        fn the_cells_and_the_stepper_take_the_inks_of_the_states_at_rest() {
            let rest = cell(PALETTE.surface, PALETTE.field_stroke, PALETTE.text);
            for (scene, mut screen) in [("night", at_night()), ("day", by_day())] {
                let bounds = fields(&mut screen);
                let frame = screen.at_rest_clipped();
                let shapes = shapes_of(&frame);
                for (index, field) in FIELDS.iter().enumerate() {
                    let (rect, stepper) = (bounds[2 * index], bounds[2 * index + 1]);
                    let who = format!("{scene}, {}", field.name);
                    assert_eq!(
                        looks_of(&shapes, rect),
                        vec![rest; field.parts.len()],
                        "{who}"
                    );
                    let pill = pill(&shapes, stepper);
                    assert_eq!(pill.fill, PALETTE.surface, "{who}: the pill");
                    assert_eq!(pill.stroke, Stroke::new(1.0, PALETTE.field_stroke), "{who}");
                    let marks = chevrons(&shapes, stepper);
                    assert_eq!(marks.len(), 2, "{who}: two chevrons");
                    for mark in marks {
                        assert_eq!(mark.stroke.width, 1.4);
                        assert_eq!(mark.stroke.color, ColorMode::Solid(PALETTE.text), "{who}");
                    }
                }
                assert!(lit(&frame).is_empty(), "{scene}: no half is lit at rest");
                // The colons and the unit are quiet.
                let line = bounds[0].center().y - theme::CELL_LIFT;
                let quiet: Vec<&TextShape> = texts(&shapes)
                    .into_iter()
                    .filter(|text| {
                        matches!(text.galley.text(), ":" | "min") && close(middle(text).y, line)
                    })
                    .collect();
                assert_eq!(quiet.len(), 3, "{scene}: two colons and a unit");
                for text in quiet {
                    assert_eq!(
                        ink(text),
                        PALETTE.secondary,
                        "{scene}: {:?}",
                        text.galley.text()
                    );
                }
            }
        }

        #[test]
        fn the_cells_and_the_stepper_take_the_inks_of_the_states_focused_and_hovered() {
            let rest = cell(PALETTE.surface, PALETTE.field_stroke, PALETTE.text);
            let hovered = cell(PALETTE.surface_hover, PALETTE.field_stroke, PALETTE.text);
            let selected = cell(PALETTE.selection, PALETTE.field_stroke, PALETTE.text);
            // A click in a cell focuses its field and selects its part: the
            // cell is `selection` whole, the field's other cell `surface`,
            // the pointer being off the window.
            for (index, field) in FIELDS.iter().enumerate() {
                for part in 0..field.parts.len() {
                    let mut screen = at_night();
                    let bounds = fields(&mut screen);
                    let cells = cells_of(&screen.at_rest(), bounds[2 * index]);
                    let shapes = click(&mut screen, cells[part].rect.center());
                    for (other, other_field) in FIELDS.iter().enumerate() {
                        let mut want = vec![rest; other_field.parts.len()];
                        if other == index {
                            want[part] = selected;
                        }
                        assert_eq!(
                            looks_of(&shapes, bounds[2 * other]),
                            want,
                            "{} part {part} clicked, {}",
                            field.name,
                            other_field.name
                        );
                    }
                }
            }
            // The pointer over From, in the gap past the last cell too: the
            // cells that are not selected are `surface_hover`.
            let mut screen = at_night();
            let from = fields(&mut screen)[0];
            let cells = cells_of(&screen.at_rest(), from);
            let gap = pos2(from.max.x - 2.0, from.center().y);
            assert!(
                cells.iter().all(|cell| !cell.rect.contains(gap)),
                "{gap:?} is in a cell"
            );
            let shapes = screen.hover(gap);
            assert_eq!(
                looks_of(&shapes, from),
                [hovered, hovered],
                "hovered, not focused"
            );
            let over_a_cell = cells[0].rect.center();
            let shapes = screen.hover(over_a_cell);
            assert_eq!(looks_of(&shapes, from), [hovered, hovered], "over a cell");
            let shapes = click(&mut screen, over_a_cell);
            assert_eq!(looks_of(&shapes, from), [selected, rest]);
            let shapes = screen.hover(gap);
            assert_eq!(
                looks_of(&shapes, from),
                [selected, hovered],
                "the selected cell stays `selection` under the pointer"
            );
            let bounds = fields(&mut screen);
            for other in [bounds[2], bounds[4]] {
                assert!(
                    looks_of(&shapes, other).iter().all(|look| *look == rest),
                    "the other fields stay at rest"
                );
            }
        }

        #[test]
        fn the_cells_and_the_stepper_take_the_inks_of_the_states_under_a_stepper_half() {
            for (index, field) in FIELDS.iter().enumerate() {
                for half in ["up", "down"] {
                    for pressed in [false, true] {
                        let mut screen = at_night();
                        let bounds = fields(&mut screen);
                        let stepper = bounds[2 * index + 1];
                        let which = screen.control(&format!("{} {half}", field.name_label()));
                        let who = format!("{} {half}, pressed {pressed}", field.name);
                        let frame = if pressed {
                            screen.press(which.center());
                            screen.at_rest_clipped()
                        } else {
                            hovering(&mut screen, which.center())
                        };
                        let shapes = shapes_of(&frame);
                        // One lit rect: the pill inset by 1 point at radius
                        // 8, clipped to the half under the pointer.
                        let lit = lit(&frame);
                        assert_eq!(lit.len(), 1, "{who}: lit halves {lit:?}");
                        let (lit, clip) = &lit[0];
                        assert!(near(lit.rect, stepper.shrink(1.0)), "{who}: {:?}", lit.rect);
                        assert_eq!(
                            lit.corner_radius,
                            CornerRadius::same(theme::STEPPER_RADIUS - 1),
                            "{who}"
                        );
                        assert_eq!(lit.stroke, Stroke::NONE, "{who}");
                        assert_eq!(
                            lit.fill,
                            if pressed {
                                PALETTE.surface_active
                            } else {
                                PALETTE.surface_hover
                            },
                            "{who}"
                        );
                        assert!(
                            near(*clip, which),
                            "{who}: clipped to {clip:?}, not {which:?}"
                        );
                        // The pill under it, the chevrons and the cells keep
                        // their inks.
                        let under = pill(&shapes, stepper);
                        assert_eq!(under.fill, PALETTE.surface, "{who}");
                        let marks = chevrons(&shapes, stepper);
                        assert_eq!(marks.len(), 2, "{who}: two chevrons");
                        for mark in marks {
                            assert_eq!(mark.stroke.color, ColorMode::Solid(PALETTE.text), "{who}");
                        }
                        let rest = cell(PALETTE.surface, PALETTE.field_stroke, PALETTE.text);
                        for (other, other_field) in FIELDS.iter().enumerate() {
                            assert_eq!(
                                looks_of(&shapes, bounds[2 * other]),
                                vec![rest; other_field.parts.len()],
                                "{who}: the cells of {}",
                                other_field.name
                            );
                        }
                    }
                }
            }
        }

        #[test]
        fn the_cells_and_the_stepper_take_the_inks_of_the_states_when_the_schedule_is_off() {
            // The fields' `Ui` is disabled, but the view picks the disabled
            // inks itself and gives the opacity back, so each shape holds its
            // palette ink. A text's fade lands in its glyphs' vertices, not in
            // the color it was laid out in (`ink`), so the texts are read
            // from what is drawn (`drawn`): with the opacity not given back,
            // the shapes and the texts both come out at half.
            let off = cell(disabled(), PALETTE.outline, PALETTE.dim);
            let mut screen = schedule_off();
            let bounds = fields(&mut screen);
            let frame = screen.at_rest_clipped();
            let shapes = shapes_of(&frame);
            let line = bounds[0].center().y - theme::CELL_LIFT;
            for (index, field) in FIELDS.iter().enumerate() {
                let (rect, stepper) = (bounds[2 * index], bounds[2 * index + 1]);
                // Every cell and stepper halfway from `surface` to `window`
                // with an `outline` edge, every digit, chevron and name `dim`.
                assert_eq!(
                    looks_of(&shapes, rect),
                    vec![off; field.parts.len()],
                    "{}",
                    field.name
                );
                let pill = pill(&shapes, stepper);
                assert_eq!(pill.fill, disabled(), "{}: the pill", field.name);
                assert_eq!(
                    pill.stroke,
                    Stroke::new(1.0, PALETTE.outline),
                    "{}",
                    field.name
                );
                for mark in chevrons(&shapes, stepper) {
                    assert_eq!(
                        mark.stroke.color,
                        ColorMode::Solid(PALETTE.dim),
                        "{}",
                        field.name
                    );
                }
                assert_eq!(chevrons(&shapes, stepper).len(), 2);
                let names: Vec<&TextShape> = texts(&shapes)
                    .into_iter()
                    .filter(|text| {
                        text.galley.text() == field.name && close(text.pos.x, rect.min.x)
                    })
                    .collect();
                assert_eq!(names.len(), 1, "the name {}", field.name);
                assert_eq!(drawn(names[0]), PALETTE.dim, "the name {}", field.name);
            }
            // The colons and the unit are `dim` too.
            let quiet: Vec<&TextShape> = texts(&shapes)
                .into_iter()
                .filter(|text| {
                    matches!(text.galley.text(), ":" | "min") && close(middle(text).y, line)
                })
                .collect();
            assert_eq!(quiet.len(), 3);
            for text in quiet {
                assert_eq!(drawn(text), PALETTE.dim, "{:?}", text.galley.text());
            }
            // A click on a disabled field selects nothing and acts on nothing.
            let first = cells_of(&shapes, bounds[0])[0].rect.center();
            let shapes = click(&mut screen, first);
            for (index, field) in FIELDS.iter().enumerate() {
                assert_eq!(
                    looks_of(&shapes, bounds[2 * index]),
                    vec![off; field.parts.len()],
                    "{} after a click",
                    field.name
                );
            }
            assert!(screen.app.actions.is_empty(), "{:?}", screen.app.actions);
            // The pointer over a cell, or over a stepper half, lights nothing.
            let shapes = screen.hover(first);
            assert_eq!(looks_of(&shapes, bounds[0]), vec![off; 2], "hovered");
            for field in &FIELDS {
                for half in ["up", "down"] {
                    let at = screen
                        .control(&format!("{} {half}", field.name_label()))
                        .center();
                    let frame = hovering(&mut screen, at);
                    assert!(
                        lit(&frame).is_empty(),
                        "{} {half} lit under the pointer",
                        field.name
                    );
                    screen.press(at);
                    let frame = screen.at_rest_clipped();
                    assert!(
                        lit(&frame).is_empty(),
                        "{} {half} lit when pressed",
                        field.name
                    );
                    screen.release(at);
                }
            }
        }

        #[test]
        fn a_click_just_inside_the_hours_cells_right_end_selects_the_hours() {
            let mut screen = at_night();
            let from = fields(&mut screen)[0];
            let cells = cells_of(&screen.at_rest(), from);
            let line = from.center().y;
            // The last 4 points of the hours' cell went to the minutes while
            // `Spinner::input` was given the digits' rects.
            for (where_, at, selected) in [
                ("hours' right end", pos2(cells[0].rect.max.x - 1.0, line), 0),
                (
                    "minutes' left end",
                    pos2(cells[1].rect.min.x + 1.0, line),
                    1,
                ),
                ("past the last cell", pos2(from.max.x - 1.0, line), 1),
            ] {
                let mut screen = at_night();
                let shapes = click(&mut screen, at);
                let looks = looks_of(&shapes, from);
                for (part, look) in looks.iter().enumerate() {
                    let fill = if part == selected {
                        PALETTE.selection
                    } else {
                        PALETTE.surface
                    };
                    assert_eq!(
                        look.0, fill,
                        "a click at the {where_}: part {part} of {looks:?}"
                    );
                }
            }
        }

        /// Where the parts of the field at `field` stand in a frame: each
        /// cell's rect with the digits it shows, and the middle of each colon.
        fn parts_of(shapes: &[Shape], field: Rect) -> (Vec<(Rect, String)>, Vec<Pos2>) {
            let cells = cells_of(shapes, field)
                .iter()
                .map(|cell| {
                    let digits = texts_in(shapes, cell.rect);
                    assert_eq!(digits.len(), 1, "texts in the cell at {:?}", cell.rect);
                    (cell.rect, digits[0].galley.text().to_owned())
                })
                .collect();
            let colons = texts_in(shapes, field)
                .into_iter()
                .filter(|text| text.galley.text() == ":")
                .map(middle)
                .collect();
            (cells, colons)
        }

        #[test]
        fn stepping_a_time_leaves_its_cells_and_its_colon_where_they_were() {
            // The face's digits differ in width (a 1 is narrower than an 8),
            // so cells as wide as their own digits would move with each step
            // of a held stepper, and the colon and the next cell with them.
            let font = theme::semibold(15.0);
            assert!(
                width_of("1", font.clone()) + 1.0 < width_of("8", font),
                "the premise: the digits differ in width"
            );
            let mut screen = at_night();
            let bounds = fields(&mut screen);
            type Set = fn(&mut App, ClockTime);
            let sets: [(&str, Rect, Set); 2] = [
                ("From", bounds[0], |app, time| app.settings.start = time),
                ("To", bounds[2], |app, time| app.settings.end = time),
            ];
            for (name, field, set) in sets {
                // The hours through the day, with the minutes at 00, then the
                // minutes through the hour, with the hours at 11.
                let times = (0..24)
                    .map(|hour| (hour, 0))
                    .chain((0..60).map(|minute| (11, minute)));
                let mut first = None;
                for (hour, minute) in times {
                    set(&mut screen.app, ClockTime::new(hour, minute));
                    let (cells, colons) = parts_of(&screen.at_rest(), field);
                    let who = format!("{name} at {hour:02}:{minute:02}");
                    let shown: Vec<&str> =
                        cells.iter().map(|(_, digits)| digits.as_str()).collect();
                    assert_eq!(
                        shown,
                        [format!("{hour:02}"), format!("{minute:02}")],
                        "{who}"
                    );
                    let (first_cells, first_colons) =
                        first.get_or_insert_with(|| (cells.clone(), colons.clone()));
                    assert_eq!(colons.len(), 1, "{who}: a colon");
                    assert!(at(colons[0], first_colons[0]), "{who}: the colon moved");
                    for (part, ((rect, _), (was, _))) in
                        cells.iter().zip(first_cells.iter()).enumerate()
                    {
                        assert!(
                            near(*rect, *was),
                            "{who}: part {part} is {rect:?}, was {was:?}"
                        );
                    }
                }
            }
        }

        #[test]
        fn fades_cell_changes_only_with_its_count_of_digits() {
            let font = theme::semibold(15.0);
            let widest = ('0'..='9')
                .map(|digit| width_of(&digit.to_string(), font.clone()))
                .fold(0.0, f32::max);
            let mut screen = at_night();
            let fade = fields(&mut screen)[4];
            // The cell of each count of digits (1 for 5, 2 for 20 and 25, 3
            // for 120), as the first value of that count showed it.
            let mut by_count: Vec<Option<Rect>> = vec![None; 3];
            for minutes in 1..=crate::settings::FADE_MAX_MINUTES {
                screen.app.settings.fade_minutes = minutes;
                let shapes = screen.at_rest();
                let (cells, colons) = parts_of(&shapes, fade);
                let who = format!("Fade at {minutes}");
                assert_eq!(cells.len(), 1, "{who}: one cell");
                assert!(colons.is_empty(), "{who}: no colon");
                assert_eq!(cells[0].1, minutes.to_string(), "{who}");
                let rect = cells[0].0;
                assert!(
                    close(rect.min.x, fade.min.x),
                    "{who}: starts at the field's left"
                );
                let was = by_count[cells[0].1.len() - 1].get_or_insert(rect);
                assert!(
                    near(rect, *was),
                    "{who}: {rect:?}, was {was:?} for its count"
                );
                // The unit follows the cell.
                let unit: Vec<&TextShape> = texts_in(&shapes, fade)
                    .into_iter()
                    .filter(|text| text.galley.text() == "min")
                    .collect();
                assert_eq!(unit.len(), 1, "{who}: the unit");
                assert!(
                    close(unit[0].pos.x, rect.max.x + theme::UNIT_GAP),
                    "{who}: the unit"
                );
            }
            // A digit more widens it by the widest digit, to the layout's
            // rounding.
            let [one, two, three] =
                [0, 1, 2].map(|count| by_count[count].expect("a count of digits"));
            for (narrow, wide) in [(one, two), (two, three)] {
                assert!(
                    (wide.width() - narrow.width() - widest).abs() < 0.02,
                    "{} then {}, a digit is {widest}",
                    narrow.width(),
                    wide.width()
                );
            }
        }

        #[test]
        fn the_owners_picks_are_a_24_point_cell_a_32_point_key_and_semibold_15() {
            // The owner's choices of 2026-11-12 for the controls, which no
            // other test holds as a number.
            let mut screen = at_night();
            let bounds = fields(&mut screen);
            let shapes = screen.at_rest();
            let cells = cells_of(&shapes, bounds[0]);
            assert_eq!(cells[0].rect.height(), 24.0, "the cell");
            let digits = texts_in(&shapes, cells[0].rect);
            assert_eq!(font_of(digits[0]), theme::semibold(15.0), "the digits");
            let [red, close] = keys(&shapes);
            assert_eq!(
                (red.radius, close.radius),
                (16.0, 16.0),
                "the keys, 32 across"
            );
        }

        // Red Now and Close are keys.

        fn red_now_in_every_state(on: bool) {
            let make = || if on { red_now_on() } else { at_night() };
            let (fill, lit_fill, edge, glyph) = if on {
                (PALETTE.text, PALETTE.text, Stroke::NONE, PALETTE.window)
            } else {
                (
                    PALETTE.surface,
                    PALETTE.surface_hover,
                    Stroke::new(1.0, PALETTE.field_stroke),
                    PALETTE.text,
                )
            };
            let rest = make().at_rest();
            let name_at = middle(text_of(&rest, "Red Now"));
            let key_at = pos2(LEFT + theme::KEY / 2.0, bar().center().y);
            let [key, _] = keys(&rest);
            assert!(
                name_at.distance(key.center) > key.radius + 1.0,
                "the name's middle is over the key"
            );
            for pointer in Pointer::ALL {
                let who = format!("Red Now {}, {pointer:?}", if on { "on" } else { "off" });
                let shapes = with_pointer(&mut make(), pointer, key_at, name_at);
                let [key, _] = keys(&shapes);
                // A 32-point disc at the left margin on the bar's middle
                // line; the edge is stroked centered, so with it the key
                // measures 33 across (32 when on, with no edge).
                assert!(at(key.center, key_at), "{who}: {:?}", key.center);
                assert_eq!(key.radius, theme::KEY / 2.0, "{who}");
                assert_eq!(
                    key.fill,
                    if pointer.lights() { lit_fill } else { fill },
                    "{who}"
                );
                assert_eq!(key.stroke, edge, "{who}: the edge");
                assert_eq!(
                    key.radius + key.stroke.width / 2.0,
                    theme::KEY / 2.0 + if on { 0.0 } else { 0.5 },
                    "{who}"
                );
                // The glyph: a polygon and a line over the key, in one ink.
                let (polygons, lines) = over(&shapes, &key);
                assert_eq!((polygons.len(), lines.len()), (1, 1), "{who}: the glyph");
                assert_eq!(polygons[0].fill, glyph, "{who}: the glyph's sun");
                assert_eq!(lines[0].1.color, glyph, "{who}: the glyph's line");
                // The name, 8 points right of the key.
                let name = name_of(&shapes, "Red Now", theme::medium(13.0));
                let gap = name.pos.x - (key.center.x + key.radius);
                assert!(
                    close(gap, theme::KEY_GAP),
                    "{who}: the name is {gap} from the key"
                );
            }
        }

        #[test]
        fn red_now_is_a_key_that_lights_with_the_pointer_anywhere_in_its_hit_area() {
            red_now_in_every_state(false);
        }

        #[test]
        fn red_now_on_is_a_key_in_the_text_ink_with_no_edge_in_every_state() {
            red_now_in_every_state(true);
        }

        #[test]
        fn close_is_a_gold_key_with_a_cross_that_lights_with_the_pointer_anywhere_in_its_hit_area()
        {
            let rest = at_night().at_rest();
            let name_at = middle(text_of(&rest, "Close"));
            let key_at = pos2(RIGHT - theme::KEY / 2.0, bar().center().y);
            let [_, key] = keys(&rest);
            assert!(
                name_at.distance(key.center) > key.radius + 1.0,
                "the name is over the key"
            );
            for pointer in Pointer::ALL {
                let who = format!("Close, {pointer:?}");
                let shapes = with_pointer(&mut at_night(), pointer, key_at, name_at);
                let [_, key] = keys(&shapes);
                assert!(at(key.center, key_at), "{who}: {:?}", key.center);
                assert_eq!(key.radius, theme::KEY / 2.0, "{who}");
                assert_eq!(
                    key.fill,
                    if pointer.lights() {
                        PALETTE.accent_hover
                    } else {
                        PALETTE.accent
                    },
                    "{who}"
                );
                assert_eq!(key.stroke, Stroke::NONE, "{who}: no edge");
                // The cross: two 1.7-point strokes in `on_accent`, 9 points
                // square about the center, one along each diagonal.
                let (polygons, lines) = over(&shapes, &key);
                assert!(polygons.is_empty(), "{who}: no glyph");
                assert_eq!(lines.len(), 2, "{who}: {lines:?}");
                let reach = theme::CROSS / 2.0;
                for diagonal in [
                    [vec2(-reach, -reach), vec2(reach, reach)],
                    [vec2(-reach, reach), vec2(reach, -reach)],
                ] {
                    let [from, to] = diagonal.map(|offset| key.center + offset);
                    assert!(
                        lines.iter().any(|(points, stroke)| {
                            *stroke == Stroke::new(theme::CROSS_STROKE, PALETTE.on_accent)
                                && ((at(points[0], from) && at(points[1], to))
                                    || (at(points[0], to) && at(points[1], from)))
                        }),
                        "{who}: no stroke from {from:?} to {to:?} in {lines:?}"
                    );
                }
                // The name, 8 points left of the key.
                let name = name_of(&shapes, "Close", theme::semibold(13.0));
                let gap = (key.center.x - key.radius) - (name.pos.x + name.galley.size().x);
                assert!(
                    close(gap, theme::KEY_GAP),
                    "{who}: the name is {gap} from the key"
                );
            }
        }

        // The new pairs meet in the picture.

        /// The fill that shows at `at` under the shape painted at `index` of
        /// `frame`: the topmost rect or disc painted before it that covers
        /// `at` inside its clip rectangle.
        fn ground(frame: &[(Rect, Shape)], index: usize, at: Pos2) -> Color32 {
            frame[..index]
                .iter()
                .rev()
                .find_map(|(clip, shape)| {
                    if !clip.contains(at) {
                        return None;
                    }
                    match shape {
                        Shape::Rect(rect)
                            if rect.rect.contains(at) && rect.fill != Color32::TRANSPARENT =>
                        {
                            Some(rect.fill)
                        }
                        Shape::Circle(disc)
                            if disc.center.distance(at) <= disc.radius
                                && disc.fill != Color32::TRANSPARENT =>
                        {
                            Some(disc.fill)
                        }
                        _ => None,
                    }
                })
                .expect("the window's ground stands behind every shape")
        }

        /// What the frames so far showed behind each pair's front.
        #[derive(Default)]
        struct Seen {
            digits: Vec<Color32>,
            chevrons: Vec<Color32>,
            glyphs: Vec<Color32>,
            crosses: Vec<Color32>,
            /// The pairs that meet at a rim rather than on a fill, each as
            /// what it is, its color and the ground it faces: a cell's fill
            /// and a cell's or pill's edge on the window, a key on the panel,
            /// the focus ring on the ground it is drawn on.
            at_a_rim: Vec<(&'static str, Color32, Color32)>,
        }

        /// The frame of `screen` that `frame` makes, checked: each front
        /// stands on the ground its pair is checked against.
        fn study(
            screen: &mut Screen,
            frame: impl FnOnce(&mut Screen) -> Vec<(Rect, Shape)>,
            seen: &mut Seen,
        ) {
            let bounds = fields(screen);
            let frame = frame(screen);
            let shapes = shapes_of(&frame);
            let steppers = [bounds[1], bounds[3], bounds[5]];
            let cell_grounds = [
                PALETTE.surface,
                PALETTE.surface_hover,
                PALETTE.selection,
                disabled(),
            ];
            let pill_grounds = [
                PALETTE.surface,
                PALETTE.surface_hover,
                PALETTE.surface_active,
                disabled(),
            ];
            let [red, close] = keys(&shapes);
            let cells: Vec<Rect> = [bounds[0], bounds[2], bounds[4]]
                .into_iter()
                .flat_map(|field| cells_of(&shapes, field))
                .map(|cell| cell.rect)
                .collect();
            let (mut digits, mut quiet, mut names) = (0, 0, 0);
            for (index, (_, shape)) in frame.iter().enumerate() {
                match shape {
                    Shape::Rect(ring)
                        if ring.stroke_kind == StrokeKind::Outside
                            && ring.stroke.color == PALETTE.accent =>
                    {
                        // The focus ring: on the panel round a key's hit
                        // area, on the window round a field or a stepper,
                        // with the same ground just outside and just inside.
                        let want = if bar().contains(ring.rect.center()) {
                            PALETTE.panel
                        } else {
                            PALETTE.window
                        };
                        for beside in [
                            ring.rect.center_top() - vec2(0.0, 2.0),
                            ring.rect.center_top() + vec2(0.0, 1.0),
                        ] {
                            assert_eq!(ground(&frame, index, beside), want, "the ring");
                        }
                        seen.at_a_rim.push(("ring", ring.stroke.color, want));
                    }
                    Shape::Rect(body)
                        if cells
                            .iter()
                            .chain(&steppers)
                            .any(|rect| near(*rect, body.rect)) =>
                    {
                        // A cell or a pill stands on the window, which faces
                        // its fill and its edge across its rim.
                        let beside = ground(&frame, index, body.rect.center_top() - vec2(0.0, 1.0));
                        assert_eq!(beside, PALETTE.window, "a cell or pill at {:?}", body.rect);
                        seen.at_a_rim.push(("fill", body.fill, beside));
                        seen.at_a_rim.push(("edge", body.stroke.color, beside));
                    }
                    Shape::Circle(disc) if [red.center, close.center].contains(&disc.center) => {
                        // A key stands on the panel.
                        let beside =
                            ground(&frame, index, disc.center - vec2(0.0, disc.radius + 1.0));
                        assert_eq!(beside, PALETTE.panel, "a key at {:?}", disc.center);
                        let which = if disc.center == red.center {
                            "Red Now's key"
                        } else {
                            "Close's key"
                        };
                        seen.at_a_rim.push((which, disc.fill, beside));
                    }
                    Shape::Text(text) => {
                        let (content, middle) = (text.galley.text(), middle(text));
                        if font_of(text) == theme::semibold(15.0) && content != ":" {
                            // A digit stands in a cell: `text` on a cell's fill.
                            let ground = ground(&frame, index, middle);
                            assert!(cell_grounds.contains(&ground), "{content:?} on {ground:?}");
                            seen.digits.push(ground);
                            digits += 1;
                        } else if content == ":" || content == "min" {
                            // The colon and the unit stand on the window.
                            assert_eq!(
                                ground(&frame, index, middle),
                                PALETTE.window,
                                "{content:?}"
                            );
                            quiet += 1;
                        } else if content == "Red Now" || content == "Close" {
                            // A name stands on the panel, clear of both keys.
                            let rect = Rect::from_min_size(text.pos, text.galley.size());
                            for corner in [rect.left_top(), rect.right_bottom(), rect.center()] {
                                assert_eq!(
                                    ground(&frame, index, corner),
                                    PALETTE.panel,
                                    "{content}"
                                );
                            }
                            for key in [&red, &close] {
                                assert!(
                                    rect.clamp(key.center).distance(key.center) > key.radius,
                                    "{content} touches a key"
                                );
                            }
                            names += 1;
                        }
                    }
                    Shape::Path(path) if steppers.iter().any(|s| is_chevron(path, *s)) => {
                        // A chevron stands in its stepper, on the pill or on
                        // the half that is lit.
                        for point in &path.points {
                            let ground = ground(&frame, index, *point);
                            assert!(pill_grounds.contains(&ground), "a chevron on {ground:?}");
                            seen.chevrons.push(ground);
                        }
                    }
                    Shape::Path(path) if path.points.iter().all(|p| in_key(p, &red)) => {
                        // Red Now's sun stands in its key.
                        for point in &path.points {
                            assert_eq!(ground(&frame, index, *point), red.fill, "the sun");
                        }
                        seen.glyphs.push(red.fill);
                    }
                    Shape::LineSegment { points, .. } => {
                        for (key, seen) in [(&red, &mut seen.glyphs), (&close, &mut seen.crosses)] {
                            if points.iter().all(|p| in_key(p, key)) {
                                for point in points {
                                    assert_eq!(ground(&frame, index, *point), key.fill, "a stroke");
                                }
                                seen.push(key.fill);
                            }
                        }
                    }
                    _ => {}
                }
            }
            assert_eq!(
                (digits, quiet, names),
                (5, 3, 2),
                "digits, colons and unit, names"
            );
        }

        #[test]
        fn the_new_pairs_meet_in_the_picture() {
            let mut seen = Seen::default();
            // At rest, the pointer in a gap of From, From selected, a stepper
            // half hovered and pressed, and the keys hovered.
            study(
                &mut at_night(),
                |screen| screen.at_rest_clipped(),
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let from = screen.control("From: ");
                    hovering(screen, pos2(from.max.x - 2.0, from.center().y))
                },
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let from = screen.control("From: ");
                    click(screen, pos2(from.min.x + 5.0, from.center().y));
                    screen.at_rest_clipped()
                },
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let up = screen.control("From up").center();
                    hovering(screen, up)
                },
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let down = screen.control("To down").center();
                    screen.press(down);
                    screen.at_rest_clipped()
                },
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let name = middle(text_of(&screen.at_rest(), "Red Now"));
                    hovering(screen, name)
                },
                &mut seen,
            );
            study(
                &mut at_night(),
                |screen| {
                    let name = middle(text_of(&screen.at_rest(), "Close"));
                    hovering(screen, name)
                },
                &mut seen,
            );
            study(
                &mut schedule_off(),
                |screen| screen.at_rest_clipped(),
                &mut seen,
            );
            study(
                &mut red_now_on(),
                |screen| screen.at_rest_clipped(),
                &mut seen,
            );
            // Red Now and Close focused: the ring on the panel.
            for id in ["red-now", "close"] {
                study(
                    &mut at_night(),
                    |screen| {
                        screen
                            .ctx
                            .memory_mut(|memory| memory.request_focus(Id::new(id)));
                        screen.at_rest_clipped()
                    },
                    &mut seen,
                );
            }

            // The pairs were all met: a digit on each cell
            // fill, a chevron on each pill fill, Red Now's glyph on each of
            // its keys' fills, and Close's cross on both of its.
            for (what, found, wanted) in [
                (
                    "digits",
                    &seen.digits,
                    vec![
                        PALETTE.surface,
                        PALETTE.surface_hover,
                        PALETTE.selection,
                        disabled(),
                    ],
                ),
                (
                    "chevrons",
                    &seen.chevrons,
                    vec![
                        PALETTE.surface,
                        PALETTE.surface_hover,
                        PALETTE.surface_active,
                        disabled(),
                    ],
                ),
                (
                    "Red Now's glyph",
                    &seen.glyphs,
                    vec![PALETTE.surface, PALETTE.surface_hover, PALETTE.text],
                ),
                (
                    "Close's cross",
                    &seen.crosses,
                    vec![PALETTE.accent, PALETTE.accent_hover],
                ),
            ] {
                for fill in wanted {
                    assert!(found.contains(&fill), "no {what} met {fill:?}: {found:?}");
                }
            }

            // The pairs that meet at a rim: a selected cell and the disabled
            // edge on the window, the focus ring and the keys on the panel.
            for pair in [
                ("fill", PALETTE.selection, PALETTE.window),
                ("edge", PALETTE.outline, PALETTE.window),
                ("ring", PALETTE.accent, PALETTE.panel),
                ("Close's key", PALETTE.accent, PALETTE.panel),
                ("Close's key", PALETTE.accent_hover, PALETTE.panel),
                ("Red Now's key", PALETTE.text, PALETTE.panel),
            ] {
                assert!(
                    seen.at_a_rim.contains(&pair),
                    "no {pair:?} met: {:?}",
                    seen.at_a_rim
                );
            }
        }

        // Sizes, hit areas, labels, focus and keys.

        #[test]
        fn the_fields_and_steppers_keep_todays_rects() {
            let bounds = fields(&mut at_night());
            // Today's numbers: the fields
            // 94 wide at 24, 152 and 280, 30 tall from 441 to 471, each
            // stepper 4 points past its field, the last ending at the right
            // margin.
            for (index, (left, right)) in [(24.0, 118.0), (152.0, 246.0), (280.0, 374.0)]
                .into_iter()
                .enumerate()
            {
                let (field, stepper) = (bounds[2 * index], bounds[2 * index + 1]);
                let want = Rect::from_min_max(pos2(left, 441.0), pos2(right, 471.0));
                assert_eq!(field, want, "{}'s field", FIELDS[index].name);
                let want = Rect::from_min_size(pos2(right + 4.0, 441.0), theme::STEPPER);
                assert_eq!(stepper, want, "{}'s stepper", FIELDS[index].name);
            }
            assert_eq!(
                bounds[5].max.x, RIGHT,
                "the last stepper ends at the margin"
            );
        }

        #[test]
        fn the_keys_hit_areas_are_the_key_the_name_and_a_little_more() {
            let mut screen = at_night();
            let (red, close) = (screen.control("Red Now"), screen.control("Close"));
            let line = bar().center().y;
            let width = |name: &str, font: egui::FontId| {
                theme::KEY + theme::KEY_GAP + width_of(name, font) + theme::KEY_TAIL
            };
            let red_width = width("Red Now", theme::medium(13.0));
            let close_width = width("Close", theme::semibold(13.0));
            let key_tall = |left: f32, width: f32| {
                Rect::from_min_size(pos2(left, line - theme::KEY / 2.0), vec2(width, theme::KEY))
            };
            assert!(
                near(red, key_tall(LEFT, red_width)),
                "Red Now's hit area {red:?}"
            );
            assert!(
                near(close, key_tall(RIGHT - close_width, close_width)),
                "Close's hit area {close:?}"
            );
            // A press 1 point inside the far end, past the name, acts, and the
            // name's center acts. (egui picks the nearest clickable widget
            // within its `interact_radius` of a press that lands on none, so
            // "outside" is past that.)
            let radius = screen
                .ctx
                .style_of(egui::Theme::Dark)
                .interaction
                .interact_radius;
            let rest = screen.at_rest();
            for (label, rect, inside, outside, name) in [
                (
                    "Red Now",
                    red,
                    pos2(red.max.x - 1.0, line),
                    pos2(red.max.x + radius + 1.0, line),
                    middle(text_of(&rest, "Red Now")),
                ),
                (
                    "Close",
                    close,
                    pos2(close.min.x + 1.0, line),
                    pos2(close.min.x - radius - 1.0, line),
                    middle(text_of(&rest, "Close")),
                ),
            ] {
                let acts = |at: Pos2| {
                    let mut screen = at_night();
                    screen.press(at);
                    screen.release(at);
                    std::mem::take(&mut screen.app.actions)
                };
                let action = if label == "Red Now" {
                    Action::RedNow(true)
                } else {
                    Action::HideWindow
                };
                assert!(
                    rect.contains(inside) && rect.contains(name),
                    "{label}: {rect:?}"
                );
                assert!(
                    acts(inside).contains(&action),
                    "{label}: a press at {inside:?}"
                );
                assert!(
                    acts(name).contains(&action),
                    "{label}: a press on the name at {name:?}"
                );
                assert_eq!(
                    acts(outside),
                    [],
                    "{label}: a press at {outside:?}, outside"
                );
                // The area is 32 points tall: a press just inside its top
                // and its bottom, over the name, acts, and one past egui's
                // reach above or below it does nothing.
                for (where_, at) in [
                    ("just inside the top", pos2(name.x, rect.min.y + 1.0)),
                    ("just inside the bottom", pos2(name.x, rect.max.y - 1.0)),
                ] {
                    assert!(
                        rect.contains(at) && acts(at).contains(&action),
                        "{label}: a press {where_}, at {at:?}"
                    );
                }
                for (where_, at) in [
                    ("above", pos2(name.x, rect.min.y - radius - 1.0)),
                    ("below", pos2(name.x, rect.max.y + radius + 1.0)),
                ] {
                    assert_eq!(acts(at), [], "{label}: a press {where_}, at {at:?}");
                }
            }
        }

        #[test]
        fn a_focused_control_draws_its_ring() {
            let mut screen = at_night();
            let bounds = fields(&mut screen);
            let (red, close) = (screen.control("Red Now"), screen.control("Close"));
            let ring_radius = |radius: f32| CornerRadius::same((radius + 2.0).round() as u8);
            let controls = [
                ("From", Id::new("from"), bounds[0], ring_radius(6.0)),
                (
                    "From's stepper",
                    Id::new("from").with("up"),
                    bounds[1],
                    ring_radius(f32::from(theme::STEPPER_RADIUS)),
                ),
                (
                    "Red Now",
                    Id::new("red-now"),
                    red,
                    ring_radius(theme::KEY / 2.0),
                ),
                (
                    "Close",
                    Id::new("close"),
                    close,
                    ring_radius(theme::KEY / 2.0),
                ),
            ];
            let accent = Stroke::new(1.0, PALETTE.accent);
            let rings = |shapes: &[Shape]| -> Vec<RectShape> {
                rects(shapes)
                    .into_iter()
                    .filter(|shape| {
                        shape.stroke_kind == StrokeKind::Outside && shape.stroke == accent
                    })
                    .cloned()
                    .collect()
            };
            assert_eq!(rings(&screen.at_rest()), [], "no ring with nothing focused");
            for (name, id, rect, radius) in controls {
                let mut screen = at_night();
                screen.ctx.memory_mut(|memory| memory.request_focus(id));
                let found = rings(&screen.at_rest());
                assert_eq!(found.len(), 1, "{name}: rings {found:?}");
                // 2 points outside the control's rect, at its radius plus 2.
                assert!(
                    near(found[0].rect, rect.expand(2.0)),
                    "{name}: {:?}",
                    found[0].rect
                );
                assert_eq!(found[0].corner_radius, radius, "{name}");
                assert_eq!(
                    found[0].fill,
                    Color32::TRANSPARENT,
                    "{name}: a line, not a fill"
                );
            }
        }

        #[test]
        fn typing_and_stepping_in_a_focused_field() {
            let mut screen = at_night();
            let from = fields(&mut screen)[0];
            let cells = cells_of(&screen.at_rest(), from);
            let selected = cell(PALETTE.selection, PALETTE.field_stroke, PALETTE.text);
            let rest = cell(PALETTE.surface, PALETTE.field_stroke, PALETTE.text);
            let start = |hour, minute| vec![Action::SetStart(ClockTime::new(hour, minute))];
            // A click in the hours' cell focuses From and selects the hours.
            // The frames' actions stay in `app.actions`, never applied, so
            // From stays 21:00.
            let shapes = click(&mut screen, cells[0].rect.center());
            assert_eq!(looks_of(&shapes, from), [selected, rest]);
            screen.app.actions.clear();
            // The digit 5 replaces the hours and keeps the minutes, and the
            // hours stay selected: a second digit may follow.
            screen.frame(vec![Event::Text("5".to_owned())]);
            assert_eq!(screen.app.actions, start(5, 0), "the digit 5");
            screen.app.actions.clear();
            let shapes = screen.at_rest();
            assert_eq!(looks_of(&shapes, from), [selected, rest], "after the 5");
            // Up adds an hour.
            screen.frame(vec![key_down(Key::ArrowUp, Modifiers::NONE)]);
            assert_eq!(screen.app.actions, start(22, 0), "Up");
            screen.app.actions.clear();
            // Right moves the selection to the minutes, which Up then moves in
            // fives.
            screen.frame(vec![key_down(Key::ArrowRight, Modifiers::NONE)]);
            assert_eq!(screen.app.actions, [], "Right edits nothing");
            let shapes = screen.at_rest();
            assert_eq!(looks_of(&shapes, from), [rest, selected], "after Right");
            screen.frame(vec![key_down(Key::ArrowUp, Modifiers::NONE)]);
            assert_eq!(screen.app.actions, start(21, 5), "Up on the minutes");
        }
    }

    /// The two sliders through whole frames of the window (`ui::draw` on a
    /// `test_app`), with pointer events: what the sliders put on the screen.
    /// The rows' center lines come from the view's own rows, so that moving a
    /// row does not strand these tests;
    /// `layout::the_rows_sit_where_the_spec_puts_them` keeps those rows at
    /// their numbers.
    mod sliders {
        use super::*;
        use crate::color::Tint;
        use crate::ui::testing::{self, circles, ink, middle, rects, sides, texts};
        use egui::epaint::{CircleShape, RectShape, TextShape};
        use egui::{Event, Mesh, Shape};

        /// The strips run across the margins.
        const LEFT_END: f32 = 24.0;
        const TRAVEL: f32 = 372.0;

        /// A slider row: its label, which names its widget, and the y of its
        /// center line.
        #[derive(Clone, Copy)]
        struct Row {
            label: &'static str,
            y: f32,
        }

        const TEMPERATURE_ROW: Row = Row {
            label: TEMPERATURE,
            y: TEMPERATURE_TOP + STRIP_BELOW_TOP,
        };
        const BRIGHTNESS_ROW: Row = Row {
            label: "Brightness",
            y: BRIGHTNESS_TOP + STRIP_BELOW_TOP,
        };

        /// A strip is this far from its center line to either edge.
        const HALF_TRACK: f32 = theme::SLIDER_TRACK / 2.0;

        /// The rectangle of the strip whose center line is at `y`.
        fn track(y: f32) -> Rect {
            Rect::from_min_max(
                pos2(LEFT_END, y - HALF_TRACK),
                pos2(LEFT_END + TRAVEL, y + HALF_TRACK),
            )
        }

        fn night(kelvin: u16, brightness: u8) -> Look {
            Look { kelvin, brightness }
        }

        /// The window through a series of frames, so that hover, focus and
        /// drag carry from one frame to the next.
        type Window = testing::Screen;

        impl Window {
            /// The screen is already tinted `live`.
            fn tinted(mut self, live: Tint) -> Self {
                self.app.status.tint = live;
                self
            }

            /// The row has the keyboard.
            fn focus(&mut self, row: Row) -> Vec<Shape> {
                self.ctx
                    .memory_mut(|memory| memory.request_focus(Id::new(row.label)));
                self.at_rest()
            }
        }

        /// A strip's position at a point of x.
        fn position(x: f32) -> f32 {
            (x - LEFT_END) / TRAVEL
        }

        /// A point of x at a position of a strip.
        fn x_of(position: f32) -> f32 {
            LEFT_END + TRAVEL * position
        }

        fn veil(color: Color32) -> Color32 {
            Palette::DARK.veiled(color)
        }

        /// What white becomes at a position of the scale, on a neutral
        /// screen.
        fn scale(position: f32) -> Color32 {
            color::scale_tint(position).apply(Color32::WHITE)
        }

        /// The one strip drawn over `row`'s center line, and where it comes in
        /// the paint order.
        fn strip_at(shapes: &[Shape], row: Row) -> (usize, &Mesh) {
            let found: Vec<(usize, &Mesh)> = shapes
                .iter()
                .enumerate()
                .filter_map(|(at, shape)| match shape {
                    Shape::Mesh(mesh)
                        if !mesh.vertices.is_empty()
                            && mesh.vertices.iter().all(|v| {
                                v.pos.y == track(row.y).min.y || v.pos.y == track(row.y).max.y
                            }) =>
                    {
                        Some((at, &**mesh))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(found.len(), 1, "strips at y {}", row.y);
            found[0]
        }

        fn strip(shapes: &[Shape], row: Row) -> &Mesh {
            strip_at(shapes, row).1
        }

        /// The discs that round the ends of `row`'s strip, left and right.
        fn ends(shapes: &[Shape], row: Row) -> Vec<&CircleShape> {
            circles(shapes)
                .into_iter()
                .filter(|disc| disc.radius == HALF_TRACK && disc.center.y == row.y)
                .collect()
        }

        /// Where a strip first turns veiled, read off its vertices; none if
        /// it never does. `light` is its color at a position before the veil.
        fn edge(mesh: &Mesh, light: impl Fn(f32) -> Color32) -> Option<f32> {
            mesh.vertices
                .iter()
                .filter(|v| v.color == veil(light(position(v.pos.x))))
                .map(|v| v.pos.x)
                .min_by(f32::total_cmp)
        }

        /// Checks every vertex of `mesh` is `light` left of `edge` and its
        /// veil right of it.
        fn assert_lit_then_veiled(mesh: &Mesh, edge: f32, light: impl Fn(f32) -> Color32) {
            let (lit, veiled) = sides(mesh, edge);
            for vertex in lit {
                assert_eq!(
                    vertex.color,
                    light(position(vertex.pos.x)),
                    "x {} should be lit",
                    vertex.pos.x
                );
            }
            for vertex in veiled {
                assert_eq!(
                    vertex.color,
                    veil(light(position(vertex.pos.x))),
                    "x {} should be veiled",
                    vertex.pos.x
                );
            }
        }

        #[test]
        fn the_temperature_strip_is_lit_up_to_the_value() {
            // The value's x is where the stop's tick is: 137.8 for 2700 K and
            // 279.8 for 1200 K.
            for (kelvin, edge_at, left_lit, right_lit) in [
                (1200, Some(279.8), true, false),
                (2700, Some(137.8), true, false),
                // Daylight is all veiled, and pure red all lit.
                (6500, Some(27.0), false, false),
                (850, None, true, true),
            ] {
                let shapes = Window::new(night(kelvin, 100)).at_rest();
                let mesh = strip(&shapes, TEMPERATURE_ROW);
                match (edge(mesh, scale), edge_at) {
                    (Some(found), Some(want)) => {
                        assert!(
                            (found - want).abs() < 0.05,
                            "{kelvin} K: the edge is at {found}, not {want}"
                        );
                        assert_lit_then_veiled(mesh, found, scale);
                    }
                    (None, None) => {
                        for vertex in &mesh.vertices {
                            assert_eq!(vertex.color, scale(position(vertex.pos.x)));
                        }
                    }
                    (found, want) => panic!("{kelvin} K: the edge is at {found:?}, not {want:?}"),
                }
                let discs = ends(&shapes, TEMPERATURE_ROW);
                assert_eq!(discs.len(), 2, "{kelvin} K");
                for (disc, lit, at) in [(discs[0], left_lit, 0.0), (discs[1], right_lit, 1.0)] {
                    let want = if lit { scale(at) } else { veil(scale(at)) };
                    assert_eq!(disc.fill, want, "{kelvin} K at {at}");
                }
            }
        }

        #[test]
        fn the_lit_edge_sits_at_the_value_near_the_ends_too() {
            let y = TEMPERATURE_ROW.y;
            let strip = track(y);
            let assert_edge_at = |shapes: &[(Rect, Shape)], want: f32, what: &str| {
                let edge = testing::lit_edge(shapes, strip, scale);
                assert!(
                    (edge - want).abs() <= 0.5,
                    "{what}: the lit part ends at {edge}, not at {want}"
                );
            };
            // At rest, on the scale's own values, the two ends among them.
            for kelvin in [6500, 1200, 860, 850] {
                let want = x_of(color::redness_of(kelvin));
                let shapes = Window::new(night(kelvin, 100)).at_rest_clipped();
                assert_edge_at(&shapes, want, &format!("{kelvin} K at rest"));
            }
            // In hand the knob follows the pointer into the left round end:
            // 0.004 of the way is 1.5 points in from the left edge. Pure red's
            // zone holds the last 10.4 points at the right end, so 0.995, 1.9
            // short of it, goes there.
            for (position, held) in [
                (0.0, 0.0),
                (0.004, 0.004),
                // Clear of every detent's catch, which holds the value.
                (0.4, 0.4),
                (0.995, 1.0),
                (1.0, 1.0),
            ] {
                let at = x_of(position);
                let y = TEMPERATURE_ROW.y;
                let shapes = Window::new(night(1200, 100)).drag_clipped(pos2(60.0, y), pos2(at, y));
                assert_edge_at(&shapes, x_of(held), &format!("{position} in hand"));
            }
        }

        #[test]
        fn the_pairs_the_palette_checks_meet_in_the_picture() {
            let p = Palette::DARK;
            let shapes = Window::new(night(1200, 100)).at_rest();
            let mesh = strip(&shapes, TEMPERATURE_ROW);
            // The lit strip and the veiled strip meet at the value, and they
            // are the two reds that the palette test compares.
            let lit = Color32::from_rgb(0xFF, 0, 0);
            let at_edge = edge(mesh, scale).expect("a veiled part");
            let reds: Vec<u8> = mesh
                .vertices
                .iter()
                .filter(|v| (v.pos.x - at_edge).abs() < 1e-3)
                .map(|v| v.color.r())
                .collect();
            assert!(reds.contains(&lit.r()), "no lit red at the edge: {reds:?}");
            assert!(
                reds.contains(&p.veiled(lit).r()),
                "no veiled red at the edge: {reds:?}"
            );
            for vertex in &mesh.vertices {
                assert!(
                    [lit.r(), p.veiled(lit).r()].contains(&vertex.color.r()),
                    "x {}: red {}",
                    vertex.pos.x,
                    vertex.color.r()
                );
            }
            // The veiled strip meets the window: the window's ground is
            // painted under the whole strip, and the veiled end disc lies
            // on it.
            let y = TEMPERATURE_ROW.y;
            let strip_rect = track(y);
            let ground = shapes
                .iter()
                .position(|shape| {
                    matches!(shape, Shape::Rect(rect)
                        if rect.fill == p.window && rect.rect.contains_rect(strip_rect))
                })
                .expect("the window's ground");
            let (strip_at, _) = strip_at(&shapes, TEMPERATURE_ROW);
            assert!(ground < strip_at, "the ground is painted over the strip");
            assert_eq!(ends(&shapes, TEMPERATURE_ROW)[1].fill, p.veiled(scale(1.0)));
        }

        #[test]
        fn the_strip_is_drawn_so_that_the_tinted_screen_shows_the_scale() {
            for kelvin in [3000, 1200] {
                let live = night(kelvin, 100).tint();
                // Where the scale is as red as the screen.
                let as_red = color::redness_of(kelvin);
                // The night is pure red, so the whole strip is lit.
                let shapes = Window::new(night(850, 100)).tinted(live).at_rest();
                let mesh = strip(&shapes, TEMPERATURE_ROW);
                for vertex in &mesh.vertices {
                    let at = position(vertex.pos.x);
                    let shown = live.apply(vertex.color);
                    if at <= as_red - 0.05 {
                        // Whiter than the screen can show: it shows its own
                        // color.
                        assert_eq!(shown, live.apply(Color32::WHITE), "{kelvin} K at {at}");
                    } else if at >= as_red + 0.05 {
                        // Redder: the screen shows the scale's color.
                        let want = scale(at);
                        for (a, b) in [
                            (shown.r(), want.r()),
                            (shown.g(), want.g()),
                            (shown.b(), want.b()),
                        ] {
                            assert!(
                                a.abs_diff(b) <= 1,
                                "{kelvin} K at {at}: {shown:?} for {want:?}"
                            );
                        }
                    }
                }
                // The ends: the screen's own color at the left, pure red,
                // unchanged, at the right.
                let discs = ends(&shapes, TEMPERATURE_ROW);
                assert_eq!(live.apply(discs[0].fill), live.apply(Color32::WHITE));
                assert_eq!(discs[1].fill, Color32::from_rgb(0xFF, 0, 0));
                assert_eq!(live.apply(discs[1].fill), Color32::from_rgb(0xFF, 0, 0));
            }
        }

        /// The stop name set in 11.5 that reads `name`.
        fn name<'a>(shapes: &'a [Shape], name: &str) -> &'a TextShape {
            let found: Vec<&TextShape> = texts(shapes)
                .into_iter()
                .filter(|text| {
                    text.galley.text() == name
                        && text.galley.job.sections[0].format.font_id.size == 11.5
                })
                .collect();
            assert_eq!(found.len(), 1, "{name} named {} times", found.len());
            found[0]
        }

        /// The ticks under `row`'s strip, from the left: 1 point wide and 3
        /// tall, from 5 points below its center line.
        fn ticks_under(shapes: &[Shape], row: Row) -> Vec<&RectShape> {
            let mut ticks: Vec<&RectShape> = rects(shapes)
                .into_iter()
                .filter(|tick| {
                    tick.rect.width() == 1.0
                        && tick.rect.height() == 3.0
                        && tick.rect.min.y == row.y + 5.0
                })
                .collect();
            ticks.sort_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x));
            ticks
        }

        #[test]
        fn the_stop_the_value_sits_on_is_in_ink() {
            let p = Palette::DARK;
            let names = ["Daylight", "Incandescent", "Desert", "Ember", "Pure red"];
            for (kelvin, lit) in [
                (1200, Some("Ember")),
                (2700, Some("Incandescent")),
                (6500, Some("Daylight")),
                (850, Some("Pure red")),
                (shared::DESERT_KELVIN, Some("Desert")),
                // Between the stops none is lit, 5 K off Desert as well.
                (1500, None),
                (shared::DESERT_KELVIN + 5, None),
            ] {
                let shapes = Window::new(night(kelvin, 100)).at_rest();
                for stop in names {
                    let want = if lit == Some(stop) { p.text } else { p.dim };
                    assert_eq!(ink(name(&shapes, stop)), want, "{stop} at {kelvin} K");
                }
                // Only the three stops inside the track have ticks, and the
                // lit one's is in ink as well.
                let ticks = ticks_under(&shapes, TEMPERATURE_ROW);
                assert_eq!(ticks.len(), 3, "{kelvin} K");
                for (tick, stop) in ticks.iter().zip(["Incandescent", "Desert", "Ember"]) {
                    let want = if lit == Some(stop) { p.text } else { p.dim };
                    assert_eq!(tick.fill, want, "the tick of {stop} at {kelvin} K");
                }
            }
        }

        #[test]
        fn a_drag_lights_the_stop_it_lands_on_in_the_same_frame() {
            let p = Palette::DARK;
            let names = ["Daylight", "Incandescent", "Desert", "Ember", "Pure red"];
            let lit = |shapes: &[Shape]| -> Vec<&str> {
                names
                    .into_iter()
                    .filter(|stop| ink(name(shapes, stop)) == p.text)
                    .collect()
            };
            let mut window = Window::new(night(1500, 100));
            assert!(lit(&window.at_rest()).is_empty());
            // The move lands on 2700 K: the stop is lit in the frame of the
            // move, before the app has taken the new look.
            let y = TEMPERATURE_ROW.y;
            let on = |kelvin| pos2(x_of(color::redness_of(kelvin)), y);
            let shapes = window.drag(pos2(60.0, y), on(2700));
            assert_eq!(lit(&shapes), ["Incandescent"]);
            // Off it again: none. Then onto Ember.
            let shapes = window.frame(vec![Event::PointerMoved(on(2000))]);
            assert!(lit(&shapes).is_empty());
            let shapes = window.frame(vec![Event::PointerMoved(on(1200))]);
            assert_eq!(lit(&shapes), ["Ember"]);
        }

        #[test]
        fn the_names_sit_under_the_strip() {
            let shapes = Window::new(night(1200, 100)).at_rest();
            // 18 points under the center line.
            for stop in ["Daylight", "Incandescent", "Desert", "Ember", "Pure red"] {
                let y = middle(name(&shapes, stop)).y;
                let want = TEMPERATURE_ROW.y + 18.0;
                assert!((y - want).abs() < 0.01, "{stop} is centered at y {y}");
            }
            // The ends are flush with the margins.
            let daylight = name(&shapes, "Daylight");
            assert!((daylight.pos.x - 24.0).abs() < 0.01);
            let pure_red = name(&shapes, "Pure red");
            let right = pure_red.pos.x + pure_red.galley.size().x;
            assert!((right - 396.0).abs() < 0.01, "Pure red ends at {right}");
            // The ticks sit on whole device pixels at 2700 K, 1760 K and
            // 1200 K, and Incandescent and Ember are centered on theirs.
            // Desert's name is not: see the test of its place.
            let ticks = ticks_under(&shapes, TEMPERATURE_ROW);
            for (stop, tick, at) in [
                ("Incandescent", ticks[0], 137.8),
                ("Desert", ticks[1], 208.7),
                ("Ember", ticks[2], 279.8),
            ] {
                let center = tick.rect.center().x;
                assert!(
                    (center - at).abs() <= 0.5 + 1e-3,
                    "{stop}'s tick is at {center}"
                );
                if stop != "Desert" {
                    assert!(
                        (middle(name(&shapes, stop)).x - center).abs() < 0.01,
                        "{stop} is not centered on its tick"
                    );
                }
            }
            // The Brightness row names its ends, in dim, 18 under its
            // center line, and has no ticks.
            for (stop, flush) in [("Dim", 24.0), ("Full", 396.0)] {
                let text = name(&shapes, stop);
                assert_eq!(ink(text), Palette::DARK.dim);
                assert!((middle(text).y - (BRIGHTNESS_ROW.y + 18.0)).abs() < 0.01);
                let edge = if flush == 24.0 {
                    text.pos.x
                } else {
                    text.pos.x + text.galley.size().x
                };
                assert!((edge - flush).abs() < 0.01, "{stop} is flush at {edge}");
            }
            assert!(ticks_under(&shapes, BRIGHTNESS_ROW).is_empty());
            // And the old end names are gone.
            assert!(
                texts(&shapes)
                    .iter()
                    .all(|text| text.galley.text() != "Day")
            );
        }

        /// What `row` reads at the right of its label: the semibold 14 text
        /// on the label's line.
        fn readout(shapes: &[Shape], row: Row) -> String {
            let all = texts(shapes);
            let label = all
                .iter()
                .find(|text| text.galley.text() == row.label)
                .unwrap_or_else(|| panic!("no label {}", row.label));
            let found: Vec<String> = all
                .iter()
                .filter(|text| text.galley.job.sections[0].format.font_id == theme::semibold(14.0))
                .filter(|text| (middle(text).y - middle(label).y).abs() < 1.0)
                .map(|text| {
                    let right = text.pos.x + text.galley.size().x;
                    assert!((right - 396.0).abs() < 0.01, "a readout ends at {right}");
                    text.galley.text().to_owned()
                })
                .collect();
            assert_eq!(found.len(), 1, "{}'s readouts: {found:?}", row.label);
            found.into_iter().next().expect("one readout")
        }

        #[test]
        fn the_readout_shows_the_kelvin_on_every_stop_and_beside_it() {
            let desert = shared::DESERT_KELVIN;
            // Desert's readout is its kelvin, as Daylight's, Incandescent's
            // and Ember's are; only pure red's is its name.
            for (kelvin, want) in [
                (6500, "6500 K".to_owned()),
                (2700, "2700 K".to_owned()),
                (desert, format!("{desert} K")),
                (desert - 10, format!("{} K", desert - 10)),
                (desert + 10, format!("{} K", desert + 10)),
                (1200, "1200 K".to_owned()),
                (850, "Pure red".to_owned()),
            ] {
                let shapes = Window::new(night(kelvin, 100)).at_rest();
                assert_eq!(readout(&shapes, TEMPERATURE_ROW), want, "{kelvin} K");
            }
        }

        #[test]
        fn desert_is_a_tick_on_a_whole_pixel_with_its_name_midway_between_its_neighbors() {
            let shapes = Window::new(night(1200, 100)).at_rest();
            let ticks = ticks_under(&shapes, TEMPERATURE_ROW);
            assert_eq!(ticks.len(), 3);
            // The middle tick is Desert's, at x 208.7, and like the others it
            // sits on a whole device pixel (the test window is at 1x).
            let tick = ticks[1].rect;
            assert!(
                (tick.center().x - 208.7).abs() <= 0.5 + 1e-3,
                "Desert's tick is at {}",
                tick.center().x
            );
            assert_eq!(tick.min.x, tick.min.x.round());
            // Its name is the middle one of the five under the strip, in
            // the same font.
            let desert = name(&shapes, "Desert");
            assert_eq!(
                desert.galley.job.sections[0].format.font_id,
                theme::regular(11.5)
            );
            // The owner chose its place on 2026-10-23: midway between
            // Incandescent's name and Ember's, so the two gaps are equal to
            // the nearest whole point, not centered on the tick. It is the
            // only name that is off its tick's center.
            let spans = name_spans(&shapes);
            let (_, _, incandescent_right) = spans[1];
            let (_, desert_left, desert_right) = spans[2];
            let (_, ember_left, _) = spans[3];
            let before = desert_left - incandescent_right;
            let after = ember_left - desert_right;
            assert!(
                (before - after).abs() <= 1.0 + 1e-3,
                "the gaps are {before} and {after}"
            );
            // It still covers its tick, so it reads as that tick's, and
            // lies to the right of it, away from the nearer neighbor.
            assert!(
                desert_left < tick.center().x && tick.center().x < desert_right,
                "Desert spans {desert_left} to {desert_right}, its tick is at {}",
                tick.center().x
            );
            assert!(middle(desert).x > tick.center().x + 1.0);
            // Incandescent and Ember are centered on theirs.
            for (stop, tick) in [("Incandescent", ticks[0]), ("Ember", ticks[2])] {
                let center = middle(name(&shapes, stop)).x;
                assert!((center - tick.rect.center().x).abs() < 0.01, "{stop}");
            }
        }

        /// The five names under the Temperature track, from the left, as
        /// the x spans they cover.
        fn name_spans(shapes: &[Shape]) -> Vec<(&'static str, f32, f32)> {
            shared::STOPS
                .iter()
                .map(|stop| {
                    let text = name(shapes, stop.name);
                    (stop.name, text.pos.x, text.pos.x + text.galley.size().x)
                })
                .collect()
        }

        #[test]
        fn no_two_names_under_the_track_are_closer_than_8_points() {
            // The owner asked for "Desert" under the track although it
            // leaves less room between names than the others do: about 26
            // points from Incandescent and from Ember, where the other pairs
            // are 32.7 and 53.2 apart. What holds is a floor.
            let shapes = Window::new(night(1200, 100)).at_rest();
            let spans = name_spans(&shapes);
            assert_eq!(spans.len(), 5);
            for pair in spans.windows(2) {
                let gap = pair[1].1 - pair[0].2;
                assert!(
                    gap >= 8.0,
                    "{} and {} are {gap} points apart",
                    pair[0].0,
                    pair[1].0
                );
            }
        }

        /// A knob on a row's center line: the dark ring of radius 7 and the
        /// white of radius 5.5 inside it, each with its place in the paint
        /// order.
        struct Knob<'a> {
            ring: (usize, &'a CircleShape),
            white: (usize, &'a CircleShape),
        }

        /// The row's knob, if it shows one.
        fn knob(shapes: &[Shape], row: Row) -> Option<Knob<'_>> {
            let disc = |radius: f32| {
                let found: Vec<(usize, &CircleShape)> = shapes
                    .iter()
                    .enumerate()
                    .filter_map(|(at, shape)| match shape {
                        Shape::Circle(disc) if disc.radius == radius && disc.center.y == row.y => {
                            Some((at, disc))
                        }
                        _ => None,
                    })
                    .collect();
                assert!(found.len() <= 1, "{} discs of radius {radius}", found.len());
                found.first().copied()
            };
            match (disc(7.0), disc(5.5)) {
                (Some(ring), Some(white)) => Some(Knob { ring, white }),
                (None, None) => None,
                (ring, white) => panic!("half a knob: {ring:?}, {white:?}"),
            }
        }

        /// Checks `row` shows its ringed knob centered on x `at`, over its
        /// strip.
        fn assert_knob_at(shapes: &[Shape], row: Row, at: f32) {
            let p = Palette::DARK;
            let knob = knob(shapes, row).unwrap_or_else(|| panic!("no knob on {}", row.label));
            // A dark ring of 7 with the white inside it at 5.5: 1.5 of ring.
            assert_eq!(knob.ring.1.fill, p.on_accent);
            assert_eq!(knob.white.1.fill, p.knob);
            for disc in [knob.ring.1, knob.white.1] {
                assert!(
                    (disc.center.x - at).abs() < 0.05,
                    "{}: the knob is at {}, not {at}",
                    row.label,
                    disc.center.x
                );
            }
            // The ring lies over the strip, and the white over the ring.
            let (strip_at, _) = strip_at(shapes, row);
            assert!(strip_at < knob.ring.0 && knob.ring.0 < knob.white.0);
        }

        /// The accent hairlines (a 1-point stroke) the slider rows draw, and
        /// what else in them is filled or inked in the accent.
        fn gold(shapes: &[Shape]) -> (Vec<&RectShape>, Vec<&'static str>) {
            let p = Palette::DARK;
            let is_gold = |color: Color32| color == p.accent || color == p.accent_hover;
            let (mut hairlines, mut fills) = (vec![], vec![]);
            for shape in shapes {
                // Both rows, with their strips, knobs, names and labels.
                let bounds = shape.visual_bounding_rect();
                if bounds.min.y < TEMPERATURE_ROW.y - 30.0 || bounds.max.y > BRIGHTNESS_ROW.y + 38.0
                {
                    continue;
                }
                match shape {
                    Shape::Rect(rect) => {
                        if is_gold(rect.fill) {
                            fills.push("a rect");
                        }
                        if rect.stroke.width > 0.0 && is_gold(rect.stroke.color) {
                            hairlines.push(rect);
                        }
                    }
                    Shape::Circle(disc) => {
                        if is_gold(disc.fill) {
                            fills.push("a disc");
                        }
                        if disc.stroke.width > 0.0 && is_gold(disc.stroke.color) {
                            fills.push("a circle's stroke");
                        }
                    }
                    Shape::Mesh(mesh) if mesh.vertices.iter().any(|v| is_gold(v.color)) => {
                        fills.push("a mesh");
                    }
                    Shape::Text(text) if is_gold(ink(text)) => fills.push("a text"),
                    _ => {}
                }
            }
            (hairlines, fills)
        }

        /// The two rows, each with the night that puts its value in the
        /// middle of its strip, where that value sits, and where to rest the
        /// pointer on the row.
        fn rows() -> [(Row, Look, f32, Pos2); 2] {
            [
                // 1200 K sits at x 279.8, a point the pointer is not on.
                (
                    TEMPERATURE_ROW,
                    night(1200, 100),
                    279.8,
                    pos2(210.0, TEMPERATURE_ROW.y),
                ),
                // 60% is the middle of 20 to 100: x 210.
                (
                    BRIGHTNESS_ROW,
                    night(1200, 60),
                    210.0,
                    pos2(150.0, BRIGHTNESS_ROW.y),
                ),
            ]
        }

        #[test]
        fn a_slider_shows_its_knob_only_when_hovered_focused_or_in_hand() {
            for (row, look, value_x, hover_at) in rows() {
                let other = if row.y == TEMPERATURE_ROW.y {
                    BRIGHTNESS_ROW
                } else {
                    TEMPERATURE_ROW
                };
                // At rest, with nothing under the pointer or focused: none.
                let mut window = Window::new(look);
                let shapes = window.at_rest();
                assert!(knob(&shapes, row).is_none(), "{} at rest", row.label);
                assert!(knob(&shapes, other).is_none());

                // Hovered: on the value, not under the pointer.
                let shapes = window.hover(hover_at);
                assert_knob_at(&shapes, row, value_x);
                assert!(knob(&shapes, other).is_none(), "the other row's knob");

                // Moved off again: gone.
                let shapes = window.hover(pos2(210.0, 150.0));
                assert!(knob(&shapes, row).is_none(), "{} unhovered", row.label);

                // Focused: the same knob.
                let mut window = Window::new(look);
                let shapes = window.focus(row);
                assert_knob_at(&shapes, row, value_x);
                assert!(knob(&shapes, other).is_none(), "the other row's knob");

                // In hand: the knob follows the pointer.
                let mut window = Window::new(look);
                let shapes = window.drag(pos2(60.0, row.y), pos2(160.0, row.y));
                assert_knob_at(&shapes, row, 160.0);
                assert!(knob(&shapes, other).is_none(), "the other row's knob");
                let shapes = window.frame(vec![Event::PointerMoved(pos2(300.0, row.y))]);
                assert_knob_at(&shapes, row, 300.0);
            }
        }

        #[test]
        fn focus_rings_the_knob_in_the_accent() {
            for (row, look, value_x, _) in rows() {
                let shapes = Window::new(look).focus(row);
                let (hairlines, _) = gold(&shapes);
                assert_eq!(hairlines.len(), 1, "{}: {hairlines:?}", row.label);
                let ring = hairlines[0];
                // 1 point wide, round an 18-point box: 2 points outside the
                // knob's 14.
                assert_eq!(ring.stroke.width, 1.0);
                assert_eq!(ring.rect.size(), vec2(18.0, 18.0));
                assert!((ring.rect.center().x - value_x).abs() < 0.05);
                assert_eq!(ring.rect.center().y, row.y);
                // Gold is only this ring.
                assert_eq!(ring.fill, Color32::TRANSPARENT);
                assert_eq!(ring.stroke.color, Palette::DARK.accent);
            }
        }

        #[test]
        fn gold_is_only_the_focus_ring_on_either_slider() {
            for (row, look, _, hover_at) in rows() {
                let mut window = Window::new(look);
                let at_rest = window.at_rest();
                let hovered = window.hover(hover_at);
                let focused = Window::new(look).focus(row);
                let in_hand = Window::new(look).drag(pos2(60.0, row.y), pos2(160.0, row.y));
                for (state, shapes, hairlines) in [
                    ("rest", &at_rest, 0),
                    ("hover", &hovered, 0),
                    ("focus", &focused, 1),
                ] {
                    let (rings, fills) = gold(shapes);
                    assert!(fills.is_empty(), "{} at {state}: {fills:?}", row.label);
                    assert_eq!(rings.len(), hairlines, "{} at {state}", row.label);
                }
                // A mouse press focuses the slider too; then the ring is
                // all there is, round the knob in hand.
                let (rings, fills) = gold(&in_hand);
                assert!(fills.is_empty(), "{} in hand: {fills:?}", row.label);
                assert!(rings.len() <= 1, "{} in hand: {rings:?}", row.label);
                for ring in rings {
                    assert!((ring.rect.center().x - 160.0).abs() < 0.05);
                }
            }
        }

        #[test]
        fn the_brightness_strip_shows_the_chosen_light() {
            // The night look's light at full brightness, whatever the
            // brightness is.
            let chosen = night(1200, 100).tint().apply(Color32::WHITE);
            let shapes = Window::new(night(1200, 100)).at_rest();
            let mesh = strip(&shapes, BRIGHTNESS_ROW);
            for vertex in &mesh.vertices {
                assert_eq!(vertex.color, chosen, "x {}", vertex.pos.x);
            }
            for disc in ends(&shapes, BRIGHTNESS_ROW) {
                assert_eq!(disc.fill, chosen);
            }
            // At 60% the light is the same and the strip is veiled past the
            // middle of its range, x 210.
            let shapes = Window::new(night(1200, 60)).at_rest();
            let mesh = strip(&shapes, BRIGHTNESS_ROW);
            let found = edge(mesh, |_| chosen).expect("a veiled part");
            assert!((found - 210.0).abs() < 0.01, "the edge is at {found}");
            assert_lit_then_veiled(mesh, found, |_| chosen);
            let discs = ends(&shapes, BRIGHTNESS_ROW);
            assert_eq!(discs[0].fill, chosen);
            assert_eq!(discs[1].fill, veil(chosen));
        }

        #[test]
        fn the_brightness_light_is_drawn_through_the_live_tint() {
            let look = night(1200, 60);
            let live = look.tint();
            let shapes = Window::new(look).tinted(live).at_rest();
            let chosen = night(1200, 100).tint().through(live).apply(Color32::WHITE);
            let untinted = night(1200, 100).tint().apply(Color32::WHITE);
            assert_ne!(chosen, untinted, "the tint must matter to this test");
            let mesh = strip(&shapes, BRIGHTNESS_ROW);
            let found = edge(mesh, |_| chosen).expect("a veiled part");
            assert!((found - 210.0).abs() < 0.01);
            assert_lit_then_veiled(mesh, found, |_| chosen);
            // The screen can show no more than its own tint, so the chosen
            // light, at full brightness, shows as the screen's own color.
            let seen = live.apply(chosen);
            let wanted = look.tint().apply(Color32::WHITE);
            for (a, b) in [
                (seen.r(), wanted.r()),
                (seen.g(), wanted.g()),
                (seen.b(), wanted.b()),
            ] {
                assert!(a.abs_diff(b) <= 1, "{seen:?} for {wanted:?}");
            }
        }

        #[test]
        fn the_brightness_light_follows_a_drag_of_the_temperature() {
            let at = color::redness_of(3000);
            assert_eq!(color::kelvin_at_redness(at), 3000, "3000 K is on the scale");
            let mut window = Window::new(night(1200, 100));
            let before = strip(&window.at_rest(), BRIGHTNESS_ROW).vertices[0].color;
            assert_eq!(before, night(1200, 100).tint().apply(Color32::WHITE));
            // The same frame as the drag moves the Temperature.
            let y = TEMPERATURE_ROW.y;
            let shapes = window.drag(pos2(60.0, y), pos2(x_of(at), y));
            let temperature = strip(&shapes, TEMPERATURE_ROW);
            let found = edge(temperature, scale).expect("a veiled part");
            assert!((found - x_of(at)).abs() < 0.05, "the edge is at {found}");
            let wanted = night(3000, 100).tint().apply(Color32::WHITE);
            assert_ne!(wanted, before);
            for vertex in &strip(&shapes, BRIGHTNESS_ROW).vertices {
                assert_eq!(vertex.color, wanted, "x {}", vertex.pos.x);
            }
        }

        /// The Temperature slider's detents through the real window: pointer
        /// and key events into `ui::draw`, a frame at a time.
        mod detents {
            use super::*;
            use crate::app::{Haptics, Played, played};
            use crate::ui::testing::{button, key_down, labels, told};
            use egui::{FullOutput, Key, Modifiers, PointerButton};

            /// Where the pointer is on the Temperature row for `kelvin`, `by`
            /// points to the right of it, toward red.
            fn at(kelvin: u16, by: f32) -> Pos2 {
                pos2(x_of(color::redness_of(kelvin)) + by, TEMPERATURE_ROW.y)
            }

            /// The kelvin the Temperature slider would read at the pointer's
            /// x with no detent to snap to.
            fn kelvin_under(pointer: Pos2) -> u16 {
                color::kelvin_at_redness(position(pointer.x))
            }

            /// The secondary button going down or up at `at`, as a click
            /// with two fingers does.
            fn secondary(at: Pos2, pressed: bool) -> Event {
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Secondary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }
            }

            /// A hand on the window: the pointer and the keys, a frame for
            /// each move, `gap` seconds after the last frame.
            struct Hand {
                window: Window,
                /// The time of the last frame, in seconds.
                time: f64,
                /// How long a frame comes after the last.
                gap: f64,
                pointer: Pos2,
                /// Whether the app takes each frame's actions after it, as
                /// the real window does. Without that, the actions stay in
                /// `app.actions`, and the slider's value stays as it was.
                applies: bool,
                /// The ticks among the actions that the app has taken.
                taken: usize,
            }

            impl Hand {
                /// A window with the night `night`, whose app takes each
                /// frame's actions, so that a slider follows its own moves.
                fn new(night: Look) -> Self {
                    Self {
                        window: Window::new(night),
                        time: 1.0,
                        gap: 1.0 / 60.0,
                        pointer: Pos2::ZERO,
                        applies: true,
                        taken: 0,
                    }
                }

                /// A window whose app leaves the actions in `app.actions`.
                fn keeping(night: Look) -> Self {
                    Self {
                        applies: false,
                        ..Self::new(night)
                    }
                }

                /// One frame with `events`, `gap` seconds on.
                fn output(&mut self, events: Vec<Event>) -> FullOutput {
                    self.time += self.gap;
                    let app = &mut self.window.app;
                    let output = testing::output_at(&self.window.ctx, self.time, events, |ui| {
                        shared::draw(app, ui)
                    });
                    if self.applies {
                        self.settle();
                    }
                    output
                }

                fn frame(&mut self, events: Vec<Event>) -> Vec<Shape> {
                    testing::shapes(&mut self.output(events))
                }

                /// The app takes the actions that wait, and the ticks among
                /// them are counted.
                fn settle(&mut self) {
                    self.taken += self.waiting();
                    self.window.app.apply_actions(None);
                }

                /// The ticks among the actions that wait in the app.
                fn waiting(&self) -> usize {
                    let actions = &self.window.app.actions;
                    actions.iter().filter(|a| **a == Action::Detent).count()
                }

                /// Every tick the frames so far asked the app for.
                fn ticks(&self) -> usize {
                    self.taken + self.waiting()
                }

                /// The looks that the frames so far set, in the order they
                /// were set, among the actions that wait in the app.
                fn looks(&self) -> Vec<Look> {
                    self.window
                        .app
                        .actions
                        .iter()
                        .filter_map(|action| match action {
                            Action::SetNight { look, .. } => Some(*look),
                            _ => None,
                        })
                        .collect()
                }

                /// The looks among [`Hand::looks`] that differ from the
                /// app's night: the moves, leaving out the preview that a
                /// press asks for from its own frame, of the look it found.
                fn moves(&self) -> Vec<Look> {
                    let night = self.window.app.settings.night;
                    self.looks()
                        .into_iter()
                        .filter(|look| *look != night)
                        .collect()
                }

                /// How many times the actions that wait in the app end a
                /// preview.
                fn let_go(&self) -> usize {
                    let actions = &self.window.app.actions;
                    actions.iter().filter(|a| **a == Action::EndPreview).count()
                }

                fn kelvin(&self) -> u16 {
                    self.window.app.settings.night.kelvin
                }

                fn brightness(&self) -> u8 {
                    self.window.app.settings.night.brightness
                }

                fn move_to(&mut self, to: Pos2) {
                    self.pointer = to;
                    self.frame(vec![Event::PointerMoved(to)]);
                }

                fn press(&mut self) {
                    let at = self.pointer;
                    self.frame(vec![button(at, true)]);
                }

                fn release(&mut self) {
                    let at = self.pointer;
                    self.frame(vec![button(at, false)]);
                }

                /// The pointer goes to `at`, presses and lets go.
                fn click(&mut self, at: Pos2) {
                    self.move_to(at);
                    self.press();
                    self.release();
                }

                /// The pointer moves along its row to x `to`, `step` points
                /// a frame.
                fn slide(&mut self, to: f32, step: f32) {
                    while self.pointer.x != to {
                        let left = to - self.pointer.x;
                        let x = if left.abs() <= step {
                            to
                        } else {
                            self.pointer.x + step.copysign(left)
                        };
                        self.move_to(pos2(x, self.pointer.y));
                    }
                }

                /// The row has the keyboard.
                fn focus(&mut self, row: Row) {
                    self.window
                        .ctx
                        .memory_mut(|memory| memory.request_focus(Id::new(row.label)));
                    self.frame(vec![]);
                }

                fn key(&mut self, key: Key, modifiers: Modifiers) {
                    self.frame(vec![key_down(key, modifiers)]);
                }
            }

            /// The stop the thumper sits on, from the table's constant.
            const THUMPER_STOP: u16 = shared::DESERT_KELVIN;

            /// The buttons through the real window: the pointer's primary
            /// button going down on the "Turn red automatically" switch or
            /// Red Now asks the trackpad for a press, in that frame, and the click comes as it comes up (the
            /// owner's choice of 2026-10-23). The keys, the screen reader and
            /// a secondary button do not press, and neither do Close (the
            /// owner's choice of the same day) and the Schedule's time
            /// fields.
            ///
            /// A button's place comes from the name the screen reader gives
            /// it, so that nothing here copies the view's layout.
            mod buttons {
                use super::*;
                use egui::accesskit;

                /// The three buttons that click: the name the screen reader
                /// gives each, its id, and the action a click leaves, the
                /// app being as `hand` has it.
                fn buttons(hand: &Hand) -> [(&'static str, &'static str, Action); 3] {
                    let app = &hand.window.app;
                    [
                        (
                            "Turn red automatically",
                            "schedule-on",
                            Action::SetSchedule(!app.settings.schedule_on),
                        ),
                        ("Red Now", "red-now", Action::RedNow(!app.red_now)),
                        ("Close", "close", Action::HideWindow),
                    ]
                }

                /// Where the widget that the screen reader names `name` is,
                /// or one whose name is `name`, a colon and its value.
                fn rect_of(hand: &mut Hand, name: &str) -> Rect {
                    hand.window.ctx.enable_accesskit();
                    let mut output = hand.output(vec![]);
                    let valued = format!("{name}: ");
                    let found: Vec<Rect> = told(&mut output)
                        .into_iter()
                        .filter(|(label, _)| label == name || label.starts_with(&valued))
                        .filter_map(|(_, bounds)| bounds)
                        .collect();
                    assert_eq!(found.len(), 1, "{name}: {found:?}");
                    found[0]
                }

                /// The actions that wait in the app, which are forgotten.
                fn taken(hand: &mut Hand) -> Vec<Action> {
                    std::mem::take(&mut hand.window.app.actions)
                }

                fn presses(actions: &[Action]) -> usize {
                    actions.iter().filter(|a| **a == Action::Press).count()
                }

                /// A click by the primary button at `at`: the actions that
                /// the press frame leaves, then those of the release frame.
                fn clicking(hand: &mut Hand, at: Pos2) -> (Vec<Action>, Vec<Action>) {
                    hand.move_to(at);
                    taken(hand);
                    hand.press();
                    let pressed = taken(hand);
                    hand.release();
                    (pressed, taken(hand))
                }

                #[test]
                fn a_click_by_the_pointer_on_the_switch_or_red_now_presses_as_it_goes_down() {
                    // The first two of the buttons: Close is below.
                    for index in 0..2 {
                        let mut hand = Hand::keeping(night(1200, 100));
                        let (label, _, action) = buttons(&hand)[index].clone();
                        let at = rect_of(&mut hand, label).center();
                        let (pressed, released) = clicking(&mut hand, at);
                        // The press comes as the button goes down, and the
                        // action as egui reports the click, as it comes up.
                        assert_eq!(pressed, vec![Action::Press], "{label}, the press frame");
                        assert_eq!(released, vec![action], "{label}, the release frame");
                    }
                }

                /// A click whose press and release come in one frame, as a
                /// quick one may when frames are slow: one press, and the
                /// action.
                #[test]
                fn a_click_within_one_frame_on_the_switch_or_red_now_presses_once() {
                    for index in 0..2 {
                        let mut hand = Hand::keeping(night(1200, 100));
                        let (label, _, action) = buttons(&hand)[index].clone();
                        let at = rect_of(&mut hand, label).center();
                        hand.move_to(at);
                        taken(&mut hand);
                        hand.frame(vec![button(at, true), button(at, false)]);
                        assert_eq!(taken(&mut hand), vec![Action::Press, action], "{label}");
                    }
                }

                /// A press dragged off the button before it comes up: the
                /// press, and no click. The owner accepted it (2026-10-23).
                #[test]
                fn a_press_dragged_off_the_switch_or_red_now_presses_with_no_action() {
                    for index in 0..2 {
                        let mut hand = Hand::keeping(night(1200, 100));
                        let (label, _, _) = buttons(&hand)[index].clone();
                        let rect = rect_of(&mut hand, label);
                        hand.move_to(rect.center());
                        taken(&mut hand);
                        hand.press();
                        assert_eq!(taken(&mut hand), vec![Action::Press], "{label}");
                        // Off the button, to a point clear of every control.
                        hand.move_to(pos2(rect.center().x, rect.max.y + 60.0));
                        hand.release();
                        assert_eq!(taken(&mut hand), vec![], "{label}, let go off it");
                    }
                }

                /// The owner dropped Close's press after trying it.
                #[test]
                fn a_click_by_the_pointer_on_close_hides_the_window_and_leaves_no_press() {
                    let mut hand = Hand::keeping(night(1200, 100));
                    let at = rect_of(&mut hand, "Close").center();
                    let (pressed, released) = clicking(&mut hand, at);
                    assert_eq!(pressed, vec![], "the press frame");
                    assert_eq!(released, vec![Action::HideWindow]);
                }

                #[test]
                fn each_click_by_the_pointer_plays_one_press_on_the_trackpad() {
                    let mut hand = Hand::new(night(1200, 100));
                    hand.window.app.haptics = Haptics::counting();
                    // Close plays no press, and would quit the app, which has
                    // no tray here.
                    for (index, label) in ["Turn red automatically", "Red Now"].iter().enumerate() {
                        let at = rect_of(&mut hand, label).center();
                        hand.click(at);
                        assert_eq!(
                            played(),
                            Played {
                                ticks: 0,
                                thumps: 0,
                                presses: index + 1
                            },
                            "after {label}"
                        );
                    }
                }

                /// A hand that scrubs the Temperature from Daylight to pure red
                /// and back, across 2700 K, Desert's 1760 K and 1200 K, and
                /// clicks the switch and Red Now.
                fn scrubs_and_clicks(hand: &mut Hand) {
                    hand.move_to(pos2(LEFT_END, TEMPERATURE_ROW.y));
                    hand.press();
                    hand.slide(LEFT_END + TRAVEL, 2.0);
                    hand.slide(LEFT_END, 2.0);
                    hand.release();
                    for label in ["Turn red automatically", "Red Now"] {
                        let at = rect_of(hand, label).center();
                        hand.click(at);
                    }
                }

                #[test]
                fn a_window_on_a_test_app_plays_nothing_and_a_counting_one_plays_the_same_drive() {
                    // A test app's sinks are silent: no thread's count moves,
                    // however the hand scrubs and clicks, and the window acts
                    // all the same: the ticks are asked of the app, the
                    // detents snap the value, the buttons act.
                    let _ = Haptics::counting();
                    let mut silent = Hand::new(night(6500, 100));
                    scrubs_and_clicks(&mut silent);
                    assert_eq!(played(), Played::default());
                    assert_eq!(silent.ticks(), 7, "four going and three coming back");
                    assert!(silent.window.app.red_now, "Red Now acted");
                    assert!(!silent.window.app.settings.schedule_on, "the switch acted");

                    // The contrast: sinks that count see the same drive play
                    // the ticks of the detents and a press for each button.
                    let mut counting = Hand::new(night(6500, 100));
                    counting.window.app.haptics = Haptics::counting();
                    scrubs_and_clicks(&mut counting);
                    assert_eq!(
                        played(),
                        Played {
                            ticks: 7,
                            thumps: 0,
                            presses: 2
                        }
                    );
                    assert_eq!(counting.ticks(), silent.ticks());
                    assert_eq!(counting.kelvin(), silent.kelvin());
                    assert_eq!(counting.window.app.red_now, silent.window.app.red_now);
                }

                /// Silent sinks only leave the output out: a detent that one
                /// move of the pointer catches still snaps the value, and one
                /// that a move passes clean over still asks for its tick and
                /// lets the value follow the pointer.
                #[test]
                fn a_detent_met_in_one_move_acts_as_it_does_with_the_trackpad_silent() {
                    for counting in [false, true] {
                        let desert = shared::DESERT_KELVIN;
                        let at_desert = at(desert, 2.0);
                        let past_incandescent = at(2700, 8.0);
                        // The values the pointer alone would give.
                        assert_ne!(kelvin_under(at_desert), desert);
                        assert_ne!(kelvin_under(past_incandescent), 2700);

                        // Caught: from far left, in one move, into Desert's
                        // catch.
                        let mut hand = Hand::new(night(6500, 100));
                        if counting {
                            hand.window.app.haptics = Haptics::counting();
                        } else {
                            let _ = Haptics::counting();
                        }
                        hand.move_to(pos2(LEFT_END, TEMPERATURE_ROW.y));
                        hand.press();
                        hand.move_to(at_desert);
                        let what = format!("counting: {counting}, caught");
                        assert_eq!(hand.kelvin(), desert, "{what}");
                        assert_eq!(hand.ticks(), 1, "{what}");
                        let ticks = usize::from(counting);
                        let played_here = Played {
                            ticks,
                            thumps: 0,
                            presses: 0,
                        };
                        assert_eq!(played(), played_here, "{what}");

                        // Passed: from far left, in one move, clean over the
                        // catch of 2700 K and no farther than Desert's.
                        let mut hand = Hand::new(night(6500, 100));
                        if counting {
                            hand.window.app.haptics = Haptics::counting();
                        } else {
                            let _ = Haptics::counting();
                        }
                        hand.move_to(pos2(LEFT_END, TEMPERATURE_ROW.y));
                        hand.press();
                        hand.move_to(past_incandescent);
                        let what = format!("counting: {counting}, passed");
                        assert_eq!(hand.kelvin(), kelvin_under(past_incandescent), "{what}");
                        assert_eq!(hand.ticks(), 1, "{what}");
                        assert_eq!(played(), played_here, "{what}");
                    }
                }

                #[test]
                fn space_and_return_on_a_focused_button_leave_its_action_and_no_press() {
                    for key in [Key::Space, Key::Enter] {
                        for index in 0..3 {
                            let mut hand = Hand::keeping(night(1200, 100));
                            let (label, id, action) = buttons(&hand)[index].clone();
                            hand.window
                                .ctx
                                .memory_mut(|memory| memory.request_focus(Id::new(id)));
                            hand.frame(vec![]);
                            assert_eq!(taken(&mut hand), vec![], "{label} focused");
                            hand.key(key, Modifiers::NONE);
                            assert_eq!(taken(&mut hand), vec![action], "{label}, {key:?}");
                        }
                    }
                }

                #[test]
                fn return_with_nothing_focused_hides_the_window_and_leaves_no_press() {
                    let mut hand = Hand::keeping(night(1200, 100));
                    hand.key(Key::Enter, Modifiers::NONE);
                    assert_eq!(taken(&mut hand), vec![Action::HideWindow]);
                }

                #[test]
                fn a_screen_reader_click_leaves_the_buttons_action_and_no_press() {
                    for index in 0..3 {
                        let mut hand = Hand::keeping(night(1200, 100));
                        let (label, id, action) = buttons(&hand)[index].clone();
                        hand.frame(vec![]);
                        let request = accesskit::ActionRequest {
                            action: accesskit::Action::Click,
                            target_tree: accesskit::TreeId::ROOT,
                            target_node: Id::new(id).accesskit_id(),
                            data: None,
                        };
                        hand.frame(vec![Event::AccessKitActionRequest(request)]);
                        assert_eq!(taken(&mut hand), vec![action], "{label}");
                    }
                }

                #[test]
                fn a_secondary_click_on_a_button_leaves_everything_alone() {
                    for index in 0..3 {
                        let mut hand = Hand::keeping(night(1200, 100));
                        let (label, _, _) = buttons(&hand)[index].clone();
                        let at = rect_of(&mut hand, label).center();
                        hand.move_to(at);
                        taken(&mut hand);
                        hand.frame(vec![secondary(at, true)]);
                        hand.frame(vec![secondary(at, false)]);
                        assert_eq!(taken(&mut hand), vec![], "{label}");
                    }
                }

                #[test]
                fn clicks_on_the_time_fields_and_their_arrows_leave_no_press() {
                    // Each field, its name, and the action its arrows leave.
                    type Steps = fn(&Action) -> bool;
                    let fields: [(&str, &str, Steps); 3] = [
                        ("From", "from", |a| matches!(a, Action::SetStart(_))),
                        ("To", "to", |a| matches!(a, Action::SetEnd(_))),
                        ("Fade minutes", "fade", |a| matches!(a, Action::SetFade(_))),
                    ];
                    for (name, id, steps) in fields {
                        // The first part, and the last part, of the field.
                        let mut hand = Hand::keeping(night(1200, 100));
                        let field = rect_of(&mut hand, name);
                        for at in [
                            pos2(field.min.x + 14.0, field.center().y),
                            pos2(field.max.x - 6.0, field.center().y),
                        ] {
                            let (pressed, released) = clicking(&mut hand, at);
                            assert_eq!(pressed, vec![], "{name} at {at:?}");
                            assert_eq!(released, vec![], "{name} at {at:?}");
                            let focused = hand.window.ctx.memory(|m| m.has_focus(Id::new(id)));
                            assert!(focused, "the click at {at:?} reached {name}");
                        }
                        // The arrows still step, with no press.
                        for arrow in ["up", "down"] {
                            let mut hand = Hand::keeping(night(1200, 100));
                            let at = rect_of(&mut hand, &format!("{name} {arrow}")).center();
                            let (pressed, released) = clicking(&mut hand, at);
                            assert_eq!(pressed, vec![], "{name} {arrow}");
                            assert_eq!(presses(&released), 0, "{name} {arrow}: {released:?}");
                            assert_eq!(
                                released.iter().filter(|a| steps(a)).count(),
                                1,
                                "{name} {arrow} steps once: {released:?}"
                            );
                        }
                    }
                }
            }

            /// The Temperature slider's thumper through the real window:
            /// pointer, key and focus events into `ui::draw` at the times of
            /// the frames, and the thumps counted where the app plays them.
            /// The clock's own beats are tested in `ui::thumper`.
            mod thumper {
                use super::*;
                use crate::ui::thumper::{HOLD_DELAY, PAIR_GAP, PAIR_PERIOD};
                use egui::ViewportId;
                use std::time::Duration;

                // The beats of a hold, in seconds after it began, from the
                // clock's constants.
                const FIRST: f64 = HOLD_DELAY;
                const SECOND: f64 = HOLD_DELAY + PAIR_GAP;
                const THIRD: f64 = HOLD_DELAY + PAIR_PERIOD;
                const FOURTH: f64 = HOLD_DELAY + PAIR_PERIOD + PAIR_GAP;
                /// A frame this far ahead of a beat is clear of it.
                const SHY: f64 = 0.01;

                /// The thumps the app has played since its sinks were made.
                fn thumps() -> usize {
                    played().thumps
                }

                impl Hand {
                    /// A window whose app counts the thumps it plays.
                    fn thumping(night: Look) -> Self {
                        let mut hand = Self::new(night);
                        hand.window.app.haptics = Haptics::counting();
                        hand
                    }

                    /// One frame with `events` at `time` seconds, whatever
                    /// `gap` says, with the window focused or not.
                    fn frame_at(
                        &mut self,
                        time: f64,
                        focused: bool,
                        events: Vec<Event>,
                    ) -> FullOutput {
                        assert!(
                            time > self.time,
                            "a frame at {time} s after one at {} s",
                            self.time
                        );
                        self.time = time;
                        let (ctx, app) = (&self.window.ctx, &mut self.window.app);
                        let output = if focused {
                            testing::output_at(ctx, time, events, |ui| shared::draw(app, ui))
                        } else {
                            testing::output_unfocused_at(ctx, time, events, |ui| {
                                shared::draw(app, ui)
                            })
                        };
                        self.settle();
                        output
                    }

                    /// Frames with no events, `gap` apart and the last one at
                    /// `time`, which is returned.
                    fn run_until(&mut self, time: f64, focused: bool) -> FullOutput {
                        while self.time + self.gap < time {
                            self.frame_at(self.time + self.gap, focused, vec![]);
                        }
                        self.frame_at(time, focused, vec![])
                    }

                    /// [`Self::run_until`] with the window focused.
                    fn wait_until(&mut self, time: f64) -> FullOutput {
                        self.run_until(time, true)
                    }

                    /// The pointer goes to `at` and presses. The time of the
                    /// press frame, where a hold on `at` begins.
                    fn press_at(&mut self, at: Pos2) -> f64 {
                        self.move_to(at);
                        self.press();
                        self.time
                    }

                    /// The pointer moves rightward, a point a frame, until the
                    /// detent has caught the value on Desert. The time of
                    /// that frame.
                    fn catch_desert(&mut self) -> f64 {
                        assert_ne!(
                            self.kelvin(),
                            shared::DESERT_KELVIN,
                            "caught before the drag"
                        );
                        for _ in 0..400 {
                            self.move_to(pos2(self.pointer.x + 1.0, self.pointer.y));
                            if self.kelvin() == shared::DESERT_KELVIN {
                                return self.time;
                            }
                        }
                        panic!("the drag never caught Desert");
                    }

                    /// Two frames with no events: the second is the one that
                    /// egui's own repaint after input has left alone.
                    fn settled(&mut self) -> FullOutput {
                        self.output(vec![]);
                        self.output(vec![])
                    }
                }

                /// The four beats of a hold that began at `began`, from the
                /// `already` thumps played: a frame `SHY` before each is clear
                /// of it and one on it plays one.
                fn assert_beats(hand: &mut Hand, began: f64, already: usize) {
                    for (index, beat) in [FIRST, SECOND, THIRD, FOURTH].into_iter().enumerate() {
                        hand.wait_until(began + beat - SHY);
                        assert_eq!(
                            thumps(),
                            already + index,
                            "{SHY} s before the beat at {beat} s of the hold begun at {began} s"
                        );
                        hand.wait_until(began + beat);
                        assert_eq!(
                            thumps(),
                            already + index + 1,
                            "on the beat at {beat} s of the hold begun at {began} s"
                        );
                    }
                }

                /// What `output` asks of the window's next frame: how long to
                /// wait, or `Duration::MAX` for no frame at all.
                fn repaint_delay(output: &FullOutput) -> Duration {
                    output.viewport_output[&ViewportId::ROOT].repaint_delay
                }

                /// `output` asks for a frame `wanted` seconds on, to the
                /// millisecond: a `Duration` made from an f32 and one from an
                /// f64 round differently.
                fn assert_asks_in(output: &FullOutput, wanted: f64) {
                    let asked = repaint_delay(output);
                    assert!(
                        asked != Duration::MAX && (asked.as_secs_f64() - wanted).abs() < 1e-3,
                        "asked for {asked:?}, wanted {wanted} s"
                    );
                }

                #[test]
                fn a_press_on_desert_beats_in_pairs_counted_from_the_press() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    // The press snaps the knob and ticks in its own frame,
                    // long before the first thump comes.
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (shared::DESERT_KELVIN, 1, 0)
                    );
                    // Frames each 1/60 s.
                    hand.wait_until(pressed + FIRST - 2.0 * SHY);
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (shared::DESERT_KELVIN, 1, 0)
                    );
                    assert_beats(&mut hand, pressed, 0);
                    assert_eq!(hand.ticks(), 1);
                }

                #[test]
                fn a_still_press_snaps_at_once_and_thumps_in_the_one_frame_that_comes() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    // The press snaps in its own frame. egui gives a mouse
                    // press no frame at 0.8 s, when it turns into a drag: with
                    // a perfectly still finger the next frame is the first
                    // beat's, and it thumps with no second tick.
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (shared::DESERT_KELVIN, 1, 0)
                    );
                    hand.frame_at(pressed + FIRST, true, vec![]);
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (shared::DESERT_KELVIN, 1, 1)
                    );
                }

                #[test]
                fn a_press_on_the_knob_at_desert_beats_from_the_press_without_a_tick() {
                    let mut hand = Hand::thumping(night(shared::DESERT_KELVIN, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    assert_beats(&mut hand, pressed, 0);
                    assert_eq!((hand.kelvin(), hand.ticks()), (shared::DESERT_KELVIN, 0));
                }

                #[test]
                fn a_release_ends_the_hold_wherever_it_comes() {
                    // Well before the first beat, a moment before it, then
                    // between the thumps of the first pair and after them.
                    for (released_after, played) in [
                        (0.5, 0),
                        (FIRST - 2.0 * SHY, 0),
                        (FIRST + 0.05, 1),
                        (SECOND + 0.1, 2),
                    ] {
                        let mut hand = Hand::thumping(night(1200, 100));
                        let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                        hand.wait_until(pressed + released_after);
                        assert_eq!(thumps(), played, "{released_after} s into the hold");
                        hand.release();
                        // Frames on past every beat that would have come.
                        hand.wait_until(pressed + FOURTH + 1.0);
                        assert_eq!(thumps(), played, "released {released_after} s into it");
                    }
                }

                #[test]
                fn a_press_after_a_release_waits_the_whole_delay_from_itself() {
                    // Let go while armed (a click, which snaps the knob onto
                    // Desert), or after the first thump, and press again a
                    // moment later: the beats of the first hold, still to come,
                    // never play, and the second's come from its own press.
                    for (released_after, played) in [(0.3, 0), (FIRST + 0.05, 1)] {
                        let mut hand = Hand::thumping(night(1200, 100));
                        let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                        hand.wait_until(pressed + released_after);
                        hand.release();
                        hand.wait_until(hand.time + 0.1);
                        let again = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                        assert_eq!(thumps(), played, "released {released_after} s in");
                        assert_beats(&mut hand, again, played);
                        assert_eq!(hand.kelvin(), shared::DESERT_KELVIN);
                    }
                }

                #[test]
                fn a_drag_off_desert_ends_the_hold_and_a_catch_begins_another() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    hand.wait_until(pressed + 0.5);
                    // 8 points: past the detent's release, 5, and past the 6
                    // that make egui call the press a drag.
                    hand.move_to(at(shared::DESERT_KELVIN, 8.0));
                    assert_ne!(hand.kelvin(), shared::DESERT_KELVIN);
                    hand.wait_until(pressed + FOURTH + 1.0);
                    assert_eq!(thumps(), 0, "after a drag off before the first beat");
                    // Back into the catch, long after the press: the 1.5 s
                    // count from the catch, not from the press.
                    hand.move_to(at(shared::DESERT_KELVIN, 2.0));
                    let caught = hand.time;
                    assert_eq!((hand.kelvin(), thumps()), (shared::DESERT_KELVIN, 0));
                    assert_beats(&mut hand, caught, 0);
                    // And away again, in the middle of the beats.
                    hand.move_to(at(shared::DESERT_KELVIN, 8.0));
                    assert_ne!(hand.kelvin(), shared::DESERT_KELVIN);
                    hand.wait_until(hand.time + 3.0);
                    assert_eq!(thumps(), 4, "after a drag off a hold that had beaten");
                }

                #[test]
                fn a_press_off_desert_never_thumps() {
                    let on_brightness = pos2(x_of(0.5), BRIGHTNESS_ROW.y);
                    for (knob, pointer) in [
                        (1200, at(1300, 0.0)),
                        (1200, at(2000, 0.0)),
                        // The knob at Desert, and the pointer elsewhere.
                        (shared::DESERT_KELVIN, at(1300, 0.0)),
                        // Brightness has no thumper, whatever the knob above.
                        (shared::DESERT_KELVIN, on_brightness),
                        (1200, on_brightness),
                    ] {
                        let mut hand = Hand::thumping(night(knob, 100));
                        let pressed = hand.press_at(pointer);
                        hand.wait_until(pressed + 3.0);
                        assert_eq!(
                            thumps(),
                            0,
                            "pressing {pointer:?} with the knob at {knob} K"
                        );
                    }
                }

                #[test]
                fn the_keys_never_thump() {
                    let option = Modifiers::ALT;
                    for (start, keys) in [
                        // Onto Desert by a step, and by a jump from either side.
                        (
                            shared::DESERT_KELVIN + 30,
                            vec![(Key::ArrowRight, Modifiers::NONE)],
                        ),
                        (2700, vec![(Key::ArrowRight, option)]),
                        (1200, vec![(Key::ArrowLeft, option)]),
                        // Already on it, and off it and back.
                        (shared::DESERT_KELVIN, vec![]),
                        (
                            shared::DESERT_KELVIN,
                            vec![(Key::ArrowRight, option), (Key::ArrowLeft, option)],
                        ),
                    ] {
                        let mut hand = Hand::thumping(night(start, 100));
                        hand.focus(TEMPERATURE_ROW);
                        for (key, modifiers) in keys {
                            hand.key(key, modifiers);
                        }
                        assert_eq!(hand.kelvin(), shared::DESERT_KELVIN, "from {start} K");
                        hand.wait_until(hand.time + 3.0);
                        assert_eq!(thumps(), 0, "from {start} K");
                        assert_eq!(hand.kelvin(), shared::DESERT_KELVIN, "from {start} K");
                    }
                }

                #[test]
                fn a_key_that_lands_on_desert_during_a_press_elsewhere_starts_no_hold() {
                    // A step short of Desert, and outside its 3-point catch.
                    let short = shared::DESERT_KELVIN + 70;
                    let mut hand = Hand::thumping(night(short, 100));
                    hand.focus(TEMPERATURE_ROW);
                    let pressed = hand.press_at(at(short, 0.0));
                    hand.key(Key::ArrowRight, Modifiers::NONE);
                    assert_eq!(hand.kelvin(), shared::DESERT_KELVIN);
                    hand.wait_until(pressed + 3.0);
                    assert_eq!(thumps(), 0);
                }

                /// The knob near Desert, a press 4 points from it, inside the
                /// 5-point release but outside the 3-point catch, and → onto
                /// Desert. The press found the value off the stop, so it
                /// never holds it there: not while egui has yet to call it a
                /// drag, nor once it does, at 0.8 s.
                #[test]
                fn a_key_onto_desert_under_a_press_in_its_release_starts_no_hold() {
                    let mut hand = Hand::thumping(night(shared::DESERT_KELVIN + 30, 100));
                    hand.focus(TEMPERATURE_ROW);
                    let pointer = at(shared::DESERT_KELVIN, 4.0);
                    let pressed = hand.press_at(pointer);
                    hand.key(Key::ArrowRight, Modifiers::NONE);
                    assert_eq!(hand.kelvin(), shared::DESERT_KELVIN);
                    hand.wait_until(pressed + 3.0);
                    assert_eq!(thumps(), 0);
                    // The drag took the value from where the press landed.
                    assert_eq!(hand.kelvin(), kelvin_under(pointer));
                }

                /// A drag catches Desert in the frame where → lands the value
                /// there too. The key sets the value, so that frame begins no
                /// hold; the next, with the drag holding the stop, does.
                #[test]
                fn a_key_onto_desert_in_the_frame_a_drag_catches_it_begins_no_hold_there() {
                    let mut hand = Hand::thumping(night(6500, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.move_to(at(2000, 0.0));
                    hand.press();
                    hand.slide(at(shared::DESERT_KELVIN, -4.0).x, 1.0);
                    assert_ne!(hand.kelvin(), shared::DESERT_KELVIN);
                    let caught = at(shared::DESERT_KELVIN, -2.0);
                    hand.pointer = caught;
                    hand.frame(vec![
                        Event::PointerMoved(caught),
                        key_down(Key::ArrowRight, Modifiers::NONE),
                    ]);
                    assert_eq!(hand.kelvin(), shared::DESERT_KELVIN);
                    hand.output(vec![]);
                    let held = hand.time;
                    assert_beats(&mut hand, held, 0);
                }

                /// A secondary button held on Desert, as a click with two
                /// fingers, never thumps: the thumper takes the primary one.
                #[test]
                fn a_secondary_button_held_on_desert_never_thumps() {
                    for knob in [1200, shared::DESERT_KELVIN] {
                        let mut hand = Hand::thumping(night(knob, 100));
                        let pointer = at(shared::DESERT_KELVIN, 0.0);
                        hand.move_to(pointer);
                        hand.frame(vec![secondary(pointer, true)]);
                        let pressed = hand.time;
                        hand.wait_until(pressed + 3.0);
                        assert_eq!(thumps(), 0, "from {knob} K");
                        assert_eq!(hand.kelvin(), knob, "from {knob} K");
                    }
                }

                #[test]
                fn a_key_that_moves_the_value_off_ends_the_hold_and_the_pointer_begins_another() {
                    let mut hand = Hand::thumping(night(6500, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.move_to(at(2000, 0.0));
                    hand.press();
                    let caught = hand.catch_desert();
                    // A second into the hold, a step toward day moves the
                    // value off Desert, which ends the hold.
                    hand.wait_until(caught + 1.0);
                    hand.key(Key::ArrowLeft, Modifiers::NONE);
                    assert_ne!(hand.kelvin(), shared::DESERT_KELVIN);
                    // The pointer is still down in the catch, and holds the
                    // value on Desert again: a new hold, from this frame.
                    hand.output(vec![]);
                    let again = hand.time;
                    assert_eq!(hand.kelvin(), shared::DESERT_KELVIN);
                    // The first hold's first three beats pass without a
                    // thump, though the second hold has yet to reach its own...
                    assert!(caught + THIRD < again + FIRST - SHY);
                    hand.wait_until(caught + THIRD + 0.005);
                    assert_eq!(thumps(), 0, "a beat of the hold that the key ended");
                    // ...which come from the frame that began it.
                    assert_beats(&mut hand, again, 0);
                }

                #[test]
                fn a_key_in_a_press_not_yet_a_drag_ends_the_hold_and_the_next_waits_the_whole_delay()
                 {
                    // The knob away from Desert or on it, and the pointer
                    // pressed on Desert: egui calls the press a drag only at
                    // 0.8 s, and until then the press holds the value on the
                    // stop where it landed.
                    for knob in [1200, shared::DESERT_KELVIN] {
                        let mut hand = Hand::thumping(night(knob, 100));
                        hand.focus(TEMPERATURE_ROW);
                        let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                        hand.wait_until(pressed + 0.3);
                        // A step toward day moves the value, which ends the
                        // hold.
                        hand.key(Key::ArrowLeft, Modifiers::NONE);
                        assert_ne!(hand.kelvin(), knob, "from {knob} K");
                        assert_ne!(hand.kelvin(), shared::DESERT_KELVIN, "from {knob} K");
                        // The press, still undecided, holds it on Desert again
                        // from the next frame: a new hold, from that frame and
                        // not from the press, so the first hold's first beat
                        // passes without a thump.
                        hand.output(vec![]);
                        let again = hand.time;
                        assert!(pressed + FIRST < again + FIRST - SHY);
                        assert_beats(&mut hand, again, 0);
                        assert_eq!(hand.kelvin(), shared::DESERT_KELVIN, "from {knob} K");
                    }
                }

                #[test]
                fn a_drag_that_catches_desert_begins_the_hold_at_the_catch() {
                    let mut hand = Hand::thumping(night(6500, 100));
                    let pressed = hand.press_at(at(2000, 0.0));
                    // The press has outlasted a hold's first beat by the time
                    // the drag sets out: only a hold counted from the press
                    // would thump at the catch.
                    hand.wait_until(pressed + FIRST + 0.5);
                    let caught = hand.catch_desert();
                    assert_eq!(thumps(), 0, "in the frame of the catch");
                    assert_beats(&mut hand, caught, 0);
                }

                /// A pointer down on Desert, armed or already beating, with the
                /// window unfocused for one frame, or for the rest of the time
                /// the hold would have beat. The frames that find the window
                /// unfocused play no thump and ask for no frame; the focus
                /// comes back with the pointer still down, and the hold's
                /// delay is the whole 1.5 s from that frame.
                fn unfocused_then_back(beating: bool, for_long: bool) {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    let played = if beating {
                        hand.wait_until(pressed + FIRST);
                        1
                    } else {
                        // Well before egui calls the press a drag, at 0.8 s.
                        hand.wait_until(pressed + 0.25);
                        0
                    };
                    let case = format!("beating {beating}, away for long {for_long}");
                    let output = hand.frame_at(hand.time + 0.05, false, vec![]);
                    assert_eq!(repaint_delay(&output), Duration::MAX, "{case}");
                    assert_eq!(thumps(), played, "{case}");
                    if for_long {
                        hand.run_until(pressed + FOURTH + 1.0, false);
                        assert_eq!(thumps(), played, "{case}");
                    }
                    // The window is back, the pointer still down on Desert.
                    hand.output(vec![]);
                    let back = hand.time;
                    assert_beats(&mut hand, back, played);
                }

                #[test]
                fn an_unfocused_window_ends_an_armed_hold_and_the_next_waits_the_whole_delay() {
                    // Away for as long as the hold would have beat, so that
                    // egui has called the press a drag by the time it is back.
                    unfocused_then_back(false, true);
                }

                #[test]
                fn an_unfocused_frame_ends_a_beating_hold_and_the_next_waits_the_whole_delay() {
                    unfocused_then_back(true, false);
                    unfocused_then_back(true, true);
                }

                /// One unfocused frame in a press that landed on Desert and
                /// that egui has yet to call a drag (the first 0.8 s) ends the
                /// hold. The next frame finds it again, with the press still
                /// undecided, and the first thump comes 1.5 s after that
                /// frame, not after the press.
                #[test]
                fn a_blink_of_the_focus_in_an_armed_hold_makes_the_delay_start_afresh() {
                    unfocused_then_back(false, false);
                }

                /// A press 2.5 points off the stop snaps onto it in its own
                /// frame, and the hold counts from that frame, with the one
                /// tick that the snap made.
                #[test]
                fn a_snapping_press_is_the_start_of_the_hold_with_one_tick_in_all() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(THUMPER_STOP, 2.5));
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (THUMPER_STOP, 1, 0)
                    );
                    assert_beats(&mut hand, pressed, 0);
                    assert_eq!(hand.ticks(), 1);
                }

                /// The frame that follows the press asks for the first beat
                /// from the press, not from itself.
                #[test]
                fn a_snapping_press_asks_for_a_frame_at_the_first_beat_counted_from_itself() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(THUMPER_STOP, 0.0));
                    hand.wait_until(pressed + 0.25);
                    let output = hand.frame_at(pressed + 0.5, true, vec![]);
                    assert_asks_in(&output, FIRST - 0.5);
                }

                #[test]
                fn a_release_before_the_first_beat_leaves_the_snap_and_no_thump() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(THUMPER_STOP, 0.0));
                    hand.wait_until(pressed + 1.0);
                    hand.release();
                    // Frames on past every beat that would have come.
                    hand.wait_until(pressed + FOURTH + 1.0);
                    assert_eq!(
                        (hand.kelvin(), hand.ticks(), thumps()),
                        (THUMPER_STOP, 1, 0)
                    );
                }

                /// The press lands in the stop's zone and the same frame's
                /// move takes the pointer past the release: the knob is never
                /// shown on the stop, no beat comes, and from the next frame
                /// on nothing asks for one. The press's own frame finds the
                /// stop where the press landed, so it arms a hold for that
                /// one frame, which the next ends.
                #[test]
                fn a_press_on_the_stop_that_its_own_frame_lets_go_never_thumps() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let start = at(THUMPER_STOP, 2.5);
                    hand.move_to(start);
                    hand.pointer = at(THUMPER_STOP, 10.0);
                    let pressed = hand.time + hand.gap;
                    hand.frame(vec![button(start, true), Event::PointerMoved(hand.pointer)]);
                    assert_eq!((hand.kelvin(), hand.ticks(), thumps()), (1200, 0, 0));
                    // The frames that follow ask for nothing, and no beat comes.
                    let output = hand.settled();
                    assert_eq!(repaint_delay(&output), Duration::MAX);
                    hand.wait_until(pressed + FOURTH + 1.0);
                    assert_eq!(thumps(), 0);
                    assert_eq!(hand.ticks(), 0);
                    assert_eq!(hand.kelvin(), kelvin_under(hand.pointer));
                }

                #[test]
                fn a_window_with_no_hold_asks_for_no_frame() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    // At rest.
                    let output = hand.settled();
                    assert_eq!(repaint_delay(&output), Duration::MAX, "at rest");
                    // The pointer over Desert, up.
                    hand.move_to(at(shared::DESERT_KELVIN, 0.0));
                    let output = hand.settled();
                    assert_eq!(repaint_delay(&output), Duration::MAX, "the pointer up");
                    // Down at 1300 K, and on the Brightness slider.
                    for (name, pointer) in [
                        ("down at 1300 K", at(1300, 0.0)),
                        ("down on Brightness", pos2(x_of(0.5), BRIGHTNESS_ROW.y)),
                    ] {
                        let mut hand = Hand::thumping(night(shared::DESERT_KELVIN, 100));
                        hand.press_at(pointer);
                        let output = hand.settled();
                        assert_eq!(repaint_delay(&output), Duration::MAX, "{name}");
                    }
                }

                #[test]
                fn an_armed_hold_asks_for_a_frame_at_the_first_beat() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    let output = hand.settled();
                    assert_asks_in(&output, pressed + FIRST - hand.time);
                    // The same for a knob that was on Desert already.
                    let mut hand = Hand::thumping(night(shared::DESERT_KELVIN, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    let output = hand.settled();
                    assert_asks_in(&output, pressed + FIRST - hand.time);
                }

                #[test]
                fn a_beating_hold_asks_for_a_frame_at_the_next_beat() {
                    let mut hand = Hand::thumping(night(1200, 100));
                    let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                    hand.wait_until(pressed + FIRST);
                    // 20 ms after the first thump, the second is next...
                    let now = pressed + FIRST + 0.02;
                    let output = hand.frame_at(now, true, vec![]);
                    assert_asks_in(&output, pressed + SECOND - now);
                    // ...and 20 ms after the second, the next pair's first.
                    hand.wait_until(pressed + SECOND);
                    let now = pressed + SECOND + 0.02;
                    let output = hand.frame_at(now, true, vec![]);
                    assert_asks_in(&output, pressed + THIRD - now);
                    assert_eq!(thumps(), 2);
                }

                #[test]
                fn a_released_hold_asks_for_no_frame() {
                    for beating in [false, true] {
                        let mut hand = Hand::thumping(night(1200, 100));
                        let pressed = hand.press_at(at(shared::DESERT_KELVIN, 0.0));
                        hand.wait_until(pressed + if beating { SECOND } else { 0.5 });
                        hand.release();
                        let output = hand.settled();
                        assert_eq!(
                            repaint_delay(&output),
                            Duration::MAX,
                            "released while beating {beating}"
                        );
                    }
                }
            }

            #[test]
            fn a_press_and_a_drag_past_a_detent_leave_one_action() {
                let mut hand = Hand::keeping(night(1300, 100));
                hand.move_to(at(1300, 0.0));
                hand.press();
                // In 1-point moves, over 1200 K and on past 1100 K.
                hand.slide(at(1100, 2.0).x, 1.0);
                assert_eq!(hand.ticks(), 1);
                let kelvins: Vec<u16> = hand.looks().iter().map(|look| look.kelvin).collect();
                assert!(kelvins.contains(&1200), "the drag never caught 1200 K");
                assert_eq!(kelvins.last(), Some(&kelvin_under(hand.pointer)));
                hand.release();
                assert_eq!(hand.ticks(), 1);
            }

            #[test]
            fn brightness_dragged_end_to_end_leaves_no_action() {
                let mut hand = Hand::keeping(night(1200, 100));
                hand.move_to(pos2(LEFT_END, BRIGHTNESS_ROW.y));
                hand.press();
                hand.slide(LEFT_END + TRAVEL, 6.0);
                hand.slide(LEFT_END, 6.0);
                hand.release();
                // It was dragged: the look reached both ends of the scale.
                let brightness: Vec<u8> = hand.looks().iter().map(|l| l.brightness).collect();
                for end in [color::MIN_BRIGHTNESS, color::MAX_BRIGHTNESS] {
                    assert!(brightness.contains(&end), "{end}% in {brightness:?}");
                }
                assert_eq!(hand.ticks(), 0);
                assert!(hand.window.app.actions.contains(&Action::EndPreview));
            }

            /// The app plays each tick action as one tick (see
            /// `app::tests`); here, the window leaves one for each detent a
            /// scrub catches, and the trackpad plays them all.
            #[test]
            fn a_scrub_plays_one_tick_for_each_detent_it_catches() {
                let mut hand = Hand::new(night(6500, 100));
                hand.window.app.haptics = Haptics::counting();
                // A hand scrubbing the track from Daylight to pure red and
                // back, in 2-point moves, plays one for each detent it
                // catches: 2700 K, Desert, 1200 K and pure red going, and
                // coming back the three inside the track, since leaving pure
                // red catches nothing.
                hand.move_to(pos2(LEFT_END, TEMPERATURE_ROW.y));
                hand.press();
                hand.slide(LEFT_END + TRAVEL, 2.0);
                assert_eq!((hand.ticks(), hand.kelvin()), (4, color::PURE_RED_KELVIN));
                hand.slide(LEFT_END, 2.0);
                hand.release();
                assert_eq!(hand.ticks(), 7);
                assert_eq!(
                    played(),
                    Played {
                        ticks: 7,
                        thumps: 0,
                        presses: 0
                    }
                );
            }

            #[test]
            fn a_held_detent_leaves_no_more_actions_and_a_new_catch_leaves_one() {
                let mut hand = Hand::new(night(1300, 100));
                hand.move_to(at(1300, 0.0));
                hand.press();
                hand.slide(at(1200, -2.0).x, 1.0);
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 1200));
                // The pointer trembles inside the release: the value stays.
                for off in [4.0, -4.0, 4.9, -4.9, 0.0, 3.0, -3.0, 4.5] {
                    hand.move_to(at(1200, off));
                    assert_eq!((hand.ticks(), hand.kelvin()), (1, 1200), "{off} points");
                }
                // Out past the release, the value follows the pointer, and
                // back into the catch it ticks once more.
                hand.move_to(at(1200, 6.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (1, kelvin_under(hand.pointer))
                );
                assert_ne!(hand.kelvin(), 1200);
                hand.move_to(at(1200, 2.0));
                assert_eq!((hand.ticks(), hand.kelvin()), (2, 1200));
            }

            #[test]
            fn a_frame_that_crosses_two_detents_leaves_one_action() {
                let mut hand = Hand::new(night(6000, 100));
                hand.move_to(pos2(30.0, TEMPERATURE_ROW.y));
                hand.press();
                // The drag begins clear of every detent.
                hand.move_to(pos2(40.0, TEMPERATURE_ROW.y));
                assert_eq!(hand.ticks(), 0);
                // One frame, three moves, over 2700 K and Desert, to a place
                // clear of them.
                let ahead = pos2(240.0, TEMPERATURE_ROW.y);
                hand.pointer = ahead;
                hand.frame(vec![
                    Event::PointerMoved(pos2(100.0, TEMPERATURE_ROW.y)),
                    Event::PointerMoved(pos2(170.0, TEMPERATURE_ROW.y)),
                    Event::PointerMoved(ahead),
                ]);
                assert_eq!((hand.ticks(), hand.kelvin()), (1, kelvin_under(ahead)));
            }

            #[test]
            fn ticks_less_than_fifty_milliseconds_apart_are_one() {
                for (gap, ticks) in [(0.03, 1), (0.0625, 2)] {
                    let mut hand = Hand::new(night(3000, 100));
                    hand.gap = gap;
                    hand.move_to(at(3000, 0.0));
                    hand.press();
                    // Over 2700 K, and back over it.
                    hand.move_to(at(2500, 0.0));
                    assert_eq!(hand.ticks(), 1, "{gap} s apart");
                    hand.move_to(at(3000, 0.0));
                    assert_eq!(hand.ticks(), ticks, "{gap} s apart");
                }
            }

            #[test]
            fn the_last_tick_outlasts_the_drag() {
                let mut hand = Hand::new(night(1500, 100));
                hand.gap = 0.01;
                // Two clicks, each landing on a detent from elsewhere: the
                // second release comes 30 ms after the first.
                hand.click(at(2700, 0.0));
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
                hand.click(at(1200, 0.0));
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 1200));
                // A click long after ticks again.
                hand.gap = 0.1;
                hand.click(at(2700, 0.0));
                assert_eq!((hand.ticks(), hand.kelvin()), (2, 2700));
            }

            #[test]
            fn a_click_within_three_points_of_a_detent_lands_on_it() {
                for (kelvin, off) in [
                    (2700, -2.5),
                    (2700, 0.0),
                    (2700, 2.5),
                    (shared::DESERT_KELVIN, -2.5),
                    (shared::DESERT_KELVIN, 2.5),
                ] {
                    let mut hand = Hand::new(night(1500, 100));
                    let pointer = at(kelvin, off);
                    hand.click(pointer);
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (1, kelvin),
                        "a click {off} points from {kelvin} K"
                    );
                }
                // Clear of them it goes where the pointer is, with no tick.
                let mut hand = Hand::new(night(1500, 100));
                hand.click(at(2000, 0.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (0, kelvin_under(hand.pointer))
                );
            }

            #[test]
            fn a_click_on_the_knob_at_a_detent_keeps_it_without_a_tick() {
                // Out to the release, which is wider than the catch.
                for off in [0.0, 2.0, 4.0, -4.0] {
                    let mut hand = Hand::new(night(1200, 100));
                    let pointer = at(1200, off);
                    hand.click(pointer);
                    assert_eq!((hand.ticks(), hand.kelvin()), (0, 1200), "{off} points");
                    if off.abs() > 3.0 {
                        // Not the catch's doing: the pointer reads another kelvin.
                        assert_ne!(kelvin_under(pointer), 1200);
                    }
                }
                // Past the release a click is a click.
                let mut hand = Hand::new(night(1200, 100));
                hand.click(at(1200, 6.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (0, kelvin_under(hand.pointer))
                );
            }

            /// The knob near Desert, a press 4 points from it, inside the
            /// release but outside the catch, and → onto Desert before
            /// the press ends. The press found the value off the detent,
            /// so it keeps nothing there: it lands where it would have
            /// without the key.
            #[test]
            fn a_press_keeps_only_the_detent_the_value_was_on_as_it_began() {
                // A click lands where it is let go.
                let desert = shared::DESERT_KELVIN;
                let mut hand = Hand::new(night(desert + 30, 100));
                hand.focus(TEMPERATURE_ROW);
                let pointer = at(desert, 4.0);
                hand.move_to(pointer);
                hand.press();
                hand.key(Key::ArrowRight, Modifiers::NONE);
                assert_eq!(hand.kelvin(), desert);
                hand.release();
                assert_ne!(kelvin_under(pointer), desert);
                assert_eq!((hand.ticks(), hand.kelvin()), (0, kelvin_under(pointer)));
                // And so does a press that egui calls a drag only 0.8 s on.
                let mut hand = Hand::new(night(desert + 30, 100));
                hand.focus(TEMPERATURE_ROW);
                hand.move_to(pointer);
                hand.press();
                hand.key(Key::ArrowRight, Modifiers::NONE);
                assert_eq!(hand.kelvin(), desert);
                hand.gap = 0.25;
                for _ in 0..4 {
                    hand.output(vec![]);
                }
                assert_eq!((hand.ticks(), hand.kelvin()), (0, kelvin_under(pointer)));
                // With no key, the press keeps the detent the value is on.
                let mut hand = Hand::new(night(desert, 100));
                hand.move_to(pointer);
                hand.press();
                hand.gap = 0.25;
                for _ in 0..4 {
                    hand.output(vec![]);
                }
                assert_eq!((hand.ticks(), hand.kelvin()), (0, desert));
            }

            /// A secondary button, as a click with two fingers, neither
            /// clicks nor drags a slider, whether it is let go at once or
            /// held as egui calls it a drag.
            #[test]
            fn a_secondary_button_leaves_the_sliders_alone() {
                for row in [TEMPERATURE_ROW, BRIGHTNESS_ROW] {
                    let mut hand = Hand::keeping(night(4500, 60));
                    // A click.
                    let start = pos2(LEFT_END + 40.0, row.y);
                    hand.move_to(start);
                    hand.frame(vec![secondary(start, true)]);
                    hand.frame(vec![secondary(start, false)]);
                    // A drag across the track.
                    hand.frame(vec![secondary(start, true)]);
                    hand.slide(LEFT_END + TRAVEL, 6.0);
                    let end = hand.pointer;
                    hand.frame(vec![secondary(end, false)]);
                    assert_eq!(hand.looks(), vec![], "{}", row.label);
                    assert_eq!(hand.ticks(), 0, "{}", row.label);
                    assert_eq!(
                        (hand.kelvin(), hand.brightness()),
                        (4500, 60),
                        "{}",
                        row.label
                    );
                }
            }

            #[test]
            fn a_detent_caught_and_let_go_in_the_press_frame_does_not_tick() {
                // The press lands on 2700 K, and the same frame's move pulls
                // it free, past the 5-point release: the knob is never shown
                // on 2700 K. More than 6 points from the press, egui sees the
                // drag on the next frame, and the value follows the pointer.
                for (press, then) in [(2.0, 10.0), (2.5, -6.0)] {
                    let mut hand = Hand::new(night(1500, 100));
                    let start = at(2700, press);
                    hand.move_to(start);
                    hand.pointer = at(2700, then);
                    hand.frame(vec![button(start, true), Event::PointerMoved(hand.pointer)]);
                    assert_eq!(
                        hand.kelvin(),
                        1500,
                        "pressed {press} points off, then {then}"
                    );
                    hand.output(vec![]);
                    assert_eq!(hand.ticks(), 0, "pressed {press} points off, then {then}");
                    assert_eq!(hand.kelvin(), kelvin_under(hand.pointer));
                    assert_ne!(hand.kelvin(), 2700);
                }
            }

            #[test]
            fn a_press_that_moves_within_the_release_before_the_drag_is_seen_lands_and_ticks() {
                let mut hand = Hand::new(night(1500, 100));
                hand.move_to(at(2700, -2.5));
                hand.press();
                // 7 points on, egui sees the drag, still within 5 points. The
                // pointer is out of the 3-point catch by then: where the press
                // landed caught the detent.
                hand.move_to(at(2700, 4.5));
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
            }

            #[test]
            fn a_press_within_three_points_of_a_detent_snaps_and_ticks_in_its_own_frame() {
                for (kelvin, off) in [
                    (2700, -2.5),
                    (2700, 0.0),
                    (2700, 2.5),
                    (THUMPER_STOP, -2.5),
                    (THUMPER_STOP, 0.0),
                    (THUMPER_STOP, 2.5),
                    (1200, -2.5),
                    (1200, 2.5),
                ] {
                    let mut hand = Hand::new(night(1500, 100));
                    hand.move_to(at(kelvin, off));
                    hand.press();
                    // Before any release.
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (1, kelvin),
                        "a press {off} points from {kelvin} K"
                    );
                    // The release adds no tick and leaves the value.
                    hand.release();
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (1, kelvin),
                        "released {off} points from {kelvin} K"
                    );
                }
            }

            #[test]
            fn the_frame_of_a_snapping_press_leaves_one_look_and_one_tick() {
                let mut hand = Hand::keeping(night(1500, 100));
                hand.move_to(at(2700, 2.5));
                assert_eq!(hand.window.app.actions, vec![]);
                hand.press();
                // The real screen previews the snap from this frame on.
                assert_eq!(
                    std::mem::take(&mut hand.window.app.actions),
                    vec![
                        Action::SetNight {
                            look: night(2700, 100),
                            dragging: true
                        },
                        Action::Detent
                    ]
                );
                // The let-go sets the look, with no tick, and ends the
                // preview; the app saves it as after any change.
                hand.release();
                assert_eq!(
                    hand.window.app.actions,
                    vec![
                        Action::SetNight {
                            look: night(2700, 100),
                            dragging: false
                        },
                        Action::EndPreview
                    ]
                );
            }

            /// Every press by the primary button previews the look from its
            /// own frame, on either slider, and the frame it comes up in ends
            /// the preview once, however the press ended: a click, a drag,
            /// or a press dragged off the window's controls.
            #[test]
            fn a_press_previews_from_its_own_frame_and_its_let_go_ends_the_preview() {
                let rows = [
                    (TEMPERATURE_ROW, at(2700, 12.0)),
                    (BRIGHTNESS_ROW, pos2(LEFT_END + 100.0, BRIGHTNESS_ROW.y)),
                ];
                for (row, pointer) in rows {
                    for ending in ["click", "drag", "off"] {
                        let mut hand = Hand::new(night(1500, 60));
                        let found = hand.window.app.settings.night;
                        hand.move_to(pointer);
                        hand.window.app.actions.clear();
                        hand.applies = false;
                        hand.press();
                        let what = format!("{} {ending}", row.label);
                        assert_eq!(
                            hand.window.app.actions,
                            vec![Action::SetNight {
                                look: found,
                                dragging: true
                            }],
                            "{what}, the press frame"
                        );
                        hand.settle();
                        hand.applies = true;
                        match ending {
                            "click" => {}
                            "drag" => hand.slide(pointer.x + 40.0, 4.0),
                            // Down off the sliders, over the window's foot.
                            _ => hand.move_to(pos2(pointer.x, pointer.y + 200.0)),
                        }
                        hand.applies = false;
                        hand.release();
                        assert_eq!(hand.let_go(), 1, "{what}");
                        let previews: Vec<bool> = hand
                            .window
                            .app
                            .actions
                            .iter()
                            .filter_map(|action| match action {
                                Action::SetNight { dragging, .. } => Some(*dragging),
                                _ => None,
                            })
                            .collect();
                        assert!(!previews.contains(&true), "{what}: {previews:?}");
                        // Nothing more ends it.
                        hand.settle();
                        hand.frame(vec![]);
                        assert_eq!(hand.let_go(), 0, "{what}, after");
                    }
                }
            }

            /// A secondary button never holds a slider, so it never
            /// previews.
            #[test]
            fn a_secondary_press_on_a_slider_previews_nothing() {
                for row in [TEMPERATURE_ROW, BRIGHTNESS_ROW] {
                    let mut hand = Hand::keeping(night(1500, 60));
                    let pointer = pos2(LEFT_END + 100.0, row.y);
                    hand.move_to(pointer);
                    hand.frame(vec![secondary(pointer, true)]);
                    hand.frame(vec![secondary(pointer, false)]);
                    assert_eq!(hand.window.app.actions, vec![], "{}", row.label);
                }
            }

            /// A key that sets the value in the press's own frame takes the
            /// frame: the press snaps nothing and ticks for nothing the knob
            /// is never shown on. The release catches the detent from the
            /// value the press began on, with its one tick.
            #[test]
            fn a_key_in_the_frame_of_a_snapping_press_takes_the_frame_with_no_tick() {
                let mut hand = Hand::new(night(1500, 100));
                hand.focus(TEMPERATURE_ROW);
                let pointer = at(2700, -2.5);
                hand.move_to(pointer);
                hand.frame(vec![
                    button(pointer, true),
                    key_down(Key::ArrowLeft, Modifiers::NONE),
                ]);
                let stepped = hand.kelvin();
                assert_ne!(stepped, 1500, "the key moved the value");
                assert_ne!(stepped, 2700);
                assert_eq!(hand.ticks(), 0, "the press frame");
                hand.release();
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
            }

            /// A key in the frame where a drag reaches a detent: that frame
            /// shows the key's value with no tick, and the next shows the
            /// drag on the detent, with the tick.
            #[test]
            fn a_key_in_the_frame_a_drag_catches_a_detent_puts_the_tick_in_the_next() {
                let mut hand = Hand::new(night(1500, 100));
                hand.gap = 0.1;
                hand.focus(TEMPERATURE_ROW);
                hand.move_to(at(2700, 14.0));
                hand.press();
                hand.slide(at(2700, 6.0).x, 2.0);
                assert_eq!(hand.ticks(), 0, "outside the catch");
                let caught = at(2700, 2.0);
                hand.pointer = caught;
                hand.frame(vec![
                    Event::PointerMoved(caught),
                    key_down(Key::ArrowRight, Modifiers::NONE),
                ]);
                assert_ne!(hand.kelvin(), 2700);
                assert_eq!(hand.ticks(), 0, "the key's frame");
                hand.output(vec![]);
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
            }

            /// egui tells of no button down on a widget in a frame that lets
            /// one go, so a release and a new press in one frame end the old
            /// press there: the new one keeps nothing the old one caught.
            #[test]
            fn a_press_in_the_frame_that_lets_the_last_go_keeps_nothing_of_it() {
                let mut hand = Hand::new(night(1500, 100));
                hand.focus(TEMPERATURE_ROW);
                hand.move_to(at(2700, 0.0));
                hand.press();
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
                // A key moves the value off, so the press keeps 2700 only as
                // the detent it caught; then a drag far off.
                hand.key(Key::ArrowRight, Modifiers::NONE);
                hand.slide(at(2700, 40.0).x, 4.0);
                assert_ne!(hand.kelvin(), 2700);
                // The release and a new press 4 points from 2700, within its
                // release but outside its catch, in one frame.
                let again = at(2700, 4.0);
                hand.frame(vec![button(hand.pointer, false), button(again, true)]);
                hand.pointer = again;
                hand.release();
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (1, kelvin_under(again)),
                    "the new press kept 2700"
                );
            }

            #[test]
            fn a_press_in_the_zone_of_pure_red_snaps_to_it_in_its_own_frame() {
                let end = LEFT_END + TRAVEL;
                // Pure red's zone is the last 10.4 points of the track.
                for from_end in [0.0, 5.0, 10.0] {
                    let mut hand = Hand::new(night(1500, 100));
                    hand.move_to(pos2(end - from_end, TEMPERATURE_ROW.y));
                    hand.press();
                    let want = (1, color::PURE_RED_KELVIN);
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        want,
                        "{from_end} points from the end"
                    );
                    hand.release();
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        want,
                        "released {from_end} points from the end"
                    );
                }
                // Just clear of the zone, the press moves nothing.
                let mut hand = Hand::keeping(night(1500, 100));
                hand.move_to(pos2(end - 11.0, TEMPERATURE_ROW.y));
                hand.press();
                assert_eq!((hand.moves(), hand.ticks()), (vec![], 0));
            }

            #[test]
            fn a_press_on_the_knob_at_its_detent_moves_and_ticks_nothing() {
                for off in [0.0, 2.0, 4.0, -4.0] {
                    let mut hand = Hand::keeping(night(1200, 100));
                    hand.move_to(at(1200, off));
                    hand.press();
                    assert_eq!(
                        (hand.moves(), hand.ticks()),
                        (vec![], 0),
                        "pressed {off} points from the knob"
                    );
                    hand.release();
                    assert_eq!(
                        (hand.moves(), hand.ticks()),
                        (vec![], 0),
                        "released {off} points from the knob"
                    );
                }
            }

            #[test]
            fn a_press_outside_the_catch_moves_nothing_until_the_release() {
                for off in [4.0, -4.0] {
                    let mut hand = Hand::new(night(1500, 100));
                    let pointer = at(2700, off);
                    hand.move_to(pointer);
                    hand.press();
                    assert_eq!((hand.ticks(), hand.kelvin()), (0, 1500), "{off} points");
                    // The release lands where it is let go, with no tick.
                    hand.release();
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (0, kelvin_under(pointer)),
                        "{off} points"
                    );
                    assert_ne!(hand.kelvin(), 2700);
                }
            }

            #[test]
            fn a_pure_red_caught_and_let_go_in_the_press_frame_does_not_tick() {
                // The press lands in pure red's zone, and the same frame's move
                // takes the pointer well clear of it.
                let mut hand = Hand::new(night(1500, 100));
                let start = pos2(LEFT_END + TRAVEL - 5.0, TEMPERATURE_ROW.y);
                hand.move_to(start);
                hand.pointer = pos2(LEFT_END + TRAVEL - 40.0, TEMPERATURE_ROW.y);
                hand.frame(vec![button(start, true), Event::PointerMoved(hand.pointer)]);
                assert_eq!((hand.ticks(), hand.kelvin()), (0, 1500));
                // The next frame, egui sees the drag and the value follows.
                hand.output(vec![]);
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (0, kelvin_under(hand.pointer))
                );
                assert_ne!(hand.kelvin(), color::PURE_RED_KELVIN);
            }

            #[test]
            fn a_drag_after_a_snapping_press_lets_go_past_five_points_with_no_second_tick() {
                let mut hand = Hand::new(night(1500, 100));
                // Frames more than 50 ms apart, so that the limit stays out of
                // the way.
                hand.gap = 0.1;
                hand.move_to(at(2700, 1.0));
                hand.press();
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
                // A later frame drags 9 points: past the 5-point release.
                hand.move_to(at(2700, 10.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (1, kelvin_under(hand.pointer))
                );
                assert_ne!(hand.kelvin(), 2700);
                // And back within the catch it ticks once more, re-armed.
                hand.move_to(at(2700, 2.0));
                assert_eq!((hand.ticks(), hand.kelvin()), (2, 2700));
            }

            #[test]
            fn a_key_under_a_snapped_press_does_not_change_the_detent_it_keeps() {
                // The release lands back on the detent the press caught, and
                // so does the drag that egui sees 0.8 s on.
                for still in [false, true] {
                    let mut hand = Hand::new(night(1500, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.move_to(at(2700, 0.0));
                    hand.press();
                    assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700));
                    // → moves the value before egui calls the press a click or a
                    // drag.
                    hand.key(Key::ArrowRight, Modifiers::NONE);
                    assert_ne!(hand.kelvin(), 2700, "the key moved the value");
                    if still {
                        hand.gap = 0.25;
                        for _ in 0..4 {
                            hand.output(vec![]);
                        }
                    } else {
                        hand.release();
                    }
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (1, 2700),
                        "still press {still}"
                    );
                }
            }

            #[test]
            fn the_fifty_millisecond_limit_holds_across_presses() {
                for (gap, ticks) in [(0.01, 1), (0.1, 2)] {
                    let mut hand = Hand::new(night(1500, 100));
                    hand.gap = gap;
                    hand.click(at(2700, 0.0));
                    assert_eq!((hand.ticks(), hand.kelvin()), (1, 2700), "{gap} s apart");
                    // A second press in the zone of another detent, three frames
                    // on: it snaps, and ticks only if the limit has passed.
                    hand.move_to(at(1200, 0.0));
                    hand.press();
                    assert_eq!(
                        (hand.ticks(), hand.kelvin()),
                        (ticks, 1200),
                        "{gap} s apart"
                    );
                }
            }

            #[test]
            fn a_secondary_press_in_a_detents_zone_moves_and_ticks_nothing() {
                let mut hand = Hand::keeping(night(1500, 100));
                let pointer = at(2700, 0.0);
                hand.move_to(pointer);
                hand.frame(vec![secondary(pointer, true)]);
                assert_eq!((hand.looks(), hand.ticks()), (vec![], 0));
                hand.frame(vec![secondary(pointer, false)]);
                assert_eq!((hand.looks(), hand.ticks()), (vec![], 0));
                assert_eq!(hand.kelvin(), 1500);
            }

            #[test]
            fn a_press_on_the_brightness_slider_moves_nothing_in_its_frame() {
                // Over the places that the Temperature slider's detents are at,
                // and at both ends.
                let xs = [2700, THUMPER_STOP, 1200]
                    .map(|kelvin| at(kelvin, 0.0).x)
                    .into_iter()
                    .chain([LEFT_END, LEFT_END + TRAVEL]);
                for x in xs {
                    let mut hand = Hand::keeping(night(1500, 60));
                    hand.move_to(pos2(x, BRIGHTNESS_ROW.y));
                    hand.press();
                    assert_eq!(
                        (hand.moves(), hand.ticks()),
                        (vec![], 0),
                        "pressed at x {x}"
                    );
                    assert_eq!((hand.kelvin(), hand.brightness()), (1500, 60));
                }
            }

            #[test]
            fn the_hold_ends_with_the_pointer() {
                let mut hand = Hand::new(night(1300, 100));
                hand.move_to(at(1300, 0.0));
                hand.press();
                hand.slide(at(1200, 0.0).x, 1.0);
                assert_eq!((hand.ticks(), hand.kelvin()), (1, 1200));
                // Pulled free, and let go there.
                hand.move_to(at(1200, 8.0));
                hand.release();
                assert_ne!(hand.kelvin(), 1200);
                // A click 4 points from 1200 K, with the value elsewhere, is
                // not held by what the last drag held.
                hand.click(at(1200, 4.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (1, kelvin_under(hand.pointer))
                );
                // Nor is a new drag from there.
                hand.move_to(at(1200, 20.0));
                hand.release();
                hand.move_to(at(1200, 4.0));
                hand.press();
                hand.move_to(at(1200, 12.0));
                assert_eq!(
                    (hand.ticks(), hand.kelvin()),
                    (1, kelvin_under(hand.pointer))
                );
            }

            #[test]
            fn the_arrow_keys_stop_on_a_detent_and_the_next_press_moves_on() {
                // Toward red, from a step short of 1200 K and of Desert; then
                // toward day, from a step short of 1200 K, Desert and 2700 K.
                let desert = shared::DESERT_KELVIN;
                let past =
                    |kelvin: u16, by: f32| color::kelvin_at_redness(color::redness_of(kelvin) + by);
                for (start, press, onto, then) in [
                    (1250, Key::ArrowRight, 1200, 1160),
                    (1250, Key::ArrowUp, 1200, 1160),
                    (
                        past(desert, -0.01),
                        Key::ArrowRight,
                        desert,
                        past(desert, 0.02),
                    ),
                    (past(1200, 0.01), Key::ArrowLeft, 1200, past(1200, -0.02)),
                    (
                        past(desert, 0.01),
                        Key::ArrowLeft,
                        desert,
                        past(desert, -0.02),
                    ),
                    (past(2700, 0.01), Key::ArrowDown, 2700, past(2700, -0.02)),
                ] {
                    let mut hand = Hand::new(night(start, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.key(press, Modifiers::NONE);
                    assert_eq!(hand.kelvin(), onto, "{press:?} from {start} K");
                    hand.key(press, Modifiers::NONE);
                    assert_eq!(hand.kelvin(), then, "{press:?} on from {onto} K");
                    assert_eq!(hand.ticks(), 0, "{press:?} from {start} K");
                }
            }

            #[test]
            fn option_arrows_jump_between_the_stops() {
                let mut hand = Hand::new(night(4000, 100));
                hand.focus(TEMPERATURE_ROW);
                let mut seen = vec![];
                for _ in 0..5 {
                    hand.key(Key::ArrowRight, Modifiers::ALT);
                    seen.push(hand.kelvin());
                }
                // Pure red is the last stop that way.
                let desert = shared::DESERT_KELVIN;
                assert_eq!(seen, [2700, desert, 1200, 850, 850]);
                let mut seen = vec![];
                for _ in 0..5 {
                    hand.key(Key::ArrowLeft, Modifiers::ALT);
                    seen.push(hand.kelvin());
                }
                assert_eq!(seen, [1200, desert, 2700, 6500, 6500]);
                // Up and down jump the same ways.
                hand.key(Key::ArrowUp, Modifiers::ALT);
                assert_eq!(hand.kelvin(), 2700);
                hand.key(Key::ArrowDown, Modifiers::ALT);
                assert_eq!(hand.kelvin(), 6500);
                assert_eq!(hand.ticks(), 0);
            }

            #[test]
            fn option_with_an_arrow_moves_to_the_next_stop_and_no_further() {
                // From between two stops: the nearest that way, and there it
                // stays, where a step on top of it would leave 1200 K.
                // The stops on either side of 1500 K and of 2000 K.
                let desert = shared::DESERT_KELVIN;
                for (start, press, onto) in [
                    (1500, Key::ArrowRight, 1200),
                    (1500, Key::ArrowLeft, desert),
                    (2000, Key::ArrowRight, desert),
                    (2000, Key::ArrowLeft, 2700),
                ] {
                    let mut hand = Hand::new(night(start, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.key(press, Modifiers::ALT);
                    assert_eq!(hand.kelvin(), onto, "{press:?} from {start} K");
                    assert_eq!(hand.ticks(), 0);
                }
            }

            #[test]
            fn the_pages_stop_on_a_detent_they_pass() {
                let near =
                    |kelvin: u16, by: f32| color::kelvin_at_redness(color::redness_of(kelvin) + by);
                // Four steps toward red from 0.05 short of 1200 K...
                let mut hand = Hand::new(night(near(1200, -0.05), 100));
                hand.focus(TEMPERATURE_ROW);
                hand.key(Key::PageUp, Modifiers::NONE);
                assert_eq!(hand.kelvin(), 1200);
                hand.key(Key::PageUp, Modifiers::NONE);
                assert!(hand.kelvin() < 1200, "{} K", hand.kelvin());
                // ...and four toward day from 0.05 past 2700 K.
                let mut hand = Hand::new(night(near(2700, 0.05), 100));
                hand.focus(TEMPERATURE_ROW);
                hand.key(Key::PageDown, Modifiers::NONE);
                assert_eq!(hand.kelvin(), 2700);
                hand.key(Key::PageDown, Modifiers::NONE);
                assert!(hand.kelvin() > 2700, "{} K", hand.kelvin());
                assert_eq!(hand.ticks(), 0);
            }

            #[test]
            fn the_keys_reach_pure_red_and_leave_it() {
                // Toward red from 900 K, each press moves on until pure red,
                // and there the value stays. 860 K is the last value short of
                // pure red's zone.
                for (press, want) in [
                    (Key::ArrowRight, vec![880, 870, 860, 850, 850]),
                    (Key::ArrowUp, vec![880, 870, 860, 850, 850]),
                    (Key::PageUp, vec![860, 850, 850]),
                ] {
                    let mut hand = Hand::new(night(900, 100));
                    hand.focus(TEMPERATURE_ROW);
                    let mut seen = vec![];
                    for _ in &want {
                        hand.key(press, Modifiers::NONE);
                        seen.push(hand.kelvin());
                    }
                    assert_eq!(seen, want, "{press:?} from 900 K");
                    assert_eq!(hand.ticks(), 0, "{press:?}");
                }
                // Back from pure red, each press leaves it.
                for (press, want) in [
                    (Key::ArrowLeft, 860),
                    (Key::ArrowDown, 860),
                    (Key::PageDown, 870),
                ] {
                    let mut hand = Hand::new(night(850, 100));
                    hand.focus(TEMPERATURE_ROW);
                    hand.key(press, Modifiers::NONE);
                    assert_eq!(hand.kelvin(), want, "{press:?} from pure red");
                    assert_eq!(hand.ticks(), 0, "{press:?}");
                }
            }

            #[test]
            fn on_brightness_option_arrows_step_like_plain_ones() {
                let mut hand = Hand::new(night(1200, 60));
                hand.focus(BRIGHTNESS_ROW);
                for (press, modifiers, want) in [
                    (Key::ArrowRight, Modifiers::NONE, 65),
                    (Key::ArrowRight, Modifiers::ALT, 70),
                    (Key::ArrowLeft, Modifiers::ALT, 65),
                    (Key::ArrowUp, Modifiers::ALT, 70),
                    (Key::ArrowDown, Modifiers::NONE, 65),
                    (Key::ArrowLeft, Modifiers::NONE, 60),
                ] {
                    hand.key(press, modifiers);
                    assert_eq!(hand.brightness(), want, "{press:?} with {modifiers:?}");
                }
                assert_eq!((hand.kelvin(), hand.ticks()), (1200, 0));
            }

            #[test]
            fn the_frame_of_a_snap_reads_desert_everywhere() {
                let p = Palette::DARK;
                let desert = shared::DESERT_KELVIN;
                let mut hand = Hand::new(night(desert + 40, 100));
                hand.window.ctx.enable_accesskit();
                let mut output = hand.output(vec![]);
                let shapes = testing::shapes(&mut output);
                assert_eq!(
                    readout(&shapes, TEMPERATURE_ROW),
                    format!("{} K", desert + 40)
                );
                assert!(labels(&mut output).contains(&format!("Temperature: {} K", desert + 40)));
                // The drag lands 2.5 points past Desert, where the pointer
                // reads some other kelvin, and snaps onto it.
                hand.move_to(pos2(60.0, TEMPERATURE_ROW.y));
                hand.press();
                let pointer = at(desert, 2.5);
                assert_ne!(kelvin_under(pointer), desert);
                hand.pointer = pointer;
                let mut output = hand.output(vec![Event::PointerMoved(pointer)]);
                // The label and the picture, both of the frame of the move.
                let labels = labels(&mut output);
                assert!(
                    labels.contains(&format!("Temperature: {desert} K, Desert")),
                    "{labels:?}"
                );
                let shapes = testing::shapes(&mut output);
                assert_eq!(readout(&shapes, TEMPERATURE_ROW), format!("{desert} K"));
                let ticks = ticks_under(&shapes, TEMPERATURE_ROW);
                assert_eq!(ticks.len(), 3);
                for (tick, want) in ticks.iter().zip([p.dim, p.text, p.dim]) {
                    assert_eq!(tick.fill, want);
                }
                assert_eq!((hand.ticks(), hand.kelvin()), (1, desert));
            }

            /// The card's title: the night look, in bold 28.
            fn card_title(shapes: &[Shape]) -> String {
                let found: Vec<String> = texts(shapes)
                    .iter()
                    .filter(|text| text.galley.job.sections[0].format.font_id == theme::bold(28.0))
                    .map(|text| text.galley.text().to_owned())
                    .collect();
                assert_eq!(found.len(), 1, "titles: {found:?}");
                found.into_iter().next().expect("one title")
            }

            #[test]
            fn the_card_reads_a_clicked_snap_in_its_frame_and_another_frame_follows() {
                let desert = shared::DESERT_KELVIN;
                let mut hand = Hand::new(night(desert + 40, 100));
                let pointer = at(desert, 2.5);
                assert_ne!(kelvin_under(pointer), desert);
                hand.move_to(pointer);
                // The press snaps the knob onto Desert in its own frame, with
                // no wait for the release.
                let mut output = hand.output(vec![button(pointer, true)]);
                // egui asks for the next frame at once, as after every frame
                // with input (`Context::begin_pass`, egui 0.36.2).
                let root = &output.viewport_output[&egui::ViewportId::ROOT];
                assert_eq!(root.repaint_delay, std::time::Duration::ZERO);
                let shapes = testing::shapes(&mut output);
                // The card reads the frame's look, as the readout does.
                assert_eq!(readout(&shapes, TEMPERATURE_ROW), format!("{desert} K"));
                assert_eq!(card_title(&shapes), format!("{desert} K"));
                assert_eq!((hand.ticks(), hand.kelvin()), (1, desert));
                // And so does the frame after, once the app has the look.
                let shapes = hand.frame(vec![]);
                assert_eq!(readout(&shapes, TEMPERATURE_ROW), format!("{desert} K"));
                assert_eq!(card_title(&shapes), format!("{desert} K"));
            }

            #[test]
            fn a_brightness_drag_reads_its_new_percentage_in_the_frame_of_the_move() {
                let mut hand = Hand::new(night(1200, 100));
                hand.window.ctx.enable_accesskit();
                hand.move_to(pos2(LEFT_END + TRAVEL, BRIGHTNESS_ROW.y));
                hand.press();
                // The middle of the strip is 60%.
                let pointer = pos2(LEFT_END + TRAVEL / 2.0, BRIGHTNESS_ROW.y);
                hand.pointer = pointer;
                let mut output = hand.output(vec![Event::PointerMoved(pointer)]);
                let labels = labels(&mut output);
                assert!(labels.contains(&"Brightness: 60%".to_owned()), "{labels:?}");
                let shapes = testing::shapes(&mut output);
                assert_eq!(readout(&shapes, BRIGHTNESS_ROW), "60%");
                // The Temperature beside it has not moved.
                assert_eq!(readout(&shapes, TEMPERATURE_ROW), "1200 K");
            }
        }
    }
}
