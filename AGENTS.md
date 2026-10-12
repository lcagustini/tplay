# tplay

Desktop audio player written in Rust, using eframe/egui for the GUI and rodio for playback. Includes local-library browsing, LAN-only SMB browsing, playlists, themes, visualizers, an equalizer, and mass-storage iPod transfer. Network access is limited to user-specified hosts; there are no internet services.

## Rules

- Read the relevant source files before changing code.
- Never screenshot or inspect the GUI visually. Use tests, logs, instrumentation, configuration, or ask the user for feedback.
- Do not hardcode constants. Use shared constants, config, or theme tokens in `themes/<id>/theme.json`.
- Keep comments concise and explain why, not what.
- Run `cargo fmt` before committing. Commit messages must not include GPG/SSH signature trailers.
- Update this file after significant changes. Prefer deletion over unnecessary additions.

## Architecture

- `src/main.rs`: window shell, audio-device setup, theme state, and frame order. Keeps the `MixerDeviceSink` alive; dropping it stops the audio callback.
- `src/app.rs`: `TPlayApp`, app state and logic; no egui/eframe dependency. `update()` advances state and reports whether repaint is needed. It receives a `Mixer`, not an audio device.
- `src/config.rs`: config-file types, defaults, playback preferences, loading/saving, and persistence throttling.
- `src/library.rs`: directory listing, tag/duration/rating reads, scanning, sorting, playlist file formats, and local-volume discovery.
- `src/library_db.rs`: persistent library tag cache, play statistics, Smart Views, and database persistence.
- `src/tracks.rs`: source-independent track access for local and SMB tracks; the only module that accesses the filesystem for track operations. Includes tag writing.
- `src/network.rs`: SMB browsing and spool-to-cache playback. Files are downloaded fully before decoding; rodio does not read directly from the network.
- `src/playlist.rs`: stateless playlist operations (sort, shuffle, deduplicate, RNG). Playlist state and shuffle navigation remain in `app.rs`; playlist file formats remain in `library.rs`.
- `src/audio/eq.rs`: 10-band EQ and presets. `EqSettings` owns shared live settings; `EqSource` reads them without rebuilding the sink.
- `src/audio/transition.rs`: pure gapless/crossfade planning and gain calculations.
- `src/gui/`: panes, dialogs, docking, and themes. `src/gui/coordinator.rs` draws the UI; panes receive the app, theme state, and a `Ui`.
- `src/ipod/`: iPod device discovery, database/artwork handling, copying in both directions, and mount/eject operations.
- `src/logging.rs`: installs the logger used by the app and dependencies.
- `themes/<id>/`: theme JSON, SVG icons, and optional bitmap frames/faces.

### Ownership boundaries

- The shell owns the `ThemeState` and audio device; `TPlayApp` owns app logic and the shared `Mixer`.
- Config-file shape and playback preferences live in `config.rs`; EQ settings in `audio::eq`; library browsing state in `library`; transition planning in `audio::transition`.
- Keep `app.rs` egui-free. Put UI state in the GUI layer and expose app-owned groups through their existing accessors.
- A method on `TPlayApp` should manage app-owned state or coordinate effects across owners. Avoid methods that merely forward a getter or setter to one owner.
- Tests should use injected paths and temporary directories, not the developer's real `~/.config/tplay` files.

## Build and verification

Run before pushing:
```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

`cargo test` runs the integration-test binaries in `tests/`. CI runs format, clippy, tests, then a release build on `v*` tag pushes only; plain pushes and pull requests do not trigger CI. The Linux CI build needs `libasound2-dev` and `mesa-vulkan-drivers`; the latter enables the real wGPU visualizer callback test through software Vulkan.

## Important dependencies and constraints

- eframe 0.36 uses `App::ui(&mut Ui, &mut Frame)`; the shell passes the `Ui` to the coordinator.
- The default eframe renderer is wgpu. Visualizer callbacks must use `egui_wgpu::CallbackTrait`; a GL callback is silently dropped by the wgpu renderer.
- rodio 0.22 uses `Player` (formerly `Sink`) and `MixerDeviceSink` (formerly `OutputStream`). `Decoder::try_from(File)` sets the byte length needed for FLAC seeking.
- wgpu is a dev-dependency because app code uses the version re-exported by `eframe::egui_wgpu`; naming it as a normal dependency could introduce a second incompatible copy.
- `symphonia` has no Opus codec in the version currently used. Raw AAC can decode, but its duration estimate from lofty does not match the decoder duration, so `.aac` is excluded. AIFF is excluded because an 8 kHz file is misread; a 44.1 kHz fixture works, so re-enabling requires rate-specific verification.
- Do not re-add `glow` or `egui_glow`; they are no longer used.

## Playback

- Balance is applied by `BalanceSource`, reading a shared `Arc<RwLock<f32>>`; it does not require rebuilding the sink.
- Gapless playback pre-buffers the next track at volume zero and switches when the current track drains.
- Crossfade plays outgoing and incoming tracks simultaneously on the shared mixer with an equal-power fade. It takes precedence over gapless when both are enabled.
- Transition decisions are kept in pure functions and tested separately. Shuffle navigation uses peek/commit operations: peeking must not mutate history or RNG; commit only when the next track is actually armed.
- `Player` sink fields may retain the names `sink` and `xf_sink` even though their type is now `Player`.
- Cancelling an active crossfade can leave metadata pointing at a track that is no longer playing. A proper fix needs the outgoing track ID; do not try to fix this by changing `cancel_xf()` alone.

## Library and tracks

- The library database is `~/.config/tplay/library.json`; settings are in `~/.config/tplay/config.json`.
- `TrackDb` keeps tag data and play statistics in separate maps. A rescan must not overwrite play counts or `first_seen`.
- Malformed config/database files are copied to `.broken` rather than silently overwritten. Config writes use write-then-rename.
- Track ratings are stored in the audio file's Popularimeter tag. Clearing a rating removes the tag because the format cannot represent an unrated value.
- Track operations should use `tracks.rs` rather than branching on local versus remote paths in callers.
- SMB playback is spool-then-play. Cache paths retain the source extension so rodio can identify the format.
- Playlist extensions are `.tplay`, `.m3u`, `.m3u8`, and `.pls`. Preserve `smb://` entries as URIs; do not join them as relative paths.
- Local volume discovery reads `/proc/self/mounts` and `/dev/disk/by-label/`; pseudo-filesystems, boot mounts, and network mounts are excluded.

## iPod

- `src/ipod/` supports copying albums/playlists to a mass-storage iPod, removing tracks, and mounting/ejecting the device.
- The shared `Job`/busy state coordinates the four transfer directions.
- iPod database and artwork changes must preserve the database signature and record relationships. Batch removals into one database write rather than writing once per track.
- Device browsing walks nested folders under `iPod_Control/Music`; do not assume tracks are directly inside the selected directory.
- Keep artwork records intact when removing a track unless the operation safely rebuilds the whole artwork database.

## GUI, themes, and docking

- Themes are data-driven. `Themes::load()` searches user config themes, themes beside the executable, and `./themes/`, merging by theme ID. User themes win ID clashes.
- Each theme JSON defines an ID, name, dark/light base, 13 palette tokens, and optional groups for typography, chrome, rows, panes, docking, visualizers, frames, faces, knobs, and digits. Optional groups default to current rendering behavior.
- Icons may fall back when missing; bitmap frames/faces do not fall back across themes.
- `themes/recolor_icons.py` only changes SVG `fill`/`stroke` hex values. Do not change path data or recolor `fill="none"`.
- Dialogs are egui `Modal`s with state in egui memory; keep UI-only state out of `TPlayApp`.
- Dock layout is saved in `~/.config/tplay/dock_layout.json` when it changes or when the app closes.
- With egui 0.36, draw panels inside the provided `Ui`; use `DockArea::show_inside`. In headless GUI tests, dispose of `TexturesDelta` from each frame to avoid debug assertions.
- Do not use `Fonts::has_glyph` to verify glyph support; inspect `Font::characters()` instead.

## Logging and persistence

- `logging::init()` installs the single stderr logger and should run before config/theme loading.
- Logs use elapsed time since initialization. App-local targets are omitted; foreign targets are retained to preserve diagnostic origin.
- Avoid logging inside per-frame polling loops unless the loop actually finds work.
- Config and library writes are content-compared and debounced. Preserve deterministic serialization; unordered map output can make unchanged data appear changed every frame.
- Unknown `viz_view` values must fall back to the default without discarding other config fields. Keep the serialized config spelling compatible with existing files.
- `Config::Default` and serde defaults for absent fields must agree. Missing files mean fresh installs; malformed files are backed up.

## Tests

- Keep tests in `tests/`; do not add inline tests under `src/`.
- Prefer testing pure logic directly. Avoid maintaining mirrored implementations when production logic can be extracted and tested.
- Use temporary directories and injected paths for tests. Do not let tests read or write the user's real library/config.
- The playback integration harness uses a software mixer and pumps its source. `Player::try_seek` blocks until the mixer services the request; use the test harness's concurrent pump for live-player seeks.
- Headless GUI tests can test layout and emitted callbacks, but not the actual cpal audio callback. A wGPU callback test may use software Vulkan.
- Relevant suites include `config_persistence.rs`, `library_db.rs`, `playlist_shuffle.rs`, `playlist_edit.rs`, `playback_integration.rs`, `gapless_crossfade.rs`, `eq_live.rs`, `viz_tests.rs`, `gui_tests.rs`, `tracks_tests.rs`, `smb_helpers.rs`, and `audio_fixtures.rs`.

## Common edit guidance

- Add a setting to both the live preference owner and config serialization/defaults; clamp values on load as well as in setters.
- For new theme-dependent UI, add a theme token rather than a hardcoded value.
- When adding audio formats, verify both decoding and agreement between decoder duration and tag-reader duration.
- When changing playlist operations, preserve the current track by track identity rather than slot index, and preserve the shuffle peek/commit invariant.
- When changing SMB behavior, test URI parsing and path joining with remote URIs as well as local paths.
- For iPod database changes, verify signatures, artwork references, backups, and batch-write behavior.

## Deferred / unsupported

- ReplayGain/loudness normalization and per-track EQ are deferred.
- WMA, APE, WavPack, tracker modules, and MIDI are not supported by the current audio stack.
- Add a format only when its decoder and metadata/duration behavior can be verified.

## Remember

- Update this file after significant changes.
- Prefer deletion over addition.
- No hardcoded constants in pane code; use theme JSON or shared constants.
