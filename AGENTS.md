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
- `src/audio/viz.rs`: visualization tap source, ring buffer, FFT, per-bin smoothing, log-spaced bands, wave envelope, and Chladni mode picker.
- `src/audio/balance.rs`: balance control via `BalanceSource` reading `Arc<RwLock<f32>>`.
- `src/gui/`: panes, dialogs, docking, and themes. `src/gui/coordinator.rs` draws the UI; panes receive the app, theme state, and a `Ui`.
- `src/gui/panes/visualizer/`: 9 views (Bars, Wave, Radial, Spectrogram, Flame, Chladni, Nebula, Plasma, Trails); 4 are shader views via `gpu.rs`.
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

## Visualization system

- **Tap source**: `TapSource` wraps `EqSource` (post-EQ), mono-downmixes, pushes into `VizBuf`, forwards samples untouched.
- **Ring buffer** (`VizBuf`): 16384 mono samples (`VIZ_BUFFER_CAP`), cursor-based smooth scrolling. Fixes stutter from cpal burst delivery (8192 samples per callback every ~186 ms). Cursor advances by `rate * dt` per frame, consuming backlog over subsequent frames. `read_window` (advancing), `read_window_peek` (non-advancing), `advance` (cursor-only), `take_arrival` (consumes arrival flag).
- **FFT**: 1024-point radix-2 Cooley-Tukey with Hann window (`FFT_SIZE`), built once via `LazyLock`. Output: 32 log-spaced bands (`VIZ_BANDS`) at 20 Hz .. Nyquist of source's actual sample rate.
- **Smoothing**: per-bin attack/release via `smoothing_for_a_dt(dt, tau_ms)` — time-constant based, not per-frame fraction, so behavior is frame-rate independent. `DB_FLOOR = -60.0 dB` shared constant across DSP and all shaders (interpolated into GLSL prelude).
- **Wave envelope**: `compute_wave` → `WAVE_BUCKETS=128` mirrored peak envelope (GL 3.3 `MAX_FRAGMENT_UNIFORM_VECTORS >= 224` limits `u_bands[32]` + `wave[128]` = 160 vectors).
- **Chladni mode picker**: `mode_now` — continuous dual barycentre read over two disjoint band ranges (`MODE_BAND_LO..=MODE_BAND_SPLIT` and `MODE_BAND_SPLIT+1..=MODE_BAND_HI`), mapped to `1..=MAX_MODE` (10). One-pole settle with `PLATE_TAU=0.25s`. `MODE_POWER=8.0` weights toward peaks (soft argmax). Degenerate `n==m` structurally unreachable via `MODE_SPLIT`.
- **Views** (9): Bars, Wave, Radial, Spectrogram, Flame, Chladni, Nebula, Plasma, Trails. Nebula/Plasma/Chladni/Trails are shader views via `gui/panes/visualizer/gpu.rs` (fixed uniform block, generated prelude, single vertex shader, `Shape::Callback` contract). Trails uses ping-pong `Fbo` pair for feedback (decay via `feedback_for_a_dt`). `SHADER_VIEWS` table in `views/mod.rs` dispatches; tests sweep this table.
- **Color rule**: CPU views use `palette` tokens; shaders must read palette uniforms (tested by `no_shader_hardcodes_a_colour` + `every_shader_reads_the_palette`).
- **Per-view egui memory keys**: smoothing/history/feedback keyed by view — switching views does not share state (`every_view_keys_its_own_memory`).

## Library and tracks

- The library database is `~/.config/tplay/library.json`; settings are in `~/.config/tplay/config.json`.
- `TrackDb` keeps tag data and play statistics in separate maps. A rescan must not overwrite play counts or `first_seen`.
- Malformed config/database files are copied to `.broken` rather than silently overwritten. Config writes use write-then-rename.
- Track ratings are stored in the audio file's Popularimeter tag. Clearing a rating removes the tag because the format cannot represent an unrated value.
- Track operations should use `tracks.rs` rather than branching on local versus remote paths in callers.
- SMB playback is spool-then-play. Cache paths retain the source extension so rodio can identify the format.
- Playlist extensions are `.tplay`, `.m3u`, `.m3u8`, and `.pls`. Preserve `smb://` entries as URIs; do not join them as relative paths.
- Local volume discovery reads `/proc/self/mounts` and `/dev/disk/by-label/`; pseudo-filesystems, boot mounts, and network mounts are excluded.
- **Volume label decoding**: `/proc/self/mounts` uses octal `\NNN` escapes; `/dev/disk/by-label/` uses hex `\xNN`. Both escape literal backslash first (`\134`, `\x5C`), so decoding is unconditional and safe.
- **`tracks.rs` unified identity**:
  - `open/info/probe/cover/is_ready`: take track id (local path or `smb://` URI), resolve internally via `local_file_in`/`local_file_now_in` (injected spool dir for tests).
  - `normalize`: canonicalize local (drop missing), keep remote verbatim.
  - `split_for_tags`: splits batch into local/remote, skips cached — cache test lives here to avoid per-frame `PathBuf` clones.
  - `TagReader`: owns local scan receiver + remote tag request logic; `request` → `drain_into`/`absorb`.
  - `write_tags`: single write path; creates tag if missing; removes rating tag to clear; refuses remote tracks.

## iPod

- `src/ipod/` supports copying albums/playlists to a mass-storage iPod, removing tracks, and mounting/ejecting the device.
- The shared `Job`/busy state coordinates the four transfer directions.
- iPod database and artwork changes must preserve the database signature and record relationships. Batch removals into one database write rather than writing once per track.
- Device browsing walks nested folders under `iPod_Control/Music`; do not assume tracks are directly inside the selected directory.
- Keep artwork records intact when removing a track unless the operation safely rebuilds the whole artwork database.
- **AAC conversion**: target bitrates 128/256/320 kbps (`AAC_BITRATES`), configured via `Config::ipod_aac` (None = copy as-is).

## SMB network

- **Worker thread**: single `std::thread` with tokio current-thread runtime, processes `SmbCmd` sequentially (ListShares/ListDir/Spool/Save/Tags). Replies via std mpsc drained per frame by `Network::drain`.
- **Credentials**: `ServerCfg` (host + username) persisted; passwords session-memory only (`Network::passwords`), never written to config.
- **URI model**: `smb://host/share/rel/path` carried as `PathBuf` in playlist. `split_uri`/`share_uri`/`dir_uri`/`child_uri`/`uri_parent`/`parse_server_input`/`is_remote`/`is_self_or_parent`.
- **Spool cache**: `<cache>/tplay/smb/` with FNV-1a 64-bit key (`spool_key`), extension preserved. `cache_path_in` (injected dir for tests).
- **Eviction**: `Config::spool_cache_mb` (default 2048 MiB). `select_evictions` (pure): largest-unmarked-first, **never a played file** (`mark_played` exempts). Runs after tag batch, not per frame. Keyed by stem, not filename — `CacheEntry` carries `path` to avoid re-walk.
- **Tag fetching**: `Network::fetch_tags` filters by in-flight set, `TAG_ATTEMPTS_MAX=3` retry cap, groups by host (creds per command). `run_tags` spools WHOLE file → reads tags from cache copy (prefix reads cannot serve WAV/M4A/FLAC/MP3 reliably). Results: `Ok` cached even if blank (definitive "no tags"), `Err` NOT cached (retryable).
- **In-flight slots** (feed `busy()`): `pending` (playback spool), `fetch_req` (playlist download), `saving` (count, not bool — multiple saves queue), `tagging` (set of URIs with tag request in flight). Stale replies dropped by host/uri match.

## GUI, themes, and docking

- Themes are data-driven. `Themes::load()` searches user config themes, themes beside the executable, and `./themes/`, merging by theme ID. User themes win ID clashes.
- **Theme token groups** (all optional, defaults in code):
  - `Palette`: 13 colors (bg, panel_bg, text_primary, text_secondary, accent, border, progress_fill, slider_track, slider_handle, row_even, row_odd, btn_hover, focus_ring)
  - `Layout`: EQ sizing (eq_slider_min_h/max_h, eq_band_w_min, eq_header_gap, eq_band_gap)
  - `Type`: Family (Proportional/Monospace) + 4 role sizes (heading, body, meta, time)
  - `Chrome`: Widget appearance (radius, border_w, fg_w, focus_w, button_border, cursor_w, handle_aspect, disabled_alpha, expansion, weak_text, text_edit_bg, window_stroke, window_radius, backdrop, motion, item_gap, button_pad, menu_pad)
  - `Rows`: List row chrome (height, radius, stripe_w, inset, gap, trailing_w, dim, tint_alpha, header_h, title_floor, cell_gap, columns[4] sized to `SORT_OPTIONS.len()`)
  - `Panes`: Per-pane sizing (topbar_h, logo, chrome_icon, transport_icon, small_icon, folder_icon, tiny_icon, toolbar_icon, volume_icon, sidebar_w, sidebar_row_h, sidebar_row_w, sidebar_server_w, sidebar_form_w, breadcrumb_h, breadcrumb_max_w, field_w, seek_min_w, meta_w, eq_band_w_max, confirm_w, edit_w, edit_label_w, edit_star, nocover_frac, gap, pad)
  - `Dock`: Tab strip + splitter chrome (tab_h, tab_radius, tab_bg, tab_bg_active, tab_stroke, drag_fill, tab_text, tab_text_active, split_w, split_color, split_color_active)
  - `Viz`: Visualizer palette overrides (bg, accent)
  - `Frames`: 9-sliced bitmaps (titlebar, pane, eq) with `Stretch` insets + `smooth` filter. `nine_slice` rendering.
  - `Faces`: Button face grids (4 states: normal/pressed/disabled/lit) keyed by `Icon` slot. `Face::cell_for` picks by enabled/hovered/pressed/lit.
  - `Knobs`: Slider knob grids, same shape as Faces.
  - `Digits`: Digit strip for time display (character grid).
- Icons: per-theme SVGs under `<theme>/icons/`, rasterized at startup/switch via `resvg` with 4× supersampling + box-average on premultiplied alpha. Fallback: default theme's icon → unicode glyph.
- Frames/faces: decoded to textures from `<theme>/frames/` and `<theme>/faces/`; **no cross-theme fallback** (a frame is a whole surface).
- `themes/recolor_icons.py` only changes SVG `fill`/`stroke` hex values. Do not change path data or recolor `fill="none"`.
- Dialogs are egui `Modal`s with state in egui memory; keep UI-only state out of `TPlayApp`.
- Dock layout saved in `~/.config/tplay/dock_layout.json` on change or close. **Named layouts** under `~/.config/tplay/layouts/` (save/load/delete via ☰ menu). `apply_min_pane_sizes` enforces measured content minimums.
- With egui 0.36, draw panels inside provided `Ui`; use `DockArea::show_inside`. In headless GUI tests, dispose `TexturesDelta` each frame.
- Do not use `Fonts::has_glyph` to verify glyph support; inspect `Font::characters()` instead.
- Window controls: decorations off; top bar carries minimize/maximize/close via per-theme icons. Bar drags window via `StartDrag`.

## Album Cover pane

- Embedded art (ID3v2/MP4/Vorbis pictures) via `lofty`; fallback to `folder.jpg`/`Folder.jpg`/`cover.jpg`/`Cover.jpg`/`folder.png`/`cover.png` beside track (`COVER_FILES`).
- Texture cached in egui memory keyed by track path; `None` caches "checked, no art" so artless tracks don't re-scan.
- Letterboxed into pane; themed placeholder at `theme.panes.nocover_frac` of pane short side (icon → glyph fallback).
- Remote tracks: art read from spool-cache copy (playback uses same cache). Sibling-file fallback looks beside cache copy (misses folder art on shares — would need second SMB transfer).

## Smart Views (`library_db::SmartView`)

- Rule-based track selection (AND of constraints), saved in `library.json`.
- Constraints: `unrated`, `rated_min`/`max`, `added_within_days`, `top_played` (ranking, not filter — orders by play count, ties by track id, never pads with unplayed), `genre`/`year` (stored in rule, not in `TrackInfo`).
- Empty rule matches nothing (not everything) — `an_empty_rule_matches_nothing_rather_than_everything`.
- View identified by name; `add_view` replaces same-named.
- What a view sees = tag cache contents (folders browsed this/previous sessions). No indexer.
- `db.select(rule, epoch)` → track list for playlist load via `TPlayApp::load_smart_view`.

## Playlist operations (`playlist.rs`)

- `sort_tracks(&mut tracks, cache, col)` — by `SORT_OPTIONS` column (Title/Artist/Album/Duration).
- `reverse_tracks`, `shuffle_tracks` (XorShift64 RNG), `dedup` (keeps first occurrence, collapses non-adjacent repeats, share+local mount of same file = two tracks).
- Stateless, pure functions tested in `playlist_tests.rs` / `playlist_edit.rs`.

## Transition system (`audio/transition.rs`)

- `ArmInput` → `ArmPlan` (pure): crossfade/gapless decision, gain math, readiness checks (next track pre-buffered, duration known).
- `xf_gains(remaining, crossfade, crossfade_secs)`: equal-power fade; gapless = (1.0, 0.0) during pre-buffer.
- `seek_or_skip(source, target)`: decoder seek vs decode-to-position.
- `build_gapless_next(track, eq_shared, balance_shared, viz)`: constructs incoming source with EQ+balance+viz tap.
- All pure, tested in `gapless_crossfade.rs` (28 tests).

## EQ system (`audio/eq.rs`)

- 10 bands (31 Hz – 16 kHz), ±12 dB clamp.
- Presets: Flat, Rock, Pop, Classical, Jazz, Bass, Treble, Vocal.
- `EqSettings` owns live gains + enabled; `EqSource` reads without sink rebuild.
- `EQ_BANDS = 10` constant shared with preset definitions.

## Balance control (`audio/balance.rs`)

- `BalanceSource` reads `Arc<RwLock<f32>>` per audio frame; no sink rebuild.

## Config persistence

- `config.json` + `library.json` + `dock_layout.json` all use same policy.
- `Persisted` struct: last content written + timestamp. `due(now, closing)` → skip serialization inside debounce window (0.5s). `wants_write(content, now, closing)` → content compare after debounce.
- `note_written` called **before** write (failed write not retried every frame).
- `flush_config` / `flush_db` call `snapshot` (every field explicit — new field = compile error), serialize, compare, write via `atomic_write` (`.part` → rename).
- `VizView` deserialization (`de_viz_view`): accepts both variant name and dropdown label; unknown falls back to default without discarding other fields.
- Unknown `viz_view` values must fall back to the default without discarding other config fields. Keep serialized config spelling compatible with existing files.
- `Config::Default` and serde defaults for absent fields must agree. Missing files mean fresh installs; malformed files are backed up.

## Logging and persistence

- `logging::init()` installs the single stderr logger and should run before config/theme loading.
- Logs use elapsed time since initialization. App-local targets are omitted; foreign targets are retained to preserve diagnostic origin.
- Avoid logging inside per-frame polling loops unless the loop actually finds work.
- Config and library writes are content-compared and debounced. Preserve deterministic serialization; unordered map output can make unchanged data appear changed every frame.

## Tests

- Keep tests in `tests/`; do not add inline tests under `src/`. `tests/no_inline_tests.rs` enforces this.
- Prefer testing pure logic directly. Avoid maintaining mirrored implementations when production logic can be extracted and tested.
- Use temporary directories and injected paths for tests. Do not let tests read or write the user's real library/config.
- The playback integration harness uses a software mixer and pumps its source. `Player::try_seek` blocks until the mixer services the request; use the test harness's concurrent pump (`pump_during`) for live-player seeks.
- Headless GUI tests can test layout and emitted callbacks, but not the actual cpal audio callback. A wGPU callback test may use software Vulkan (`mesa-vulkan-drivers`).
- **Test fixtures**: `tests/fixtures/` — eight committed real audio files (one per format: MP3, FLAC, OGG, WAV, M4A, AIFF, AAC, WMA), 12.27s / 44.1 kHz recording (CC0, freesound #246148). `tests/common.rs` builds minimal fixtures for formats without encoder deps.
- **Relevant suites**:
  - `config_persistence.rs` — serde round-trip, defaults, JSON keys, `should_flush` throttle, unknown `viz_view` fallback
  - `library_db.rs` — two-map invariant, rescan preserves play counts/first_seen, Smart Views, malformed file handling, file written in key order
  - `playlist_shuffle.rs` — peek/commit invariant (both directions), reset semantics, pool membership
  - `playlist_edit.rs` — content ops + state repairs, shuffle cycle through app is permutation, stale drag index guard
  - `playback_integration.rs` — real `TPlayApp` + software mixer, advance(), seek, crossfade
  - `gapless_crossfade.rs` — `fade_gains`/`xf_gains`/`arm_plan`/`build_gapless_next` (28 tests)
  - `eq_live.rs` / `eq_tests.rs` — EQ settings, presets, live source
  - `viz_tests.rs` — `VizBuf`, FFT, `compute_bands`/`compute_wave`, Chladni mode picker
  - `gui_tests.rs` — pane geometry, sidebar traps, visualizer fill geometry (convex polygon + feathering), shader view properties (harness never names view, shader table agreement, per-view memory keys, callback count/rect), row culling, dock layout
  - `tracks_tests.rs` — `tracks` module hermetic (local_file_in/now_in, remote id readers, normalize, split_for_tags, TagReader, write_tags)
  - `smb_helpers.rs` — URI/spool hermetic (is_remote, split_uri, parse_server_input, uri builders, FNV-1a, cache_path)
  - `library_tests.rs` / `library_search.rs` — tag reading, sorting, search filter
  - `tag_cache.rs` — cross-scan behaviors (idle reader, new request replaces in-flight)
  - `balance_tests.rs` — curve, mono/stereo routing, bit-transparent passthrough, live change mid-stream, seek clears half-frame
  - `ipod_tests.rs` — device discovery, DB/artwork, copy/remove, mount/eject
  - `mounted_volumes.rs` — volume parsing with injected label maps
  - `font_fallback.rs` — bundled fonts miss U+2010, fallback installs it
  - `contrast_tests.rs` — WCAG contrast of bundled themes
  - `layout_save_load.rs` — named layout round-trips, finite viewport fix
  - `audio_fixtures.rs` — fixture validation, duration assertions
  - `seek_position.rs` — seek arithmetic
  - `settings_owners.rs` — four state groups (Prefs, EqSettings, ThemeState, LibraryState)
  - `ui_polish_tests.rs` — theme apply, layout tokens, icon_button lit state
  - `drag_drop.rs` — move_track/remove_track index math
  - `cover_tests.rs` — album art reading

## Common edit guidance

- Add a setting to both the live preference owner and config serialization/defaults; clamp values on load as well as in setters.
- For new theme-dependent UI, add a theme token rather than a hardcoded value.
- When adding audio formats, verify both decoding and agreement between decoder duration and tag-reader duration.
- When changing playlist operations, preserve the current track by track identity rather than slot index, and preserve the shuffle peek/commit invariant.
- When changing SMB behavior, test URI parsing and path joining with remote URIs as well as local paths.
- For iPod database changes, verify signatures, artwork references, backups, and batch-write behavior.
- When adding a visualizer view: `views/<name>.rs` + `VizView` variant (`ALL` + `name`) + one match arm in `gui/panes/visualizer.rs`, update `ALL`'s pinned length in `tests/app_integration.rs`.
- When adding a **shader** visualizer view: same six steps (add `FRAG` const + `SHADER_VIEWS` row); `gpu.rs` is finished — `the_gl_harness_never_names_a_view` fails if you edit it. Palette uniforms only.
- When changing docking behavior: `gui/coordinator.rs` (`apply_min_pane_sizes`, `default_tree`, `save_layout`/`load_layout`/`list_layouts`, Layouts menu section).
- When adding a config field: `config.rs` — `Config` field + `default_*` fn. Then either `TPlayApp` field (both `new()` and `snapshot`, which lists every field explicitly) or, for a playback setting, `Prefs` field + clamping setter + `TPlayApp` delegate.
- When adding a playback setting: `config::Prefs` (field + clamping setter) + `Config` field; GUI reaches it as `app.prefs()`.

## Deferred / unsupported

- ReplayGain/loudness normalization and per-track EQ are deferred.
- WMA, APE, WavPack, tracker modules, and MIDI are not supported by the current audio stack.
- Add a format only when its decoder and metadata/duration behavior can be verified.
- Nothing is stored or read for ReplayGain today: `TrackInfo` has no gain field; adding the field before there is a reader is a field nothing writes. When it lands it is cheap — the field rides `TrackInfo` and `library.json` carries it, and a gain is read where `duration` and `rating` are: in `read_info`.

## Remember

- Update this file after significant changes.
- Prefer deletion over addition.
- No hardcoded constants in pane code; use theme JSON or shared constants.