# tplay

Desktop audio player. Rust, eframe/egui GUI, rodio playback. Single window, no network. 13 source files (~3100 lines) + `themes/` data folder; read all of them before changing anything — there is nothing else to explore.

## Rules

- Never screenshot or try to see the GUI. Use instrumentation (debug prints, egui memory, config files) or ask the user for feedback instead.
- Never add hardcoded constants. Use the config and theme files (`~/.config/tplay/*`, `themes/<id>/theme.json` layout/palette tokens).

## Layout

- `src/main.rs` — bootstrap: window options (680x460 default, resizable, min 320x160, no native title bar), hands off to `app::TPlayApp`.
- `src/app.rs` — `TPlayApp`: ALL app state + logic, zero UI code. Owns the rodio stream/sink. The GUI calls in through pub methods; read-only getters sit at the bottom. **No docking state** — that lives in GUI layer.
- `src/lib.rs` — public re-exports for testing (`app`, `audio`, `gui`, `library`).
- `src/gui/mod.rs` — declares `coordinator` + `panes` + `theme`.
- `src/gui/coordinator.rs` — `update_ui(app, ctx)`: applies the theme's egui visuals, then draws one frame's `DockArea` (egui_dock). Implements `TabViewer` for `Pane` enum; calls the four pane functions in `ui()`. Loads dock layout from egui memory + `~/.config/tplay/dock_layout.json`; saves to disk only when the layout changes or the app closes. The ☰ menu re-adds closed panes.
- `src/gui/theme.rs` — data-driven theme system. `Themes::load()` scans `~/.config/tplay/themes/` (user, wins on id clash), `<exe_dir>/themes/` (shipped with the app), `./themes/` (dev: `cargo run` from repo root) and merges by theme id. Each `themes/<id>/theme.json` carries `id`, `name`, `base` (dark/light), `metadata_font` (monospace/…), and the 14-token `Palette` (`--bg`, `--accent`, `--row-even`, …) as hex strings; invalid files are skipped with an eprintln, and a hardcoded dark fallback theme guarantees a non-empty list (broken install). `apply(ctx, &Theme)` maps tokens onto egui `Visuals` each frame so a mid-session switch lands instantly. Selection persists to `~/.config/tplay/config.json` (the `theme` field); missing config = `dark`.
- `themes/<id>/icons/*.png` — per-theme icon set (logo, play, pause, stop, prev, next, shuffle, repeat, volume, remove, sort_asc, sort_desc, star_on, star_off, folder, minimize, maximize). Missing PNGs fall back to the default theme's (`dark`), then to unicode glyphs. `theme::load_icons` decodes them synchronously into egui textures at startup and on skin switch (NO egui async loader — the URI loader path showed pending/error placeholders and stretched buttons).
- `src/gui/panes/{now_playing,playlist,equalizer,library}.rs` — one free function per pane, each taking `(app: &mut TPlayApp, ui: &mut egui::Ui)`. Position-independent — work identically docked anywhere.
- `src/library.rs` — pure logic: directory listing, tag/duration reading via `lofty`, background scan thread (`scan_files` + mpsc), `TrackInfo` cache, sorting (`sort_key` with DEL prefix for empty values), playlist read/write (`.tplay` JSON).
- `src/audio/mod.rs` — file-level helpers, no playback logic:
  - `probe_duration` — symphonia-based duration probe; fallback when rodio's `Decoder::total_duration()` is None (mainly MP3). This is why `symphonia` is a direct dep.
  - `flac_has_seektable`, `build_flac_seektable` — hand-rolled FLAC binary format code: walks frame headers via symphonia's demuxer, writes a SEEKTABLE block into the file's PADDING block. Runs on a background thread (see below).
- `src/audio/eq.rs` — 10-band graphic EQ (`EqSource` wrapping `rodio::Source<Item=f32>`). RBJ peaking EQ coefficients (w3.org audio-eq-cookbook). Gains live in `Arc<RwLock<EqShared>>` shared with GUI; `EqSource::refresh()` read-locks per sample, UI write-locks — no sink rebuild, no audio restart. 0 dB = exact identity filter (A=1).
- `tests/` — ALL tests live here; each file is its own auto-discovered test binary (cargo runs each once). `tests/common.rs` provides shared helpers (temp dirs, minimal WAV/FLAC generators, duration assertions). Individual suites: `eq_tests.rs`, `flac_tests.rs`, `playlist_tests.rs`, `library_tests.rs`, `gui_tests.rs`, plus the second-wave suites below.
- **Headless boundary**: `TPlayApp::new()` needs a real audio device + eframe `CreationContext`, so no test constructs it. Pure playback logic (shuffle navigation, drag index math) is mirrored branch-for-branch as free functions in `tests/` — the same convention `playlist_tests.rs` and `drag_drop.rs` follow. The `pub` static helpers that *are* reachable (`fmt_duration`, `format_freq`, `EQ_PRESETS`, `Pane::ALL`) are tested directly in `app_integration.rs`, along with `probe_duration` on a real WAV.
- Second-wave suites (added to lock down logic the first wave didn't reach):
  - `app_integration.rs` — TPlayApp static helpers, Pane/EQ preset invariants, `probe_duration`.
  - `config_persistence.rs` — `Config` serde round-trip, missing-field defaults, JSON key shape. Requires `Config`/`EqData`/`LibraryData` and their fields to be `pub` (they are — the structs document the on-disk format for tests).
  - `dock_layout.rs` — egui_dock round-trip of tabs/splits/floating windows via `lay_out` + the `finite` viewport substitution (a never-painted leaf has `Rect::NOTHING` viewports that serialize as `null`, which serde_json can't read back — a session always saves after painting).
  - `drag_drop.rs` — every branch of `move_track`/`remove_track` current-index adjustment.
  - `library_search.rs` — sort_key beyond the basics: case-insensitivity, year, genre, zero-padded millisecond duration ordering (9 s < 10 s).
  - `eq_live.rs` — `EqSource` live behavior at Source level: 0 dB exact identity, band boost amplitude, gain change mid-iteration, disable/reenable.
  - `tag_cache.rs` — the `drain_tag_scan` mpsc pattern (Empty/Disconnected branches) and dropped-receiver replacement.
  - `playlist_shuffle.rs` — reset semantics (remove/move/play_track/toggle) and newly-added tracks joining the unplayed pool.
  - `contrast_tests.rs` — WCAG contrast verification of the *bundled* themes: text (primary/secondary/accent) on every surface ≥ 4.5:1, UI components (focus ring, progress fill, slider handle) ≥ 3:1, and the active-row tint composite keeping title text ≥ 4.5:1. `eq_band_w_min` floor ≤ 320px window. Guards palette regressions.
  - `ui_polish_tests.rs` — theme-app behavior: `apply` zeroes `animation_time` (instant switch) and maps tokens to `Visuals`; new layout tokens (`text_meta`/`text_time`/`row_tint_alpha`) round-trip through `load_from`; the 10 × `eq_band_w_min` floor stays under the minimum window.
  - `layout_save_load.rs` — named layout save/load round-trips through the coordinator helpers: `save_layout`/`load_layout`/`list_layouts` + the finite-viewport fix for serde stability.

## Build / verify

- `cargo check` — compiles fast with cached target/.
- `cargo test` — runs all tests; several FLAC seektable builder tests are `#[ignore]` because the minimal FLAC fixtures carry no audio frames (need a real FLAC fixture to enable).
- `[profile.release] opt-level` is deliberately absent — 3 is Cargo's default.

## Dependencies — each one is load-bearing

| dep | why |
|---|---|
| eframe | GUI + windowing (egui) |
| rodio (`symphonia-all`) | decode + playback: mp3, flac, ogg, wav, m4a |
| symphonia (direct) | MP3 duration probing rodio can't do; FLAC frame walking in the seektable builder |
| rfd | native file dialog |
| serde / serde_json | playlist save/load (JSON) + dock layout (JSON via serde) |
| dirs | cross-platform config directory (`~/.config/tplay/`) |
| egui_dock | docking layout: tab drag/split/tear-off; serde for layout persistence |
| image (png-only) | decodes theme icons (PNG → egui texture) synchronously at startup / skin switch |
| lofty | audio tag reading for every pane's track display — ID3v2, Vorbis comments, MP4 ilst, RIFF INFO (artist/album/year/genre/track + duration) |

---

## Playlist

- `playlist: Vec<PathBuf>` — ordered list of tracks; no duplicates (checked on add)
- `playlist_file: Option<PathBuf>` — the `.tplay` file this playlist was saved to / loaded from; `Some` → Save overwrites it, `None` → Save opens the dialog
- `playlist_dirty: bool` — edited since the last save/load; set on add/remove/move, cleared on save/load/new. Drives whether New/Load ask a confirm.
- `current_index: Option<usize>` — index of currently playing track in playlist; `None` = not playing from playlist (direct open from Library)
- `shuffle: bool` — shuffle mode enabled
- `repeat: bool` — repeat mode enabled
- `played: Vec<usize>` — history of indices played in this shuffle cycle (for prev/next)
- `rng_state: u64` — XorShift64 state for shuffle (replaces `fastrand` dependency)
- **Add Files / track additions come from the Library** — the `+` row button and the **Add All** header button call `add_files` (dedup checks); the old Add Files dialog button in the pane footer is gone
- **Header**: the pane shows the current playlist's name at the top — the tracked file's stem when saved/loaded, else "Untitled", via the single `app.playlist_name()` getter (no pane-side derivation; follows New/Save/Load live)
- Click track name to play; drag track name to reorder; ✕ icon button removes track (✕ and the right-aligned FORMAT column sit in a fixed 24px row; rows alternate `--row-even`/`--row-odd`, the active row gets a full-row `--accent` tint plus a 3px `--accent` left stripe; row content is inset 6px so the leading number clears the stripe; rows show the tagged **title** — filenames render without their extension until the tag scan lands — and a truncated `artist · album` secondary block, with FORMAT carrying the type)
- **Shuffle** — plays each track once in random order; clicking a track resets shuffle
- **Repeat** — loops playlist (sequential) or shuffle cycle
- Transport (play/prev/next/stop) uses the theme's icon textures; the shuffle/repeat mode toggles sit beside them (same icon set, lit via egui's selected visuals while active)
- Auto-advance (`advance`) fires on natural track end (sink empty, unpaused, `current_path` set), called from `TPlayApp::update()` every frame
- **Save Playlist** — first save opens a native save dialog (defaults to the Library's current folder, suggested `playlist.tplay`) and writes the current playlist as a `.tplay` file (`{"paths": [...]}` only — shuffle/repeat are appwide, never playlist content; `library::write_playlist`). The pane remembers the file (`playlist_file`), so later saves overwrite it directly — no dialog. Loading a `.tplay` from the Library also sets the tracked file (save writes back to the source); **Create Playlist** clears it so a fresh playlist asks where to go. Button hover shows the overwrite target ("Overwrite playlist: …").
- **New Playlist** — clears the current playlist (replaced the old Load button); the button label reads **Create Playlist**. Loading now happens in the Library by clicking a `.tplay` file. New and Load confirm (native Yes/No, `TPlayApp::confirm`) only when the playlist has unsaved edits (`playlist_dirty`) — a clean switch is silent. The ✕ per-row remove always confirms.
- Action buttons (Create Playlist / Save Playlist) live in a `horizontal_wrapped` row at the bottom of the pane (`item_spacing.x = 12` between them).
- **Shuffle algorithm** (in `app.rs`): `next_track_index` picks a random unplayed index (XorShift64); `prev_track_index` pops from `played` history; `repeat=true` restarts the cycle when the pool is exhausted. `reset_shuffle()` clears `played` — called on toggle, play_track, remove/move, load, stop.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory (`tplay.drag_from`, `tplay.drag_hover`) and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.

---

## Equalizer

- 10 bands at `audio::eq::EQ_FREQUENCIES` (20, 100, 300, 600, 1K, 3K, 5K, 8K, 12K, 16K — the reference UI's labels; the same constant drives the filters *and* the pane labels, so they can't drift).
- Pane header: left `ON` toggle, then `Reset` button and preset dropdown — all three grouped in one row with matching button chrome. Presets: `EQ_PRESETS` in app.rs — Flat, Rock, Pop, Jazz, Classical, Electronic, Vocal. Curves follow sfxengine.com/blog/best-equalizer-settings-for-music (Rock ⇐ Rock/Metal, Pop ⇐ V-Shape, Jazz ⇐ Treble Boost, Electronic ⇐ Bass Boost, Vocal ⇐ Vocal Enhancement). Selecting one applies its gains immediately; any manual slider tweak switches the selection to `EQ_PRESET_CUSTOM`. Preset is derived from gains (never stored — one source of truth) in `~/.config/tplay/config.json`.
- **Live EQ**: `EqSource` chains 10 biquad filters (Direct Form II transposed) in series. `refresh()` runs per sample frame; reads shared gains under a read lock; rebuilds only changed bands (fresh `Biquad` with zeroed state — no click from stale history). `enabled=false` skips processing entirely.
- **Sliders**: vertical, trailing fill (theme `--progress-fill`), height clamped to theme layout tokens (`eq_slider_min_h`..`eq_slider_max_h`). Band width dynamic: max 80px, shrinks with spacing to fit the pane; the pane width is floored at `10 × eq_band_w_min` (no scrollbars on the EQ pane). Frequency labels use the `text_meta` size. (A "scale the slider on press" effect isn't implemented — egui has no transform API for widgets; the drag already responds instantly with `animation_time = 0`.)

---
 
## Visualizer
 
- Dockable pane (**Visualizer**), closed by default, auto-listed in the ☰ menu. No new theme tokens, no new icons.
- Header: **Bars / Wave** toggle, persisted in `config.json` (`viz_wave`) so the choice survives restarts.
- Audio pipeline: `TapSource` wraps `EqSource` (post-EQ → visualizes exactly what you hear). Mono-downmixes, pushes into a capped ring buffer (`VizBuf`, ~100 ms / 4096 samples). `load_file` and `seek` append the tap; `load_file`/`stop()` clear the buffer.
- **Bars mode**: 1024-point radix-2 FFT with Hann window → 32 log-spaced bands → dB-normalized (-60..0) → per-bin attack/release smoothing (classic WMP feel).
- **Wave mode**: mirrored time-domain **peak envelope** downsampled from the ring buffer (one `|sample|` peak per ~2px display column, from a ~23 ms window), rendered as adjacent filled columns + outline strokes — column fills use `rect_filled` (polygon tessellation of raw sample waves rendered as garbage).
- Drawing: full-rect `ui.painter()`, palette colors only (`accent`, `progress_fill`, `bg`), `scroll_bars = [false, false]` like the EQ pane.
- No new dependencies — hand-rolled FFT and smoothing in `src/audio/viz.rs` (pure logic, tested without an audio device).
 
---
 
## Library

- Filesystem browser (`TPlayApp::navigate_to`): the current folder's subfolders + audio files rendered as one combined row list (`library::Entry::Dir`/`File`), `..` row to move up. Subfolders are `📁` rows, audio files are tag-table rows. Dot-prefixed (hidden) subfolders are skipped by default — a "Show hidden folders" checkbox in the ☰ menu toggles it (`TPlayApp::show_hidden`/`set_show_hidden`, persisted in `config.json`).
- **Header**: folder icon + strong current-folder name (full path on hover) + ★ toggle; right side shows a "N tracks · M folders · K playlists" composition count (from the in-memory `library_entries`, no scan) and the Add All button. The 6-column sortable header renders **only when the folder has audio files** — folders/playlists-only folders skip it (no tag columns to align to).
- **Playlist files as rows**: `.tplay` files (see Playlist) are listed interleaved with the songs (`library::is_playlist` widens `list_dir`'s filter). A playlist row shows its full filename plus a "Playlist" tag right after the name (not floated to the far right), no tag cells and no `+`. Click → confirm (only if the current playlist has unsaved edits) → `load_playlist_from`, dumping the saved tracks into the Playlist pane for editing. Playlists are **not** tag-scanned (`navigate_to`/`library_scanning` exclude them, or "Scanning…" would stick) and **Add All** skips them — a playlist file isn't audio.
- **Favorites** ★ toggles the current folder; persisted with the last browsed dir to `~/.config/tplay/config.json` (`{library: {favorites, last_dir, show_hidden}}`), restored on startup. The favorites column is 120px wide (fits the 320px minimum window alongside the file list).
- Each file row is a fixed-column table: track-no title (flexible), then Artist/Album/Year/Genre/Duration cells mirroring the header, with a `+` button — all from the **background tag scan** — `library::scan_files` (a thread per scan) tag-reads every file with `lofty` (ID3v2 / Vorbis comments / MP4 ilst / RIFF INFO) and sends `(path, TrackInfo)` over an mpsc channel that `TPlayApp::update()` drains into the shared `tag_cache` each frame. Revisits are instant (cache check skips the scan); filenames stand in until the scan lands; a "Scanning…" label shows while any current-folder file is missing from the cache.
- **Sortable header**: the file list has a clickable column header (Title / Artist / Album / Year / Genre / Duration) above the ScrollArea — one column per row cell (no separate Name column). Click picks that column ascending; clicking the active column flips direction (`TPlayApp::set_library_sort` → `apply_library_sort`, comparator `library::sort_key` keyed by `SORT_OPTIONS` index). **Folders participate**: they're untagged entries, so Title sorts them by their own name (interleaved with files) and the tag columns sink them below the files. The active column shows a theme sort icon (accent-colored per theme: `sort_asc`/`sort_desc` — generating these was the fix for the old ▾/▴ glyphs, which are tofu in every bundled egui font) and accent color. Untagged fields sort last (empty strings, missing durations).
- **Sort key encoding** (`library::sort_key`): returns a lowercase string for case-insensitive sorting; empty/missing values get a `\x7f` (DEL) prefix so they sort after normal content; durations become zero-padded milliseconds. Folders are untagged (empty strings → sink last on tag columns).
- Click a song row → `play_file` — a **direct open**: `current_index = None`, the playlist's sequential flow and shuffle state are untouched, and auto-advance won't cascade off it. The `+` row button and the **Add All** header button add tracks to the current playlist via the existing `add_files` dedup.
- Search box filters the current folder's rows by title/artist/album (filename pre-scan, combined into one haystack string).
- Row styling reuses the playlist pattern: `--row-even/odd` banding, 3px accent stripe + tint on the currently playing row. Folder rows paint the per-theme `folder` icon (tinted from `text_secondary`, baked at generation); the favorite toggle uses `star_on`/`star_off` (accent/secondary); the per-favorite remove row button and the sort direction are theme icons too. Glyph fallbacks must exist in the bundled egui font stack (Ubuntu-Light / NotoEmoji / emoji-icon-font) or they tofu.
- **Per-session init flag**: `tplay.library.init` in egui memory ensures `navigate_to` runs once on the first frame.

---

## Themes (data-driven, dark / retro / neon)

- All colors flow from each theme's JSON `Palette` tokens mirroring the reference CSS custom properties. Panes paint rows/labels/metadata straight from tokens via `app.theme().palette` (the `metadata_font` from `app.theme().metadata_font`).
- Token → egui `Visuals` mapping (see docs on `theme::apply`): `--bg` → panel/window fill, `--text-primary` → `override_text_color`, `--progress-fill` → `selection.bg_fill` (the slider's trailing fill), `--slider-track` → `inactive.bg_fill` (rail), `--slider-handle` → `inactive.fg_stroke` (knob), `--focus-ring` → `selection.stroke`, `--accent` → hyperlink/active fill. `--row-even/odd` and `--text-secondary` can't be expressed in Visuals — panes use them directly.
- Theme files are plain JSON + PNGs — creating/modifying a theme is drop-in editing, no rebuild. Theme selection persists in `~/.config/tplay/config.json`; config absence or unknown id = `dark`.
- Icons: `themes/<id>/icons/<name>.png` (17 fixed names). Resolution: own file → default theme's file → glyph. Preloaded synchronously into `egui::TextureHandle`s (`TPlayApp.icons`, one per `Icon::ALL` slot) at startup and in `set_theme`; `app.theme_icon(Icon)` hands them to `theme::icon_button`/`theme::icon`. The 7 newer icons (sort/star/folder/window chrome) are generated 20×20 PNGs with colors baked from each theme's `theme.json` palette tokens at generation time (accent / text_secondary / text_primary) — no runtime tinting, matching the hand-drawn transport icons' convention.
- Retro: light Winamp grays + blue (`accent`/`progress_fill`/`focus_ring` = `#15449e`, a darker royal blue than the original — measured to keep accent-as-text ≥ 4.5:1 on the light surfaces), `metadata_font: monospace` for times/metadata (monospace digits are tabular — the seek-bar time labels can't jitter in retro). Neon: near-black blue + mint accent/pink progress.
- **Layout tokens**: each theme may include a `layout` object with EQ sizing values (`eq_slider_min_h`, `eq_slider_max_h`, `eq_band_w_min`, `eq_header_gap`, `eq_band_gap`), the type scale (`text_meta` 12px, `text_time` 13px — threaded through every pane's label/font sizes, no magic sizes in pane code), and the active-row tint alpha (`row_tint_alpha`, default 0.10). These drive the equalizer pane's minimum/maximum slider height, minimum band width (the dock floors the pane's width at `10 × eq_band_w_min` — default 30, so the floor (300px) fits inside the 320px minimum window since the band row clips below it — no scrollbars), spacing, and the row highlight, replacing hardcoded values. Defaults live in code (`Layout::with_defaults`).
- **Active-row tint**: `theme::row` paints a *translucent* accent overlay (`row_tint_alpha` alpha) over the banded bg plus a 3px accent stripe — not `accent.gamma_multiply()`, which crushed to near-black and rendered the highlight invisible. The overlay keeps title text ≥ 9:1 in every theme (see `contrast_tests`).
- **No widget animations**: `theme::apply` sets `style.animation_time = 0.0` each frame, so a mid-session theme switch (and every hover/active transition) lands instantly — no color cross-fade smear. Winamp-snappy on purpose.
- **Theme JSON schema** — `id` and `palette` (14 tokens) are required; everything else has a default:
  ```json
  {
    "id": "dark",
    "name": "Dark",
    "base": "dark",
    "metadata_font": "proportional",
    "palette": { "bg": "#141414", "panel_bg": "#1d1d1d", "text_primary": "#e0e0e0", "text_secondary": "#8c8c8c", "accent": "#2ea3f0", "border": "#383838", "progress_fill": "#2ea3f0", "slider_track": "#2b2b2b", "slider_handle": "#9a9a9a", "row_even": "#171717", "row_odd": "#202020", "btn_hover": "#2a2a2a", "focus_ring": "#2ea3f0" },
    "layout": { "eq_slider_min_h": 60, "eq_slider_max_h": 220, "eq_band_w_min": 30, "eq_header_gap": 6, "eq_band_gap": 2, "text_meta": 12, "text_time": 13, "row_tint_alpha": 0.1 }
  }
  ```

---

## Docking (egui_dock)

- Five panes as tabs: **Now Playing**, **Playlist**, **Equalizer**, **Library**, **Visualizer**
- **Pane sizing**: per-pane `PaneSizing` policy in coordinator.rs — `Fill` panes (Playlist, Equalizer, Library) take their dock share and resize via separator drag; `Fixed` panes (Now Playing) are pinned to their measured content height each frame (the Now Playing pane records its scope height to egui memory under `tplay.pane_content_h`, and `apply_min_pane_sizes` rewrites the enclosing split's `fraction` before `DockArea::show`, so the separator snaps back — no empty dead zone below the controls). Minimum floors are enforced twice per frame: `apply_min_pane_sizes` runs before `DockArea::show`, and again **after** it (the splitter drag and floating-window resize happen inside `show()` and ignore the floors), then `ctx.request_repaint()` lands the corrected fractions next frame - so dragging below the EQ sliders' minimum height snaps back instead of sticking.
- Default layout: top row (Now Playing tab) ~15%, bottom (Playlist) ~85%
- Drag tabs to reorder; drag onto split overlays to dock left/right/top/bottom/center
- Resize panes via draggable splitters
- Tear tabs off into floating windows
- Close pane via tab X button (disabled — panes close via the ☰ menu checkbox); reopen via the ☰ menu button (app logo, per-theme `logo.png`) at top-left, which also lists the Theme selector (theme switcher) and the **Layouts** section
- Tab bodies are wrapped in a ScrollArea by egui_dock; the EQ pane disables both scrollbars (`scroll_bars` → `[false, false]`) and sizes its 10 bands to the pane width, so nothing can overflow
- Layout auto-saves to `~/.config/tplay/dock_layout.json` (JSON via serde) on layout change or app close, and restores on startup
- **Named layouts** (☰ menu → Layouts): save the current dock arrangement to `~/.config/tplay/layouts/<name>.json`; the menu lists every saved layout by stem — click to load, ✕ to delete. **Save current layout…** overwrites the tracked layout file if one exists (like playlists' `playlist_file`), otherwise opens a native save dialog defaulting to the layouts dir. **Load layout…** opens any layout file from anywhere and tracks it for future saves. Tracking is session-local (restart restores via `dock_layout.json`).
- **Window controls**: decorations are off (no native title bar — `main.rs`), so the top bar carries its own right-aligned controls — minimize, maximize/restore, close — drawn from the per-theme `minimize`/`maximize`/`remove` (✕) icons via `theme::icon_button` (the old 🗕/🗖/🗙 emoji glyphs tofu'd). The bar itself drags the window via `StartDrag` (a bottom-of-z-stack `interact`); the menu logo and the control buttons drawn after still win their own clicks.
- The ☰ menu also carries the "Show hidden folders" checkbox (Library toggle).

---

## Non-obvious machinery

- **Seek fast/slow path** (`seek` in app.rs): try `sink.try_seek()`; if it fails, reopen the file and play through with `source.skip_duration(target)`. FLAC without a seektable cannot seek fast — hence the builder.
- **EQ Source wrapper**: Custom `rodio::Source` that chains 10 biquad filters in series; applies per-sample in `next()`. Uses RBJ peaking EQ coefficients. Sample rate from inner source. Decoder outputs i16, converted to f32 via `convert_samples::<f32>()` before EQ. **EQ settings are live**: gains live in `Arc<RwLock<EqShared>>` shared with the GUI; `EqSource::refresh()` uses read locks (concurrent), UI uses write locks — no sink rebuild, no audio restart. 0 dB is an exact identity filter (RBJ peaking at A=1), so enabled/disabled needs no bypass path.
- **FLAC seektable build**: loading an FLAC lacking a SEEKTABLE block spawns a thread running `build_flac_seektable` (~1 s). Field `seektable_ready: Option<Arc<AtomicBool>>`: `Some(flag)` while building, `None` = file already had one or isn't FLAC (fast path allowed, via `unwrap_or(true)` in `seek`). While building, seeks fall to the slow path.
- **Visualization tap source** (`src/audio/viz.rs`): `TapSource` wraps `EqSource` (post-EQ) and mono-downmixes into a capped ring buffer (`VizBuf`, 4096 samples ≈ 93 ms). The GUI snapshots the tail each frame, runs a 1024-point radix-2 FFT with Hann window, maps to 32 log-spaced bands, dB-normalizes, and applies per-bin attack/release smoothing. Both `load_file` and `seek` append the tap; `load_file`/`stop()` clear the buffer so stopped playback shows flat idle. Pure logic — no audio device needed for tests.
  - **Phase 1**: read STREAMINFO (extract sample_rate) and locate the PADDING block (position, length, is_last).
  - **Phase 2**: walk audio packets via the symphonia demuxer (frame headers only, no decoding) to collect byte offsets at ~1-second intervals (sample_rate samples apart).
  - **Phase 3**: overwrite the PADDING block with a SEEKTABLE (18-byte entries). Remainder handling: <4 leftover bytes → sacrifice one slot; ≥4 leftover bytes → write a trailing PADDING block.
- **Slider pinning**: `seek_target: Option<f32>` holds the seekbar at the intended position until `sink.get_pos()` catches up; prevents snap-back to 0 during slow-path seeks.
- **Sink replacement on load/seek**: replacing the `Sink` drops the old decoder and releases the file handle (required on Windows while the background thread writes the seektable).
- **Playlist auto-advance** (`advance` in app.rs): runs every frame; fires only when a track drained naturally — sink empty, unpaused, and `current_path` set. The `current_path.is_none()` guard is what stops a failed load from cascading through the whole list one entry per frame.
- **Per-frame UI state in egui Memory**: `TPlayApp` has zero UI state. The seek slider position (`tplay.seek` in now_playing.rs), the drag-and-drop state (`tplay.drag_from` / `tplay.drag_hover` in playlist.rs), the library init flag and search query (`tplay.library.init` / `tplay.library.query` in library.rs) are stored via `ui.ctx().memory_mut` so they survive across frames and between panes.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.
- **Shuffle order**: `played: Vec<usize>` is the history of indices played in this shuffle cycle. `next_track_index` picks random from the unplayed pool; `prev_track_index` pops from `played`. `repeat=true` restarts the cycle when the pool is exhausted. Clicking a track directly resets shuffle (`played.clear()`). Reordering/deleting calls `reset_shuffle()`.
- **Dock state in egui Memory**: `DockState<Pane>` lives in `ctx.data()` under `tplay.dock_state`. Pure UI state — not in `TPlayApp`. Layout persists to disk each frame.
- **Shared tag scan**: one cache (`tag_cache`) + one background `scan_files` thread serve every pane. `TPlayApp::ensure_tags(paths)` starts the scan for whatever's missing from the cache (dropping any in-flight receiver — per-path cache means a dropped scan just restarts next request); results drain per frame in `drain_tag_scan`, the app's only other thread besides the FLAC seektable builder. Triggers: library `navigate_to`, playlist `add_files`, `load_playlist_from`. `load_file` reads the playing track's tags inline (one file) so Now Playing shows title · artist immediately. `library_scanning()` recomputes as "any current-folder file missing from the cache".
- **Appwide settings**: one `~/.config/tplay/config.json` holds everything — theme, EQ (enabled + gains), shuffle/repeat, visualizer mode (`viz_wave`), volume, last loaded playlist, and the library state (favorites, last dir, show hidden). Saved via `TPlayApp::save_config()` on any change, loaded once in `new()`. Shuffle/repeat are global playback modes — loading a playlist never touches them. Volume is applied to the sink at startup. The last loaded `.tplay` (`last_playlist`) is re-loaded on startup (Library + tag scan + tracked file all restored), so Save Playlist keeps overwriting the same file across sessions.
  - **Config JSON structure**:
    ```json
    {
      "theme": "dark",
      "eq": { "enabled": false, "gains": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
      "shuffle": false,
      "repeat": false,
      "viz_wave": false,
      "volume": 1.0,
      "last_playlist": "/path/to/playlist.tplay",
      "library": { "favorites": ["/path/to/fav"], "last_dir": "/path/to/music", "show_hidden": false }
    }
    ```
- **Confirm dialogs**: `TPlayApp::confirm(title, desc, at_risk)` wraps `rfd::MessageDialog` (Yes/No). Used for New Playlist, loading a playlist over the current one, and per-track ✕ removal. `at_risk = false` (e.g. an empty playlist) skips the dialog. Save-overwrite is already confirmed by the native save dialog.
- **XorShift64 RNG** (`app.rs`): inline 64-bit XorShift (`state ^= state << 13; state ^= state >> 7; state ^= state << 17`) replaces `fastrand`. Used for shuffle track selection. Fixed seed `0xC0FFEE`.

---

## Config & data files

| file | purpose |
|---|---|
| `~/.config/tplay/config.json` | unified app settings (theme, EQ, shuffle, repeat, volume, last playlist, library state) |
| `~/.config/tplay/dock_layout.json` | egui_dock layout (tabs, splits, floating windows) |
| `~/.config/tplay/layouts/<name>.json` | named dock layout (tabs, splits, floating windows) — created via ☰ menu |
| `~/.config/tplay/themes/<id>/theme.json` | user theme override (wins on id clash) |
| `<exe_dir>/themes/<id>/theme.json` | shipped themes (dark, retro, neon) |
| `./themes/<id>/theme.json` | dev themes (cargo run from repo root) |
| `<playlist>.tplay` | playlist file: `{"paths": ["/abs/path", "relative/path"]}` — relative paths resolve against the playlist's own directory |

---

## Key constants & egui memory IDs (grep targets)

| constant | file | purpose |
|---|---|---|
| `EQ_FREQUENCIES` | audio/eq.rs | 10 band frequencies (Hz) — same constant drives filters and UI labels |
| `EQ_PRESETS` | app.rs | 7 named gain curves |
| `SORT_OPTIONS` | library.rs | 6 library columns |
| `VIZ_BANDS` | audio/viz.rs | 32 log-spaced output bands for drawing |
| `FFT_SIZE` | audio/viz.rs | 1024-point FFT window |
| `VIZ_BUFFER_CAP` | audio/viz.rs | 4096 mono samples ring buffer cap |
| `DEFAULT_THEME_ID` | gui/theme.rs | `"dark"` |
| `DOCK_LAYOUT_FILE` | gui/coordinator.rs | `"dock_layout.json"` |
| `LAYOUTS_DIR` | gui/coordinator.rs | `"layouts"` (named layouts subdir) |
| `NAMED_LAYOUT_FILE` | gui/coordinator.rs | `"tplay.named_layout_file"` (egui memory key) |
| `PANE_CONTENT_H` | gui/coordinator.rs | `"tplay.pane_content_h"` (egui memory key) |
| `PANE_CONTENT_W` | gui/coordinator.rs | `"tplay.pane_content_w"` (egui memory key) |
| `DOCK_ID` | gui/coordinator.rs | `"tplay.dock_state"` (egui memory key) |
| `DOCK_SAVED_JSON` | gui/coordinator.rs | `"tplay.dock_layout_saved"` (egui memory key) |
| `SEEK_ID` | gui/panes/now_playing.rs | `"tplay.seek"` (egui memory key) |
| `LIB_INIT` | gui/panes/library.rs | `"tplay.library.init"` (egui memory key) |
| `LIB_QUERY` | gui/panes/library.rs | `"tplay.library.query"` (egui memory key) |
| `AUDIO_EXTENSIONS` | library.rs | `["mp3","wav","ogg","flac","m4a"]` |

---

## Test conventions

- **All tests live in `tests/`** — no `#[cfg(test)]` modules in src. Cargo auto-discovers each `tests/<suite>.rs` as its own binary, so every suite runs exactly once (`tests/main.rs` was removed — it re-declared the suites and doubled every run).
- `tests/common.rs` helpers: `temp_dir()`, `test_dir(name)`, `write_wav(path)` (1s 8kHz mono PCM), `write_minimal_flac(path)`, `write_flac_with_sample_rate(path, sr, padding)`, `assert_duration_approx`.
- FLAC seektable builder tests are `#[ignore]` — they need real FLAC files with audio frames for symphonia to parse; the synthetic fixtures have headers only.
- Test isolation: each test makes its own temp dir under `/tmp/tplay-test-<pid>/<name>/` and removes it.
- Run: `cargo test` (all suites, once each).

---

## Common edit patterns

| task | files to touch |
|---|---|
| Add a theme | `themes/<id>/theme.json` + `themes/<id>/icons/*.png` (17 names) |
| Change EQ band frequencies | `audio/eq.rs` (`EQ_FREQUENCIES`) — updates both filters and UI labels |
| Add EQ preset | `app.rs` (`EQ_PRESETS` array) |
| Add audio format | `library.rs` (`AUDIO_EXTENSIONS`), `app.rs` (`audio_dialog` filter), Cargo.toml (rodio features) |
| Change a pane's UI | the one function in `gui/panes/<pane>.rs` |
| Change visualizer rendering | `gui/panes/visualizer.rs` (bars/wave draw), `audio/viz.rs` (FFT/smoothing) |
| Change docking behavior | `gui/coordinator.rs` (`apply_min_pane_sizes`, `default_tree`, `save_layout`/`load_layout`/`list_layouts`, Layouts menu section) |
| Add a config field | `app.rs` (`Config` struct + `save_config`/`load_config`) |
| Change tag fields | `library.rs` (`TrackInfo`, `read_info`, `SORT_OPTIONS`, `sort_key`) |
| Adjust seek behavior | `app.rs` (`seek`, `advance`, `load_file`) |

---

## Remember

- Update this file after every significant change.
- The codebase is intentionally small — prefer deletion over addition.
- All constants come from theme JSON or shared constants; no magic numbers in pane code.