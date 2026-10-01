#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod gui;
mod library;
mod library_db;
mod network;
mod playlist;
mod tracks;

use eframe::egui;
use gui::theme::{ThemeState, Themes};
use rodio::{cpal::BufferSize, DeviceSinkBuilder, MixerDeviceSink};

/// The shell: everything `eframe` needs, wrapping the app.
///
/// `TPlayApp` is pure state + logic — it holds no `egui::Context` and owns no
/// theme, so it cannot be the `eframe::App`. This struct owns the two things
/// that are genuinely UI, and drives one frame in the order that matters:
/// draw first (so a click is reflected in the same frame), then let the app
/// advance, drain and persist, then repaint if anything is still in flight.
///
/// Splitting it this way is what lets `app.rs` depend on nothing but the audio,
/// config, library, network and track modules — see `AGENTS.md`.
struct TPlay {
    app: app::TPlayApp,
    /// Active theme + loadable list + decoded icons. A switch re-decodes the
    /// icon textures, which needs the `Context` — hence the theme living here
    /// rather than in the app.
    themes: ThemeState,
    /// The device-backed sink, kept alive and never read. The app holds a
    /// `Mixer` (an `Arc` clone of this one's), which is all it needs to attach
    /// sinks; but nothing *pulls* that mixer except this one's cpal callback, so
    /// dropping it would silently freeze playback rather than error.
    _output: MixerDeviceSink,
    /// The previous frame's `RawInput::time`, so the real interval can be measured
    /// rather than believed. See `eframe::App::raw_input_hook` below.
    prev_frame_time: Option<f64>,
}

impl eframe::App for TPlay {
    /// Tell egui what a frame actually took, because **eframe does not**.
    ///
    /// `prepare_raw_input` fills in `raw_input.time` and hands the rest to this
    /// hook, but never touches `predicted_dt` — so it stays at `RawInput`'s
    /// `1.0 / 60.0` default, restored every frame by the `mem::take` that clears
    /// `pending_raw_input`. The field is documented as "the time delta egui should
    /// assume", and this app is the only thing that knows the real one.
    ///
    /// It matters because the visualizer's smoothing is time-based, and a
    /// hardcoded 60 fps makes every one of those constants wrong by the display's
    /// refresh ratio — on a 240 Hz display a trail decays a quarter as fast as it
    /// should and a per-frame band smoother converges four times too fast, which
    /// is the judder. See `app::frame_dt_from` for what is believed and what is
    /// rejected.
    ///
    /// Also fixes egui's own internals, which read the same field for animations
    /// and window focus timing; `theme::apply` zeroes `animation_time` anyway, so
    /// nothing here is visible except through the app's own dt consumers.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(dt) = app::frame_dt_from(self.prev_frame_time, raw_input.time.unwrap_or(0.0)) {
            raw_input.predicted_dt = dt;
        }
        self.prev_frame_time = raw_input.time;
    }

    /// eframe 0.36 renamed this hook from `update(&Context)` to `ui(&mut Ui)`:
    /// a frame is now a `Ui`, and everything a panel wants is expressed inside
    /// it. The `Context` is one `clone()` away from the `Ui`, so the coordinator
    /// still takes a `&Context` and the split with `app.rs` is unchanged.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        gui::coordinator::update_ui(&mut self.app, &mut self.themes, &ctx, ui);
        let now = ctx.input(|i| i.time);
        let closing = ctx.input(|i| i.viewport().close_requested());
        if self.app.update(now, closing, &self.themes.current().id) {
            ctx.request_repaint();
        }
    }
}

fn main() -> eframe::Result<()> {
    let config = config::load();
    // A too-small buffer is the classic cause of ALSA "underrun occurred" at
    // track transitions (the crossfade/gapless arm decodes two files at once).
    // Fall back to the device-chosen default if the device rejects the fixed
    // size. The device lives here, not in `TPlayApp` — the app holds only the
    // `Mixer` this stream's callback pulls from.
    let output = match DeviceSinkBuilder::from_default_device().and_then(|b| {
        b.with_buffer_size(BufferSize::Fixed(config.buffer_size.clamp(512, 65536)))
            .open_stream()
    }) {
        Ok(s) => s,
        Err(_) => DeviceSinkBuilder::open_default_sink().expect("No audio output device found"),
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPlay")
            .with_inner_size([680.0, 460.0])
            .with_min_inner_size([320.0, 160.0])
            // No native title bar: the controls live in the app's own top bar
            // (right-aligned next to the logo), and that bar is draggable.
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "TPlay",
        native_options,
        Box::new(|cc| {
            // The visualizer's shader views draw through a wgpu callback, and a
            // `RenderPipeline` has to name the surface format it will draw into —
            // which is the one thing the callback API cannot tell a view. This is
            // the only place with a `CreationContext`, so the harness is given it
            // here. It needs eframe's wgpu backend, which is what 0.36 uses by
            // default; see `gui::panes::visualizer::gpu::init`.
            match cc.wgpu_render_state.as_ref() {
                Some(r) => gui::panes::visualizer::gpu::init(&r.device, &r.queue, r.target_format),
                None => eprintln!(
                    "tplay: no wgpu render state, so every visualizer view draws its \
                     background. This build needs eframe's wgpu backend."
                ),
            }
            // App-wide text fallback: a system font appended after egui's bundled
            // ones so any Unicode in tags renders (no tofu boxes).
            gui::theme::install_fallback_fonts(&cc.egui_ctx, gui::theme::SYSTEM_FONT_CANDIDATES);
            let themes = ThemeState::load(&cc.egui_ctx, Themes::load(), &config.theme);
            let app =
                app::TPlayApp::new(&config, library_db::TrackDb::load(), output.mixer().clone());
            // `output` moves in here and is never read: dropping the
            // `MixerDeviceSink` stops the cpal callback, and with nothing
            // pulling the mixer the sink never advances. The `Mixer` the app
            // holds is a clone of an `Arc` and outlives the stream on its own —
            // but playback stops without this field.
            Ok(Box::new(TPlay {
                app,
                themes,
                _output: output,
                prev_frame_time: None,
            }))
        }),
    )
}
