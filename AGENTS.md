# tplay

Desktop audio player. Rust, eframe/egui GUI, rodio playback. Single window, no network, no persistence. 10 source files (~980 lines); read all of them before changing anything — there is nothing else to explore.

## Layout

- `src/main.rs` — bootstrap: window options (680x460 default, resizable), hands off to `app::TPlayApp`.
- `src/app.rs` — `TPlayApp`: ALL app state + logic, zero UI code. Owns the rodio stream/sink. The GUI calls in through pub methods; read-only getters sit at the bottom. **No docking state** — that lives in GUI layer.
- `src/gui/mod.rs` — declares `coordinator` + `panes`.
- `src/gui/coordinator.rs` — `update_ui(app, ctx)`: one frame's `DockArea` (egui_dock). Implements `TabViewer` for `Pane` enum; calls the four pane functions in `ui()`. Loads/saves dock layout from egui memory + `~/.config/tplay/dock_layout.ron`. The `+` tab-bar button re-adds closed panes.
- `src/gui/panes/{title,seekbar,transport,playlist,equalizer}.rs` — one free function per pane, each taking `(app: &mut TPlayApp, ui: &mut egui::Ui)`. Position-independent — work identically docked anywhere.
- `src/audio/mod.rs` — file-level helpers, no playback logic:
  - `probe_duration` — symphonia-based duration probe; fallback when rodio's `Decoder::total_duration()` is None (mainly MP3). This is why `symphonia` is a direct dep.
  - `flac_has_seektable`, `build_flac_seektable` — hand-rolled FLAC binary format code: walks frame headers via symphonia's demuxer, writes a SEEKTABLE block into the file's PADDING block. Runs on a background thread (see below).

## Playlist

- `playlist: Vec<PathBuf>` — ordered list of tracks; no duplicates (checked on add)
- `current_index: Option<usize>` — index of currently playing track in playlist; `None` = not playing from playlist
- `shuffle: bool` — shuffle mode enabled
- `repeat: bool` — repeat mode enabled
- `shuffle_order: Vec<usize>` — shuffled indices for shuffle playback
- `shuffle_pos: usize` — current position in shuffle_order
- **Add Files** button opens native multi-select dialog (`TPlayApp::audio_dialog()`, pub(crate)); skipped if already in playlist
- Click track name to play; drag track name to reorder; ✕ button removes track
- **Shuffle** (🔀) — plays each track once in random order; new tracks added go into unplayed pool; clicking a track resets shuffle
- **Repeat** (🔁) — loops playlist (sequential) or shuffle cycle
- ▶ plays first track (or first shuffled track if shuffle on); ⏮ previous track; ⏭ next track
- Auto-advance (`advance`) fires on natural track end (sink empty, unpaused, `current_path` set), called from `TPlayApp::update()` every frame
- **Save Playlist** / **Load Playlist** buttons — manual save/load to `~/.config/tplay/playlist.json`

## Docking (egui_dock)

- Five panes as tabs: **Track**, **Seek**, **Controls**, **Playlist**, **Equalizer**
- Default layout: top row (Title+Seek+Controls tabs) ~35%, bottom (Playlist) ~65%
- Drag tabs to reorder; drag onto split overlays to dock left/right/top/bottom/center
- Resize panes via draggable splitters
- Tear tabs off into floating windows
- Close pane via tab X button; reopen via `+` dropdown on any tab bar
- Tab bodies are wrapped in a ScrollArea by egui_dock; the EQ pane disables both scrollbars (`scroll_bars` → `[false, false]`) and sizes its 10 bands to the pane width, so nothing can overflow
- Layout auto-saves to `~/.config/tplay/dock_layout.ron` (RON via serde) and restores on startup

## Non-obvious machinery

- **Seek fast/slow path** (`seek` in app.rs): try `sink.try_seek()`; if it fails, reopen the file and play through with `source.skip_duration(target)`. FLAC without a seektable cannot seek fast — hence the builder.
- **EQ Source wrapper**: Custom `rodio::Source` that chains 10 biquad filters in series; applies per-sample in `next()`. Uses RBJ peaking EQ coefficients. Sample rate from inner source. Decoder outputs i16, converted to f32 via `convert_samples::<f32>()` before EQ. **EQ settings are live**: gains live in `Arc<Mutex<EqShared>>` shared with the GUI; `EqSource::refresh()` swaps in fresh coefficients (and resets filter state, so no click) within a sample of a slider move. No sink rebuild — the track never restarts. 0 dB is an exact identity filter (RBJ peaking at A=1), so enabled/disabled needs no bypass path.
- **FLAC seektable build**: loading an FLAC lacking a SEEKTABLE block spawns a thread running `build_flac_seektable` (~1 s). Field `seektable_ready: Option<Arc<AtomicBool>>`: `Some(flag)` while building, `None` = file already had one or isn't FLAC (fast path allowed, via `unwrap_or(true)` in `seek`). While building, seeks fall to the slow path.
- **Slider pinning**: `seek_target: Option<f32>` holds the seekbar at the intended position until `sink.get_pos()` catches up; prevents snap-back to 0 during slow-path seeks.
- **Sink replacement on load/seek**: replacing the `Sink` drops the old decoder and releases the file handle (required on Windows while the background thread writes the seektable).
- **Playlist auto-advance** (`advance` in app.rs): runs every frame; fires only when a track drained naturally — sink empty, unpaused, and `current_path` set. The `current_path.is_none()` guard is what stops a failed load from cascading through the whole list one entry per frame.
- **Per-frame UI state in egui Memory**: `TPlayApp` has zero UI state. The seek slider position (`SEEK_ID` in seekbar.rs) and the drag-and-drop state (`tplay.drag_from` / `tplay.drag_hover` in playlist.rs) are stored via `ui.ctx().memory_mut` so they survive across frames and between panes.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.
- **Shuffle order**: `shuffle_order` is a Fisher-Yates shuffled permutation of playlist indices. `shuffle_pos` tracks progress. New tracks inserted randomly into the unplayed portion. Clicking a track directly resets shuffle. Reordering/deleting regenerates shuffle order.
- **Dock state in egui Memory**: `DockState<Pane>` + `dock_open` + `dock_pending_add` live in `ctx.data()` under `tplay.dock_state`, `tplay.dock_open`, `tplay.dock_pending_add`. Pure UI state — not in `TPlayApp`. Layout persists to disk each frame.

## Dependencies — each one is load-bearing

| dep | why |
|---|---|
| eframe | GUI + windowing (egui) |
| rodio (`symphonia-all`) | decode + playback: mp3, flac, ogg, wav, m4a |
| symphonia (direct) | MP3 duration probing rodio can't do; FLAC frame walking in the seektable builder |
| rfd | native file dialog |
| serde / serde_json | playlist save/load (JSON) + dock layout (RON via serde) |
| dirs | cross-platform config directory (`~/.config/tplay/`) |
| fastrand | fast RNG for shuffle (zero-dep) |
| egui_dock | docking layout: tab drag/split/tear-off; serde for layout persistence |
| ron | RON serialization for dock layout (human-readable, no schema needed) |

`[profile.release] opt-level` is deliberately absent — 3 is Cargo's default.

## Build / verify

- `cargo check` — compiles fast with cached target/.
- Remember to update this file after every significant change
