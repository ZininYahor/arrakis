//! The same seam as the GL painter, on Metal: egui_glow's shaders and state in MSL, drawing egui's
//! meshes into IOSurfaces of ours, which a sublayer of the window's view shows. No GL at all.

use std::collections::HashMap;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::time::Instant;

use ::winit_crate::window::{Window, WindowId};
use block2::RcBlock;
use dispatch2::DispatchData;
use egui::epaint::{ClippedPrimitive, Primitive, textures::TexturesDelta};
use egui::{
    Color32, ColorImage, Context, Event, Rect, TextureFilter, TextureId, TextureOptions,
    TextureWrapMode, UserData, ViewportId, ViewportIdSet,
};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSView;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFType};
use objc2_foundation::NSString;
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth,
};
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLGPUFamily,
    MTLIndexType, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType,
    MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor,
    MTLSamplerMinMagFilter, MTLSamplerState, MTLScissorRect, MTLSize, MTLStorageMode,
    MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{CALayer, CATransaction};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

pub use self::close::empty_closed_surface;
#[cfg(feature = "demo")]
pub use self::close::park_a_surface_for_a_test;
pub(crate) use self::spare::drop_spare;
use self::wait::{Frames, Handler, Stage};
#[cfg(feature = "demo")]
use crate::failure::FailAt;
use crate::failure::{self, Flag};
use crate::hold::{Hold, Waited};
use crate::{RenderState, RendererOptions, WgpuConfiguration, WgpuError};

mod close;
mod spare;
mod wait;

/// Why a step of the painter could not go on. It goes to [`Frames::fail`], and the painter then paints
/// nothing.
type Failed = String;
type Capture = (ViewportId, Vec<UserData>, ColorImage);
type Device = ProtocolObject<dyn MTLDevice>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Sampler = Retained<ProtocolObject<dyn MTLSamplerState>>;
type CommandBuffer = Retained<ProtocolObject<dyn MTLCommandBuffer>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// The shader's source, which the painter compiles when [`METALLIB`] will not serve.
const SHADER: &str = include_str!("shader.metal");

/// `shader.metal` compiled ahead by Xcode's compiler, so that a first window skips the compile from
/// source: `scripts/build-metallib.sh` writes it and its stamp. A slice and not an array typed by its
/// length, so that a file of another length stops a build only at the assert below, which names the
/// script. That stops every other build, as a wrong length in `src/precomputed.rs` does:
/// `cargo test --features demo` too, as it builds this crate outside `cfg(test)` for the app. Only
/// this crate's own tests, `cargo test -p egui-wgpu --features demo --lib`, build without the
/// assert and reach the tests' message.
const METALLIB: &[u8] = include_bytes!("shader.metallib");

// Every build but the tests' refuses a file that is not the stamp's length, and names the script.
#[cfg(not(test))]
#[expect(
    clippy::disallowed_macros,
    reason = "an assert in a const stops the build and never runs"
)]
const _: () = assert!(
    METALLIB.len() == crate::shader_stamp::BYTES,
    "src/shader.metallib is not the length its stamp gives. Rebuild both with \
     scripts/build-metallib.sh, which needs Xcode, then run `cargo test --features demo`."
);

/// What `ARRAKIS_METAL_FAIL=metallib` hands Metal in place of the file: bytes that are not a
/// `metallib`, which Metal refuses as it would a damaged file.
#[cfg(feature = "demo")]
const NOT_A_METALLIB: &[u8] = b"ARRAKIS_METAL_FAIL=metallib";

/// The frames whose geometry the GPU may be reading at once.
const SLOTS: usize = 3;

/// The least a geometry buffer holds.
const SMALLEST_BUFFER: usize = 64 * 1024;

/// What counts as a window at rest under `ARRAKIS_METAL_FAIL=rest`, here and in the drive that leaves
/// its window so.
#[cfg(feature = "demo")]
pub const REST: std::time::Duration = std::time::Duration::from_secs(1);

/// What the shaders read in the pixel stage.
#[repr(C)]
struct Pixel {
    height: f32,
    dither: f32,
}

/// A surface, and the texture that draws into it.
struct Target {
    surface: CFRetained<IOSurfaceRef>,
    texture: Texture,
}

struct Image {
    texture: Texture,
    sampler: Sampler,
    size: [usize; 2],
}

/// One frame's geometry, which the GPU reads until that frame's command buffer completes.
#[derive(Default)]
struct Slot {
    vertices: Option<Buffer>,
    indices: Option<Buffer>,
    /// The frame's command buffer. It goes only once a wait has seen it finish, so a failed wait leaves
    /// it here for [`Self::finished`] to read.
    last: Option<CommandBuffer>,
}

impl Slot {
    /// Waits for this slot's frame, if one is in flight, and keeps its handle. A wait that reaches its
    /// limit raises the flag (`wait`), and the caller then reads [`Frames::failed`] before it writes
    /// to what that frame reads, or lets go of the handle.
    fn settle(&self, frames: &Frames) {
        if let Some(last) = &self.last {
            frames.wait_for(last, Stage::Completed);
        }
    }

    /// Whether this slot's frame has reached a final status, read with no wait.
    fn finished(&self) -> bool {
        self.last
            .as_ref()
            .is_none_or(|last| Stage::Completed.reached_by(last.status()))
    }
}

/// A change to the layers that takes effect at once, with no implicit animation.
fn at_once(change: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    change();
    CATransaction::commit();
}

struct Gpu {
    device: Retained<Device>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Pipeline,
    /// Where `pipeline`'s library came from.
    shader: Shader,
}

/// The sublayer that shows the frames, and the window whose view it is in.
struct Shown {
    window: WindowId,
    layer: Retained<CALayer>,
}

pub struct Painter {
    context: Context,
    dithering: bool,
    gpu: Option<Gpu>,
    shown: Option<Shown>,
    /// The surface this frame draws into, until the layer is handed it. It comes from [`spare`], or is
    /// new.
    drawing: Option<Target>,
    /// The surface the layer shows. A burst's frames take turns on it and the spare: each sets the
    /// layer's contents to the other's pointer, so Core Animation notices with no help.
    showing: Option<Target>,
    /// `showing` is of a size before [`Self::size`], so [`Self::present`] gives it back to no spare.
    shows_old_size: bool,
    size: [usize; 2],
    images: HashMap<TextureId, Image>,
    samplers: HashMap<TextureOptions, Sampler>,
    slots: [Slot; SLOTS],
    frame: usize,
    capture_tx: mpsc::Sender<Capture>,
    capture_rx: mpsc::Receiver<Capture>,
    hold: Hold,
    frames: Arc<Frames>,
    /// What every frame's command buffer calls: one block for the painter's life.
    handler: Handler,
    /// `ARRAKIS_METAL_FAIL`, read as the painter starts.
    #[cfg(feature = "demo")]
    fail_at: Option<FailAt>,
    /// `ARRAKIS_METAL_TEAR=1`, read as the painter starts.
    #[cfg(feature = "demo")]
    tear: bool,
    /// When the last paint came, for `ARRAKIS_METAL_FAIL=rest` alone.
    #[cfg(feature = "demo")]
    last_paint: Option<Instant>,
}

/// A surface, or none when the system gives none.
fn new_surface(width: usize, height: usize) -> Option<CFRetained<IOSurfaceRef>> {
    // SAFETY: the keys are the framework's own constants, and the values are the CFNumbers that
    // `IOSurfaceCreate` documents for them.
    unsafe {
        let keys: [&CFType; 4] = [
            kIOSurfaceWidth.as_ref(),
            kIOSurfaceHeight.as_ref(),
            kIOSurfaceBytesPerElement.as_ref(),
            kIOSurfacePixelFormat.as_ref(),
        ];
        let w = CFNumber::new_i64(width as i64);
        let h = CFNumber::new_i64(height as i64);
        let b = CFNumber::new_i64(4);
        // 'BGRA'
        let f = CFNumber::new_i64(0x4247_5241);
        let values: [&CFType; 4] = [w.as_ref(), h.as_ref(), b.as_ref(), f.as_ref()];
        let dict = CFDictionary::<CFType, CFType>::from_slices(&keys, &values);
        IOSurfaceRef::new(dict.as_opaque())
    }
}

/// Where a library comes from.
#[derive(Clone, Copy, Debug)]
enum Shader {
    /// A `metallib`'s bytes: [`METALLIB`], or bytes that are not one, to fail on purpose.
    File(&'static [u8]),
    /// [`SHADER`], compiled at run time.
    Source,
}

/// The library, loaded from a file or compiled from the source. The one place a shader enters the
/// app.
fn shader_library(
    device: &Device,
    shader: Shader,
) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, Failed> {
    match shader {
        Shader::File(bytes) => {
            // Wraps the static bytes with no copy. A plain object made in this process, which is nil
            // only when an allocation fails, and that ends the process anyway.
            let data = DispatchData::from_static_bytes(bytes);
            device.newLibraryWithData_error(&data).map_err(|error| {
                format!("no library from the file: {}", error.localizedDescription())
            })
        }
        Shader::Source => device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADER), None)
            .map_err(|error| format!("no library: {}", error.localizedDescription())),
    }
}

/// How the warning that a file does not serve begins: `scripts/painter-fallback.sh` counts it, and
/// `tests/painter_fallback_script.rs` holds the two together.
pub const METALLIB_REFUSED: &str = "shader.metallib does not serve";

/// A `metallib` that painters start on, and what they learned of it. The first painter that it does
/// not serve says why, and every later one on it goes straight to the source: the file does not change
/// in a process, so it would fail, and say so, at every window again. The process has one
/// ([`process_file`]); a test keeps its own, as a file that fails on purpose would send every other
/// test's painter to the source.
pub(crate) struct ShaderFile {
    bytes: &'static [u8],
    /// Raised by the first painter that the file did not serve, and never lowered.
    refused: AtomicBool,
    /// Where the last painter that started on it took its library from, for a drive's report.
    last: AtomicU8,
}

impl ShaderFile {
    const NONE: u8 = 0;
    const FROM_FILE: u8 = 1;
    const FROM_SOURCE: u8 = 2;

    pub(crate) const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            refused: AtomicBool::new(false),
            last: AtomicU8::new(Self::NONE),
        }
    }

    /// The bytes to try, or none once the file failed a painter.
    fn to_try(&self) -> Option<&'static [u8]> {
        (!self.refused.load(Ordering::Relaxed)).then_some(self.bytes)
    }

    /// Notes that the file did not serve: true the first time only, which says why.
    fn refuse(&self) -> bool {
        !self.refused.swap(true, Ordering::Relaxed)
    }

    /// Notes where a painter's library came from: the [`Gpu`]'s own [`Shader`].
    fn note(&self, shader: Shader) {
        let from = match shader {
            Shader::File(_) => Self::FROM_FILE,
            Shader::Source => Self::FROM_SOURCE,
        };
        self.last.store(from, Ordering::Relaxed);
    }

    /// `file` or `source`: where the last painter that started on it took its library from, if one
    /// got that far.
    fn last(&self) -> Option<&'static str> {
        match self.last.load(Ordering::Relaxed) {
            Self::FROM_FILE => Some("file"),
            Self::FROM_SOURCE => Some("source"),
            _ => None,
        }
    }
}

/// The file of the app's painters, for the process's life.
fn process_file() -> &'static ShaderFile {
    static FILE: OnceLock<ShaderFile> = OnceLock::new();
    FILE.get_or_init(|| ShaderFile::new(the_file()))
}

/// [`METALLIB`], or under `ARRAKIS_METAL_FAIL=metallib` bytes that are not one.
fn the_file() -> &'static [u8] {
    #[cfg(feature = "demo")]
    if failure::fail_at() == Some(FailAt::Metallib) {
        return NOT_A_METALLIB;
    }
    METALLIB
}

/// Where the process's last Metal painter took its library from, for a drive's report: `file`
/// (`shader.metallib`) or `source`. None before a painter got that far.
pub fn shader_from() -> Option<&'static str> {
    process_file().last()
}

/// The pipeline from `file`, or else from the source: a nil library, a missing function or a pipeline
/// that fails on the file's library. The first painter that the file does not serve says so; every
/// later one goes straight to the source, and says nothing. A file that will not serve says nothing
/// of the GPU, so it does not go to glow; only a source that fails too is the painter's failure.
fn file_or_source(device: &Device, file: &ShaderFile) -> Result<(Pipeline, Shader), Failed> {
    if let Some(bytes) = file.to_try() {
        match pipeline(device, Shader::File(bytes)) {
            Ok(pipeline) => return Ok((pipeline, Shader::File(bytes))),
            Err(why) => {
                if file.refuse() {
                    log::warn!(
                        "{METALLIB_REFUSED} ({why}), so every painter of this process compiles the \
                         shader from its source"
                    );
                }
            }
        }
    }
    Ok((pipeline(device, Shader::Source)?, Shader::Source))
}

fn start_gpu(file: &ShaderFile) -> Result<Gpu, Failed> {
    #[cfg(feature = "demo")]
    if failure::fail_at() == Some(FailAt::Pipeline) {
        return Err("ARRAKIS_METAL_FAIL=pipeline".to_owned());
    }
    let device = MTLCreateSystemDefaultDevice().ok_or("no device")?;
    let queue = device.newCommandQueue().ok_or("no command queue")?;
    let (pipeline, shader) = file_or_source(&device, file)?;
    let gpu = Gpu {
        device,
        queue,
        pipeline,
        shader,
    };
    // What a drive reports is what the GPU holds.
    file.note(gpu.shader);
    Ok(gpu)
}

/// The render pipeline on `shader`'s library: egui_glow's blend, into BGRA.
fn pipeline(device: &Device, shader: Shader) -> Result<Pipeline, Failed> {
    let library = shader_library(device, shader)?;
    let vs = library
        .newFunctionWithName(&NSString::from_str("vs"))
        .ok_or("no vertex function")?;
    let fs = library
        .newFunctionWithName(&NSString::from_str("fs"))
        .ok_or("no fragment function")?;
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(Some(&vs));
    desc.setFragmentFunction(Some(&fs));
    // SAFETY: attachment 0 exists on every device.
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    // `egui_glow-0.36.2/src/painter.rs:313-324`.
    color.setBlendingEnabled(true);
    color.setRgbBlendOperation(MTLBlendOperation::Add);
    color.setAlphaBlendOperation(MTLBlendOperation::Add);
    color.setSourceRGBBlendFactor(MTLBlendFactor::One);
    color.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    color.setSourceAlphaBlendFactor(MTLBlendFactor::OneMinusDestinationAlpha);
    color.setDestinationAlphaBlendFactor(MTLBlendFactor::One);
    device
        .newRenderPipelineStateWithDescriptor_error(&desc)
        .map_err(|error| format!("no pipeline: {}", error.localizedDescription()))
}

/// The side of the largest texture this device takes. Metal has no call that tells it: Apple's Metal
/// Feature Set Tables give it by GPU family, 16384 from Apple3 and on Mac2, 8192 below.
fn largest_texture_side(device: &Device) -> usize {
    if device.supportsFamily(MTLGPUFamily::Apple3) || device.supportsFamily(MTLGPUFamily::Mac2) {
        16384
    } else {
        8192
    }
}

fn scissor(
    [width, height]: [usize; 2],
    pixels_per_point: f32,
    clip_rect: Rect,
) -> Option<MTLScissorRect> {
    // `egui_glow-0.36.2/src/painter.rs:764-796`, with Metal's origin at the top.
    let (width, height) = (width as i32, height as i32);
    let min_x = (pixels_per_point * clip_rect.min.x).round() as i32;
    let min_y = (pixels_per_point * clip_rect.min.y).round() as i32;
    let max_x = (pixels_per_point * clip_rect.max.x).round() as i32;
    let max_y = (pixels_per_point * clip_rect.max.y).round() as i32;
    let min_x = min_x.clamp(0, width);
    let min_y = min_y.clamp(0, height);
    let max_x = max_x.clamp(min_x, width);
    let max_y = max_y.clamp(min_y, height);
    (max_x > min_x && max_y > min_y).then_some(MTLScissorRect {
        x: min_x as usize,
        y: min_y as usize,
        width: (max_x - min_x) as usize,
        height: (max_y - min_y) as usize,
    })
}

/// Makes sure the buffer holds `bytes`, and returns its first byte. Grows by doubling. `None` when the
/// device gives no buffer of that size, and the buffer is then gone.
fn room(device: &Device, buffer: &mut Option<Buffer>, bytes: usize) -> Option<NonNull<c_void>> {
    let wanted = bytes.max(SMALLEST_BUFFER);
    if buffer.as_ref().is_none_or(|b| b.length() < wanted) {
        *buffer = device.newBufferWithLength_options(
            wanted.checked_next_power_of_two()?,
            MTLResourceOptions::StorageModeShared,
        );
    }
    Some(buffer.as_ref()?.contents())
}

/// A plain 2D, unmipped descriptor.
fn texture_descriptor(
    format: MTLPixelFormat,
    [width, height]: [usize; 2],
    usage: MTLTextureUsage,
) -> Retained<MTLTextureDescriptor> {
    // SAFETY: a nonzero size from a surface or an image, within the device's limit, and no mipmaps.
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, width, height, false,
        )
    };
    desc.setUsage(usage);
    desc.setStorageMode(MTLStorageMode::Shared);
    desc
}

impl Painter {
    pub async fn new(
        context: Context,
        _options: WgpuConfiguration,
        _transparent: bool,
        renderer: RendererOptions,
    ) -> Self {
        Self::start(context, renderer.dithering, failure::process_flag())
    }

    /// The GPU and an empty painter, with no window yet, on the process's file. With no GPU, `flag`
    /// goes up before the first pass, which eframe runs anyway, and the painter paints nothing.
    fn start(context: Context, dithering: bool, flag: &'static Flag) -> Self {
        Self::start_on(context, dithering, flag, process_file())
    }

    /// [`Self::start`] on `file`: a test's own, or the process's.
    fn start_on(context: Context, dithering: bool, flag: &'static Flag, file: &ShaderFile) -> Self {
        let (capture_tx, capture_rx) = mpsc::channel();
        let frames = Arc::new(Frames::new(flag));
        let handler = wait::handler(&frames);
        let gpu = start_gpu(file).map_or_else(
            |why| {
                frames.fail(&why);
                None
            },
            Some,
        );
        Self {
            context,
            dithering,
            gpu,
            shown: None,
            drawing: None,
            showing: None,
            shows_old_size: false,
            size: [0, 0],
            images: HashMap::new(),
            samplers: HashMap::new(),
            slots: Default::default(),
            frame: 0,
            capture_tx,
            capture_rx,
            hold: Hold::default(),
            frames,
            handler,
            #[cfg(feature = "demo")]
            fail_at: failure::fail_at(),
            #[cfg(feature = "demo")]
            tear: failure::tear(),
            #[cfg(feature = "demo")]
            last_paint: None,
        }
    }

    fn target(&self, [width, height]: [usize; 2]) -> Result<Target, Failed> {
        let gpu = self.gpu.as_ref().ok_or("no GPU")?;
        let surface = new_surface(width, height).ok_or("no surface")?;
        let desc = texture_descriptor(
            MTLPixelFormat::BGRA8Unorm,
            [width, height],
            MTLTextureUsage::RenderTarget,
        );
        let texture = gpu
            .device
            .newTextureWithDescriptor_iosurface_plane(&desc, &surface, 0)
            .ok_or("no texture over the surface")?;
        Ok(Target { surface, texture })
    }

    pub fn render_state(&self) -> Option<RenderState> {
        None
    }

    /// Whether `window` has its layer already: eframe calls `set_window` on every frame, so a
    /// comparison is all this may cost. Another window is a failure: the painter keeps its first
    /// window's layer, so the new one would stay blank. The app has one window and no
    /// `show_viewport`, and a second has to move the layer.
    fn shows_in(&self, window: WindowId) -> bool {
        let Some(shown) = &self.shown else {
            return false;
        };
        if shown.window != window {
            self.frames
                .fail("a second window on a painter that shows its first");
        }
        true
    }

    /// `Ok` on every path, so `eframe::Error::Wgpu` never comes: a failure goes to [`Frames::fail`].
    pub async fn set_window(
        &mut self,
        _viewport_id: ViewportId,
        window: Option<Arc<Window>>,
    ) -> Result<(), WgpuError> {
        let Some(window) = window else {
            // The window stays, as a close's does not: nothing else would take the layer off it.
            if let Some(Shown { layer, .. }) = self.let_go_of_window() {
                at_once(|| {
                    // SAFETY: nil clears a layer's contents.
                    unsafe { layer.setContents(None) };
                    layer.removeFromSuperlayer();
                });
            }
            return Ok(());
        };
        if self.frames.failed() || self.shows_in(window.id()) {
            return Ok(());
        }
        self.frames.set_window(Arc::downgrade(&window));
        if let Err(why) = self.show_in(&window) {
            self.frames.fail(&why);
        }
        Ok(())
    }

    /// Gives `window`'s view the sublayer that shows the frames.
    fn show_in(&mut self, window: &Window) -> Result<(), Failed> {
        #[cfg(feature = "demo")]
        if self.fail_at == Some(FailAt::Surface) {
            return Err("ARRAKIS_METAL_FAIL=surface".to_owned());
        }
        let handle = window
            .window_handle()
            .map_err(|error| format!("no window handle: {error}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("a window that is not AppKit's".to_owned());
        };
        // SAFETY: winit hands out a live NSView, and this runs on the main thread.
        let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
        view.setWantsLayer(true);
        let root = view.layer().ok_or("a view with no layer")?;
        let layer = CALayer::new();
        layer.setContentsScale(window.scale_factor());
        layer.setOpaque(true);
        at_once(|| {
            layer.setFrame(root.bounds());
            root.addSublayer(&layer);
        });
        self.shown = Some(Shown {
            window: window.id(),
            layer,
        });
        Ok(())
    }

    /// The device's own limit: `ui::cap_texture_side` in the app holds egui's atlas far below it.
    pub fn max_texture_side(&self) -> Option<usize> {
        let gpu = self.gpu.as_ref()?;
        Some(largest_texture_side(&gpu.device))
    }

    pub fn on_window_resize_state_change(&mut self, _id: ViewportId, _resizing: bool) {}

    pub fn on_window_resized(&mut self, _id: ViewportId, _w: NonZeroU32, _h: NonZeroU32) {}

    fn sampler(&mut self, options: TextureOptions) -> Result<Sampler, Failed> {
        if let Some(sampler) = self.samplers.get(&options) {
            return Ok(sampler.clone());
        }
        let device = &self.gpu.as_ref().ok_or("no GPU")?.device;
        // Where glow would make mipmaps, a texture that asks gets none. Nothing here asks:
        // `cover.rs` pins `TextureOptions::LINEAR`.
        if options.mipmap_mode.is_some() {
            log::warn!("no mipmaps on Metal, though these options ask: {options:?}");
        }
        let filter = |filter| match filter {
            TextureFilter::Nearest => MTLSamplerMinMagFilter::Nearest,
            TextureFilter::Linear => MTLSamplerMinMagFilter::Linear,
        };
        let wrap = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
            TextureWrapMode::Repeat => MTLSamplerAddressMode::Repeat,
            TextureWrapMode::MirroredRepeat => MTLSamplerAddressMode::MirrorRepeat,
        };
        let desc = MTLSamplerDescriptor::new();
        desc.setMagFilter(filter(options.magnification));
        desc.setMinFilter(filter(options.minification));
        desc.setSAddressMode(wrap);
        desc.setTAddressMode(wrap);
        let sampler = device
            .newSamplerStateWithDescriptor(&desc)
            .ok_or("no sampler")?;
        self.samplers.insert(options, sampler.clone());
        Ok(sampler)
    }

    /// The textures' updates. A write waits for the frames in flight, which may be reading. When one
    /// of those waits reaches its limit nothing is written.
    fn update_textures(&mut self, delta: &TexturesDelta) -> Result<(), Failed> {
        if !delta.set.is_empty() {
            let frames = &self.frames;
            self.slots.iter().for_each(|slot| slot.settle(frames));
            if frames.failed() {
                return Err("a texture's update found the painter failed".to_owned());
            }
            self.slots.iter_mut().for_each(|slot| slot.last = None);
        }
        let changes = delta
            .set
            .iter()
            .flat_map(|(id, changes)| changes.iter().map(move |change| (id, change)));
        for (id, change) in changes {
            let egui::ImageData::Color(image) = &change.image;
            let [width, height] = image.size;
            let sampler = self.sampler(change.options)?;
            let existing = self.images.get(id).map(|image| image.size);
            let keep = change.pos.is_some() || existing == Some([width, height]);
            if !keep {
                let device = &self.gpu.as_ref().ok_or("no GPU")?.device;
                let desc = texture_descriptor(
                    MTLPixelFormat::RGBA8Unorm,
                    [width, height],
                    MTLTextureUsage::ShaderRead,
                );
                let texture = device
                    .newTextureWithDescriptor(&desc)
                    .ok_or("no texture for an image")?;
                self.images.insert(
                    *id,
                    Image {
                        texture,
                        sampler: sampler.clone(),
                        size: [width, height],
                    },
                );
            }
            // A partial update of a texture that was never set: egui sends none.
            let Some(target) = self.images.get_mut(id) else {
                continue;
            };
            target.sampler = sampler;
            let [x, y] = change.pos.unwrap_or([0, 0]);
            let region = MTLRegion {
                origin: MTLOrigin { x, y, z: 0 },
                size: MTLSize {
                    width,
                    height,
                    depth: 1,
                },
            };
            let bytes: NonNull<[Color32]> = NonNull::from(image.pixels.as_slice());
            // SAFETY: the image holds the region's pixels, four bytes each in Color32's RGBA order,
            // and nothing reads the texture now: the frames in flight have completed.
            unsafe {
                target
                    .texture
                    .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                        region,
                        0,
                        bytes.cast(),
                        width * 4,
                    );
            }
        }
        Ok(())
    }

    /// Encodes and commits one frame into the current target. Returns when it was encoded.
    fn draw(
        &mut self,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[ClippedPrimitive],
    ) -> Result<Instant, Failed> {
        let size = self.size;
        let slot_index = self.frame % SLOTS;
        #[cfg(feature = "demo")]
        let (clear_color, clipped_primitives) = if self.tear {
            // The whole window is one flat grey, 96 then 100, so that a capture holds exactly one of
            // them unless the compositor showed two frames at once ("Tears").
            let grey = if self.frame.is_multiple_of(2) {
                96.0
            } else {
                100.0
            } / 255.0;
            ([grey, grey, grey, 1.0], &[][..])
        } else {
            (clear_color, clipped_primitives)
        };
        self.frame += 1;
        let frames = &*self.frames;
        let slot = self.slots.get_mut(slot_index).ok_or("no such slot")?;
        slot.settle(frames);
        if frames.failed() {
            return Err("a frame found the painter failed".to_owned());
        }
        slot.last = None;
        let mut vertex_bytes = 0;
        let mut index_bytes = 0;
        for primitive in clipped_primitives {
            if let Primitive::Mesh(mesh) = &primitive.primitive {
                vertex_bytes += std::mem::size_of_val(mesh.vertices.as_slice());
                index_bytes += std::mem::size_of_val(mesh.indices.as_slice());
            }
        }
        let gpu = self.gpu.as_ref().ok_or("no GPU")?;
        let vertices_at =
            room(&gpu.device, &mut slot.vertices, vertex_bytes).ok_or("no vertex buffer")?;
        let indices_at =
            room(&gpu.device, &mut slot.indices, index_bytes).ok_or("no index buffer")?;
        let vertices = slot.vertices.as_ref().ok_or("no vertex buffer")?;
        let indices = slot.indices.as_ref().ok_or("no index buffer")?;
        let target = self.drawing.as_ref().ok_or("no target")?;
        let handler = &self.handler;
        #[cfg(feature = "demo")]
        let stall = self.fail_at == Some(FailAt::Stall);
        autoreleasepool(|_| {
            let pass = MTLRenderPassDescriptor::new();
            // SAFETY: attachment 0 exists on every device.
            let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            attachment.setTexture(Some(&target.texture));
            attachment.setLoadAction(MTLLoadAction::Clear);
            attachment.setStoreAction(MTLStoreAction::Store);
            attachment.setClearColor(MTLClearColor {
                red: f64::from(clear_color[0]),
                green: f64::from(clear_color[1]),
                blue: f64::from(clear_color[2]),
                alpha: f64::from(clear_color[3]),
            });
            let buffer = gpu.queue.commandBuffer().ok_or("no command buffer")?;
            let encoder = buffer
                .renderCommandEncoderWithDescriptor(&pass)
                .ok_or("no encoder")?;
            encoder.setRenderPipelineState(&gpu.pipeline);
            let screen = [
                size[0] as f32 / pixels_per_point,
                size[1] as f32 / pixels_per_point,
            ];
            let pixel = Pixel {
                height: size[1] as f32,
                dither: if self.dithering { 1.0 } else { 0.0 },
            };
            // SAFETY: both are plain values the shaders read by these slots and sizes, and Metal
            // copies them before the call returns.
            unsafe {
                encoder.setVertexBytes_length_atIndex(
                    NonNull::from(&screen).cast(),
                    std::mem::size_of_val(&screen),
                    1,
                );
                encoder.setFragmentBytes_length_atIndex(
                    NonNull::from(&pixel).cast(),
                    std::mem::size_of_val(&pixel),
                    0,
                );
            }
            let (mut vertex_at, mut index_at) = (0, 0);
            for primitive in clipped_primitives {
                let Primitive::Mesh(mesh) = &primitive.primitive else {
                    continue;
                };
                let vertex_size = std::mem::size_of_val(mesh.vertices.as_slice());
                let index_size = std::mem::size_of_val(mesh.indices.as_slice());
                // SAFETY: `room` made both buffers large enough for all the meshes, and shared
                // storage is ours to write: this slot's last frame has completed.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        mesh.vertices.as_ptr().cast::<u8>(),
                        vertices_at.as_ptr().cast::<u8>().add(vertex_at),
                        vertex_size,
                    );
                    std::ptr::copy_nonoverlapping(
                        mesh.indices.as_ptr().cast::<u8>(),
                        indices_at.as_ptr().cast::<u8>().add(index_at),
                        index_size,
                    );
                }
                let (this_vertex, this_index) = (vertex_at, index_at);
                vertex_at += vertex_size;
                index_at += index_size;
                let Some(rect) = scissor(size, pixels_per_point, primitive.clip_rect) else {
                    continue;
                };
                let Some(image) = self.images.get(&mesh.texture_id) else {
                    log::warn!("Failed to find texture {:?}", mesh.texture_id);
                    continue;
                };
                encoder.setScissorRect(rect);
                // SAFETY: the slots are the shaders', the offsets lie inside the buffers, and the
                // indices point into this mesh's vertices.
                unsafe {
                    encoder.setVertexBuffer_offset_atIndex(Some(vertices), this_vertex, 0);
                    encoder.setFragmentTexture_atIndex(Some(&image.texture), 0);
                    encoder.setFragmentSamplerState_atIndex(Some(&image.sampler), 0);
                    encoder
                        .drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                            MTLPrimitiveType::Triangle,
                            mesh.indices.len(),
                            MTLIndexType::UInt32,
                            indices,
                            this_index,
                        );
                }
            }
            encoder.endEncoding();
            let encoded = Instant::now();
            // SAFETY: the block is live: the painter keeps it, and Metal retains it for each buffer.
            // Its signature is the handlers', and what it captures crosses threads (`wait`). Both go
            // on before the commit, as Metal asks.
            unsafe {
                buffer.addScheduledHandler(RcBlock::as_ptr(handler));
                buffer.addCompletedHandler(RcBlock::as_ptr(handler));
            }
            buffer.commit();
            #[cfg(feature = "demo")]
            if stall {
                frames.stall();
            }
            // What a swap gives: the frame is on its way before the compositor is told.
            frames.wait_for(&buffer, Stage::Scheduled);
            slot.last = Some(buffer);
            if frames.failed() {
                return Err("a frame's wait found the painter failed".to_owned());
            }
            Ok(encoded)
        })
    }

    /// One frame into the current target, with no window: the textures' updates, the meshes and the
    /// frees. Returns when it was encoded, or `None` when it failed, and then the flag is up.
    fn render(
        &mut self,
        size: [usize; 2],
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[ClippedPrimitive],
        textures_delta: &mut TexturesDelta,
    ) -> Option<Instant> {
        let rendered = self.try_render(
            size,
            pixels_per_point,
            clear_color,
            clipped_primitives,
            textures_delta,
        );
        rendered.map_err(|why| self.frames.fail(&why)).ok()
    }

    fn try_render(
        &mut self,
        size: [usize; 2],
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[ClippedPrimitive],
        textures_delta: &mut TexturesDelta,
    ) -> Result<Instant, Failed> {
        if self.size != size {
            // The old size's surfaces go, here too, but for the one the layer shows: that stays until
            // this frame's replaces it, and a close finds it if this frame fails.
            drop_spare();
            self.drawing = None;
            self.shows_old_size = true;
            self.size = size;
        }
        if self.drawing.is_none() {
            let target = match spare::take() {
                Some(target) => target,
                None => self.target(size)?,
            };
            self.drawing = Some(target);
        }
        self.update_textures(textures_delta)?;
        // eframe appends each frame's delta to what the painter hands back: left in place, every
        // partial set stays alive and is written again on every later frame.
        textures_delta.set.clear();
        let encoded = self.draw(pixels_per_point, clear_color, clipped_primitives)?;
        for id in textures_delta.free.drain() {
            self.images.remove(&id);
        }
        Ok(encoded)
    }

    /// The layer follows a new size of the window. Its contents stay until the next frame's.
    fn fit_layer(&self, scale: f64) {
        if let Some(Shown { layer, .. }) = &self.shown
            && let Some(superlayer) = layer.superlayer()
        {
            at_once(|| {
                layer.setContentsScale(scale);
                layer.setFrame(superlayer.bounds());
            });
        }
    }

    /// Hands the layer the surface this frame drew, and gives back the one it stops showing, unless
    /// that one is of an old size: it goes here, as a later frame would draw into it at the wrong size.
    /// A new pointer each frame, so no private call.
    fn present(&mut self) -> Option<Target> {
        let target = self.drawing.take()?;
        if let Some(Shown { layer, .. }) = &self.shown {
            let object: &AnyObject = target.surface.as_ref();
            // SAFETY: an IOSurface is valid layer contents: "typically a CGImageRef or an
            // IOSurfaceRef" (`QuartzCore/CALayer.h`).
            at_once(|| unsafe { layer.setContents(Some(object)) });
        }
        let replaced = self.showing.replace(target);
        if std::mem::take(&mut self.shows_old_size) {
            None
        } else {
            replaced
        }
    }

    /// The end of a frame, with its surface drawn: hands the layer the surface, waits as `wait` says,
    /// and parks the surface the layer stopped showing if that wait says so. Otherwise that surface
    /// goes at the end of this call, as nothing would come to drop it. Returns what eframe takes off
    /// the frame's time, as glow's path does for the swap: the commit and the show, and the wait for
    /// the refresh.
    fn present_and_wait(
        &mut self,
        encoded: Instant,
        wait: impl FnOnce(&mut Hold) -> Waited,
    ) -> f32 {
        let replaced = self.present();
        let presented = encoded.elapsed();
        let waited = wait(&mut self.hold);
        if waited.parks
            && let Some(target) = replaced
        {
            spare::park(target);
        }
        (presented + waited.took).as_secs_f32()
    }

    /// The frame just drawn, as egui's screenshots hold it. An empty image when the painter failed,
    /// and no caller sends that on, or when it has drawn no frame.
    fn read_screen(&self) -> ColorImage {
        self.slots.iter().for_each(|slot| slot.settle(&self.frames));
        let target = self.drawing.as_ref();
        let (Some(target), false) = (target, self.frames.failed()) else {
            return ColorImage::new([0, 0], Vec::new());
        };
        let [width, height] = self.size;
        let mut pixels = vec![0_u8; width * height * 4];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width,
                height,
                depth: 1,
            },
        };
        // SAFETY: the buffer holds the region's pixels, the frame has completed, and the storage
        // is shared.
        unsafe {
            target.texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::from(pixels.as_mut_slice()).cast(),
                width * 4,
                region,
                0,
            );
        }
        let colors = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&[b, g, r, a]| Color32::from_rgba_premultiplied(r, g, b, a))
            .collect();
        ColorImage::new([width, height], colors)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint_and_update_textures(
        &mut self,
        viewport_id: ViewportId,
        pixels_per_point: f32,
        clear_color: [f32; 4],
        clipped_primitives: &[ClippedPrimitive],
        textures_delta: &mut TexturesDelta,
        capture_data: Vec<UserData>,
        window: &Arc<Window>,
    ) -> f32 {
        #[cfg(feature = "demo")]
        if let Some(why) = self.fail_here() {
            self.frames.fail(why);
        }
        // A failed painter paints nothing, and what it leaves unapplied is eframe's to drop: a delta
        // dropped full panics in a debug build (`epaint`'s `textures.rs`).
        if self.frames.failed() {
            textures_delta.clear();
            return 0.0;
        }
        let physical = window.inner_size();
        let size = [physical.width as usize, physical.height as usize];
        if size.contains(&0) {
            // No surface has no size. The delta stays for the frame that has one.
            return 0.0;
        }
        if self.size != size {
            self.fit_layer(window.scale_factor());
        }
        let Some(encoded) = self.render(
            size,
            pixels_per_point,
            clear_color,
            clipped_primitives,
            textures_delta,
        ) else {
            textures_delta.clear();
            return 0.0;
        };
        if !capture_data.is_empty() {
            let image = self.read_screen();
            if self.frames.failed() {
                textures_delta.clear();
                return 0.0;
            }
            let _ = self.capture_tx.send((viewport_id, capture_data, image));
            self.context.request_repaint_of(viewport_id);
        }
        self.present_and_wait(encoded, |hold| hold.wait(window))
    }

    /// Why this paint fails on purpose, under `ARRAKIS_METAL_FAIL`: `frame` at the first, and `rest`
    /// at the first that comes at least [`REST`] after the one before. Each says so once: every
    /// failure queues a block and a redraw, which until the close lands would add frames to the failed
    /// window's count. Reads the clock only under `rest`, as a measured drag runs this code.
    #[cfg(feature = "demo")]
    fn fail_here(&mut self) -> Option<&'static str> {
        let why = match self.fail_at {
            Some(FailAt::Frame) => "ARRAKIS_METAL_FAIL=frame",
            Some(FailAt::Rest) => {
                let now = Instant::now();
                let last = self.last_paint.replace(now);
                if !last.is_some_and(|last| now.duration_since(last) >= REST) {
                    return None;
                }
                "ARRAKIS_METAL_FAIL=rest"
            }
            _ => return None,
        };
        self.fail_at = None;
        Some(why)
    }

    pub fn handle_screenshots(&self, events: &mut Vec<Event>) {
        for (viewport_id, user_data, screenshot) in self.capture_rx.try_iter() {
            let screenshot = Arc::new(screenshot);
            for data in user_data {
                events.push(Event::Screenshot {
                    viewport_id,
                    user_data: data,
                    image: Arc::clone(&screenshot),
                });
            }
        }
    }

    pub fn gc_viewports(&mut self, _active: &ViewportIdSet) {}

    pub fn destroy(&mut self) {
        self.take_down();
    }

    /// The window is going, or leaves this painter: its link ends first, and the caller says what
    /// becomes of the layer.
    fn let_go_of_window(&mut self) -> Option<Shown> {
        self.frames.set_window(Weak::new());
        self.hold.close();
        self.shown.take()
    }

    /// Every close comes here, a failed window's too, whose next window is glow's: nothing of this
    /// painter's stays, the spare included, and no pause is left to drop it. The layer stays as it is
    /// and goes with the view, as a window shown without its contents is the flash that
    /// this painter keeps out, so every surface still held waits for the window to go:
    /// [`close`]. A frame still unfinished may yet render into them, and what that does to an emptied
    /// surface is unread, so then they only lose this reference. A healthy close has waited, and parks.
    fn take_down(&mut self) {
        drop(self.let_go_of_window());
        let frames = &self.frames;
        self.slots.iter().for_each(|slot| slot.settle(frames));
        let finished = self.slots.iter().all(Slot::finished);
        self.slots = Default::default();
        let held = [spare::take(), self.drawing.take(), self.showing.take()]
            .into_iter()
            .flatten()
            .map(|Target { surface, .. }| surface);
        if finished {
            close::park(held.collect());
        }
        self.images.clear();
        self.samplers.clear();
        self.gpu = None;
        self.size = [0, 0];
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        self.take_down();
    }
}

#[cfg(test)]
mod tests;
