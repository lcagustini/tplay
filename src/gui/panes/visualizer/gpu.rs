//! GL harness for the shader-drawn visualizer views.
//!
//! The one escape hatch egui offers is a `Shape::Callback`, and `epaint`
//! documents the contract: the backend sets the viewport to the callback's rect
//! and **restores any state the callback changed** — program, VAO, blend. So a
//! view may bind whatever it likes inside the callback and egui puts it back.
//! `egui_glow::paint_primitives` honours that literally, calling
//! `prepare_painting` under the comment `// Restore state` after every callback.
//!
//! **The two-tier rule.** DSP stays on the CPU and drawing moves to the GPU.
//! [`crate::audio::viz::compute_bands`] is 17 µs a frame and is covered by
//! `viz_tests.rs`; a hand-written WGSL/GLSL FFT would cost a buffer upload per
//! frame to save 0.1% of a frame, and would move the one part that can be
//! tested headlessly into the one part that cannot. So a shader view computes
//! its inputs exactly as the CPU views do and receives them as uniforms.
//!
//! **The uniform block is a fixed superset, and it is finished.** [`Uniforms`]
//! is every input a visualization may ask for; a shader declares only what it
//! references, and the whole block is uploaded unconditionally every frame. That
//! works because GLSL compilers strip unreferenced uniforms, and glow reports a
//! stripped one as `None` and skips the upload — so a look that reads two of
//! the eleven fields costs two wasted no-op calls and nothing else. A look that
//! needs a *new kind* of input is the only thing that may edit this file, and
//! that is a harness decision rather than per-view boilerplate.
//!
//! **A view touches nothing here.** A visualization is a `draw` fn and a shader
//! string, and the harness names no `VizView` — which is what keeps "adding a
//! view never means editing the harness" a property rather than a promise.
//! `tests/gui_tests.rs` reads this file's source to keep it that way.
//!
//! GL 3.3 core is the floor and no fallback is written below it: 3.3 is 2012,
//! and a `#version 120` twin would be a second copy of every shader to maintain
//! for hardware that does not ship. A driver that cannot compile a shader logs
//! it once and the pane keeps its background — see [`resources`].

use crate::audio::viz::{compute_level, VizBuf, VIZ_BANDS};
use crate::gui::theme::Palette;
use eframe::egui;
use glow::HasContext;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

/// Where a shader's output goes.
///
/// The seam a feedback view needs: one that samples its own previous frame cannot
/// do that from the screen, so it renders into an owned framebuffer first and
/// then presents the result.
///
/// `Clone` rather than `Copy` because the offscreen arm holds an `Arc`. A
/// [`Target`] is captured by the `Fn` paint callback, which runs on every frame
/// that shares one `Shape`, so the capture has to be a refcount bump and not a
/// move — the alternative would be a `Mutex` around the whole draw path, or
/// `Arc::make_mut` cloning a GL object name, which is worse than both.
#[derive(Clone)]
pub enum Target {
    /// Whatever framebuffer egui has bound, which is the pane's rect.
    Screen,
    /// An owned RGBA8 framebuffer, at the size the pane currently is. Held by
    /// value and refcounted, so a view hands the same `Arc` to every frame and
    /// the harness never has to know when the view stopped asking for it.
    Offscreen(Arc<Fbo>),
}

/// Every input a shader view may read. A fixed superset, deliberately: see the
/// module docs for why it is uploaded unconditionally.
///
/// Colours are straight (non-premultiplied) sRGB in 0..1, because egui blends
/// premultiplied vertex colours into a framebuffer it treats as linear storage —
/// so a shader that consumed premultiplied components would darken every edge it
/// drew next to an egui-drawn one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    /// Smoothed band levels in dB, `-60..0`, as `compute_bands` leaves them.
    pub bands: [f32; VIZ_BANDS],
    /// `(rms, peak)` in 0..1 — loudness, for a look that wants amount not shape.
    pub level: (f32, f32),
    pub accent: [f32; 4],
    pub bg: [f32; 4],
    /// The brighter token, for the body/edge split bars.rs makes.
    pub progress_fill: [f32; 4],
    /// Seconds since startup. The wall clock, not egui's `animation_time`, which
    /// `theme::apply` zeroes every frame.
    pub time: f32,
    /// Seconds since the previous frame, for anything that must be frame-rate
    /// independent rather than frame-counted.
    pub dt: f32,
    /// The pane rect in physical pixels — fragment count scales with DPI, so a
    /// shader that wants a point-space coordinate must divide by `pixels_per_point`.
    pub resolution: [f32; 2],
    /// A pair of mode numbers, for the views that pick one.
    pub modes: [f32; 2],
    /// How much of the previous frame survives, for a feedback view.
    pub feedback: f32,
}

impl Uniforms {
    /// Collect everything a shader view can read, in one place.
    ///
    /// The harness owns the block, so it also owns assembling it: a view never
    /// names a uniform, which is what keeps the set closed. Level is measured
    /// here rather than taken from the caller because it is two reads of a
    /// buffer the tap already filled — a few microseconds against the ~17 µs
    /// `compute_bands` already costs, and cheaper than the plumbing to let a
    /// view decline it.
    pub fn pack(
        viz: &VizBuf,
        bands: [f32; VIZ_BANDS],
        palette: &Palette,
        rect: egui::Rect,
        ctx: &egui::Context,
    ) -> Self {
        let ppp = ctx.pixels_per_point();
        let (time, dt) = ctx.input(|i| (i.time as f32, i.predicted_dt));
        let (rms, peak) = compute_level(viz);
        Self {
            bands,
            level: (rms, peak),
            accent: srgb(palette.accent),
            bg: srgb(palette.bg),
            progress_fill: srgb(palette.progress_fill),
            time,
            dt,
            resolution: [rect.width() * ppp, rect.height() * ppp],
            modes: [0.0, 0.0],
            feedback: 0.0,
        }
    }
}

fn srgb(c: egui::Color32) -> [f32; 4] {
    [
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        c.a() as f32 / 255.0,
    ]
}

/// Queue `frag` to fill `rect` this frame.
///
/// The uniform upload happens here rather than in the callback so that the GLSL
/// block and the values that fill it are described by the same struct — a
/// mismatch is then a compile error instead of a wrong picture.
pub fn add_fullscreen(
    painter: &egui::Painter,
    rect: egui::Rect,
    frag: &'static str,
    uniforms: Uniforms,
    target: Target,
) {
    painter.add(egui::Shape::Callback(egui::PaintCallback {
        rect,
        callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |info, gl| {
            draw(gl, &info, frag, &uniforms, target.clone());
        })),
    }));
}

/// A feedback view's state: the ping-pong pair, and which way round it is.
///
/// A view holds one of these in egui memory under its own key, which supplies
/// the two things a `Shape::Callback` cannot: targets that **outlive the frame**
/// that asked for them, and a **parity** that alternates them.
///
/// The pair sits behind a `Mutex` because the only place a GL context exists is
/// inside the paint callback, and that callback is `Fn` — it cannot mutate a
/// capture. So the creation pass writes the finished pair through the slot and
/// the *next* frame's `draw` picks it up. The parity is outside the lock because
/// it is a plain CPU decision made before any callback runs.
///
/// `id` is the view's egui-memory key, and it is **not** used to write anything
/// back: everything this struct needs to carry across frames already lives in
/// the slot, so the view only has to store the `Feedback` itself.
#[derive(Clone, Default)]
pub struct Feedback {
    built: Arc<Mutex<Option<Pair>>>,
    parity: bool,
}

/// The two targets, as `(source, destination)` for the frame about to be drawn.
type Pair = (Arc<Fbo>, Arc<Fbo>);

impl Feedback {
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Pair>> {
        self.built.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Drop the targets, so the next frame builds a fresh cleared pair.
    ///
    /// Called on a view switch, where the old contents are *another view's*
    /// trail, and on a resize, where they are the wrong shape. Either reads as a
    /// smear rather than as a fault, which is why it is worth an explicit call.
    ///
    /// The parity resets with them, so the first frame of a new pair is always
    /// the same target — otherwise which half of the swap is source would depend
    /// on how many frames the previous view happened to draw.
    pub fn invalidate(&mut self) {
        *self.slot() = None;
        self.parity = false;
    }

    /// Queue this frame's two passes: `accumulate` into the destination, then
    /// `present` it onto the pane.
    ///
    /// A no-op on the first frame after a fresh, a resize or a view switch, since
    /// the targets do not exist yet. That frame shows the background the pane
    /// already painted, which is the honest cost of a GL context that exists only
    /// while something is being drawn.
    pub fn draw(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        accumulate: &'static str,
        present: &'static str,
        uniforms: Uniforms,
    ) {
        let wanted = fbo_size((rect.width(), rect.height()));
        let ready = {
            let slot = self.slot();
            slot.as_ref()
                .filter(|(a, b)| a.size() == wanted && b.size() == wanted)
                .cloned()
        };

        let Some((a, b)) = ready else {
            self.create(painter, rect, wanted);
            return;
        };
        let (src, dst) = if self.parity { (b, a) } else { (a, b) };
        self.parity = !self.parity;

        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let gl = painter.gl();
                // A freshly created target holds undefined contents, and
                // `glClear` ignores the scissor box, so this clears the whole
                // target rather than the scissored part. One frame of
                // undefined texture otherwise shows as a flash of garbage.
                if dst.needs_clear() {
                    clear(gl, &dst);
                    dst.mark_drawn();
                }
                run_pass(
                    gl,
                    &info,
                    accumulate,
                    &uniforms,
                    Target::Offscreen(dst.clone()),
                    Some(&src),
                );
                // The viewport is re-derived because pass one rebound both.
                // The destination is the sampler here: the present pass reads
                // back exactly what the accumulate pass wrote.
                run_pass(gl, &info, present, &uniforms, Target::Screen, Some(&dst));
            })),
        }));
    }

    /// Build the pair in the one place that has a GL context.
    fn create(&self, painter: &egui::Painter, rect: egui::Rect, size: (i32, i32)) {
        let slot = self.built.clone();
        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                let gl = painter.gl();
                let pixels = (size.0 as f32, size.1 as f32);
                if let (Some(a), Some(b)) = (Fbo::new(gl, pixels), Fbo::new(gl, pixels)) {
                    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((a, b));
                }
            })),
        }));
    }
}

/// An owned RGBA8 framebuffer and the texture it renders into.
///
/// `Clone` is a refcount bump, not a GL copy, which is what lets a view hold one
/// in egui memory and hand the same `Arc` to every frame.
///
/// `Send + Sync` is a consequence of the design, not an oversight: the paint
/// callback is `Send + Sync`, so a target has to cross into it. The GL objects
/// are only ever used on the thread that owns the context, which is the thread
/// egui paints on, and the one mutable field is an `AtomicBool` written once a
/// frame — so a relaxed ordering is the correct one, and a `Mutex` would be a
/// lock on the paint path for no gain.
pub struct Fbo {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    size: (i32, i32),
    /// Set once the target has been rendered into, so a first frame starts from a
    /// defined state rather than from whatever was in a fresh texture.
    drawn: AtomicBool,
}

impl Clone for Fbo {
    fn clone(&self) -> Self {
        Self {
            framebuffer: self.framebuffer,
            texture: self.texture,
            size: self.size,
            drawn: AtomicBool::new(self.drawn.load(Ordering::Relaxed)),
        }
    }
}

// SAFETY: `glow::Framebuffer` and `glow::Texture` are `NonZeroU32` newtypes with
// no interior state of their own, and the one mutable field is an `AtomicBool`.
// The GL objects are only ever used on the thread that holds the context, which
// is the thread egui paints on — and `egui_glow::CallbackFn` requires `Sync`, so
// this is the bound the library's own signature asks for rather than a widening
// this module chose.
unsafe impl Send for Fbo {}
unsafe impl Sync for Fbo {}

impl Fbo {
    /// Create a target of `size` physical pixels.
    ///
    /// Returns `None` rather than a zero-sized framebuffer, which GL rejects
    /// with `INVALID_VALUE` and which would otherwise surface as a blank pane
    /// with nothing in the log.
    pub fn new(gl: &glow::Context, size: (f32, f32)) -> Option<Arc<Fbo>> {
        let (w, h) = fbo_size(size);
        // SAFETY: `gl` is the context egui is painting with. Every object created
        // here is deleted on every failure path below, so a rejected framebuffer
        // leaks nothing; a successful one lives for the process, as documented on
        // the type.
        unsafe {
            let texture = gl.create_texture().ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            // Linear filtering, clamped edges. The accumulate pass samples this
            // at a different scale than it renders it, and `REPEAT` on a
            // non-power-of-two texture is an error on every GL 3.3 driver.
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );

            let framebuffer = gl.create_framebuffer().ok()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                eprintln!(
                    "tplay: feedback framebuffer incomplete ({status:#x}) — the view keeps \
                     its background"
                );
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return None;
            }
            Some(Arc::new(Fbo {
                framebuffer,
                texture,
                size: (w, h),
                drawn: AtomicBool::new(false),
            }))
        }
    }

    pub fn size(&self) -> (i32, i32) {
        self.size
    }

    /// Whether this target has been rendered into since it was created.
    pub fn needs_clear(&self) -> bool {
        !self.drawn.load(Ordering::Relaxed)
    }

    pub fn mark_drawn(&self) {
        self.drawn.store(true, Ordering::Relaxed);
    }

    /// The texture to sample, on texture unit 0.
    ///
    /// Binds it and leaves unit 0 active, which is what the harness's
    /// `u_prev = 0` upload assumes.
    pub fn bind_texture(&self, gl: &glow::Context) {
        // SAFETY: a texture this module created, bound to a unit egui's own state
        // reset rebinds after the callback.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
        }
    }

    /// Make this the render target.
    pub fn bind(&self, gl: &glow::Context) {
        // SAFETY: a framebuffer this module created and verified complete. egui
        // rebinds its own after the callback (see `draw`'s safety note).
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        }
    }
}

/// The render target's pixel size: rounded up, and **never zero**.
///
/// Both halves matter and neither is cosmetic. A pane can legitimately measure
/// zero on one side for a frame while a splitter is dragged, and GL's
/// `texImage2D` and `glFramebufferTexture2D` both reject a zero dimension with
/// `INVALID_VALUE` — so a round-down, or a bare pass-through of the pane's size,
/// makes the whole view blank with nothing in any log. A floor of 1 keeps the
/// call valid, and the accumulation pass then has somewhere to write.
pub fn fbo_size(size: (f32, f32)) -> (i32, i32) {
    let px = |v: f32| {
        // `ceil` rather than `round`: a target one pixel short of the pane is
        // stretched by the sampler, and one pixel over costs nothing.
        (v.ceil() as i32).max(1)
    };
    (px(size.0), px(size.1))
}

/// How much of the previous frame survives into this one, given a frame time.
///
/// **Time-based, not frame-counted**, and that is the whole point: a per-frame
/// multiplier is a fixed fraction per frame, so the trail is twice as long at
/// 30 Hz as at 60 Hz, and nothing in a screenshot would reveal it. This is the
/// same "inject the clock" discipline as `config::should_flush` and as `vu.rs`'s
/// `HOLD_SECS`.
///
/// `hold_secs` is the time constant, so the trail's visible length is a number
/// the view's own constant states rather than one that depends on the frame rate.
pub fn feedback_for_a_dt(dt: f32, hold_secs: f32) -> f32 {
    // `<= 0` and not `< 0`, which is the whole point of the guard. `dt` comes
    // from egui's input, so it is legitimately zero on a session's first frame —
    // and `exp(0.0)` is exactly 1.0, i.e. "keep the entire previous frame". A
    // predicted frame time that is ever zero again freezes the view on one frame
    // for good, which is the single worst failure this function can have and the
    // one no screenshot of a working build would ever show. Dropping the frame is
    // the safe direction: the trail shortens, it does not stop.
    if hold_secs <= 0.0 || !dt.is_finite() || dt <= 0.0 {
        return 0.0;
    }
    // Continuous decay: after `hold_secs` exactly `1/e` of the frame is left, at
    // any frame rate. The clamp is belt-and-braces for a `dt` so small the
    // exponential rounds to exactly 1.0, which is the freeze again by another
    // route.
    let kept = (-dt / hold_secs).exp();
    kept.clamp(0.0, 0.999)
}

// -- the GL half -------------------------------------------------------------
// Everything below touches `glow` and `unsafe`. Nothing above this line does.

/// `#version` must be the first bytes of the source, so it is a prefix built
/// here rather than a line in a shader a view wrote.
const VERSION: &str = "#version 330 core\n";

/// The one vertex shader, shared by every look. A fullscreen triangle needs no
/// attributes, so there is no vertex buffer anywhere in this module.
const VERT: &str = r#"
void main() {
    // (0,0) (2,0) (0,2) -> the oversized triangle that covers clip space.
    vec2 corner = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// A compiled program plus every uniform location in the superset.
///
/// `Copy` so it can be handed out from under the cache's lock. Every location is
/// an `Option` because GLSL strips what a shader does not reference, and glow
/// reports a stripped uniform as `None` — which is what lets one struct describe
/// eleven uniforms for a shader that reads two.
#[derive(Clone, Copy)]
struct Program {
    handle: glow::Program,
    bands: Option<glow::UniformLocation>,
    level: Option<glow::UniformLocation>,
    accent: Option<glow::UniformLocation>,
    bg: Option<glow::UniformLocation>,
    progress_fill: Option<glow::UniformLocation>,
    time: Option<glow::UniformLocation>,
    dt: Option<glow::UniformLocation>,
    resolution: Option<glow::UniformLocation>,
    modes: Option<glow::UniformLocation>,
    feedback: Option<glow::UniformLocation>,
    prev: Option<glow::UniformLocation>,
}

struct Gl {
    /// One empty VAO for everything. Core profile refuses a draw with no VAO
    /// bound even when the shader reads no attributes.
    vao: Option<glow::VertexArray>,
    /// Keyed by fragment source: the assembled program is a pure function of it,
    /// and two views with identical shaders correctly share one program.
    programs: HashMap<&'static str, Option<Program>>,
}

static GL: LazyLock<Mutex<Gl>> = LazyLock::new(|| {
    Mutex::new(Gl {
        vao: None,
        programs: HashMap::new(),
    })
});

fn draw(
    painter: &egui_glow::Painter,
    info: &egui::PaintCallbackInfo,
    frag: &'static str,
    uniforms: &Uniforms,
    target: Target,
) {
    run_pass(painter.gl(), info, frag, uniforms, target, None);
}

/// One fullscreen pass: bind the target, set the viewport, upload, draw.
fn run_pass(
    gl: &glow::Context,
    info: &egui::PaintCallbackInfo,
    frag: &'static str,
    uniforms: &Uniforms,
    target: Target,
    sample: Option<&Arc<Fbo>>,
) {
    let (program, vao) = resources(gl, frag);
    let (Some(program), Some(vao)) = (program, vao) else {
        return;
    };

    let viewport = info.viewport_in_pixels();
    let clip = info.clip_rect_in_pixels();
    // SAFETY: the context is current on the thread egui is painting on, and
    // `egui_glow`'s own doc for `PaintCallback` promises the state changed here
    // is restored afterwards — which `paint_primitives` does by re-running
    // `prepare_painting`. Every call below is one of viewport/scissor, a
    // framebuffer or texture bind, state toggles, a program and VAO bind, a
    // uniform upload, or a draw, and each is followed by nothing that can leave
    // the context in a state egui does not reset. `info` comes from egui
    // mid-frame, so the viewport is already the callback's rect; it is set again
    // because correct-by-construction is not worth less than
    // correct-by-accident.
    unsafe {
        match target {
            Target::Screen => {} // egui already bound the framebuffer to draw into
            Target::Offscreen(fbo) => fbo.bind(gl),
        }
        if let Some(src) = sample {
            src.bind_texture(gl);
        }
        gl.disable(glow::BLEND);
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(
            clip.left_px,
            clip.from_bottom_px,
            clip.width_px,
            clip.height_px,
        );
        gl.viewport(
            viewport.left_px,
            viewport.from_bottom_px,
            viewport.width_px,
            viewport.height_px,
        );
        gl.use_program(Some(program.handle));
        gl.bind_vertex_array(Some(vao));
        program.upload(gl, uniforms);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }
}

/// Fill a target with transparent black.
///
/// A fresh texture's contents are undefined, so the first accumulate pass would
/// otherwise mix against whatever the driver left there. `glClear` ignores the
/// scissor box, so the whole target is cleared — which is what is wanted, and why
/// this cannot be a scissored quad.
fn clear(gl: &glow::Context, fbo: &Fbo) {
    // SAFETY: a framebuffer this module created and verified complete, cleared
    // and then immediately drawn over.
    unsafe {
        fbo.bind(gl);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
    }
}

impl Program {
    /// The whole superset, every frame, unconditionally. See the module docs:
    /// a uniform this shader stripped reports `None` and glow skips it.
    unsafe fn upload(&self, gl: &glow::Context, u: &Uniforms) {
        gl.uniform_1_f32_slice(self.bands.as_ref(), &u.bands);
        gl.uniform_2_f32(self.level.as_ref(), u.level.0, u.level.1);
        gl.uniform_4_f32_slice(self.accent.as_ref(), &u.accent);
        gl.uniform_4_f32_slice(self.bg.as_ref(), &u.bg);
        gl.uniform_4_f32_slice(self.progress_fill.as_ref(), &u.progress_fill);
        gl.uniform_1_f32(self.time.as_ref(), u.time);
        gl.uniform_1_f32(self.dt.as_ref(), u.dt);
        gl.uniform_2_f32_slice(self.resolution.as_ref(), &u.resolution);
        gl.uniform_2_f32_slice(self.modes.as_ref(), &u.modes);
        gl.uniform_1_f32(self.feedback.as_ref(), u.feedback);
        // The feedback texture is unit 0. Declared by the superset and read by
        // no shipped view yet; `Trails` binds it to an owned framebuffer.
        gl.uniform_1_i32(self.prev.as_ref(), 0);
    }
}

/// The cached program for `frag` and the shared VAO, in one lock.
///
/// A `None` in the cache is a *recorded failure*, not a miss: a driver that
/// cannot compile a shader would otherwise be asked again on every one of 60
/// frames a second, and a compile is a driver round trip and a log line.
fn resources(
    gl: &glow::Context,
    frag: &'static str,
) -> (Option<Program>, Option<glow::VertexArray>) {
    let mut state = GL.lock().unwrap_or_else(PoisonError::into_inner);
    if state.vao.is_none() {
        state.vao = unsafe { gl.create_vertex_array() }.ok();
    }
    if !state.programs.contains_key(frag) {
        let built = build(gl, frag);
        state.programs.insert(frag, built);
    }
    (state.programs.get(frag).copied().flatten(), state.vao)
}

/// Compile and link one program, or record why it could not be built.
fn build(gl: &glow::Context, frag: &'static str) -> Option<Program> {
    // SAFETY: `gl` is the context egui is painting with. Every object created
    // here is deleted on the failure paths, and a program that links is kept
    // for the process — its shaders are detached and deleted immediately, which
    // is what leaves the program itself valid.
    unsafe {
        let program = gl.create_program().ok()?;
        for (stage, source) in [
            (glow::VERTEX_SHADER, format!("{VERSION}{VERT}")),
            (glow::FRAGMENT_SHADER, fragment_source(frag)),
        ] {
            let Some(shader) = gl.create_shader(stage).ok() else {
                gl.delete_program(program);
                return None;
            };
            gl.shader_source(shader, &source);
            gl.compile_shader(shader);
            if !gl.get_shader_completion_status(shader) {
                eprintln!(
                    "tplay: {} shader failed to compile: {}",
                    stage_name(stage),
                    gl.get_shader_info_log(shader).trim()
                );
                gl.delete_shader(shader);
                gl.delete_program(program);
                return None;
            }
            gl.attach_shader(program, shader);
            // The program holds what it needs once attached, so the shader object
            // goes now rather than leaking one pair per view for the session.
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            eprintln!(
                "tplay: shader program failed to link: {}",
                gl.get_program_info_log(program).trim()
            );
            gl.delete_program(program);
            return None;
        }
        let loc = |name: &str| gl.get_uniform_location(program, name);
        Some(Program {
            handle: program,
            bands: loc("u_bands[0]"),
            level: loc("u_level"),
            accent: loc("u_accent"),
            bg: loc("u_bg"),
            progress_fill: loc("u_progress_fill"),
            time: loc("u_time"),
            dt: loc("u_dt"),
            resolution: loc("u_resolution"),
            modes: loc("u_modes"),
            feedback: loc("u_feedback"),
            prev: loc("u_prev"),
        })
    }
}

fn stage_name(stage: u32) -> &'static str {
    if stage == glow::VERTEX_SHADER {
        "vertex"
    } else {
        "fragment"
    }
}

/// Prefix a view's shader body with the version, the fragment output, and only
/// the uniforms it actually mentions.
///
/// Two properties, one function. The band count is interpolated from
/// [`VIZ_BANDS`], so the array cannot drift out of step with the DSP — a
/// literal would fail *silently*, since extra uniforms are dropped and missing
/// ones read as undefined. And a shader cannot reference an input the harness
/// does not upload, because the harness owns this list, so no test is needed to
/// keep the two in step.
fn fragment_source(frag: &str) -> String {
    let mut out = String::with_capacity(frag.len() + 512);
    out.push_str(VERSION);
    out.push_str("out vec4 frag_color;\n");
    for (name, decl) in declarations() {
        if mentions(frag, name) {
            out.push_str(&decl);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(frag);
    out
}

/// The superset as `(identifier, declaration)` pairs, filtered by what a shader
/// references. Order is the field order of [`Uniforms`].
///
/// `VIZ_BANDS` is offered as a `const` and not left to the shader to hardcode,
/// for the same reason the array length is interpolated: a shader writing its own
/// `32` would compile fine and read the wrong band after the band count moved.
fn declarations() -> Vec<(&'static str, String)> {
    vec![
        ("VIZ_BANDS", format!("const int VIZ_BANDS = {VIZ_BANDS};")),
        ("u_bands", format!("uniform float u_bands[{VIZ_BANDS}];")),
        ("u_level", "uniform vec2 u_level;".into()),
        ("u_accent", "uniform vec4 u_accent;".into()),
        ("u_bg", "uniform vec4 u_bg;".into()),
        ("u_progress_fill", "uniform vec4 u_progress_fill;".into()),
        ("u_time", "uniform float u_time;".into()),
        ("u_dt", "uniform float u_dt;".into()),
        ("u_resolution", "uniform vec2 u_resolution;".into()),
        ("u_modes", "uniform vec2 u_modes;".into()),
        ("u_feedback", "uniform float u_feedback;".into()),
        ("u_prev", "uniform sampler2D u_prev;".into()),
    ]
}

/// Whether `src` uses `ident` as a whole word.
///
/// The boundary test exists for one reason: without it `u_bands` matches inside
/// `u_bands_scaled`, and a false positive costs one unused declaration and one
/// no-op upload. So the failure mode here is free, and there is nothing to gain
/// from parsing GLSL properly.
fn mentions(src: &str, ident: &str) -> bool {
    if ident.is_empty() {
        return false;
    }
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    src.match_indices(ident).any(|(at, _)| {
        let before = at == 0 || !word(src.as_bytes()[at - 1]);
        let end = at + ident.len();
        let after = end >= src.len() || !word(src.as_bytes()[end]);
        before && after
    })
}
