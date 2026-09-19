# tplay

Desktop audio player. Rust, eframe/egui GUI, rodio playback. Single window, no network, no persistence. 10 source files (~980 lines); read all of them before changing anything — there is nothing else to explore.

## Layout

- `src/main.rs` — bootstrap: window options (340x520 default, resizable), hands off to `app::TPlayApp`.
- `src/app.rs` — `TPlayApp`: ALL app state + logic, zero UI code. Owns the rodio stream/sink. The GUI calls in through pub methods; read-only getters sit at the bottom.
- `src/gui/mod.rs` — declares `coordinator` + `panes`.
- `src/gui/coordinator.rs` — `update_ui(app, ctx)`: one frame's `CentralPanel`, calls the four panes in order. Nothing else lives here.
- `src/gui/panes/{title,seekbar,transport,playlist}.rs` — one free function per UI row, each taking `(app: &mut TPlayApp, ui: &mut egui::Ui)`.
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

## Non-obvious machinery

- **Seek fast/slow path** (`seek` in app.rs): try `sink.try_seek()`; if it fails, reopen the file and play through with `source.skip_duration(target)`. FLAC without a seektable cannot seek fast — hence the builder.
- **FLAC seektable build**: loading an FLAC lacking a SEEKTABLE block spawns a thread running `build_flac_seektable` (~1 s). Field `seektable_ready: Option<Arc<AtomicBool>>`: `Some(flag)` while building, `None` = file already had one or isn't FLAC (fast path allowed, via `unwrap_or(true)` in `seek`). While building, seeks fall to the slow path.
- **Slider pinning**: `seek_target: Option<f32>` holds the seekbar at the intended position until `sink.get_pos()` catches up; prevents snap-back to 0 during slow-path seeks.
- **Sink replacement on load/seek**: replacing the `Sink` drops the old decoder and releases the file handle (required on Windows while the background thread writes the seektable).
- **Playlist auto-advance** (`advance` in app.rs): runs every frame; fires only when a track drained naturally — sink empty, unpaused, and `current_path` set. The `current_path.is_none()` guard is what stops a failed load from cascading through the whole list one entry per frame.
- **Per-frame UI state in egui Memory**: `TPlayApp` has zero UI state. The seek slider position (`SEEK_ID` in seekbar.rs) and the drag-and-drop state (`tplay.drag_from` / `tplay.drag_hover` in playlist.rs) are stored via `ui.ctx().memory_mut` so they survive across frames and between panes.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.
- **Shuffle order**: `shuffle_order` is a Fisher-Yates shuffled permutation of playlist indices. `shuffle_pos` tracks progress. New tracks are inserted randomly into the unplayed portion. Clicking a track directly resets shuffle. Reordering/deleting regenerates shuffle order.

## Dependencies — each one is load-bearing

| dep | why |
|---|---|
| eframe | GUI + windowing (egui) |
| rodio (`symphonia-all`) | decode + playback: mp3, flac, ogg, wav, m4a |
| symphonia (direct) | MP3 duration probing rodio can't do; FLAC frame walking in the seektable builder |
| rfd | native file dialog |
| serde / serde_json | playlist save/load (JSON) |
| dirs | cross-platform config directory (`~/.config/tplay/`) |
| fastrand | fast RNG for shuffle (zero-dep) |

`[profile.release] opt-level` is deliberately absent — 3 is Cargo's default.

## Build / verify

- `cargo check` — compiles fast with cached target/.
- Remember to update this file after every significant change
