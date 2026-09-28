//! Chladni3D view — the Chladni figure as a raymarched plate, rather than as a
//! contour on a flat grid.
//!
//! It sits **beside** `chladni`, not in place of it, and that is the interesting
//! part: the two views share the same field, the same mode numbers and the same
//! hysteretic pick, and differ only in what they do with them. The wireframe view
//! draws the nodal set as marching-squares segments, which is a *contour* — a
//! line where the field crosses a threshold. This one marches the field through
//! a volume and accumulates density, which is what sand in a real plate does.
//! Neither is the other with different colours, so retiring either would lose a
//! read rather than gain one.
//!
//! **The mode numbers are not recomputed here.** `pick_mode` moved to
//! `audio::viz` for exactly this reason: it is a pure function of the band array
//! plus the currently held pair, and having it in a view file would have made one
//! view depend on another. What this view owns is only the *state* — the held
//! pair and its countdown, under its own egui-memory key.

use crate::audio::viz::{compute_bands, pick_mode, VizBuf, HOLD_FRAMES, VIZ_BANDS};
use crate::gui::panes::visualizer::gpu::{self, Target};
use crate::gui::theme::Palette;
use eframe::egui;

fn prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev.chladni3d")
}

/// Deliberately not `chladni.rs`'s key. Two views sharing one mode key means
/// switching from one to the other starts this view off holding a figure it never
/// picked — and `pick_mode` would then treat it as a deliberate choice, so the
/// first challenger within `MARGIN_DB` would be ignored and the plate shown would
/// be the other view's. Nothing about that is visible: both views animate, and
/// the symptom is a figure that is wrong for a second and a half.
fn mode_id() -> egui::Id {
    egui::Id::new("tplay.viz.chladni3d.mode")
}

pub const FRAG: &str = r#"
// The plate as a fraction of the pane's short side, matching `chladni.rs`'s
// PLATE_FRAC. A square, because stretching it distorts the figure's symmetry,
// which is most of the content. Written here rather than interpolated: a value a
// shader needs to agree with a CPU view is two literals, and making it a uniform
// would spend a slot every other look then has to share.
const float PLATE_FRAC = 0.94;

// The nodal set, per the same field the CPU view samples — |sin(pi n x) sin(pi m y)
// - sin(pi m x) sin(pi n y)| — read as a *density* rather than as a threshold.
// `u_modes` is the (n, m) pair `pick_mode` chose on the CPU, which is where the
// hysteresis lives; the GPU is only asked to draw the figure that pick names.
float field(vec3 p) {
    float n = u_modes.x;
    float m = u_modes.y;
    float px = 3.14159265 * n * p.x;
    float py = 3.14159265 * n * p.y;
    float qx = 3.14159265 * m * p.x;
    float qy = 3.14159265 * m * p.y;
    return abs(sin(px) * sin(qy) - sin(qx) * sin(py));
}

// A height march rather than a volume march. The field is a function of (x, y)
// only, so the third axis is a thin slab standing in for the plate's thickness
// — marching a genuine volume would be 48 steps of a function that does not vary
// along z, which is the same picture for three times the work.
float slab(vec2 xy) {
    float acc = 0.0;
    // Six slices. The nodal set is smooth enough that this resolves it; the CPU
    // view needs 48x48 marching-squares cells for the same figure, and this is
    // the per-fragment equivalent of a coarser grid than that.
    for (int i = 0; i < 6; i++) {
        float z = (float(i) + 0.5) / 6.0 - 0.5;
        acc += field(vec3(xy, z));
    }
    return acc / 6.0;
}

void main() {
    vec2 uv = gl_FragCoord.xy / u_resolution;
    vec2 centred = (uv - 0.5) * vec2(u_resolution.x / max(u_resolution.y, 1.0), 1.0);

    // The plate, square and centred, the same geometry `segments` uses.
    float size = PLATE_FRAC;
    vec2 plate = centred / size * 0.5;
    if (abs(plate.x) > 1.0 || abs(plate.y) > 1.0) {
        // Off the plate. Painting the background token rather than discarding, so
        // the shader owns the whole rect and there is no hard edge to alias at.
        frag_color = vec4(u_bg.rgb, 1.0);
        return;
    }

    // The field is antisymmetric, so it is zero along the diagonal — that line is
    // the figure's mirror symmetry and it is where the sand collects first.
    float d = slab(plate);
    float nodal = 1.0 - smoothstep(0.0, 0.10, d);

    // A second, wider band around the nodal line. One threshold alone gives a
    // hard 1px line; the falloff is what reads as a settled ridge, and it is
    // free here because the field is already evaluated.
    float glow = 1.0 - smoothstep(0.0, 0.42, d);

    // The same body/edge split bars.rs and the shader views make: a dim mass in
    // the accent, the sharp nodal set in the brighter token.
    vec3 col = mix(u_bg.rgb, u_accent.rgb, glow * 0.30);
    col = mix(col, u_progress_fill.rgb, nodal);

    // A fade at the plate's edge, so the figure does not end on a hard rectangle.
    // There is no animation in this view: the plate is a *figure*, and it changes
    // when `pick_mode` says it does, which is the same contract the wireframe view
    // has. Rotating it would make the mirror symmetry unreadable, and that
    // symmetry is most of what the figure is.
    float edge = 1.0 - smoothstep(0.92, 1.0, max(abs(plate.x), abs(plate.y)));
    col = mix(u_bg.rgb, col, edge);
    frag_color = vec4(col, 1.0);
}
"#;

pub fn draw(painter: &egui::Painter, rect: egui::Rect, viz: &VizBuf, palette: &Palette) {
    let mut prev: [f32; VIZ_BANDS] = painter.ctx().memory_mut(|m| {
        m.data
            .get_temp::<[f32; VIZ_BANDS]>(prev_id())
            .unwrap_or([-60.0; VIZ_BANDS])
    });

    // The same constants as `chladni.rs`, and for the same reason: the smoothing
    // is a real-time filter on the spectrum, and the stickiness is `pick_mode`'s
    // margin and hold. Deliberately unpinned — both 0.08 and 0.3 converge within
    // a few dozen frames of a hard step, so a test over these numbers would pass
    // either way and prove nothing.
    const ATTACK: f32 = 0.3;
    const RELEASE: f32 = 0.6;
    compute_bands(viz, &mut prev, ATTACK, RELEASE);

    let held: Option<(usize, usize, u32)> =
        painter.ctx().memory_mut(|m| m.data.get_temp(mode_id()));
    let (current, hold) = match held {
        Some((n, m, hold)) => (Some((n, m)), hold.saturating_sub(1)),
        None => (None, 0),
    };
    let (n, m) = pick_mode(&prev, current, hold);
    let switched = Some((n, m)) != current;
    painter.ctx().memory_mut(|mem| {
        mem.data.insert_temp(prev_id(), prev);
        mem.data
            .insert_temp(mode_id(), (n, m, if switched { HOLD_FRAMES } else { hold }));
    });

    let mut uniforms = gpu::Uniforms::pack(viz, prev, palette, rect, painter.ctx());
    uniforms.modes = [n as f32, m as f32];
    gpu::add_fullscreen(painter, rect, FRAG, uniforms, Target::Screen);
}
