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

use crate::audio::viz::{compute_wave, VizBuf, DB_FLOOR, VIZ_BANDS, WAVE_BUCKETS};
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
// `Default` is hand-written rather than derived: `Default` is not implemented for
// arrays longer than 32, and the band and wave arrays are both longer than that.
// A struct with a `wave` in it is never a sensible all-zero block, so this exists
// only because `#[derive(Default)]` is the habit — `pack` is the constructor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uniforms {
    /// Smoothed band levels in dB, `-60..0`, as `compute_bands` leaves them.
    pub bands: [f32; VIZ_BANDS],
    /// `WAVE_BUCKETS` peak-envelope buckets over the ring buffer's tail, so a
    /// time-domain look interpolates between them rather than asking for a
    /// column count.
    ///
    /// **A fixed count, not a pane-derived one.** The CPU version sized this to
    /// the pane's pixel width, which meant the array length moved with the
    /// splitter; a uniform array's length is part of its declaration, so it has
    /// to be a constant. See `WAVE_BUCKETS` for why it is 128 and not 256.
    pub wave: [f32; WAVE_BUCKETS],
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
    /// names a uniform, which is what keeps the set closed.
    ///
    /// **The block is expected to shrink, and it did.** It carried `u_hold`,
    /// `u_slider_track` and `u_border` for the VU meter and then `u_level` with
    /// it, because that view was the sole reader of all four. A field here is not
    /// free even when no shader declares it: `u_level` was filled by a full pass
    /// over 4 096 samples on **every frame of every view**, for a uniform nobody
    /// uploaded. `compute_level` went with it — see the note where it used to
    /// live in `audio::viz`.
    pub fn pack(
        viz: &VizBuf,
        bands: [f32; VIZ_BANDS],
        palette: &Palette,
        rect: egui::Rect,
        ctx: &egui::Context,
    ) -> Self {
        let ppp = ctx.pixels_per_point();
        let (time, dt) = ctx.input(|i| (i.time as f32, i.predicted_dt));
        Self {
            bands,
            wave: compute_wave(viz, WAVE_BUCKETS)
                .try_into()
                .unwrap_or([0.0; WAVE_BUCKETS]),
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
    /// Set once target creation has failed, so it is not retried every frame.
    failed: Arc<AtomicBool>,
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
        // A driver that cannot give us a framebuffer is asked **once**. Without
        // this, a failure means two textures and two framebuffers created and
        // released 60 times a second, for ever — the same class of runaway as the
        // per-frame rebuild this function used to do. The program cache learned
        // this lesson; the feedback path did not.
        if self.failed.load(Ordering::Relaxed) {
            return;
        }
        let wanted = target_size(rect, painter.ctx().pixels_per_point());
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

    /// Build the pair in the one place that has a GL context, releasing whatever
    /// it replaces.
    ///
    /// **The old pair is destroyed here, in the callback, because that is the only
    /// place a context exists.** An `Fbo` cannot implement `Drop` — deleting a GL
    /// object needs the context, which `Drop` has no way to reach — so before this,
    /// the only way to lose a target was for the pane's size to change, and every
    /// one of those leaked two textures and two framebuffers. See [`target_size`]
    /// for why the size used to change far more often than it should have.
    fn create(&self, painter: &egui::Painter, rect: egui::Rect, size: (i32, i32)) {
        let slot = self.built.clone();
        let failed = self.failed.clone();
        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                let gl = painter.gl();
                let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
                // Released before the new pair is made, so the peak is one pair
                // rather than two.
                if let Some((a, b)) = guard.take() {
                    a.destroy(gl);
                    b.destroy(gl);
                }
                match (Fbo::new(gl, size), Fbo::new(gl, size)) {
                    (Some(a), Some(b)) => *guard = Some((a, b)),
                    _ => failed.store(true, Ordering::Relaxed),
                }
            })),
        }));
    }
}

/// The pixel size a feedback target should be, quantised.
///
/// **Physical pixels, not points.** The pane rect is in points and the screen is
/// in physical pixels, and on a HiDPI display those differ by the scale factor —
/// so sizing a render target from the rect alone gives a target a *quarter* of
/// the on-screen area at 2x, and the trail is drawn at a quarter of the
/// resolution and stretched. That reads as a soft low-resolution smear rather
/// than as a bug.
///
/// **Quantised to a multiple of [`TARGET_GRID`], and that half is the
/// load-bearing one.** A dock's pane rect jitters by fractions of a pixel as a
/// splitter settles, and an exact size check reads that jitter as "the targets
/// are the wrong size" — so every jitter rebuilt both targets, and each rebuild
/// leaked the pair it replaced. Rounding to a coarse grid means the size changes
/// only when the pane really did, which is the one event that should cost a
/// rebuild.
///
/// Coarse rather than exact because the content is a soft accumulating trail: a
/// few pixels of upscale is invisible, whereas being wrong by a pixel every frame
/// is not.
pub fn target_size(rect: egui::Rect, pixels_per_point: f32) -> (i32, i32) {
    let quantise = |points: f32| {
        let cells = (points * pixels_per_point / TARGET_GRID as f32).ceil();
        // The floor is a whole grid cell, so a zero or negative extent — which a
        // pane really does report for a frame while a splitter is dragged —
        // cannot produce a zero-sized target. GL rejects one with `INVALID_VALUE`
        // and a driver reporting an error on a path nobody checks leaves nothing
        // in any log, so the symptom is a blank pane and nothing else.
        ((cells as i32).max(1) * TARGET_GRID).max(TARGET_GRID)
    };
    (quantise(rect.width()), quantise(rect.height()))
}

/// The grid [`target_size`] rounds to, in physical pixels.
const TARGET_GRID: i32 = 64;

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
    /// `size` must come from [`target_size`], which floors it — see there for why
    /// a zero dimension is the failure mode worth designing out.
    pub fn new(gl: &glow::Context, size: (i32, i32)) -> Option<Arc<Fbo>> {
        let (w, h) = size;
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

    /// Release the GL objects.
    ///
    /// A free function rather than a `Drop` because deleting a GL object needs the
    /// context and `Drop` has no way to reach it — so any target that went out of
    /// scope without this leaked both its framebuffer and its texture. Called from
    /// the one place that has a context, when a target is replaced.
    pub fn destroy(&self, gl: &glow::Context) {
        // SAFETY: both objects were created by `Fbo::new`, and nothing reachable
        // still refers to them — `Feedback::create` has taken the pair out of the
        // slot, and the other `Arc` to each target died with the frame that queued
        // the callback. Deleting a still-bound name is defined by GL (it unbinds),
        // and egui restores its own state after the callback regardless.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
    }
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
out vec2 v_uv;

void main() {
    // (0,0) (2,0) (0,2) -> the oversized triangle that covers clip space.
    vec2 corner = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
    // 0..1 across the callback's OWN rect. The viewport is set to the pane, so
    // NDC -1..1 *is* the pane and `corner` is already that fraction — no uniform
    // and no per-fragment divide.
    //
    // **This varying exists because `gl_FragCoord` is window-relative, and every
    // view here is pane-relative.** Dividing `gl_FragCoord.xy` by the pane's size
    // gives a uv that runs from `pane_left / pane_w` to that plus one, so a dock
    // tab anywhere but the left edge of the window reads part of its pattern from
    // outside itself and saturates on the rest: the bars and the wave froze on a
    // vertical seam at a constant, and the rings drew off-centre because `0.5` is
    // `pane_w / 2` of *window* x. `gl_FragCoord` is also **bottom-left origin**,
    // which put the ridgeline upside down.
    //
    // y is flipped here, once, because that is the one place the difference can
    // be stated once: every view reasons in the pane's top-down point space, and
    // `uv.y == 0.0` has to mean the top of the pane for all of them.
    v_uv = vec2(corner.x, 1.0 - corner.y);
}
"#;

/// The complete vertex shader source, for a test that links against it.
///
/// `pub` only so `every_shader_compiles_and_links` can build the same pair the
/// driver does. Nothing in the app reads this — and the `main` binary compiles
/// this module too, which is the only reason the exemption needs saying aloud.
#[allow(dead_code)]
pub fn vertex_source() -> String {
    format!("{VERSION}{VERT}")
}

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
    wave: Option<glow::UniformLocation>,
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

static REPORTED: LazyLock<Mutex<std::collections::HashSet<&'static str>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Print `msg` the first time `key` is seen, and never again.
///
/// The project cannot look at the window, so a shader view's only way to report
/// anything is stderr. A blank pane has two very different causes — a program
/// that would not build, and one that builds but is handed a viewport of zero —
/// and they are indistinguishable from outside, so the harness says which.
fn diag(key: &'static str, msg: impl FnOnce() -> String) {
    let mut seen = REPORTED.lock().unwrap_or_else(PoisonError::into_inner);
    if seen.insert(key) {
        eprintln!("tplay: [shader] {}", msg());
    }
}

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
    // One-shot, because a per-frame path that reports is a per-frame path that
    // spams — and because a shader that draws nothing and a shader that is never
    // reached look identical from outside the window.
    diag("first-draw", || {
        format!(
            "first draw: viewport {}x{} px, clip {}x{}, resolution uniform {:.0}x{:.0}, \
             {} program",
            viewport.width_px,
            viewport.height_px,
            clip.width_px,
            clip.height_px,
            uniforms.resolution[0],
            uniforms.resolution[1],
            if sample.is_some() {
                "feedback"
            } else {
                "screen"
            },
        )
    });
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
        match &target {
            // **Bind the default framebuffer, do not assume it.** For a single
            // pass egui has it bound already, but a feedback view's first pass
            // rebinds to its own target, and the second pass then draws the
            // *present* into that offscreen texture instead of onto the screen —
            // which renders correctly, raises no error, and shows a blank pane.
            // `None` is the default framebuffer, which is where egui paints.
            Target::Screen => gl.bind_framebuffer(glow::FRAMEBUFFER, None),
            Target::Offscreen(fbo) => fbo.bind(gl),
        }
        if let Some(src) = sample {
            src.bind_texture(gl);
        }
        gl.disable(glow::BLEND);
        gl.enable(glow::SCISSOR_TEST);
        // **An offscreen pass fills the whole target.** The callback's viewport
        // and clip are the *pane's* rectangle in window coordinates, which is the
        // right frame for the default framebuffer and the wrong one for an owned
        // target: a framebuffer's viewport is its own space, so using the pane's
        // window position wrote the accumulation at a scroll offset inside the
        // texture and dropped whatever fell outside it. The quantised target is
        // then slightly larger than the pane, and `v_uv` spans all of it, so the
        // present pass upscales by at most one grid cell.
        let (sx, sy, sw, sh) = match &target {
            Target::Screen => (
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            ),
            Target::Offscreen(fbo) => {
                let (w, h) = fbo.size();
                (0, 0, w, h)
            }
        };
        let (vx, vy, vw, vh) = match &target {
            Target::Screen => (
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            ),
            Target::Offscreen(fbo) => {
                let (w, h) = fbo.size();
                (0, 0, w, h)
            }
        };
        gl.scissor(sx, sy, sw, sh);
        gl.viewport(vx, vy, vw, vh);
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
        gl.uniform_1_f32_slice(self.wave.as_ref(), &u.wave);
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
    // here is released on every failure path, and on success the two shader
    // objects are detached and deleted *after* linking, which leaves the program
    // itself valid and holding its own copy of the compiled code.
    unsafe {
        let program = gl.create_program().ok()?;
        // Held so they can be released after the link. **A shader must stay
        // attached until then**: `glAttachShader` before a link only records the
        // association, and `glDetachShader` undoes it, so detaching straight away
        // — which this did, to avoid leaking the object — leaves the program with
        // nothing attached and `glLinkProgram` fails with "no shaders attached to
        // the program". The shaders are freed on the line below the link, which is
        // the earliest that is legal.
        let mut shaders: Vec<glow::Shader> = Vec::with_capacity(2);
        for (stage, source) in [
            (glow::VERTEX_SHADER, format!("{VERSION}{VERT}")),
            (glow::FRAGMENT_SHADER, fragment_source(frag)),
        ] {
            let Some(shader) = gl.create_shader(stage).ok() else {
                for s in &shaders {
                    gl.delete_shader(*s);
                }
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
                for s in &shaders {
                    gl.delete_shader(*s);
                }
                gl.delete_shader(shader);
                gl.delete_program(program);
                return None;
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }

        gl.link_program(program);
        let linked = gl.get_program_link_status(program);
        if !linked {
            eprintln!(
                "tplay: shader program failed to link: {}",
                gl.get_program_info_log(program).trim()
            );
        }
        // Detach and delete whether or not it linked: a linked program keeps its
        // own copy of the compiled code, and an unlinked one is about to be
        // deleted anyway.
        for shader in &shaders {
            gl.detach_shader(program, *shader);
            gl.delete_shader(*shader);
        }
        if !linked {
            gl.delete_program(program);
            return None;
        }
        diag("program-built", || {
            format!(
                "linked a program from {} bytes of fragment source",
                frag.len()
            )
        });
        let loc = |name: &str| gl.get_uniform_location(program, name);
        Some(Program {
            handle: program,
            bands: loc("u_bands[0]"),
            wave: loc("u_wave[0]"),
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
///
/// `pub` so `every_shader_compiles_and_links` can validate **exactly** the source
/// the driver gets, assembled by this same function, rather than a hand-copied
/// approximation in a test that could drift from the real thing.
pub fn fragment_source(frag: &str) -> String {
    let mut out = String::with_capacity(frag.len() + 1024);
    out.push_str(VERSION);
    out.push_str("in vec2 v_uv;\n");
    out.push_str("out vec4 frag_color;\n");
    // A shared helper brings its own inputs with it, so the declarations it needs
    // are pulled in by *its* presence and not only by the body mentioning them.
    let bearing = mentions(frag, "spectrum_at_bearing");
    // `spectrum_at_bearing` reads the bands, so it needs the same two
    // declarations the body would have needed for itself.
    let needed: &[&str] = if bearing {
        &["VIZ_BANDS", "u_bands", "level"]
    } else {
        &[]
    };
    for (name, decl) in declarations() {
        if mentions(frag, name) || needed.contains(&name) {
            out.push_str(&decl);
            out.push('\n');
        }
    }
    out.push('\n');
    if bearing {
        out.push_str(BEARING);
        out.push('\n');
    }
    out.push_str(frag);
    out
}

/// The dB → 0..1 mapping every view draws with, built from the DSP's own constant.
///
/// It was a hand-written `float level(float d)` in six view bodies plus two inlined
/// copies, and the number in it is a **boundary between two halves that cannot see
/// each other** — see [`DB_FLOOR`]. Interpolating it here means the floor is
/// written down once, and `the_db_floor_is_one_number_across_the_dsp_and_every_shader`
/// is what proves the two ends still agree.
///
/// Emitted on a mention of `level`, `DB_FLOOR` or `DB_SPAN`, so a view gets it by
/// calling `level(u_bands[i])` and a view that wants the raw range for its own
/// arithmetic can name the constants instead.
fn level_source() -> String {
    format!(
        "const float DB_FLOOR = {DB_FLOOR};\n\
         const float DB_SPAN = {};\n\
         float level(float d) {{\n    return clamp((d - DB_FLOOR) / DB_SPAN, 0.0, 1.0);\n}}",
        -DB_FLOOR
    )
}

/// The one safe way to read the spectrum **as a function of a direction**, and the
/// only reason this string exists.
///
/// Three of the views want "the band at the angle this fragment sits at", and the
/// obvious spelling of that is `atan(y, x)` indexed into `u_bands`. **`atan` has a
/// branch cut**: it jumps from `+pi` to `-pi` along `x < 0` at `y == 0`, which is
/// a fixed line up the middle of the pane's left side. So the band index — and
/// with it whatever the index drives — steps discontinuously there, and **no edge
/// fade can hide it, because the break is in the *function* and not at a boundary
/// of it.** That is what the two seams reported from `Plasma` and `Trails` were.
///
/// A projection onto the first two harmonics of the bearing is the same shape — a
/// band lookup smeared around the circle, `R * cos(a - phi)` for the two
/// coefficients — and it is continuous and periodic by construction, so there is
/// no cut left to find. The third harmonic is what stops the result being
/// symmetric under a half turn, which one harmonic alone would be.
///
/// It is in the harness rather than copied into three view bodies because the
/// alternative is three copies of a hazard, and a copy can be got wrong in a way
/// the original cannot. `a_shader_never_indexes_a_band_through_an_angle` is what
/// makes that a rule rather than a preference: the broken form is forbidden and
/// this is the replacement, so a new view that wants a radial spectrum read has one
/// path to find rather than one trap to rediscover.
const BEARING: &str = r#"
// A continuous, periodic read of the spectrum around the circle.
//
// `p` is the fragment's position relative to the pane's centre. The `max` guards
// that centre, where the length is zero and the division is undefined — and it is
// a `max` rather than an `if` because a guard has to be as continuous as the thing
// it guards: a branch here would put the discontinuity somewhere new.
float spectrum_at_bearing(vec2 p) {
    vec2 dir = p / max(length(p), 0.12);
    float b0 = 0.0, b1 = 0.0, b2 = 0.0;
    for (int i = 0; i < VIZ_BANDS; i++) {
        float a = 6.2831853 * float(i) / float(VIZ_BANDS);
        float l = level(u_bands[i]);
        b0 += l * cos(a);
        b1 += l * sin(a);
        b2 += l * cos(2.0 * a);
    }
    float n = float(VIZ_BANDS);
    return clamp(0.5 + (0.6 * (b0 * dir.x + b1 * dir.y) + 0.3 * b2) / n, 0.0, 1.0);
}
"#;

/// The superset as `(identifier, declaration)` pairs, filtered by what a shader
/// references. Order is the field order of [`Uniforms`].
///
/// `VIZ_BANDS` is offered as a `const` and not left to the shader to hardcode,
/// for the same reason the array length is interpolated: a shader writing its own
/// `32` would compile fine and read the wrong band after the band count moved.
fn declarations() -> Vec<(&'static str, String)> {
    vec![
        ("VIZ_BANDS", format!("const int VIZ_BANDS = {VIZ_BANDS};")),
        ("level", level_source()),
        ("u_bands", format!("uniform float u_bands[{VIZ_BANDS}];")),
        (
            "WAVE_BUCKETS",
            format!("const int WAVE_BUCKETS = {WAVE_BUCKETS};"),
        ),
        ("u_wave", format!("uniform float u_wave[{WAVE_BUCKETS}];")),
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
