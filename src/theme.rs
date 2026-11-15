//! The window's look: the desert of Arrakis at night. Surfaces in dark
//! oxide, as the spice colors the sand, San Francisco and pills, and a
//! golden spice for what is on.
//!
//! Contrast is carried in luminance, never in hue alone: the window is
//! often seen through its own red tint, where only the red channel
//! survives. The gold is bright in red, so what is on (a switch, Close)
//! stays the brightest thing on the window through the tint.

use egui::{Color32, FontId, Vec2, vec2};
use fastframe_fonts::{Monospace, Primary, Weight};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub window: Color32,
    /// The player bar at the foot.
    pub panel: Color32,
    /// Cards, fields, buttons.
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub outline: Color32,
    /// The edge of a time field and its stepper: what keeps them apart
    /// from the window, where their fill barely differs.
    pub field_stroke: Color32,
    pub text: Color32,
    pub secondary: Color32,
    /// The third line of the now-playing card, on its wash.
    pub faint: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    /// The accent, a third of the way up from the ground: a selected part
    /// of a time.
    pub selection: Color32,
    /// A switch's knob, and a slider's.
    pub knob: Color32,
    /// The card's progress track: white, mostly clear.
    pub track: Color32,
    /// The warm of the cover's low sky, which washes the now-playing card
    /// by [`CARD_WASH`].
    pub wash: Color32,
    /// The scene on the cover.
    pub cover: Cover,
}

/// The cover's scene, as it looks at noon: the night look tints it.
///
/// It is lit from behind, so its parts step down in the red channel alone
/// from the sun to the worm: sky, far dune, near dune, worm. The worm is the
/// color of the sand it comes out of, and may merge with it; it reads
/// against the sun and the sky behind it, and its teeth against its throat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cover {
    pub sky_top: Color32,
    /// The sky at the foot of the dunes.
    pub sky_low: Color32,
    pub sun: Color32,
    pub far: Color32,
    /// The far dune's slip face, in its own shade.
    pub lee: Color32,
    pub near: Color32,
    /// The worm's lip at its outer edge: the near dune's hue at a lower
    /// value.
    pub worm: Color32,
    /// The lip where it meets the throat: it is shaded in from `worm`.
    pub worm_shade: Color32,
    /// The lip's rim along its outer edge, brightest at the top, lit only
    /// where the sun is behind it. Lighter than `worm` and `worm_shade` in
    /// every channel: its light is added to the lip.
    pub worm_rim: Color32,
    /// The teeth of the outer ring: the deeper rings are lost toward the
    /// throat's dark.
    pub tooth: Color32,
    /// The dark of the gullet inside the lip.
    pub throat: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        window: Color32::from_rgb(0x1A, 0x0C, 0x07),
        panel: Color32::from_rgb(0x22, 0x11, 0x09),
        surface: Color32::from_rgb(0x2C, 0x17, 0x0D),
        surface_hover: Color32::from_rgb(0x37, 0x1D, 0x11),
        surface_active: Color32::from_rgb(0x43, 0x24, 0x15),
        outline: Color32::from_rgb(0x3B, 0x20, 0x13),
        field_stroke: Color32::from_rgb(0x66, 0x41, 0x2B),
        text: Color32::from_rgb(0xFF, 0xEE, 0xE2),
        secondary: Color32::from_rgb(0xE3, 0xBF, 0xA6),
        faint: Color32::from_rgb(0xC5, 0x9C, 0x82),
        dim: Color32::from_rgb(0xA0, 0x78, 0x60),
        accent: Color32::from_rgb(0xFF, 0xB3, 0x47),
        accent_hover: Color32::from_rgb(0xFF, 0xC5, 0x6E),
        on_accent: Color32::from_rgb(0x1F, 0x10, 0x04),
        selection: Color32::from_rgb(0x6E, 0x3F, 0x1C),
        knob: Color32::WHITE,
        track: Color32::from_rgba_premultiplied(52, 52, 52, 52),
        wash: Color32::from_rgb(0xF0, 0x8A, 0x4B),
        cover: Cover {
            sky_top: Color32::from_rgb(0x3A, 0x16, 0x0C),
            sky_low: Color32::from_rgb(0xE8, 0x83, 0x4A),
            sun: Color32::from_rgb(0xFF, 0xD9, 0xA0),
            far: Color32::from_rgb(0x7A, 0x35, 0x18),
            lee: Color32::from_rgb(0x58, 0x24, 0x0F),
            near: Color32::from_rgb(0x46, 0x20, 0x0F),
            worm: Color32::from_rgb(0x3F, 0x1D, 0x0E),
            worm_shade: Color32::from_rgb(0x24, 0x10, 0x0A),
            worm_rim: Color32::from_rgb(0x6C, 0x3A, 0x1E),
            tooth: Color32::from_rgb(0xBE, 0x9A, 0x76),
            throat: Color32::from_rgb(0x17, 0x09, 0x05),
        },
    };
}

impl Palette {
    /// `color` as a slider's strip shows it past the value: moved toward the
    /// window by [`SLIDER_VEIL`].
    pub fn veiled(&self, color: Color32) -> Color32 {
        color.lerp_to_gamma(self.window, SLIDER_VEIL)
    }
}

/// The strip at the top under the system's traffic lights: it drags the
/// window, and the lights sit centered in it, as far from the top as the
/// first one is from the left.
pub const TITLE_BAR: f32 = 44.0;
pub const WINDOW: Vec2 = vec2(420.0, 562.0);
pub const MARGIN: f32 = 24.0;
pub const RADIUS: u8 = 8;
/// How far the now-playing card moves from the surface to the cover's
/// color.
pub const CARD_WASH: f32 = 0.12;
pub const SWITCH: Vec2 = vec2(40.0, 22.0);
/// A time field, and the stepper beside it.
pub const FIELD_HEIGHT: f32 = 30.0;
pub const STEPPER: Vec2 = vec2(18.0, FIELD_HEIGHT);
/// The stepper is a pill: its corner, and its lit half's, one point in.
pub const STEPPER_RADIUS: u8 = 9;
/// Each part of a time stands in a cell of its own, inside its field and
/// `CELL_INSET` in from the field's top and foot, so its height is the
/// field's less the two.
pub const CELL_INSET: f32 = 3.0;
pub const CELL_HEIGHT: f32 = FIELD_HEIGHT - 2.0 * CELL_INSET;
/// The room on each side of a cell's digits.
pub const CELL_PAD: f32 = 6.0;
pub const CELL_RADIUS: u8 = 4;
/// How far above the field's middle line a cell's digits, the colon and the
/// unit are centered.
pub const CELL_LIFT: f32 = 1.0;
/// What parts two cells, beyond the colon's width: the colon stands in it.
pub const COLON_GAP: f32 = 4.0;
/// From a cell to the unit that follows it.
pub const UNIT_GAP: f32 = 5.0;
/// Red Now and Close are round keys this wide, with their names beside them.
pub const KEY: f32 = 32.0;
/// From a key to its name.
pub const KEY_GAP: f32 = 8.0;
/// A key's hit area runs this far past its name's far end.
pub const KEY_TAIL: f32 = 4.0;
/// Close's cross: how far across it runs, and its strokes' width.
pub const CROSS: f32 = 9.0;
pub const CROSS_STROKE: f32 = 1.7;
/// The sun of Red Now's glyph, the upper half of a disc: its radius, and how
/// far its flat side lies below the key's center.
pub const SUNDOWN_RADIUS: f32 = 4.6;
pub const SUNDOWN_SIDE: f32 = 2.5;
/// The line under the sun: how long and how wide it is, and how far its middle
/// lies below the sun's flat side.
pub const SUNDOWN_LINE: f32 = 16.0;
pub const SUNDOWN_LINE_WIDTH: f32 = 1.5;
pub const SUNDOWN_LINE_DROP: f32 = 1.6;
pub const BAR_HEIGHT: f32 = 60.0;
/// A slider's strip: its height, with round ends of half of it.
pub const SLIDER_TRACK: f32 = 6.0;
/// A slider's knob, ring included.
pub const SLIDER_KNOB: f32 = 14.0;
/// The knob's dark ring.
pub const KNOB_RING: f32 = 1.5;
/// How far the part of a slider's strip past its value moves from its colors
/// toward the window.
pub const SLIDER_VEIL: f32 = 0.6;
/// A stop's tick under a slider's strip: how wide it is, how tall, and how far
/// its top hangs below the strip's center line.
pub const SLIDER_TICK_WIDTH: f32 = 1.0;
pub const SLIDER_TICK_HEIGHT: f32 = 3.0;
pub const SLIDER_TICK_GAP: f32 = 5.0;
/// From a slider's center line to the center of the names under its strip.
pub const SLIDER_NAME_GAP: f32 = 18.0;

pub fn regular(size: f32) -> FontId {
    Weight::Regular.font_id(size)
}

pub fn medium(size: f32) -> FontId {
    Weight::Medium.font_id(size)
}

pub fn semibold(size: f32) -> FontId {
    Weight::SemiBold.font_id(size)
}

pub fn bold(size: f32) -> FontId {
    Weight::Bold.font_id(size)
}

/// The families the window draws with that `fonts` gives no face: Proportional,
/// Monospace and the three named weights. On macOS the build carries no font
/// of its own (neither Inter nor egui's), so when the Mac's San Francisco
/// cannot be found or read, every one of them is empty.
fn families_without_a_face(fonts: &egui::FontDefinitions) -> Vec<egui::FontFamily> {
    let used = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
        .into_iter()
        .chain(Weight::ALL.map(Weight::family));
    let mut empty = Vec::new();
    for family in used {
        let has_a_face = fonts
            .families
            .get(&family)
            .is_some_and(|faces| !faces.is_empty());
        if !has_a_face && !empty.contains(&family) {
            empty.push(family);
        }
    }
    empty
}

/// San Francisco, and dark whatever the system is. On macOS the build carries
/// no font of its own (neither Inter nor egui's), so both families,
/// Proportional and Monospace, are San Francisco; if the Mac cannot give it,
/// the window draws no text, and this logs that as an error.
///
/// It also turns off egui's keyboard zoom (Cmd+Plus, Cmd+Minus, Cmd+0). The
/// window is a fixed size, so a zoom only crops it: above about 2.1 the
/// pointer cannot reach the Temperature slider, and the steps from 1 to 5
/// made the font atlas again 7 times, the first past zoom 2.6
/// (`MAX_TEXTURE_SIDE` in `src/ui/mod.rs`). Every window gets a new
/// `Context`, and each runs this, so it holds for a reopened one too.
pub fn install(ctx: &egui::Context) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    let fonts = fastframe_fonts::FontSetup::default()
        .primary(Primary::System)
        .monospace(Monospace::Inter)
        .system_fallbacks(false)
        .definitions();
    let without_a_face = families_without_a_face(&fonts);
    if !without_a_face.is_empty() {
        // `fastframe-fonts` logs "Inter stays" at info level: it does not,
        // this build has no Inter.
        log::error!(
            "no system interface font (San Francisco) found: the window will draw no text \
             ({without_a_face:?} have no face)"
        );
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    let palette = Palette::DARK;
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals = egui::Visuals::dark();
        let visuals = &mut style.visuals;
        visuals.panel_fill = palette.window;
        visuals.window_fill = palette.surface;
        visuals.window_stroke = egui::Stroke::new(1.0, palette.outline);
        visuals.window_corner_radius = egui::CornerRadius::same(RADIUS);
        visuals.override_text_color = Some(palette.text);
        visuals.selection.bg_fill = palette.selection;
        visuals.selection.stroke = egui::Stroke::new(1.0, palette.accent);
        style.interaction.tooltip_delay = 0.5;
        for text_style in [egui::TextStyle::Body, egui::TextStyle::Small] {
            style.text_styles.insert(text_style, regular(12.5));
        }
    });
}

/// The contrast of two colors as the red channel alone gives it: what is
/// left of them through a pure red tint.
#[cfg(test)]
pub(crate) fn red_contrast(a: Color32, b: Color32) -> f32 {
    let [a, b] = [a, b].map(|color| egui::Rgba::from(color).r() + 0.05);
    a.max(b) / a.min(b)
}

/// Runs `run` in a frame on a context with the window's own fonts: egui makes
/// the fonts of `set_fonts` live at the start of a frame. The tests of this
/// module and of `ui::view` lay text out with it.
#[cfg(test)]
pub(crate) fn in_a_frame<T>(run: impl FnOnce(&mut egui::Ui) -> T) -> T {
    let ctx = egui::Context::default();
    install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, WINDOW)),
        ..Default::default()
    };
    let (mut run, mut ran) = (Some(run), None);
    let mut output = ctx.run_ui(input, |ui| {
        ran = run.take().map(|run| run(ui));
    });
    // No renderer here to take the font atlas.
    output.textures_delta.clear();
    ran.expect("a frame ran")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::cover;
    use crate::ui::view::tests::{drawn_lines, fixed_lines};

    /// Text reads at 4.5:1, quiet text and the parts of a control at 3:1,
    /// and what marks a choice or an edge at 1.8:1, all in the red channel
    /// alone.
    const TEXT: f32 = 4.5;
    const QUIET: f32 = 3.0;
    const MARK: f32 = 1.8;

    /// An ink and a ground, and the least contrast they may have.
    type Pair = (&'static str, Color32, Color32, f32);

    /// The app's icon (`app_icon`) is the cover's sun and dunes through the
    /// night tint, with the sun in the accent, outside it. The tint's red
    /// multiplier is 1, so the icon's red channel is the cover's, and these
    /// pairs check that it stays so. The icon exists in the demo build only.
    #[cfg(feature = "demo")]
    fn icon_pairs(p: &Palette) -> Vec<Pair> {
        use crate::app_icon::{NIGHT, SUN_DROP};
        use crate::ui::cover::shapes::{DUNES, GROUND};
        use crate::ui::cover::{SUN, SUN_CENTER};

        /// An exception to `MARK`, by the owner's choice. In red alone the
        /// sun's lowest visible rim against the sky is 1.43:1, under the
        /// 1.8:1; the owner chose the icon on 2026-10-23, with the sun set
        /// that low, and the picture does not change for this. The master is
        /// that icon with the sun's rim smoothed once, as the cover's is (2004 of
        /// 1,048,576 pixels differ, by at most 20 of 255, all within 1 px of
        /// the sun's edge). The pair holds the measured ratio instead, with a
        /// floor just under it: the rim fails here if it gets worse.
        const RIM: f32 = 1.42;

        let c = p.cover;
        let night = NIGHT.tint();
        assert!(
            (night.red - 1.0).abs() < 1e-3,
            "the night tint dims the red channel by {}",
            night.red
        );
        let seen = |color| night.apply(color);
        let icon_sky =
            |row: f32| seen(c.sky_top).lerp_to_gamma(seen(c.sky_low), (row / GROUND).min(1.0));
        // The sun is set `SUN_DROP` lower than on the cover.
        let sun = SUN_CENTER + egui::vec2(0.0, SUN_DROP);
        // The lowest point of the sun's edge that no dune hides.
        let rim = (0..3600)
            .map(|tenth| {
                let angle = (tenth as f32 / 10.0).to_radians();
                sun + SUN * egui::vec2(angle.cos(), angle.sin())
            })
            .filter(|edge| DUNES.iter().all(|dune| !dune.covers(*edge, 0.0)))
            .map(|edge| edge.y)
            .fold(f32::MIN, f32::max);
        vec![
            ("icon far dune", seen(c.far), icon_sky(0.7 * GROUND), MARK),
            ("icon near dune", seen(c.near), seen(c.far), MARK),
            // The sun's center row.
            ("icon sun", p.accent, icon_sky(sun.y), MARK),
            ("icon sun's low rim", p.accent, icon_sky(rim), RIM),
        ]
    }

    #[cfg(not(feature = "demo"))]
    fn icon_pairs(_: &Palette) -> Vec<Pair> {
        Vec::new()
    }

    #[test]
    fn the_palette_reads_through_red() {
        // The two pairs of the window under 1.8:1, each with a floor of its
        // own. The first is by the owner's choice: see "the lip's inner edge
        // on the throat" below. It measures 1.15:1.
        const SINKS: f32 = 1.1;
        // The second is a disabled time field's edge, `outline` on `window`
        // (1.55:1), as the field's was before the cells. A disabled control
        // says that it is out of use by being quiet: its digits stay readable
        // (`dim` on the disabled fill, 6.01:1) and it takes no press, so its
        // edge need not be a mark of 1.8:1.
        const DISABLED_EDGE: f32 = 1.5;
        let p = Palette::DARK;
        // What a disabled cell and a disabled stepper are filled with: halfway
        // from `surface` to `window`.
        let disabled = crate::ui::paint::mix(p.surface, p.window, 0.5);
        let card = p.surface.lerp_to_gamma(p.wash, CARD_WASH);
        let c = p.cover;
        // The sky behind the crest of the far dune.
        let sky = c.sky_top.lerp_to_gamma(c.sky_low, 0.7);
        // The sky at the head's height, which the lip's sides meet at pure
        // red: the highest sky the lip meets.
        let lip_sky = cover::sky(&c, cover::HEAD.y);
        // The Temperature strip's lit color is any point of the scale: all
        // have red 255, so they read alike in the red channel.
        for redness in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            assert_eq!(
                crate::color::scale_tint(redness).apply(Color32::WHITE).r(),
                255,
                "the scale at {redness} has lost red"
            );
        }
        let lit = Color32::from_rgb(0xFF, 0, 0);
        let veiled = p.veiled(lit);
        let pairs = [
            ("text", p.text, p.window, TEXT),
            ("text", p.text, p.surface, TEXT),
            (
                "faint on window (the README's plate of figures)",
                p.faint,
                p.window,
                TEXT,
            ),
            ("secondary", p.secondary, p.surface, TEXT),
            ("dim", p.dim, p.window, QUIET),
            ("card title", p.text, card, TEXT),
            ("card state", p.secondary, card, TEXT),
            ("card detail", p.faint, card, QUIET),
            ("on accent", p.on_accent, p.accent, TEXT),
            ("Red Now on", p.window, p.text, TEXT),
            ("switch", p.accent, p.surface_active, QUIET),
            ("knob ring", p.on_accent, p.accent, QUIET),
            ("focus", p.accent, p.window, QUIET),
            ("selection", p.text, p.selection, TEXT),
            // The time fields: each part of a time in a cell, with a stepper
            // beside them. A selected cell is `selection` whole, parted from
            // the next cell by the window's own ground, so it meets `window`
            // and not `surface`. The colon and Fade's unit stand on `window`
            // between and after the cells; the digits meet only their cell's
            // fill, and the stepper's chevron only its half's.
            ("selected cell", p.selection, p.window, MARK),
            ("colon and unit", p.secondary, p.window, QUIET),
            // `text` on a hovered fill holds the digits of a hovered cell, a
            // lit half's chevron, and the glyph of a hovered or pressed key.
            ("on a hovered fill", p.text, p.surface_hover, TEXT),
            ("chevron on a pressed half", p.text, p.surface_active, MARK),
            ("disabled digits", p.dim, disabled, TEXT),
            ("disabled colon, unit and name", p.dim, p.window, TEXT),
            ("disabled edge", p.outline, p.window, DISABLED_EDGE),
            ("field edge", p.field_stroke, p.window, MARK),
            // The keys: the names stand on the bar, and so does Red Now's key
            // when it is on. Close's key is gold with its cross on it.
            ("names and Red Now on", p.text, p.panel, TEXT),
            ("Close key", p.accent, p.panel, MARK),
            ("Close key hovered", p.accent_hover, p.panel, MARK),
            ("Close cross hovered", p.on_accent, p.accent_hover, MARK),
            ("focus on the bar", p.accent, p.panel, QUIET),
            ("Red Now edge", p.field_stroke, p.panel, MARK),
            ("progress track", p.window.blend(p.track), p.window, MARK),
            // A slider's strip: the lit part meets the veiled part at the
            // value, and the veiled part meets the window along its sides
            // and at its far end.
            ("lit strip on veiled strip", lit, veiled, QUIET),
            ("veiled strip on window", veiled, p.window, MARK),
            // The knob's ring meets the strip all round the knob, lit on one
            // side of the value and veiled on the other, and the white meets
            // the ring inside it.
            ("knob ring on lit strip", p.on_accent, lit, QUIET),
            ("knob ring on veiled strip", p.on_accent, veiled, QUIET),
            ("knob on its ring", p.knob, p.on_accent, QUIET),
            // The stop the value sits on, in ink, beside the dim ones.
            ("lit stop name on a dim one", p.text, p.dim, MARK),
            ("far dune", c.far, sky, MARK),
            ("near dune", c.near, c.far, MARK),
            // The maw may merge with the sand it comes out of, the dunes, but
            // reads against the sun and the sky behind it at every stage of
            // the rise (the owner's choices, 2026-10-23). Its lip is lightest
            // on its rim, which is lit only where the sun stands behind it;
            // where the sky is behind it, the lip is its plain color, and the
            // highest sky it meets is at its sides at pure red.
            ("worm's rim on the sun", c.worm_rim, c.sun, TEXT),
            ("worm on the sky", c.worm, lip_sky, TEXT),
            ("teeth", c.tooth, c.throat, TEXT),
            // The outer ring of teeth meets the lip's inner edge too, and the
            // deeper rings are lost in the throat's dark on purpose, so
            // only the outer one is paired.
            ("teeth on the lip's inner edge", c.tooth, c.worm_shade, TEXT),
            // The exception: the lip's inner edge sinks into the throat's
            // dark. The owner chose the sand tone on 2026-10-23: no dark
            // outline, so the lip is shaded in to `worm_shade`, which is
            // 1.15:1 against the throat in red. It stands apart from the
            // throat by the teeth between them, not by this pair.
            (
                "lip's inner edge on the throat",
                c.worm_shade,
                c.throat,
                SINKS,
            ),
        ];
        for (name, ink, ground, least) in pairs.into_iter().chain(icon_pairs(&p)) {
            let contrast = red_contrast(ink, ground);
            assert!(
                contrast >= least,
                "{name}: {ink:?} on {ground:?} is {contrast:.2}:1, under {least}:1"
            );
        }
    }

    /// The cover is lit from behind: in the red channel alone its parts step
    /// down from the sun to the worm, as `Cover` states.
    #[test]
    fn the_cover_steps_down_in_red_from_the_sun_to_the_worm() {
        let c = Palette::DARK.cover;
        // The sky behind the crest of the far dune.
        let sky = c.sky_top.lerp_to_gamma(c.sky_low, 0.7);
        let steps = [
            ("sun", c.sun),
            ("sky", sky),
            ("far dune", c.far),
            ("near dune", c.near),
            ("worm", c.worm),
            ("worm shade", c.worm_shade),
            ("throat", c.throat),
        ];
        for pair in steps.windows(2) {
            let ((above, lighter), (below, darker)) = (pair[0], pair[1]);
            assert!(
                lighter.r() > darker.r(),
                "{above} ({}) is not lighter in red than {below} ({})",
                lighter.r(),
                darker.r()
            );
        }
        // The rim's light is added to the lip, so the rim is lighter than the
        // lip in every channel, at its outer edge and where it meets the
        // throat.
        for lip in [c.worm, c.worm_shade] {
            assert!(
                c.worm_rim.r() > lip.r() && c.worm_rim.g() > lip.g() && c.worm_rim.b() > lip.b(),
                "the rim {:?} is not lighter than the lip {lip:?}",
                c.worm_rim
            );
        }
    }

    #[test]
    fn the_veil_moves_a_color_toward_the_window() {
        let p = Palette::DARK;
        // The window is what the veil moves toward, so it stays put.
        assert_eq!(p.veiled(p.window), p.window);
        // The veil: SLIDER_VEIL of the way to the window, in sRGB.
        for color in [Color32::WHITE, Color32::from_rgb(0xFF, 0x80, 0x00)] {
            assert_eq!(p.veiled(color), color.lerp_to_gamma(p.window, SLIDER_VEIL));
        }
        // Every channel lands strictly between the color and the window,
        // whichever side of the window's it starts on.
        for color in [Color32::WHITE, Color32::from_rgb(0xFF, 0x80, 0x00)] {
            let veiled = p.veiled(color);
            for (before, after, ground) in [
                (color.r(), veiled.r(), p.window.r()),
                (color.g(), veiled.g(), p.window.g()),
                (color.b(), veiled.b(), p.window.b()),
            ] {
                let (low, high) = (before.min(ground), before.max(ground));
                assert!(
                    (low..=high).contains(&after) && after != before,
                    "{color:?} veiled is {veiled:?}: {after} is not between {before} and {ground}"
                );
            }
        }
        // White loses every channel.
        let white = p.veiled(Color32::WHITE);
        assert!(white.r() < 255 && white.g() < 255 && white.b() < 255);
    }

    /// Runs `inspect` on the fonts that a frame of the window draws with.
    fn on_the_fonts<T>(inspect: impl FnOnce(&mut egui::epaint::FontsView<'_>) -> T) -> T {
        in_a_frame(|ui| ui.ctx().fonts_mut(inspect))
    }

    /// Every family the window draws with, each defined as `faces`.
    fn every_family_as(faces: &[&str]) -> egui::FontDefinitions {
        let mut fonts = egui::FontDefinitions::empty();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
            .into_iter()
            .chain(Weight::ALL.map(Weight::family))
        {
            let faces = faces.iter().map(|face| (*face).to_owned()).collect();
            fonts.families.insert(family, faces);
        }
        fonts
    }

    #[test]
    fn a_family_with_no_face_is_reported() {
        let every_family = vec![
            egui::FontFamily::Proportional,
            egui::FontFamily::Monospace,
            Weight::Medium.family(),
            Weight::SemiBold.family(),
            Weight::Bold.family(),
        ];
        // What `fastframe-fonts` builds when the Mac has no interface face to
        // find: every family there, and every one empty.
        assert_eq!(families_without_a_face(&every_family_as(&[])), every_family);
        // No family defined at all is the same case.
        assert_eq!(
            families_without_a_face(&egui::FontDefinitions::empty()),
            every_family
        );
        // A face in each: nothing to report, until one family loses it.
        let mut fonts = every_family_as(&["face"]);
        assert_eq!(families_without_a_face(&fonts), []);
        fonts
            .families
            .insert(egui::FontFamily::Monospace, Vec::new());
        assert_eq!(
            families_without_a_face(&fonts),
            [egui::FontFamily::Monospace]
        );
    }

    #[test]
    fn install_gives_every_family_a_face() {
        // The fonts `install` builds, as the frame holds them. On a Mac that
        // cannot give San Francisco this fails, as `install` logs an error.
        let without = on_the_fonts(|fonts| families_without_a_face(fonts.definitions()));
        assert!(without.is_empty(), "{without:?} have no face to draw with");
    }

    #[test]
    fn every_family_is_the_interface_face_alone() {
        let (families, font_data) = on_the_fonts(|fonts| {
            let definitions = fonts.definitions();
            (
                definitions.families.clone(),
                definitions.font_data.keys().cloned().collect::<Vec<_>>(),
            )
        });
        // The interface face is registered once a weight, under the name
        // `fastframe_fonts` gives that weight; nothing else may be.
        let faces = Weight::ALL.map(Weight::name);
        for (family, listed) in &families {
            for face in listed {
                assert!(faces.contains(&face.as_str()), "{family:?} lists {face}");
            }
        }
        for face in &font_data {
            assert!(faces.contains(&face.as_str()), "{face} is embedded");
        }
        // Proportional is regular, each named weight has its own family, and
        // Monospace draws with the regular face: every one of them has a face
        // to draw with, and it leads.
        let mut weights = Weight::ALL
            .map(|weight| (weight.family(), weight.name()))
            .to_vec();
        weights.push((egui::FontFamily::Monospace, Weight::Regular.name()));
        for (family, face) in weights {
            let listed = families.get(&family).map(Vec::as_slice).unwrap_or_default();
            assert_eq!(listed.first().map(String::as_str), Some(face), "{family:?}");
        }
    }

    #[test]
    fn monospace_draws_with_the_regular_face() {
        let text = "Red until 23:30".to_owned();
        let (proportional, monospace) = on_the_fonts(|fonts| {
            let mut width = |family| {
                let font = FontId::new(12.5, family);
                fonts
                    .layout_no_wrap(text.clone(), font, Color32::WHITE)
                    .size()
                    .x
            };
            (
                width(egui::FontFamily::Proportional),
                width(egui::FontFamily::Monospace),
            )
        });
        assert!(monospace > 0.0, "Monospace lays out {text:?} with no width");
        // It is the same face, not a second one.
        assert_eq!(monospace, proportional);
    }

    /// What of the window's copy changes at run time: the figures of a time, a
    /// temperature, a percentage and a count of displays, the colon of a
    /// time, the middle dot between the parts of the detail line, and the
    /// ellipsis that egui puts where a line is cut (`ui::paint::fit`).
    const CHANGES: &str = "0123456789:\u{b7}\u{2026}";

    /// Each character of `lines` that the font it is drawn in has no glyph
    /// for, as "'c' in Family", once each.
    fn missing_glyphs(lines: &[(String, FontId)]) -> Vec<String> {
        on_the_fonts(|view| {
            let mut missing = Vec::new();
            for (line, font) in lines {
                // Not `has_glyph`: it says no for every character when the
                // family's one face is also the one that draws the
                // missing-glyph box.
                let held = view.fonts.font(&font.family).characters().clone();
                for c in line.chars() {
                    let gap = format!("{c:?} in {:?}", font.family);
                    let drawn = held.contains_key(&c) && view.glyph_width(font, c) > 0.0;
                    if !drawn && !missing.contains(&gap) {
                        missing.push(gap);
                    }
                }
            }
            missing
        })
    }

    #[test]
    fn every_glyph_the_window_draws_is_in_the_interface_face() {
        // With no font of egui's or Inter's, nothing falls back to draw a
        // glyph the interface face lacks. The card's fixed lines come from
        // the view's own list, each in the font it is drawn in; what changes
        // at run time is checked in every weight.
        let mut lines = fixed_lines();
        lines
            .extend([regular, medium, semibold, bold].map(|font| (CHANGES.to_owned(), font(14.0))));
        let missing = missing_glyphs(&lines);
        assert!(missing.is_empty(), "no glyph for {}", missing.join(", "));
    }

    #[test]
    fn every_glyph_the_sliders_draw_is_in_the_interface_face() {
        // The texts of the frames the view draws at looks on each stop of the
        // Temperature and at each end of the Brightness: the stops' and the
        // ends' names (in `regular(11.5)`), both readouts (`semibold(14.0)`)
        // and the labels (`medium(14.0)`), each in the font it is drawn in.
        // A renamed stop or a changed readout is checked as it is drawn.
        let lines = drawn_lines();
        assert!(!lines.is_empty());
        let missing = missing_glyphs(&lines);
        assert!(missing.is_empty(), "no glyph for {}", missing.join(", "));
    }

    #[test]
    fn a_glyph_the_interface_face_lacks_is_reported() {
        // U+10FFFD, the last code point of the Private Use Area, is in no
        // interface face (U+E000, its first, is in Inter). Adding it to the
        // copy fails the test above for all four families.
        let lines =
            [regular, medium, semibold, bold].map(|font| ("Red \u{10fffd}".to_owned(), font(14.0)));
        let missing = missing_glyphs(&lines);
        assert_eq!(missing.len(), 4, "{missing:?}");
        assert!(
            missing
                .iter()
                .all(|gap| gap.starts_with("'\\u{10fffd}' in"))
        );
    }

    /// A dependency's feature or a new line in `Cargo.toml` can switch
    /// egui's four fonts back on through feature unification (`default_fonts`):
    /// they are in the default definitions when it is on.
    #[test]
    fn egui_has_no_font_of_its_own() {
        let fonts = egui::FontDefinitions::default();
        let names: Vec<_> = fonts.font_data.keys().collect();
        assert!(names.is_empty(), "egui embeds {names:?} again");
    }

    /// The same for Inter's 880 KB (`inter`): the default face is Inter when
    /// the feature is on. Only on macOS, where San Francisco draws: the
    /// other targets keep the feature (`Cargo.toml`), for they have no
    /// San Francisco and Inter is their face.
    #[cfg(target_os = "macos")]
    #[test]
    fn fastframe_fonts_has_no_inter() {
        assert_eq!(Primary::default(), Primary::System);
    }
}
