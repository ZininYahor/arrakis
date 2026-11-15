//! The cover's counters are process-wide statics. This is the only test in
//! its binary, so what they count between two reads is its own doing.

mod common;

use std::collections::HashMap;

use arrakis::color::{DAY_KELVIN, Look, PURE_RED_KELVIN};
use arrakis::model::Action;
use arrakis::probe;
use arrakis::settings::Settings;
use arrakis::ui::cover::counts;
use common::Window;
use egui::{Color32, ColorImage};

/// The partial set that a move of a box sends, from its image composed for
/// the old rise to the one for the new: as wide as the box, and of the rows
/// from the first that is not clear in either image to the last. None when
/// neither image has a row that is not clear.
fn set_between(old: &ColorImage, new: &ColorImage) -> Option<[usize; 2]> {
    let width = new.size[0];
    let lit_rows = |image: &ColorImage| {
        let lit: Vec<usize> = (image.pixels.chunks(width).enumerate())
            .filter(|(_, row)| row.iter().any(|pixel| *pixel != Color32::TRANSPARENT))
            .map(|(y, _)| y)
            .collect();
        lit.first()
            .zip(lit.last())
            .map(|(first, last)| (*first, *last))
    };
    [lit_rows(old), lit_rows(new)]
        .into_iter()
        .flatten()
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        .map(|(first, last)| [width, last + 1 - first])
}

/// The bytes of a set of `size`, four to a pixel.
fn bytes_of([width, height]: [usize; 2]) -> u64 {
    4 * (width * height) as u64
}

#[test]
fn the_cover_is_built_and_uploaded_once_and_the_temperature_sends_only_the_rims_light_and_the_shade()
 {
    probe::set_clock(12.0 * 60.0);
    let start = counts::read();
    let night = Settings::default().night.kelvin;
    let mut window = Window::new(probe::app(Settings::default()));

    // The first frame builds the layers and sends the cover's six images:
    // the back, 272 pixels square at 2x, the boxes of the maw, the gap and
    // the front, which are smaller, and the rim's light and the shade at the
    // sand, each in its box. The bytes
    // are exactly theirs. It draws the maw at the rise of the night look it
    // starts with, where the rim's light and the shade were composed for.
    let (mut drawn, first_sets) = window.frame_sets(Vec::new());
    assert!(drawn.is_some(), "the night look stirs the maw");
    let first = counts::read();
    assert_eq!(first.layer_builds - start.layer_builds, 1);
    assert_eq!(first.rim_composes, start.rim_composes);
    assert_eq!(first.shade_composes, start.shade_composes);
    let uploaded = first.uploaded_bytes - start.uploaded_bytes;
    assert_eq!(uploaded, common::cover_bytes(272));
    // The six images, whole: one set for each, of its own size. The shade's
    // box is the sixth, and the one set of its size is sent whole.
    let six = probe::cover_images(probe::cover_layers(272), night);
    let mut sizes: Vec<[usize; 2]> = six.iter().map(|image| image.size).collect();
    let mut sent_whole = first_sets.clone();
    sizes.sort_unstable();
    sent_whole.sort_unstable();
    assert_eq!(sent_whole, sizes, "the first frame sends all six, whole");
    let shade_size = six[5].size;
    assert_eq!(
        first_sets.iter().filter(|set| **set == shade_size).count(),
        1,
        "the shade's box, {shade_size:?}, is sent whole, once"
    );
    // The gap and the front are cropped to the pixels they fill: with full
    // pictures the six images held three of them, the maw's box, the rim's
    // light's and the shade's.
    let picture = 4 * 272 * 272;
    assert!(uploaded > 0);
    assert!(uploaded < 3 * picture, "{uploaded} B");

    // Frames that change nothing cost the cover nothing.
    for _ in 0..3 {
        window.frame(Vec::new());
    }
    assert_eq!(counts::read(), first);

    // A new Temperature where the maw is under the sand: none of it is
    // drawn, so nothing is built, composed or sent to the GPU.
    let day = Look {
        kelvin: DAY_KELVIN,
        ..window.app.settings.night
    };
    assert_ne!(day, window.app.settings.night);
    window.app.apply(
        Action::SetNight {
            look: day,
            dragging: false,
        },
        None,
    );
    window.frame(Vec::new());
    assert_eq!(counts::read(), first);

    // A drag from pure red to the day and back: 30 frames, the slider at a
    // new Temperature in each. No frame builds the layers again. A frame
    // whose maw stands at a new rise composes the rim's light once, and the
    // shade once where the new rise moves its picture, and each compose sends
    // the rows of its box from the first that holds something in the old
    // picture or the new to the last, at most the whole box; and a second
    // frame at the same rise composes and sends nothing.
    //
    // A rise is new when the maw is drawn at a place other than the one it
    // was last drawn at: its rect moves with each thousandth of the rise, and
    // by more than a float's step. A frame in the day draws no maw and leaves
    // the place it was last drawn at, so a drag back from the day to that
    // rise composes nothing.
    let span = u32::from(DAY_KELVIN - PURE_RED_KELVIN);
    let kelvins: Vec<u16> = (0..30u32)
        .map(|step| {
            let along = if step < 15 { step } else { 29 - step };
            PURE_RED_KELVIN + (span * along / 14) as u16
        })
        .collect();
    // The rim's light and the shade for each Temperature of the drag, and for
    // the night look it starts from, composed from one build of the layers.
    let wanted: Vec<u16> = std::iter::once(night)
        .chain(kelvins.iter().copied())
        .collect();
    let composed = probe::cover_rim_and_shade(probe::cover_layers(272), &wanted);
    let boxes: HashMap<u16, [ColorImage; 2]> = wanted.into_iter().zip(composed).collect();
    let [rim_width, shade_width] = [0, 1].map(|which| boxes[&night][which].size[0]);
    assert_ne!(
        rim_width, shade_width,
        "the two boxes are told apart by width"
    );
    let [rim_box, shade_box] = [common::rim_bytes(272), common::shade_bytes(272)];

    let mut drawn_at = night;
    let (mut new_rises, mut rim_in_all, mut shade_in_all) = (0, 0, 0);
    let mut last = first;
    for (step, &kelvin) in kelvins.iter().enumerate() {
        let look = Look {
            kelvin,
            ..window.app.settings.night
        };
        window.app.apply(
            Action::SetNight {
                look,
                dragging: true,
            },
            None,
        );
        let (maw, sets) = window.frame_sets(Vec::new());
        let sent: u64 = sets.iter().copied().map(bytes_of).sum();
        assert_eq!(window.app.settings.night.kelvin, kelvin, "step {step}");
        let now = counts::read();
        assert_eq!(now.layer_builds, first.layer_builds, "step {step}");
        let composes = now.rim_composes - last.rim_composes;
        assert!(composes <= 1, "step {step}, {kelvin} K: {composes}");
        // Exactly one compose for a new rise, and none for the rest.
        let new_rise = maw.is_some() && maw != drawn;
        assert_eq!(
            composes,
            u64::from(new_rise),
            "step {step}, {kelvin} K: the maw at {maw:?}, last drawn at {drawn:?}"
        );
        // The shade is composed at most once at each of those, and at no
        // other: a new rise that slides the teeth as the last did leaves its
        // picture as it was, and composes none of it. One that moves its
        // picture composes it.
        let (old, new) = (&boxes[&drawn_at], &boxes[&kelvin]);
        let shade_composed = now.shade_composes - last.shade_composes;
        assert!(
            shade_composed <= u64::from(new_rise),
            "step {step}, {kelvin} K: the shade, {shade_composed} composes"
        );
        if new_rise && old[1] != new[1] {
            assert_eq!(shade_composed, 1, "step {step}, {kelvin} K: the shade");
        }
        new_rises += u64::from(new_rise);
        drawn = maw.or(drawn);

        // The bytes counted are exactly those the frame sent: none without
        // a compose, and at most the light's box and the shade's with one.
        let at = format!("step {step}, {kelvin} K: {sent} B");
        assert_eq!(now.uploaded_bytes - last.uploaded_bytes, sent, "{at}");
        assert!(sent <= composes * (rim_box + shade_box), "{at}");

        // And each box composed sends the rows from the first that holds
        // some at the rise it was composed for before or the one it is
        // composed for now, to the last, as one partial set; nothing of a box
        // that neither rise holds anything in, or that is not composed; and
        // no other texture is set. The shade's picture at the rise last drawn
        // is the one it last composed.
        let composed = [new_rise, shade_composed == 1];
        let wanted = |which: usize| match composed[which] {
            true => set_between(&old[which], &new[which]),
            false => None,
        };
        let of_width = |width: usize| -> Vec<[usize; 2]> {
            (sets.iter().copied())
                .filter(|set| set[0] == width)
                .collect()
        };
        let [rim_sets, shade_sets] = [rim_width, shade_width].map(of_width);
        assert_eq!(
            rim_sets,
            wanted(0).into_iter().collect::<Vec<_>>(),
            "{at}: the rim's light"
        );
        assert_eq!(
            shade_sets,
            wanted(1).into_iter().collect::<Vec<_>>(),
            "{at}: the shade"
        );
        assert_eq!(
            sets.len(),
            rim_sets.len() + shade_sets.len(),
            "{at}: no other set"
        );
        rim_in_all += rim_sets.iter().copied().map(bytes_of).sum::<u64>();
        shade_in_all += shade_sets.iter().copied().map(bytes_of).sum::<u64>();
        if new_rise {
            drawn_at = kelvin;
        }

        // A second frame at the same rise composes and sends nothing.
        let (_, again) = window.frame_sets(Vec::new());
        assert!(
            again.is_empty(),
            "step {step}, {kelvin} K, again: {again:?}"
        );
        assert_eq!(counts::read(), now, "step {step}, {kelvin} K, again");
        last = now;
    }
    // The maw rose and sank through the drag: the light was composed for
    // each frame where it stood at a new rise, exactly as often, and the
    // shade at most as often; neither for those where it was under the sand
    // or back at the rise last drawn.
    let composes = last.rim_composes - first.rim_composes;
    assert_eq!(composes, new_rises);
    let shade_composes = last.shade_composes - first.shade_composes;
    assert!(
        0 < shade_composes && shade_composes <= new_rises,
        "{shade_composes} of {new_rises}"
    );
    assert!(0 < new_rises, "the drag moved the maw");
    // A compose sends only the rows that hold something in the old picture
    // or the new: less than a box a compose, of each.
    assert!(0 < rim_in_all, "the drag sent the light");
    assert!(0 < shade_in_all, "the drag sent the shade");
    assert!(rim_in_all < composes * rim_box, "{rim_in_all} B of light");
    assert!(
        shade_in_all < shade_composes * shade_box,
        "{shade_in_all} B of shade"
    );
    assert_eq!(
        last.uploaded_bytes - first.uploaded_bytes,
        rim_in_all + shade_in_all
    );
    // The drag covered the whole slider, both ways, in steps.
    assert_eq!(kelvins.first(), Some(&PURE_RED_KELVIN));
    assert_eq!(kelvins.iter().max(), Some(&DAY_KELVIN));
    assert_eq!(kelvins.last(), Some(&PURE_RED_KELVIN));
    assert!(kelvins[..15].windows(2).all(|pair| pair[0] < pair[1]));
}
