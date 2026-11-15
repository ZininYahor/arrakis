//! The cover of the night look: a maw and dunes under a low sun. The maw
//! comes up out of the sand as far as the look is red, showing as soon as it
//! stirs, and at pure red it stands sunk in the sand in front of the sun,
//! covering most of it; none of the worm's body is drawn. The maw is a lip and
//! a throat ([`shapes`]) with many small teeth in the throat, in rings
//! ([`rings`]). The shapes are sampled into layers once per size and sent to
//! the GPU as six textures, drawn back to front: the back, the maw, the rim's
//! light, the shade at the sand, a gap and the front. The maw, the gap and the
//! front are each cropped to the box of pixels they fill. A new rise moves the
//! maw's rect, and the CPU composes the rim's light again for it, and the
//! shade where the rise moves its picture, each in a small box, and sends the
//! rows of each box from the first that is not clear in the old picture or
//! the new to the last: the light slides with the maw, but the sun that
//! lights it stands still (see [`RimLight`]); the teeth sink into the
//! throat's dark near the sand, which stands still too (see [`Shade`]); no
//! rect can draw either. Nothing else is
//! built or uploaded, and a frame at the same rise composes nothing. The
//! look's tint is applied as the textures are drawn. The images are built
//! before the window's first frame, on a thread started when the window is
//! asked for (see [`prebuilt`]).

use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use egui::{
    Color32, ColorImage, Id, Pos2, Rect, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};

use crate::color::Tint;
use crate::theme::Cover;
use shapes::{DUNES, GAP, GRID, GROUND, LIP, MAW, Part, THROAT, maw};

#[cfg(feature = "demo")]
pub(crate) mod banner;
mod rings;
pub(crate) mod shapes;

/// The cover's maw is its shape (`shapes`) at this size.
const SCALE: f32 = 0.8;
/// Where the maw stands when it is all the way out: sunk so far that its
/// lowest point is at the crest of the sand in front, and the sand and its
/// gap hide the bottom of it.
pub(crate) const HEAD: Pos2 = pos2(9.0, 11.6);
/// The sun's radius, a little more than the lip's.
pub(crate) const SUN: f32 = (LIP + 1.1) * SCALE;
/// The sun's center: above the head, so that at pure red the maw covers most
/// of the sun and a crescent of it shows above.
pub(crate) const SUN_CENTER: Pos2 = pos2(HEAD.x, 8.8);
/// The line above the mound where the sand's keep-out begins at the head: the
/// maw shows only above it.
const KEEP_OUT: f32 = GROUND - MOUND.0 - GAP;
/// How far down the maw is before it stirs: the top of its lip at the
/// keep-out line, so nothing of it shows, and the first stir of the slider
/// brings it into view. Measured from the ground, the first stretch of the
/// rise would be spent under the keep-out, and the slider would move with
/// the cover standing still.
const DROP: f32 = KEEP_OUT - (HEAD.y - LIP * SCALE);
/// How red the look gets before the maw stirs.
const STIRS: f32 = 0.25;
/// How far above the sand's keep-out line the teeth start to sink into the
/// throat's dark, in the grid's units: through the sand's gap the outer teeth
/// would meet the low sky and the far dune, which no flat color of theirs
/// stands apart from.
const SHADE: f32 = 1.0;
/// The hump of sand it comes up through: its height and half its width.
const MOUND: (f32, f32) = (1.0, 4.6);
/// Samples per pixel along each side, on a pixel an edge crosses and on
/// every pixel of the throat.
const SAMPLES: usize = 4;
/// Clear pixels around the maw's image, so that sampling between its edge and
/// the clear beyond it, as the maw slides, finds clear and not the edge.
const MARGIN: usize = 1;
/// How the cover's textures are sampled. The maw's rect sits a fraction of a
/// pixel off the pixel grid, so each of its pixels falls between two rows of
/// its image, and linear filtering blends them by their distance, as
/// `compose` blends two rows by `lerp_to_gamma`. Drawn one texel to a device
/// pixel, the GPU may call that magnifying or minifying: both are linear.
/// Wrapping clamps to the edge, where the margin is clear, and there are no
/// mipmaps, since nothing is drawn smaller than it is. That is exactly egui's
/// `TextureOptions::LINEAR`.
const FILTER: TextureOptions = TextureOptions::LINEAR;

/// How far out of the sand the maw stands, from 0 to 1, for a look
/// `redness` of the way to pure red: it stirs a quarter of the way, where it
/// starts to show, and is all the way out at pure red. The rise is a
/// smoothstep of the red between those two, so the maw eases up as it stirs
/// and eases to a stop as it arrives.
fn rise(redness: f32) -> f32 {
    smoothstep(((redness - STIRS) / (1.0 - STIRS)).clamp(0.0, 1.0))
}

/// The rise for a look `redness` of the way to pure red, in thousandths: the
/// steps the cover draws it in.
pub(crate) fn rise_of(redness: f32) -> u16 {
    (rise(redness) * 1000.0).round() as u16
}

/// `t`, from 0 to 1, eased in and out: slow at both ends.
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// How far below its place the maw is at `rise` thousandths, in the grid's
/// units.
fn lowered(rise: u16) -> f32 {
    DROP * (1.0 - f32::from(rise) / 1000.0)
}

/// How far below its place the maw is at `rise` thousandths, in the pixels of
/// a cover `size` pixels square.
fn sunk(size: usize, rise: u16) -> f32 {
    lowered(rise) * size as f32 / GRID
}

/// How far below its place the maw is drawn at `rise` thousandths, in the
/// pixels of a cover `size` pixels square. None at 0, where the maw is under
/// the sand: it is not drawn at all, and neither is the gap, whose only job
/// is to hide it.
fn drawn_down(size: usize, rise: u16) -> Option<f32> {
    (rise > 0).then(|| sunk(size, rise))
}

/// The side of the cover's images, in pixels, for a cover `width` points
/// wide on a display of `scale` pixels to the point.
fn pixel_size(width: f32, scale: f32) -> usize {
    (width * scale).round() as usize
}

/// The cover in `rect` for a look `redness` of the way to pure red, seen
/// through `tint`. Up to six quads, back to front: the back, the maw in a
/// rect moved down by what it is still sunk, the rim's light, the shade at
/// the sand, the gap and the front. The maw, the rim's light, the shade and
/// the gap are drawn together or not at all, the rim's light is composed
/// again only when the rise is new, and the shade only when the new rise
/// moves its picture. The tint is their
/// vertex color, and the blend happens in gamma space (neither painter's
/// textures are sRGB: the Metal painter's are `Unorm`, as egui_glow's are),
/// as `lerp_to_gamma` and `over` do on the CPU.
///
/// One cover a pass: the rim's light is one texture, which holds the light of
/// one rise, and so is the shade, so two covers at two rises drawn in one
/// pass would both show the light and the shade composed last. No pass draws
/// two. The one caller is the now-playing
/// card (`view::now_playing`), once in each pass over `ui::draw`, the demo's
/// shots and films included; the Scale plate draws no cover, and the app
/// icon draws the scenery alone. A debug build asserts it. The README's banner
/// draws none either: [`blended`] blends the same six images on the CPU, in
/// this order, for the banner and for the tests, and is the one function that
/// must follow a change to it.
pub fn draw(ui: &Ui, rect: Rect, redness: f32, scene: &Cover, tint: Tint) {
    let ctx = ui.ctx();
    let size = pixel_size(rect.width(), ctx.pixels_per_point());
    let rise = rise_of(redness);
    let id = Id::new("cover");
    let cached = ctx
        .data(|data| data.get_temp::<Arc<Textures>>(id))
        .filter(|textures| textures.back.size() == [size, size] && textures.scene == *scene);
    let textures = cached.unwrap_or_else(|| {
        let images = prebuilt::images(prebuilt::Key {
            size,
            scene: *scene,
        });
        let textures = Arc::new(Textures::load(ctx, *scene, images, rise));
        ctx.data_mut(|data| data.insert_temp(id, Arc::clone(&textures)));
        textures
    });
    let all = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let white = tint.apply(Color32::WHITE);
    let painter = ui.painter();
    let paint = |painter: &egui::Painter, sprite: &Sprite, down: f32| {
        let at = sprite.place.rect(rect, size, down);
        painter.image(sprite.texture.id(), at, all, white);
    };
    painter.image(textures.back.id(), rect, all, white);
    if let Some(down) = drawn_down(size, rise) {
        // While the maw is sunk its image reaches below the cover, where
        // the sand hides it; the clip keeps the rest off the card.
        paint(&painter.with_clip_rect(rect), &textures.maw, down);
        // The rim's light and the shade are on the cover's own pixels,
        // inside it. The gap over them hides them with the maw, by as much
        // as the sand hides that.
        #[cfg(debug_assertions)]
        one_light_a_pass(ctx, rise);
        paint(painter, &textures.light_for(rise).sprite, 0.0);
        paint(painter, &textures.shade_for(rise).sprite, 0.0);
        paint(painter, &textures.gap, 0.0);
    }
    paint(painter, &textures.front, 0.0);
}

/// Asserts that this pass has drawn no cover's light at a rise other than
/// `rise`: see [`draw`]. Debug builds only.
#[cfg(debug_assertions)]
fn one_light_a_pass(ctx: &egui::Context, rise: u16) {
    let pass = (ctx.viewport_id(), ctx.cumulative_pass_nr());
    let id = Id::new("cover").with("light drawn");
    let last = ctx.data_mut(|data| {
        let last = data.get_temp::<((egui::ViewportId, u64), u16)>(id);
        data.insert_temp(id, (pass, rise));
        last
    });
    if let Some((when, drawn)) = last {
        assert!(
            when != pass || drawn == rise,
            "the cover's light was drawn at rises {drawn} and {rise} in one pass, and it is one texture"
        );
    }
}

/// What the cover has cost since the process started, for the driver's
/// report. Demo builds only.
#[cfg(feature = "demo")]
pub mod counts {
    use std::sync::atomic::{AtomicU64, Ordering};

    use egui::ColorImage;

    static LAYER_BUILDS: AtomicU64 = AtomicU64::new(0);
    static RIM_COMPOSES: AtomicU64 = AtomicU64::new(0);
    static SHADE_COMPOSES: AtomicU64 = AtomicU64::new(0);
    static UPLOADED_BYTES: AtomicU64 = AtomicU64::new(0);
    static PREBUILT_USED: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
    pub struct CoverCounts {
        /// Times the layers were built, and their six images sent to the
        /// GPU, the rim's light and the shade among them, composed for that
        /// frame's rise: once per window and size, and never while the maw
        /// slides, since it only moves its rect.
        pub layer_builds: u64,
        /// Times the rim's light was composed again, after the build: once
        /// for each frame whose maw stands at a rise the light was not
        /// composed for last, and for no other frame. Each sends the rows of
        /// the light's box from the first that holds light at the old rise or
        /// the new to the last, and none when neither holds any.
        pub rim_composes: u64,
        /// Times the shade at the sand was composed again, after the build:
        /// once for each of those frames whose rise moves the shade's
        /// picture from the one it was composed for last (`Shade::key`), and
        /// for no other frame. Each sends the rows of the shade's box as the
        /// rim's light's are sent.
        pub shade_composes: u64,
        /// The bytes all those images held.
        pub uploaded_bytes: u64,
    }

    /// The counts so far.
    pub fn read() -> CoverCounts {
        CoverCounts {
            layer_builds: LAYER_BUILDS.load(Ordering::Relaxed),
            rim_composes: RIM_COMPOSES.load(Ordering::Relaxed),
            shade_composes: SHADE_COMPOSES.load(Ordering::Relaxed),
            uploaded_bytes: UPLOADED_BYTES.load(Ordering::Relaxed),
        }
    }

    /// How many of those builds a window's first frame found done already,
    /// on the thread that [`super::prebuilt`] starts when a window is asked
    /// for. The rest were built by the first frame itself.
    pub fn prebuilt_used() -> u64 {
        PREBUILT_USED.load(Ordering::Relaxed)
    }

    pub(super) fn add_prebuilt_used() {
        PREBUILT_USED.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts the `images` of a build of the layers, sent to the GPU.
    pub(super) fn add(images: [&ColorImage; 6]) {
        LAYER_BUILDS.fetch_add(1, Ordering::Relaxed);
        for image in images {
            add_bytes(image);
        }
    }

    /// Counts the rim's light, composed again for a new rise, and the rows
    /// of it `sent` to the GPU, if any were.
    pub(super) fn add_rim(sent: Option<&ColorImage>) {
        RIM_COMPOSES.fetch_add(1, Ordering::Relaxed);
        if let Some(image) = sent {
            add_bytes(image);
        }
    }

    /// Counts the shade at the sand, composed again for a new rise that
    /// moves it, and the rows of it `sent` to the GPU, if any were.
    pub(super) fn add_shade(sent: Option<&ColorImage>) {
        SHADE_COMPOSES.fetch_add(1, Ordering::Relaxed);
        if let Some(image) = sent {
            add_bytes(image);
        }
    }

    fn add_bytes(image: &ColorImage) {
        let bytes = std::mem::size_of_val(image.pixels.as_slice());
        UPLOADED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

/// The cover's images, built on a thread from the moment a window is asked
/// for, so that the window's first frame only uploads them, with the rim's
/// light and the shade composed for its rise, each in a small box.
///
/// A window takes tens of milliseconds to come up, since eframe makes it and
/// its painter before the first frame, and the images take about 5 ms to
/// build at 2x. [`window_asked_for`] starts the build as soon as the app knows
/// a window is coming, for the main screen's scale: eframe opens every window
/// centered there, wherever the last one was. Where that scale is unknown
/// (off the main thread, or off macOS), it builds for the size the cover last
/// needed. The cover takes the images whenever it has none on the GPU for its
/// size and scene, and builds them itself if they are not there. It never
/// waits for the thread: a build that has not finished, or one for another
/// size (the window moved to an external display at 1x), costs no more than
/// no build at all.
///
/// One slot holds the result, only for as long as a window may take it: the
/// first frame empties it, a closing window empties it, and a build that
/// finishes after either is dropped. Memory at rest is what it was.
pub(crate) mod prebuilt {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    #[cfg(test)]
    use std::sync::mpsc;
    use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, TryLockError};
    use std::thread::JoinHandle;

    use super::{Images, Layers, pixel_size};
    use crate::theme::Cover;
    use crate::ui::view::{COVER_RECT, COVER_SCENE};

    /// The largest scale a first guess believes, in pixels to the point. No
    /// Mac has more than 2, and a wild number would build a huge picture.
    const MAX_SCALE: f32 = 4.0;

    /// What a set of images is for: the cover's side in pixels and its
    /// scene, the two things the GPU's copy is keyed on.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) struct Key {
        pub(crate) size: usize,
        pub(crate) scene: Cover,
    }

    /// Finished images, and the build they came from.
    struct Ready {
        build: u64,
        key: Key,
        images: Images,
    }

    struct Shared {
        /// The build the slot waits for. Starting a build, a take and a close
        /// each move it on, so a build that finishes under an older number is
        /// not wanted.
        build: AtomicU64,
        /// The side the cover last needed images for, at its last take: a
        /// window's first frame, or a frame at a new scale. 0 before any.
        last_size: AtomicUsize,
        /// The finished images. A build holds this only to put them in or
        /// clear them, never while it builds.
        ready: Mutex<Option<Ready>>,
        /// What a build runs while it holds `ready`, once: how a test stops
        /// it there.
        #[cfg(test)]
        in_lock: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    /// The slot between the thread that builds the images and the first
    /// frame that uploads them. A handle: the thread holds one too.
    #[derive(Clone)]
    pub(crate) struct Slot(Arc<Shared>);

    impl Slot {
        pub(crate) fn new() -> Self {
            Self(Arc::new(Shared {
                build: AtomicU64::new(0),
                last_size: AtomicUsize::new(0),
                ready: Mutex::new(None),
                #[cfg(test)]
                in_lock: Mutex::new(None),
            }))
        }

        /// A window is about to open: starts the build for a cover at
        /// `scale`, the pixels to the point of the display the window opens
        /// on, if the app can tell without a window. If it cannot, or the
        /// scale is not one a display has, the build is for the size the
        /// cover last needed, and there is none before the first.
        pub(crate) fn ask(&self, scale: impl FnOnce() -> Option<f32>) -> Option<JoinHandle<()>> {
            let size = scale()
                .filter(|scale| scale.is_finite() && 0.0 < *scale && *scale <= MAX_SCALE)
                .map(|scale| pixel_size(COVER_RECT.width(), scale))
                .or_else(|| self.last_size())?;
            let key = Key {
                size,
                scene: COVER_SCENE,
            };
            self.start_with(key, |key| Layers::new(key.size, key.scene).images())
        }

        /// Builds the images for `key` with `build` on a thread of its own, and
        /// puts them in the slot unless the slot has moved on by then. Drops
        /// whatever the slot held. None if the thread could not start.
        pub(crate) fn start_with(
            &self,
            key: Key,
            build: impl FnOnce(Key) -> Images + Send + 'static,
        ) -> Option<JoinHandle<()>> {
            let build_number = self.cancel();
            let slot = self.clone();
            let thread = std::thread::Builder::new()
                .name("arrakis-cover".to_owned())
                .spawn(move || slot.finish(build_number, key, build(key)));
            match thread {
                Ok(thread) => Some(thread),
                Err(error) => {
                    log::warn!("unable to start the cover's thread: {error}");
                    None
                }
            }
        }

        /// The cover has no images on the GPU for `key`, at a window's first
        /// frame or at any frame after it whose size or scene is new:
        /// remembers the size for a build whose scale is unknown, and takes
        /// the images if the build is done and was for `key`. Either way a
        /// build still under way is no longer wanted, and the slot is empty
        /// once no build holds its lock. Never waits: a lock a build holds
        /// is a miss, and that build empties the slot before it lets go.
        pub(crate) fn take(&self, key: Key) -> Option<Images> {
            self.0.last_size.store(key.size, Ordering::SeqCst);
            self.0.build.fetch_add(1, Ordering::SeqCst);
            let ready = self.try_ready()?.take()?;
            (ready.key == key).then_some(ready.images)
        }

        /// The window is gone: empties the slot, and drops a build that is
        /// still under way when it finishes.
        pub(crate) fn close(&self) {
            self.cancel();
        }

        /// The side the cover last needed images for, in pixels, at its last
        /// [`Slot::take`].
        pub(crate) fn last_size(&self) -> Option<usize> {
            Some(self.0.last_size.load(Ordering::SeqCst)).filter(|size| *size > 0)
        }

        /// Moves the build number on, so that no build under way is wanted,
        /// and empties the slot unless a build holds its lock: that build
        /// empties it before it lets go. Returns the new number.
        fn cancel(&self) -> u64 {
            let build = self.0.build.fetch_add(1, Ordering::SeqCst) + 1;
            // Dropped after the lock is let go.
            let _old = self.try_ready().and_then(|mut ready| ready.take());
            build
        }

        /// The thread's last step: puts `images` in the slot if `build` is
        /// still the one wanted.
        ///
        /// A `take` or a `cancel` that finds the lock held moves the number
        /// on but cannot empty the slot. So whoever holds the lock does it
        /// for them: it drops whatever images are not the current build's,
        /// its own or a newer build's, and does it again after letting go if
        /// the number moved on meanwhile, until it did not.
        fn finish(&self, build: u64, key: Key, images: Images) {
            let mut images = Some(images);
            loop {
                let mut ready = self.lock_ready();
                let current = self.0.build.load(Ordering::SeqCst);
                // The number only grows, so it is `build` on the first turn
                // at most, while the images are still here.
                let stale = if current == build {
                    images
                        .take()
                        .and_then(|images| ready.replace(Ready { build, key, images }))
                } else if ready.as_ref().is_some_and(|ready| ready.build != current) {
                    ready.take()
                } else {
                    None
                };
                #[cfg(test)]
                self.run_in_lock();
                drop(ready);
                // Dropped after the lock is let go.
                drop(stale);
                if self.0.build.load(Ordering::SeqCst) == current {
                    return;
                }
            }
        }

        /// The slot, for the thread, which may wait for the instant another
        /// holds it.
        fn lock_ready(&self) -> MutexGuard<'_, Option<Ready>> {
            self.0.ready.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// The slot, for the window's thread, which never waits: None while
        /// the build thread holds it.
        fn try_ready(&self) -> Option<MutexGuard<'_, Option<Ready>>> {
            match self.0.ready.try_lock() {
                Ok(ready) => Some(ready),
                Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            }
        }
    }

    /// Test support: what the tests look at in the slot, and how they make it
    /// busy.
    #[cfg(test)]
    impl Slot {
        /// Whether no images wait in the slot.
        pub(crate) fn is_empty(&self) -> bool {
            self.lock_ready().is_none()
        }

        /// The slot's lock, held: how a test makes `take`, `close` or a
        /// build's last step find it busy.
        pub(crate) fn hold(&self) -> impl Sized + '_ {
            self.lock_ready()
        }

        /// Whether the slot's lock is held.
        pub(crate) fn is_busy(&self) -> bool {
            matches!(self.0.ready.try_lock(), Err(TryLockError::WouldBlock))
        }

        /// Stops the next build that holds the slot's lock right there, once
        /// it has put its images in or cleared the slot: the receiver hears
        /// when it got there, and the build goes on at the sender's word, or
        /// once the sender is dropped.
        pub(crate) fn stop_in_lock(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (stopped, there) = mpsc::channel();
            let (go_on, wait) = mpsc::channel::<()>();
            let stop = Box::new(move || {
                let _ = stopped.send(());
                let _ = wait.recv();
            });
            *self
                .0
                .in_lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(stop);
            (there, go_on)
        }

        /// Runs what [`Slot::stop_in_lock`] left, once.
        fn run_in_lock(&self) {
            let stop = self
                .0
                .in_lock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(stop) = stop {
                stop();
            }
        }
    }

    /// The images for `key`: the ones `slot` holds if they are done, else
    /// built here, as a cover with no thread would.
    pub(crate) fn images_from(slot: &Slot, key: Key) -> Images {
        match slot.take(key) {
            Some(images) => {
                #[cfg(feature = "demo")]
                super::counts::add_prebuilt_used();
                images
            }
            None => Layers::new(key.size, key.scene).images(),
        }
    }

    /// The process's slot: there is one window at a time.
    static SLOT: LazyLock<Slot> = LazyLock::new(Slot::new);

    /// The images a window's first frame draws, from the process's slot.
    pub(super) fn images(key: Key) -> Images {
        images_from(&SLOT, key)
    }

    /// A window is about to open, on the process's slot: see [`Slot::ask`].
    pub(crate) fn window_asked_for(
        first_scale: impl FnOnce() -> Option<f32>,
    ) -> Option<JoinHandle<()>> {
        SLOT.ask(first_scale)
    }

    /// The window is gone, on the process's slot: see [`Slot::close`].
    pub(crate) fn window_gone() {
        SLOT.close();
    }
}

/// The cover on the GPU, for one scene and size: the size is the back's.
struct Textures {
    scene: Cover,
    back: TextureHandle,
    maw: Sprite,
    gap: Sprite,
    front: Sprite,
    /// The rim's light, as it was last composed, for a rise. A lock, since
    /// the cover is shared and a texture is written through its own handle.
    rim: Mutex<Composed<u16>>,
    /// What the rim's light is composed from.
    light: RimLight,
    /// The shade at the sand, as it was last composed, for what of a rise
    /// its picture depends on ([`Shade::key`]), behind a lock as the rim's
    /// light is.
    shade: Mutex<Composed<ShadeKey>>,
    /// What the shade is composed from.
    teeth: Shade,
}

/// A cropped layer on the GPU, and where it goes in the cover.
struct Sprite {
    texture: TextureHandle,
    place: Place,
}

/// A texture the CPU composes again for a new rise, on the GPU, and what it
/// holds: the rim's light, or the shade at the sand. `K` is what of the rise
/// its picture depends on: the rise itself for the rim's light, and
/// [`Shade::key`] for the shade, whose picture stays the same over the rises
/// that slide the teeth by the same rows.
struct Composed<K> {
    /// The image, in its box.
    sprite: Sprite,
    /// What it was last composed for.
    key: K,
    /// The rows of its box that are not clear in that picture, from the
    /// first to the last: none at rise 0, nor where the sun is behind none of
    /// the rim or no tooth stands near the sand.
    rows: Range<usize>,
    /// Counts each compose after the first and what it sends. Demo builds
    /// only.
    #[cfg(feature = "demo")]
    count: fn(Option<&ColorImage>),
}

impl<K: PartialEq> Composed<K> {
    /// Composes the image again for `key` with `compose`, unless it was last
    /// composed for that key, and sends the rows of its box from the first
    /// that is not clear in the old picture or the new to the last, changed
    /// or not, as one partial set: every other row is clear in both. A
    /// compose where both pictures are clear sends nothing.
    fn hold(&mut self, key: K, compose: impl FnOnce() -> ColorImage) {
        if self.key == key {
            return;
        }
        let image = compose();
        let rows = lit_rows(&image);
        let changed = spanning(self.rows.clone(), rows.clone());
        let sent = (!changed.is_empty()).then(|| rows_of(&image, changed.clone()));
        #[cfg(feature = "demo")]
        (self.count)(sent.as_ref());
        if let Some(sent) = sent {
            (self.sprite.texture).set_partial([0, changed.start], sent, FILTER);
        }
        (self.key, self.rows) = (key, rows);
    }
}

/// `held`, composed for `key` with `compose` if it is not already: see
/// [`Composed::hold`].
fn composed_for<K: PartialEq>(
    held: &Mutex<Composed<K>>,
    key: K,
    compose: impl FnOnce() -> ColorImage,
) -> MutexGuard<'_, Composed<K>> {
    let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
    held.hold(key, compose);
    held
}

impl Textures {
    /// Sends `images` to the GPU, with the rim's light and the shade
    /// composed for `rise`.
    fn load(ctx: &egui::Context, scene: Cover, images: Images, rise: u16) -> Self {
        let rim = Crop {
            image: images.rim.compose(rise),
            place: images.rim.place,
        };
        let shade = Crop {
            image: images.shade.compose(rise),
            place: images.shade.place,
        };
        #[cfg(feature = "demo")]
        counts::add([
            &images.back,
            &images.maw.image,
            &images.gap.image,
            &images.front.image,
            &rim.image,
            &shade.image,
        ]);
        let load = |name: &str, image: ColorImage| ctx.load_texture(name, image, FILTER);
        let sprite = |name: &str, crop: Crop| Sprite {
            texture: load(name, crop.image),
            place: crop.place,
        };
        // The textures load in the order the fields are written: the shade
        // last.
        Self {
            scene,
            back: load("cover-back", images.back),
            maw: sprite("cover-maw", images.maw),
            gap: sprite("cover-gap", images.gap),
            front: sprite("cover-front", images.front),
            rim: Mutex::new(Composed {
                rows: lit_rows(&rim.image),
                sprite: sprite("cover-rim", rim),
                key: rise,
                #[cfg(feature = "demo")]
                count: counts::add_rim,
            }),
            light: images.rim,
            shade: Mutex::new(Composed {
                rows: lit_rows(&shade.image),
                sprite: sprite("cover-shade", shade),
                key: images.shade.key(rise),
                #[cfg(feature = "demo")]
                count: counts::add_shade,
            }),
            teeth: images.shade,
        }
    }

    /// The rim's light for `rise`, composed for it unless it holds that rise
    /// already: see [`Composed::hold`]. A move between two rises that light
    /// no row sends nothing.
    fn light_for(&self, rise: u16) -> MutexGuard<'_, Composed<u16>> {
        composed_for(&self.rim, rise, || self.light.compose(rise))
    }

    /// The shade at the sand for `rise`, composed for it unless it holds the
    /// same picture already: unless the rise it was last composed for slides
    /// the teeth by the same rows ([`Shade::key`]). A move between two rises
    /// where no tooth sinks sends nothing.
    fn shade_for(&self, rise: u16) -> MutexGuard<'_, Composed<ShadeKey>> {
        composed_for(&self.shade, self.teeth.key(rise), || {
            self.teeth.compose(rise)
        })
    }
}

/// The rows of `image` that are not clear, from the first to the last: none
/// when it is clear all over.
fn lit_rows(image: &ColorImage) -> Range<usize> {
    let lit = |row: &[Color32]| row.iter().any(|pixel| *pixel != Color32::TRANSPARENT);
    let rows = || image.pixels.chunks(image.size[0]);
    match (rows().position(lit), rows().rposition(lit)) {
        (Some(first), Some(last)) => first..last + 1,
        _ => 0..0,
    }
}

/// The span from the first row of `a` and `b` to the last: the rows of
/// both, and those between. An empty span adds no rows.
fn spanning(a: Range<usize>, b: Range<usize>) -> Range<usize> {
    if a.is_empty() {
        b
    } else if b.is_empty() {
        a
    } else {
        a.start.min(b.start)..a.end.max(b.end)
    }
}

/// The `rows` of `image`, as an image of their own, as wide.
fn rows_of(image: &ColorImage, rows: Range<usize>) -> ColorImage {
    let width = image.size[0];
    let pixels = image.pixels[rows.start * width..rows.end * width].to_vec();
    ColorImage::new([width, rows.len()], pixels)
}

/// What the cover sends to the GPU: the back, an image `size` pixels square,
/// three layers cropped to their boxes, and what the rim's light and the
/// shade at the sand are composed from.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Images {
    /// The sky with the sun in it, and the far dune.
    pub(crate) back: ColorImage,
    /// The maw all the way out, cropped to the pixels it fills and a clear
    /// margin.
    pub(crate) maw: Crop,
    /// The back multiplied by the keep-out's alpha: the part of the back
    /// that the sand hides the maw with. Laid over the maw over the back, it
    /// gives `compose`'s blend. Cropped to the pixels it fills.
    pub(crate) gap: Crop,
    /// The near dune, the ground and the mound, cropped to the pixels they
    /// fill.
    pub(crate) front: Crop,
    /// The rim's layer and the sun behind it, in the box where some rise
    /// lights the rim.
    pub(crate) rim: RimLight,
    /// The teeth's layer and the band above the sand, in the box where some
    /// rise sinks a tooth into the throat's dark.
    pub(crate) shade: Shade,
}

/// The light the lip's rim catches, as far as the sun stands behind it: one
/// of the two parts of the cover that no rect can move (the other is the
/// [`Shade`]), since the light slides with the maw while the sun that lights
/// it stands still. The CPU composes it at each new rise, in the box of the
/// cover where some rise lights the rim, and the GPU draws it between the maw
/// and the shade.
///
/// The light has no alpha: it is added to what is under it. The gap over it
/// multiplies it by how far the sand lets the maw show, as `compose`
/// multiplies it, so the light is composed without that.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct RimLight {
    /// The side of the cover it is for, in pixels.
    size: usize,
    /// The box: the pixels that the rim's layer slides over, at some rise
    /// from the first thousandth to all the way out, and that the sun stands
    /// behind.
    place: Place,
    /// The first row of the cover that `layer` holds: the box's top, less the
    /// rows a pixel finds the layer up to, at the first thousandth.
    from: usize,
    /// The rim's layer in the box's columns, from the row `from` down to the
    /// box's last row: all of it that slides into the box.
    layer: Vec<Color32>,
    /// Where `layer` has light, about one pixel in twenty of it: the only
    /// ones a rise needs to look under.
    lit: Vec<usize>,
    /// How far the sun stands behind each pixel of the box.
    sunlit: Vec<f32>,
}

impl RimLight {
    /// The rim's light for a cover `size` pixels square, from the rim's layer
    /// `rim` and the sun's `sunlit`, both of the cover's size.
    fn new(size: usize, rim: &[Color32], sunlit: &[f32]) -> Self {
        let slid = SlidBox::new(size, rim, |i| sunlit[i] > 0.0, |_| ());
        let layer: Vec<Color32> = slid.window(size, slid.from, |i| rim[i]);
        let lit = (0..layer.len())
            .filter(|&i| layer[i] != Color32::TRANSPARENT)
            .collect();
        Self {
            size,
            place: slid.place(),
            from: slid.from,
            layer,
            lit,
            sunlit: slid.window(size, slid.first[1], |i| sunlit[i]),
        }
    }

    /// The rim's light in its box, with the maw out by `rise` thousandths:
    /// the rim's layer slid as [`Slide`] slides the maw's layers, as far as
    /// the sun stands behind each pixel. Clear at 0, where the maw is under
    /// the sand.
    ///
    /// A pixel of the box slides between two of the layer's, the slide's
    /// rows above it and one more, and is clear where both are. So only the
    /// pixels that the layer's light slides to are composed, at most two for
    /// each lit pixel of the layer, and the rest are left clear: the same
    /// picture as composing every pixel of the box.
    pub(crate) fn compose(&self, rise: u16) -> ColorImage {
        let slide = Slide::new(self.size, rise);
        let [width, height] = self.place.extent;
        let top = self.place.at[1] as usize;
        let mut pixels = vec![Color32::TRANSPARENT; width * height];
        for &at in &self.lit {
            let (column, row) = (at % width, self.from + at / width);
            for y in [row + slide.rows, row + slide.rows + 1] {
                let Some(i) = (y.checked_sub(top))
                    .filter(|&y| y < height)
                    .map(|y| y * width + column)
                else {
                    continue;
                };
                let sunlit = self.sunlit[i];
                if sunlit > 0.0 {
                    // The layer is clear above the rows it holds.
                    let light = slide.above(y, |row| {
                        row.checked_sub(self.from)
                            .map_or(Color32::TRANSPARENT, |row| self.layer[row * width + column])
                    });
                    pixels[i] = slide.blend(light).gamma_multiply(sunlit);
                }
            }
        }
        ColorImage::new(self.place.extent, pixels)
    }
}

/// The shade at the sand: within [`SHADE`] grid units above the sand's
/// keep-out line the teeth sink into the throat's dark, on a smoothstep, the
/// deeper the nearer the line. Through the sand's gap the outer teeth meet
/// the low sky and the far dune, and no flat color of theirs stands apart
/// from both; sunk in the dark they do. The teeth slide with the maw while
/// the line stands still, so no rect can draw it: the CPU composes it at each
/// new rise that moves its picture ([`Shade::key`]), in the box of the cover
/// where some rise sinks a tooth, and the GPU draws it over the maw and the
/// rim's light, under the gap.
///
/// It is the throat's color, as deep as the band is at each pixel, wherever
/// the pixel slides between two pixels of the teeth's layer and either holds
/// any of a tooth. Laid over the maw there, it takes each sample of a tooth
/// toward the throat by the band, and leaves the throat as it is: the same
/// as laying the throat over the teeth alone, which no pixel of the maw keeps
/// apart. The gap over it multiplies it by how far the sand lets the maw
/// show, as it does the rim's light, so it is composed without that, and
/// where the sand hides the maw whole there is nothing to shade.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Shade {
    /// The side of the cover it is for, in pixels.
    size: usize,
    /// The box: the pixels where the band takes some of a tooth into the
    /// dark that the teeth's layer slides over, at some rise from the first
    /// thousandth to all the way out.
    place: Place,
    /// Which pixels of the teeth's layer hold any of a tooth, in the box's
    /// columns, from the box's top less the rows a pixel finds the layer up
    /// to at the first thousandth, down to the box's last row: all of it that
    /// slides into the box.
    teeth: Vec<bool>,
    /// The pixels of the box that some rise darkens, in the box's order: the
    /// only ones a compose looks at.
    dark: Vec<Dark>,
}

/// A pixel of the shade's box that some rise darkens.
#[derive(Clone, Copy)]
struct Dark {
    /// Where it is in the box.
    at: usize,
    /// Where the pixel of the teeth's layer in its own row and column is in
    /// the shade's `teeth`: the one that slides to it all the way out.
    under: usize,
    /// The throat's color, as deep as the band is here.
    shade: Color32,
}

impl Shade {
    /// The shade for a cover `size` pixels square, from the teeth's layer
    /// `teeth`, the `band` above the sand and the `throat`'s color, the first
    /// two of the cover's size.
    fn new(size: usize, teeth: &[Color32], band: &[f32], throat: Color32) -> Self {
        // The shade at a pixel of the cover, where a tooth slides to it. A
        // band too faint to change the throat's color by a step is no shade.
        let shade = |i: usize| throat.gamma_multiply(band[i]);
        let mut darkened = Vec::new();
        let slid = SlidBox::new(
            size,
            teeth,
            |i| shade(i) != Color32::TRANSPARENT,
            |pixel| darkened.push(pixel),
        );
        let place = slid.place();
        let width = place.extent[0];
        let [left, top] = slid.first;
        let mut dark: Vec<Dark> = (darkened.into_iter())
            .map(|[x, y]| {
                let column = x - left;
                Dark {
                    at: (y - top) * width + column,
                    under: (y - slid.from) * width + column,
                    shade: shade(y * size + x),
                }
            })
            .collect();
        dark.sort_unstable_by_key(|dark| dark.at);
        Self {
            size,
            place,
            teeth: slid.window(size, slid.from, |i| teeth[i] != Color32::TRANSPARENT),
            dark,
        }
    }

    /// The shade in its box, with the maw out by `rise` thousandths: the
    /// teeth's layer slid as [`Slide`] slides the maw's layers, and the
    /// throat's color as deep as the band is wherever a tooth slides to.
    /// Clear at 0, where the maw is under the sand.
    ///
    /// Only the pixels of the box that some rise darkens are looked at, and
    /// for each only the two pixels of the layer it slides between, the
    /// slide's rows above it and one more: the same picture as looking at
    /// every pixel of the box.
    ///
    /// It reads `rise` only for whether it is above 0, the slide's whole
    /// rows and whether the slide blends in a share of one more
    /// ([`Slide::shows`]): its [`Shade::key`], which changes with it.
    pub(crate) fn compose(&self, rise: u16) -> ColorImage {
        let slide = Slide::new(self.size, rise);
        let [width, height] = self.place.extent;
        let mut pixels = vec![Color32::TRANSPARENT; width * height];
        if rise > 0 {
            // How far back in `teeth` the two pixels lie, from the one in a
            // pixel's own row; none of them above the layer's first row.
            let back = [slide.rows, slide.rows + 1].map(|rows| rows * width);
            let holds = |under: usize, back: usize| {
                (under.checked_sub(back)).is_some_and(|i| self.teeth[i])
            };
            for dark in &self.dark {
                if slide.shows(back.map(|back| holds(dark.under, back))) {
                    pixels[dark.at] = dark.shade;
                }
            }
        }
        ColorImage::new(self.place.extent, pixels)
    }

    /// What the shade's picture at `rise` depends on, and all it depends on
    /// ([`Shade::compose`]): none at 0, where the maw is under the sand and
    /// the shade is clear; else the whole rows the slide moves the teeth's
    /// layer by, and whether it blends in a share of one more. Two rises with
    /// the same key compose the same picture, so the cover composes the
    /// shade again only for a new key. At 272 pixels the maw moves about 0.12
    /// pixels a thousandth, so a key holds for about eight thousandths.
    fn key(&self, rise: u16) -> ShadeKey {
        let slide = Slide::new(self.size, rise);
        (rise > 0).then_some(ShadeSlide {
            rows: slide.rows,
            blends: slide.part > 0.0,
        })
    }
}

/// What the shade's picture depends on, of a rise: see [`Shade::key`].
type ShadeKey = Option<ShadeSlide>;

/// How a rise above 0 slides the teeth's layer, as far as the shade can
/// tell: the slide's whole rows, and whether it blends in a share of one
/// more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShadeSlide {
    rows: usize,
    blends: bool,
}

/// A layer of the cover cropped to a box, and where the box goes.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Crop {
    /// The box's pixels: `place.extent` in size.
    pub(crate) image: ColorImage,
    place: Place,
}

/// Where a cropped image goes in the cover. A cover's size is not part of
/// it: the box is in the pixels of the cover it was cropped from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    /// The image's top left in the cover, in pixels. Negative where the
    /// maw's margin reaches beyond the cover.
    at: [i32; 2],
    /// The image's size, in pixels.
    extent: [usize; 2],
}

impl Place {
    /// The image's top left from the cover's, in pixels, when it is drawn
    /// `down` pixels below its place. Only the maw moves, and only down:
    /// everything else is on whole pixels, so that nothing is resampled
    /// sideways.
    fn offset(&self, down: f32) -> Vec2 {
        vec2(self.at[0] as f32, self.at[1] as f32 + down)
    }

    /// The image's rect in the `cover` rect, a cover `size` pixels square,
    /// one texel to a pixel of the cover's own image, `down` pixels below its
    /// place.
    fn rect(&self, cover: Rect, size: usize, down: f32) -> Rect {
        let pixel = cover.width() / size as f32;
        let extent = vec2(self.extent[0] as f32, self.extent[1] as f32);
        Rect::from_min_size(cover.min + self.offset(down) * pixel, extent * pixel)
    }
}

/// The box of a part of the cover that slides with the maw and that the CPU
/// composes again as the rise changes, the rim's light or the shade, in a
/// cover of some size: the pixels that a layer of the maw slides over at
/// some rise, from the first thousandth to all the way out, and that the
/// part keeps.
struct SlidBox {
    /// The box's top left and bottom right pixels, as [x, y], both inside
    /// it: the one pixel at [0, 0] when the part keeps none.
    first: [usize; 2],
    last: [usize; 2],
    /// The first row of the cover that the layer's window holds: the box's
    /// top, less the rows a pixel finds the layer up to, at the first
    /// thousandth.
    from: usize,
}

impl SlidBox {
    /// The box for a cover `size` pixels square, of the pixels that some
    /// pixel of `layer` that is not clear slides over and that `keeps` takes,
    /// both by the index of a pixel in the cover. `each` is given every pixel
    /// the box holds that way, as [x, y], column by column.
    fn new(
        size: usize,
        layer: &[Color32],
        keeps: impl Fn(usize) -> bool,
        mut each: impl FnMut([usize; 2]),
    ) -> Self {
        // A pixel finds the layer this many rows above itself at most: at
        // the first thousandth the maw is sunk the most, and the slide
        // blends two rows.
        let reach = Slide::new(size, 1).rows + 1;
        let keeps = &keeps;
        let slid = (0..size).flat_map(|x| {
            // The lowest row so far that holds some of the layer, in this
            // column.
            let mut lowest = None;
            (0..size)
                .filter(move |&y| {
                    let i = y * size + x;
                    if layer[i] != Color32::TRANSPARENT {
                        lowest = Some(y);
                    }
                    lowest.is_some_and(|row| y - row <= reach) && keeps(i)
                })
                .map(move |y| [x, y])
        });
        let [first, last] = corners(slid.inspect(|&pixel| each(pixel)));
        Self {
            first,
            last,
            from: first[1].saturating_sub(reach),
        }
    }

    /// Where the box goes in the cover.
    fn place(&self) -> Place {
        let [first, last] = [self.first, self.last];
        Place {
            at: first.map(|n| n as i32),
            extent: [0, 1].map(|axis| last[axis] + 1 - first[axis]),
        }
    }

    /// `pick` of each pixel of a cover `size` pixels square in the box's
    /// columns, by its index in the cover, row by row from the row `top` of
    /// the cover down to the box's last.
    fn window<T>(&self, size: usize, top: usize, pick: impl Fn(usize) -> T) -> Vec<T> {
        let columns = self.first[0]..=self.last[0];
        (top..=self.last[1])
            .flat_map(|y| columns.clone().map(move |x| y * size + x))
            .map(pick)
            .collect()
    }
}

/// The first and the last pixel of the box that holds every one of `points`,
/// as [x, y]: its top left and its bottom right, both inside it. With no
/// points, the box is the one pixel at [0, 0].
fn corners(points: impl IntoIterator<Item = [usize; 2]>) -> [[usize; 2]; 2] {
    let (mut first, mut last) = ([usize::MAX; 2], [0; 2]);
    for [x, y] in points {
        first = [first[0].min(x), first[1].min(y)];
        last = [last[0].max(x), last[1].max(y)];
    }
    if first[0] > last[0] {
        [[0, 0], [0, 0]]
    } else {
        [first, last]
    }
}

/// `pixels`, a layer `size` pixels square, cropped to the box of the pixels
/// that are not clear, and `margin` clear pixels around it, whether or not
/// they are in the cover. A layer with nothing in it is a single clear pixel
/// and its margin.
fn crop(pixels: &[Color32], size: usize, margin: usize) -> Crop {
    let [first, last] = corners(
        (pixels.iter().enumerate())
            .filter(|(_, pixel)| **pixel != Color32::TRANSPARENT)
            .map(|(i, _)| [i % size, i / size]),
    );
    let at = first.map(|n| n as i32 - margin as i32);
    let extent = [0, 1].map(|axis| last[axis] + 1 - first[axis] + 2 * margin);
    let image = (0..extent[0] * extent[1])
        .map(|i| {
            let x = at[0] + (i % extent[0]) as i32;
            let y = at[1] + (i / extent[0]) as i32;
            let inside = (0..size as i32).contains(&x) && (0..size as i32).contains(&y);
            if inside {
                pixels[y as usize * size + x as usize]
            } else {
                Color32::TRANSPARENT
            }
        })
        .collect();
    Crop {
        image: ColorImage::new(extent, image),
        place: Place { at, extent },
    }
}

/// The cover's parts, `size` pixels square, premultiplied, back to front.
#[cfg_attr(test, derive(Clone))]
pub(crate) struct Layers {
    size: usize,
    /// The sky with the sun in it, and the far dune.
    back: Vec<Color32>,
    /// The maw all the way out: the lip, the throat and the teeth in it, and
    /// no body.
    worm: Vec<Color32>,
    /// The light the lip's rim catches with the maw all the way out, kept
    /// apart from it: it is added only where the sun is behind the rim.
    rim: Vec<Color32>,
    /// How far the sun stands behind each pixel, from 0 to 1, for the rim.
    sunlit: Vec<f32>,
    /// How far each pixel keeps the maw out: the sand in front, and a gap
    /// above it.
    keep_out: Vec<f32>,
    /// The near dune, the ground and the mound.
    front: Vec<Color32>,
    /// How much of each pixel the teeth fill with the maw all the way out,
    /// as white: the shade looks only at which pixels hold any.
    teeth: Vec<Color32>,
    /// How far the teeth sink into the throat's dark at each pixel, from 0
    /// to 1: the band above the sand's keep-out line (see [`shade_band`]).
    band: Vec<f32>,
    /// The throat's color, which the teeth sink into.
    throat: Color32,
}

/// How far the teeth sink into the throat's dark at each pixel of a picture
/// `size` pixels square, from `keep_out`, how far each pixel keeps the maw
/// out: all the way from the sand's keep-out line down, where a column's
/// pixels first keep out half of the maw or more, and less and less on a
/// smoothstep up to [`SHADE`] grid units above it, where the teeth keep their
/// own colors. Where the sand keeps the maw out whole there is nothing to
/// sink, and it is 0.
fn shade_band(keep_out: &[f32], size: usize) -> Vec<f32> {
    let pixel = GRID / size as f32;
    // The first row of each column where the keep-out begins: the ground's
    // rows keep it all out, so every column has one.
    let cut: Vec<usize> = (0..size)
        .map(|x| {
            (0..size)
                .find(|&y| keep_out[y * size + x] >= 0.5)
                .unwrap_or(size)
        })
        .collect();
    (0..size * size)
        .map(|i| {
            if keep_out[i] >= 1.0 {
                return 0.0;
            }
            let (x, y) = (i % size, i / size);
            let above = (cut[x] as f32 - y as f32) * pixel;
            smoothstep((1.0 - above / SHADE).clamp(0.0, 1.0))
        })
        .collect()
}

/// Whether the sand in front of the worm reaches `q`, or comes within
/// `margin` of it from below.
fn in_front(q: Pos2, margin: f32) -> bool {
    let (height, half) = MOUND;
    let across = ((q.x - HEAD.x) / half).clamp(-1.0, 1.0);
    let mound = height * (1.0 - across * across).powi(2);
    q.y >= GROUND - mound - margin || DUNES[1].covers(q, margin)
}

/// The center of pixel `i` of a picture `size` pixels square, as a point of
/// the grid.
fn center(i: usize, size: usize) -> Pos2 {
    pos2((i % size) as f32 + 0.5, (i / size) as f32 + 0.5) * (GRID / size as f32)
}

/// The cover's sky at height `y` of the grid, before the sun and the dunes:
/// `sky_top` at the top, running down to `sky_low` at the ground.
pub(crate) fn sky(scene: &Cover, y: f32) -> Color32 {
    scene
        .sky_top
        .lerp_to_gamma(scene.sky_low, (y / GROUND).min(1.0))
}

/// How much of the pixel at `q`, `scale` grid units across, a disc of
/// `radius` around `center` covers.
fn disc(q: Pos2, center: Pos2, radius: f32, scale: f32) -> f32 {
    ((radius - q.distance(center)) / scale + 0.5).clamp(0.0, 1.0)
}

/// The far dune with its slip face, `size` pixels square: the layer the sky
/// and the sun show behind. The cover and `scenery` draw it alike.
fn far_dune(size: usize, scene: &Cover) -> Vec<Color32> {
    let far = &DUNES[0];
    layer(size, |q| {
        far.covers(q, 0.0)
            .then(|| if far.in_lee(q) { scene.lee } else { scene.far })
    })
}

/// The sky at the pixel center `q`, `scale` grid units across, with the sun
/// centered at `sun_at` on it: one blend per pixel center. [`disc`] already
/// antialiases the sun's edge over one pixel, and the pixel is the sky and
/// `scene.sun` mixed by that much. The sun is not sampled through [`layer`] as
/// well, which would smooth its edge a second time, over two pixels. The cover
/// and `scenery` draw it alike.
fn sky_with_sun(scene: &Cover, q: Pos2, sun_at: Pos2, scale: f32) -> Color32 {
    sky(scene, q.y).lerp_to_gamma(scene.sun, disc(q, sun_at, SUN, scale))
}

/// The cover's sun and dunes with no worm and no mound, `size` pixels square:
/// the sky, the sun (the cover's own radius, `drop` grid units lower than the
/// cover puts it, since with the maw gone nothing hides the sky under it), the
/// far dune with its slip face and the near dune and ground. The app's icon
/// (`app_icon`) is this picture.
#[cfg(feature = "demo")]
pub(crate) fn scenery(size: usize, scene: &Cover, drop: f32) -> Vec<Color32> {
    let scale = GRID / size as f32;
    let sun_at = pos2(SUN_CENTER.x, SUN_CENTER.y + drop);
    let back = far_dune(size, scene);
    let front = layer(size, |q| {
        (q.y >= GROUND || DUNES[1].covers(q, 0.0)).then_some(scene.near)
    });
    (0..size * size)
        .map(|i| {
            let air = sky_with_sun(scene, center(i, size), sun_at, scale);
            over(front[i], over(back[i], air))
        })
        .collect()
}

impl Layers {
    pub(crate) fn new(size: usize, scene: Cover) -> Self {
        let scale = GRID / size as f32;
        let far = &DUNES[0];
        // The sun is behind a pixel all the way from `GLARE` inside its edge,
        // and less and less out to the edge. The far dune hides it the same
        // way: not at all at its crest, and all of it from `GLARE` inside the
        // dune, where its own curve says it is, not where its antialiased
        // edge falls.
        let sunlit = (0..size * size)
            .map(|i| {
                let q = center(i, size);
                let sun = (SUN - q.distance(SUN_CENTER)) / GLARE;
                if sun <= 0.0 {
                    return 0.0;
                }
                let hidden = far.depth(q) / GLARE;
                smoothstep(sun.min(1.0)) * (1.0 - smoothstep(hidden.clamp(0.0, 1.0)))
            })
            .collect();
        let back = far_dune(size, &scene);
        let back = (0..size * size)
            .map(|i| {
                let air = sky_with_sun(&scene, center(i, size), SUN_CENTER, scale);
                over(back[i], air)
            })
            .collect();
        let [worm, rim, teeth] = maw_parts(size, &scene);
        let keep_out: Vec<f32> = layer(size, |q| in_front(q, GAP).then_some(Color32::WHITE))
            .into_iter()
            .map(|pixel| f32::from(pixel.a()) / 255.0)
            .collect();
        let front = layer(size, |q| in_front(q, 0.0).then_some(scene.near));
        Self {
            size,
            back,
            worm,
            rim,
            sunlit,
            band: shade_band(&keep_out, size),
            keep_out,
            front,
            teeth,
            throat: scene.throat,
        }
    }

    /// The images the GPU holds, which stay the same for every rise, and what
    /// the rim's light and the shade are composed from at each. The back is
    /// the layer itself, moved; the others are cropped from theirs.
    ///
    /// The maw has a clear margin, since it slides by a fraction of a pixel
    /// and its edge must not be sampled against the clamped edge of its
    /// image. The gap, the front, the rim's light and the shade are drawn one
    /// texel to a device pixel on whole pixels and need none.
    pub(crate) fn images(self) -> Images {
        let size = self.size;
        let gap: Vec<Color32> = (self.back.iter())
            .zip(&self.keep_out)
            .map(|(back, keep)| back.gamma_multiply(*keep))
            .collect();
        Images {
            maw: crop(&self.worm, size, MARGIN),
            gap: crop(&gap, size, 0),
            front: crop(&self.front, size, 0),
            rim: RimLight::new(size, &self.rim, &self.sunlit),
            shade: Shade::new(size, &self.teeth, &self.band, self.throat),
            back: ColorImage::new([size, size], self.back),
        }
    }

    /// The picture with the maw out by `rise` thousandths, composed on the
    /// CPU in one pass: the oracle for what the six textures blend to. At 0
    /// the maw is under the sand and not drawn at all: its lip's top is then
    /// at the keep-out line, and the antialiased edges of the two would leave
    /// a faint trace of it in the day look.
    #[cfg(test)]
    fn compose(&self, rise: u16) -> ColorImage {
        let size = self.size;
        let slide = Slide::new(size, rise);
        let pixels = (0..size * size)
            .map(|i| {
                let (x, y) = (i % size, i / size);
                let body = self.maw_pixel(&slide, x, y, 1.0 - self.keep_out[i]);
                over(self.front[i], over(body, self.back[i]))
            })
            .collect();
        ColorImage::new([size, size], pixels)
    }

    /// The maw at (x, y) of the picture, slid by `slide`, with `shown` of it
    /// out of the sand, its teeth sunk into the throat's dark there as far as
    /// the band says, and with its rim's light as far as the sun is behind it
    /// here.
    #[cfg(test)]
    fn maw_pixel(&self, slide: &Slide, x: usize, y: usize, shown: f32) -> Color32 {
        let body = self.sunk(slide, x, y).gamma_multiply(shown);
        let lit = shown * self.sunlit[y * self.size + x];
        if lit > 0.0 {
            over(slide.pixel(&self.rim, x, y).gamma_multiply(lit), body)
        } else {
            body
        }
    }

    /// The maw's layer at (x, y) of the picture, slid by `slide`, taken
    /// toward the throat's color by the band wherever either pixel of the
    /// teeth's layer it slides between holds any of a tooth: the shade, from
    /// the whole layers. The throat is as much of it as the maw covers here.
    #[cfg(test)]
    fn sunk(&self, slide: &Slide, x: usize, y: usize) -> Color32 {
        let worm = slide.pixel(&self.worm, x, y);
        let band = self.band[y * self.size + x];
        let teeth = (slide.between(&self.teeth, x, y)).map(|pixel| pixel != Color32::TRANSPARENT);
        if band > 0.0 && slide.shows(teeth) {
            let dark = self.throat.gamma_multiply(f32::from(worm.a()) / 255.0);
            worm.lerp_to_gamma(dark, band)
        } else {
            worm
        }
    }
}

/// How far down the maw's layers are slid for a rise: whole rows of the
/// picture, and a share of one more.
struct Slide {
    /// The picture's side, in pixels: what a whole layer's rows are long.
    #[cfg(test)]
    size: usize,
    rise: u16,
    rows: usize,
    part: f32,
}

impl Slide {
    /// The slide for the maw out by `rise` thousandths, in a picture `size`
    /// pixels square.
    fn new(size: usize, rise: u16) -> Self {
        let down = sunk(size, rise);
        Self {
            #[cfg(test)]
            size,
            rise,
            rows: down.floor() as usize,
            part: down.fract(),
        }
    }

    /// Where the maw's head is, as a point of the grid.
    #[cfg(test)]
    fn head(&self) -> Pos2 {
        HEAD + egui::vec2(0.0, lowered(self.rise))
    }

    /// The two pixels of a whole layer of the maw that the one at (x, y)
    /// slides between: see [`Slide::above`].
    #[cfg(test)]
    fn between(&self, layer: &[Color32], x: usize, y: usize) -> [Color32; 2] {
        self.above(y, |row| layer[row * self.size + x])
    }

    /// The two pixels of a layer of the maw that the one at row `y` of the
    /// picture slides between, where `column` gives the layer's pixel at a
    /// row of the picture in that pixel's column: those the slide's rows and
    /// one more row above it. Clear above the top of the picture, and clear
    /// everywhere while the maw is under the sand.
    fn above(&self, y: usize, column: impl Fn(usize) -> Color32) -> [Color32; 2] {
        let at = |up: usize| {
            y.checked_sub(up)
                .filter(|_| self.rise > 0)
                .map_or(Color32::TRANSPARENT, &column)
        };
        [at(self.rows), at(self.rows + 1)]
    }

    /// The pixel of a whole layer of the maw that slides to (x, y).
    #[cfg(test)]
    fn pixel(&self, layer: &[Color32], x: usize, y: usize) -> Color32 {
        self.blend(self.between(layer, x, y))
    }

    /// The pixel that slides between `near`, the slide's rows above, and
    /// `far`, one more row above.
    fn blend(&self, [near, far]: [Color32; 2]) -> Color32 {
        // Most of a layer is clear, and a share of the same color is it.
        if near == far {
            near
        } else {
            near.lerp_to_gamma(far, self.part)
        }
    }

    /// Whether the pixel that slides between `near` and `far`, as for
    /// [`Slide::blend`], shows any of a part that they hold: the far one only
    /// where the slide blends some of it in.
    fn shows(&self, [near, far]: [bool; 2]) -> bool {
        near || (far && self.part > 0.0)
    }
}

/// [`maw_parts`] without the teeth's layer: the maw and its rim's light, as
/// the tests of the two take them.
#[cfg(test)]
fn maw_layers(size: usize, scene: &Cover) -> (Vec<Color32>, Vec<Color32>) {
    let [worm, rim, _] = maw_parts(size, scene);
    (worm, rim)
}

/// The maw all the way out, as three layers `size` pixels square, from one
/// pass over its shape: the lip, the throat and the teeth in it; the light
/// the lip's rim catches, kept apart from them; and how much of each pixel
/// the teeth fill, as white, for the shade at the sand.
///
/// The lip and the throat, with the cover's own teeth in the throat: small and
/// many, lost in the dark as they go in. The throat, and a pixel round it, is
/// sampled whole.
fn maw_parts(size: usize, scene: &Cover) -> [Vec<Color32>; 3] {
    let teeth = rings::teeth();
    let throat = THROAT * SCALE + GRID / size as f32;
    sample(
        size,
        |q| {
            let v = (q - HEAD) / SCALE;
            // The cover has teeth of its own: inside the throat there is
            // nothing for the shape to tell apart.
            if v.length() < THROAT {
                Some(Part::Throat)
            } else {
                maw(MAW + v)
            }
        },
        |q, part| {
            let v = (q - HEAD) / SCALE;
            match part {
                None => [Color32::TRANSPARENT; 3],
                Some(Part::Lip) => [
                    lip(v, scene),
                    rim_light(v, scene).unwrap_or(Color32::TRANSPARENT),
                    Color32::TRANSPARENT,
                ],
                Some(Part::Throat) => {
                    let tooth = teeth(v);
                    [
                        tooth.map_or(scene.throat, |dark| {
                            scene.tooth.lerp_to_gamma(scene.throat, dark)
                        }),
                        Color32::TRANSPARENT,
                        tooth.map_or(Color32::TRANSPARENT, |_| Color32::WHITE),
                    ]
                }
            }
        },
        |q| q.distance(HEAD) < throat,
    )
}

/// How far in from the lip's outer edge its rim reaches, in the shapes'
/// units: a little under half the lip, which is `LIP - THROAT` deep.
const RIM: f32 = 0.7;
/// Where the rim gives out on the way down round the lip: a little below
/// the level of the maw's center, where the sine of the angle down from
/// that level is `RIM_ENDS`. The rim is brightest at the top, and fades as
/// the square of the way from there to here.
const RIM_ENDS: f32 = 0.15;
/// How far inside the sun's edge the rim catches all of the sun's light, in
/// the grid's units; out to the edge it catches less and less, on a
/// smoothstep. A rim lit at a pixel is then this far from the sky, and the
/// sky beside the sun is never behind a lit rim: the lip there is its plain
/// color, at 4.5:1 or more against it at every stage of the rise. The far
/// dune hides the sun from the rim over the same distance, inside its crest,
/// so that the rim's light never steps from one pixel to the next where the
/// lip crosses the crest.
const GLARE: f32 = 0.6;

/// The lip at `v`, a point of it measured from the maw's center in the
/// shapes' units, without its rim's light (see [`rim_light`]).
///
/// The lip is the sand's hue at a lower value, and it may merge with the
/// sand it comes out of: it stands apart by shading, not by a dark outline.
/// It is `worm` at its outer edge and darkens to `worm_shade` where it meets
/// the throat, on a smoothstep.
fn lip(v: egui::Vec2, scene: &Cover) -> Color32 {
    let r = v.length();
    let inward = ((LIP - r) / (LIP - THROAT)).clamp(0.0, 1.0);
    scene
        .worm
        .lerp_to_gamma(scene.worm_shade, smoothstep(inward))
}

/// The light the lip's rim catches at `v`, measured as for [`lip`]: what
/// it adds to the lip there, as a premultiplied color with no alpha, or
/// None where the rim gives out.
///
/// Along the lip's outer edge a soft rim takes the lip to `worm_rim`,
/// brightest at the top; it fades as the square of the way in from the edge
/// to `RIM`, and as the square of the way down from the top to `RIM_ENDS`.
/// The rim is lit by the sun behind it, so the cover adds this light only as
/// far as the sun is behind the rim where the maw then stands. `worm_rim`
/// is lighter than the lip in every channel, so the light is never less
/// than none.
fn rim_light(v: egui::Vec2, scene: &Cover) -> Option<Color32> {
    let r = v.length();
    let edge = (1.0 - (LIP - r) / RIM).clamp(0.0, 1.0);
    // `v.y / r` is the sine of the angle down from the level: -1 at the top.
    let up = ((RIM_ENDS - v.y / r) / (1.0 + RIM_ENDS)).clamp(0.0, 1.0);
    let lit = edge * edge * up * up;
    (lit > 0.0).then(|| {
        let (lip, to) = (lip(v, scene).to_array(), scene.worm_rim.to_array());
        let [red, green, blue] = [0, 1, 2].map(|c| {
            ((f32::from(to[c]) - f32::from(lip[c])) * lit)
                .round()
                .max(0.0) as u8
        });
        Color32::from_rgba_premultiplied(red, green, blue, 0)
    })
}

/// Premultiplied `top` laid over `under`.
fn over(top: Color32, under: Color32) -> Color32 {
    let keep = 1.0 - f32::from(top.a()) / 255.0;
    let [r, g, b, a] = [0, 1, 2, 3].map(|c| {
        (f32::from(top.to_array()[c]) + f32::from(under.to_array()[c]) * keep).round() as u8
    });
    Color32::from_rgba_premultiplied(r, g, b, a)
}

/// What the GPU draws at the cover's pixel (x, y) of `crop`, a layer drawn
/// `down` pixels below its place: its image sampled between two rows at the
/// rect's fractional place, clear outside the rect.
#[cfg(any(test, feature = "demo"))]
fn quad_pixel(crop: &Crop, down: f32, x: usize, y: usize) -> Color32 {
    let [width, height] = crop.image.size;
    let offset = crop.place.offset(down);
    // The texel row and column at the pixel's center, less half a texel: the
    // row above the sample, and how far below it it is.
    let (column, row) = (x as f32 - offset.x, y as f32 - offset.y);
    assert_eq!(column.fract(), 0.0, "the rect is on whole pixels across");
    let (inside_x, inside_y) = (column + 0.5, row + 0.5);
    if !(0.0..=width as f32).contains(&inside_x) || !(0.0..=height as f32).contains(&inside_y) {
        return Color32::TRANSPARENT;
    }
    let (above, part) = (row.floor(), row - row.floor());
    let texel = |row: f32| {
        let row = row.clamp(0.0, height as f32 - 1.0) as usize;
        crop.image.pixels[row * width + column as usize]
    };
    texel(above).lerp_to_gamma(texel(above + 1.0), part)
}

/// The cover as the GPU blends its six textures, in 8-bit steps, on the CPU:
/// the back, the maw sampled between two rows of its image at its rect's
/// fractional place, then the rim's light and the shade composed for `light`
/// and `shade`, the gap and the front, each sampled at its own rect, laid
/// over what is there and rounded to eight bits. Where the maw is not drawn at
/// `rise`, nor are the rim's light, the shade and the gap. The rim's light and
/// the shade are composed for `rise` when the cover is drawn right; another
/// rise is a stale texture, and None leaves the texture out of the blend
/// altogether.
///
/// The one copy of the blend: the cover's tests take it for the oracle of what
/// the six textures show, and the README's banner (`banner::picture`) makes
/// its square of it, so a change to the layers or the order of [`draw`]
/// follows here and nowhere else.
#[cfg(any(test, feature = "demo"))]
pub(crate) fn blended(
    images: &Images,
    rise: u16,
    light: Option<u16>,
    shade: Option<u16>,
) -> ColorImage {
    let size = images.back.size[0];
    let down = drawn_down(size, rise);
    let rim = light.map(|light| Crop {
        image: images.rim.compose(light),
        place: images.rim.place,
    });
    let shade = shade.map(|shade| Crop {
        image: images.shade.compose(shade),
        place: images.shade.place,
    });
    let pixels = (0..size * size)
        .map(|i| {
            let (x, y) = (i % size, i / size);
            let mut seen = images.back.pixels[i];
            if let Some(down) = down {
                seen = over(quad_pixel(&images.maw, down, x, y), seen);
                if let Some(rim) = &rim {
                    seen = over(quad_pixel(rim, 0.0, x, y), seen);
                }
                if let Some(shade) = &shade {
                    seen = over(quad_pixel(shade, 0.0, x, y), seen);
                }
                seen = over(quad_pixel(&images.gap, 0.0, x, y), seen);
            }
            over(quad_pixel(&images.front, 0.0, x, y), seen)
        })
        .collect();
    ColorImage::new([size, size], pixels)
}

/// `size` by `size` pixels of `paint`, the picture's color at a point of
/// the grid or None where it is clear, premultiplied. A pixel whose four
/// corners agree takes their color; one an edge crosses averages its
/// samples. No part of the cover but the teeth is small enough to fall
/// between corners; see [`fine_layer`].
fn layer(size: usize, paint: impl Fn(egui::Pos2) -> Option<Color32>) -> Vec<Color32> {
    fine_layer(size, paint, |_| false)
}

/// [`layer`], but every pixel whose center `fine` holds averages its
/// samples, whatever its corners say: what is there may be finer than a
/// pixel.
///
/// The maw's throat needs it. At the window's cover, 136 points at 2×, the
/// innermost ring of teeth is about 20 pixels from the head, so its 40
/// teeth stand 3 pixels apart and 1.7 wide at the base, half that at 1×;
/// and every tooth narrows to a point. A pixel whose four corners fall in
/// the dark between two teeth may hold one, which the corners alone would
/// drop. Only the throat's disc is sampled whole, about a tenth of the
/// picture.
fn fine_layer(
    size: usize,
    paint: impl Fn(egui::Pos2) -> Option<Color32>,
    fine: impl Fn(egui::Pos2) -> bool,
) -> Vec<Color32> {
    let [pixels] = sample(
        size,
        &paint,
        |_, color| [color.unwrap_or(Color32::TRANSPARENT)],
        fine,
    );
    pixels
}

/// `N` layers at once, each `size` by `size` pixels, premultiplied, from one
/// pass over the picture.
///
/// `kind` says what is at a point of the grid, and `shade` gives the `N`
/// colors there for a point and its kind. Pixels are told apart by their
/// corners' kind, not by their colors: a pixel whose four corners are of one
/// kind is shaded once, at its center, so that a color that changes smoothly
/// across a part, like the lip's, costs one shade a pixel; one an edge
/// crosses averages the shades of its samples, and so does every pixel whose
/// center `fine` holds, whatever its corners say: what is there may be finer
/// than a pixel.
fn sample<K: Copy + PartialEq, const N: usize>(
    size: usize,
    kind: impl Fn(egui::Pos2) -> K,
    shade: impl Fn(egui::Pos2, K) -> [Color32; N],
    fine: impl Fn(egui::Pos2) -> bool,
) -> [Vec<Color32>; N] {
    let scale = GRID / size as f32;
    let at = |x: f32, y: f32| pos2(x * scale, y * scale);
    let mut corners = Vec::with_capacity((size + 1) * (size + 1));
    for y in 0..=size {
        for x in 0..=size {
            corners.push(kind(at(x as f32, y as f32)));
        }
    }
    let corner = |x: usize, y: usize| corners[y * (size + 1) + x];
    let offset = |n: usize| (n as f32 + 0.5) / SAMPLES as f32;
    let mut layers: [Vec<Color32>; N] = std::array::from_fn(|_| Vec::with_capacity(size * size));
    // Plain loops: this runs over every pixel, and an unoptimized build is
    // slowed by iterator adapters there.
    for y in 0..size {
        for x in 0..size {
            let first = corner(x, y);
            let middle = center(y * size + x, size);
            let whole = !fine(middle)
                && corner(x + 1, y) == first
                && corner(x, y + 1) == first
                && corner(x + 1, y + 1) == first;
            let pixel = if whole {
                shade(middle, first)
            } else {
                let mut sum = [[0u32; 4]; N];
                for k in 0..SAMPLES * SAMPLES {
                    let q = at(
                        x as f32 + offset(k % SAMPLES),
                        y as f32 + offset(k / SAMPLES),
                    );
                    let colors = shade(q, kind(q));
                    for n in 0..N {
                        let color = colors[n].to_array();
                        for c in 0..4 {
                            sum[n][c] += u32::from(color[c]);
                        }
                    }
                }
                let count = (SAMPLES * SAMPLES) as u32;
                std::array::from_fn(|n| {
                    let [r, g, b, a] = sum[n].map(|total| ((total + count / 2) / count) as u8);
                    Color32::from_rgba_premultiplied(r, g, b, a)
                })
            };
            for n in 0..N {
                layers[n].push(pixel[n]);
            }
        }
    }
    layers
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use egui::TextureId;
    use egui::epaint::{ImageData, Mesh};

    use super::prebuilt::{Key, Slot, images_from};
    use super::*;
    use crate::color::{
        DAY_KELVIN, Look, MAX_BRIGHTNESS, PURE_RED_KELVIN, kelvin_at_redness, redness_of,
    };
    use crate::theme::{Palette, red_contrast};
    use crate::ui::headless::{Frame, Headless};
    use crate::ui::temperature;
    use crate::ui::view::{COVER_RECT, COVER_SCENE};

    /// The index of the pixel at a point of the grid, in a picture `size`
    /// pixels square.
    fn index(q: egui::Pos2, size: usize) -> usize {
        let (x, y) = (
            (q.x / GRID * size as f32) as usize,
            (q.y / GRID * size as f32) as usize,
        );
        y * size + x
    }

    /// The cover's pixel at a point of the grid.
    fn at(image: &ColorImage, q: egui::Pos2) -> Color32 {
        image.pixels[index(q, image.size[0])]
    }

    #[test]
    fn the_worm_rises_with_the_red() {
        let scene = Palette::DARK.cover;
        let size = 72;
        let (layers, bare) = with_and_without_the_maw(size, scene);
        // The top row where the maw shows, if it does: the first where the
        // picture differs from the same one drawn with no maw, faint or not.
        let top = |rise: u16| {
            let (image, open) = (layers.compose(rise), bare.compose(rise));
            (0..size * size)
                .find(|&i| image.pixels[i] != open.pixels[i])
                .map(|i| i / size)
        };
        // At no red the cover is sky and sand alone.
        assert_eq!(top(0), None, "down");
        // As the red grows, the maw climbs: each step of the rise here
        // slides it up by more than a row.
        let mut before = size;
        for rise in (50..=1000).step_by(50) {
            let row = top(rise).unwrap_or_else(|| panic!("rise {rise}: no maw in sight"));
            assert!(
                row < before,
                "rise {rise}: the maw's top is row {row} after {before}"
            );
            before = row;
        }
        // All the way out, its top is the lip's top over the head, and the
        // lip shows its own color there, whole, where the sky was.
        let lip_top = ((HEAD.y - LIP * SCALE) / GRID * size as f32) as usize;
        assert!(
            before.abs_diff(lip_top) <= 1,
            "the maw's top is row {before}, not {lip_top}"
        );
        let lip = HEAD - egui::vec2(0.0, (LIP + THROAT) / 2.0 * SCALE);
        let own = layers.worm[index(lip, size)];
        assert_eq!(own.a(), 255, "the lip is whole there");
        assert_eq!(at(&layers.compose(1000), lip), own, "out");
        assert_ne!(
            at(&bare.compose(1000), lip),
            own,
            "nothing else is that color"
        );
    }

    /// The rise the view gives the cover for the night look at `kelvin`:
    /// from the Temperature slider's position, not from the live tint.
    fn rise_at(kelvin: u16) -> f32 {
        let look = Look {
            kelvin,
            brightness: MAX_BRIGHTNESS,
        };
        rise(temperature(look).0)
    }

    #[test]
    fn the_worm_stirs_a_quarter_of_the_way_to_pure_red() {
        assert_eq!(rise_at(DAY_KELVIN), 0.0, "the day look");
        assert_eq!(rise_at(PURE_RED_KELVIN), 1.0, "pure red");
        // Every look on the slider, from the day to pure red.
        let mut before = 0.0;
        for kelvin in (PURE_RED_KELVIN..=DAY_KELVIN).rev().step_by(10) {
            let look = Look {
                kelvin,
                brightness: MAX_BRIGHTNESS,
            };
            let redness = temperature(look).0;
            let rise = rise_at(kelvin);
            if redness <= STIRS {
                assert_eq!(rise, 0.0, "{kelvin} K is {redness} red: the maw is under");
            } else {
                assert!(rise > 0.0, "{kelvin} K is {redness} red: the worm stirs");
            }
            assert!(rise >= before, "{kelvin} K: {rise} after {before}");
            before = rise;
        }
        let halfway = rise_at(kelvin_at_redness(0.625));
        assert!(0.4 < halfway && halfway < 0.6, "{halfway}");
    }

    /// The cover's layers at `size` pixels, and the same with the maw's
    /// layers, it and its rim's light, cleared: the cover as it would be
    /// drawn with no maw in it.
    fn with_and_without_the_maw(size: usize, scene: Cover) -> (Layers, Layers) {
        let mut bare = Layers::new(size, scene);
        bare.worm.fill(Color32::TRANSPARENT);
        bare.rim.fill(Color32::TRANSPARENT);
        (Layers::new(size, scene), bare)
    }

    #[test]
    fn nothing_but_the_maw_comes_out() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let (layers, bare) = with_and_without_the_maw(size, scene);
        let pixel = GRID / size as f32;
        let radius = LIP * SCALE;
        // The layer itself holds no trace beyond the lip, but a pixel for
        // its edge: a body drawn again, however faint, fails here, where the
        // sand may hide it.
        for (i, color) in layers.worm.iter().enumerate() {
            if *color != Color32::TRANSPARENT {
                let beyond = center(i, size).distance(HEAD) - radius;
                assert!(beyond <= pixel, "pixel {i} is {beyond} beyond the lip");
            }
        }
        // At every rise the picture differs from the one with no maw only
        // within the lip of where the head is then: a pixel more for the
        // edge, and one more for the slide between two rows.
        for rise in (0..=1000u16).step_by(50) {
            let head = Slide::new(size, rise).head();
            let (image, open) = (layers.compose(rise), bare.compose(rise));
            let mut differ = 0;
            for (i, (pixel_in, pixel_out)) in image.pixels.iter().zip(&open.pixels).enumerate() {
                if pixel_in != pixel_out {
                    differ += 1;
                    let beyond = center(i, size).distance(head) - radius;
                    assert!(
                        beyond <= 2.0 * pixel,
                        "rise {rise}: pixel {i} is {beyond} beyond the lip"
                    );
                }
            }
            if rise == 1000 {
                assert!(differ > 0, "the maw is out at pure red");
            }
        }
    }

    #[test]
    fn the_maw_ends_sunk_into_the_sand() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let layers = Layers::new(size, scene);
        let image = layers.compose(1000);
        let slide = Slide::new(size, 1000);
        // Of the maw's whole pixels, those the picture does not show as the
        // maw is out of the sand, with the rim's light the sun gives them
        // there, are hidden by the sand.
        let (mut whole, mut hidden) = (0, 0);
        for (i, color) in layers.worm.iter().enumerate() {
            if color.a() == 255 {
                whole += 1;
                let lit = layers.maw_pixel(&slide, i % size, i / size, 1.0);
                if image.pixels[i] != lit {
                    hidden += 1;
                    assert!(
                        center(i, size).y >= HEAD.y,
                        "pixel {i} in the top half of the maw is hidden"
                    );
                }
            }
        }
        assert!(hidden > 0, "the sand cuts the bottom of the maw");
        let share = hidden as f32 / whole as f32;
        assert!(
            (0.05..=0.15).contains(&share),
            "the sand hides {share} of the maw"
        );
        assert_eq!(at(&image, HEAD), scene.throat, "the throat is at the head");
    }

    /// The owner chose these places from the rendered variants (the one
    /// "sunk to the sand, sun halfway", 2026-10-22): the maw's head at pure
    /// red, and the sun's center above it. A change to either needs their
    /// word.
    #[test]
    fn the_final_places_are_the_owners_choice() {
        assert_eq!(HEAD, pos2(9.0, 11.6));
        assert_eq!(SUN_CENTER, pos2(9.0, 8.8));
    }

    #[test]
    fn the_maw_is_unseen_until_it_stirs() {
        let scene = Palette::DARK.cover;
        let (layers, bare) = with_and_without_the_maw(272, scene);
        // No pixel differs from the picture with no maw in it, faint or not.
        let (image, open) = (layers.compose(0), bare.compose(0));
        let differ: Vec<_> = (0..image.pixels.len())
            .filter(|&i| image.pixels[i] != open.pixels[i])
            .map(|i| (center(i, 272), image.pixels[i], open.pixels[i]))
            .collect();
        assert!(differ.is_empty(), "{differ:?}");
        for step in 0..=25 {
            let redness = STIRS * step as f32 / 25.0;
            assert_eq!(rise(redness), 0.0, "{redness} red");
        }
    }

    #[test]
    fn the_maw_shows_as_soon_as_it_stirs() {
        let scene = Palette::DARK.cover;
        let (layers, bare) = with_and_without_the_maw(272, scene);
        // Walk the slider's red from where the maw stirs, as the view
        // does, until the picture first differs from the one with no maw.
        let shows = |redness: f32| {
            let rise = (rise(redness) * 1000.0).round() as u16;
            layers.compose(rise).pixels != bare.compose(rise).pixels
        };
        let first = (0..=150)
            .map(|step| STIRS + step as f32 * 0.005)
            .find(|redness| shows(*redness))
            .expect("the maw shows before pure red");
        assert!(first <= STIRS + 0.03, "the maw first shows at {first} red");
    }

    #[test]
    fn the_maw_covers_most_of_the_sun() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let (layers, bare) = with_and_without_the_maw(size, scene);
        let (image, open) = (layers.compose(1000), bare.compose(1000));
        // The sun's pixels as drawn with no maw in front, and the ones the
        // maw leaves in view: those above its top, and all.
        let radius = LIP * SCALE;
        let sun: Vec<usize> = (0..size * size)
            .filter(|&i| open.pixels[i] == scene.sun)
            .collect();
        let left: Vec<usize> = sun
            .iter()
            .copied()
            .filter(|&i| image.pixels[i] == scene.sun)
            .collect();
        let above = left
            .iter()
            .filter(|&&i| center(i, size).y < HEAD.y - radius)
            .count();
        let (left, above) = (
            left.len() as f32 / sun.len() as f32,
            above as f32 / sun.len() as f32,
        );
        assert!(left < 0.5, "the maw leaves {left} of the sun in view");
        assert!(above >= 0.2, "{above} of the sun shows above the maw");
        // At pure red: sun above the maw, and sky in the corners.
        assert_eq!(at(&image, pos2(9.0, 5.0)), scene.sun, "above the maw");
        for corner in [pos2(0.1, 0.1), pos2(17.9, 0.1)] {
            assert_ne!(at(&image, corner), scene.sun, "{corner:?}");
        }
    }

    #[test]
    fn the_rise_is_a_smoothstep_of_the_red() {
        assert_eq!(rise(STIRS), 0.0);
        assert_eq!(rise(1.0), 1.0);
        assert_eq!(rise_at(PURE_RED_KELVIN), 1.0);
        let halfway = rise((STIRS + 1.0) / 2.0);
        assert!(0.4 < halfway && halfway < 0.6, "{halfway}");
        // Of the way from `STIRS` to pure red, by `t`.
        let redness = |t: f32| STIRS + t * (1.0 - STIRS);
        for t in [0.05, 0.1, 0.25, 0.4, 0.6, 0.75, 0.9, 0.95] {
            let smoothstep = 3.0 * t * t - 2.0 * t * t * t;
            assert!((rise(redness(t)) - smoothstep).abs() < 1e-5, "t = {t}");
            // It eases: under the linear rise before the middle, over it after.
            if t < 0.5 {
                assert!(rise(redness(t)) < t, "t = {t}");
            } else {
                assert!(rise(redness(t)) > t, "t = {t}");
            }
        }
    }

    /// [`blended`] with the rim's light and the shade composed for `rise`,
    /// as the cover draws them: the cover as the GPU blends the six textures.
    fn blend_like_the_gpu(images: &Images, rise: u16) -> ColorImage {
        blended(images, rise, Some(rise), Some(rise))
    }

    /// How far `gpu` is from `cpu`: the largest difference in any channel of
    /// any pixel, and how many pixels differ by more than 2.
    fn distance(cpu: &ColorImage, gpu: &ColorImage) -> (u8, usize) {
        let (mut worst, mut beyond) = (0, 0);
        for (a, b) in cpu.pixels.iter().zip(&gpu.pixels) {
            let most = (a.to_array().iter().zip(b.to_array()))
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap();
            worst = worst.max(most);
            beyond += usize::from(most > 2);
        }
        (worst, beyond)
    }

    /// Whether a blend that is `worst` off at most, and has `beyond` pixels
    /// off by more than 2, meets the tolerance for a cover `size`
    /// pixels square: 4 of 255 at most, and no more than 1% of the pixels
    /// beyond 2.
    fn is_tolerated(size: usize, (worst, beyond): (u8, usize)) -> bool {
        worst <= 4 && beyond * 100 <= size * size
    }

    /// The layers for a cover `size` pixels square, and the images made from
    /// a second copy of them, since the images take the layers.
    fn layers_and_images(size: usize) -> (Layers, Images) {
        (layers_of(size), layers_of(size).images())
    }

    #[test]
    fn the_six_textures_blend_to_what_compose_gives() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            for rise in [0, 1, 7, 100, 250, 333, 500, 667, 900, 999, 1000] {
                let (cpu, gpu) = (layers.compose(rise), blend_like_the_gpu(&images, rise));
                let off = distance(&cpu, &gpu);
                assert!(
                    is_tolerated(size, off),
                    "{size} px, rise {rise}: {off:?} (the largest difference, and the pixels off by more than 2)"
                );
            }
        }
    }

    /// `images` with the maw's image drawn `by` pixels across and down from
    /// where it belongs.
    fn with_the_maw_moved(images: &Images, by: [i32; 2]) -> Images {
        let mut moved = images.clone();
        moved.maw.place.at = [
            images.maw.place.at[0] + by[0],
            images.maw.place.at[1] + by[1],
        ];
        moved
    }

    /// `images` with the gap cleared: nothing laid over the maw to hide it
    /// above the sand.
    fn without_the_gap(images: &Images) -> Images {
        let mut bare = images.clone();
        bare.gap.image.pixels.fill(Color32::TRANSPARENT);
        bare
    }

    /// `images` with the rim's light's layer cleared: nothing for the rise to
    /// light, as if the fifth texture were not there.
    fn without_the_rims_light(images: &Images) -> Images {
        let mut bare = images.clone();
        bare.rim.layer.fill(Color32::TRANSPARENT);
        bare.rim.lit.clear();
        bare
    }

    /// `images` with the rim lit all round: the sun behind every pixel of the
    /// rim's box, as it was before the sun lit it.
    fn with_the_rim_lit_all_round(images: &Images) -> Images {
        let mut lit = images.clone();
        lit.rim.sunlit.fill(1.0);
        lit
    }

    /// The rise of the Temperature slider's step before the one that leaves
    /// the maw at `rise`: the step ten kelvin toward the day from the first
    /// step, going from pure red, whose rise is `rise` or less. A light
    /// composed for it is the stale light of a frame that moved the maw by
    /// one step and did not compose the light again.
    fn the_previous_step(rise: u16) -> u16 {
        let step = (PURE_RED_KELVIN..DAY_KELVIN)
            .step_by(10)
            .find(|&kelvin| thousandths(kelvin) <= rise)
            .expect("the day is at rise 0");
        thousandths(step + 10)
    }

    /// The rise nearest to `rise` (below it first) where the maw's layers
    /// slide by other whole rows than they do at `rise`, in a picture `size`
    /// pixels square. A shade composed for it is a stale shade that differs:
    /// the shade's colors depend on the band alone, so one composed for a rise
    /// that slides by the same rows, with a share of one more row at both, is
    /// the same image as the fresh one.
    fn the_nearest_rise_with_other_rows(size: usize, rise: u16) -> Option<u16> {
        let rows = Slide::new(size, rise).rows;
        (1..=1000u16).find_map(|by| {
            [rise.checked_sub(by), Some(rise + by)]
                .into_iter()
                .flatten()
                .find(|other| (1..=1000).contains(other) && Slide::new(size, *other).rows != rows)
        })
    }

    /// A blend with one thing wrong in it: the images it blends, the rise its
    /// rim's light is composed for at the maw's `rise` (None: left out of the
    /// blend), the rise its shade is composed for at the maw's `rise` in a
    /// picture `size` pixels square (the same), and the rises where the fault
    /// must show.
    struct Fault {
        what: &'static str,
        images: Images,
        light: fn(u16) -> Option<u16>,
        shade: fn(usize, u16) -> Option<u16>,
        rises: &'static [u16],
    }

    #[test]
    fn the_oracles_tolerance_fails_a_misplaced_or_missing_layer() {
        const EVERY: &[u16] = &[250, 500, 750, 1000];
        // At pure red the sun stands behind the whole rim, and a rim lit all
        // round looks as it does (`at_pure_red_the_rim_looks_as_it_does_lit_all_round`),
        // so the fault can show only below it.
        const BELOW_PURE_RED: &[u16] = &[250, 500, 750];
        // A stale light is the one composed a step (ten kelvin) of the
        // slider ago: the smallest move of the maw. It shows at all four.
        let fresh: fn(u16) -> Option<u16> = Some;
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            let fault = |what, images, light, rises| Fault {
                what,
                images,
                light,
                shade: |_, rise| Some(rise),
                rises,
            };
            let mutations = [
                fault(
                    "the maw one row lower",
                    with_the_maw_moved(&images, [0, 1]),
                    fresh,
                    EVERY,
                ),
                fault(
                    "the maw one column across",
                    with_the_maw_moved(&images, [1, 0]),
                    fresh,
                    EVERY,
                ),
                fault("no gap", without_the_gap(&images), fresh, EVERY),
                fault(
                    "no rim's light",
                    without_the_rims_light(&images),
                    |_| None,
                    EVERY,
                ),
                fault(
                    "the rim lit all round",
                    with_the_rim_lit_all_round(&images),
                    fresh,
                    BELOW_PURE_RED,
                ),
                fault(
                    "a stale rim's light, from the step before",
                    images.clone(),
                    |rise| Some(the_previous_step(rise)),
                    EVERY,
                ),
                Fault {
                    shade: |_, _| None,
                    ..fault("no shade", images.clone(), fresh, EVERY)
                },
                Fault {
                    shade: the_nearest_rise_with_other_rows,
                    ..fault(
                        "a stale shade, from the nearest rise that slides the teeth by other rows",
                        images.clone(),
                        fresh,
                        EVERY,
                    )
                },
            ];
            for rise in EVERY.iter().copied() {
                let cpu = layers.compose(rise);
                // The unbroken blend passes, so that a failure below is the
                // mutation's.
                let whole = distance(&cpu, &blend_like_the_gpu(&images, rise));
                assert!(
                    is_tolerated(size, whole),
                    "{size} px, rise {rise}: {whole:?}"
                );
                for fault in mutations.iter().filter(|fault| fault.rises.contains(&rise)) {
                    let (light, shade) = ((fault.light)(rise), (fault.shade)(size, rise));
                    // A stale shade is a fault where it differs from the
                    // fresh one.
                    if let Some(stale) = shade.filter(|stale| *stale != rise) {
                        assert_ne!(
                            fault.images.shade.compose(stale),
                            fault.images.shade.compose(rise),
                            "{size} px, rise {rise}: {} is the fresh shade",
                            fault.what
                        );
                    }
                    let off = distance(&cpu, &blended(&fault.images, rise, light, shade));
                    assert!(
                        !is_tolerated(size, off),
                        "{size} px, rise {rise}: {} is within the tolerance, at {off:?}",
                        fault.what
                    );
                }
            }
        }
    }

    #[test]
    fn the_rise_arrives_in_small_steps() {
        let jump = rise_at(PURE_RED_KELVIN) - rise_at(PURE_RED_KELVIN + 10);
        assert!((0.0..=0.02).contains(&jump), "860 K to 850 K is {jump}");
        // Each of the slider's last ten-kelvin steps.
        for kelvin in (PURE_RED_KELVIN..PURE_RED_KELVIN + 100).step_by(10) {
            let jump = rise_at(kelvin) - rise_at(kelvin + 10);
            assert!(
                (0.0..=0.03).contains(&jump),
                "{} K to {kelvin} K is {jump}",
                kelvin + 10
            );
        }
    }

    /// The lip at `v` as the cover shows it with the sun all the way behind
    /// it: with all of its rim's light.
    fn sunlit_lip(v: egui::Vec2, scene: &Cover) -> Color32 {
        over(
            rim_light(v, scene).unwrap_or(Color32::TRANSPARENT),
            lip(v, scene),
        )
    }

    /// The channels of a color added up.
    fn sum(color: Color32) -> u32 {
        u32::from(color.r()) + u32::from(color.g()) + u32::from(color.b())
    }

    /// How much of the light that would take the lip at `v` to `worm_rim`
    /// the rim adds there, from 0 to 1, over the three channels.
    fn rim_share(v: egui::Vec2, scene: &Cover) -> f32 {
        let all = sum(scene.worm_rim) - sum(lip(v, scene));
        rim_light(v, scene).map_or(0.0, |light| sum(light) as f32 / all as f32)
    }

    #[test]
    fn the_lip_darkens_from_its_outer_edge_to_the_throat() {
        let scene = Palette::DARK.cover;
        // Straight down, where the rim has given out.
        let down = |r: f32| egui::vec2(0.0, r);
        assert_eq!(lip(down(LIP), &scene), scene.worm, "outer edge");
        assert_eq!(lip(down(THROAT), &scene), scene.worm_shade, "inner edge");
        assert_eq!(rim_light(down(LIP), &scene), None, "no rim down there");
        // Each channel moves from one to the other and never back.
        let steps: Vec<[u8; 4]> = (0..=40)
            .map(|i| lip(down(LIP - (LIP - THROAT) * i as f32 / 40.0), &scene).to_array())
            .collect();
        for c in 0..3 {
            let (from, to) = (scene.worm.to_array()[c], scene.worm_shade.to_array()[c]);
            assert!(from > to, "the lip darkens inward in channel {c}");
            assert!(
                steps.windows(2).all(|pair| pair[1][c] <= pair[0][c]),
                "channel {c}: {steps:?}"
            );
        }
        // On a smoothstep: halfway in it is halfway between the two, and it
        // eases, with less than a quarter of the way done at a quarter.
        let middle = lip(down((LIP + THROAT) / 2.0), &scene);
        let halfway = scene.worm.lerp_to_gamma(scene.worm_shade, 0.5);
        for (got, want) in middle.to_array().into_iter().zip(halfway.to_array()) {
            assert!(got.abs_diff(want) <= 1, "{middle:?} is not {halfway:?}");
        }
        let done = |inward: f32| {
            let color = lip(down(LIP - (LIP - THROAT) * inward), &scene);
            f32::from(scene.worm.r() - color.r()) / f32::from(scene.worm.r() - scene.worm_shade.r())
        };
        assert!(
            done(0.25) < 0.25 && done(0.75) > 0.75,
            "{} {}",
            done(0.25),
            done(0.75)
        );
    }

    #[test]
    fn the_rim_is_brightest_at_the_top_and_gives_out_toward_the_sides() {
        let scene = Palette::DARK.cover;
        let up = |r: f32| egui::vec2(0.0, -r);
        // At the top of the outer edge the lip is the rim's color, all of it
        // the light the sun gives: no cover with it.
        assert_eq!(sunlit_lip(up(LIP), &scene), scene.worm_rim);
        assert_eq!(rim_light(up(LIP), &scene).map(|light| light.a()), Some(0));
        // In from the edge the rim fades, and `RIM` in it is gone.
        let inward: Vec<f32> = (0..=20)
            .map(|i| rim_share(up(LIP - RIM * i as f32 / 20.0), &scene))
            .collect();
        assert!(
            inward.windows(2).all(|pair| pair[1] <= pair[0]),
            "{inward:?}"
        );
        assert!(inward[0] > 0.99 && inward[5] > 0.0, "{inward:?}");
        assert_eq!(inward[20], 0.0, "{RIM} in from the edge");
        assert_eq!(
            sunlit_lip(up(LIP - RIM), &scene),
            lip(up(LIP - RIM), &scene)
        );
        // Along the edge, from the top to the sides, it fades.
        let along: Vec<f32> = (0..=90)
            .map(|deg| {
                let angle = (deg as f32).to_radians();
                rim_share(egui::vec2(angle.sin(), -angle.cos()) * LIP, &scene)
            })
            .collect();
        assert!(along.windows(2).all(|pair| pair[1] <= pair[0]), "{along:?}");
        // At the level of the center it is faint, on either side.
        for side in [-1.0, 1.0] {
            let share = rim_share(egui::vec2(side * LIP, 0.0), &scene);
            assert!(
                share < 0.02,
                "{share} of the rim's light at the level of the center"
            );
        }
        assert!(along[90] < 0.02 && along[45] > 0.3, "{along:?}");
        // From `RIM_ENDS` down it is gone.
        for sine in [RIM_ENDS, 0.3, 0.6, 1.0] {
            for side in [-1.0, 1.0] {
                let v = egui::vec2(side * (1.0f32 - sine * sine).sqrt(), sine) * LIP;
                assert_eq!(rim_share(v, &scene), 0.0, "{sine} down from the level");
            }
        }
    }

    #[test]
    fn a_pixel_the_corners_miss_is_kept_where_the_layer_is_sampled_whole() {
        // 36 pixels of half a unit each.
        let size = 36;
        let scale = GRID / size as f32;
        let (inside, outside) = (8, 28);
        // A line a fifth of a pixel wide, a little left of the middle of a
        // column: no pixel's corner falls on it. And a block with edges.
        let line = |column: usize| (column as f32 + 0.375) * scale;
        let red = Color32::from_rgb(255, 0, 0);
        let paint = |q: Pos2| {
            let on_line = [inside, outside]
                .iter()
                .any(|&column| (q.x - line(column)).abs() <= 0.1 * scale);
            let in_block = (6.3..11.7).contains(&q.x) && (1.3..2.7).contains(&q.y);
            if on_line {
                Some(Color32::WHITE)
            } else {
                in_block.then_some(red)
            }
        };
        // Whole only round a disc that holds the first line.
        let disc = |q: Pos2| q.distance(pos2(4.5, 9.0)) < 2.0;
        let coarse = layer(size, paint);
        let whole = fine_layer(size, paint, disc);
        assert!(coarse.contains(&red), "the block is there");
        for (i, (kept, plain)) in whole.iter().zip(&coarse).enumerate() {
            let (x, near) = (i % size, disc(center(i, size)));
            if !near {
                assert_eq!(kept, plain, "pixel {i} outside the disc");
            }
            // The line is between the corners: the plain layer drops it, and
            // the whole one keeps what it covers of the pixel.
            if x == outside || x == inside {
                assert_eq!(*plain, Color32::TRANSPARENT, "pixel {i}");
            }
            if x == inside && near {
                let covered = f32::from(kept.a()) / 255.0;
                assert!(
                    (0.15..=0.3).contains(&covered),
                    "pixel {i} holds {covered} of a pixel's width of the line"
                );
            } else if x == inside || x == outside {
                assert_eq!(*kept, Color32::TRANSPARENT, "pixel {i}");
            }
        }
    }

    #[test]
    fn the_worm_layer_keeps_the_teeth_the_corners_miss() {
        let scene = Palette::DARK.cover;
        // At the window's cover at 2x and at 1x, the layer's throat is what a
        // picture four times as fine shows, averaged down. Sampled by its
        // corners alone, the inner rings' teeth drop out, and pixels of the
        // throat are off by 80 levels in red or more.
        for size in [272, 136] {
            let (worm, _) = maw_layers(size, &scene);
            let (finer, _) = maw_layers(size * 4, &scene);
            let mut worst = 0;
            for i in (0..size * size).filter(|&i| center(i, size).distance(HEAD) < THROAT * SCALE) {
                let (x, y) = (i % size, i / size);
                let total: u32 = (0..16)
                    .map(|k| (y * 4 + k / 4) * size * 4 + x * 4 + k % 4)
                    .map(|j| u32::from(finer[j].r()))
                    .sum();
                let averaged = (total + 8) / 16;
                worst = worst.max(averaged.abs_diff(u32::from(worm[i].r())));
            }
            assert!(
                worst <= 40,
                "size {size}: a pixel of the throat is {worst} levels off"
            );
        }
    }

    /// The mean of the 16 pixels of a layer four times as fine that one pixel
    /// of the coarse one holds, in channel `c`.
    fn averaged_down(finer: &[Color32], size: usize, i: usize, c: usize) -> u32 {
        let (x, y) = (i % size, i / size);
        let total: u32 = (0..16)
            .map(|k| (y * 4 + k / 4) * size * 4 + x * 4 + k % 4)
            .map(|j| u32::from(finer[j].to_array()[c]))
            .sum();
        (total + 8) / 16
    }

    #[test]
    fn the_lip_and_its_rim_match_a_finer_picture_averaged_down() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let pixel = GRID / size as f32;
        let (worm, rim) = maw_layers(size, &scene);
        let (finer_worm, finer_rim) = maw_layers(size * 4, &scene);
        // The lip's pixels: from a pixel in from the throat to a pixel out
        // from the lip's edge. Where the lip is whole the two agree to a
        // level or two; where its edge crosses a pixel, a coarse pixel
        // counts sixteen samples to the fine one's 256, and may be some
        // levels off.
        let (mut whole, mut edge) = (0, 0);
        for i in 0..size * size {
            let from_head = center(i, size).distance(HEAD);
            if !(THROAT * SCALE + pixel..=LIP * SCALE + pixel).contains(&from_head) {
                continue;
            }
            let is_whole = worm[i].a() == 255 && averaged_down(&finer_worm, size, i, 3) == 255;
            for (layer, finer) in [(&worm, &finer_worm), (&rim, &finer_rim)] {
                for c in 0..4 {
                    let off = averaged_down(finer, size, i, c)
                        .abs_diff(u32::from(layer[i].to_array()[c]));
                    let worst = if is_whole { &mut whole } else { &mut edge };
                    *worst = (*worst).max(off);
                }
            }
        }
        assert!(whole <= 2, "a whole pixel of the lip is {whole} levels off");
        assert!(edge <= 32, "a pixel at the lip's edge is {edge} levels off");
    }

    #[test]
    fn the_lip_and_its_rim_come_from_one_pass_shaded_once_a_pixel() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let pixel = GRID / size as f32;
        let (worm, rim) = maw_layers(size, &scene);
        let (mut whole, mut lit) = (0, 0);
        for i in 0..size * size {
            // The rim's light lies only where the lip does.
            assert!(
                rim[i] == Color32::TRANSPARENT || worm[i].a() > 0,
                "pixel {i}: the rim's light is outside the lip"
            );
            // A pixel well inside the lip is the lip's color at its center,
            // and the rim's light there, exactly.
            let from_head = center(i, size).distance(HEAD);
            if (THROAT * SCALE + 2.0 * pixel..=LIP * SCALE - 2.0 * pixel).contains(&from_head) {
                let v = (center(i, size) - HEAD) / SCALE;
                assert_eq!(worm[i], lip(v, &scene), "pixel {i}");
                let light = rim_light(v, &scene).unwrap_or(Color32::TRANSPARENT);
                assert_eq!(rim[i], light, "pixel {i}");
                whole += 1;
                lit += usize::from(light != Color32::TRANSPARENT);
            }
        }
        assert!(whole > 1000 && lit > 100, "{whole} pixels, {lit} lit");
    }

    #[test]
    fn a_part_that_shades_smoothly_costs_one_shade_a_pixel() {
        let size = 272;
        let radius = LIP * SCALE;
        let shades = std::cell::Cell::new(0);
        // A disc whose color changes smoothly across it, so that no two of a
        // pixel's corners would match by color.
        let color_at =
            |q: Pos2| Color32::from_rgb((q.distance(HEAD) / radius * 200.0) as u8 + 20, 0, 0);
        // The module's sampler: `quad_pixel` is the GPU's.
        let [pixels] = super::sample(
            size,
            |q| (q.distance(HEAD) < radius).then_some(()),
            |q, inside| {
                [inside.map_or(Color32::TRANSPARENT, |()| {
                    shades.set(shades.get() + 1);
                    color_at(q)
                })]
            },
            |_| false,
        );
        let touched = pixels.iter().filter(|pixel| pixel.a() > 0).count();
        assert!(touched > 10_000, "{touched} pixels");
        // A shade a pixel, and sixteen for each pixel the edge crosses, about
        // one in forty here: not sixteen for every pixel.
        assert!(
            shades.get() < touched * 2,
            "{} shades for {touched} pixels",
            shades.get()
        );
        // And a whole pixel is the color at its center.
        let middle = index(HEAD + egui::vec2(radius / 2.0, 0.0), size);
        assert_eq!(pixels[middle], color_at(center(middle, size)));
    }

    #[test]
    fn a_slide_moves_a_layer_down_by_the_drop_and_loses_none_of_it() {
        let size = 36;
        let mut layer = vec![Color32::TRANSPARENT; size * size];
        let marked = Color32::from_rgba_premultiplied(200, 0, 0, 255);
        layer[2 * size + 5] = marked;
        // While the maw is under the sand, nothing of it is there.
        let under = Slide::new(size, 0);
        assert!((0..size * size).all(|i| under.pixel(&layer, i % size, i / size).a() == 0));
        assert_eq!(under.head(), HEAD + egui::vec2(0.0, DROP));
        // All the way out, it has not moved.
        let out = Slide::new(size, 1000);
        assert_eq!(out.pixel(&layer, 5, 2), marked);
        assert_eq!(out.head(), HEAD);
        // On the way, the mark slides down by the drop, in pixels: the
        // alpha in a column keeps its total, and its middle moves by the
        // drop.
        for rise in [1, 100, 250, 500, 640, 900, 999] {
            let slide = Slide::new(size, rise);
            let alphas: Vec<f32> = (0..size)
                .map(|y| f32::from(slide.pixel(&layer, 5, y).a()))
                .collect();
            let total: f32 = alphas.iter().sum();
            assert!((total - 255.0).abs() <= 2.0, "rise {rise}: {total}");
            let middle: f32 = alphas
                .iter()
                .enumerate()
                .map(|(y, a)| y as f32 * a)
                .sum::<f32>()
                / total;
            let drop = lowered(rise) * size as f32 / GRID;
            assert!(
                (middle - 2.0 - drop).abs() < 0.05,
                "rise {rise}: {middle}, not {}",
                2.0 + drop
            );
        }
    }

    /// The most the light the rim adds may change from one pixel to the
    /// next along the lip, as a share of the most it adds. The sun's edge
    /// fades over `GLARE`, about nine pixels here, so by a sixth of the
    /// light a pixel at its steepest, and 0.2 along the lip with the rim's
    /// own change and the rounding to a level; the far dune's crest, which
    /// hid the light within one pixel before and stepped it by 0.6, fades
    /// over the same.
    const GENTLE: f32 = 0.3;

    #[test]
    fn the_rims_light_changes_gently_along_the_lip_at_the_dunes_crest_as_at_the_suns_edge() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let pixel = GRID / size as f32;
        let lit = Layers::new(size, scene);
        let mut dark = Layers::new(size, scene);
        dark.rim.fill(Color32::TRANSPARENT);
        let most = f32::from(scene.worm_rim.r() - scene.worm.r());
        // Two pixels in from the lip's outer edge: the lip whole, and its
        // rim strong.
        let radius = LIP * SCALE - 2.0 * pixel;
        let steps = (std::f32::consts::TAU * radius / pixel).ceil() as usize;
        let (mut worst, mut at_crest, mut at_edge) = (0.0f32, 0.0f32, 0.0f32);
        for rise in (50..=1000).step_by(50) {
            let head = Slide::new(size, rise).head();
            let (with, without) = (lit.compose(rise), dark.compose(rise));
            // The light the rim adds at a point of the lip, as a share of the
            // most it adds; None off the picture, and where the sand's
            // keep-out reaches.
            let added = |q: Pos2| {
                let on_the_picture = (0.0..GRID).contains(&q.x) && (0.0..GRID).contains(&q.y);
                on_the_picture
                    .then(|| index(q, size))
                    .filter(|&i| lit.keep_out[i] == 0.0 && lit.front[i].a() == 0)
                    .map(|i| {
                        (f32::from(with.pixels[i].r()) - f32::from(without.pixels[i].r())) / most
                    })
            };
            let walk: Vec<Pos2> = (0..=steps)
                .map(|n| {
                    let angle = n as f32 / steps as f32 * std::f32::consts::TAU;
                    head + egui::vec2(angle.cos(), angle.sin()) * radius
                })
                .collect();
            for pair in walk.windows(2) {
                let (Some(a), Some(b)) = (added(pair[0]), added(pair[1])) else {
                    continue;
                };
                worst = worst.max((a - b).abs());
                // Where the walk crosses the far dune's crest, or the band of
                // the sun's edge, how strong the light there is.
                let across = |f: &dyn Fn(Pos2) -> bool| f(pair[0]) != f(pair[1]);
                if across(&|q| DUNES[0].depth(q) >= 0.0) {
                    at_crest = at_crest.max(a.max(b));
                }
                if across(&|q| q.distance(SUN_CENTER) < SUN - GLARE / 2.0) {
                    at_edge = at_edge.max(a.max(b));
                }
            }
        }
        // The walk does meet a strong light at both, or the bound would say
        // nothing.
        assert!(
            at_crest > 0.2,
            "the strongest light at the crest is {at_crest}"
        );
        assert!(
            at_edge > 0.2,
            "the strongest light at the sun's edge is {at_edge}"
        );
        assert!(worst <= GENTLE, "the light steps by {worst} in a pixel");
    }

    /// Whether two colors are within `by` levels in every channel.
    fn near(a: Color32, b: Color32, by: u8) -> bool {
        a.to_array()
            .into_iter()
            .zip(b.to_array())
            .all(|(a, b)| a.abs_diff(b) <= by)
    }

    #[test]
    fn the_outer_teeth_meet_the_throat_in_the_layer() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let (worm, _) = maw_layers(size, &scene);
        let teeth = rings::teeth();
        // Pixels of ring 0, whose center is in a tooth and which the layer
        // shows in the tooth's color.
        let outer: Vec<usize> = (0..size * size)
            .filter(|&i| teeth((center(i, size) - HEAD) / SCALE) == Some(0.0))
            .filter(|&i| near(worm[i], scene.tooth, 3))
            .collect();
        assert!(!outer.is_empty(), "no pixel shows ring 0's teeth whole");
        // One of them has the throat's own color within two pixels.
        let meets = |i: usize| {
            let (x, y) = ((i % size) as i32, (i / size) as i32);
            (-2..=2).any(|dy| {
                (-2..=2).any(|dx| {
                    let (x, y) = ((x + dx) as usize, (y + dy) as usize);
                    near(worm[y * size + x], scene.throat, 3)
                })
            })
        };
        assert!(outer.iter().any(|&i| meets(i)), "no tooth meets the throat");
    }

    /// The rises the maw's edge at the gap is read at: 25 to 1000 in steps of
    /// 25, and the night look's.
    fn edge_rises() -> Vec<u16> {
        let night = crate::settings::Settings::default().night.kelvin;
        let mut rises: Vec<u16> = (25..=1000).step_by(25).collect();
        rises.push(thousandths(night));
        rises
    }

    /// What a pixel of the maw is, where the scan of the edge at the gap
    /// reads it.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Edge {
        /// A pixel of a tooth, which the maw shows whole.
        Tooth,
        /// A pixel of the lip, which the maw shows whole.
        Lip,
        /// A pixel of the throat between the teeth.
        Throat,
    }

    /// What the gap shows whole at the other end of a pair.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Shows {
        Sky,
        FarDune,
    }

    /// A pixel of the maw and one 1 or 2 pixels away in its row or column,
    /// where the gap shows the back whole, as the picture shows them.
    struct Meeting {
        edge: Edge,
        behind: Shows,
        rise: u16,
        at: [usize; 2],
        /// The reds of the two pixels: the maw's, then the back's.
        reds: [u8; 2],
        /// Their contrast in the red channel alone.
        ratio: f32,
    }

    /// Every pair of the maw's pixels that the cover shows whole, and the
    /// back's pixels 1 or 2 away in a row or column that the gap shows whole
    /// and the near dune does not cover, in the pictures that `layers`
    /// compose at `rises`.
    ///
    /// A pixel of the maw is one the sand's keep-out leaves out of reach
    /// (0.02 at most), with no near dune on it. A tooth is a pixel whose
    /// center is in one of the teeth, inside the throat, and which the maw's
    /// layer slides to whole; the lip is a whole pixel between the throat and
    /// the lip's edge, and the throat is a pixel inside it that holds no tooth
    /// at its center. The gap shows the back whole where it keeps out 0.98 or
    /// more, and the far dune is what is behind where it covers the pixel's
    /// center, else the low sky.
    fn meetings(layers: &Layers, rises: &[u16]) -> Vec<Meeting> {
        let size = layers.size;
        let teeth = rings::teeth();
        let mut found = Vec::new();
        for &rise in rises {
            let image = layers.compose(rise);
            let slide = Slide::new(size, rise);
            for y in 2..size - 2 {
                for x in 2..size - 2 {
                    let i = y * size + x;
                    if layers.keep_out[i] > 0.02 || layers.front[i].a() != 0 {
                        continue;
                    }
                    let mut seen = Vec::new();
                    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                        for step in 1..=2isize {
                            let j = (y as isize + dy * step) as usize * size
                                + (x as isize + dx * step) as usize;
                            if layers.keep_out[j] >= 0.98 && layers.front[j].a() == 0 {
                                let far = DUNES[0].covers(center(j, size), 0.0);
                                seen.push((j, if far { Shows::FarDune } else { Shows::Sky }));
                            }
                        }
                    }
                    if seen.is_empty() {
                        continue;
                    }
                    let v = (center(i, size) - slide.head()) / SCALE;
                    let own = slide.pixel(&layers.worm, x, y);
                    let edge = if v.length() >= LIP || own.a() < 255 && v.length() >= THROAT {
                        continue;
                    } else if v.length() >= THROAT {
                        Edge::Lip
                    } else if teeth(v).is_some() {
                        if own.a() < 255 {
                            continue;
                        }
                        Edge::Tooth
                    } else {
                        Edge::Throat
                    };
                    for (j, behind) in seen {
                        let (mine, theirs) = (image.pixels[i], image.pixels[j]);
                        found.push(Meeting {
                            edge,
                            behind,
                            rise,
                            at: [x, y],
                            reds: [mine.r(), theirs.r()],
                            ratio: red_contrast(mine, theirs),
                        });
                    }
                }
            }
        }
        found
    }

    /// The pairs of `edge` against `behind` among `found`.
    fn pairs_of(found: &[Meeting], edge: Edge, behind: Shows) -> Vec<&Meeting> {
        (found.iter())
            .filter(|pair| pair.edge == edge && pair.behind == behind)
            .collect()
    }

    /// The least contrast in the red channel among `pairs`.
    fn least_ratio(pairs: &[&Meeting]) -> f32 {
        (pairs.iter())
            .map(|pair| pair.ratio)
            .reduce(f32::min)
            .expect("some pairs")
    }

    #[test]
    fn no_tooth_meets_the_gap_under_1_8_in_the_red_channel() {
        // The cover's three sizes: a tooth on a 1x display is read too. The
        // pairs are fewer at the smaller ones, where the edge is shorter.
        for (size, found_at_least) in [(136, 60), (204, 80), (272, 100)] {
            let found = meetings(&layers_of(size), &edge_rises());
            for behind in [Shows::Sky, Shows::FarDune] {
                let teeth = pairs_of(&found, Edge::Tooth, behind);
                // The scan finds the edge: it would pass as well on a picture
                // where it no longer reads one.
                assert!(
                    teeth.len() > found_at_least,
                    "{size} px, teeth on the {behind:?}: {} pairs",
                    teeth.len()
                );
                for pair in &teeth {
                    assert!(
                        pair.ratio >= 1.8,
                        "{size} px, rise {}: the tooth at {:?} meets the {behind:?} at {:.2}:1 (red {} and {})",
                        pair.rise,
                        pair.at,
                        pair.ratio,
                        pair.reds[0],
                        pair.reds[1]
                    );
                }
            }
        }

        // Without the shade the same scan finds the teeth on the low sky
        // under 1.8, at rise 750 among them: the scan reads the edge the
        // shade takes the teeth off.
        let size = 272;
        let mut today = layers_of(size);
        today.band.fill(0.0);
        let before = meetings(&today, &[750]);
        let sky = pairs_of(&before, Edge::Tooth, Shows::Sky);
        let under: Vec<_> = sky.iter().filter(|pair| pair.ratio < 1.8).collect();
        assert!(
            !under.is_empty(),
            "no tooth meets the sky under 1.8 at rise 750 without the shade; the least is {:.2}:1",
            least_ratio(&sky)
        );
    }

    #[test]
    fn the_lip_and_the_throat_keep_their_contrast_against_the_sky_through_the_gap() {
        let found = meetings(&layers_of(272), &edge_rises());
        for (edge, floor) in [(Edge::Lip, 2.98), (Edge::Throat, 2.54)] {
            let pairs = pairs_of(&found, edge, Shows::Sky);
            assert!(pairs.len() > 100, "{edge:?}: {} pairs", pairs.len());
            let least = least_ratio(&pairs);
            assert!(
                least >= floor,
                "the {edge:?} meets the sky at {least:.3}:1, not {floor}:1 or more"
            );
        }
    }

    #[test]
    fn the_shade_changes_only_the_pixels_where_a_tooth_is_in_the_band() {
        let size = 272;
        let layers = layers_of(size);
        // The same cover with the band zeroed: the picture with no shade.
        let mut bare = layers_of(size);
        bare.band.fill(0.0);
        let mut changed = std::collections::BTreeMap::new();
        for rise in edge_rises() {
            let (shaded, unshaded) = (layers.compose(rise), bare.compose(rise));
            let slide = Slide::new(size, rise);
            let mut differ = 0;
            for i in 0..size * size {
                let (x, y) = (i % size, i / size);
                // The code shades a pixel where the band is above 0 and the
                // pixel of the teeth's layer that slides to it holds any of a
                // tooth: the near one, or the far one where the slide blends
                // some of it in (`Slide::shows`).
                let teeth =
                    (slide.between(&layers.teeth, x, y)).map(|pixel| pixel != Color32::TRANSPARENT);
                let shades = layers.band[i] > 0.0 && slide.shows(teeth);
                let unchanged = shaded.pixels[i] == unshaded.pixels[i];
                if !shades {
                    assert!(
                        unchanged,
                        "rise {rise}, pixel ({x}, {y}): out of the band or clear of the teeth, and shaded"
                    );
                } else {
                    // The other half: a pixel the code shades takes the
                    // maw's color toward the throat's dark by the band, so it
                    // changes, unless the maw there already is the dark, or
                    // the band is under half a step of a channel from it: a
                    // color rounds to whole steps. This is the maw's layer,
                    // as the picture may also hide a change, by how much of
                    // the maw the sand lets show and by the rim's light.
                    let worm = slide.pixel(&layers.worm, x, y);
                    let dark = layers.throat.gamma_multiply(f32::from(worm.a()) / 255.0);
                    let farthest = (worm.to_array().into_iter().zip(dark.to_array()))
                        .map(|(now, deep)| f32::from(now.abs_diff(deep)) * layers.band[i])
                        .fold(0.0, f32::max);
                    assert!(
                        layers.sunk(&slide, x, y) != worm || farthest <= 0.5,
                        "rise {rise}, pixel ({x}, {y}): shaded by {}, and unchanged though the maw is {worm:?} and the dark {dark:?}",
                        layers.band[i]
                    );
                }
                differ += usize::from(!unchanged);
            }
            changed.insert(rise, differ);
        }
        // Where a tooth stands in the band it does change the picture, or the
        // test would pass on a shade that does nothing: at the night look's
        // rise and at 750, where the teeth meet the sky.
        let night = thousandths(crate::settings::Settings::default().night.kelvin);
        for rise in [night, 750] {
            assert!(changed[&rise] > 0, "rise {rise}: no pixel is shaded");
        }
        // And the teeth stand clear of the band while the maw is low.
        assert_eq!(changed[&25], 0, "rise 25");
    }

    #[test]
    fn the_inner_rings_are_lost_in_the_throats_dark_in_the_layer() {
        let scene = Palette::DARK.cover;
        let size = 544;
        let (worm, _) = maw_layers(size, &scene);
        let teeth = rings::teeth();
        // The brightest pixel in red of each ring's teeth, whose center is in
        // a tooth, keyed by how lost the ring is.
        let mut brightest = std::collections::BTreeMap::new();
        for (i, pixel) in worm.iter().enumerate() {
            if let Some(lost) = teeth((center(i, size) - HEAD) / SCALE) {
                let key = (lost * 1000.0).round() as u32;
                let best: &mut u8 = brightest.entry(key).or_default();
                *best = (*best).max(pixel.r());
            }
        }
        let reds: Vec<u8> = brightest.values().copied().collect();
        assert_eq!(reds.len(), 6, "the rings: {reds:?}");
        // The outer ring is the tooth's own color, and each ring in is
        // darker, down to a last that still differs from the throat.
        assert!(reds[0].abs_diff(scene.tooth.r()) <= 3, "{reds:?}");
        assert!(reds.windows(2).all(|pair| pair[1] < pair[0]), "{reds:?}");
        assert!(reds[5] > scene.throat.r(), "{reds:?}");
    }

    /// Whether the far dune is clear of the pixel at `q` by a couple of
    /// pixels: its picture holds no dune at all.
    fn clear_of_the_dune(q: Pos2, size: usize) -> bool {
        !DUNES[0].covers(q + egui::vec2(0.0, 2.0 * GRID / size as f32), 0.0)
    }

    /// Whether the far dune covers the pixel at `q` through and through,
    /// with the sun's glare and a pixel of it above.
    fn deep_in_the_dune(q: Pos2, size: usize) -> bool {
        DUNES[0].depth(q) >= GLARE + GRID / size as f32
    }

    /// The value of the `(key, value)` point whose key is nearest `at`.
    fn nearest(points: &[(f32, f32)], at: f32) -> f32 {
        points
            .iter()
            .min_by(|a, b| (a.0 - at).abs().total_cmp(&(b.0 - at).abs()))
            .map(|point| point.1)
            .expect("a point")
    }

    #[test]
    fn the_suns_light_on_the_rim_fades_inside_the_suns_edge() {
        let size = 272;
        let layers = Layers::new(size, Palette::DARK.cover);
        let sun = |i: usize| center(i, size).distance(SUN_CENTER);
        // Outside the sun's edge there is none, however near.
        let outside: Vec<usize> = (0..size * size).filter(|&i| sun(i) >= SUN).collect();
        assert!(outside.len() > size * size / 2);
        assert!(outside.iter().all(|&i| layers.sunlit[i] == 0.0));
        // Beyond the glare's reach in from the edge, all of it, clear of the
        // dune; and none deep under the dune, whichever side of the edge.
        let clear: Vec<usize> = (0..size * size)
            .filter(|&i| clear_of_the_dune(center(i, size), size))
            .collect();
        let inner: Vec<usize> = clear
            .iter()
            .copied()
            .filter(|&i| sun(i) <= SUN - GLARE)
            .collect();
        assert!(inner.len() > 100, "{} pixels", inner.len());
        assert!(inner.iter().all(|&i| layers.sunlit[i] == 1.0));
        let buried: Vec<usize> = (0..size * size)
            .filter(|&i| sun(i) < SUN && deep_in_the_dune(center(i, size), size))
            .collect();
        assert!(
            buried.len() > 50,
            "{} pixels of the sun are under the dune",
            buried.len()
        );
        assert!(
            buried.iter().all(|&i| layers.sunlit[i] == 0.0),
            "the dune hides the sun from the rim"
        );
        // In the glare's band it falls off toward the edge, to half a
        // glare's depth in, and to next to nothing at the edge.
        let mut band: Vec<(f32, f32)> = clear
            .iter()
            .filter(|&&i| (SUN - GLARE..SUN).contains(&sun(i)))
            .map(|&i| (sun(i), layers.sunlit[i]))
            .collect();
        band.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(band.len() > 100, "{} pixels", band.len());
        assert!(
            band.windows(2).all(|pair| pair[1].1 <= pair[0].1),
            "falls off outward"
        );
        let at_depth = |share: f32| nearest(&band, SUN - GLARE * share);
        let (half, edge) = (at_depth(0.5), at_depth(0.05));
        assert!((0.35..=0.65).contains(&half), "{half} halfway in");
        assert!(edge < 0.1, "{edge} at the edge");
        // The far dune hides it the same way: the light is all there at the
        // dune's crest, falls off over the same glare inside it, measured
        // across the slope, and is gone from there in. Not in one pixel, the
        // width of the dune's antialiased edge.
        // The sun's own share of the light is divided out, so that the dune's
        // is seen alone. The sun fills so little of the dune that this is
        // told only up to most of a glare in.
        let mut inside: Vec<(f32, f32)> = (0..size * size)
            .filter_map(|i| {
                let depth = DUNES[0].depth(center(i, size));
                let share = smoothstep(((SUN - sun(i)) / GLARE).clamp(0.0, 1.0));
                (share > 0.05 && (0.0..=GLARE).contains(&depth))
                    .then(|| (depth, layers.sunlit[i] / share))
            })
            .collect();
        inside.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(inside.len() > 100, "{} pixels", inside.len());
        assert!(
            inside.windows(2).all(|pair| pair[1].1 <= pair[0].1 + 1e-4),
            "falls off inward"
        );
        let (crest, half, deep) = (
            nearest(&inside, 0.05 * GLARE),
            nearest(&inside, 0.5 * GLARE),
            nearest(&inside, 0.7 * GLARE),
        );
        assert!(crest > 0.9, "{crest} at the crest");
        assert!((0.35..=0.65).contains(&half), "{half} halfway in");
        assert!((0.1..=0.35).contains(&deep), "{deep} most of a glare in");
    }

    #[test]
    fn the_rims_light_shows_only_where_the_sun_is_behind_it() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let lit = Layers::new(size, scene);
        let mut dark = Layers::new(size, scene);
        dark.rim.fill(Color32::TRANSPARENT);
        // Every rise from the maw's stir to pure red, with the five stages
        // of the Temperature slider that the owner named.
        let mut rises: Vec<u16> = (0..=1000).step_by(25).collect();
        rises.extend(
            [2700, 2000, 1760, 1200, PURE_RED_KELVIN]
                .map(|kelvin| (rise_at(kelvin) * 1000.0).round() as u16),
        );
        let mut shown = Vec::new();
        for rise in rises {
            let (with, without) = (lit.compose(rise), dark.compose(rise));
            let lit_here: Vec<usize> = (0..size * size)
                .filter(|&i| with.pixels[i] != without.pixels[i])
                .collect();
            for &i in &lit_here {
                // Light is added: the pixel is never darker for it.
                let (a, b) = (with.pixels[i].to_array(), without.pixels[i].to_array());
                assert!(
                    a.iter().zip(b).all(|(a, b)| *a >= b),
                    "rise {rise}, pixel {i}"
                );
                // And it is added where the sun is: not against the sky outside
                // its edge, not against the dune that hides it.
                let q = center(i, size);
                assert!(
                    q.distance(SUN_CENTER) < SUN && lit.sunlit[i] > 0.0,
                    "rise {rise}: the rim is lit at {q:?}, where the sun is not behind it"
                );
            }
            shown.push((rise, lit_here.len()));
        }
        // It does light the rim at pure red, where the maw stands in front of
        // the sun, and at 2000 K, where it has risen to the sun's foot. Low
        // in the rise, up to 2700 K, the rim stands against sky and dunes
        // alone, and none of the light is there.
        let at = |rise: u16| shown.iter().find(|(r, _)| *r == rise).map(|(_, n)| *n);
        assert!(at(1000) > Some(100), "{shown:?}");
        assert!(
            at((rise_at(2000) * 1000.0).round() as u16) > Some(0),
            "{shown:?}"
        );
        let low = (rise_at(2700) * 1000.0).round() as u16;
        assert_eq!(at(low), Some(0), "{shown:?}");
        assert!(
            shown
                .iter()
                .filter(|(rise, _)| *rise <= 100)
                .all(|(_, n)| *n == 0),
            "{shown:?}"
        );
    }

    #[test]
    fn at_pure_red_the_rim_looks_as_it_does_lit_all_round() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let layers = Layers::new(size, scene);
        // The rim as it was before it was lit by the sun: in the lip, lit
        // wherever it is, in front of the sky and the dunes too.
        let mut all_round = Layers::new(size, scene);
        all_round.sunlit.fill(1.0);
        let apart = |rise: u16| {
            let (a, b) = (layers.compose(rise), all_round.compose(rise));
            a.pixels
                .iter()
                .zip(&b.pixels)
                .flat_map(|(a, b)| a.to_array().into_iter().zip(b.to_array()))
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or(0)
        };
        let pure = apart(1000);
        assert!(pure <= 1, "pure red differs by {pure} levels");
        // Anywhere else in the rise the two differ a good deal: the sun is
        // not behind the rim there.
        let midway = apart(150);
        assert!(midway >= 20, "midway they differ by {midway} levels");
    }

    #[test]
    fn at_pure_red_the_rim_meets_the_sun() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let image = Layers::new(size, scene).compose(1000);
        let pixel = GRID / size as f32;
        let top = HEAD - egui::vec2(0.0, LIP * SCALE);
        // Just past the top of the lip is the sun's own color, and just in
        // from it the lip is lighter than its plain color, in red.
        let outside = at(&image, top - egui::vec2(0.0, 2.0 * pixel));
        let inside = at(&image, top + egui::vec2(0.0, pixel));
        assert_eq!(outside, scene.sun);
        assert!(inside.r() > scene.worm.r(), "{inside:?} is not lit");
        assert!(red_contrast(inside, outside) >= 4.5);
    }

    /// What is behind the lip at a point.
    #[derive(Clone, Copy)]
    enum Behind {
        Sky,
        Sun,
    }

    /// The lowest contrast of the lip against the sky and against the sun,
    /// in red: None for what it does not meet.
    struct Contrast {
        sky: Option<f32>,
        sun: Option<f32>,
    }

    /// The lowest contrast of the lip, in red, against the sky and against
    /// the sun, in the picture at `rise`.
    ///
    /// It takes the pairs the picture draws: a pixel the lip covers whole,
    /// and one 2 pixels past it that the lip does not reach, or the sand's
    /// keep-out hides whole. The first lies 2 pixels in from the lip's round
    /// edge, or beside the keep-out's cut. What is behind the second, away
    /// from the soft edges of the sun and the dune, is the sky or the sun;
    /// the dunes are not paired with the lip.
    fn lowest_contrast(layers: &Layers, rise: u16) -> Contrast {
        let size = layers.size;
        let pixel = GRID / size as f32;
        let image = layers.compose(rise);
        let slide = Slide::new(size, rise);
        // The maw's layer is the two rows of it that slide to (x, y).
        let alphas = |x: usize, y: usize| slide.between(&layers.worm, x, y).map(|pixel| pixel.a());
        let whole =
            |x: usize, y: usize| alphas(x, y) == [255, 255] && layers.keep_out[y * size + x] == 0.0;
        let clear = |x: usize, y: usize| {
            let i = y * size + x;
            (alphas(x, y) == [0, 0] || layers.keep_out[i] == 1.0) && layers.front[i].a() == 0
        };
        let behind = |q: Pos2| {
            if DUNES[0].covers(q, 2.0 * pixel) {
                return None;
            }
            let from_sun = q.distance(SUN_CENTER);
            if from_sun < SUN - 2.0 * pixel {
                Some(Behind::Sun)
            } else if from_sun > SUN + 2.0 * pixel {
                Some(Behind::Sky)
            } else {
                None
            }
        };
        let pixel_at = |q: Pos2| {
            let (x, y) = ((q.x / pixel).floor(), (q.y / pixel).floor());
            (x >= 0.0 && y >= 0.0 && (x as usize) < size && (y as usize) < size)
                .then_some((x as usize, y as usize))
        };
        let head = slide.head();
        let (mut sky, mut sun): (Option<f32>, Option<f32>) = (None, None);
        let mut pair = |lip: (usize, usize), past: (usize, usize)| {
            if !whole(lip.0, lip.1) || !clear(past.0, past.1) {
                return;
            }
            // The lip, not the throat or the teeth in it: a pixel more than
            // 2 pixels out from the line between them.
            let across = center(lip.1 * size + lip.0, size).distance(head) / SCALE;
            if across < THROAT + 2.0 * pixel / SCALE {
                return;
            }
            let Some(behind) = behind(center(past.1 * size + past.0, size)) else {
                return;
            };
            let ratio = red_contrast(
                image.pixels[lip.1 * size + lip.0],
                image.pixels[past.1 * size + past.0],
            );
            let slot = match behind {
                Behind::Sky => &mut sky,
                Behind::Sun => &mut sun,
            };
            *slot = Some(slot.map_or(ratio, |lowest| lowest.min(ratio)));
        };
        // The lip's round edge: 2 pixels in and 2 out, along each radius.
        let radius = LIP * SCALE;
        for step in 0..2880 {
            let angle = step as f32 / 2880.0 * std::f32::consts::TAU;
            let along = egui::vec2(angle.cos(), angle.sin());
            if let (Some(lip), Some(past)) = (
                pixel_at(head + along * (radius - 2.0 * pixel)),
                pixel_at(head + along * (radius + 2.0 * pixel)),
            ) {
                pair(lip, past);
            }
        }
        // The cut: a pixel of the lip, and one 2 pixels from it that the
        // sand's keep-out hides.
        for y in 0..size {
            for x in 0..size {
                for (dx, dy) in [(0i32, 2i32), (2, 0), (-2, 0), (0, -2)] {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    if (0..size as i32).contains(&px)
                        && (0..size as i32).contains(&py)
                        && layers.keep_out[py as usize * size + px as usize] == 1.0
                    {
                        pair((x, y), (px as usize, py as usize));
                    }
                }
            }
        }
        Contrast { sky, sun }
    }

    #[test]
    fn the_lip_reaches_4_5_to_1_against_the_sky_and_the_sun_at_every_stage_of_the_rise() {
        let scene = Palette::DARK.cover;
        let size = 272;
        let layers = Layers::new(size, scene);
        // Just after the maw stirs, then every 50th thousandth of the rise,
        // and the five stages the owner named: 2700 K, 2000 K, Desert
        // (1760 K), 1200 K and pure red.
        let stages = [2700, 2000, 1760, 1200, PURE_RED_KELVIN]
            .map(|kelvin| (rise_at(kelvin) * 1000.0).round() as u16);
        let rises: Vec<u16> = [25]
            .into_iter()
            .chain((50..=1000).step_by(50))
            .chain(stages)
            .collect();
        let across = |layers: &Layers, rises: &[u16]| -> Vec<(u16, Contrast)> {
            rises
                .iter()
                .map(|&rise| (rise, lowest_contrast(layers, rise)))
                .collect()
        };
        let measured = across(&layers, &rises);
        for (rise, lowest) in &measured {
            for ratio in [lowest.sky, lowest.sun].into_iter().flatten() {
                assert!(ratio >= 4.5, "rise {rise}: the lip meets {ratio:.2}:1");
            }
        }
        // The probe sees both pairs across the rise, so the checks above ran:
        // the sky from the first stirs, the sun once the maw has come up to
        // it.
        let sky = measured.iter().filter(|(_, c)| c.sky.is_some()).count();
        let sun = measured.iter().filter(|(_, c)| c.sun.is_some()).count();
        assert!(
            sky >= 20 && sun >= 15,
            "met the sky in {sky} rises, the sun in {sun}"
        );
        // And it sees a rim lit wherever it is, as it was before the sun
        // lit it: against the sky, early in the rise, that falls under 4.5:1.
        let mut all_round = Layers::new(size, scene);
        all_round.sunlit.fill(1.0);
        let early: Vec<u16> = rises.iter().copied().filter(|&rise| rise <= 400).collect();
        let lowest = across(&all_round, &early)
            .into_iter()
            .filter_map(|(_, lowest)| lowest.sky)
            .fold(f32::MAX, f32::min);
        assert!(
            lowest < 4.5,
            "a rim lit all round meets the sky at {lowest:.2}:1"
        );
    }

    /// The scales a window meets on a Mac. The cover is `COVER_RECT`'s 136
    /// points at each, so its pictures are 136, 204 and 272 pixels square.
    const SCALES: [f32; 3] = [1.0, 1.5, 2.0];

    /// The size of the cover's pictures, in pixels, at `scale` pixels to the
    /// point.
    fn size_at(scale: f32) -> usize {
        pixel_size(COVER_RECT.width(), scale)
    }

    /// The layers for a cover `size` pixels square.
    fn layers_of(size: usize) -> Layers {
        Layers::new(size, Palette::DARK.cover)
    }

    /// The maw's rect in `COVER_RECT`, a cover `size` pixels square, at
    /// `rise` thousandths.
    fn maw_rect(place: &Place, size: usize, rise: u16) -> Rect {
        let down = drawn_down(size, rise).expect("stirred");
        place.rect(COVER_RECT, size, down)
    }

    #[test]
    fn the_maw_is_drawn_from_the_first_thousandth_of_the_rise() {
        for size in [136, 204, 272] {
            assert_eq!(drawn_down(size, 0), None, "{size} px");
            for rise in 1..=1000 {
                assert_eq!(
                    drawn_down(size, rise),
                    Some(sunk(size, rise)),
                    "{size} px, rise {rise}"
                );
            }
        }
    }

    #[test]
    fn the_maw_is_sunk_less_as_it_rises_and_by_the_pixels_of_its_cover() {
        // All the way out at 1000, and sunk the more the less it has risen.
        for size in [136, 204, 272] {
            assert_eq!(sunk(size, 1000), 0.0, "{size} px");
            for rise in 0..1000 {
                assert!(
                    sunk(size, rise) > sunk(size, rise + 1),
                    "{size} px, rise {rise}"
                );
            }
        }
        // The same stretch of the picture is twice the pixels in a cover of
        // twice the size.
        for rise in [0, 1, 250, 500, 999, 1000] {
            let (small, large) = (sunk(136, rise), sunk(272, rise));
            assert!((large - 2.0 * small).abs() < 1e-3, "rise {rise}");
        }
    }

    #[test]
    fn the_maw_stands_at_its_place_when_all_the_way_out() {
        for scale in SCALES {
            let size = size_at(scale);
            let place = layers_of(size).images().maw.place;
            let pixel = COVER_RECT.width() / size as f32;
            let at = vec2(place.at[0] as f32, place.at[1] as f32);
            let rect = maw_rect(&place, size, 1000);
            let off = rect.min - (COVER_RECT.min + at * pixel);
            assert!(off.length() < 1e-4, "{scale}x: {off:?} from its place");
        }
    }

    #[test]
    fn the_maw_slides_up_with_the_rise_and_never_across() {
        for scale in SCALES {
            let size = size_at(scale);
            let place = layers_of(size).images().maw.place;
            let pixel = COVER_RECT.width() / size as f32;
            for rise in 1..1000 {
                let low = maw_rect(&place, size, rise);
                let high = maw_rect(&place, size, rise + 1);
                let at = format!("{scale}x, rise {rise} to {}", rise + 1);
                assert_eq!(high.min.x, low.min.x, "{at}: across");
                assert!((high.size() - low.size()).length() < 1e-4, "{at}: size");
                // Up by what it is sunk the less, in the cover's pixels.
                let moved = low.min.y - high.min.y;
                let expected = (sunk(size, rise) - sunk(size, rise + 1)) * pixel;
                assert!(moved > 0.0, "{at}: moved {moved}");
                assert!(
                    (moved - expected).abs() < 1e-4,
                    "{at}: {moved}, not {expected}"
                );
            }
        }
    }

    /// Whether `n` is a whole number of device pixels.
    fn whole(n: f32) -> bool {
        (n - n.round()).abs() < 1e-3
    }

    #[test]
    fn the_maw_is_one_texel_to_a_pixel_and_across_on_whole_pixels() {
        for scale in SCALES {
            let size = size_at(scale);
            // The cover is drawn one texel to a device pixel too, and starts
            // on one.
            assert_eq!(COVER_RECT.width() * scale, size as f32, "{scale}x");
            assert!(whole(COVER_RECT.min.x * scale), "{scale}x: cover across");
            assert!(whole(COVER_RECT.min.y * scale), "{scale}x: cover down");
            let maw = layers_of(size).images().maw;
            let [width, height] = maw.image.size;
            for rise in [1, 250, 500, 750, 1000] {
                let rect = maw_rect(&maw.place, size, rise);
                let at = format!("{scale}x, rise {rise}");
                // A texel to a device pixel, so nothing is resampled across.
                assert!(
                    (rect.width() * scale - width as f32).abs() < 1e-3,
                    "{at}: width"
                );
                assert!(
                    (rect.height() * scale - height as f32).abs() < 1e-3,
                    "{at}: height"
                );
                let across = rect.min.x * scale;
                assert!(whole(across), "{at}: x is {across}");
            }
        }
    }

    #[test]
    fn the_gap_and_the_front_are_one_texel_to_a_pixel_on_whole_pixels_in_the_cover() {
        for scale in SCALES {
            let size = size_at(scale);
            let images = layers_of(size).images();
            for (name, crop) in [("gap", &images.gap), ("front", &images.front)] {
                let [width, height] = crop.image.size;
                let rect = crop.place.rect(COVER_RECT, size, 0.0);
                let at = format!("{scale}x, the {name}");
                // Neither slides, so both sit on whole pixels across and
                // down, a texel to a device pixel, and nothing is resampled.
                assert!(
                    (rect.width() * scale - width as f32).abs() < 1e-3,
                    "{at}: width"
                );
                assert!(
                    (rect.height() * scale - height as f32).abs() < 1e-3,
                    "{at}: height"
                );
                let (across, down) = (rect.min.x * scale, rect.min.y * scale);
                assert!(whole(across), "{at}: x is {across}");
                assert!(whole(down), "{at}: y is {down}");
                // And inside the cover: with no margin they need no clip.
                assert!(
                    COVER_RECT.contains_rect(rect.expand(-1e-3)),
                    "{at}: {rect:?}"
                );
            }
        }
    }

    /// `crop` of a layer `size` pixels square, `margin` clear pixels around
    /// its pixels: every pixel of the layer is inside it, it holds the
    /// layer's own pixels and clear ones beyond the cover, and its first and
    /// last pixels that are not clear are `margin` from its edges, and no
    /// more.
    fn assert_crop_is_the_layer_and_a_margin(
        what: &str,
        layer: &[Color32],
        size: usize,
        crop: &Crop,
        margin: usize,
    ) {
        let [width, height] = crop.image.size;
        assert_eq!(crop.place.extent, crop.image.size, "{what}");
        let [left, top] = crop.place.at;
        let inside =
            |x: i32, y: i32| (0..size as i32).contains(&x) && (0..size as i32).contains(&y);
        let (mut filled, mut first, mut last) = (0, [width, height], [0, 0]);

        // Every pixel of the layer is inside the crop.
        for (i, pixel) in layer.iter().enumerate() {
            if *pixel != Color32::TRANSPARENT {
                filled += 1;
                let (x, y) = ((i % size) as i32 - left, (i / size) as i32 - top);
                assert!(
                    (0..width as i32).contains(&x) && (0..height as i32).contains(&y),
                    "{what}: pixel {i} is outside the crop"
                );
            }
        }
        assert!(filled > 0, "{what}: the layer has pixels");

        // The crop is the layer's own pixels, and clear beyond the cover.
        for j in 0..height {
            for i in 0..width {
                let (x, y) = (left + i as i32, top + j as i32);
                let expected = if inside(x, y) {
                    layer[y as usize * size + x as usize]
                } else {
                    Color32::TRANSPARENT
                };
                let pixel = crop.image.pixels[j * width + i];
                assert_eq!(pixel, expected, "{what}: crop pixel ({i}, {j})");
                if pixel != Color32::TRANSPARENT {
                    first = [first[0].min(i), first[1].min(j)];
                    last = [last[0].max(i), last[1].max(j)];
                }
            }
        }

        // A clear margin all around, and no more.
        assert_eq!(first, [margin, margin], "{what}: the first row and column");
        assert_eq!(
            last,
            [width - 1 - margin, height - 1 - margin],
            "{what}: and the last"
        );
    }

    /// The gap's pixels over the whole cover: the back multiplied by the
    /// keep-out's alpha.
    fn gap_of(layers: &Layers) -> Vec<Color32> {
        (layers.back.iter())
            .zip(&layers.keep_out)
            .map(|(back, keep)| back.gamma_multiply(*keep))
            .collect()
    }

    /// `crop` put back in a cover `size` pixels square, clear around.
    fn uncropped(crop: &Crop, size: usize) -> Vec<Color32> {
        let mut cover = vec![Color32::TRANSPARENT; size * size];
        let [width, height] = crop.image.size;
        for j in 0..height {
            for i in 0..width {
                let (x, y) = (crop.place.at[0] + i as i32, crop.place.at[1] + j as i32);
                if (0..size as i32).contains(&x) && (0..size as i32).contains(&y) {
                    cover[y as usize * size + x as usize] = crop.image.pixels[j * width + i];
                }
            }
        }
        cover
    }

    #[test]
    fn the_maws_image_holds_every_pixel_of_the_worm_and_a_clear_margin() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            let what = format!("{size} px, the maw");
            assert_crop_is_the_layer_and_a_margin(&what, &layers.worm, size, &images.maw, MARGIN);
        }
    }

    #[test]
    fn the_gap_and_the_front_are_cropped_to_the_pixels_they_fill() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            let what = format!("{size} px, the gap");
            let gap = gap_of(&layers);
            assert_crop_is_the_layer_and_a_margin(&what, &gap, size, &images.gap, 0);
            let what = format!("{size} px, the front");
            assert_crop_is_the_layer_and_a_margin(&what, &layers.front, size, &images.front, 0);
            // Most of each layer is clear, the sky above the sand: the box
            // is less than half of the cover.
            for (name, crop) in [("gap", &images.gap), ("front", &images.front)] {
                let [width, height] = crop.image.size;
                assert!(
                    width * height * 2 < size * size,
                    "{size} px: the {name} is {width} by {height}"
                );
            }
        }
    }

    #[test]
    fn a_layer_with_nothing_in_it_crops_to_one_clear_pixel_and_its_margin() {
        let clear = vec![Color32::TRANSPARENT; 16];
        for margin in [0, 1, 3] {
            let crop = crop(&clear, 4, margin);
            let side = 1 + 2 * margin;
            assert_eq!(crop.image.size, [side, side], "margin {margin}");
            assert!(crop.image.pixels.iter().all(|p| *p == Color32::TRANSPARENT));
            assert_eq!(crop.place.at, [-(margin as i32); 2], "margin {margin}");
        }
    }

    /// The rim's light at (x, y) of a cover with the maw slid by `slide`,
    /// worked out from the layers alone: the rim's layer slid as the maw is,
    /// as far as the sun is behind the pixel. It is the rim's term of
    /// `Layers::maw_pixel`, before the sand hides it with the maw.
    fn rims_light_at(layers: &Layers, slide: &Slide, x: usize, y: usize) -> Color32 {
        let sunlit = layers.sunlit[y * layers.size + x];
        slide.pixel(&layers.rim, x, y).gamma_multiply(sunlit)
    }

    /// Every rise the tests of the rim's light look at in full: every `step`th
    /// from 0, and the first and last thousandths.
    fn rises_by(step: usize) -> Vec<u16> {
        let mut rises: Vec<u16> = (0..=1000).step_by(step).collect();
        rises.extend([1, 1000]);
        rises.sort_unstable();
        rises.dedup();
        rises
    }

    #[test]
    fn the_rims_light_is_the_layers_rim_term_in_its_box_and_clear_outside_it() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            let light = &images.rim;
            let [width, height] = light.place.extent;
            let [left, top] = light
                .place
                .at
                .map(|n| usize::try_from(n).expect("in the cover"));
            for rise in rises_by(25) {
                let image = light.compose(rise);
                assert_eq!(image.size, light.place.extent, "{size} px, rise {rise}");
                let slide = Slide::new(size, rise);
                let (mut lit, mut inside) = (0, 0);
                for y in 0..size {
                    for x in 0..size {
                        let wanted = rims_light_at(&layers, &slide, x, y);
                        let in_box =
                            (left..left + width).contains(&x) && (top..top + height).contains(&y);
                        if in_box {
                            inside += 1;
                            let got = image.pixels[(y - top) * width + (x - left)];
                            assert_eq!(got, wanted, "{size} px, rise {rise}: ({x}, {y})");
                        } else {
                            assert_eq!(
                                wanted,
                                Color32::TRANSPARENT,
                                "{size} px, rise {rise}: ({x}, {y}) is lit outside the box"
                            );
                        }
                        lit += usize::from(wanted != Color32::TRANSPARENT);
                    }
                }
                assert_eq!(inside, width * height, "{size} px: the box is in the cover");
                // At rise 0 the maw is under the sand and the compose is
                // clear; at pure red the sun is behind the rim.
                match rise {
                    0 => assert!(image.pixels.iter().all(|p| *p == Color32::TRANSPARENT)),
                    1000 => assert!(lit > 100, "{size} px: {lit} lit pixels at pure red"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn the_rims_box_holds_every_pixel_that_any_rise_lights_and_is_a_pixel_wider_at_most() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            // The pixels that any rise from the first thousandth lights, as
            // the corners of their bounds. Only a pixel of the rim's layer
            // that holds light can light a pixel of the cover: the one it
            // slides to, or the one below it.
            let (mut first, mut last) = ([size, size], [0, 0]);
            let holds_light: Vec<(usize, usize)> = (0..size * size)
                .filter(|&i| layers.rim[i] != Color32::TRANSPARENT)
                .map(|i| (i % size, i / size))
                .collect();
            for rise in 1..=1000 {
                let slide = Slide::new(size, rise);
                for &(x, row) in &holds_light {
                    for y in [row + slide.rows, row + slide.rows + 1] {
                        if y < size && rims_light_at(&layers, &slide, x, y) != Color32::TRANSPARENT
                        {
                            first = [first[0].min(x), first[1].min(y)];
                            last = [last[0].max(x), last[1].max(y)];
                        }
                    }
                }
            }
            assert!(first[0] <= last[0], "{size} px: some rise lights the rim");
            let place = images.rim.place;
            let (box_first, box_last) = (
                place.at.map(i64::from),
                [0, 1].map(|axis| i64::from(place.at[axis]) + place.extent[axis] as i64 - 1),
            );
            for axis in [0, 1] {
                let (low, high) = (first[axis] as i64, last[axis] as i64);
                assert!(
                    box_first[axis] <= low && high <= box_last[axis],
                    "{size} px, axis {axis}: the box {box_first:?}..{box_last:?} is short of {low}..{high}"
                );
                // The box is worked out without the blend's exact weights, so
                // it may be a pixel wider than what the thousand rises light,
                // and no more.
                assert!(
                    low - box_first[axis] <= 1 && box_last[axis] - high <= 1,
                    "{size} px, axis {axis}: the box {box_first:?}..{box_last:?} is wide of {low}..{high}"
                );
            }
        }
    }

    #[test]
    fn a_rim_with_no_light_to_show_has_a_one_pixel_box_that_composes_clear() {
        let size = 8;
        let clear = vec![Color32::TRANSPARENT; size * size];
        let light_at = |lit: &[usize]| -> Vec<Color32> {
            (0..size * size)
                .map(|i| match lit.contains(&i) {
                    true => Color32::from_rgb(200, 100, 50),
                    false => Color32::TRANSPARENT,
                })
                .collect()
        };
        let cases = [
            (
                "a rim layer with no light",
                clear.clone(),
                vec![1.0; size * size],
            ),
            (
                "no sun behind the rim",
                light_at(&[2 * size + 3]),
                vec![0.0; size * size],
            ),
            ("neither", clear.clone(), vec![0.0; size * size]),
            // The one pixel of the box holds light, and no sun is behind it.
            (
                "light at the corner only",
                light_at(&[0]),
                vec![0.0; size * size],
            ),
        ];
        let scene = Palette::DARK.cover;
        for (what, rim, sunlit) in cases {
            let light = RimLight::new(size, &rim, &sunlit);
            assert_eq!(light.place.at, [0, 0], "{what}");
            assert_eq!(light.place.extent, [1, 1], "{what}");
            for rise in rises_by(100) {
                let image = light.compose(rise);
                assert_eq!(image.size, [1, 1], "{what}, rise {rise}");
                assert_eq!(image.pixels, [Color32::TRANSPARENT], "{what}, rise {rise}");
            }

            // And the cover composes it again for a new rise without a
            // panic, and sends nothing: no row of it is lit at either rise.
            let ctx = egui::Context::default();
            let images = Images {
                back: ColorImage::new([size, size], clear.clone()),
                maw: crop(&clear, size, MARGIN),
                gap: crop(&clear, size, 0),
                front: crop(&clear, size, 0),
                rim: light,
                shade: Shade::new(size, &clear, &vec![0.0; size * size], scene.throat),
            };
            let textures = Textures::load(&ctx, scene, images, 1000);
            // The first frame's textures are taken, as the renderer would.
            ctx.tex_manager().write().take_delta().clear();
            let lit = textures.light_for(500);
            assert_eq!((lit.key, lit.rows.clone()), (500, 0..0), "{what}");
            drop(lit);
            let mut delta = ctx.tex_manager().write().take_delta();
            let (empty, sent) = (delta.is_empty(), format!("{:?}", delta.set.keys()));
            delta.clear();
            assert!(empty, "{what}: {sent}");
        }
    }

    #[test]
    fn a_shade_with_nothing_to_sink_has_a_one_pixel_box_that_composes_clear() {
        let size = 8;
        let scene = Palette::DARK.cover;
        let clear = vec![Color32::TRANSPARENT; size * size];
        let teeth_at = |holding: &[usize]| -> Vec<Color32> {
            (0..size * size)
                .map(|i| match holding.contains(&i) {
                    true => Color32::WHITE,
                    false => Color32::TRANSPARENT,
                })
                .collect()
        };
        let cases = [
            ("no tooth", clear.clone(), vec![1.0; size * size]),
            (
                "no band above the sand",
                teeth_at(&[2 * size + 3]),
                vec![0.0; size * size],
            ),
            ("neither", clear.clone(), vec![0.0; size * size]),
            // The one pixel of the box holds a tooth, and no band is above it.
            (
                "a tooth at the corner only",
                teeth_at(&[0]),
                vec![0.0; size * size],
            ),
            // A band too faint to change the throat's color by a step.
            (
                "a band too faint to show",
                teeth_at(&[2 * size + 3]),
                vec![1e-6; size * size],
            ),
        ];
        for (what, teeth, band) in cases {
            let shade = Shade::new(size, &teeth, &band, scene.throat);
            assert_eq!(shade.place.at, [0, 0], "{what}");
            assert_eq!(shade.place.extent, [1, 1], "{what}");
            for rise in rises_by(100) {
                let image = shade.compose(rise);
                assert_eq!(image.size, [1, 1], "{what}, rise {rise}");
                assert_eq!(image.pixels, [Color32::TRANSPARENT], "{what}, rise {rise}");
            }

            // And the cover composes it again for a new rise without a
            // panic, and sends nothing: no row of it holds shade at either
            // rise.
            let ctx = egui::Context::default();
            let images = Images {
                back: ColorImage::new([size, size], clear.clone()),
                maw: crop(&clear, size, MARGIN),
                gap: crop(&clear, size, 0),
                front: crop(&clear, size, 0),
                rim: RimLight::new(size, &clear, &vec![0.0; size * size]),
                shade,
            };
            let textures = Textures::load(&ctx, scene, images, 1000);
            // The first frame's textures are taken, as the renderer would.
            ctx.tex_manager().write().take_delta().clear();
            let held = textures.shade_for(500);
            let key = textures.teeth.key(500);
            assert_eq!((held.key, held.rows.clone()), (key, 0..0), "{what}");
            drop(held);
            let mut delta = ctx.tex_manager().write().take_delta();
            let (empty, sent) = (delta.is_empty(), format!("{:?}", delta.set.keys()));
            delta.clear();
            assert!(empty, "{what}: {sent}");
        }
    }

    /// The rim's light's boxes at each size. A scene that moves the box fails
    /// this and not the test above.
    #[test]
    fn the_rims_box_is_the_one_the_energy_baseline_gives() {
        for (size, extent, at) in [
            (136, [66, 52], [35, 54]),
            (204, [100, 79], [52, 81]),
            (272, [134, 105], [69, 108]),
        ] {
            let place = layers_of(size).images().rim.place;
            assert_eq!((place.extent, place.at), (extent, at), "{size} px");
        }
    }

    /// The shade at the sand at (x, y) of a cover with the maw slid by
    /// `slide`, worked out from the layers alone: the throat's color as deep
    /// as the band is, wherever the pixel the teeth's layer slides to holds
    /// any of a tooth, else clear. It is the shade's term of the oracle's
    /// picture, before the maw's alpha: the picture lays it over the maw.
    fn shade_at(layers: &Layers, slide: &Slide, x: usize, y: usize) -> Color32 {
        let teeth = (slide.between(&layers.teeth, x, y)).map(|pixel| pixel != Color32::TRANSPARENT);
        if slide.shows(teeth) {
            layers
                .throat
                .gamma_multiply(layers.band[y * layers.size + x])
        } else {
            Color32::TRANSPARENT
        }
    }

    /// The shade's boxes at each size. A scene that moves the box fails this
    /// and not the tests below.
    #[test]
    fn the_shades_box_is_the_one_the_energy_baseline_gives() {
        for (size, extent, at) in [
            (136, [48, 25], [44, 95]),
            (204, [72, 38], [66, 142]),
            (272, [96, 49], [88, 190]),
        ] {
            let place = layers_of(size).images().shade.place;
            assert_eq!((place.extent, place.at), (extent, at), "{size} px");
        }
    }

    /// The median of `rows`: the middle one, or the two middle ones where
    /// they agree.
    fn median(mut rows: Vec<usize>) -> usize {
        rows.sort_unstable();
        let middle = rows.len() / 2;
        if rows.len().is_multiple_of(2) {
            assert_eq!(rows[middle - 1], rows[middle], "two medians");
        }
        rows[middle]
    }

    /// What a sweep of the Temperature from pure red down sends of the shade
    /// at 272 px.
    struct ShadeSweep {
        /// The new rises it met.
        new_rises: usize,
        /// The new rises that composed the shade.
        composes: usize,
        /// The rows of each partial set the shade sent, one for each that
        /// sent any.
        rows: Vec<usize>,
        /// The bytes of a row of the shade's box.
        row_bytes: u64,
    }

    impl ShadeSweep {
        /// The bytes of a set of `rows`.
        fn bytes(&self, rows: usize) -> u64 {
            self.row_bytes * rows as u64
        }

        /// The bytes of all the sets.
        fn bytes_in_all(&self) -> u64 {
            self.bytes(self.rows.iter().sum())
        }
    }

    /// The sweep, read off the cover's own textures: a cover loaded at pure
    /// red, then drawn at each rise from 999 down in steps of `step`
    /// thousandths while above 0 (the new rises: 999 for a step of 1, 199 for
    /// 5, 39 for 25), each time taking the textures the renderer would. The
    /// first frame's bytes are the second thing it gives.
    fn sweep_the_shade(step: usize) -> (ShadeSweep, u64) {
        let ctx = egui::Context::default();
        let images = layers_of(272).images();
        let width = images.shade.place.extent[0];
        let textures = Textures::load(&ctx, Palette::DARK.cover, images, 1000);
        let mut first = ctx.tex_manager().write().take_delta();
        let first_frame = (first.set.values().flatten())
            .map(|delta| 4 * delta.image.size().iter().product::<usize>() as u64)
            .sum();
        first.clear();
        let shade = textures.shade_for(1000).sprite.texture.id();
        let mut held = textures.teeth.key(1000);
        let mut sweep = ShadeSweep {
            new_rises: 0,
            composes: 0,
            rows: Vec::new(),
            row_bytes: 4 * width as u64,
        };
        for rise in (1..1000)
            .rev()
            .filter(|rise| ((1000 - rise) as usize).is_multiple_of(step))
        {
            sweep.new_rises += 1;
            let key = textures.shade_for(rise).key;
            let composed = key != held;
            sweep.composes += usize::from(composed);
            held = key;
            let mut delta = ctx.tex_manager().write().take_delta();
            assert!(delta.free.is_empty(), "rise {rise}");
            assert!(delta.set.keys().all(|id| *id == shade), "rise {rise}");
            let sets: Vec<_> = delta.set.values().flatten().collect();
            assert!(sets.len() <= 1, "rise {rise}: {} sets", sets.len());
            for set in &sets {
                assert!(composed, "rise {rise}: sent though it did not compose");
                assert_eq!(set.pos.map(|at| at[0]), Some(0), "rise {rise}");
                assert_eq!(set.image.size()[0], width, "rise {rise}");
                sweep.rows.push(set.image.size()[1]);
            }
            delta.clear();
        }
        (sweep, first_frame)
    }

    /// The sweep of the shade: at 272 px from pure red down, the new rises
    /// that compose it, those that send, the rows at the median and at the
    /// most, and the first frame's bytes. A change to the shade or its key
    /// moves them.
    #[test]
    fn the_shades_sweep_is_the_one_the_energy_baseline_gives() {
        // New rises, composes, sets sent, rows at the median and at the most.
        let cases = [
            (1, 999, 120, 102, 39, 45),
            (5, 199, 119, 102, 39, 45),
            (25, 39, 39, 34, 40, 45),
        ];
        for (step, new_rises, composes, sends, median_rows, most_rows) in cases {
            let (sweep, first_frame) = sweep_the_shade(step);
            let at = format!("steps of {step}");
            assert_eq!(sweep.new_rises, new_rises, "{at}");
            assert_eq!(sweep.composes, composes, "{at}: composes");
            assert_eq!(sweep.rows.len(), sends, "{at}: composes that send");
            assert_eq!(median(sweep.rows.clone()), median_rows, "{at}: median");
            assert_eq!(sweep.rows.iter().max(), Some(&most_rows), "{at}: most");
            // The first frame sends the six images whole, the shade's box
            // among them, before any new rise.
            assert_eq!(first_frame, 611_480, "{at}: the first frame's bytes");
            // The bytes of the sweep's sets.
            if step == 1 {
                assert_eq!(sweep.bytes(median_rows), 14_976);
                assert_eq!(sweep.bytes(most_rows), 17_280);
                assert_eq!(sweep.bytes_in_all(), 1_376_640);
            }
        }
        // The shade's whole box, which the first frame sends.
        let [width, height] = layers_of(272).images().shade.place.extent;
        assert_eq!(4 * width * height, 18_816);
    }

    /// The same sweep with the key left out, composing the shade at every new
    /// rise as the cover did before the key: what it sends beside the sweep
    /// above, and so the saving.
    #[test]
    fn the_sweep_with_the_key_left_out_sends_what_the_energy_baseline_gives_beside_it() {
        let shade = layers_of(272).images().shade;
        let width = shade.place.extent[0];
        let mut held = lit_rows(&shade.compose(1000));
        let mut rows = Vec::new();
        for rise in (1..1000).rev() {
            let now = lit_rows(&shade.compose(rise));
            let sent = spanning(held.clone(), now.clone());
            if !sent.is_empty() {
                rows.push(sent.len());
            }
            held = now;
        }
        assert_eq!(rows.len(), 847);
        assert_eq!(median(rows.clone()), 39);
        assert_eq!(rows.iter().max(), Some(&45));
        let bytes_in_all = (4 * width * rows.iter().sum::<usize>()) as u64;
        assert_eq!(bytes_in_all, 11_330_688);
        // The key takes the sweep's shade to about an eighth.
        let (keyed, _) = sweep_the_shade(1);
        assert!(
            8 * keyed.bytes_in_all() < bytes_in_all && bytes_in_all < 9 * keyed.bytes_in_all(),
            "{} B of {bytes_in_all} B",
            keyed.bytes_in_all()
        );
    }

    #[test]
    fn the_shade_is_the_layers_shade_term_in_its_box_and_clear_outside_it() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            let shade = &images.shade;
            let [width, height] = shade.place.extent;
            let [left, top] = shade
                .place
                .at
                .map(|n| usize::try_from(n).expect("in the cover"));
            for rise in rises_by(7) {
                let image = shade.compose(rise);
                assert_eq!(image.size, shade.place.extent, "{size} px, rise {rise}");
                let slide = Slide::new(size, rise);
                let (mut shaded, mut inside) = (0, 0);
                for y in 0..size {
                    for x in 0..size {
                        let wanted = shade_at(&layers, &slide, x, y);
                        let in_box =
                            (left..left + width).contains(&x) && (top..top + height).contains(&y);
                        if in_box {
                            inside += 1;
                            let got = image.pixels[(y - top) * width + (x - left)];
                            assert_eq!(got, wanted, "{size} px, rise {rise}: ({x}, {y})");
                        } else {
                            assert_eq!(
                                wanted,
                                Color32::TRANSPARENT,
                                "{size} px, rise {rise}: ({x}, {y}) is shaded outside the box"
                            );
                        }
                        shaded += usize::from(wanted != Color32::TRANSPARENT);
                    }
                }
                assert_eq!(inside, width * height, "{size} px: the box is in the cover");
                // At rise 0 the maw is under the sand and the compose is
                // clear; where the maw stands out, some teeth sink.
                match rise {
                    0 => assert!(image.pixels.iter().all(|p| *p == Color32::TRANSPARENT)),
                    700 => assert!(shaded > 50, "{size} px: {shaded} pixels at {rise}"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn the_shades_box_holds_every_pixel_that_any_rise_darkens_and_no_more() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            // The pixels that any rise from the first thousandth darkens, as
            // the corners of their bounds. Only a pixel in the band can be.
            let in_band: Vec<(usize, usize)> = (0..size * size)
                .filter(|&i| layers.band[i] > 0.0)
                .map(|i| (i % size, i / size))
                .collect();
            let (mut first, mut last) = ([size, size], [0, 0]);
            for rise in 1..=1000 {
                let slide = Slide::new(size, rise);
                for &(x, y) in &in_band {
                    if shade_at(&layers, &slide, x, y) != Color32::TRANSPARENT {
                        first = [first[0].min(x), first[1].min(y)];
                        last = [last[0].max(x), last[1].max(y)];
                    }
                }
            }
            assert!(first[0] <= last[0], "{size} px: some rise darkens a pixel");
            let place = images.shade.place;
            assert_eq!(
                place.at,
                first.map(|n| n as i32),
                "{size} px: the box's corner"
            );
            assert_eq!(
                place.extent,
                [0, 1].map(|axis| last[axis] + 1 - first[axis]),
                "{size} px: the box's extent"
            );
        }
    }

    /// Two rises with one key compose one picture, whatever the key is made
    /// of: so the cover may compose the shade again only for a new key. And
    /// the key is not so fine that it never holds.
    #[test]
    fn rises_with_one_key_compose_one_shade() {
        for size in [136, 204, 272] {
            let shade = layers_of(size).images().shade;
            // The first picture composed for each key met, and how many
            // rises have met it.
            let mut met: Vec<(ShadeKey, ColorImage, usize)> = Vec::new();
            for rise in 0..=1000 {
                let (key, image) = (shade.key(rise), shade.compose(rise));
                match met.iter_mut().find(|(held, ..)| *held == key) {
                    Some((_, first, rises)) => {
                        assert!(image == *first, "{size} px, rise {rise}: {key:?}");
                        *rises += 1;
                    }
                    None => met.push((key, image, 1)),
                }
            }
            // The maw under the sand, at rise 0, shades nothing, and no rise
            // above it has its key: the cover loaded in the day composes the
            // shade at the first rise above 0.
            let day = met.iter().find(|(key, ..)| *key == shade.key(0));
            let (_, clear, rises) = day.expect("a key for rise 0");
            assert!(lit_rows(clear).is_empty(), "{size} px: rise 0 is clear");
            assert_eq!(*rises, 1, "{size} px: a rise above 0 has rise 0's key");
            // The keys hold over several rises each, and the pictures they
            // give are not all clear: the test sees keys that matter.
            assert!(met.len() < 1001 / 4, "{size} px: {} keys", met.len());
            let shading = met
                .iter()
                .filter(|(_, image, _)| !lit_rows(image).is_empty());
            assert!(shading.count() > 10, "{size} px");
        }
    }

    /// The rises the Temperature slider draws the maw at, from pure red down
    /// to the first above 0, each with the first kelvin that gives it: the
    /// slider moves by whole kelvins, and several give one rise.
    fn slider_rises() -> Vec<(u16, u16)> {
        let mut rises: Vec<(u16, u16)> = Vec::new();
        for kelvin in PURE_RED_KELVIN..=DAY_KELVIN {
            let rise = thousandths(kelvin);
            if rise > 0 && rises.last().is_none_or(|&(_, last)| last != rise) {
                rises.push((kelvin, rise));
            }
        }
        rises
    }

    /// Whether a move of the maw from rise `from` to `rise` leaves a row
    /// between the first and the last that the shade holds at either that is
    /// the same in both pictures.
    fn a_row_of_the_span_is_unchanged(shade: &Shade, [from, rise]: [u16; 2]) -> bool {
        let (old, new) = (shade.compose(from), shade.compose(rise));
        let width = old.size[0];
        let row = |image: &ColorImage, y: usize| image.pixels[y * width..(y + 1) * width].to_vec();
        (rows_to_send_by(|rise| shade.compose(rise), [from, rise]))
            .any(|y| row(&old, y) == row(&new, y))
    }

    #[test]
    fn a_new_rise_with_the_shades_key_sends_the_rims_light_and_none_of_the_shade() {
        for scale in SCALES {
            let size = size_at(scale);
            let images = layers_of(size).images();
            let shade = &images.shade;
            let holds = |rise: u16| !lit_rows(&shade.compose(rise)).is_empty();
            let rises = slider_rises();
            // Two rises the slider meets one after the other, where the shade
            // is out, with one key and so one picture; and the next rise
            // after them with another key.
            let at = (rises.windows(3)).position(|three| {
                let [(_, a), (_, b), (_, c)] = [three[0], three[1], three[2]];
                holds(a) && shade.key(a) == shade.key(b) && shade.key(b) != shade.key(c)
            });
            let &[(from_kelvin, from), (kelvin, rise), (next_kelvin, next)] =
                &rises[at.expect("a rise before a change of key")..][..3]
            else {
                unreachable!("three rises")
            };
            let mut window = Headless::new(scale);
            window.night(from_kelvin);
            let first = window.frame();
            let [.., rim, shade_id] = ids_of(&first, &images, from);

            // The new rise composes the rim's light, and none of the shade
            // is set; the shade is still drawn, and as it was.
            window.night(kelvin);
            let frame = window.frame();
            let at = format!("{scale}x, rise {from} to {rise}");
            assert_the_rims_rows(&frame, [rim, shade_id], &images, [from, rise]);
            assert!(
                frame.cover_textures().iter().all(|(id, _)| *id != shade_id),
                "{at}: the shade is set"
            );
            assert!(shade.key(from) == shade.key(rise) && from != rise, "{at}");
            assert_eq!(frame.cover_meshes().len(), 6, "{at}");

            // The rise after, with another key, sends the shade's rows from
            // the first that holds it at the last picture composed, which the
            // one before holds still, to the last: one partial set.
            window.night(next_kelvin);
            let frame = window.frame();
            assert_the_shades_rows(&frame, [rim, shade_id], &images, [rise, next]);
            let sets = frame.cover_textures();
            assert_eq!(
                sets.iter().filter(|(id, _)| *id == shade_id).count(),
                1,
                "{scale}x, rise {rise} to {next}: one partial set of the shade"
            );
        }
    }

    #[test]
    fn a_new_rise_that_moves_the_key_sends_the_span_of_rows_changed_or_not() {
        for scale in SCALES {
            let size = size_at(scale);
            let images = layers_of(size).images();
            let rises = slider_rises();
            // Two rises one after the other with different keys, whose
            // pictures keep a row of the span the same.
            let at = (rises.windows(2)).position(|pair| {
                let [(_, a), (_, b)] = [pair[0], pair[1]];
                images.shade.key(a) != images.shade.key(b)
                    && a_row_of_the_span_is_unchanged(&images.shade, [a, b])
            });
            let &[(from_kelvin, from), (kelvin, rise)] =
                &rises[at.expect("a move of the key")..][..2]
            else {
                unreachable!("two rises")
            };
            let mut window = Headless::new(scale);
            window.night(from_kelvin);
            let first = window.frame();
            let [.., rim, shade] = ids_of(&first, &images, from);
            window.night(kelvin);
            let frame = window.frame();
            // One partial set of the whole span, at its first row: the row
            // the pictures share is sent with the rest.
            assert_the_shades_rows(&frame, [rim, shade], &images, [from, rise]);
            let rows = rows_to_send_by(|rise| images.shade.compose(rise), [from, rise]);
            let sent: Vec<_> = (frame.cover_textures().into_iter())
                .filter(|(id, _)| *id == shade)
                .collect();
            assert_eq!(sent.len(), 1, "{scale}x");
            assert_eq!(sent[0].1.image.size()[1], rows.len(), "{scale}x");
            assert_eq!(sent[0].1.pos, Some([0, rows.start]), "{scale}x");
        }
    }

    #[test]
    fn a_cover_loaded_in_the_day_composes_the_shade_at_its_first_rise() {
        for scale in SCALES {
            let size = size_at(scale);
            let images = layers_of(size).images();
            assert!(
                (1..=1000).all(|rise| images.shade.key(rise) != images.shade.key(0)),
                "{scale}x: a rise above 0 has the key of the day"
            );
            let mut window = Headless::new(scale);
            window.night(DAY_KELVIN);
            let first = window.frame();
            let [.., rim, shade] = ids_of(&first, &images, 0);
            // Pure red is the most the maw can be out: the shade holds the
            // teeth there, and the whole of it is sent, as the picture it
            // replaces is clear.
            window.night(PURE_RED_KELVIN);
            let frame = window.frame();
            assert_the_shades_rows(&frame, [rim, shade], &images, [0, 1000]);
            let rows = rows_to_send_by(|rise| images.shade.compose(rise), [0, 1000]);
            assert!(!rows.is_empty(), "{scale}x: the shade holds some at 1000");
            assert!(
                (frame.cover_textures().iter()).any(|(id, _)| *id == shade),
                "{scale}x: the shade is sent"
            );
        }
    }

    #[test]
    fn the_corners_hold_every_point_and_no_points_give_the_pixel_at_the_corner() {
        assert_eq!(corners(std::iter::empty()), [[0, 0], [0, 0]]);
        assert_eq!(corners([[3, 5]]), [[3, 5], [3, 5]]);
        assert_eq!(corners([[7, 2], [3, 9], [5, 5]]), [[3, 2], [7, 9]]);
        // The top left and the bottom right come from different points.
        assert_eq!(corners([[0, 4], [6, 0]]), [[0, 0], [6, 4]]);
        // In any order.
        let points = [[4, 1], [2, 8], [9, 3], [2, 2]];
        let mut reversed = points;
        reversed.reverse();
        assert_eq!(corners(points), [[2, 1], [9, 8]]);
        assert_eq!(corners(reversed), corners(points));
        // A layer cropped by them: its box is theirs, and the margin around.
        let size = 10;
        let mut layer = vec![Color32::TRANSPARENT; size * size];
        for [x, y] in points {
            layer[y * size + x] = Color32::WHITE;
        }
        let place = crop(&layer, size, 1).place;
        assert_eq!(place.at, [1, 0]);
        assert_eq!(place.extent, [8 + 2, 8 + 2]);
    }

    #[test]
    fn the_lit_rows_run_from_the_first_with_light_to_the_last() {
        let width = 3;
        let image = |lit: &[usize], height: usize| {
            let mut pixels = vec![Color32::TRANSPARENT; width * height];
            for &row in lit {
                pixels[row * width + 1] = Color32::from_rgb(9, 0, 0);
            }
            ColorImage::new([width, height], pixels)
        };
        assert_eq!(lit_rows(&image(&[], 5)), 0..0);
        assert_eq!(lit_rows(&image(&[2], 5)), 2..3);
        assert_eq!(lit_rows(&image(&[1, 3], 5)), 1..4);
        assert_eq!(lit_rows(&image(&[0, 4], 5)), 0..5);
        // A pixel with no color but some alpha is not clear.
        let mut faint = image(&[], 5);
        faint.pixels[4 * width] = Color32::from_rgba_premultiplied(0, 0, 0, 1);
        assert_eq!(lit_rows(&faint), 4..5);

        assert_eq!(spanning(0..0, 0..0), 0..0);
        assert_eq!(spanning(0..0, 3..5), 3..5);
        assert_eq!(spanning(3..5, 0..0), 3..5);
        assert_eq!(spanning(3..5, 1..2), 1..5);
        assert_eq!(spanning(1..2, 6..9), 1..9);

        let tall = image(&[0, 1, 2, 3, 4], 5);
        let rows = rows_of(&tall, 1..4);
        assert_eq!(rows.size, [width, 3]);
        assert_eq!(rows.pixels, tall.pixels[width..4 * width]);
    }

    /// A Temperature drag, frame by frame, with the GPU's copy of the rim's
    /// light kept as the renderer keeps it: each partial set written over it.
    /// After every frame it is the light composed for the rise the maw was
    /// last drawn at, and each frame sent exactly the rows from the first
    /// that holds light at the old rise or the new to the last: none at the
    /// same rise, in the day, or between two rises that light no row.
    #[test]
    fn the_rows_a_drag_sends_of_the_rims_light_keep_it_as_compose_gives_it() {
        for scale in [1.0, 1.5, 2.0] {
            let mut window = Headless::new(scale);
            let size = pixel_size(COVER_RECT.width(), scale);
            let images = layers_of(size).images();
            let [width, height] = images.rim.place.extent;
            let mut drawn = night_rise(&window);
            let first = window.frame();
            let [.., rim, shade] = ids_of(&first, &images, drawn);
            let mut texture = images.rim.compose(drawn);
            let (mut sends, mut rows_sent, mut quiet) = (0, 0, 0);
            // To the day in steps of 70 K and back in steps of 130 K, so the
            // way back meets other rises.
            let there = (PURE_RED_KELVIN..=DAY_KELVIN).step_by(70);
            let back = (PURE_RED_KELVIN..=DAY_KELVIN).rev().step_by(130);
            for kelvin in there.chain(back) {
                window.night(kelvin);
                let frame = window.frame();
                let at = format!("{scale}x, {kelvin} K");
                assert!(frame.free.is_empty(), "{at}");
                let rise = thousandths(kelvin);
                let rows = if rise == 0 || rise == drawn {
                    0..0
                } else {
                    rows_to_send(&images.rim, [drawn, rise])
                };
                // The shade at the sand, which a new rise composes too, is
                // left out.
                let set: Vec<_> = (frame.cover_textures().into_iter())
                    .filter(|(id, _)| *id != shade)
                    .collect();
                if rows.is_empty() {
                    assert!(set.is_empty(), "{at}: sent {} sets", set.len());
                    quiet += usize::from(rise != 0 && rise != drawn);
                } else {
                    assert_eq!(set.len(), 1, "{at}");
                    let (id, delta) = set[0];
                    assert_eq!(*id, rim, "{at}");
                    assert_eq!(delta.pos, Some([0, rows.start]), "{at}");
                    let ImageData::Color(image) = &delta.image;
                    assert_eq!(image.size, [width, rows.len()], "{at}");
                    texture.pixels[rows.start * width..rows.end * width]
                        .copy_from_slice(&image.pixels);
                    sends += 1;
                    rows_sent += rows.len();
                }
                if rise > 0 {
                    drawn = rise;
                }
                assert!(
                    texture == images.rim.compose(drawn),
                    "{at}: the GPU's light is not the one for rise {drawn}"
                );
            }
            // The drag sent the light many times, and always less than its
            // box; and some new rises sent nothing at all.
            assert!(sends > 10, "{scale}x: {sends}");
            assert!(rows_sent < sends * height, "{scale}x: {rows_sent} rows");
            assert!(quiet > 0, "{scale}x");
        }
    }

    /// A Temperature drag, frame by frame, with the GPU's copy of the shade
    /// at the sand kept as the renderer keeps it: each partial set written
    /// over it. After every frame it is the shade composed for the rise the
    /// maw was last drawn at, and each frame sent exactly the rows from the
    /// first that holds shade at the old rise or the new to the last: none at
    /// the same rise, in the day, between two rises with one key
    /// ([`Shade::key`]), or between two rises that shade no row.
    #[test]
    fn the_rows_a_drag_sends_of_the_shade_keep_it_as_compose_gives_it() {
        for scale in [1.0, 1.5, 2.0] {
            let mut window = Headless::new(scale);
            let size = pixel_size(COVER_RECT.width(), scale);
            let images = layers_of(size).images();
            let [width, height] = images.shade.place.extent;
            let mut drawn = night_rise(&window);
            let first = window.frame();
            let [.., shade] = ids_of(&first, &images, drawn);
            let mut texture = images.shade.compose(drawn);
            let (mut sends, mut rows_sent, mut quiet) = (0, 0, 0);
            // To the day in steps of 70 K and back in steps of 130 K, so the
            // way back meets other rises.
            let there = (PURE_RED_KELVIN..=DAY_KELVIN).step_by(70);
            let back = (PURE_RED_KELVIN..=DAY_KELVIN).rev().step_by(130);
            for kelvin in there.chain(back) {
                window.night(kelvin);
                let frame = window.frame();
                let at = format!("{scale}x, {kelvin} K");
                assert!(frame.free.is_empty(), "{at}");
                let rise = thousandths(kelvin);
                // A rise with the key of the one last drawn holds the shade
                // composed for that one, and composes none.
                let key = |rise: u16| images.shade.key(rise);
                let rows = if rise == 0 || key(rise) == key(drawn) {
                    0..0
                } else {
                    rows_to_send_by(|rise| images.shade.compose(rise), [drawn, rise])
                };
                // The rim's light, which a new rise composes too, is left
                // out.
                let set: Vec<_> = (frame.cover_textures().into_iter())
                    .filter(|(id, _)| *id == shade)
                    .collect();
                if rows.is_empty() {
                    assert!(set.is_empty(), "{at}: sent {} sets", set.len());
                    quiet += usize::from(rise != 0 && rise != drawn);
                } else {
                    assert_eq!(set.len(), 1, "{at}");
                    let (id, delta) = set[0];
                    assert_eq!(*id, shade, "{at}");
                    assert_eq!(delta.pos, Some([0, rows.start]), "{at}");
                    let ImageData::Color(image) = &delta.image;
                    assert_eq!(image.size, [width, rows.len()], "{at}");
                    texture.pixels[rows.start * width..rows.end * width]
                        .copy_from_slice(&image.pixels);
                    sends += 1;
                    rows_sent += rows.len();
                }
                if rise > 0 {
                    drawn = rise;
                }
                assert!(
                    texture == images.shade.compose(drawn),
                    "{at}: the GPU's shade is not the one for rise {drawn}"
                );
            }
            // The drag sent the shade many times, and always less than its
            // box; and some new rises sent none of it at all.
            assert!(sends > 10, "{scale}x: {sends}");
            assert!(rows_sent < sends * height, "{scale}x: {rows_sent} rows");
            assert!(quiet > 0, "{scale}x");
        }
    }

    /// Covers drawn by one pass of `ctx` for each of `passes`, at the
    /// rednesses it lists.
    fn draw_covers(ctx: &egui::Context, passes: &[&[f32]]) {
        for rednesses in passes {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                for &redness in *rednesses {
                    draw(ui, COVER_RECT, redness, &COVER_SCENE, Tint::NEUTRAL);
                }
            });
            output.textures_delta.clear();
        }
    }

    /// The rim's light is one texture: a second cover at another rise in the
    /// same pass would show the light composed last on both.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "in one pass")]
    fn a_debug_build_stops_a_second_cover_at_another_rise_in_one_pass() {
        draw_covers(&egui::Context::default(), &[&[1.0, 0.6]]);
    }

    #[test]
    fn one_rise_a_pass_draws_as_many_covers_as_it_likes() {
        let ctx = egui::Context::default();
        draw_covers(
            &ctx,
            &[
                // The same rise twice in a pass, one rise in each pass.
                &[1.0, 1.0],
                &[0.6, 0.6],
                &[0.8],
                // A cover in the day draws no light, beside one that does.
                &[0.0, 0.8],
                &[0.7, 0.0],
            ],
        );
    }

    #[test]
    fn the_back_is_moved_into_its_image_and_not_copied() {
        let layers = layers_of(136);
        let back = layers.back.as_ptr();
        let images = layers.images();
        assert_eq!(images.back.pixels.as_ptr(), back);
    }

    #[test]
    fn the_gap_is_the_back_where_the_keep_out_hides_the_maw() {
        for size in [136, 204, 272] {
            let (layers, images) = layers_and_images(size);
            // The back is opaque, so the gap's alpha is the keep-out's.
            assert!(
                layers.back.iter().all(|pixel| pixel.a() == 255),
                "{size} px"
            );
            // The gap as it is on the cover: its box put back, clear around.
            let gap_image = uncropped(&images.gap, size);
            let (mut clear, mut partly, mut whole) = (0, 0, 0);
            for (i, keep) in layers.keep_out.iter().enumerate() {
                let gap = gap_image[i];
                assert_eq!(
                    gap,
                    layers.back[i].gamma_multiply(*keep),
                    "{size} px: pixel {i}"
                );
                assert_eq!(
                    gap.a(),
                    (keep * 255.0).round() as u8,
                    "{size} px: pixel {i}"
                );
                if *keep == 0.0 {
                    clear += 1;
                    assert_eq!(gap, Color32::TRANSPARENT, "{size} px: pixel {i}");
                } else if *keep == 1.0 {
                    whole += 1;
                    assert_eq!(gap, layers.back[i], "{size} px: pixel {i}");
                } else {
                    partly += 1;
                }
            }
            // The test sees every kind of pixel: the sky, the sand and its
            // antialiased edge.
            assert!(
                clear > 0 && partly > 0 && whole > 0,
                "{size} px: {clear}, {partly}, {whole}"
            );
            assert_eq!(images.back.size, [size, size]);
            assert_eq!(images.back.pixels, layers.back, "{size} px: back");
            assert_eq!(
                uncropped(&images.front, size),
                layers.front,
                "{size} px: front"
            );
        }
    }

    /// The ids of the textures that hold the `images` a frame set at `rise`:
    /// the back, the maw, the gap, the front, and the rim's light and the
    /// shade for `rise`, matched by their pixels.
    fn ids_of(frame: &Frame, images: &Images, rise: u16) -> [TextureId; 6] {
        let find = |wanted: &ColorImage| {
            let found: Vec<_> = (frame.cover_textures().into_iter())
                .filter(|(_, delta)| matches!(&delta.image, ImageData::Color(image) if **image == *wanted))
                .collect();
            assert_eq!(found.len(), 1, "one texture holds a picture of the cover");
            found[0].0
        };
        [
            &images.back,
            &images.maw.image,
            &images.gap.image,
            &images.front.image,
            &images.rim.compose(rise),
            &images.shade.compose(rise),
        ]
        .map(find)
    }

    /// The rise of the window's night look, in thousandths.
    fn night_rise(window: &Headless) -> u16 {
        thousandths(window.app.settings.night.kelvin)
    }

    /// The bounding box of a mesh's vertices.
    fn bounds(mesh: &Mesh) -> Rect {
        Rect::from_points(
            &mesh
                .vertices
                .iter()
                .map(|vertex| vertex.pos)
                .collect::<Vec<_>>(),
        )
    }

    /// The rise in thousandths the view gives the cover for the night look
    /// at `kelvin`.
    fn thousandths(kelvin: u16) -> u16 {
        rise_of(redness_of(kelvin))
    }

    fn assert_near(rect: Rect, wanted: Rect, what: &str) {
        let off = (rect.min - wanted.min)
            .length()
            .max((rect.max - wanted.max).length());
        assert!(off < 1e-3, "{what}: {rect:?}, not {wanted:?}");
    }

    #[test]
    fn egui_gets_six_textures_for_the_cover_that_the_maw_can_slide_on() {
        let mut window = Headless::new(2.0);
        let rise = night_rise(&window);
        let frame = window.frame();
        let textures = frame.cover_textures();
        assert_eq!(textures.len(), 6, "the cover's six, besides the font atlas");
        // Whole textures, filtered linearly (the maw sits between two rows of
        // its image), clamped to the edge (where its margin is clear) and
        // with no mipmaps.
        for (id, delta) in &textures {
            assert_eq!(delta.options, TextureOptions::LINEAR, "{id:?}");
            assert_eq!(delta.pos, None, "{id:?}: a whole texture");
        }
        // The back is a picture of the cover, and the others are their
        // boxes: each as small as its pixels allow.
        let images = layers_of(272).images();
        let mut sizes: Vec<_> = textures
            .iter()
            .map(|(_, delta)| delta.image.size())
            .collect();
        sizes.sort();
        let mut wanted = [
            images.back.size,
            images.maw.image.size,
            images.gap.image.size,
            images.front.image.size,
            images.rim.place.extent,
            images.shade.place.extent,
        ];
        wanted.sort();
        assert_eq!(sizes, wanted);
        assert_eq!(images.back.size, [272, 272]);
        for crop in [&images.maw, &images.gap, &images.front] {
            assert!(crop.image.size[1] < 272, "{:?}", crop.image.size);
        }
        assert!(images.rim.place.extent[1] < 272);
        assert!(images.shade.place.extent[1] < 272);
        // They are the layers' own images, one texture each.
        let ids = ids_of(&frame, &images, rise);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            6
        );
    }

    #[test]
    fn the_meshes_come_back_maw_rim_shade_gap_front_and_the_maw_is_clipped_to_the_cover() {
        let mut window = Headless::new(2.0);
        let rise = night_rise(&window);
        assert!(0 < rise, "the night look stirs the maw");
        let frame = window.frame();
        let images = layers_of(272).images();
        let [back, maw, gap, front, rim, shade] = ids_of(&frame, &images, rise);
        let meshes = frame.cover_meshes();
        let order: Vec<_> = meshes.iter().map(|(_, mesh)| mesh.texture_id).collect();
        assert_eq!(order, [back, maw, rim, shade, gap, front], "back to front");
        // Each is one rect, drawn whole: the back over the cover, the maw
        // where the rise puts it, and the rim's light, the shade, the gap and
        // the front over their boxes.
        let down = drawn_down(272, rise).expect("stirred");
        let wanted = [
            COVER_RECT,
            images.maw.place.rect(COVER_RECT, 272, down),
            images.rim.place.rect(COVER_RECT, 272, 0.0),
            images.shade.place.rect(COVER_RECT, 272, 0.0),
            images.gap.place.rect(COVER_RECT, 272, 0.0),
            images.front.place.rect(COVER_RECT, 272, 0.0),
        ];
        for (index, ((clip, mesh), wanted)) in meshes.iter().zip(wanted).enumerate() {
            assert_eq!(mesh.vertices.len(), 4, "mesh {index}");
            assert_eq!(mesh.indices.len(), 6, "mesh {index}");
            let uv = Rect::from_points(&mesh.vertices.iter().map(|v| v.uv).collect::<Vec<_>>());
            assert_near(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), "uv");
            assert_near(bounds(mesh), wanted, &format!("mesh {index}'s rect"));
            if index == 1 {
                assert_eq!(*clip, COVER_RECT, "the maw is clipped to the cover");
            }
        }
        // The look's tint is the vertex color of all six.
        let tints: Vec<_> = meshes
            .iter()
            .map(|(_, mesh)| mesh.vertices[0].color)
            .collect();
        assert!(tints.windows(2).all(|pair| pair[0] == pair[1]), "{tints:?}");
        let mut vertices = meshes.iter().flat_map(|(_, mesh)| &mesh.vertices);
        assert!(vertices.all(|vertex| vertex.color == tints[0]));
    }

    /// Asserts that the box of the cover that `pick` takes from the images
    /// (the texture's id from `ids_of`'s six, and its place) is drawn once,
    /// one texel to a device pixel on whole pixels, at each of the scales a
    /// Mac meets: in a window's first frame and in the frame after a new
    /// rise, where the maw's rect moves and this one does not.
    fn assert_one_texel_to_a_pixel(
        what: &str,
        pick: impl Fn([TextureId; 6], &Images) -> (TextureId, Place),
    ) {
        for scale in SCALES {
            let size = size_at(scale);
            let mut window = Headless::new(scale);
            let rise = night_rise(&window);
            assert!(0 < rise, "{scale}x: the night look stirs the maw");
            let first = window.frame();
            let images = layers_of(size).images();
            let (id, place) = pick(ids_of(&first, &images, rise), &images);
            let check = |frame: &Frame, when: &str| {
                let meshes = frame.cover_meshes();
                let drawn: Vec<_> = meshes.iter().filter(|(_, m)| m.texture_id == id).collect();
                let at = format!("{scale}x, {when}, the {what}");
                assert_eq!(drawn.len(), 1, "{at}: drawn once");
                let rect = bounds(&drawn[0].1);
                let [width, height] = place.extent;
                // A texel to a device pixel, and the rect starts and ends on
                // a whole one, across and down: nothing is resampled.
                assert!(
                    (rect.width() * scale - width as f32).abs() < 1e-3,
                    "{at}: width"
                );
                assert!(
                    (rect.height() * scale - height as f32).abs() < 1e-3,
                    "{at}: height"
                );
                for (name, n) in [
                    ("left", rect.min.x),
                    ("top", rect.min.y),
                    ("right", rect.max.x),
                    ("bottom", rect.max.y),
                ] {
                    assert!(whole(n * scale), "{at}: {name} is {}", n * scale);
                }
                // Where the box is in the cover, and inside it: no clip needed.
                assert_near(
                    rect,
                    place.rect(COVER_RECT, size, 0.0),
                    &format!("{at}: rect"),
                );
                assert!(
                    COVER_RECT.contains_rect(rect.expand(-1e-3)),
                    "{at}: {rect:?}"
                );
            };
            check(&first, "the first frame");
            window.night(PURE_RED_KELVIN);
            assert_ne!(thousandths(PURE_RED_KELVIN), rise, "{scale}x: a new rise");
            check(&window.frame(), "a new rise");
        }
    }

    #[test]
    fn the_rims_light_is_one_texel_to_a_pixel_on_whole_pixels_in_every_frame() {
        assert_one_texel_to_a_pixel("rim's light", |[.., rim, _], images| {
            (rim, images.rim.place)
        });
    }

    #[test]
    fn the_shade_is_one_texel_to_a_pixel_on_whole_pixels_in_every_frame() {
        assert_one_texel_to_a_pixel("shade", |[.., shade], images| (shade, images.shade.place));
    }

    /// The rows of `image` that hold some light, found pixel by pixel.
    fn rows_with_light(image: &ColorImage) -> Vec<usize> {
        let [width, height] = image.size;
        (0..height)
            .filter(|&y| (0..width).any(|x| image.pixels[y * width + x] != Color32::TRANSPARENT))
            .collect()
    }

    /// The rows a move of the maw from rise `from` to `rise` must send of the
    /// rim's light in `light`: from the first row that holds light at either
    /// rise to the last.
    fn rows_to_send(light: &RimLight, rises: [u16; 2]) -> Range<usize> {
        rows_to_send_by(|rise| light.compose(rise), rises)
    }

    /// The rows a move from rise `from` to `rise` must send of a box that
    /// `compose` gives at each rise: from the first row that holds anything
    /// at either rise to the last.
    fn rows_to_send_by(
        compose: impl Fn(u16) -> ColorImage,
        [from, rise]: [u16; 2],
    ) -> Range<usize> {
        let mut rows = rows_with_light(&compose(from));
        rows.extend(rows_with_light(&compose(rise)));
        match (rows.iter().min(), rows.iter().max()) {
            (Some(&first), Some(&last)) => first..last + 1,
            _ => 0..0,
        }
    }

    /// Asserts that `frame` set the rim's light, composed for `rise` over
    /// the light of `from`: one partial set to the texture `rim`, as wide as
    /// the rim's box, of the rows from the first that holds light at either
    /// rise to the last, and freed nothing. The shade at the sand, the
    /// texture `shade`, which a new rise composes too where it moves the
    /// shade's picture, is left out: [`assert_the_shades_rows`] holds it.
    fn assert_the_rims_rows(
        frame: &Frame,
        [rim, shade]: [TextureId; 2],
        images: &Images,
        [from, rise]: [u16; 2],
    ) {
        let at = format!("rise {from} to {rise}");
        assert!(frame.free.is_empty(), "{at}: {:?}", frame.free);
        let set: Vec<_> = (frame.cover_textures().into_iter())
            .filter(|(id, _)| *id != shade)
            .collect();
        assert_eq!(set.len(), 1, "{at}: one texture set");
        let (id, delta) = set[0];
        assert_eq!(*id, rim, "{at}: the rim's light");
        let rows = rows_to_send(&images.rim, [from, rise]);
        assert!(!rows.is_empty(), "{at}: some row holds light");
        assert_eq!(delta.pos, Some([0, rows.start]), "{at}: a partial set");
        assert_eq!(delta.options, TextureOptions::LINEAR, "{at}");
        let width = images.rim.place.extent[0];
        assert_eq!(delta.image.size(), [width, rows.len()], "{at}: the rows");
        let light = images.rim.compose(rise);
        let wanted = &light.pixels[rows.start * width..rows.end * width];
        assert!(
            matches!(&delta.image, ImageData::Color(image) if image.pixels == wanted),
            "{at}: the light for the rise"
        );
    }

    #[test]
    fn a_frame_at_the_same_rise_sends_nothing_and_neither_does_one_back_from_the_day() {
        let mut window = Headless::new(2.0);
        let images = layers_of(272).images();
        let (kelvin, rise) = (window.app.settings.night.kelvin, night_rise(&window));
        assert!(0 < rise, "the night look stirs the maw");
        let first = window.frame();
        let [back, maw, gap, front, rim, shade] = ids_of(&first, &images, rise);
        let order = |frame: &Frame| -> Vec<TextureId> {
            (frame.cover_meshes().iter())
                .map(|(_, mesh)| mesh.texture_id)
                .collect()
        };
        assert_eq!(order(&first), [back, maw, rim, shade, gap, front]);
        let sends_nothing = |frame: &Frame, what: &str| {
            assert!(frame.cover_textures().is_empty(), "{what}: sets a texture");
            assert!(frame.free.is_empty(), "{what}: frees {:?}", frame.free);
        };

        // The same rise, again and again: nothing is sent, and the cover is
        // drawn as before, the rim's light with it.
        for again in 0..3 {
            let frame = window.frame();
            sends_nothing(&frame, &format!("the same rise, frame {again}"));
            assert_eq!(order(&frame), [back, maw, rim, shade, gap, front]);
        }

        // The day: the maw is not drawn, nor the rim's light, and nothing is
        // sent for it. It stays so in the frames after.
        window.night(DAY_KELVIN);
        assert_eq!(thousandths(DAY_KELVIN), 0);
        for again in 0..2 {
            let frame = window.frame();
            sends_nothing(&frame, &format!("the day, frame {again}"));
            assert_eq!(order(&frame), [back, front], "the day, frame {again}");
        }

        // Back from the day to the rise last composed: the light is still
        // the one for it, and nothing is sent.
        window.night(kelvin);
        let frame = window.frame();
        sends_nothing(&frame, "back at the first rise");
        assert_eq!(order(&frame), [back, maw, rim, shade, gap, front]);

        // A new rise sends the light for it, and only that.
        let (other, other_rise) = (1500, thousandths(1500));
        assert!(0 < other_rise && other_rise != rise, "{other_rise}");
        window.night(other);
        let frame = window.frame();
        assert_the_rims_rows(&frame, [rim, shade], &images, [rise, other_rise]);
        assert_eq!(order(&frame), [back, maw, rim, shade, gap, front]);

        // The day, and then a rise that is not the one last composed: the
        // light is composed for it.
        window.night(DAY_KELVIN);
        sends_nothing(&window.frame(), "the day again");
        window.night(kelvin);
        let frame = window.frame();
        assert_the_rims_rows(&frame, [rim, shade], &images, [other_rise, rise]);
        // And the first rise is the one it now holds: a frame at the same
        // rise, once more, sends nothing.
        sends_nothing(&window.frame(), "the first rise, again");
    }

    #[test]
    fn a_new_rise_moves_the_maws_rect_and_sets_the_rims_light_over_the_old_in_place() {
        let mut window = Headless::new(2.0);
        let pixel = COVER_RECT.width() / 272.0;
        let kelvins = [PURE_RED_KELVIN, 1000, 1200, 1500, 2000];
        let rises = kelvins.map(thousandths);
        assert_eq!(rises[0], 1000, "pure red is all the way out");
        assert!(
            rises
                .windows(2)
                .all(|pair| pair[0] > pair[1] && pair[1] > 0),
            "{rises:?}"
        );

        window.night(kelvins[0]);
        let first = window.frame();
        let images = layers_of(272).images();
        let [.., rim, shade] = ids_of(&first, &images, rises[0]);
        let before = first.cover_meshes();
        assert_eq!(before.len(), 6);
        let mut from = rises[0];
        for (kelvin, rise) in kelvins.into_iter().zip(rises).skip(1) {
            window.night(kelvin);
            let frame = window.frame();
            // Nothing is freed, and the rim's light is set, composed for the
            // new rise and written over the old in place; the shade, which
            // this test leaves to `assert_the_shades_rows`, is set aside.
            assert_the_rims_rows(&frame, [rim, shade], &images, [from, rise]);
            from = rise;
            let after = frame.cover_meshes();
            assert_eq!(after.len(), 6, "{kelvin} K");
            for index in [0, 2, 3, 4, 5] {
                // The back, the rim's light, the shade, the gap and the front
                // stay where they are, but for the tint they are seen through.
                let (a, b) = (&before[index].1, &after[index].1);
                assert_eq!(a.indices, b.indices, "{kelvin} K, mesh {index}");
                let place = |mesh: &Mesh| {
                    mesh.vertices
                        .iter()
                        .map(|v| (v.pos, v.uv))
                        .collect::<Vec<_>>()
                };
                assert_eq!(place(a), place(b), "{kelvin} K, mesh {index}");
                assert_eq!(before[index].0, after[index].0, "{kelvin} K, mesh {index}");
            }
            // The maw's vertices differ only in y, by what it is sunk the more.
            let (a, b) = (&before[1].1, &after[1].1);
            assert_eq!(a.indices, b.indices, "{kelvin} K");
            let drop = (sunk(272, rise) - sunk(272, rises[0])) * pixel;
            assert!(drop > 0.0, "{kelvin} K");
            for (up, down) in a.vertices.iter().zip(&b.vertices) {
                assert_eq!(up.pos.x, down.pos.x, "{kelvin} K: across");
                assert_eq!(up.uv, down.uv, "{kelvin} K: uv");
                assert!(
                    (down.pos.y - up.pos.y - drop).abs() < 1e-3,
                    "{kelvin} K: {drop}"
                );
            }
            assert_eq!(before[1].0, after[1].0, "{kelvin} K: clip");
        }
    }

    /// Asserts that `frame` set the shade at the sand, composed for `rise`
    /// over the shade of `from`: one partial set to the texture `shade`, as
    /// wide as the shade's box, of the rows from the first that holds shade
    /// at either rise to the last; and no set of it where neither rise holds
    /// any. Nothing is set but the shade and the rim's light, the texture
    /// `rim`, and nothing is freed.
    fn assert_the_shades_rows(
        frame: &Frame,
        [rim, shade]: [TextureId; 2],
        images: &Images,
        [from, rise]: [u16; 2],
    ) {
        let at = format!("rise {from} to {rise}");
        assert!(frame.free.is_empty(), "{at}: {:?}", frame.free);
        let sets = frame.cover_textures();
        assert!(
            sets.iter().all(|(id, _)| *id == rim || *id == shade),
            "{at}: a texture besides the rim's light and the shade is set"
        );
        let mine: Vec<_> = sets.iter().filter(|(id, _)| *id == shade).collect();
        let rows = rows_to_send_by(|rise| images.shade.compose(rise), [from, rise]);
        if rows.is_empty() {
            assert!(
                mine.is_empty(),
                "{at}: sent the shade, though neither rise holds any"
            );
            return;
        }
        assert_eq!(mine.len(), 1, "{at}: one partial set of the shade");
        let (_, delta) = mine[0];
        assert_eq!(delta.pos, Some([0, rows.start]), "{at}: a partial set");
        assert_eq!(delta.options, TextureOptions::LINEAR, "{at}");
        let width = images.shade.place.extent[0];
        assert_eq!(delta.image.size(), [width, rows.len()], "{at}: the rows");
        let shaded = images.shade.compose(rise);
        let wanted = &shaded.pixels[rows.start * width..rows.end * width];
        assert!(
            matches!(&delta.image, ImageData::Color(image) if image.pixels == wanted),
            "{at}: the shade for the rise"
        );
    }

    #[test]
    fn a_new_rise_that_moves_the_shade_sends_its_rows_as_one_partial_set() {
        let mut window = Headless::new(2.0);
        let images = layers_of(272).images();
        // The kelvin nearest to a rise, going from pure red to the day.
        let kelvin_at = |wanted: u16| {
            (PURE_RED_KELVIN..=DAY_KELVIN)
                .min_by_key(|&kelvin| thousandths(kelvin).abs_diff(wanted))
                .expect("a kelvin")
        };
        // Down from pure red to where the teeth have left the band and the
        // shade holds nothing, and up again to pure red.
        let kelvins = [1000, 750, 500, 250, 80, 50, 1000].map(kelvin_at);
        let rises = kelvins.map(thousandths);
        let holds_shade = |rise: u16| !rows_with_light(&images.shade.compose(rise)).is_empty();
        assert!(
            rises[..4].iter().all(|&rise| holds_shade(rise)),
            "{rises:?}"
        );
        assert!(
            rises[4..6].iter().all(|&rise| !holds_shade(rise)),
            "{rises:?}"
        );
        assert!(
            rises[4..6].iter().all(|&rise| 0 < rise && rise < 100),
            "{rises:?}"
        );

        window.night(kelvins[0]);
        let first = window.frame();
        let [.., rim, shade] = ids_of(&first, &images, rises[0]);
        let mut from = rises[0];
        for (kelvin, rise) in kelvins.into_iter().zip(rises).skip(1) {
            window.night(kelvin);
            let frame = window.frame();
            assert_the_shades_rows(&frame, [rim, shade], &images, [from, rise]);
            from = rise;
        }
    }

    #[test]
    fn at_no_rise_there_is_no_maw_to_draw_and_no_gap_to_hide_it() {
        let mut window = Headless::new(2.0);
        window.night(DAY_KELVIN);
        assert_eq!(thousandths(DAY_KELVIN), 0);
        let frame = window.frame();
        let images = layers_of(272).images();
        // The maw's, the rim's, the shade's and the gap's textures are there,
        // and unused.
        let [back, maw, gap, front, rim, shade] = ids_of(&frame, &images, 0);
        let order: Vec<_> = frame
            .cover_meshes()
            .iter()
            .map(|(_, mesh)| mesh.texture_id)
            .collect();
        assert_eq!(order, [back, front]);
        assert!(!order.contains(&maw));
        assert!(!order.contains(&rim));
        assert!(!order.contains(&shade));
        assert!(!order.contains(&gap));
    }

    #[test]
    fn the_gap_is_drawn_with_the_maw_and_never_without_it() {
        let mut window = Headless::new(2.0);
        let rise = night_rise(&window);
        let [back, maw, gap, front, rim, shade] =
            ids_of(&window.frame(), &layers_of(272).images(), rise);
        let (mut with, mut without) = (0, 0);
        // The slider from pure red to the day, through where the maw stirs.
        for kelvin in (PURE_RED_KELVIN..=DAY_KELVIN).step_by(50) {
            window.night(kelvin);
            let frame = window.frame();
            let order: Vec<_> = frame
                .cover_meshes()
                .iter()
                .map(|(_, mesh)| mesh.texture_id)
                .collect();
            if thousandths(kelvin) > 0 {
                with += 1;
                assert_eq!(order, [back, maw, rim, shade, gap, front], "{kelvin} K");
            } else {
                without += 1;
                assert_eq!(order, [back, front], "{kelvin} K");
            }
        }
        assert!(with > 0 && without > 0, "{with} with, {without} without");
    }

    #[test]
    fn a_new_scale_frees_the_old_textures_and_sets_six_new() {
        let mut window = Headless::new(2.0);
        let rise = night_rise(&window);
        let first = window.frame();
        let old = ids_of(&first, &layers_of(272).images(), rise);

        window.scale = 1.5;
        let second = window.frame();
        let new = ids_of(&second, &layers_of(204).images(), rise);
        assert!(
            new.iter().all(|id| !old.contains(id)),
            "new textures: {new:?}"
        );
        assert_eq!(second.cover_textures().len(), 6);
        for id in old {
            assert!(second.free.contains(&id), "{id:?} is freed");
        }
        assert_eq!(
            second.free.len(),
            6,
            "only the cover's six: {:?}",
            second.free
        );

        // The cover is drawn from the new ones, back to front.
        let order: Vec<_> = second
            .cover_meshes()
            .iter()
            .map(|(_, mesh)| mesh.texture_id)
            .collect();
        let [back, maw, gap, front, rim, shade] = new;
        assert_eq!(order, [back, maw, rim, shade, gap, front]);

        // A second frame at the new scale sets and frees nothing.
        let third = window.frame();
        assert!(third.cover_textures().is_empty());
        assert!(third.free.is_empty(), "{:?}", third.free);
    }

    // The images built on a thread, ahead of the window's first frame. Every
    // test here has a slot of its own: the process's slot is `draw`'s, and
    // the tests above reach it from threads that run at the same time.

    /// How long a call that must not wait gets before a test calls it a wait.
    /// A call that waits for the thread would hang the test; this fails it.
    const NO_WAIT: Duration = Duration::from_secs(20);

    /// The key of the window's cover `size` pixels square.
    fn key(size: usize) -> Key {
        Key {
            size,
            scene: COVER_SCENE,
        }
    }

    /// A scene that is not the window's, by one color: the sun's, so that the
    /// back differs as well as the layers on it.
    fn another_scene() -> Cover {
        let scene = Cover {
            sun: Color32::from_rgb(1, 2, 3),
            ..Palette::DARK.cover
        };
        assert_ne!(scene, Palette::DARK.cover);
        scene
    }

    /// A slot whose thread has built the window's images for a display of
    /// `scale` pixels to the point, and put them in.
    fn ready_at(scale: f32) -> Slot {
        let slot = Slot::new();
        let thread = slot.ask(|| Some(scale)).expect("the thread starts");
        thread.join().expect("the build ends");
        assert!(!slot.is_empty(), "the build is in the slot");
        slot
    }

    /// The images of the cover 136 pixels square, the cheapest to build.
    fn small() -> Images {
        layers_of(136).images()
    }

    /// A build that waits for the sender's word, then hands out `images`.
    fn held_back(images: Images) -> (mpsc::Sender<()>, impl FnOnce(Key) -> Images + Send) {
        let (release, wait) = mpsc::channel();
        let build = move |_: Key| {
            let _ = wait.recv();
            images
        };
        (release, build)
    }

    /// `call` on a thread of its own, with a copy of the slot's handle, and
    /// its answer. It fails the test if the call waits for longer than
    /// `NO_WAIT`, where on this thread it would hang it.
    fn without_waiting<T: Send + 'static>(
        slot: &Slot,
        call: impl FnOnce(&Slot) -> T + Send + 'static,
    ) -> T {
        let (answer, answered) = mpsc::channel();
        let slot = slot.clone();
        std::thread::spawn(move || {
            let _ = answer.send(call(&slot));
        });
        answered
            .recv_timeout(NO_WAIT)
            .expect("the call waited for the build or the lock")
    }

    /// Whether `got` holds the pixels of `wanted`, in the same places. The
    /// images have no `PartialEq`, and a failed `assert_eq!` on a picture
    /// would print every pixel.
    fn assert_same(got: &Images, wanted: &Images, what: &str) {
        assert!(got.back == wanted.back, "{what}: the back differs");
        for (name, got, wanted) in [
            ("maw", &got.maw, &wanted.maw),
            ("gap", &got.gap, &wanted.gap),
            ("front", &got.front, &wanted.front),
        ] {
            assert!(got.place == wanted.place, "{what}: the {name}'s place");
            assert!(got.image == wanted.image, "{what}: the {name}'s pixels");
        }
        let (got, wanted) = (&got.rim, &wanted.rim);
        assert!(got.place == wanted.place, "{what}: the rim's place");
        assert!(got.from == wanted.from, "{what}: the rim's first row");
        assert!(got.layer == wanted.layer, "{what}: the rim's layer");
        assert!(got.sunlit == wanted.sunlit, "{what}: the rim's sun");
    }

    #[test]
    fn the_prebuilt_images_are_the_inline_ones_byte_for_byte() {
        for (scale, size) in [(1.0, 136), (1.5, 204), (2.0, 272)] {
            // The sizes the window meets, so that a drift of `COVER_RECT` or
            // of the rounding fails here.
            assert_eq!(pixel_size(COVER_RECT.width(), scale), size, "{scale}x");
            let slot = ready_at(scale);
            let prebuilt = slot
                .take(key(size))
                .unwrap_or_else(|| panic!("{size} px: the images are not there for their key"));
            assert_eq!(prebuilt.back.size, [size, size]);
            assert_same(&prebuilt, &layers_of(size).images(), &format!("{size} px"));
        }
    }

    #[test]
    fn a_hit_hands_out_the_prebuilt_images_and_does_not_build_them_again() {
        // A build that makes the images of another scene under the window's
        // key, so that its images can be told from an inline build's.
        let marked = Layers::new(136, another_scene()).images();
        let slot = Slot::new();
        let thread = slot
            .start_with(key(136), |key| {
                Layers::new(key.size, another_scene()).images()
            })
            .expect("the thread starts");
        thread.join().expect("the build ends");

        let got = images_from(&slot, key(136));
        assert_same(&got, &marked, "the slot's images");
        assert!(got.back != small().back, "not the inline images");
        assert!(slot.is_empty());
    }

    #[test]
    fn a_key_that_is_not_the_builds_falls_back_to_building_inline() {
        // Another size, as on an external display at 1x.
        let slot = ready_at(2.0);
        assert!(slot.take(key(136)).is_none());
        assert!(slot.is_empty(), "a miss empties the slot");

        let slot = ready_at(2.0);
        let inline = images_from(&slot, key(136));
        assert_eq!(inline.back.size, [136, 136]);
        assert_same(&inline, &small(), "136 px, built inline");
        assert!(slot.is_empty());

        // Another scene, at the same size.
        let slot = ready_at(2.0);
        let other = Key {
            size: 272,
            scene: another_scene(),
        };
        assert!(slot.take(other).is_none());
        assert!(slot.is_empty());

        // The miss took the images away: the window's own key finds nothing.
        let slot = ready_at(2.0);
        assert!(slot.take(key(136)).is_none());
        assert!(slot.take(key(272)).is_none());
    }

    #[test]
    fn a_slot_left_after_a_close_is_dropped() {
        let slot = ready_at(2.0);
        slot.close();
        assert!(slot.is_empty());
        assert!(slot.take(key(272)).is_none());
    }

    #[test]
    fn a_build_that_ends_after_a_close_is_dropped() {
        let slot = Slot::new();
        let (release, build) = held_back(small());
        let thread = slot.start_with(key(136), build).expect("the thread starts");
        slot.close();
        release.send(()).expect("the build waits");
        thread.join().expect("the build ends");
        assert!(slot.is_empty(), "the late images were not wanted");
        assert!(slot.take(key(136)).is_none());
    }

    #[test]
    fn a_build_that_ends_after_the_first_frame_built_inline_is_dropped() {
        let slot = Slot::new();
        let (release, build) = held_back(small());
        let thread = slot.start_with(key(136), build).expect("the thread starts");
        // The first frame finds nothing yet, and builds for itself.
        let inline = images_from(&slot, key(136));
        assert_same(&inline, &small(), "the first frame's own build");
        release.send(()).expect("the build waits");
        thread.join().expect("the build ends");
        assert!(slot.is_empty(), "the late images were not wanted");
    }

    #[test]
    fn a_build_that_ends_in_a_slot_with_a_newer_build_is_dropped() {
        let slot = Slot::new();
        let (release, first) = held_back(small());
        let old = slot.start_with(key(136), first).expect("the thread starts");

        // The second build is done before the first one ends.
        let second = layers_of(204).images();
        let new = slot
            .start_with(key(204), move |_| second)
            .expect("the thread starts");
        new.join().expect("the build ends");
        assert!(!slot.is_empty());

        release.send(()).expect("the build waits");
        old.join().expect("the build ends");
        let images = slot.take(key(204)).expect("the newest build's images");
        assert_eq!(images.back.size, [204, 204]);
    }

    #[test]
    fn starting_a_build_drops_what_the_slot_held() {
        let slot = ready_at(2.0);
        let (release, build) = held_back(small());
        let thread = slot.start_with(key(136), build).expect("the thread starts");
        assert!(slot.is_empty(), "the old images are not wanted any more");
        release.send(()).expect("the build waits");
        thread.join().expect("the build ends");
        assert!(slot.take(key(272)).is_none());
    }

    #[test]
    fn the_first_frame_does_not_wait_for_a_build_under_way() {
        let slot = Slot::new();
        let (release, build) = held_back(small());
        let thread = slot.start_with(key(136), build).expect("the thread starts");

        // The thread holds no lock while it builds, and the first frame
        // answers with nothing; it does not wait for the build, which is
        // still blocked.
        let answer = without_waiting(&slot, |slot| slot.take(key(136)));
        assert!(answer.is_none());
        assert!(!thread.is_finished(), "the build was still under way");

        // The same for a close and for another build.
        without_waiting(&slot, |slot| slot.close());
        let (release_next, next) = held_back(small());
        let next = without_waiting(&slot, move |slot| slot.start_with(key(204), next))
            .expect("the thread starts");
        assert!(!thread.is_finished());

        release.send(()).expect("the build waits");
        release_next.send(()).expect("the build waits");
        thread.join().expect("the build ends");
        next.join().expect("the build ends");
        assert!(!slot.is_empty(), "the newest build is the one wanted");
    }

    #[test]
    fn a_busy_lock_makes_the_first_frame_and_the_close_miss_and_not_wait() {
        let slot = ready_at(2.0);
        let busy = slot.hold();
        assert!(without_waiting(&slot, |slot| slot.take(key(272))).is_none());
        without_waiting(&slot, |slot| slot.close());
        drop(busy);
    }

    #[test]
    fn a_build_that_ends_while_the_lock_is_busy_is_dropped_if_the_slot_moved_on() {
        for how in ["close", "take", "nothing"] {
            let slot = Slot::new();
            let (release, build) = held_back(small());
            let thread = slot.start_with(key(136), build).expect("the thread starts");

            // The build puts its images in, and stops there with the lock
            // held.
            let (there, go_on) = slot.stop_in_lock();
            release.send(()).expect("the build waits");
            there
                .recv_timeout(NO_WAIT)
                .expect("the build stops in the lock");
            assert!(slot.is_busy(), "{how}");

            // Meanwhile the window closes or takes the images, and misses,
            // or nothing happens.
            match how {
                "close" => without_waiting(&slot, |slot| slot.close()),
                "take" => assert!(
                    without_waiting(&slot, |slot| slot.take(key(136))).is_none(),
                    "a busy lock is a miss"
                ),
                _ => {}
            }
            go_on.send(()).expect("the build is stopped");
            thread.join().expect("the build ends");

            // The build saw the slot move on, and took its images out again:
            // only a build nothing has moved on from is kept.
            assert_eq!(slot.is_empty(), how != "nothing", "{how}");
        }
    }

    #[test]
    fn a_newer_builds_images_are_dropped_by_an_older_build_that_held_the_lock_at_a_miss() {
        for how in ["close", "take"] {
            // Build A runs, and the window closes.
            let slot = Slot::new();
            let (release_old, old_build) = held_back(small());
            let old = slot
                .start_with(key(136), old_build)
                .expect("the thread starts");
            slot.close();

            // A window is asked for again: build B starts, ends and is kept.
            let newer = small();
            let new = slot
                .start_with(key(204), move |_| newer)
                .expect("the thread starts");
            new.join().expect("the build ends");
            assert!(!slot.is_empty(), "{how}: B's images are in");

            // A ends, finds B's images still wanted, and stops with the lock
            // held.
            let (there, go_on) = slot.stop_in_lock();
            release_old.send(()).expect("the build waits");
            there.recv_timeout(NO_WAIT).expect("A stops in the lock");
            assert!(slot.is_busy(), "{how}");
            assert!(!old.is_finished(), "{how}: A is still under way");

            // The window closes, or a frame takes B's images, and misses.
            match how {
                "close" => without_waiting(&slot, |slot| slot.close()),
                _ => assert!(
                    without_waiting(&slot, |slot| slot.take(key(204))).is_none(),
                    "a busy lock is a miss"
                ),
            }
            go_on.send(()).expect("A is stopped");
            old.join().expect("the build ends");

            // A, the last to hold the lock, took B's images out: nothing
            // waits for them now.
            assert!(slot.is_empty(), "{how}: B's images were left at rest");
        }
    }

    #[test]
    fn the_build_is_for_the_main_screens_scale_and_else_for_the_last_size() {
        // The last window was on a display at 1x, and its cover needed 136
        // pixels.
        let slot = Slot::new();
        assert_eq!(slot.last_size(), None);
        assert!(slot.take(key(136)).is_none());
        assert_eq!(slot.last_size(), Some(136));

        // The next one opens on the main screen, at 2x: the build is for the
        // screen, not for the last window.
        let thread = slot.ask(|| Some(2.0)).expect("the thread starts");
        thread.join().expect("the build ends");
        let images = slot.take(key(272)).expect("the build was for 272 px");
        assert_eq!(images.back.size, [272, 272]);
        assert_eq!(slot.last_size(), Some(272));

        // No scale, as off the main thread, or one no display has: the build
        // is for the size the cover last needed.
        assert!(slot.take(key(136)).is_none());
        for scale in [None, Some(f32::NAN), Some(0.0), Some(4.5)] {
            let thread = slot.ask(|| scale).expect("the thread starts");
            thread.join().expect("the build ends");
            let images = slot
                .take(key(136))
                .unwrap_or_else(|| panic!("{scale:?}: the build was not for 136 px"));
            assert_eq!(images.back.size, [136, 136], "{scale:?}");
        }

        // None yet: the build is for the scale given.
        let slot = Slot::new();
        let thread = slot.ask(|| Some(1.5)).expect("the thread starts");
        thread.join().expect("the build ends");
        assert_eq!(slot.last_size(), None, "a build is not a window");
        let images = slot.take(key(204)).expect("the build was for 204 px");
        assert_eq!(images.back.size, [204, 204]);
        assert_eq!(slot.last_size(), Some(204));
    }

    #[test]
    fn no_build_starts_without_a_size() {
        for scale in [
            None,
            Some(f32::NAN),
            Some(f32::INFINITY),
            Some(0.0),
            Some(-1.0),
            Some(4.5),
            Some(100.0),
        ] {
            let slot = Slot::new();
            assert!(slot.ask(|| scale).is_none(), "{scale:?}");
            assert!(slot.is_empty(), "{scale:?}");
            assert_eq!(slot.last_size(), None, "{scale:?}");
        }
    }

    #[test]
    fn a_finished_build_is_stored_once() {
        let slot = ready_at(1.0);
        assert!(slot.take(key(136)).is_some());
        assert!(slot.take(key(136)).is_none());
        assert!(slot.is_empty());
    }

    /// `scenery`, the picture the app's icon is drawn from, against the
    /// cover's own layers.
    #[cfg(feature = "demo")]
    mod the_icons_scenery {
        use super::*;

        const SIZE: usize = 272;

        /// The index of the pixel that holds the point `q` of the grid.
        fn index_of(q: Pos2) -> usize {
            let along = |n: f32| (n / GRID * SIZE as f32) as usize;
            along(q.y) * SIZE + along(q.x)
        }

        /// The points of the grid that the cover and the scenery draw alike,
        /// away from every edge: the sky, the sun's center, the far dune's
        /// body and its slip face.
        const ALIKE: [(&str, Pos2); 4] = [
            ("the sky", pos2(1.0, 1.0)),
            ("the sun's center", SUN_CENTER),
            ("the far dune's body", pos2(11.0, 15.5)),
            ("the far dune's slip face", pos2(15.5, 16.5)),
        ];

        fn within_one(a: Color32, b: Color32) -> bool {
            a.to_array()
                .into_iter()
                .zip(b.to_array())
                .all(|(a, b)| a.abs_diff(b) <= 1)
        }

        #[test]
        fn it_draws_what_the_covers_back_does_where_nothing_edges() {
            let scene = Palette::DARK.cover;
            let layers = Layers::new(SIZE, scene);
            let picture = scenery(SIZE, &scene, 0.0);
            for (what, q) in ALIKE {
                // Neither the near dune nor the mound hides the point, so the
                // cover's back is what shows there.
                assert!(!in_front(q, 0.0), "{what} is under the sand in front");
                let (got, wanted) = (picture[index_of(q)], layers.back[index_of(q)]);
                assert!(
                    within_one(got, wanted),
                    "{what} at {q:?}: scenery gives {got:?}, the cover {wanted:?}"
                );
            }
            // The far dune's body and its slip face are in their own shades.
            assert!(within_one(picture[index_of(ALIKE[2].1)], scene.far));
            assert!(within_one(picture[index_of(ALIKE[3].1)], scene.lee));
        }

        /// The sun's rim fades over one pixel, as the cover's does: one blend
        /// per pixel center. Drawn through `layer` as well, which samples
        /// each pixel an edge crosses, the rim fades over about two pixels.
        #[test]
        fn its_sun_has_the_covers_rim() {
            let scene = Palette::DARK.cover;
            let layers = Layers::new(SIZE, scene);
            let picture = scenery(SIZE, &scene, 0.0);
            let scale = GRID / SIZE as f32;
            // The pixels the sun's edge crosses, clear of the dunes and the
            // sand, which the two draw by their own layers.
            let rim: Vec<usize> = (0..SIZE * SIZE)
                .filter(|&i| {
                    let q = center(i, SIZE);
                    let on = disc(q, SUN_CENTER, SUN, scale);
                    0.0 < on && on < 1.0 && !in_front(q, 1.0) && !DUNES[0].covers(q, 1.0)
                })
                .collect();
            assert!(rim.len() > 100, "only {} pixels on the rim", rim.len());
            for i in rim {
                assert_eq!(
                    picture[i],
                    layers.back[i],
                    "the rim pixel ({}, {})",
                    i % SIZE,
                    i / SIZE
                );
            }
        }

        #[test]
        fn it_leaves_out_the_worm_and_the_mound() {
            let scene = Palette::DARK.cover;
            let picture = scenery(SIZE, &scene, 0.0);
            // The cover's mound of sand in front of the maw stands above the
            // ground, clear of the near dune: the cover has the near dune's
            // color there, and the scenery the far dune's behind it.
            let mound = pos2(11.8, 16.9);
            assert!(in_front(mound, 0.0), "the cover's sand reaches {mound:?}");
            assert!(DUNES[0].covers(mound, 0.0) && !DUNES[1].covers(mound, 0.0));
            assert!(!DUNES[0].in_lee(mound), "{mound:?} is not on the slip face");
            assert_eq!(Layers::new(SIZE, scene).front[index_of(mound)], scene.near);
            assert!(within_one(picture[index_of(mound)], scene.far));
            for (what, color) in [
                ("the lip", scene.worm),
                ("a tooth", scene.tooth),
                ("the throat", scene.throat),
            ] {
                assert!(!picture.contains(&color), "{what} is in the scenery");
            }
        }

        #[test]
        fn the_drop_moves_the_suns_center_down_the_grid() {
            let scene = Palette::DARK.cover;
            let (cover, dropped) = (scenery(SIZE, &scene, 0.0), scenery(SIZE, &scene, 1.4));
            // The sun is 5.28 across its radius: at (9, 14.5) the cover's sun
            // (center 8.8) has ended and the dropped one (10.2) has not, and
            // at (9, 4.2) it is the reverse.
            let low = pos2(SUN_CENTER.x, 14.5);
            let high = pos2(SUN_CENTER.x, 4.2);
            for q in [low, high] {
                assert!(!in_front(q, 0.0), "{q:?} is under the sand in front");
                assert!(
                    !DUNES[0].covers(q, 0.0),
                    "{q:?} is under the far dune, which would hide the sun"
                );
            }
            for (what, got, wanted) in [
                (
                    "sky under the cover's sun",
                    cover[index_of(low)],
                    sky(&scene, low.y),
                ),
                (
                    "sun under the dropped one",
                    dropped[index_of(low)],
                    scene.sun,
                ),
                (
                    "the cover's sun at the top",
                    cover[index_of(high)],
                    scene.sun,
                ),
                (
                    "sky above the dropped one",
                    dropped[index_of(high)],
                    sky(&scene, high.y),
                ),
            ] {
                assert!(within_one(got, wanted), "{what}: {got:?}, not {wanted:?}");
            }
        }
    }

    /// The README's banner (`banner.rs`), which only a demo build draws: its
    /// picture at two pixels to the point is 1600 by 600, the cover's square
    /// the last 600 columns.
    #[cfg(feature = "demo")]
    mod the_banner {
        use std::sync::LazyLock;

        use super::*;
        use crate::ui::testing::banner_picture_2x;

        /// The banner's height, and the cover's side in it, in pixels.
        const SIDE: usize = 600;
        /// The banner's width, and the column where the cover's square starts.
        const WIDTH: usize = 1600;
        const LEFT: usize = WIDTH - SIDE;

        /// The cover's own layers at the banner's size, in the window's scene,
        /// for the one-pass oracle. Made once, since a cover this size takes a
        /// moment to draw in a test build.
        static LAYERS: LazyLock<Layers> = LazyLock::new(|| Layers::new(SIDE, COVER_SCENE));
        /// The same cover's six images, from the same layers: what the square
        /// must be made of.
        static IMAGES: LazyLock<Images> = LazyLock::new(|| LAYERS.clone().images());

        /// The banner's cover: its last `SIDE` columns.
        fn square(picture: &ColorImage) -> ColorImage {
            let [width, height] = picture.size;
            let pixels = (0..height)
                .flat_map(|y| picture.pixels[y * width + width - SIDE..(y + 1) * width].iter())
                .copied()
                .collect();
            ColorImage::new([SIDE, height], pixels)
        }

        /// The rise the window draws its cover at with the default settings,
        /// read off the maw's rect that its frame sends: the one rise whose rect
        /// it is. It does not come from `rise_of`, which the banner calls.
        fn the_windows_rise() -> u16 {
            let size = size_at(2.0);
            let images = layers_of(size).images();
            let mut window = Headless::new(2.0);
            let frame = window.frame();
            let meshes = frame.cover_meshes();
            // Back to front: the back, then the maw (`the_meshes_come_back_...`).
            let drawn = bounds(&meshes[1].1);
            let off = |rise: u16| {
                let down = drawn_down(size, rise).expect("stirred");
                let rect = images.maw.place.rect(COVER_RECT, size, down);
                (rect.min - drawn.min)
                    .length()
                    .max((rect.max - drawn.max).length())
            };
            let rise = (1..=1000)
                .min_by(|a, b| off(*a).total_cmp(&off(*b)))
                .expect("a rise");
            assert!(off(rise) < 1e-3, "no rise puts the maw at {drawn:?}");
            assert!(rise < 1000, "the default night look is not pure red");
            rise
        }

        /// The largest difference in any channel between two pixels.
        fn step(a: Color32, b: Color32) -> u8 {
            (a.to_array().iter().zip(b.to_array()))
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .expect("four channels")
        }

        #[test]
        fn the_banners_square_is_what_the_covers_one_pass_oracle_composes() {
            let picture = banner_picture_2x();
            assert_eq!(picture.size, [WIDTH, SIDE]);
            let square = square(&picture);
            let rise = the_windows_rise();
            let agrees = |picture: &ColorImage| is_tolerated(SIDE, distance(&square, picture));
            let composed = LAYERS.compose(rise);
            assert!(
                agrees(&composed),
                "{:?} (the largest difference, and the pixels off by more than 2)",
                distance(&square, &composed)
            );
            // Controls: it fails for the maw moved a pixel, and for the other
            // rises it could have been drawn at.
            for by in [[1, 0], [0, 1]] {
                let moved = blend_like_the_gpu(&with_the_maw_moved(&IMAGES, by), rise);
                assert!(!agrees(&moved), "the maw moved by {by:?}");
            }
            for other in [0, 500, 1000] {
                assert!(!agrees(&LAYERS.compose(other)), "rise {other}");
            }
        }

        /// The rows where the step between columns `join - 1` and `join` of
        /// `picture` is larger than a smooth picture's: more than 3 times the
        /// larger of the steps beside it, and 2 levels. A flat row lets 2 levels
        /// through, which the gap's antialiased edge rounds off, and an
        /// antialiased edge, which ramps up to twice as steep in the middle as
        /// at its ends, lets through the 3 times.
        fn rows_with_a_seam(picture: &ColorImage, join: usize) -> Vec<usize> {
            let at = |x: usize, y: usize| picture.pixels[y * WIDTH + x];
            (0..SIDE)
                .filter(|&y| {
                    let across = step(at(join - 1, y), at(join, y));
                    let beside = step(at(join - 2, y), at(join - 1, y))
                        .max(step(at(join, y), at(join + 1, y)));
                    u16::from(across) > 3 * u16::from(beside) + 2
                })
                .collect()
        }

        /// `picture` with its columns left of `join` changed by `change`, which
        /// takes a pixel and the one `by` rows above it.
        fn left_of_changed(
            picture: &ColorImage,
            join: usize,
            by: usize,
            change: impl Fn(Color32, Color32) -> Color32,
        ) -> ColorImage {
            let mut changed = picture.clone();
            for y in 0..SIDE {
                for x in 0..join {
                    let above = picture.pixels[y.saturating_sub(by) * WIDTH + x];
                    changed.pixels[y * WIDTH + x] = change(picture.pixels[y * WIDTH + x], above);
                }
            }
            changed
        }

        #[test]
        fn the_sky_and_the_dunes_run_on_left_of_the_square_with_no_seam() {
            let picture = banner_picture_2x();
            // At the square's left side, and at the stretch's own join, where
            // its second cover's width ends and its first begins: the stretch is
            // 1000 columns, 600 and then 400.
            for join in [LEFT, LEFT - SIDE] {
                assert_eq!(rows_with_a_seam(&picture, join), Vec::<usize>::new());
                // Controls: the same test finds the seam in a picture whose
                // dunes left of the join stand 2 rows lower, and in one whose
                // sky left of it is 3 levels greener.
                let lower = left_of_changed(&picture, join, 2, |_, above| above);
                assert!(!rows_with_a_seam(&lower, join).is_empty(), "lower, {join}");
                let greener = left_of_changed(&picture, join, 0, |pixel, _| {
                    Color32::from_rgb(pixel.r(), pixel.g().saturating_add(3), pixel.b())
                });
                assert!(
                    !rows_with_a_seam(&greener, join).is_empty(),
                    "greener, {join}"
                );
            }
        }

        #[test]
        fn the_sky_at_the_left_edge_is_the_skys_there_and_the_dunes_are_not_flat() {
            let picture = banner_picture_2x();
            let at = |x: usize, y: usize| picture.pixels[y * WIDTH + x];
            // The sky depends on the row alone: far from the dunes, the first
            // column is the square's neighbour's, row by row.
            let dunes_top = SIDE * 2 / 3;
            for y in 0..dunes_top {
                assert_eq!(at(0, y), at(LEFT - 1, y), "row {y}");
            }
            // Below the ground's first row the sand is the near sand, and above
            // it, in some rows, a far dune stands: no flat horizon.
            let ground = (GROUND / GRID * SIDE as f32).ceil() as usize;
            for y in ground + 2..SIDE {
                assert_eq!(at(0, y), COVER_SCENE.near, "ground, row {y}");
            }
            let dune_rows = (dunes_top..ground).filter(|&y| at(0, y) == COVER_SCENE.far);
            assert!(
                dune_rows.count() > 0,
                "no dune above the ground at column 0"
            );
        }
    }
}
