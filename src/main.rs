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
            let config = config::load();
            let themes = ThemeState::load(&cc.egui_ctx, Themes::load(), &config.theme);
            Ok(Box::new(TPlay { app: app::TPlayApp::new(&config), themes }))
        }),
    )
}
