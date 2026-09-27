#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod gui;
mod library;
mod network;
mod tracks;

use eframe::egui;
use gui::theme::{ThemeState, Themes};
use rodio::{cpal::BufferSize, OutputStream, OutputStreamBuilder};

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
    /// The cpal output stream, kept alive and never read. The app holds a
    /// `Mixer` (an `Arc` clone of this one's), which is all it needs to attach
    /// sinks; but nothing *pulls* that mixer except this stream's callback, so
    /// dropping it would silently freeze playback rather than error.
    _output: OutputStream,
}

impl eframe::App for TPlay {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        gui::coordinator::update_ui(&mut self.app, &mut self.themes, ctx);
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
    let output = match OutputStreamBuilder::from_default_device().map(|b| {
        b.with_buffer_size(BufferSize::Fixed(config.buffer_size.clamp(512, 65536)))
            .open_stream()
    }) {
        Ok(Ok(s)) => s,
        _ => OutputStreamBuilder::open_default_stream().expect("No audio output device found"),
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
            // App-wide text fallback: a system font appended after egui's bundled
            // ones so any Unicode in tags renders (no tofu boxes).
            gui::theme::install_fallback_fonts(&cc.egui_ctx, gui::theme::SYSTEM_FONT_CANDIDATES);
            let themes = ThemeState::load(&cc.egui_ctx, Themes::load(), &config.theme);
            let app = app::TPlayApp::new(&config, output.mixer().clone());
            // `output` moves in here and is never read: dropping the
            // `OutputStream` stops the cpal callback, and with nothing pulling
            // the mixer the sink never advances. The `Mixer` the app holds is a
            // clone of an `Arc` and outlives the stream on its own — but
            // playback stops without this field.
            Ok(Box::new(TPlay {
                app,
                themes,
                _output: output,
            }))
        }),
    )
}
