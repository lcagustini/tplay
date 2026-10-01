//! wgpu harness for the shader-drawn visualizer views.
//!
//! The one escape hatch egui offers is a `Shape::Callback`, and `epaint`
//! documents the contract: the backend sets the viewport to the callback's rect
//! and restores the state the callback touched. A view may therefore issue its
//! own draw commands from inside one.
//!
//! **eframe 0.36 renders with wgpu, so this speaks wgpu.** That is not a
//! preference: eframe's default features *changed*, dropping `glow` in favour of
//! `wgpu`, and a wgpu renderer cannot downcast a `egui_glow::CallbackFn` — the
//! docs say so plainly, *"if the type cannot be downcast to the type expected by
//! the current backend the callback will not be drawn"*. Every view therefore
//! went blank and nothing else did, because ordinary shapes and textures are
//! backend-agnostic. See `init` for the one piece of setup that needs.
//!
//! **The two-tier rule.** DSP stays on the CPU and drawing moves to the GPU.
//! [`crate::audio::viz::compute_bands`] is 17 µs a frame and is covered by
//! `viz_tests.rs`; a hand-written WGSL FFT would cost a buffer upload per frame
//! to save 0.1% of a frame, and would move the one part that can be tested
//! headlessly into the one part that cannot. So a shader view computes its
//! inputs exactly as the CPU views do and receives them as uniforms.
//!
//! **The uniform block is a fixed superset, and it is finished.** [`Uniforms`]
//! is every input a visualization may ask for, and the whole block is uploaded
//! unconditionally every frame. Under GLSL that was free because the compiler
//! stripped what a shader did not reference; under WGSL nothing is stripped, and
//! it is still the right shape, because one `struct` and one bind group serve
//! every view — so a look that reads two of the fields costs nothing and a look
//! needing a *new kind* of input is the only thing that may edit this file.
//!
//! **Every member is a `vec4`, and that is the load-bearing part.** WGSL gives
//! an array in the uniform address space a **16-byte element stride** whatever
//! its element type, so `array<f32, 32>` costs 512 bytes, not 128. A buffer
//! packed at 4 bytes per `f32` is then read as garbage — and *silently* garbage,
//! because naga validates the shader against the shader and the buffer is ours.
//! So the arrays are `array<vec4<f32>, N/4>` and every scalar rides in a lane.
//! [`Uniforms::write_block`] is the single writer and [`wgsl_struct`] the single
//! declaration, and a test compares the two against naga's own layout.
//!
//! **A view touches nothing here.** A visualization is a `draw` fn and a shader
//! body, and the harness names no view — which is what keeps "adding a view never
//! means editing the harness" a property rather than a promise.
//! `tests/gui_tests.rs` reads this file's source to keep it that way.

use crate::audio::viz::{compute_wave, VizBuf, DB_FLOOR, VIZ_BANDS, WAVE_BUCKETS};
use crate::gui::theme::Palette;
use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

// -- the CPU half --------------------------------------------------------------
// Nothing below this line touches wgpu. Nothing above it does.

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
    /// splitter; an array's length is part of its declaration, so it has to be a
    /// constant.
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
    /// names an input, which is what keeps the set closed.
    ///
    /// **The block is expected to shrink, and it did.** It carried `u_hold`,
    /// `u_slider_track` and `u_border` for the VU meter and then `u_level` with
    /// it, because that view was the sole reader of all four. A field here is not
    /// free even when no shader declares it: `u_level` was filled by a full pass
    /// over 4 096 samples on **every frame of every view**, for an input nobody
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

    /// The block as the GPU reads it: a flat run of `f32` in [`wgsl_struct`]'s
    /// order, four to a `vec4`.
    ///
    /// **The one place a uniform buffer is written.** The order and the padding
    /// are both dictated by the WGSL declaration, so a mismatch is a test
    /// failure (`the_uniform_block_matches_the_shader_struct`) rather than a
    /// picture made of noise.
    pub fn write_block(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(Self::BLOCK_FLOATS);
        out.extend_from_slice(&self.bands);
        out.extend_from_slice(&self.wave);
        out.extend_from_slice(&self.accent);
        out.extend_from_slice(&self.bg);
        out.extend_from_slice(&self.progress_fill);
        // One lane each, in the order `wgsl_struct` declares them.
        out.extend_from_slice(&[self.time, self.dt, self.feedback, 0.0]);
        out.extend_from_slice(&[self.resolution[0], self.resolution[1], 0.0, 0.0]);
        out.extend_from_slice(&[self.modes[0], self.modes[1], 0.0, 0.0]);
        debug_assert_eq!(out.len(), Self::BLOCK_FLOATS);
        out
    }

    /// Floats in the block, which is `4 *` the `vec4` count.
    pub const BLOCK_FLOATS: usize = VIZ_BANDS + WAVE_BUCKETS + 12 + 4 + 4 + 4;

    /// The block in bytes — the buffer's size and the bind group layout's
    /// `min_binding_size`, so wgpu checks the buffer against the shader instead of
    /// trusting this number.
    pub const BLOCK_BYTES: u64 = (Self::BLOCK_FLOATS * 4) as u64;
}

fn srgb(c: egui::Color32) -> [f32; 4] {
    [
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        c.a() as f32 / 255.0,
    ]
}

/// A feedback view's state: the ping-pong pair, and which way round it is.
///
/// A view holds one of these in egui memory under its own key, which supplies
/// the two things a `Shape::Callback` cannot: targets that **outlive the frame**
/// that asked for them, and a **parity** that alternates them.
///
/// The pair sits behind a `Mutex` because a texture can only be made with the
/// `Device`, and the only step that has one is the callback's `prepare`. The
/// parity is outside the lock because it is a plain CPU decision made before any
/// of that runs.
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
type Pair = (Arc<FeedbackTarget>, Arc<FeedbackTarget>);

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
    /// the same target — otherwise which half of the swap was source would
    /// depend on how many frames the previous view happened to draw.
    pub fn invalidate(&mut self) {
        *self.slot() = None;
        self.parity = false;
    }

    /// Queue this frame's two passes: `accumulate` into the destination, then
    /// `present` it onto the pane.
    ///
    /// A no-op on the first frame after a fresh, a resize or a view switch, since
    /// the targets do not exist yet. That frame shows the background the pane
    /// already painted, which is the honest cost of a target that can only be
    /// made where the `Device` is.
    /// The callback [`Self::draw`] queues, exposed so a test can drive it.
    ///
    /// The mirror of [`gpu::callback`] for the feedback half, and for the same
    /// reason: the test must drive the object the pane queues rather than a second
    /// copy of it. `pass` is `None` on the frame that is only asking for the pair
    /// to be created, and the two shader names travel *inside* a pass — they are
    /// what the callback draws, not what it is made of.
    pub fn callback(
        &self,
        helpers: &'static str,
        pass: Option<FeedbackPass>,
        wanted: (i32, i32),
        uniforms: Uniforms,
    ) -> FeedbackCallback {
        FeedbackCallback {
            slot: self.built.clone(),
            failed: self.failed.clone(),
            wanted,
            helpers,
            pass,
            uniforms,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        helpers: &'static str,
        accumulate: &'static str,
        present: &'static str,
        uniforms: Uniforms,
        advance: bool,
    ) {
        // A driver that cannot give us a render target is asked **once**. Without
        // this, a failure means two textures created and released 60 times a
        // second, for ever — the same class of runaway as the per-frame rebuild
        // this function used to do. The pipeline cache learned this lesson; the
        // feedback path did not.
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
            // Creation happens in the callback's `prepare`, which is the only
            // place a `Device` exists. Until then there is nothing to draw, and
            // the pane's own background stands in.
            painter.add(egui::Shape::Callback(
                egui_wgpu::Callback::new_paint_callback(
                    rect,
                    self.callback(helpers, None, wanted, uniforms),
                ),
            ));
            return;
        };
        // Parity only advances when something is written, so a held frame keeps
        // presenting the same target rather than alternating between the newest
        // picture and the one before it.
        let (src, dst) = if self.parity { (b, a) } else { (a, b) };
        if advance {
            self.parity = !self.parity;
        }

        painter.add(egui::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(
                rect,
                self.callback(
                    helpers,
                    Some(FeedbackPass::Feedback {
                        accumulate,
                        present,
                        src,
                        dst,
                        advance,
                    }),
                    wanted,
                    uniforms,
                ),
            ),
        ));
    }
}

/// The pixel size a feedback target should be, quantised.
///
/// **Physical pixels, not points.** The pane rect is in points and the screen is
/// in physical pixels, and on a HiDPI display those differ by the scale factor —
/// so sizing a render target from the rect alone gives a target a *quarter* of
/// the on-screen area at 2x, and the trail is drawn at a quarter resolution and
/// looks like a soft smear.
///
/// **Quantised, and the reason is churn.** A dock's pane rect jitters by
/// fractions of a pixel as a splitter settles, and an exact size check reads that
/// jitter as "wrong size" — so every jitter rebuilt both targets. Rounding to a
/// coarse grid means the size changes only when the pane really did, which is the
/// one event that should cost a rebuild.
///
/// Coarse rather than exact because the content is a soft accumulating trail: a
/// few pixels of upscale is invisible, whereas being wrong by a pixel every frame
/// is not.
pub fn target_size(rect: egui::Rect, pixels_per_point: f32) -> (i32, i32) {
    let quantise = |points: f32| {
        let cells = (points * pixels_per_point / TARGET_GRID as f32).ceil();
        // The floor is a whole grid cell, so a zero or negative extent — which a
        // pane really does report for a frame while a splitter is dragged —
        // cannot produce a zero-sized target, which wgpu rejects as
        // `INVALID_VALUE`. A driver reporting an error on a path nobody checks
        // leaves nothing in any log, so the symptom is a blank pane and nothing
        // else.
        ((cells as i32).max(1) * TARGET_GRID).max(TARGET_GRID)
    };
    (quantise(rect.width()), quantise(rect.height()))
}

/// The grid [`target_size`] rounds to, in physical pixels.
const TARGET_GRID: i32 = 64;

/// How much of the previous frame survives into this one, given a frame time.
///
/// **Time-based, not frame-counted**, and that is the whole point: a per-frame
/// multiplier is a fixed fraction per frame, so the trail is twice as long at
/// 30 Hz as at 60 Hz, and nothing in a screenshot would reveal it. This is the
/// same "inject the clock" discipline as `config::should_flush`.
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

// -- the WGSL half -------------------------------------------------------------
// Everything below this line touches wgpu. Nothing above it does.

/// The one vertex stage, shared by every look. A fullscreen triangle needs no
/// attributes, so there is no vertex buffer anywhere in this module.
const VERT: &str = r#"
struct VOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vi : u32) -> VOut {
    var out : VOut;
    // (0,0) (2,0) (0,2) -> the oversized triangle that covers clip space.
    let corner = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    out.pos = vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
    // 0..1 across the callback's OWN rect. The viewport is set to the pane, so
    // NDC -1..1 *is* the pane and `corner` is already that fraction — no uniform
    // and no per-fragment divide.
    //
    // **This varying exists because a pixel coordinate is window-relative, and
    // every view here is pane-relative.** `@builtin(position)` is window- and
    // device-relative; dividing it by the pane's size gives a uv that runs from
    // `pane_left / pane_w` to that plus one, so a dock tab anywhere but the left
    // edge of the window reads part of its pattern from outside itself and
    // saturates on the rest: the bars and the wave froze on a vertical seam at a
    // constant, and the rings drew off-centre because `0.5` is `pane_w / 2` of
    // *window* x. It is also **bottom-left origin**, which put the ridgeline
    // upside down.
    //
    // y is flipped here, once, because that is the one place the difference can
    // be stated once: every view reasons in the pane's top-down point space, and
    // `uv.y == 0.0` has to mean the top of the pane for all of them.
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}
"#;

/// The uniform block's WGSL declaration, in [`Uniforms::write_block`]'s order.
///
/// **Generated from the same constants the DSP uses, and every member is a
/// `vec4`** — see the module docs for why that is the load-bearing part. Both
/// arrays are `N / 4` long, which is why `VIZ_BANDS` and `WAVE_BUCKETS` must
/// both be multiples of four; a test says so rather than letting a future
/// constant produce a shader that silently reads the wrong lane.
fn wgsl_struct() -> String {
    format!(
        "struct Uniforms {{\n\
         \x20   bands      : array<vec4<f32>, {}>,\n\
         \x20   wave       : array<vec4<f32>, {}>,\n\
         \x20   accent     : vec4<f32>,\n\
         \x20   bg         : vec4<f32>,\n\
         \x20   progress   : vec4<f32>,\n\
         \x20   scalars    : vec4<f32>,\n\
         \x20   resolution : vec4<f32>,\n\
         \x20   modes      : vec4<f32>,\n\
         }}",
        VIZ_BANDS / 4,
        WAVE_BUCKETS / 4
    )
}

/// A shared helper brings its own inputs with it, so it is emitted in full and a
/// body mentioning it is enough.
const BEARING: &str = r#"
// A continuous, periodic read of the spectrum around the circle.
//
// `p` is the fragment's position relative to the pane's centre. The `max` guards
// that centre, where the length is zero and the division is undefined — and it is
// a `max` rather than an `if` because a guard has to be as continuous as the thing
// it guards: a branch here would put the discontinuity somewhere new.
fn spectrum_at_bearing(p : vec2<f32>) -> f32 {
    let dir = p / max(length(p), 0.12);
    var b0 = 0.0;
    var b1 = 0.0;
    var b2 = 0.0;
    for (var i = 0u; i < VIZ_BANDS; i = i + 1u) {
        let a = 6.2831853 * f32(i) / f32(VIZ_BANDS);
        let l = level(band_at(i));
        b0 = b0 + l * cos(a);
        b1 = b1 + l * sin(a);
        b2 = b2 + l * cos(2.0 * a);
    }
    let n = f32(VIZ_BANDS);
    return clamp(0.5 + (0.6 * (b0 * dir.x + b1 * dir.y) + 0.3 * b2) / n, 0.0, 1.0);
}
"#;

/// The complete shader module for one view: the shared prelude, then the body.
///
/// **The prelude is unconditional.** Under GLSL this filtered the declarations
/// down to whatever the body mentioned, because the compiler would have
/// stripped the rest anyway; under WGSL nothing is stripped, and a declared
/// binding is free. That deletes the filter *and* the class of bug it could
/// have — a body whose helper needed an input the filter did not know about.
pub fn wgsl_module(helpers: &str, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 2048);
    out.push_str(&wgsl_struct());
    out.push('\n');
    out.push_str(
        "@group(0) @binding(0) var<uniform> U : Uniforms;\n\
         @group(0) @binding(1) var u_samp : sampler;\n\
         @group(0) @binding(2) var u_prev : texture_2d<f32>;\n",
    );
    out.push('\n');
    // The dB floor is derived from the DSP's own constant rather than written
    // out, so a floor that stops being half the ceiling cannot leave a literal
    // span behind.
    out.push_str(&format!(
        "const DB_FLOOR : f32 = {DB_FLOOR};\n\
         const DB_SPAN : f32 = {};\n\
         const VIZ_BANDS : u32 = {}u;\n\
         const WAVE_BUCKETS : u32 = {}u;\n\
         \n\
         fn level(d : f32) -> f32 {{\n    return clamp((d - DB_FLOOR) / DB_SPAN, 0.0, 1.0);\n}}\n",
        -DB_FLOOR, VIZ_BANDS, WAVE_BUCKETS
    ));
    out.push_str(BEARING);
    out.push('\n');
    out.push_str(
        "// One accessor per input, so a view never learns the block's shape.\n\
         // The arrays are `vec4`-packed: see the module docs on the uniform\n\
         // stride, which is why indexing is a shift and a mask.\n\
         fn band_at(i : u32) -> f32 { return U.bands[i >> 2u][i & 3u]; }\n\
         fn wave_at(i : u32) -> f32 { return U.wave[i >> 2u][i & 3u]; }\n\
         fn u_accent() -> vec4<f32> { return U.accent; }\n\
         fn u_bg() -> vec4<f32> { return U.bg; }\n\
         fn u_progress_fill() -> vec4<f32> { return U.progress; }\n\
         fn u_time() -> f32 { return U.scalars.x; }\n\
         fn u_dt() -> f32 { return U.scalars.y; }\n\
         fn u_feedback() -> f32 { return U.scalars.z; }\n\
         fn u_resolution() -> vec2<f32> { return U.resolution.xy; }\n\
         fn u_modes() -> vec2<f32> { return U.modes.xy; }\n",
    );
    out.push('\n');
    out.push_str(VERT);
    out.push('\n');
    // A view's own module-scope functions, which WGSL has no way to nest inside
    // the entry point — so they are a separate slot rather than something the
    // prelude knows about. The harness concatenates; it never reads them.
    if !helpers.is_empty() {
        out.push_str(helpers);
        out.push('\n');
    }
    // The body assigns `frag_color` and reads `v_uv`, both declared here so a
    // view writes neither a signature nor an output declaration. `@location(0)`
    // is the swapchain; the two passes of a feedback view use the same module and
    // the same entry point, differing only in what they sample.
    out.push_str(
        "@fragment\n\
         fn fs_main(in : VOut) -> @location(0) vec4<f32> {\n\
         \x20   let v_uv = in.uv;\n\
         \x20   var frag_color : vec4<f32> = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n",
    );
    out.push_str(body);
    out.push_str("\n    return frag_color;\n}\n");
    out
}

/// The complete module, for a test that validates it the way the driver will.
#[allow(dead_code)]
pub fn module_source(helpers: &str, body: &str) -> String {
    wgsl_module(helpers, body)
}

/// Where a pipeline, the bind group layout and the one shared sampler live.
///
/// **A static rather than the renderer's `callback_resources`,** and the
/// difference is lifetime: `callback_resources` is dropped and rebuilt with the
/// render pass, which is right for a pass-scoped object and wrong for a pipeline
/// that is expensive to make and identical every time. A `RenderPipeline` is a
/// refcounted handle inside wgpu, so holding one for the process costs a
/// refcount, and the trade is one shader compilation per view per session.
struct Gpu {
    device: wgpu::Device,
    /// **Kept because wgpu 30 moved it off `Device`**, and the uniform upload
    /// needs it. It is here for the cached-buffer rewrite in `prepare`; the
    /// callback's own `queue` argument would do for a fresh buffer per frame.
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// A 1x1 texture for the views that sample nothing, so one bind group
    /// layout serves every shader.
    blank: wgpu::TextureView,
    /// Keyed by body text: the pipeline is a pure function of it, and two views
    /// with identical shaders correctly share one.
    /// Keyed by body *and* the format it was built for. A feedback view's
    /// accumulate pass renders into an owned `Rgba8Unorm` target while everything
    /// else renders into the surface, and a wgpu pipeline carries its colour
    /// target format — so one body is one pipeline only because no body is ever
    /// drawn to both. Keying on the format makes that true by construction rather
    /// than by an assumption nobody would notice breaking.
    pipelines: HashMap<(&'static str, &'static str, wgpu::TextureFormat), Option<Entry>>,
}

/// A built shader, plus the uniform buffer it reads.
///
/// **The buffer is cached beside the pipeline and rewritten each frame, not
/// rebuilt.** `add_fullscreen` constructs a *new* callback object every frame, so
/// nothing else could outlive a frame — and a ~750-byte buffer allocated 60 times
/// a second is real driver work.
///
/// That means the buffer is per *body*, so two views sharing a shader body would
/// share a buffer. They cannot both draw in one frame: the pane draws only the
/// selected view, and it is a single ComboBox. The invariant is stated here
/// because it is the whole reason this is safe, and because a second view
/// drawing beside it is the change that would break it.
struct Entry {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
}

static GPU: LazyLock<Mutex<Option<Gpu>>> = LazyLock::new(|| Mutex::new(None));

/// Give the harness the device and the surface format, once per process.
///
/// **Called from `main.rs`, because that is the only place with a
/// `CreationContext`** — and the surface format is not something a callback can
/// discover: a `RenderPipeline` has to name the format of the attachment it will
/// draw into, and `prepare` is handed a size and a scale factor and nothing
/// else. Everything else about the callback API is self-sufficient; this is the
/// one hole in it.
///
/// Returns whether there is a wgpu renderer to draw with. False means the
/// program runs and every view shows the pane's background, with the reason on
/// stderr — which is the honest outcome for a machine with no wgpu adapter, and
/// not something to crash over.
/// Take the three things the harness needs, and nothing else.
///
/// The surface's **format** is the one input the callback API cannot hand a view:
/// a `PaintCallback` is told its rect in points and nothing about the surface it
/// will be composited onto. That is why this function exists at all, and why it
/// takes three arguments rather than the whole `RenderState` — the rest of that
/// struct (an `Arc<RwLock<Renderer>>`, the adapter list, the surface config) is
/// eframe's business, and depending on it would make this unreachable from a test
/// that has no window and no `Renderer`.
///
/// So the caller reports whether there is a backend at all, and this says how to
/// set up given one.
pub fn init(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat) {
    // Sampled with a filter: a feedback view renders at the quantised target size
    // and presents it across a pane that is rarely exactly that size, so the
    // present pass is a linear upsample and a blocky one would read as a
    // staircase. Clamped, because the accumulate pass samples outside its own
    // edges.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("tplay.viz.sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let fragment = |binding: u32, vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: match binding {
            0 => wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                // The block's real size, so wgpu checks the buffer against the
                // shader rather than trusting this number.
                min_binding_size: wgpu::BufferSize::new(Uniforms::BLOCK_BYTES),
            },
            1 => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            _ => wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tplay.viz.layout"),
        entries: &[
            fragment(0, wgpu::ShaderStages::FRAGMENT),
            fragment(1, wgpu::ShaderStages::FRAGMENT),
            fragment(2, wgpu::ShaderStages::FRAGMENT),
        ],
    });

    let blank = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("tplay.viz.blank"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let mut gpu = GPU.lock().unwrap_or_else(PoisonError::into_inner);
    *gpu = Some(Gpu {
        device: device.clone(),
        queue: queue.clone(),
        target_format,
        layout,
        sampler,
        blank,
        pipelines: HashMap::new(),
    });
}

/// The format a feedback target is created in.
///
/// **`Rgba8Unorm`, not `Rgba8UnormSrgb`,** and that is deliberate: the shaders
/// write straight (non-premultiplied) sRGB components, which is what the GL path
/// did into a framebuffer treated as linear storage. An `-Srgb` view would have
/// the sampler and the blend apply a linear→sRGB conversion the GL path never
/// did, and every colour would come out wrong in a way no test can see.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The pipeline for one shader body, built once.
///
/// A `None` is a **recorded** failure rather than a miss: a driver that cannot
/// compile would otherwise be asked again 60 times a second, and a compile is a
/// driver round trip and a log line. This is also why the blank pane was such a
/// bad symptom to debug by eye: a broken shader and a working one both render as
/// the background, so the failure is indistinguishable from the success case by
/// anything you can see. `eprintln!` goes to stderr, which is where to look.
fn entry(
    gpu: &mut Gpu,
    helpers: &'static str,
    body: &'static str,
    format: wgpu::TextureFormat,
) -> Option<(wgpu::RenderPipeline, wgpu::Buffer)> {
    let key = (helpers, body, format);
    if !gpu.pipelines.contains_key(&key) {
        let built = build_entry(gpu, helpers, body, format);
        gpu.pipelines.insert(key, built);
    }
    // Cloned out so the `&mut` borrow of the cache ends here: the caller still
    // needs the sampler, the blank view and the queue. Both handles are
    // refcounts, so this costs nothing measurable.
    gpu.pipelines
        .get(&key)
        .and_then(|e| e.as_ref())
        .map(|e| (e.pipeline.clone(), e.buffer.clone()))
}

fn build_entry(
    gpu: &Gpu,
    helpers: &'static str,
    body: &'static str,
    format: wgpu::TextureFormat,
) -> Option<Entry> {
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tplay.viz.shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl_module(helpers, body).into()),
        });
    let layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tplay.viz.pipeline_layout"),
            bind_group_layouts: &[Some(&gpu.layout)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tplay.viz.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // **No blend.** The pane's background is painted by egui and the
                    // view covers it outright, so a shader that returned a partly
                    // transparent colour would composite against nothing.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
    // A `None` here is a *recorded* failure: `create_render_pipeline` reports
    // through wgpu's error scope and a panic rather than a value, so the real
    // check is `every_shader_validates`, which runs naga over the same source and
    // names the shader. This function exists to be told "no" at all.
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tplay.viz.uniforms"),
        size: Uniforms::BLOCK_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Some(Entry { pipeline, buffer })
}

/// The bind group for one draw, rebuilt each frame.
///
/// Cheap and obviously correct: a bind group embeds the buffer it reads, so
/// caching one would mean caching the buffer with it, and the buffer's contents
/// are this frame's band levels.
fn bind_group(gpu: &Gpu, buffer: &wgpu::Buffer, sampled: &wgpu::TextureView) -> wgpu::BindGroup {
    gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tplay.viz.bind_group"),
        layout: &gpu.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&gpu.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(sampled),
            },
        ],
    })
}

/// The wgpu viewport that makes a fullscreen triangle cover **the pane**.
///
/// The vertex stage has no vertex buffer — three synthesised corners — so the
/// *viewport* is the only thing that decides which fragments it reaches. Left at
/// egui_wgpu's whole-surface viewport, every view paints over whatever else is on
/// screen, which is exactly what this pane did: a shader that renders somewhere
/// plausible is the failure mode it has already had twice (see
/// `no_shader_reads_a_window_relative_pixel_coordinate`).
///
/// **The trap is `PaintCallbackInfo::viewport_in_pixels`, and it is a trap because
/// the method is right and the use is wrong.** It reports `from_bottom_px`, which
/// is OpenGL's convention; wgpu's viewport origin is the **top** left. Fed to
/// `set_viewport` it puts the pane a window-height off and mirrored — a real
/// report, a real off-screen render, and no error anywhere. `info.viewport` is the
/// same rect in **egui points, y down from the top**, which is the space wgpu
/// wants, so the whole conversion is a scale by `pixels_per_point`.
///
/// A free function rather than three lines at each call site, because this is the
/// one piece of the port that no headless test can reach and the one the last two
/// bugs lived in — see `the_pane_viewport_is_top_left_pixels_not_gl_bottom_left`.
///
/// Returns `(x, y, width, height)`, all in physical pixels.
pub fn pane_viewport(viewport: egui::Rect, pixels_per_point: f32) -> [f32; 4] {
    [
        viewport.min.x * pixels_per_point,
        viewport.min.y * pixels_per_point,
        viewport.width() * pixels_per_point,
        viewport.height() * pixels_per_point,
    ]
}

/// Push this frame's block into the cached buffer for `body`.
///
/// `bytemuck` is not a dependency and this is one line: a uniform buffer is a
/// flat run of little-endian `f32`, and `f32::to_le_bytes` is that.
fn upload(gpu: &Gpu, buffer: &wgpu::Buffer, uniforms: &Uniforms) {
    let block = uniforms.write_block();
    debug_assert_eq!((block.len() * 4) as u64, Uniforms::BLOCK_BYTES);
    let bytes: Vec<u8> = block.iter().flat_map(|f| f.to_le_bytes()).collect();
    gpu.queue.write_buffer(buffer, 0, &bytes);
}

/// Queue `body` to fill `rect` this frame.
///
/// The uniform upload happens here rather than in the callback so that the WGSL
/// block and the values that fill it are described by the same struct — a
/// mismatch is then a test failure instead of a wrong picture.
pub fn add_fullscreen(
    painter: &egui::Painter,
    rect: egui::Rect,
    helpers: &'static str,
    body: &'static str,
    uniforms: Uniforms,
) {
    painter.add(egui::Shape::Callback(
        egui_wgpu::Callback::new_paint_callback(rect, callback(helpers, body, uniforms)),
    ));
}

/// The callback [`add_fullscreen`] queues, exposed so a test can drive it.
///
/// A `pub fn` rather than a `#[cfg(test)]` hook: the test needs the *same* object
/// the pane queues, and a second construction site would be exactly the drift the
/// view table exists to prevent. It is inert on its own — a callback does nothing
/// until a renderer calls it.
pub fn callback(helpers: &'static str, body: &'static str, uniforms: Uniforms) -> VizCallback {
    VizCallback {
        helpers,
        body,
        uniforms,
    }
}

/// What this frame's callback draws: a feedback accumulate followed by a present.
pub enum FeedbackPass {
    Feedback {
        accumulate: &'static str,
        present: &'static str,
        src: Arc<FeedbackTarget>,
        dst: Arc<FeedbackTarget>,
        /// Record this frame's audio into the target, or hold the picture.
        ///
        /// **The accumulate is the only thing here that moves.** The present pass
        /// just samples what is already in the target, so skipping the accumulate
        /// leaves the last frame on screen — which is what pausing is supposed to
        /// look like. Gating the whole callback instead, at the pane, was the
        /// obvious thing and it is wrong: the pane goes blank, so the view reads
        /// as *gone* rather than as *held*.
        advance: bool,
    },
}

/// The callback for a view that draws straight to the screen. See
/// [`callback`].
pub struct VizCallback {
    helpers: &'static str,
    body: &'static str,
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for VizCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Ok(mut gpu) = GPU.lock() else {
            return Vec::new();
        };
        let Some(gpu) = gpu.as_mut() else {
            return Vec::new();
        };
        // The upload happens here, where the queue is, and the draw in `paint`.
        // This is also the one step that builds a pipeline, so a driver that
        // cannot compile is asked once rather than sixty times a second.
        if let Some((_, buffer)) = entry(gpu, self.helpers, self.body, gpu.target_format) {
            upload(gpu, &buffer, &self.uniforms);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let Ok(mut gpu) = GPU.lock() else {
            return;
        };
        let Some(gpu) = gpu.as_mut() else {
            return;
        };
        let Some((pipeline, buffer)) = entry(gpu, self.helpers, self.body, gpu.target_format)
        else {
            return;
        };
        // **The viewport is the pane, and the scissor is left entirely alone.**
        // Both halves of that are argued on [`pane_viewport`]; what belongs here is
        // only that egui_wgpu has already clipped to `info.clip_rect` for this
        // primitive, and it owns resetting the viewport and scissor for the next
        // one.
        let [x, y, w, h] = pane_viewport(info.viewport, info.pixels_per_point);
        render_pass.set_viewport(x, y, w, h, 0.0, 1.0);
        let bind_group = bind_group(gpu, &buffer, &gpu.blank);
        render_pass.set_pipeline(&pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        // The oversized triangle: three vertices, no index buffer, no attributes.
        render_pass.draw(0..3, 0..1);
    }
}

/// Must this pair be (re)built at `wanted`?
///
/// **Both halves matter and the second one is the bug this exists for.** A pair
/// that is *absent* must be created; so must one whose targets are not `wanted`,
/// because the pane sized the request from the rect it is drawing into and a
/// texture of a different size cannot be drawn into. Testing only for absence
/// meant a resized pane kept a pair at the old size, `Feedback::draw` went on
/// queueing `pass: None`, and the view never drew again — permanently blank, with
/// no error and no log line, after any resize.
///
/// Free and `Pair`-free so a test can reach it: it is a question about two pairs of
/// integers, and the reason it was wrong for a release is that nothing could ask
/// it.
pub fn pair_is_unusable(pair: Option<((i32, i32), (i32, i32))>, wanted: (i32, i32)) -> bool {
    match pair {
        None => true,
        Some((a, b)) => a != wanted || b != wanted,
    }
}

/// The callback for a feedback view's two passes.
pub struct FeedbackCallback {
    slot: Arc<Mutex<Option<Pair>>>,
    failed: Arc<AtomicBool>,
    wanted: (i32, i32),
    /// The view's module-scope functions, shared by both passes.
    helpers: &'static str,
    /// `None` on the frame that creates the pair, which has nothing to draw yet.
    pass: Option<FeedbackPass>,
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for FeedbackCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Ok(mut gpu) = GPU.lock() else {
            return Vec::new();
        };
        let Some(gpu) = gpu.as_mut() else {
            return Vec::new();
        };

        // The frame that was queued because no pair of the right size exists.
        // Creating it is here rather than in a `create` step of its own because
        // this is the only place a `Device` is available, and wgpu releases the old
        // pair when it is replaced — so there is no destroy call and no `Drop` to
        // get wrong.
        //
        // **The size is part of the condition, and it is the whole fix.** This
        // asked only whether the slot was empty, so after a resize the pair was
        // still there — at the old size — the check passed, nothing was rebuilt,
        // and `Feedback::draw` kept queueing `pass: None` for ever. The pane went
        // blank on every resize and never came back, with no error anywhere: the
        // view drew nothing because the callback it queued could not draw.
        //
        // It reads as a rendering fault and is a lifetime one. The size compare is
        // the same one `Feedback::draw` uses to decide the pair is unusable, so the
        // two cannot disagree about what "the right size" means.
        if self.pass.is_none() {
            let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            if pair_is_unusable(
                slot.as_ref().map(|(a, b)| (a.size(), b.size())),
                self.wanted,
            ) {
                match (
                    FeedbackTarget::new(&gpu.device, self.wanted),
                    FeedbackTarget::new(&gpu.device, self.wanted),
                ) {
                    (Some(a), Some(b)) => *slot = Some((Arc::new(a), Arc::new(b))),
                    _ => self.failed.store(true, Ordering::Relaxed),
                }
            }
            return Vec::new();
        }

        let Some(FeedbackPass::Feedback {
            accumulate,
            src,
            dst,
            advance,
            ..
        }) = &self.pass
        else {
            return Vec::new();
        };
        // A fresh target holds undefined contents, so it is cleared before the
        // first accumulate rather than after. One frame of undefined texture
        // otherwise shows as a flash of garbage.
        //
        // **Cleared to the pane's background, not to black.** Both feedback views
        // present their target directly, so a black clear means an unwarmed
        // history reads as a black hole in the pane — and the spectrogram cannot
        // paper over it in the shader either, because a floor below 1.0 that
        // hides black also washes the background into every column of the
        // history, which is a blur of its own. Clearing to `u_bg` lets the
        // present pass be a straight read-back at full strength.
        //
        // The pass ends when `rp` is dropped — wgpu 30 has no `end()` to call.
        if dst.needs_clear() {
            let bg = wgpu::Color {
                r: self.uniforms.bg[0] as f64,
                g: self.uniforms.bg[1] as f64,
                b: self.uniforms.bg[2] as f64,
                a: 1.0,
            };
            let rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tplay.viz.clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(bg),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            drop(rp);
            dst.mark_drawn();
        }

        // **After** the clear, not before it: a fresh target is undefined, and
        // the present pass samples it whether or not anything was recorded. Putting
        // the gate here rather than around the accumulate pass also skips the
        // pipeline lookup and the uniform upload, which are pure waste on a frame
        // that records nothing.
        if !advance {
            return Vec::new();
        }

        let Some((pipeline, buffer)) = entry(gpu, self.helpers, accumulate, OFFSCREEN_FORMAT)
        else {
            return Vec::new();
        };
        upload(gpu, &buffer, &self.uniforms);

        // The accumulate pass draws into the offscreen target, so its viewport
        // is the target's own extent — an offscreen render pass sizes its own
        // frame, and the pane's window rectangle is meaningless in it. The quantised
        // target is then a little larger than the pane, and the present pass
        // upscales across it.
        let bind_group = bind_group(gpu, &buffer, &src.view);
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tplay.viz.accumulate"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // No viewport and no scissor: a fresh pass already covers the whole
        // attachment, which *is* the target. The pane's screen rect has no meaning
        // here — this pass fills its own texture, and the present pass is what
        // places it.
        rp.set_pipeline(&pipeline);
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..3, 0..1);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let Ok(mut gpu) = GPU.lock() else {
            return;
        };
        let Some(gpu) = gpu.as_mut() else {
            return;
        };
        let Some(FeedbackPass::Feedback { present, dst, .. }) = &self.pass else {
            return;
        };
        let Some((pipeline, buffer)) = entry(gpu, self.helpers, present, gpu.target_format) else {
            return;
        };
        // Same top-left conversion as `VizCallback::paint`, and the scissor is
        // again egui's: it is the *clip* that limits this pass, and egui has
        // already applied it.
        let [x, y, w, h] = pane_viewport(info.viewport, info.pixels_per_point);
        render_pass.set_viewport(x, y, w, h, 0.0, 1.0);
        // The destination is the sampler here: the present pass reads back
        // exactly what the accumulate pass wrote. Its own uniform buffer, because
        // it is a different shader with a different `Entry`.
        upload(gpu, &buffer, &self.uniforms);
        let bind_group = bind_group(gpu, &buffer, &dst.view);
        render_pass.set_pipeline(&pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// An owned RGBA8 render target and the view that samples it.
///
/// **No `Send`/`Sync` override to justify here:** wgpu's `Texture` is already
/// both. The GL version of this type needed `unsafe impl Send/Sync` for exactly
/// that reason, which is what a backend change here buys.
pub struct FeedbackTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (i32, i32),
    /// Set once this target has been rendered into, so a first frame starts from
    /// a defined state rather than from whatever was in a fresh texture.
    drawn: AtomicBool,
}

impl FeedbackTarget {
    fn new(device: &wgpu::Device, size: (i32, i32)) -> Option<Self> {
        let (w, h) = size;
        // `size` must come from [`target_size`], which floors it — see there for
        // why a zero dimension is the failure mode worth designing out.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tplay.viz.target"),
            size: wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Some(Self {
            texture,
            view,
            size,
            drawn: AtomicBool::new(false),
        })
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
}

impl std::fmt::Debug for FeedbackTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedbackTarget")
            .field("texture", &self.texture)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}
