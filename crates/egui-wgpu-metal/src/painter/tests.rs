//! The painter on real Metal, with no window: `Painter::start`, then `render` into the first target
//! and `read_screen`. Metal itself is not mocked. A test that needs a device says so and skips only
//! where `MTLCopyAllDevices` is empty, as on a VM. Nothing here opens a window. The tests of the swap
//! set a layer of their own, in no view and on no screen, to read what each frame gives it.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::time::Duration;

use egui::epaint::{ImageDelta, Mesh, Vertex, WHITE_UV};
use egui::{Pos2, pos2, vec2};
use objc2_io_surface::IOSurfacePurgeabilityState;
use objc2_metal::{MTLCommandBufferStatus, MTLCopyAllDevices};

use super::*;
use crate::shader_stamp as stamp;

/// Where this Mac has no Metal device, a test that needs one says so and passes without it.
fn has_a_device() -> bool {
    let any = MTLCopyAllDevices().count() > 0;
    if !any {
        eprintln!("skipped: MTLCopyAllDevices is empty, so there is no Metal device here");
    }
    any
}

/// A flag of the test's own, as the process's would stop every other test's painter. For the process's
/// life, as Metal's handlers may outlive the test.
fn own_flag() -> &'static Flag {
    Box::leak(Box::default())
}

fn painter(dithering: bool) -> Option<Painter> {
    has_a_device().then(|| Painter::start(Context::default(), dithering, own_flag()))
}

fn device() -> Option<Retained<Device>> {
    has_a_device().then(|| MTLCreateSystemDefaultDevice().expect("a device"))
}

/// What a frame is cleared to: 51 in each channel.
const CLEAR: [f32; 4] = [0.2, 0.2, 0.2, 1.0];
const CLEARED: Color32 = Color32::from_rgb(51, 51, 51);
/// A 1 by 1 white texture, which solid meshes sample.
const WHITE: TextureId = TextureId::Managed(0);
const IMAGE: TextureId = TextureId::Managed(1);

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

/// A target of `size` pixels, in points.
fn target_rect([width, height]: [usize; 2], pixels_per_point: f32) -> Rect {
    Rect::from_min_size(
        Pos2::ZERO,
        vec2(
            width as f32 / pixels_per_point,
            height as f32 / pixels_per_point,
        ),
    )
}

/// A delta that clears itself when it goes. epaint's own `Drop` panics on a delta that is not empty,
/// which in a test that failed on an assertion is a panic during a panic: the whole run aborts and
/// the other tests report nothing.
#[derive(Default)]
struct Delta(TexturesDelta);

impl std::ops::Deref for Delta {
    type Target = TexturesDelta;

    fn deref(&self) -> &TexturesDelta {
        &self.0
    }
}

impl std::ops::DerefMut for Delta {
    fn deref_mut(&mut self) -> &mut TexturesDelta {
        &mut self.0
    }
}

impl Drop for Delta {
    fn drop(&mut self) {
        self.0.clear();
    }
}

fn set(delta: &mut TexturesDelta, id: TextureId, change: ImageDelta) {
    delta.set.entry(id).or_default().push(change);
}

/// A delta that sets the white texture.
fn with_white() -> Delta {
    let mut delta = Delta::default();
    let white = ColorImage::filled([1, 1], Color32::WHITE);
    set(
        &mut delta,
        WHITE,
        ImageDelta::full(white, TextureOptions::NEAREST),
    );
    delta
}

fn flat(rect: Rect, color: Color32) -> Mesh {
    let mut mesh = Mesh::with_texture(WHITE);
    mesh.add_colored_rect(rect, color);
    mesh
}

fn textured(rect: Rect, uv: Rect, id: TextureId) -> Mesh {
    let mut mesh = Mesh::with_texture(id);
    mesh.add_rect_with_uv(rect, uv, Color32::WHITE);
    mesh
}

/// A rectangle with a color at each corner, left to right, top row first.
fn gradient(rect: Rect, [tl, tr, bl, br]: [Color32; 4]) -> Mesh {
    let mut mesh = Mesh::with_texture(WHITE);
    for (pos, color) in [
        (rect.left_top(), tl),
        (rect.right_top(), tr),
        (rect.left_bottom(), bl),
        (rect.right_bottom(), br),
    ] {
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color,
        });
    }
    mesh.indices.extend([0, 1, 2, 2, 1, 3]);
    mesh
}

fn clipped(mesh: Mesh, clip_rect: Rect) -> ClippedPrimitive {
    ClippedPrimitive {
        clip_rect,
        primitive: Primitive::Mesh(mesh),
    }
}

fn px(image: &ColorImage, x: usize, y: usize) -> Color32 {
    image.pixels[y * image.size[0] + x]
}

/// `image`'s first row, in red.
fn red_row(image: &ColorImage) -> Vec<u8> {
    (0..image.size[0]).map(|x| px(image, x, 0).r()).collect()
}

// ---- scissor, a pure function ----

/// The scissor as a plain tuple: x, y, width, height.
fn scissor_of(size: [usize; 2], pixels_per_point: f32, clip: Rect) -> Option<[usize; 4]> {
    scissor(size, pixels_per_point, clip).map(|s| [s.x, s.y, s.width, s.height])
}

#[test]
fn the_scissor_is_the_clip_rect_in_pixels_at_each_scale() {
    let clip = rect(10.0, 5.0, 30.0, 20.0);
    assert_eq!(scissor_of([100, 50], 1.0, clip), Some([10, 5, 20, 15]));
    assert_eq!(scissor_of([200, 100], 2.0, clip), Some([20, 10, 40, 30]));
}

#[test]
fn the_scissor_rounds_each_edge_to_the_nearest_pixel() {
    // 0.4 and 2.6 round to 0 and 3: a floor would give 0 and 2, a ceiling 1 and 3.
    assert_eq!(
        scissor_of([100, 50], 1.0, rect(0.4, 0.4, 2.6, 2.6)),
        Some([0, 0, 3, 3])
    );
    // At 1.5 the edges 1.0 and 3.0 land on 1.5 and 4.5, which round away from zero to 2 and 5.
    assert_eq!(
        scissor_of([100, 50], 1.5, rect(1.0, 1.0, 3.0, 3.0)),
        Some([2, 2, 3, 3])
    );
}

#[test]
fn a_clip_rect_past_the_edge_of_the_target_stops_at_the_edge() {
    assert_eq!(
        scissor_of([100, 50], 1.0, rect(80.0, 40.0, 200.0, 100.0)),
        Some([80, 40, 20, 10])
    );
    assert_eq!(
        scissor_of([100, 50], 1.0, rect(-10.0, -10.0, 20.0, 20.0)),
        Some([0, 0, 20, 20])
    );
    // The scale is applied before the clamp.
    assert_eq!(
        scissor_of([100, 50], 2.0, rect(40.0, 20.0, 80.0, 40.0)),
        Some([80, 40, 20, 10])
    );
    assert_eq!(
        scissor_of([100, 50], 1.0, Rect::EVERYTHING),
        Some([0, 0, 100, 50])
    );
}

#[test]
fn an_empty_or_outside_clip_rect_has_no_scissor() {
    let size = [100, 50];
    let nothing = [
        // No area.
        rect(10.0, 5.0, 10.0, 40.0),
        rect(10.0, 5.0, 60.0, 5.0),
        // Upside down.
        rect(60.0, 45.0, 10.0, 5.0),
        // Beside the target, on each side.
        rect(100.0, 0.0, 150.0, 10.0),
        rect(200.0, 0.0, 300.0, 10.0),
        rect(0.0, 50.0, 10.0, 90.0),
        rect(-50.0, 0.0, -10.0, 10.0),
        rect(0.0, -50.0, 10.0, -10.0),
        // A rect that holds nothing, as egui's culling can leave.
        Rect::NOTHING,
        // Less than half a pixel in each direction rounds to no area.
        rect(10.1, 10.1, 10.4, 10.4),
    ];
    for clip in nothing {
        assert_eq!(scissor_of(size, 1.0, clip), None, "{clip:?}");
    }
}

// ---- room, the geometry buffers' growth ----

#[test]
fn a_buffer_has_at_least_the_smallest_size_and_grows_to_a_power_of_two() {
    let Some(device) = device() else { return };
    let mut buffer: Option<Buffer> = None;
    // Less than the smallest gives the smallest, even nothing.
    for bytes in [0, 100, SMALLEST_BUFFER] {
        let at = room(&device, &mut buffer, bytes);
        let made = buffer.as_ref().expect("a buffer");
        assert_eq!(made.length(), SMALLEST_BUFFER, "{bytes} bytes");
        assert_eq!(
            at,
            Some(made.contents()),
            "it returns the buffer's first byte"
        );
    }
    let smallest = buffer.as_ref().map(Retained::as_ptr);
    // One byte more, and it doubles.
    room(&device, &mut buffer, SMALLEST_BUFFER + 1);
    assert_eq!(
        buffer.as_ref().map(|b| b.length()),
        Some(2 * SMALLEST_BUFFER)
    );
    assert_ne!(
        buffer.as_ref().map(Retained::as_ptr),
        smallest,
        "a new buffer"
    );
    // More than that goes to the next power of two, not the next multiple.
    room(&device, &mut buffer, 300_000);
    assert_eq!(buffer.as_ref().map(|b| b.length()), Some(524_288));
    // And an exact power of two stays there.
    let mut other: Option<Buffer> = None;
    room(&device, &mut other, 1 << 20);
    assert_eq!(other.as_ref().map(|b| b.length()), Some(1 << 20));
    room(&device, &mut other, (1 << 20) + 1);
    assert_eq!(other.as_ref().map(|b| b.length()), Some(1 << 21));
}

#[test]
fn a_smaller_ask_keeps_the_same_buffer() {
    let Some(device) = device() else { return };
    let mut buffer: Option<Buffer> = None;
    room(&device, &mut buffer, 100_000);
    let big = buffer.as_ref().map(Retained::as_ptr);
    let length = buffer.as_ref().map(|b| b.length());
    assert_eq!(length, Some(131_072));
    // Less than it holds, as much as it holds, and nothing: no new buffer, whose contents would be lost.
    for bytes in [70_000, 131_072, 0] {
        room(&device, &mut buffer, bytes);
        assert_eq!(buffer.as_ref().map(Retained::as_ptr), big, "{bytes} bytes");
        assert_eq!(buffer.as_ref().map(|b| b.length()), length, "{bytes} bytes");
    }
}

/// A picture of 64 by 32 pixels, one mesh of 64 one-pixel quads for each row, each pixel its own color.
fn pixel_rows() -> Vec<ClippedPrimitive> {
    let all = target_rect([64, 32], 1.0);
    (0..32_u8)
        .map(|y| {
            let mut mesh = Mesh::with_texture(WHITE);
            for x in 0..64_u8 {
                let (xf, yf) = (f32::from(x), f32::from(y));
                mesh.add_colored_rect(rect(xf, yf, xf + 1.0, yf + 1.0), pixel_color(x, y));
            }
            clipped(mesh, all)
        })
        .collect()
}

fn pixel_color(x: u8, y: u8) -> Color32 {
    Color32::from_rgb(x * 4, y * 8, 255 - x - y)
}

#[test]
fn a_frame_whose_vertices_pass_the_smallest_buffer_draws_whole() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let meshes = pixel_rows();
    let vertex_bytes: usize = meshes
        .iter()
        .map(|primitive| match &primitive.primitive {
            Primitive::Mesh(mesh) => size_of::<Vertex>() * mesh.vertices.len(),
            Primitive::Callback(_) => 0,
        })
        .sum();
    let index_bytes: usize = meshes
        .iter()
        .map(|primitive| match &primitive.primitive {
            Primitive::Mesh(mesh) => size_of::<u32>() * mesh.indices.len(),
            Primitive::Callback(_) => 0,
        })
        .sum();
    assert!(
        vertex_bytes > SMALLEST_BUFFER,
        "{vertex_bytes}: the frame is too small to grow it"
    );
    assert!(
        index_bytes < SMALLEST_BUFFER,
        "{index_bytes}: only the vertices grow"
    );

    let mut delta = with_white();
    painter.render([64, 32], 1.0, CLEAR, &meshes, &mut delta);
    let slot = &painter.slots[0];
    assert_eq!(
        slot.vertices.as_ref().map(|b| b.length()),
        Some(vertex_bytes.next_power_of_two())
    );
    assert_eq!(
        slot.indices.as_ref().map(|b| b.length()),
        Some(SMALLEST_BUFFER)
    );
    // Every mesh landed, the first and the last among them, each pixel in its own color.
    let image = painter.read_screen();
    for y in 0..32_u8 {
        for x in 0..64_u8 {
            assert_eq!(
                px(&image, x.into(), y.into()),
                pixel_color(x, y),
                "pixel ({x}, {y})"
            );
        }
    }
}

// ---- uploads ----

/// A frame of one mesh that shows `id` over the whole of an 8 by 8 target.
fn show(id: TextureId) -> Vec<ClippedPrimitive> {
    let unit = rect(0.0, 0.0, 1.0, 1.0);
    vec![clipped(
        textured(rect(0.0, 0.0, 8.0, 8.0), unit, id),
        target_rect([8, 8], 1.0),
    )]
}

const BLUE: Color32 = Color32::from_rgb(0, 0, 255);
const GREEN: Color32 = Color32::from_rgb(0, 255, 0);
const RED: Color32 = Color32::from_rgb(255, 0, 0);

#[test]
fn a_partial_upload_is_written_once_and_shows() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    let image = ColorImage::filled([4, 4], BLUE);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(delta.set.is_empty(), "the whole set is drained");
    let screen = painter.read_screen();
    assert!(screen.pixels.iter().all(|&pixel| pixel == BLUE));

    // A 2 by 2 patch at the top right of the 4 by 4 texture, as eframe appends it to what the
    // painter handed back.
    let patch = ColorImage::filled([2, 2], GREEN);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::partial([2, 0], patch, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(
        delta.set.is_empty(),
        "the partial set is drained, so no frame writes it again"
    );
    let screen = painter.read_screen();
    for y in 0..8 {
        for x in 0..8 {
            let want = if x >= 4 && y < 4 { GREEN } else { BLUE };
            assert_eq!(px(&screen, x, y), want, "pixel ({x}, {y})");
        }
    }

    // A frame with nothing to set leaves the texture as it is.
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert_eq!(px(&painter.read_screen(), 7, 0), GREEN);
}

#[test]
fn a_whole_set_of_another_size_makes_a_new_texture() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    let image = ColorImage::filled([4, 4], BLUE);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert_eq!(painter.images[&IMAGE].size, [4, 4]);

    let image = ColorImage::filled([2, 2], RED);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(delta.set.is_empty());
    assert_eq!(painter.images[&IMAGE].size, [2, 2]);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == RED)
    );
}

#[test]
fn a_partial_upload_to_a_texture_that_was_never_set_is_dropped_with_no_panic() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    let patch = ColorImage::filled([2, 2], GREEN);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::partial([2, 0], patch, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(delta.set.is_empty());
    assert!(!painter.images.contains_key(&IMAGE));
    assert!(painter.images.contains_key(&WHITE));
}

#[test]
fn a_freed_texture_is_drained_after_the_frame_that_last_drew_it() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    let image = ColorImage::filled([4, 4], BLUE);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(painter.images.contains_key(&IMAGE));

    // Freed in the frame that still draws it: egui frees after painting.
    delta.free.insert(IMAGE);
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(delta.free.is_empty(), "the frees are drained");
    assert!(!painter.images.contains_key(&IMAGE));
    assert!(
        painter.images.contains_key(&WHITE),
        "only the freed one goes"
    );
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == BLUE)
    );

    // A mesh on it afterwards draws nothing, and nothing panics.
    painter.render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == CLEARED)
    );
}

#[test]
fn a_sampler_follows_the_texture_options() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    let ends = || ColorImage::new([2, 1], vec![Color32::BLACK, Color32::WHITE]);
    // Eight pixels across, the texture's two texels stretched over them or repeated.
    let show = |uv_right: f32| {
        let uv = rect(0.0, 0.0, uv_right, 1.0);
        vec![clipped(
            textured(rect(0.0, 0.0, 8.0, 2.0), uv, IMAGE),
            target_rect([8, 2], 1.0),
        )]
    };
    let mut row = |options: TextureOptions, uv_right: f32| {
        set(&mut delta, IMAGE, ImageDelta::full(ends(), options));
        painter.render([8, 2], 1.0, CLEAR, &show(uv_right), &mut delta);
        red_row(&painter.read_screen())
    };
    assert_eq!(
        row(TextureOptions::NEAREST, 1.0),
        [0, 0, 0, 0, 255, 255, 255, 255]
    );
    let linear = row(TextureOptions::LINEAR, 1.0);
    assert_eq!((linear[0], linear[7]), (0, 255));
    assert!(linear[3] > 0 && linear[3] < 255, "{linear:?}");
    assert!(linear[4] > 0 && linear[4] < 255, "{linear:?}");
    assert!(
        linear.windows(2).all(|pair| pair[0] <= pair[1]),
        "{linear:?}"
    );
    // Past the texture's edge: the last texel, the texture again, or the texture mirrored.
    assert_eq!(
        row(TextureOptions::NEAREST, 2.0),
        [0, 0, 255, 255, 255, 255, 255, 255]
    );
    assert_eq!(
        row(TextureOptions::NEAREST_REPEAT, 2.0),
        [0, 0, 255, 255, 0, 0, 255, 255]
    );
    assert_eq!(
        row(TextureOptions::NEAREST_MIRRORED_REPEAT, 2.0),
        [0, 0, 255, 255, 255, 255, 0, 0]
    );
}

// ---- the frame's order, size and surfaces ----

#[test]
fn read_screen_is_in_eguis_order_not_the_surfaces() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    // The surface is BGRA and Color32 is RGBA: red with no blue must stay red.
    painter.render([8, 4], 1.0, [1.0, 0.0, 0.0, 1.0], &[], &mut delta);
    let screen = painter.read_screen();
    assert_eq!(screen.size, [8, 4]);
    assert!(
        screen.pixels.iter().all(|&pixel| pixel == RED),
        "cleared red"
    );
    painter.render([8, 4], 1.0, [0.0, 0.0, 1.0, 1.0], &[], &mut delta);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == BLUE),
        "cleared blue"
    );
    // A mesh's color and a texel's go the same way.
    let orange = Color32::from_rgb(200, 100, 50);
    let mesh = vec![clipped(
        flat(rect(0.0, 0.0, 4.0, 4.0), orange),
        target_rect([8, 4], 1.0),
    )];
    painter.render([8, 4], 1.0, CLEAR, &mesh, &mut delta);
    let screen = painter.read_screen();
    assert_eq!(px(&screen, 1, 1), orange);
    assert_eq!(
        px(&screen, 6, 1),
        CLEARED,
        "nothing drawn right of the mesh"
    );
}

#[test]
fn read_screen_follows_the_size_given_to_render() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    painter.render([8, 4], 1.0, [1.0, 0.0, 0.0, 1.0], &[], &mut delta);
    assert_eq!(painter.read_screen().size, [8, 4]);
    painter.render([6, 10], 1.0, [0.0, 0.0, 1.0, 1.0], &[], &mut delta);
    let screen = painter.read_screen();
    assert_eq!(screen.size, [6, 10]);
    assert_eq!(screen.pixels.len(), 60);
    assert!(
        screen.pixels.iter().all(|&pixel| pixel == BLUE),
        "the old frame is gone"
    );
    assert!(
        painter.showing.is_none(),
        "the old surfaces went with the old size"
    );
}

#[test]
fn the_scale_sets_where_a_mesh_and_its_clip_land() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    // In points: a mesh from 2 to 6, clipped at 5. At 2 pixels to the point, a mesh from pixel 4
    // to 12, clipped at 10.
    let frame = vec![clipped(
        flat(rect(2.0, 2.0, 6.0, 6.0), GREEN),
        rect(0.0, 0.0, 5.0, 8.0),
    )];
    painter.render([16, 16], 2.0, CLEAR, &frame, &mut delta);
    let screen = painter.read_screen();
    for y in 0..16 {
        for x in 0..16 {
            let want = if (4..10).contains(&x) && (4..12).contains(&y) {
                GREEN
            } else {
                CLEARED
            };
            assert_eq!(px(&screen, x, y), want, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn a_second_surface_is_a_different_object_that_keeps_its_own_frame() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    painter.render([8, 8], 1.0, [1.0, 0.0, 0.0, 1.0], &[], &mut delta);
    assert_eq!(painter.present().map(|_| ()), None, "nothing shown before");
    painter.render([8, 8], 1.0, [0.0, 0.0, 1.0, 1.0], &[], &mut delta);
    let first = CFRetained::as_ptr(&painter.showing.as_ref().expect("the first").surface);
    let second = CFRetained::as_ptr(&painter.drawing.as_ref().expect("the second").surface);
    assert_ne!(first, second);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == BLUE)
    );
    // Back to the first surface, which the second frame left alone.
    painter.drawing = painter.present();
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == RED),
        "the second frame did not draw over the first surface"
    );
}

/// A sublayer with no view above it, shown on no screen: the painter's way in with no window.
fn give_a_layer(painter: &mut Painter) -> Retained<CALayer> {
    let layer = CALayer::new();
    painter.shown = Some(Shown {
        window: WindowId::from(1),
        layer: layer.clone(),
    });
    layer
}

#[test]
fn the_layer_gets_another_surface_each_frame_and_comes_back_to_the_first_after_them_all() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let layer = give_a_layer(&mut painter);
    let mut delta = with_white();
    let mut shown = Vec::new();
    for _ in 0..5 {
        painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
        // As a frame of a burst does.
        if let Some(replaced) = painter.present() {
            spare::park(replaced);
        }
        // SAFETY: a plain read of a live layer's contents, which `present` set to an IOSurface.
        let contents = unsafe { layer.contents() }.expect("the contents the frame set");
        assert_eq!(
            Retained::as_ptr(&contents),
            CFRetained::as_ptr(&painter.showing.as_ref().expect("the frame's").surface)
                .as_ptr()
                .cast_const()
                .cast(),
            "the frame shows the surface it drew"
        );
        shown.push(Retained::as_ptr(&contents));
    }
    // What makes Core Animation show the new contents, with no private call.
    for (frame, pair) in shown.windows(2).enumerate() {
        assert_ne!(pair[0], pair[1], "frames {frame} and {}", frame + 1);
    }
    assert_eq!(shown[0], shown[2], "after both surfaces, the first");
    assert_eq!(shown[1], shown[3]);
    assert_eq!(shown[0], shown[4]);
}

#[test]
fn a_frame_with_no_layer_gives_back_what_it_replaces_all_the_same() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    assert!(painter.present().is_none());
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    assert!(painter.present().is_some());
}

#[test]
fn the_window_with_the_layer_keeps_it_and_another_window_fails_the_painter() {
    if !has_a_device() {
        return;
    }
    let flag = own_flag();
    let mut painter = Painter::start(Context::default(), false, flag);
    let window = WindowId::from(1);
    assert!(
        !painter.shows_in(window),
        "no layer yet: this window gets one"
    );
    give_a_layer(&mut painter);
    assert!(painter.shows_in(window));
    assert_eq!(flag.why(), None);
    assert!(painter.shows_in(WindowId::from(2)));
    assert_eq!(
        flag.why(),
        Some("a second window on a painter that shows its first"),
        "the new window would stay blank"
    );
}

#[test]
fn a_write_lets_go_of_the_frames_in_flight_and_a_screenshot_keeps_them() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    let in_flight = |painter: &Painter| painter.slots.iter().filter(|s| s.last.is_some()).count();
    let mut delta = with_white();
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    assert_eq!(in_flight(&painter), 2);
    painter.read_screen();
    assert_eq!(in_flight(&painter), 2, "a read has no reason to let go");
    painter
        .update_textures(&TexturesDelta::default())
        .expect("no write");
    assert_eq!(in_flight(&painter), 2, "no write, no wait");
    painter.update_textures(&with_white()).expect("a write");
    assert_eq!(in_flight(&painter), 0, "a write waited for them all");
}

// ---- the device ----

#[test]
fn a_texture_of_the_largest_side_by_one_can_be_made_on_this_device() {
    let Some(painter) = painter(false) else {
        return;
    };
    let side = painter
        .max_texture_side()
        .expect("a limit while the device lives");
    let gpu = painter.gpu.as_ref().expect("the GPU");
    for dims in [[side, 1], [1, side]] {
        let desc = texture_descriptor(
            MTLPixelFormat::RGBA8Unorm,
            dims,
            MTLTextureUsage::ShaderRead,
        );
        assert!(
            gpu.device.newTextureWithDescriptor(&desc).is_some(),
            "{dims:?}"
        );
    }
}

#[test]
fn after_destroy_there_is_no_limit_and_a_drop_after_it_is_quiet() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    assert!(painter.max_texture_side().is_some());
    let mut delta = with_white();
    painter.render([8, 8], 1.0, CLEAR, &show(WHITE), &mut delta);
    painter.destroy();
    assert_eq!(painter.max_texture_side(), None);
    assert!(painter.images.is_empty());
    assert!(painter.drawing.is_none());
    assert!(painter.showing.is_none());
    // Both of eframe's ways in: `destroy` twice, then the drop at the end of this function.
    painter.destroy();
}

#[test]
fn the_shader_loads_from_the_file_and_compiles_from_the_source_with_both_stages() {
    let Some(device) = device() else { return };
    for (from, shader) in [("file", Shader::File(METALLIB)), ("source", Shader::Source)] {
        let library =
            shader_library(&device, shader).unwrap_or_else(|why| panic!("from the {from}: {why}"));
        for name in ["vs", "fs"] {
            let found = library.newFunctionWithName(&NSString::from_str(name));
            assert!(found.is_some(), "{name}, from the {from}");
        }
    }
}

// ---- screenshots ----

#[test]
fn a_capture_comes_out_as_one_screenshot_event_for_each_user_data() {
    let Some(painter) = painter(false) else {
        return;
    };
    let other = ViewportId::from_hash_of("another viewport");
    let image = ColorImage::filled([2, 2], RED);
    let data = vec![
        UserData::new(1_u32),
        UserData::new(2_u32),
        UserData::default(),
    ];
    painter
        .capture_tx
        .send((ViewportId::ROOT, data, image.clone()))
        .expect("the receiver lives");
    painter
        .capture_tx
        .send((other, vec![UserData::new(3_u32)], image))
        .expect("the receiver lives");

    let mut events = Vec::new();
    painter.handle_screenshots(&mut events);
    let seen: Vec<(ViewportId, Option<u32>, Arc<ColorImage>)> = events
        .into_iter()
        .map(|event| match event {
            Event::Screenshot {
                viewport_id,
                user_data,
                image,
            } => {
                let value = user_data
                    .data
                    .and_then(|data| data.downcast_ref::<u32>().copied());
                (viewport_id, value, image)
            }
            other => panic!("not a screenshot: {other:?}"),
        })
        .collect();
    let ids: Vec<_> = seen.iter().map(|(id, value, _)| (*id, *value)).collect();
    assert_eq!(
        ids,
        [
            (ViewportId::ROOT, Some(1)),
            (ViewportId::ROOT, Some(2)),
            (ViewportId::ROOT, None),
            (other, Some(3)),
        ]
    );
    // One picture, shared by the events of a capture.
    assert!(Arc::ptr_eq(&seen[0].2, &seen[1].2));
    assert!(Arc::ptr_eq(&seen[0].2, &seen[2].2));
    assert!(!Arc::ptr_eq(&seen[0].2, &seen[3].2));
    assert_eq!(seen[0].2.size, [2, 2]);

    // A capture is handed over once.
    let mut again = Vec::new();
    painter.handle_screenshots(&mut again);
    assert!(again.is_empty());
}

// ---- the small golden ----

/// The golden frame's side, in pixels.
const SIDE: usize = 32;
/// The frame as it was drawn when the file was last written: 32 by 32 pixels, four bytes each, in
/// egui's order. `include_bytes!` gives a slice, so a file of another length is a failing test and
/// not a failing build.
const GOLDEN: &[u8] = include_bytes!("../golden/frame-32.rgba");
const GOLDEN_FILE: &str = "frame-32.rgba";
const MAGENTA: Color32 = Color32::from_rgb(255, 0, 255);
/// Half of green over what it covers: premultiplied, as egui hands colors to the shaders.
const HALF_GREEN: Color32 = Color32::from_rgba_premultiplied(0, 128, 0, 128);

/// The texel at column `x` and row `y` of the 4 by 4 texture.
fn texel(x: usize, y: usize) -> Color32 {
    let step = |n: usize| 30 + 60 * u8::try_from(n).expect("a small number");
    Color32::from_rgb(step(x), step(y), 100)
}

/// The fixed frame: a gradient, a texture, a clip, a blend, and the dither over it all.
///
/// - Gradient: rows 0 to 8, red at the top left, blue at the top right, yellow at the bottom
///   left and green at the bottom right.
/// - Texture: a square of 12 pixels, from (2, 10), of a 4 by 4 texture, each texel 3 pixels.
/// - Clip: a magenta rectangle from (16, 10) to the corner, clipped to (24, 14) to (32, 26).
/// - Blend: half green over the texture's lower right and the clear color beside it.
fn golden_frame() -> (Vec<ClippedPrimitive>, Delta) {
    let all = target_rect([SIDE, SIDE], 1.0);
    let mut delta = with_white();
    let texels = (0..16).map(|n| texel(n % 4, n / 4)).collect();
    let image = ColorImage::new([4, 4], texels);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    let primitives = vec![
        clipped(
            gradient(
                rect(0.0, 0.0, 32.0, 8.0),
                [RED, BLUE, Color32::YELLOW, GREEN],
            ),
            all,
        ),
        clipped(
            textured(rect(2.0, 10.0, 14.0, 22.0), rect(0.0, 0.0, 1.0, 1.0), IMAGE),
            all,
        ),
        clipped(
            flat(rect(16.0, 10.0, 32.0, 32.0), MAGENTA),
            rect(24.0, 14.0, 32.0, 26.0),
        ),
        clipped(flat(rect(10.0, 18.0, 20.0, 28.0), HALF_GREEN), all),
    ];
    (primitives, delta)
}

/// The golden frame as a fresh painter draws it, or nothing where there is no device.
fn drawn(dithering: bool) -> Option<ColorImage> {
    let mut painter = painter(dithering)?;
    let (primitives, mut delta) = golden_frame();
    painter.render([SIDE, SIDE], 1.0, CLEAR, &primitives, &mut delta);
    Some(painter.read_screen())
}

fn bytes_of(image: &ColorImage) -> Vec<u8> {
    image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect()
}

/// Why `drawn` is not `stored` within 1 of 255 in each channel, as another GPU may round apart.
fn beyond_one(stored: &[u8], drawn: &[u8]) -> Option<String> {
    beyond(1, stored, drawn)
}

/// Why `drawn` is not `stored` within `levels` of 255 in each channel: another length, or the first
/// channel past it.
fn beyond(levels: u8, stored: &[u8], drawn: &[u8]) -> Option<String> {
    if stored.len() != drawn.len() {
        return Some(format!(
            "{} bytes stored, {} drawn",
            stored.len(),
            drawn.len()
        ));
    }
    let (at, (old, new)) = stored
        .iter()
        .zip(drawn)
        .enumerate()
        .find(|(_, (old, new))| old.abs_diff(**new) > levels)?;
    let channel = ["red", "green", "blue", "alpha"][at % 4];
    let (x, y) = ((at / 4) % SIDE, (at / 4) / SIDE);
    Some(format!(
        "pixel ({x}, {y}), {channel}: {old} stored, {new} drawn"
    ))
}

/// Whether this run rewrites the file instead of checking it: only the value `1` does.
fn blessing() -> bool {
    std::env::var_os("ARRAKIS_BLESS").is_some_and(|value| value == "1")
}

#[test]
fn the_golden_comparison_allows_one_level_and_no_more() {
    assert_eq!(beyond_one(&[10, 20, 30, 255], &[10, 20, 30, 255]), None);
    assert_eq!(beyond_one(&[10, 20, 30, 255], &[11, 19, 30, 254]), None);
    let why = beyond_one(&[10, 20, 30, 255], &[10, 20, 32, 255]).expect("two levels apart");
    assert!(
        why.contains("pixel (0, 0), blue: 30 stored, 32 drawn"),
        "{why}"
    );
    let why = beyond_one(&[0; 8], &[0; 4]).expect("another length");
    assert!(why.contains("8 bytes stored, 4 drawn"), "{why}");
    // It says where: the second row's first pixel.
    let row = 4 * SIDE;
    let (stored, mut moved) = (vec![0; row + 4], vec![0; row + 4]);
    moved[row + 1] = 9;
    let why = beyond_one(&stored, &moved).expect("a difference");
    assert!(why.contains("pixel (0, 1), green"), "{why}");
}

/// What compares the file's frame with the source's: one level is a difference, and so is a frame of
/// another length, an empty one too, which a zip to the shorter would let pass.
#[test]
fn the_exact_comparison_allows_no_level_and_no_frame_of_another_length() {
    assert_eq!(beyond(0, &[10, 20, 30, 255], &[10, 20, 30, 255]), None);
    let why = beyond(0, &[10, 20, 30, 255], &[10, 21, 30, 255]).expect("one level apart");
    assert!(
        why.contains("pixel (0, 0), green: 20 stored, 21 drawn"),
        "{why}"
    );
    let why = beyond(0, &[10, 20, 30, 255], &[]).expect("an empty frame");
    assert!(why.contains("4 bytes stored, 0 drawn"), "{why}");
    let why = beyond(0, &[10, 20, 30, 255], &[10, 20]).expect("a short frame");
    assert!(why.contains("4 bytes stored, 2 drawn"), "{why}");
}

/// Rewrites `src/golden/frame-32.rgba` with `ARRAKIS_BLESS=1`, only when it is stale, then run
/// every test: the new bytes are built in at compile time.
///
/// ```sh
/// ARRAKIS_BLESS=1 cargo test -p egui-wgpu golden
/// cargo test --features demo
/// ```
#[test]
fn the_small_golden_frame_is_the_one_drawn_now() {
    let Some(image) = drawn(true) else { return };
    let fresh = bytes_of(&image);
    let Some(why) = beyond_one(GOLDEN, &fresh) else {
        return;
    };
    if blessing() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/golden")
            .join(GOLDEN_FILE);
        std::fs::write(&path, &fresh)
            .unwrap_or_else(|error| panic!("unable to write {}: {error}", path.display()));
    } else {
        panic!(
            "src/golden/{GOLDEN_FILE} is not what the painter draws now ({why}). If the change \
             is meant, rewrite the file, then run every test, which builds it in:\n    \
             ARRAKIS_BLESS=1 cargo test -p egui-wgpu golden\n    cargo test --features demo"
        );
    }
}

/// How far a pixel is from `want`, the largest of its channels.
fn off(pixel: Color32, want: [f32; 3]) -> f32 {
    let got = [pixel.r(), pixel.g(), pixel.b()];
    got.iter()
        .zip(want)
        .map(|(&got, want)| (f32::from(got) - want).abs())
        .fold(0.0, f32::max)
}

/// What the blend gives: the premultiplied source, plus the destination kept by what the source
/// lets through.
fn over(source: Color32, below: Color32) -> [f32; 3] {
    let keep = 1.0 - f32::from(source.a()) / 255.0;
    [
        f32::from(source.r()) + f32::from(below.r()) * keep,
        f32::from(source.g()) + f32::from(below.g()) * keep,
        f32::from(source.b()) + f32::from(below.b()) * keep,
    ]
}

#[test]
fn the_golden_frame_draws_what_its_meshes_say() {
    let Some(screen) = drawn(true) else { return };
    // The texture is upright, and each texel is 3 pixels of its own color. One of them is under
    // the blend.
    for y in 0..4 {
        for x in 0..4 {
            if (x, y) != (3, 3) {
                assert_eq!(
                    px(&screen, 3 + 3 * x, 11 + 3 * y),
                    texel(x, y),
                    "texel ({x}, {y})"
                );
            }
        }
    }
    // The clip keeps the magenta inside (24, 14) to (32, 26), and the clear color outside it.
    for (x, y) in [(24, 14), (31, 14), (24, 25), (31, 25)] {
        assert_eq!(px(&screen, x, y), MAGENTA, "inside at ({x}, {y})");
    }
    for (x, y) in [(23, 14), (24, 13), (24, 26), (31, 26), (16, 12), (20, 12)] {
        assert_eq!(px(&screen, x, y), CLEARED, "outside at ({x}, {y})");
    }
    // The blend is neither of its sources: over the clear color, and over a texel.
    let beside = px(&screen, 17, 25);
    assert!(off(beside, over(HALF_GREEN, CLEARED)) <= 1.0, "{beside:?}");
    assert_ne!(beside, CLEARED);
    assert_ne!(beside, GREEN);
    let on_texel = px(&screen, 12, 20);
    assert!(
        off(on_texel, over(HALF_GREEN, texel(3, 3))) <= 1.0,
        "{on_texel:?}"
    );
    assert_ne!(on_texel, texel(3, 3));
    // The gradient's corners, red at the top left and green at the bottom right: the pixel's
    // center is a little in from the corner, so the colors are not quite the corners'.
    assert!(
        off(px(&screen, 0, 0), [251.0, 16.0, 4.0]) <= 3.0,
        "{:?}",
        px(&screen, 0, 0)
    );
    assert!(
        off(px(&screen, 31, 7), [4.0, 239.0, 16.0]) <= 3.0,
        "{:?}",
        px(&screen, 31, 7)
    );
    // Opaque everywhere.
    assert!(screen.pixels.iter().all(|pixel| pixel.a() == 255));
}

#[test]
fn the_dither_moves_the_gradient_by_one_level_at_most_and_nothing_else() {
    let (Some(plain), Some(dithered)) = (drawn(false), drawn(true)) else {
        return;
    };
    let mut moved = 0;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (a, b) = (px(&plain, x, y), px(&dithered, x, y));
            let apart = [(a.r(), b.r()), (a.g(), b.g()), (a.b(), b.b())]
                .into_iter()
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or(0);
            assert!(apart <= 1, "pixel ({x}, {y}): {a:?} and {b:?}");
            if apart > 0 {
                moved += 1;
                // The gradient, and the blend's fractions: never a flat opaque area.
                let in_gradient = y < 8;
                let in_blend = (10..20).contains(&x) && (18..28).contains(&y);
                assert!(
                    in_gradient || in_blend,
                    "pixel ({x}, {y}) moved on a flat area"
                );
            }
        }
    }
    assert!(moved > 20, "the dither moved only {moved} pixels");
    // The gradient alone moves most of its pixels, so it is the dither and not the rounding.
    let gradient_moved = (0..SIDE)
        .flat_map(|x| (0..8).map(move |y| (x, y)))
        .filter(|&(x, y)| px(&plain, x, y) != px(&dithered, x, y))
        .count();
    assert!(gradient_moved > 20, "{gradient_moved}");
}

/// `egui_glow`'s interleaved gradient noise for the pixel at column `x` and row `y`, counted from
/// the top, in levels of 255: `(noise - 0.5) * 0.95`, where the noise is
/// `fract(52.9829189 * fract(dot(frag_coord, (0.06711056, 0.00583715))))` and `frag_coord` is GL's,
/// counted up from the bottom left of the target. Worked in `f32` as the GPU does, and in `f64`,
/// with nothing returned where the two cannot agree on which side of a wrap a pixel falls.
fn glow_noise(x: usize, y: usize) -> Option<f64> {
    let (fx, fy) = (x as f32 + 0.5, (SIDE - y) as f32 - 0.5);
    let f = 0.067_110_56_f32 * fx + 0.005_837_15_f32 * fy;
    let narrow = (52.982_92_f32 * f.fract()).fract();
    let (fx, fy) = (x as f64 + 0.5, (SIDE - y) as f64 - 0.5);
    let f = 0.067_110_56_f64 * fx + 0.005_837_15_f64 * fy;
    let wide = (52.982_918_9_f64 * f.fract()).fract();
    ((f64::from(narrow) - wide).abs() < 1e-3).then_some((wide - 0.5) * 0.95)
}

#[test]
fn the_dither_shifts_each_pixel_the_way_egui_glows_noise_says() {
    let (Some(plain), Some(dithered)) = (drawn(false), drawn(true)) else {
        return;
    };
    // The golden's tolerance of 1 hides which way a pixel moved, so the direction is checked here.
    // A pixel at `p` with the noise `n` is at `round(c + n)` with `c` in `[p - 0.5, p + 0.5)`: not
    // below `p` for `n > 0.05`, and not above it for `n < -0.05`.
    let (mut agreed, mut checked) = (0, 0);
    for y in 0..8 {
        for x in 0..SIDE {
            let Some(noise) = glow_noise(x, y) else {
                continue;
            };
            let (a, b) = (px(&plain, x, y), px(&dithered, x, y));
            for (p, d) in [(a.r(), b.r()), (a.g(), b.g()), (a.b(), b.b())] {
                if noise > 0.05 {
                    assert!(
                        d >= p,
                        "pixel ({x}, {y}): noise {noise:.2} lowered {p} to {d}"
                    );
                    agreed += usize::from(d > p);
                    checked += 1;
                } else if noise < -0.05 {
                    assert!(
                        d <= p,
                        "pixel ({x}, {y}): noise {noise:.2} raised {p} to {d}"
                    );
                    agreed += usize::from(d < p);
                    checked += 1;
                }
            }
        }
    }
    // Most pixels were checked, and a good share of them moved the way the noise says.
    assert!(checked > 400, "{checked}");
    assert!(agreed > 100, "{agreed} of {checked}");
}

// ---- the shader's file: `shader.metallib`, its stamp and the fallback to the source ----

/// What a failure here asks for.
const REBUILD: &str = "Rebuild the file and its stamp with scripts/build-metallib.sh, which needs \
                       Xcode and its Metal Toolchain: a changed shader needs Xcode. Then run cargo \
                       test --features demo.";

/// What `/usr/bin/shasum -a 256` says of `src/<name>`: a tool apart from the code under test, and
/// apart from the script, which has the same tool hash the files.
fn sha256_of(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(name);
    let out = std::process::Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(&path)
        .output()
        .unwrap_or_else(|error| panic!("/usr/bin/shasum does not run: {error}"));
    assert!(
        out.status.success(),
        "shasum on {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    said.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn the_file_is_the_one_its_stamp_says_was_built_from_this_source() {
    assert_eq!(
        sha256_of("shader.metal"),
        stamp::SOURCE,
        "src/shader.metal is not the source that src/shader.metallib was built from. {REBUILD}"
    );
    assert_eq!(
        sha256_of("shader.metallib"),
        stamp::FILE,
        "src/shader.metallib is not the file its stamp names. {REBUILD}"
    );
    assert_eq!(
        METALLIB.len(),
        stamp::BYTES,
        "src/shader.metallib is not the length its stamp gives. {REBUILD}"
    );
}

/// The app's minimum, read in the one form `scripts/build-metallib.sh` reads with `sed`: a line that
/// begins `MACOSX_DEPLOYMENT_TARGET = { value = "`, then a digit, digits and dots, and a `"`; and the
/// only such line. What the script refuses, this refuses.
fn deployment_target(config: &str) -> Option<&str> {
    let mut targets = config.lines().filter_map(|line| {
        let rest = line.strip_prefix("MACOSX_DEPLOYMENT_TARGET = { value = \"")?;
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let (target, after) = rest.split_at(end);
        (target.starts_with(|c: char| c.is_ascii_digit()) && after.starts_with('"'))
            .then_some(target)
    });
    let target = targets.next()?;
    targets.next().is_none().then_some(target)
}

#[test]
fn the_minimum_is_read_in_the_scripts_form_alone() {
    let line = "MACOSX_DEPLOYMENT_TARGET = { value = \"14.0\", force = true }";
    assert_eq!(deployment_target(line), Some("14.0"));
    assert_eq!(deployment_target(&format!("[env]\n{line}\n")), Some("14.0"));
    for refused in [
        "MACOSX_DEPLOYMENT_TARGET = \"14.0\"",
        " MACOSX_DEPLOYMENT_TARGET = { value = \"14.0\" }",
        "MACOSX_DEPLOYMENT_TARGET={ value = \"14.0\" }",
        "MACOSX_DEPLOYMENT_TARGET = { value = \"v14.0\" }",
        "MACOSX_DEPLOYMENT_TARGET = { value = \".14\" }",
        "MACOSX_DEPLOYMENT_TARGET = { value = \"14.0-beta\" }",
        "MACOSX_DEPLOYMENT_TARGET = { value = \"14.0 }",
        "# MACOSX_DEPLOYMENT_TARGET = { value = \"14.0\" }",
        "",
    ] {
        assert_eq!(deployment_target(refused), None, "{refused}");
    }
    assert_eq!(
        deployment_target(&format!("{line}\n{line}")),
        None,
        "two lines"
    );
}

#[test]
fn the_file_is_built_for_the_apps_minimum() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.cargo/config.toml");
    let config = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("unable to read {}: {error}", path.display()));
    assert_eq!(
        deployment_target(&config),
        Some(stamp::TARGET),
        "the app's minimum in .cargo/config.toml is not the target src/shader.metallib was built \
         for, or not in the one form scripts/build-metallib.sh reads. {REBUILD}"
    );
}

/// A file of the test's own, as the process's would send every other test's painter to the source
/// once a test fails it on purpose. For the process's life, as `own_flag` is.
fn own_file(bytes: &'static [u8]) -> &'static ShaderFile {
    Box::leak(Box::new(ShaderFile::new(bytes)))
}

/// A painter started on `file` as the app's start is on the process's, with a flag of its own, or none
/// where there is no device.
fn started_on(file: &ShaderFile) -> Option<Painter> {
    has_a_device().then(|| Painter::start_on(Context::default(), true, own_flag(), file))
}

/// The good file, as an earlier painter that it did not serve leaves it: every painter on it goes
/// straight to the source.
fn refused_file() -> &'static ShaderFile {
    let file = own_file(METALLIB);
    assert!(file.refuse(), "a new file has been refused by no painter");
    file
}

/// Whether `painter`'s pipeline came from [`METALLIB`]'s bytes. By the bytes and not the address: Rust
/// does not promise one address for every use of a constant.
fn from_the_file(painter: &Painter) -> bool {
    matches!(
        painter.gpu.as_ref().expect("a GPU").shader,
        Shader::File(bytes) if bytes == METALLIB
    )
}

fn from_the_source(painter: &Painter) -> bool {
    matches!(painter.gpu.as_ref().expect("a GPU").shader, Shader::Source)
}

/// The golden frame, dithered, as `painter` draws it.
fn drawn_by(mut painter: Painter) -> Vec<u8> {
    let (primitives, mut delta) = golden_frame();
    painter.render([SIDE, SIDE], 1.0, CLEAR, &primitives, &mut delta);
    assert!(!painter.frames.failed(), "the frame failed");
    bytes_of(&painter.read_screen())
}

/// The warnings logged in this test binary, each with the thread that logged it, so that a test counts
/// its own: libtest runs each test on a thread of its own, and the others warn too.
struct Warnings(std::sync::Mutex<Vec<(std::thread::ThreadId, String)>>);

impl log::Log for Warnings {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let said = (std::thread::current().id(), record.args().to_string());
            self.0.lock().unwrap().push(said);
        }
    }

    fn flush(&self) {}
}

static WARNINGS: Warnings = Warnings(std::sync::Mutex::new(Vec::new()));

/// The warnings this thread has logged that begin with `start`. The first call sets the logger, for
/// the rest of the binary.
fn warnings_here(start: &str) -> usize {
    static SET: std::sync::Once = std::sync::Once::new();
    SET.call_once(|| {
        log::set_logger(&WARNINGS).expect("no other logger in these tests");
        log::set_max_level(log::LevelFilter::Warn);
    });
    let here = std::thread::current().id();
    let warnings = WARNINGS.0.lock().unwrap();
    warnings
        .iter()
        .filter(|(thread, said)| *thread == here && said.starts_with(start))
        .count()
}

/// The app's own start, on the process's file, serves the pipeline from the file, and the drive's
/// report says so. A start that took the source would draw the same frame: only this shows the
/// wiring.
#[test]
fn the_apps_start_serves_the_pipeline_from_the_file() {
    let Some(painter) = painter(true) else {
        return;
    };
    assert!(
        from_the_file(&painter),
        "the app's start did not serve shader.metallib"
    );
    assert_eq!(shader_from(), Some("file"));
}

/// The hash and the stamp show that the file matches the source as typed, not that it was built from
/// it: this shows that, on this GPU, through the app's start and a start that goes to the source.
#[test]
fn the_frame_from_the_file_is_the_frame_from_the_source_byte_for_byte() {
    let (Some(file), Some(source)) = (painter(true), started_on(refused_file())) else {
        return;
    };
    assert!(from_the_file(&file) && from_the_source(&source));
    let from_file = drawn_by(file);
    let from_source = drawn_by(source);
    if let Some(why) = beyond(0, &from_source, &from_file) {
        panic!(
            "the file's frame is not the source's at {why} (stored: the source's; drawn: the \
             file's). If the_file_is_the_one_its_stamp_says_was_built_from_this_source passes, \
             shader.metal and the stamp are unchanged, and a rebuild gives the same file back: the \
             source's frame comes from the runtime compiler, which a macOS update changes while the \
             file stays frozen. No rebuild answers that, and the owner decides. If that test fails \
             too: {REBUILD}"
        );
    }
    // Two blank frames would agree too.
    if let Some(why) = beyond_one(GOLDEN, &from_file) {
        panic!("the file's frame is not the golden one: {why}");
    }
}

#[test]
fn a_file_that_will_not_load_falls_back_to_the_source_and_paints() {
    let (Some(device), Some(source)) = (device(), started_on(refused_file())) else {
        return;
    };
    let from_source = drawn_by(source);
    let bad: &[(&str, &'static [u8])] = &[
        ("bytes that are not a metallib", b"not a metallib"),
        ("the file's first half", &METALLIB[..METALLIB.len() / 2]),
        ("no bytes", &[]),
        #[cfg(feature = "demo")]
        ("ARRAKIS_METAL_FAIL=metallib's bytes", NOT_A_METALLIB),
    ];
    for &(what, bytes) in bad {
        assert!(
            pipeline(&device, Shader::File(bytes)).is_err(),
            "Metal takes {what}, so this shows no fallback"
        );
        let painter = started_on(own_file(bytes)).expect("a device");
        assert!(from_the_source(&painter), "{what}: no fallback");
        let drawn = drawn_by(painter);
        assert_eq!(
            drawn.len(),
            GOLDEN.len(),
            "{what}: the fallback's frame is not the golden's size"
        );
        if let Some(why) = beyond(0, &from_source, &drawn) {
            panic!("{what}: the fallback's frame is not the source's at {why}");
        }
    }
}

/// One file that fails and two painters, as the app makes one for each window: the first tries the
/// file, says so and takes the source, and the second goes straight to the source and says nothing.
#[test]
fn a_file_that_failed_once_sends_every_later_painter_straight_to_the_source_with_one_warning() {
    let file = own_file(b"not a metallib");
    let before = warnings_here(METALLIB_REFUSED);
    let (Some(first), Some(second)) = (started_on(file), started_on(file)) else {
        return;
    };
    assert!(from_the_source(&first) && from_the_source(&second));
    assert_eq!(
        warnings_here(METALLIB_REFUSED) - before,
        1,
        "one warning for the process, not one for each painter"
    );
    assert_eq!(file.to_try(), None, "the file is remembered as refused");
    assert_eq!(file.last(), Some("source"));
    // Straight to the source: a painter on a file that an earlier one refused does not try it, though
    // these bytes would load, and says nothing.
    let third = started_on(refused_file()).expect("a device");
    assert!(from_the_source(&third), "it tried the file again");
    assert_eq!(warnings_here(METALLIB_REFUSED) - before, 1);
}

#[test]
fn a_file_says_where_the_last_painter_on_it_took_its_library_from() {
    let file = ShaderFile::new(METALLIB);
    assert_eq!(file.last(), None, "no painter yet");
    file.note(Shader::File(METALLIB));
    assert_eq!(file.last(), Some("file"));
    file.note(Shader::Source);
    assert_eq!(file.last(), Some("source"));
}

// ---- the waits, on real command buffers ----
//
// A real status is never forged here: the stages are reached by committing real buffers, or never
// reached by leaving one uncommitted. A real `MTLCommandBufferStatus::Error` cannot be made (there is no
// way to reset a GPU, and a frame on a purged texture completes), so the handler's error branch and
// `why_it_failed` go unread by a test. What a wait does with a status of `Error` is read in `wait.rs`,
// on a forged status.

/// The GPU of a painter's start, with no painter around it.
fn gpu() -> Option<Gpu> {
    has_a_device().then(|| start_gpu(process_file()).expect("a GPU"))
}

fn empty_buffer(gpu: &Gpu) -> CommandBuffer {
    gpu.queue.commandBuffer().expect("a command buffer")
}

/// Commits `buffer` with `handler` on both stages, as `draw` does.
fn commit_with(buffer: &CommandBuffer, handler: &Handler) {
    // SAFETY: the block is live for as long as the caller holds `handler`, which every test does
    // until its buffers have completed. Its signature is the handlers', and what it captures crosses
    // threads (`wait`). Both go on before the commit, as Metal asks.
    unsafe {
        buffer.addScheduledHandler(RcBlock::as_ptr(handler));
        buffer.addCompletedHandler(RcBlock::as_ptr(handler));
    }
    buffer.commit();
}

const BOTH_STAGES: [Stage; 2] = [Stage::Scheduled, Stage::Completed];

/// Whether Metal says `buffer` has come as far as `stage`, by the status's own order, read here and
/// not through the code under test.
fn has_reached(buffer: &CommandBuffer, stage: Stage) -> bool {
    let least = match stage {
        Stage::Scheduled => MTLCommandBufferStatus::Scheduled,
        Stage::Completed => MTLCommandBufferStatus::Completed,
    };
    buffer.status().0 >= least.0
}

#[test]
fn a_committed_buffer_is_waited_for_at_each_stage_and_the_flag_stays_down() {
    let Some(gpu) = gpu() else { return };
    for stage in BOTH_STAGES {
        let flag = own_flag();
        // The limit the app has. Metal must set a status before it calls the handler that wakes
        // the wait: the other way round, the wake finds the old status and the wait runs out its
        // whole second, which raises the flag.
        let frames = Arc::new(Frames::new(flag));
        let handler = wait::handler(&frames);
        let buffer = empty_buffer(&gpu);
        commit_with(&buffer, &handler);
        frames.wait_for(&buffer, stage);
        assert!(!flag.is_up(), "{stage:?}: {:?}", flag.why());
        assert!(has_reached(&buffer, stage), "{stage:?}");
    }
}

/// A command buffer to commit from another thread. Metal's buffers may be committed from any thread,
/// and nothing else touches this one until then.
struct Held(CommandBuffer);

// SAFETY: see above.
unsafe impl Send for Held {}

impl Held {
    /// By value, so that a closure takes the whole of it and not its field.
    fn commit(self) {
        self.0.commit();
    }
}

#[test]
fn a_wait_that_sleeps_is_woken_by_metals_handler_at_each_stage() {
    let Some(gpu) = gpu() else { return };
    // The queue runs buffers in the order they were enqueued, so a buffer committed behind a place
    // that is held cannot be scheduled until the place is committed. The wait then really sleeps,
    // and only the handler's wake can end it.
    const HELD_FOR: Duration = Duration::from_millis(150);
    const LIMIT: Duration = Duration::from_secs(10);
    for stage in BOTH_STAGES {
        let flag = own_flag();
        // Far more than the sleep. A handler that never woke the wait would not show as the flag: std's
        // `wait_timeout_while` reads the status once more at the limit, finds it reached, and reports
        // no timeout. It shows as the time the wait took, which the bound below reads.
        let frames = Arc::new(Frames::with_limit(flag, LIMIT));
        let handler = wait::handler(&frames);
        let place = empty_buffer(&gpu);
        place.enqueue();
        let buffer = empty_buffer(&gpu);
        commit_with(&buffer, &handler);
        assert!(
            !has_reached(&buffer, stage),
            "{stage:?}: the buffer is behind a place that is held"
        );
        let held = Held(place);
        let from = Instant::now();
        let committer = std::thread::spawn(move || {
            std::thread::sleep(HELD_FOR);
            held.commit();
        });
        frames.wait_for(&buffer, stage);
        let waited = from.elapsed();
        committer.join().expect("the committing thread");
        assert!(
            waited >= HELD_FOR,
            "{stage:?}: the wait ended after {waited:?}, before the place was committed"
        );
        // A woken wait ends a moment after the commit. One that nothing woke ends at the limit, 10 s
        // in: half of it is 4.85 s over the hold, which no scheduler delay makes up.
        assert!(
            waited < LIMIT / 2,
            "{stage:?}: the wait took {waited:?}, so the handler did not wake it"
        );
        assert!(!flag.is_up(), "{stage:?}: {:?}", flag.why());
        assert!(has_reached(&buffer, stage), "{stage:?}");
    }
}

#[test]
fn a_buffer_that_is_never_committed_runs_the_wait_out_and_fails_with_its_stages_reason() {
    let Some(gpu) = gpu() else { return };
    const LIMIT: Duration = Duration::from_millis(10);
    for (stage, word) in [
        (Stage::Scheduled, "scheduled"),
        (Stage::Completed, "complete"),
    ] {
        let flag = own_flag();
        let frames = Frames::with_limit(flag, LIMIT);
        let buffer = empty_buffer(&gpu);
        let from = Instant::now();
        frames.wait_for(&buffer, stage);
        assert!(from.elapsed() >= LIMIT, "{stage:?}: it did not wait");
        let why = flag.why().expect("the wait ran out, so the flag is up");
        assert!(why.contains(word), "{stage:?}: {why}");
    }
}

#[test]
fn with_the_flag_up_a_wait_does_not_wait() {
    let Some(gpu) = gpu() else { return };
    let flag = own_flag();
    // A limit no test would sit through: a wait that waited would end the run long before this.
    let frames = Frames::with_limit(flag, Duration::from_secs(60));
    frames.fail("an earlier failure");
    let buffer = empty_buffer(&gpu);
    let from = Instant::now();
    for stage in BOTH_STAGES {
        frames.wait_for(&buffer, stage);
    }
    // Generous: this reads that no wait ran, not how fast one is.
    assert!(from.elapsed() < Duration::from_secs(10));
    assert_eq!(flag.why(), Some("an earlier failure"));
}

#[test]
fn a_second_failure_keeps_the_first_reason() {
    let flag = own_flag();
    let frames = Frames::new(flag);
    frames.set_window(Weak::new());
    assert!(!frames.failed());
    frames.fail("the first");
    frames.fail("the second");
    assert!(frames.failed());
    assert_eq!(flag.why(), Some("the first"));
}

#[cfg(feature = "demo")]
#[test]
fn the_stall_failure_waits_for_a_stage_that_never_comes_and_raises_the_flag() {
    let flag = own_flag();
    const LIMIT: Duration = Duration::from_millis(10);
    let frames = Frames::with_limit(flag, LIMIT);
    let from = Instant::now();
    frames.stall();
    assert!(from.elapsed() >= LIMIT, "it did not wait");
    assert!(flag.why().is_some_and(|why| why.contains("stall")));
    // And with the flag up, no more waiting: one limit for a stalled GPU.
    let frames = Frames::with_limit(flag, Duration::from_secs(60));
    let from = Instant::now();
    frames.stall();
    assert!(from.elapsed() < Duration::from_secs(10));
}

#[test]
fn many_frames_through_the_one_handler_raise_nothing() {
    let flag = own_flag();
    let Some(mut painter) = has_a_device().then(|| Painter::start(Context::default(), false, flag))
    else {
        return;
    };
    let mut delta = with_white();
    let meshes = show(WHITE);
    for frame in 0..10 * SLOTS {
        let encoded = painter.render([8, 8], 1.0, CLEAR, &meshes, &mut delta);
        assert!(encoded.is_some(), "frame {frame}: {:?}", flag.why());
        if let Some(replaced) = painter.present() {
            spare::park(replaced);
        }
    }
    // One more, which completes and draws.
    painter.render([8, 8], 1.0, CLEAR, &meshes, &mut delta);
    let screen = painter.read_screen();
    assert_eq!(screen.size, [8, 8]);
    assert!(screen.pixels.iter().all(|&p| p == Color32::WHITE));
    assert_eq!(flag.why(), None);
}

// ---- a painter that has failed, or has no GPU ----

#[test]
fn a_painter_whose_flag_is_up_paints_nothing_and_never_panics() {
    let flag = own_flag();
    let Some(mut painter) = has_a_device().then(|| Painter::start(Context::default(), false, flag))
    else {
        return;
    };
    // Before the failure the same calls work.
    let mut delta = with_white();
    assert!(
        painter
            .render([8, 8], 1.0, CLEAR, &show(WHITE), &mut delta)
            .is_some()
    );
    assert_eq!(px(&painter.read_screen(), 0, 0), Color32::WHITE);
    assert!(painter.max_texture_side().is_some());

    painter.frames.fail("by hand");
    assert!(flag.is_up());

    let mut delta = Delta::default();
    let image = ColorImage::filled([4, 4], BLUE);
    set(
        &mut delta,
        IMAGE,
        ImageDelta::full(image, TextureOptions::NEAREST),
    );
    // `render` gives no frame, and writes nothing. It leaves the delta as it was: clearing it is
    // `paint_and_update_textures`'s, which a test cannot call with no window, and a delta dropped
    // full panics in a debug build.
    assert!(
        painter
            .render([8, 8], 1.0, CLEAR, &show(IMAGE), &mut delta)
            .is_none()
    );
    assert!(!painter.images.contains_key(&IMAGE), "nothing is written");
    assert!(!delta.set.is_empty());
    // The pieces say so too.
    assert!(painter.update_textures(&delta).is_err());
    assert!(!painter.images.contains_key(&IMAGE), "nothing is written");
    assert!(painter.draw(1.0, CLEAR, &show(WHITE)).is_err());
    // A frame was drawn before, and the screen is still empty: nothing is read from a failed painter.
    let screen = painter.read_screen();
    assert_eq!(screen.size, [0, 0]);
    assert!(screen.pixels.is_empty());

    // The first reason stands through all of it.
    assert_eq!(flag.why(), Some("by hand"));
    painter.destroy();
    painter.destroy();
    assert_eq!(painter.max_texture_side(), None);
    assert_eq!(flag.why(), Some("by hand"));
    // The drop after `destroy` is quiet too.
    drop(painter);
}

#[test]
fn a_painter_with_no_gpu_gives_errors_and_the_flag_goes_up_at_its_first_frame() {
    if !has_a_device() {
        return;
    }
    let flag = own_flag();
    let mut painter = Painter::start(Context::default(), false, flag);
    // What a start that found no GPU leaves: a painter without one, and the flag, which start raises
    // by itself where there is no device. Here the flag is down, to see which call raises it.
    painter.gpu = None;
    assert!(!flag.is_up());
    assert!(painter.target([8, 8]).is_err());
    assert!(painter.sampler(TextureOptions::LINEAR).is_err());
    assert!(painter.draw(1.0, CLEAR, &[]).is_err());
    assert_eq!(painter.max_texture_side(), None);
    assert!(
        !flag.is_up(),
        "the pieces return errors, and `render` fails"
    );

    let mut delta = with_white();
    assert!(
        painter
            .render([8, 8], 1.0, CLEAR, &show(WHITE), &mut delta)
            .is_none()
    );
    assert!(flag.is_up());
    assert!(painter.images.is_empty());
    assert!(painter.drawing.is_none());
    assert_eq!(painter.read_screen().size, [0, 0]);
    painter.destroy();
    drop(painter);
}

#[test]
fn a_geometry_buffer_no_device_gives_is_no_buffer_and_no_panic() {
    let Some(device) = device() else { return };
    let mut buffer: Option<Buffer> = None;
    // Past what a power of two can hold: no size to ask for.
    assert_eq!(room(&device, &mut buffer, usize::MAX), None);
    assert!(buffer.is_none());
    // A size the device refuses.
    assert!(room(&device, &mut buffer, 100).is_some());
    assert!(buffer.is_some());
    assert_eq!(room(&device, &mut buffer, 1 << 62), None);
    assert!(
        buffer.is_none(),
        "the refused buffer is gone, not kept stale"
    );
    // And the next ask recovers.
    assert!(room(&device, &mut buffer, 100).is_some());
    assert_eq!(buffer.as_ref().map(|b| b.length()), Some(SMALLEST_BUFFER));
}

// ---- the failures and the tear that a demo build makes on purpose ----

#[cfg(feature = "demo")]
#[test]
fn only_rest_reads_the_clock_and_only_frame_fails_at_once() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    // `ARRAKIS_METAL_FAIL` is not set in a test, so a painter starts with none.
    painter.fail_at = None;
    assert_eq!(painter.fail_here(), None);
    assert!(
        painter.last_paint.is_none(),
        "a measured drag reads no clock"
    );
    for at in [
        FailAt::Pipeline,
        FailAt::Metallib,
        FailAt::Surface,
        FailAt::Stall,
        FailAt::Device,
    ] {
        painter.fail_at = Some(at);
        assert_eq!(painter.fail_here(), None, "{at:?}");
        assert!(painter.last_paint.is_none(), "{at:?} reads no clock");
    }
    painter.fail_at = Some(FailAt::Frame);
    assert!(painter.fail_here().is_some_and(|why| why.contains("frame")));
    // Once: each failure queues a block and a redraw, until the close lands.
    for paint in 1..3 {
        assert_eq!(painter.fail_here(), None, "paint {paint}");
    }
    assert!(painter.last_paint.is_none(), "frame reads no clock");
}

#[cfg(feature = "demo")]
#[test]
fn rest_fails_the_first_paint_that_comes_a_rest_after_the_one_before() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    painter.fail_at = Some(FailAt::Rest);
    assert_eq!(painter.fail_here(), None, "the first paint is no rest");
    assert!(painter.last_paint.is_some());
    assert_eq!(painter.fail_here(), None, "one soon after");

    // Half a rest ago is not a rest.
    painter.last_paint = Instant::now().checked_sub(REST / 2);
    assert_eq!(painter.fail_here(), None);

    // A rest and a little more ago is.
    painter.last_paint = Instant::now().checked_sub(REST + Duration::from_millis(10));
    assert!(painter.fail_here().is_some_and(|why| why.contains("rest")));
    // And the paint that fails is the new last: the next one is soon after it.
    assert_eq!(painter.fail_here(), None);
    // It says so once: another rest asks for nothing more.
    painter.last_paint = Instant::now().checked_sub(REST + Duration::from_millis(10));
    assert_eq!(painter.fail_here(), None);
}

#[cfg(feature = "demo")]
#[test]
fn the_tear_mode_alternates_two_flat_greys_and_draws_no_mesh() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    // `ARRAKIS_METAL_TEAR` is not set in a test.
    assert!(!painter.tear);
    painter.tear = true;
    let mut delta = with_white();
    // Primitives that would draw white over everything, and a clear color of 51: neither shows.
    let meshes = show(WHITE);
    let mut greys = Vec::new();
    for _ in 0..4 {
        painter.render([8, 8], 1.0, CLEAR, &meshes, &mut delta);
        let screen = painter.read_screen();
        let first = px(&screen, 0, 0);
        assert!(
            screen.pixels.iter().all(|&pixel| pixel == first),
            "one flat grey"
        );
        greys.push(first);
        painter.present();
    }
    let (low, high) = (Color32::from_gray(96), Color32::from_gray(100));
    assert_eq!(greys, [low, high, low, high]);
}

#[cfg(feature = "demo")]
#[test]
fn with_the_tear_mode_off_the_same_call_draws_the_primitives() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    assert!(!painter.tear);
    let mut delta = with_white();
    painter.render([8, 8], 1.0, CLEAR, &show(WHITE), &mut delta);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == Color32::WHITE)
    );
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == CLEARED),
        "and the clear color is the frame's own"
    );
}

// ---- the spare's cell and the closed surface's ----
//
// Both cells are `thread_local!`s. libtest gives each test a thread, but a run with `--test-threads=1`
// runs every test on one, so a surface that one test parked would pass for the next one's. Each test
// begins with `empty_cells`. Real surfaces throughout: a state is read off the surface itself.

const NON_VOLATILE: u32 = IOSurfacePurgeabilityState::PurgeableNonVolatile.bits();
const EMPTY: u32 = IOSurfacePurgeabilityState::PurgeableEmpty.bits();

const RED_CLEAR: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE_CLEAR: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Empties both cells of this thread.
fn empty_cells() {
    drop_spare();
    empty_closed_surface();
}

/// The purgeable state of `surface`, asked of the surface: `PurgeableKeepCurrent` changes nothing and
/// gives the old state back.
fn purgeable_state(surface: &CFRetained<IOSurfaceRef>) -> u32 {
    let mut old = u32::MAX;
    // SAFETY: a live surface, a state `IOSurfaceTypes.h` names, and a pointer to a `u32` that outlives
    // the call.
    let status = unsafe {
        surface.set_purgeable(
            IOSurfacePurgeabilityState::PurgeableKeepCurrent.bits(),
            &mut old,
        )
    };
    assert_eq!(status, libc::KERN_SUCCESS);
    old
}

/// The id of the surface in the spare's cell, if there is one. It takes the surface out and parks it
/// again: a look, which leaves the cell as it was.
fn spare_id() -> Option<u32> {
    let target = spare::take()?;
    let id = target.surface.id();
    spare::park(target);
    Some(id)
}

/// What one frame did to the surfaces: the one it drew into, and the one the layer stopped showing,
/// by id. A `Swap` holds both surfaces: the system hands the id of a surface that is gone to the next
/// one made, so a test that compares ids keeps the `Swap`s it compares alive.
struct Swap {
    drew: u32,
    /// The surface the layer showed before this frame, when it is the size of this one: the test's own
    /// note, read before the call, of the one the frame's tail may park. After a size change the tail
    /// gives back none, and this reads `None`.
    replaced: Option<u32>,
    /// The size of the surface it drew into, read from the surface itself.
    drew_size: [usize; 2],
    _held: Vec<CFRetained<IOSurfaceRef>>,
}

/// One frame of `size` as `paint_and_update_textures` ends it: it draws, then runs the tail the frame
/// runs (`Painter::present_and_wait`) with a wait of the test's own, which parks the surface the layer
/// stopped showing when `parks` and otherwise lets it go. The wait that decides `parks` in the app
/// needs a window to measure and a link to wait for, which no test has: `open-close`'s `spare_drops`
/// reads that one.
fn swap(painter: &mut Painter, delta: &mut TexturesDelta, size: [usize; 2], parks: bool) -> Swap {
    swap_in(painter, delta, size, CLEAR, parks)
}

/// [`swap`], clearing to `color`.
fn swap_in(
    painter: &mut Painter,
    delta: &mut TexturesDelta,
    size: [usize; 2],
    color: [f32; 4],
    parks: bool,
) -> Swap {
    // Read before the frame: a frame of the same size gives back what the layer showed, and one of
    // another size gives back none.
    let same_size = painter.size == size && !painter.shows_old_size;
    let replaced = painter
        .showing
        .as_ref()
        .filter(|_| same_size)
        .map(|target| target.surface.clone());
    let encoded = painter
        .render(size, 1.0, color, &[], delta)
        .expect("a frame");
    let drew = painter.drawing.as_ref().expect("a surface").surface.clone();
    painter.present_and_wait(encoded, |_| Waited {
        took: Duration::ZERO,
        parks,
    });
    Swap {
        drew: drew.id(),
        replaced: replaced.as_ref().map(|surface| surface.id()),
        drew_size: [drew.width(), drew.height()],
        _held: std::iter::once(drew).chain(replaced).collect(),
    }
}

/// A painter that has drawn one frame of `color` and shows it, and the surface it shows, which the
/// test holds a reference to as the window server may.
fn showing_one_frame(color: [f32; 4]) -> Option<(Painter, CFRetained<IOSurfaceRef>)> {
    let mut painter = painter(false)?;
    let mut delta = with_white();
    swap_in(&mut painter, &mut delta, [8, 8], color, false);
    let surface = painter
        .showing
        .as_ref()
        .expect("a surface shown")
        .surface
        .clone();
    Some((painter, surface))
}

/// What the painter's `showing` surface holds, which `read_screen` does not read: it reads the surface
/// being drawn.
fn showing_pixels(painter: &mut Painter) -> ColorImage {
    assert!(painter.drawing.is_none());
    painter.drawing = painter.showing.take();
    let image = painter.read_screen();
    painter.showing = painter.drawing.take();
    image
}

/// Whether `layer`'s contents are `surface`.
fn layer_shows(layer: &CALayer, surface: &CFRetained<IOSurfaceRef>) -> bool {
    // SAFETY: a plain read of a live layer's contents.
    let Some(contents) = (unsafe { layer.contents() }) else {
        return false;
    };
    Retained::as_ptr(&contents) == CFRetained::as_ptr(surface).as_ptr().cast_const().cast()
}

/// A painter whose layer sits in a layer of its own, as a view's does, and that layer.
fn painter_in_a_view() -> Option<(Painter, Retained<CALayer>, Retained<CALayer>)> {
    let mut painter = painter(false)?;
    let layer = give_a_layer(&mut painter);
    let view = CALayer::new();
    view.addSublayer(&layer);
    Some((painter, layer, view))
}

/// Polls a future that has no await point which waits: `set_window` is `async` for eframe's sake.
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("a future that waits"),
    }
}

#[test]
fn a_parked_surface_comes_back_as_the_same_one_and_only_once() {
    let Some(painter) = painter(false) else {
        return;
    };
    empty_cells();
    let target = painter.target([8, 8]).expect("a target");
    let id = target.surface.id();
    spare::park(target);
    assert_eq!(spare::take().map(|target| target.surface.id()), Some(id));
    assert!(spare::take().is_none(), "the cell is empty after a take");
}

#[test]
fn dropping_the_spare_says_whether_it_dropped_one_and_leaves_the_cell_empty() {
    let Some(painter) = painter(false) else {
        return;
    };
    empty_cells();
    assert!(!drop_spare(), "nothing was parked");
    spare::park(painter.target([8, 8]).expect("a target"));
    assert!(drop_spare());
    assert!(!drop_spare(), "and nothing is left");
    assert!(spare::take().is_none());
}

#[test]
fn dropping_the_spare_leaves_the_surface_the_layer_shows() {
    let Some((mut painter, layer, _view)) = painter_in_a_view() else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    swap_in(&mut painter, &mut delta, [8, 8], RED_CLEAR, true);
    swap_in(&mut painter, &mut delta, [8, 8], BLUE_CLEAR, true);
    let showing = painter.showing.as_ref().expect("a surface shown");
    let (id, surface) = (showing.surface.id(), showing.surface.clone());
    assert!(spare_id().is_some_and(|spare| spare != id), "the other one");

    assert!(drop_spare());

    let showing = painter.showing.as_ref().expect("still shown");
    assert_eq!(showing.surface.id(), id);
    assert!(
        layer_shows(&layer, &surface),
        "the layer's contents are the same"
    );
    assert_eq!(purgeable_state(&surface), NON_VOLATILE);
    let image = showing_pixels(&mut painter);
    assert_eq!(image.size, [8, 8]);
    assert!(
        image.pixels.iter().all(|&pixel| pixel == BLUE),
        "its pixels still read"
    );
}

#[test]
fn taking_the_painter_down_empties_the_spare_and_lets_go_of_what_it_drew_and_showed() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    swap(&mut painter, &mut delta, [8, 8], true);
    swap(&mut painter, &mut delta, [8, 8], true);
    // A frame under way, and a spare beside it.
    painter.render([8, 8], 1.0, CLEAR, &[], &mut delta);
    spare::park(painter.target([8, 8]).expect("a target"));
    assert!(painter.drawing.is_some() && painter.showing.is_some());
    assert!(spare_id().is_some());

    painter.destroy();

    assert!(painter.drawing.is_none());
    assert!(painter.showing.is_none());
    assert!(!drop_spare(), "the spare went with the close");
    // eframe's other way in, and then the drop at the end of this function.
    painter.destroy();
}

#[test]
fn a_new_size_empties_the_spare_keeps_what_is_shown_and_the_next_surface_has_it() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let first = swap(&mut painter, &mut delta, [8, 8], true);
    let second = swap(&mut painter, &mut delta, [8, 8], true);
    assert_eq!(spare_id(), Some(first.drew), "the first is parked");
    let old: BTreeSet<u32> = [first.drew, second.drew].into();

    painter.render([16, 12], 1.0, CLEAR, &[], &mut delta);

    let shown = painter.showing.as_ref().expect("the layer still shows it");
    assert_eq!(shown.surface.id(), second.drew);
    assert_eq!(spare_id(), None, "the parked one went, which no frame took");
    let drawing = painter.drawing.as_ref().expect("a surface");
    assert_eq!(
        (drawing.surface.width(), drawing.surface.height()),
        (16, 12)
    );
    assert!(!old.contains(&drawing.surface.id()), "a surface of its own");
}

#[test]
fn a_lone_frame_takes_nothing_makes_a_surface_and_lets_the_old_one_go() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let mut swaps: Vec<Swap> = Vec::new();
    for frame in 0..4 {
        let swap = swap(&mut painter, &mut delta, [8, 8], false);
        // The old surface is the last frame's, and it is not parked for a pause to drop.
        assert_eq!(
            swap.replaced,
            swaps.last().map(|last| last.drew),
            "frame {frame}"
        );
        assert_eq!(spare_id(), None, "frame {frame} parks nothing");
        swaps.push(swap);
    }
    let drew: BTreeSet<u32> = swaps.iter().map(|swap| swap.drew).collect();
    assert_eq!(drew.len(), 4, "a surface made for each");
}

#[test]
fn a_burst_takes_the_spare_and_two_surfaces_alternate_with_no_third() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let mut swaps: Vec<Swap> = Vec::new();
    for frame in 0..7 {
        let swap = swap(&mut painter, &mut delta, [8, 8], true);
        match frame {
            0 => assert_eq!(spare_id(), None, "nothing was shown, so nothing is parked"),
            _ => assert_eq!(
                spare_id(),
                swap.replaced,
                "frame {frame} parks what it replaced"
            ),
        }
        swaps.push(swap);
    }
    let drew: Vec<u32> = swaps.iter().map(|swap| swap.drew).collect();
    let (a, b) = (drew[0], drew[1]);
    assert_ne!(a, b);
    let want: Vec<u32> = (0..7)
        .map(|frame| if frame % 2 == 0 { a } else { b })
        .collect();
    assert_eq!(drew, want, "A, B, A, B and so on");
}

#[test]
fn after_the_spare_is_dropped_a_burst_makes_two_surfaces_and_then_takes_the_spare() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let burst: Vec<Swap> = (0..4)
        .map(|_| swap(&mut painter, &mut delta, [8, 8], true))
        .collect();
    let before: BTreeSet<u32> = burst.iter().map(|swap| swap.drew).collect();
    assert_eq!(before.len(), 2);
    // The link's pause.
    assert!(drop_spare());

    // The burst's first frame comes after a gap, with the link paused, so it parks nothing. The
    // second starts the link and finds the cell empty, and the third takes what the second parked.
    let first = swap(&mut painter, &mut delta, [8, 8], false);
    let second = swap(&mut painter, &mut delta, [8, 8], true);
    let third = swap(&mut painter, &mut delta, [8, 8], true);
    assert_ne!(first.drew, second.drew, "two surfaces made");
    assert_eq!(third.drew, first.drew, "and then the spare");
    assert!(!before.contains(&first.drew) && !before.contains(&second.drew));
    assert_eq!(spare_id(), Some(second.drew));
}

#[test]
fn read_screen_reads_the_surface_being_drawn_and_says_nothing_once_it_is_presented() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    painter.render([8, 8], 1.0, RED_CLEAR, &[], &mut delta);
    assert!(painter.present().is_none());
    // The red frame is shown, and the surface being drawn is none.
    let screen = painter.read_screen();
    assert_eq!(screen.size, [0, 0]);
    assert!(screen.pixels.is_empty());
    // The next frame is what it reads, not the surface that shows red.
    painter.render([8, 8], 1.0, BLUE_CLEAR, &[], &mut delta);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == BLUE),
        "the surface being drawn"
    );
    assert!(
        showing_pixels_after_a_draw(&mut painter)
            .pixels
            .iter()
            .all(|&pixel| pixel == RED),
        "and the shown one is still red"
    );
}

/// [`showing_pixels`] for a painter that has a frame being drawn: that frame is put aside and back.
fn showing_pixels_after_a_draw(painter: &mut Painter) -> ColorImage {
    let drawing = painter.drawing.take();
    let image = showing_pixels(painter);
    painter.drawing = drawing;
    image
}

// ---- the closed surface ----

#[test]
fn emptying_with_nothing_parked_is_false_and_leaves_a_shown_surface_alone() {
    let Some((mut painter, surface)) = showing_one_frame(RED_CLEAR) else {
        return;
    };
    empty_cells();
    assert!(!empty_closed_surface());
    assert!(!empty_closed_surface(), "and again");
    assert_eq!(purgeable_state(&surface), NON_VOLATILE);
    assert!(
        showing_pixels(&mut painter)
            .pixels
            .iter()
            .all(|&pixel| pixel == RED)
    );
}

#[test]
fn a_close_parks_the_shown_surface_and_does_not_empty_it_until_asked() {
    let Some((mut painter, surface)) = showing_one_frame(RED_CLEAR) else {
        return;
    };
    empty_cells();
    painter.destroy();
    assert!(painter.showing.is_none());
    assert!(painter.drawing.is_none());
    // The window may still be up: `take_down` alone must not blank what it shows.
    assert_eq!(purgeable_state(&surface), NON_VOLATILE);

    assert!(empty_closed_surface());
    assert_eq!(purgeable_state(&surface), EMPTY);
    assert!(!empty_closed_surface(), "once");
}

#[test]
fn a_destroy_then_the_drop_park_one_surface_and_one_emptying_finds_it() {
    let Some((mut painter, surface)) = showing_one_frame(RED_CLEAR) else {
        return;
    };
    empty_cells();
    painter.destroy();
    drop(painter);
    assert!(empty_closed_surface());
    assert_eq!(
        purgeable_state(&surface),
        EMPTY,
        "the surface the destroy parked, not another"
    );
    assert!(!empty_closed_surface());
}

#[test]
fn a_painter_that_never_drew_leaves_nothing_to_empty() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    painter.destroy();
    assert!(!empty_closed_surface());
}

#[test]
fn a_second_painters_close_drops_the_first_ones_surface_unemptied_and_parks_its_own() {
    let Some((mut first, first_surface)) = showing_one_frame(RED_CLEAR) else {
        return;
    };
    let Some((mut second, second_surface)) = showing_one_frame(BLUE_CLEAR) else {
        return;
    };
    empty_cells();
    first.destroy();
    second.destroy();
    // The first surface is let go of by the cell: this test's reference is all that is left of it,
    // and nothing emptied it.
    assert_eq!(purgeable_state(&first_surface), NON_VOLATILE);
    assert_eq!(purgeable_state(&second_surface), NON_VOLATILE);

    assert!(empty_closed_surface());
    assert_eq!(purgeable_state(&second_surface), EMPTY, "the new one");
    assert_eq!(purgeable_state(&first_surface), NON_VOLATILE, "not the old");
    assert!(!empty_closed_surface());
}

#[test]
fn a_new_painters_frame_after_an_emptying_reads_its_own_pixels() {
    let Some((mut old, old_surface)) = showing_one_frame(RED_CLEAR) else {
        return;
    };
    empty_cells();
    old.destroy();
    assert!(empty_closed_surface());
    assert_eq!(purgeable_state(&old_surface), EMPTY);

    let Some(mut painter) = painter(false) else {
        return;
    };
    let mut delta = with_white();
    painter.render([8, 8], 1.0, BLUE_CLEAR, &[], &mut delta);
    let drawing = painter.drawing.as_ref().expect("a surface");
    assert_ne!(
        drawing.surface.id(),
        old_surface.id(),
        "not the emptied one"
    );
    assert_eq!(purgeable_state(&drawing.surface), NON_VOLATILE);
    assert!(
        painter
            .read_screen()
            .pixels
            .iter()
            .all(|&pixel| pixel == BLUE),
        "its own pixels"
    );
}

#[test]
fn a_close_leaves_the_layer_in_its_superlayer_with_its_contents() {
    let Some((mut painter, layer, view)) = painter_in_a_view() else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    swap_in(&mut painter, &mut delta, [8, 8], RED_CLEAR, false);
    let surface = painter.showing.as_ref().expect("a surface").surface.clone();
    assert!(layer_shows(&layer, &surface));

    painter.destroy();

    assert!(painter.shown.is_none(), "the painter lets go of the layer");
    let superlayer = layer.superlayer().expect("still in the view's layer");
    assert_eq!(Retained::as_ptr(&superlayer), Retained::as_ptr(&view));
    assert!(
        layer_shows(&layer, &surface),
        "and still shows its contents"
    );
    assert_eq!(purgeable_state(&surface), NON_VOLATILE);
}

#[test]
fn setting_no_window_clears_the_layer_and_takes_it_out_and_leaves_the_surfaces_alone() {
    // A surface parked by an earlier close, and a spare, which this painter does not own.
    let Some((mut earlier, earlier_surface)) = showing_one_frame(BLUE_CLEAR) else {
        return;
    };
    empty_cells();
    earlier.destroy();
    let Some((mut painter, layer, _view)) = painter_in_a_view() else {
        return;
    };
    let mut delta = with_white();
    swap_in(&mut painter, &mut delta, [8, 8], RED_CLEAR, true);
    let spare = painter.target([8, 8]).expect("a target");
    let spare_surface = spare.surface.id();
    spare::park(spare);
    let surface = painter.showing.as_ref().expect("a surface").surface.clone();
    assert!(layer_shows(&layer, &surface));

    block_on(painter.set_window(ViewportId::ROOT, None)).expect("never an error");

    // SAFETY: a plain read of a live layer's contents.
    assert!(unsafe { layer.contents() }.is_none(), "cleared");
    assert!(layer.superlayer().is_none(), "and out of the view's layer");
    assert!(painter.shown.is_none());
    // The surface it showed is still the painter's, as is the cell's spare and the parked one.
    let showing = painter.showing.as_ref().expect("still the painter's");
    assert_eq!(showing.surface.id(), surface.id());
    assert_eq!(purgeable_state(&surface), NON_VOLATILE);
    assert_eq!(spare_id(), Some(spare_surface));
    assert!(empty_closed_surface(), "the earlier close's surface waits");
    assert_eq!(purgeable_state(&earlier_surface), EMPTY);
}

// ---- the frame's tail: `present_and_wait`, which the frame runs and `swap_in` runs too ----

#[test]
fn the_tail_parks_the_surface_the_layer_stopped_showing_when_the_wait_says_so() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let first = swap(&mut painter, &mut delta, [8, 8], true);
    assert_eq!(
        spare_id(),
        None,
        "the layer showed nothing before the first"
    );
    let second = swap(&mut painter, &mut delta, [8, 8], true);
    assert_eq!(second.replaced, Some(first.drew));
    assert_eq!(
        spare_id(),
        Some(first.drew),
        "the cell holds the surface the layer stopped showing"
    );
    let third = swap(&mut painter, &mut delta, [8, 8], true);
    assert_eq!(third.drew, first.drew, "and the next frame draws into it");
    assert_eq!(spare_id(), Some(second.drew), "and parks the next in turn");
}

#[test]
fn the_tail_lets_the_surface_go_when_the_wait_says_not_and_the_next_frame_makes_a_new_one() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let first = swap(&mut painter, &mut delta, [8, 8], false);
    let second = swap(&mut painter, &mut delta, [8, 8], false);
    assert_eq!(
        second.replaced,
        Some(first.drew),
        "the layer stopped showing it"
    );
    assert_eq!(spare_id(), None, "and the cell is empty");
    let third = swap(&mut painter, &mut delta, [8, 8], false);
    let drew: BTreeSet<u32> = [first.drew, second.drew, third.drew].into();
    assert_eq!(drew.len(), 3, "a surface made for each");
}

#[test]
fn the_tail_hands_the_layer_its_surface_before_the_wait_and_waits_once() {
    let Some((mut painter, layer, _view)) = painter_in_a_view() else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let encoded = painter
        .render([8, 8], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    let surface = painter.drawing.as_ref().expect("a surface").surface.clone();
    assert!(!layer_shows(&layer, &surface), "not before the tail");

    let waits = Cell::new(0);
    let shown_when_it_waited = Cell::new(false);
    painter.present_and_wait(encoded, |_| {
        waits.set(waits.get() + 1);
        shown_when_it_waited.set(layer_shows(&layer, &surface));
        Waited {
            took: Duration::ZERO,
            parks: false,
        }
    });

    assert_eq!(waits.get(), 1);
    assert!(
        shown_when_it_waited.get(),
        "the layer shows it as the wait begins"
    );
    assert!(layer_shows(&layer, &surface));
    assert!(painter.drawing.is_none());
    assert_eq!(
        painter.showing.as_ref().map(|target| target.surface.id()),
        Some(surface.id())
    );
}

#[test]
fn the_tail_reports_the_time_it_presented_and_the_time_the_wait_took() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let encoded = painter
        .render([8, 8], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    // Encoded two seconds ago, and waited one: three at least, whatever this machine's speed. Each
    // term alone is short of that.
    let encoded = encoded
        .checked_sub(Duration::from_secs(2))
        .expect("a clock that has run for 2 s");
    let reported = painter.present_and_wait(encoded, |_| Waited {
        took: Duration::from_secs(1),
        parks: false,
    });
    assert!(reported >= 3.0, "{reported}");
    assert!(reported < 60.0, "{reported}");
}

// ---- the close: what an unfinished frame may still render into ----

/// A command buffer that is never committed, so that its status stays `NotEnqueued` and never
/// reaches a final one. A stalled frame's own buffer is committed and finishes by itself.
fn a_buffer_that_never_finishes(painter: &Painter) -> CommandBuffer {
    let buffer = empty_buffer(painter.gpu.as_ref().expect("a GPU"));
    assert!(buffer.status() == MTLCommandBufferStatus::NotEnqueued);
    buffer
}

/// Waits for every frame in flight, as a wait of the test's own with the flag down: each slot then
/// holds a handle that reads finished.
fn settle_all(painter: &Painter) {
    painter
        .slots
        .iter()
        .for_each(|slot| slot.settle(&painter.frames));
    assert!(!painter.frames.failed());
    assert!(painter.slots.iter().all(Slot::finished));
}

/// The surface in the spare's cell, if there is one: a look, which leaves the cell as it was.
fn spare_surface() -> Option<CFRetained<IOSurfaceRef>> {
    let target = spare::take()?;
    let surface = target.surface.clone();
    spare::park(target);
    Some(surface)
}

/// A painter that holds three surfaces at once, one in each place a close looks: the one being
/// drawn, the one the layer shows and the spare. The real path holds two at most; the test needs
/// three to tell them apart. Clones of them are returned, as `[drawing, showing, spare]`, with every
/// slot holding the handle of a frame.
fn holding_three(
    painter: &mut Painter,
    delta: &mut TexturesDelta,
) -> [CFRetained<IOSurfaceRef>; 3] {
    swap(painter, delta, [8, 8], true);
    swap(painter, delta, [8, 8], true);
    // The layer shows the second now, the first is the spare, and this frame takes it.
    painter
        .render([8, 8], 1.0, CLEAR, &[], delta)
        .expect("a frame");
    spare::park(painter.target([8, 8]).expect("a target"));
    let held = [
        painter.drawing.as_ref().expect("a surface").surface.clone(),
        painter.showing.as_ref().expect("a surface").surface.clone(),
        spare_surface().expect("a spare"),
    ];
    let ids: BTreeSet<u32> = held.iter().map(|surface| surface.id()).collect();
    assert_eq!(ids.len(), 3, "three surfaces");
    assert!(painter.slots.iter().all(|slot| slot.last.is_some()));
    held
}

#[test]
fn a_slot_is_finished_with_no_handle_and_with_a_completed_one_and_not_with_one_in_flight() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    assert!(
        painter.slots.iter().all(Slot::finished),
        "no frame, no handle"
    );
    let mut delta = with_white();
    swap(&mut painter, &mut delta, [8, 8], false);
    settle_all(&painter);
    let slot = painter
        .slots
        .iter()
        .find(|slot| slot.last.is_some())
        .expect("a slot with the frame's handle");
    assert!(
        slot.last
            .as_ref()
            .is_some_and(|last| has_reached(last, Stage::Completed))
    );
    assert!(slot.finished(), "a completed buffer");

    // A status of `Error` is final too (`wait.rs`'s table of statuses); a real buffer that errors
    // needs a GPU reset, which no test makes. A buffer no one commits never finishes, and a wait that
    // runs out leaves it unfinished.
    let slot = Slot {
        last: Some(a_buffer_that_never_finishes(&painter)),
        ..Slot::default()
    };
    assert!(!slot.finished(), "a buffer that is not even enqueued");
    let frames = Frames::with_limit(own_flag(), Duration::from_millis(5));
    slot.settle(&frames);
    assert!(frames.failed(), "the wait ran out");
    assert!(
        !slot.finished(),
        "a wait that failed leaves the handle, and it does not read as finished"
    );
}

#[test]
fn a_close_over_a_frame_that_never_finishes_parks_nothing_and_empties_nothing() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let held = holding_three(&mut painter, &mut delta);
    settle_all(&painter);
    // One slot's frame never finishes and the others have: `all` and not `any`.
    painter.slots[0].last = Some(a_buffer_that_never_finishes(&painter));
    assert!(!painter.slots[0].finished());
    assert!(painter.slots[1].finished() && painter.slots[2].finished());
    // The flag is up, as it is when a wait has run out: the close does not wait.
    painter.frames.fail("by hand");

    painter.destroy();

    assert!(
        !empty_closed_surface(),
        "no surface was parked, so none is emptied"
    );
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), NON_VOLATILE, "{place}");
    }
}

#[test]
fn a_close_after_a_healthy_frame_waits_and_parks_every_surface() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let held = holding_three(&mut painter, &mut delta);

    painter.destroy();

    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(
            purgeable_state(surface),
            NON_VOLATILE,
            "{place}: not before asked"
        );
    }
    assert!(empty_closed_surface());
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), EMPTY, "{place}");
    }
    assert!(!empty_closed_surface(), "once");
}

#[test]
fn a_failed_window_whose_frames_have_all_finished_parks_its_surfaces_too() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let held = holding_three(&mut painter, &mut delta);
    // Final by a wait of the test's own, and then the failure.
    settle_all(&painter);
    painter.frames.fail("by hand");

    painter.destroy();

    assert!(empty_closed_surface());
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), EMPTY, "{place}");
    }
}

#[test]
fn a_frame_that_finds_the_painter_failed_keeps_every_slots_handle() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    holding_three(&mut painter, &mut delta);
    painter.frames.fail("by hand");

    // `draw` visits a slot at each call: all of them in turn.
    for call in 0..SLOTS {
        assert!(painter.draw(1.0, CLEAR, &[]).is_err(), "call {call}");
    }
    assert!(
        painter.slots.iter().all(|slot| slot.last.is_some()),
        "draw let go of a handle on a wait that failed"
    );

    // And `update_textures` with a delta, which settles all of them.
    let delta = with_white();
    assert!(painter.update_textures(&delta).is_err());
    assert!(
        painter.slots.iter().all(|slot| slot.last.is_some()),
        "update_textures let go of a handle on a wait that failed"
    );
}

#[test]
fn a_stalled_frame_is_not_forgotten_by_a_later_frame_that_found_the_painter_failed() {
    // The fault this guards: a failed wait cleared the handle, and the close then read `None` as a
    // frame that had finished, and parked surfaces the stalled one renders into. The failure comes
    // after the handle is put in, as a wait that ran out would have raised it.
    for through in ["draw", "update_textures"] {
        let Some(mut painter) = painter(false) else {
            return;
        };
        empty_cells();
        let mut delta = with_white();
        let held = holding_three(&mut painter, &mut delta);
        settle_all(&painter);
        painter.slots[0].last = Some(a_buffer_that_never_finishes(&painter));
        painter.frames.fail("by hand");

        match through {
            "draw" => {
                for _ in 0..SLOTS {
                    assert!(painter.draw(1.0, CLEAR, &[]).is_err());
                }
            }
            _ => assert!(painter.update_textures(&with_white()).is_err()),
        }
        painter.destroy();

        assert!(!empty_closed_surface(), "{through}: it parked");
        for surface in &held {
            assert_eq!(purgeable_state(surface), NON_VOLATILE, "{through}");
        }
    }
}

// ---- the close: every surface the painter holds goes through the cell ----

#[test]
fn a_close_empties_all_three_places_a_surface_is_held_and_nothing_before_asked() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let held = holding_three(&mut painter, &mut delta);

    painter.destroy();

    assert!(painter.drawing.is_none() && painter.showing.is_none());
    assert!(spare_surface().is_none(), "the spare went with the close");
    // The window may still be up: `take_down` itself empties none.
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), NON_VOLATILE, "{place}");
    }
    assert!(empty_closed_surface(), "true for many");
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), EMPTY, "{place}");
    }
    assert!(!empty_closed_surface(), "false with none");
}

#[test]
fn a_destroy_then_the_drop_leave_the_three_parked_and_one_emptying_finds_them() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let held = holding_three(&mut painter, &mut delta);

    painter.destroy();
    // The drop closes again, with nothing held: a close with none leaves the cell as it is.
    drop(painter);

    assert!(empty_closed_surface());
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&held) {
        assert_eq!(purgeable_state(surface), EMPTY, "{place}");
    }
    assert!(!empty_closed_surface());
}

#[test]
fn a_close_over_surfaces_nothing_emptied_drops_them_and_only_the_new_ones_read_empty() {
    let (Some(mut first), Some(mut second)) = (painter(false), painter(false)) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let old = holding_three(&mut first, &mut delta);
    first.destroy();
    // The second painter takes the shared spare cell after the first closed over it.
    let mut delta = with_white();
    let new = holding_three(&mut second, &mut delta);
    second.destroy();

    for surface in old.iter().chain(&new) {
        assert_eq!(purgeable_state(surface), NON_VOLATILE);
    }
    assert!(empty_closed_surface());
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&new) {
        assert_eq!(purgeable_state(surface), EMPTY, "new {place}");
    }
    for (place, surface) in ["drawing", "showing", "spare"].into_iter().zip(&old) {
        assert_eq!(purgeable_state(surface), NON_VOLATILE, "old {place}");
    }
    assert!(!empty_closed_surface());
}

// ---- a size change keeps what the layer shows ----

/// A size no surface can have: `IOSurfaceCreate` gives none.
const NO_SURFACE: [usize; 2] = [1 << 40, 1 << 40];

/// A painter with two frames of 8 by 8 shown and parked, every frame final, and a clone of the
/// surface the layer shows.
fn showing_a_burst() -> Option<(Painter, CFRetained<IOSurfaceRef>)> {
    let mut painter = painter(false)?;
    let mut delta = with_white();
    swap(&mut painter, &mut delta, [8, 8], true);
    swap(&mut painter, &mut delta, [8, 8], true);
    settle_all(&painter);
    let shown = painter.showing.as_ref().expect("a surface").surface.clone();
    Some((painter, shown))
}

#[test]
fn a_size_change_that_fails_on_the_flag_leaves_the_surface_the_layer_shows_to_the_close() {
    empty_cells();
    let Some((mut painter, shown)) = showing_a_burst() else {
        return;
    };
    painter.frames.fail("by hand");
    let mut delta = Delta::default();

    assert!(
        painter
            .render([16, 12], 1.0, CLEAR, &[], &mut delta)
            .is_none()
    );

    let still = painter.showing.as_ref().expect("the layer still shows it");
    assert_eq!(still.surface.id(), shown.id());
    painter.destroy();
    assert!(empty_closed_surface());
    assert_eq!(purgeable_state(&shown), EMPTY, "the close parked it");
}

#[test]
fn a_size_change_that_fails_for_want_of_a_surface_leaves_the_layers_one_to_the_close() {
    empty_cells();
    let Some((mut painter, shown)) = showing_a_burst() else {
        return;
    };
    assert!(
        new_surface(NO_SURFACE[0], NO_SURFACE[1]).is_none(),
        "the system gives no surface of this size"
    );
    let mut delta = Delta::default();

    assert!(
        painter
            .render(NO_SURFACE, 1.0, CLEAR, &[], &mut delta)
            .is_none()
    );
    assert!(painter.frames.failed(), "the failed frame raised the flag");

    let still = painter.showing.as_ref().expect("the layer still shows it");
    assert_eq!(still.surface.id(), shown.id());
    assert!(painter.drawing.is_none(), "no surface was made");
    painter.destroy();
    assert!(empty_closed_surface());
    assert_eq!(purgeable_state(&shown), EMPTY, "the close parked it");
}

#[test]
fn a_size_change_gives_back_no_surface_and_the_next_frame_of_that_size_gives_back_one() {
    empty_cells();
    let Some((mut painter, _shown)) = showing_a_burst() else {
        return;
    };
    let mut delta = Delta::default();

    // The old size's spare went at the size change, and what the layer showed is of the old size.
    painter
        .render([16, 12], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    assert!(painter.present().is_none(), "the old one is not given back");
    assert!(spare_id().is_none());

    // The next frame of the same size gives back the one before it.
    painter
        .render([16, 12], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    let before = painter.showing.as_ref().expect("a surface").surface.id();
    let given = painter.present().expect("the surface before it");
    assert_eq!(given.surface.id(), before);
    assert_eq!(
        (given.surface.width(), given.surface.height()),
        (16, 12),
        "of the new size"
    );
}

#[test]
fn after_a_size_change_no_surface_of_the_old_size_reaches_the_cell_and_a_burst_goes_on() {
    empty_cells();
    let Some((mut painter, _shown)) = showing_a_burst() else {
        return;
    };
    let mut delta = Delta::default();

    let changed = swap(&mut painter, &mut delta, [16, 12], true);
    assert_eq!(
        changed.replaced, None,
        "the old surface is given back to nobody"
    );
    assert_eq!(changed.drew_size, [16, 12]);
    assert_eq!(spare_id(), None, "so the cell stays empty");

    // The frame after it gives back the one the change drew, which parks, of the new size.
    let next = swap(&mut painter, &mut delta, [16, 12], true);
    assert_eq!(next.replaced, Some(changed.drew));
    assert_eq!(next.drew_size, [16, 12]);
    assert_ne!(next.drew, changed.drew);
    assert_eq!(spare_id(), Some(changed.drew));
    let spare = spare_surface().expect("a spare");
    assert_eq!((spare.width(), spare.height()), (16, 12));

    // And the burst goes on with two surfaces: the next draws into the spare.
    let after = swap(&mut painter, &mut delta, [16, 12], true);
    assert_eq!(after.drew, changed.drew);
    assert_eq!(after.drew_size, [16, 12]);
}

#[test]
fn a_frame_that_failed_after_the_size_change_leaves_the_old_surface_from_the_cell_for_good() {
    empty_cells();
    let Some((mut painter, _shown)) = showing_a_burst() else {
        return;
    };
    let mut delta = Delta::default();
    // A frame that fails after the size branch with the flag down, as `render` would raise it: no
    // GPU to make the surface with. The size is already the new one.
    let gpu = painter.gpu.take();
    assert!(
        painter
            .try_render([16, 12], 1.0, CLEAR, &[], &mut delta)
            .is_err()
    );
    assert!(!painter.frames.failed(), "try_render raises no flag");
    painter.gpu = gpu;

    // The frame that succeeds at that size: the size branch does not run, and the layer still shows
    // a surface of the old size, which present gives back to nobody.
    painter
        .try_render([16, 12], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    assert!(painter.present().is_none());
    assert_eq!(spare_id(), None);

    // And once, as the next frame gives back its own.
    painter
        .try_render([16, 12], 1.0, CLEAR, &[], &mut delta)
        .expect("a frame");
    assert!(painter.present().is_some());
}

#[test]
fn a_frame_of_the_same_size_gives_back_the_surface_before_it() {
    let Some(mut painter) = painter(false) else {
        return;
    };
    empty_cells();
    let mut delta = with_white();
    let first = swap(&mut painter, &mut delta, [8, 8], true);
    assert!(!painter.shows_old_size, "the first present took the flag");
    let second = swap(&mut painter, &mut delta, [8, 8], true);
    assert!(!painter.shows_old_size);
    assert_eq!(second.replaced, Some(first.drew));
    assert_eq!(spare_id(), Some(first.drew));
}
