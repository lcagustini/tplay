use crate::app::TPlayApp;
use crate::audio::viz::{compute_bands, compute_wave, VIZ_BANDS};
use eframe::egui;

/// egui Id for storing the previous smoothed band values
fn viz_prev_id() -> egui::Id {
    egui::Id::new("tplay.viz.prev")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VizMode { Bars, Wave }

pub fn visualizer_pane(app: &mut TPlayApp, ui: &mut egui::Ui) {
    let theme = app.theme().clone();
    let p = theme.palette;
    let layout = theme.layout.with_defaults();

    // Mode — persisted in config.json via `TPlayApp::viz_wave`/`set_viz_wave`.
    let mut mode = if app.viz_wave() { VizMode::Wave } else { VizMode::Bars };

    // Previous band values for smoothing (stored in egui memory)
    let mut prev_bands: [f32; VIZ_BANDS] = ui.ctx().memory_mut(|m| {
        m.data.get_temp::<[f32; VIZ_BANDS]>(viz_prev_id()).unwrap_or([-60.0; VIZ_BANDS])
    });

    // Header with mode toggle
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Visualizer")
                .strong()
                .color(p.text_primary)
                .font(egui::FontId::new(layout.text_meta, theme.metadata_font.clone())),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .selectable_label(matches!(mode, VizMode::Bars), "Bars")
                .clicked()
            {
                app.set_viz_wave(false);
                mode = VizMode::Bars;
            }
            if ui
                .selectable_label(matches!(mode, VizMode::Wave), "Wave")
                .clicked()
            {
                app.set_viz_wave(true);
                mode = VizMode::Wave;
            }
        });
    });
    ui.add_space(4.0);

    // Compute visualization data
    let viz = app.viz();
    match mode {
        VizMode::Bars => {
            // Attack/release constants (per-frame, 60 FPS assumed)
            const ATTACK: f32 = 0.3;
            const RELEASE: f32 = 0.92;
            compute_bands(viz, &mut prev_bands, ATTACK, RELEASE);
        }
        VizMode::Wave => {
            // For wave mode, decay prev_bands toward zero so bars don't linger
            for v in &mut prev_bands {
                *v = (*v * 0.95).max(-60.0);
            }
        }
    }

    // Persist prev_bands for next frame
    ui.ctx().memory_mut(|m| m.data.insert_temp(viz_prev_id(), prev_bands));

    // Draw the visualization
    let rect = ui.available_rect_before_wrap();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let painter = ui.painter();

    // Background
    painter.rect_filled(rect, 0.0, p.bg);

    match mode {
        VizMode::Bars => draw_bars(painter, &rect, &prev_bands, &p),
        VizMode::Wave => draw_wave(painter, &rect, viz, &p),
    }
}

fn draw_bars(painter: &egui::Painter, rect: &egui::Rect, bands: &[f32; VIZ_BANDS], palette: &crate::gui::theme::Palette) {
    let n = bands.len();
    let bar_w = (rect.width() / n as f32).max(1.0);
    let mid_y = rect.center().y;
    let max_h = rect.height() * 0.45; // leave margins top/bottom

    for (i, &db) in bands.iter().enumerate() {
        // Map -60..0 dB to 0..1
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let h = level * max_h;

        let x = rect.left() + i as f32 * bar_w;
        let w = (bar_w * 0.8).max(1.0);
        let gap = bar_w - w;

        // Mirrored: draw up and down from center
        let top = mid_y - h;
        let bottom = mid_y + h;

        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(x + gap * 0.5, top),
            egui::pos2(x + gap * 0.5 + w, bottom),
        );

        // Gradient-like: accent at center, fading toward edges
        let alpha = (0.4 + 0.6 * level).clamp(0.0, 1.0);
        let color = egui::Color32::from_rgba_unmultiplied(
            palette.accent.r(),
            palette.accent.g(),
            palette.accent.b(),
            (alpha * 255.0) as u8,
        );
        painter.rect_filled(bar_rect, 1.0, color);

        // Subtle center line
        if i % 4 == 0 {
            painter.hline(
                x + gap * 0.5..=x + gap * 0.5 + w,
                mid_y,
                egui::Stroke::new(0.5_f32, palette.accent.gamma_multiply(0.5)),
            );
        }
    }
}

fn draw_wave(painter: &egui::Painter, rect: &egui::Rect, viz: &crate::audio::viz::VizBuf, palette: &crate::gui::theme::Palette) {
    // ~2px columns read as a solid filled silhouette, not bars.
    const STRIP_W: f32 = 2.0;
    let n = (rect.width() / STRIP_W).ceil().max(2.0) as usize;
    let wave = compute_wave(viz, n);
    let bar_w = rect.width() / n as f32;

    let mid_y = rect.center().y;
    let amp = rect.height() * 0.4;

    // Fill: one adjacent column per envelope bucket, top and bottom mirrored.
    let fill_color = egui::Color32::from_rgba_unmultiplied(
        palette.accent.r(),
        palette.accent.g(),
        palette.accent.b(),
        80,
    );
    for i in 0..n {
        let x = rect.left() + i as f32 * bar_w;
        let y = wave[i] * amp;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, mid_y - y),
                egui::pos2(x + bar_w, mid_y + y),
            ),
            0.0,
            fill_color,
        );
    }

    // Outline along the upper and lower envelope curves
    let stroke = egui::Stroke::new(1.5_f32, palette.progress_fill);
    for i in 0..n - 1 {
        let x0 = rect.left() + i as f32 * bar_w;
        let x1 = x0 + bar_w;
        let y0 = wave[i] * amp;
        let y1 = wave[i + 1] * amp;
        painter.line_segment([egui::pos2(x0, mid_y - y0), egui::pos2(x1, mid_y - y1)], stroke);
        painter.line_segment([egui::pos2(x0, mid_y + y0), egui::pos2(x1, mid_y + y1)], stroke);
    }
}

impl Default for VizMode {
    fn default() -> Self {
        VizMode::Bars
    }
}