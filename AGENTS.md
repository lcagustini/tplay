# tplay

Desktop audio player. Rust, eframe/egui GUI, rodio playback. Single window, no network, no persistence. The whole codebase is 3 source files (~470 lines); read all of them before changing anything — there is nothing else to explore.

## Layout

- `src/main.rs` — bootstrap: window options (340x520 default, resizable), hands off to `gui::TPlayApp`.
- `src/gui/mod.rs` — all app state + UI in one `TPlayApp` struct; one method per UI row (`title_pane`, `seekbar_pane`, `transport_pane`, `playlist_pane`). No other modules.
- `src/audio/mod.rs` — file-level helpers, no playback logic:
  - `probe_duration` — symphonia-based duration probe; fallback when rodio's `Decoder::total_duration()` is None (mainly MP3). This is why `symphonia` is a direct dep.
  - `flac_has_seektable`, `build_flac_seektable` — hand-rolled FLAC binary format code: walks frame headers via symphonia's demuxer, writes a SEEKTABLE block into the file's PADDING block. Runs on a background thread (see below).

## Playlist

- `playlist: Vec<PathBuf>` — ordered list of tracks; no duplicates (checked on add)
- `current_index: Option<usize>` — index of currently playing track in playlist; `None` = not playing from playlist
- `drag_from`, `drag_hover: Option<usize>` — drag-and-drop reorder state (persisted across frames)
- **Add Files** button opens native multi-select dialog; skipped if already in playlist
- Click track name to play; drag track name to reorder; ✕ button removes track
- ▶ plays `playlist[0]` when nothing loaded; ⏭ advances to next playlist entry
- Auto-advance (`advance_playlist`) fires on natural track end (sink empty, unpaused, `current_path` set)

## Non-obvious machinery

- **Seek fast/slow path** (`seek_to` in gui/mod.rs): try `sink.try_seek()`; if it fails, reopen the file and play through with `source.skip_duration(target)`. FLAC without a seektable cannot seek fast — hence the builder.
- **FLAC seektable build**: loading an FLAC lacking a SEEKTABLE block spawns a thread running `build_flac_seektable` (~1 s). Field `seektable_ready: Option<Arc<AtomicBool>>`: `Some(flag)` while building, `None` = file already had one or isn't FLAC (fast path allowed, via `unwrap_or(true)` in `seek_to`). While building, seeks fall to the slow path.
- **Slider pinning**: `seek_target: Option<f32>` holds the seekbar at the intended position until `sink.get_pos()` catches up; prevents snap-back to 0 during slow-path seeks.
- **Sink replacement on load/seek**: replacing the `Sink` drops the old decoder and releases the file handle (required on Windows while the background thread writes the seektable).
- **Playlist auto-advance** (`advance_playlist` in gui/mod.rs): runs every frame; fires only when a track drained naturally — sink empty, unpaused, and `current_path` set. The `current_path.is_none()` guard is what stops a failed load from cascading through the whole list one entry per frame.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` track the drag across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.

## Dependencies — each one is load-bearing

| dep | why |
|---|---|
| eframe | GUI + windowing (egui) |
| rodio (`symphonia-all`) | decode + playback: mp3, flac, ogg, wav, m4a |
| symphonia (direct) | MP3 duration probing rodio can't do; FLAC frame walking in the seektable builder |
| rfd | native file dialog |

`[profile.release] opt-level` is deliberately absent — 3 is Cargo's default.

## Build / verify

- `cargo check` — compiles fast with cached target/.
- No test suite, no linter configured. It is a GUI app: verify behavior by running `cargo run`.
