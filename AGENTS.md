# tplay

Desktop audio player. Rust, eframe/egui GUI, rodio playback. Single window. LAN-only network access: a built-in SMB browser (no mount) plus a Volumes list of local block partitions — no internet services, connects only to user-specified hosts. 21 source files + `themes/` data folder; read all of them before changing anything — there is nothing else to explore.

## Rules

- Never screenshot or try to see the GUI. Use instrumentation (debug prints, egui memory, config files) or ask the user for feedback instead.
- Never add hardcoded constants. Use the config and theme files (`~/.config/tplay/*`, `themes/<id>/theme.json` layout/palette tokens).

## Layout

- `src/main.rs` — bootstrap: window options (680x460 default, resizable, min 320x160, no native title bar), hands off to `app::TPlayApp`.
- `src/app.rs` — `TPlayApp`: ALL app state + logic, zero UI code. Owns the rodio stream/sink. The GUI calls in through pub methods; read-only getters sit at the bottom. **No docking state** — that lives in GUI layer.
- `src/lib.rs` — public re-exports for testing (`app`, `audio`, `gui`, `library`).
- `src/gui/mod.rs` — declares `coordinator` + `panes` + `theme`.
- `src/gui/coordinator.rs` — `update_ui(app, ctx)`: applies the theme's egui visuals, then draws one frame's `DockArea` (egui_dock). Implements `TabViewer` for `Pane` enum; calls the six pane functions in `ui()`. Loads dock layout from egui memory + `~/.config/tplay/dock_layout.json`; saves to disk only when the layout changes or the app closes. The ☰ menu re-adds closed panes.
- `src/gui/theme.rs` — data-driven theme system. `Themes::load()` scans `~/.config/tplay/themes/` (user, wins on id clash), `<exe_dir>/themes/` (shipped with the app), `./themes/` (dev: `cargo run` from repo root) and merges by theme id. Each `themes/<id>/theme.json` carries `id`, `name`, `base` (dark/light), `metadata_font` (monospace/…), and the 14-token `Palette` (`--bg`, `--accent`, `--row-even`, …) as hex strings; invalid files are skipped with an eprintln, and a hardcoded dark fallback theme guarantees a non-empty list (broken install). `apply(ctx, &Theme)` maps tokens onto egui `Visuals` each frame so a mid-session switch lands instantly. Selection persists to `~/.config/tplay/config.json` (the `theme` field); missing config = `dark`.
- `themes/<id>/icons/*.png` — per-theme icon set (logo, play, pause, stop, prev, next, shuffle, repeat, volume, remove, sort_asc, sort_desc, star_on, star_off, folder, minimize, maximize, nocover, gapless, crossfade). Missing PNGs fall back to the default theme's (`dark`), then to unicode glyphs. `theme::load_icons` decodes them synchronously into egui textures at startup and on skin switch (NO egui async loader — the URI loader path showed pending/error placeholders and stretched buttons).
- `src/gui/panes/{now_playing,playlist,equalizer,library,visualizer,album_cover}.rs` — one free function per pane, each taking `(app: &mut TPlayApp, ui: &mut egui::Ui)`. Position-independent — work identically docked anywhere.
- `src/library.rs` — pure logic: directory listing, tag/duration reading via `lofty`, background scan thread (`scan_files` + mpsc), `TrackInfo` cache, sorting (`sort_key` with DEL prefix for empty values), playlist read/write (`.tplay` JSON), **local volume discovery** (`Volume::parse_mounts`/`parse_mounts_with_labels`/`mounted_volumes` from `/proc/self/mounts` + `/dev/disk/by-label/` — pretty filesystem labels win over UUID mountpoint names; `PSEUDO_FSTYPES` + `BOOT_MOUNTPOINTS` (`/efi`, `/boot`, `/boot/efi`) excluded).
- `src/network.rs` — built-in SMB browsing client (pure Rust via `smb2`, no system mount): `smb://host/share/rel/path` URI model (`is_remote`/`split_uri`/`server_uri`/`share_uri`/`dir_uri`/`child_uri`), a spool cache (`spool_key` FNV-1a 64 + `cache_path` under `<cache>/tplay/smb/` — extension kept from the URI so rodio can sniff it), `SmbCmd`/`SmbReply` channels, the smb2 ops (`run_list_shares`/`run_list_dir`/`run_spool`), and the `Network` state object owning ALL network state (servers, session passwords, browse position, pending spool, worker channels) with the browse/spool/drain API. Playback is **spool-then-play**: download the whole file to the cache before decoding, so the rodio pipeline never touches the network.
- `src/tracks.rs` — **the source-agnostic track surface** (see **Tracks: one surface per source** below). The single place that knows a track can be local or remote: `local_file`/`local_file_now` (resolve a track to bytes on disk), `normalize` (playlist entry → the id everything stores), `split_for_tags` (batch → the two tag transports), and `TagReader` (owns the local scan receiver and applies remote tag results).
- `src/audio/mod.rs` — file-level helpers, no playback logic:
  - `probe_duration` — symphonia-based duration probe; fallback when rodio's `Decoder::total_duration()` is None (mainly MP3). This is why `symphonia` is a direct dep.
- `src/audio/eq.rs` — 10-band graphic EQ (`EqSource` wrapping `rodio::Source<Item=f32>`). RBJ peaking EQ coefficients (w3.org audio-eq-cookbook). Gains live in `Arc<RwLock<EqShared>>` shared with GUI; `EqSource::refresh()` read-locks per sample, UI write-locks — no sink rebuild, no audio restart. 0 dB = exact identity filter (A=1).
- `tests/` — ALL tests live here; each file is its own auto-discovered test binary (cargo runs each once). `tests/common.rs` provides shared helpers (temp dirs, minimal WAV/FLAC generators, duration assertions). `common::write_tagged_mp3` hand-builds a real ~125 KB MP3 with real ID3v2 tags (ID3v2.3 TIT2/TPE1/TALB + 300 silent MPEG-1 Layer III frames) — it exists because no encoder dependency is available and lofty ships no test assets, and it is the fixture the whole remote-tag path is verified against. Individual suites: `eq_tests.rs`, `playlist_tests.rs`, `library_tests.rs`, `gui_tests.rs`, plus the second-wave suites below. `playlist_tests.rs` also carries the **share-playlist** serialization rules: `smb_uris_are_not_joined_onto_the_base` (the `is_relative` trap — an `smb://` entry read with a share base must survive verbatim), `relative_entries_resolve_against_a_share_uri_base` (with a premise guard proving the cache-dir base is wrong), `mixed_playlist_roundtrips_through_json`, `default_playlist_name_keeps_the_tracked_stem` and `playlist_file_name_appends_the_extension_and_strips_separators`. `library_tests.rs` carries `read_info_gives_tags_and_a_true_duration_from_a_whole_file` (the remote path's load-bearing claim: a full read gives tags AND a duration matching the file's 300 frames, which no prefix could) and `sort_entries_orders_both_sources_the_same_way` (one sorter, and the Title column interleaving folders by their own name). `smb_helpers.rs` carries `select_evictions_never_removes_a_played_file` (must free enough AND never touch a played file, even when that means falling short) and `tag_attempts_are_bounded_and_cleared_on_success` (the retry cap, and that `busy()` ignores the attempt map so a retained failure cannot pin a permanent repaint).
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
  - `font_fallback.rs` — headless verification that bundled egui fonts miss U+2010 and `install_fallback_fonts` makes both families render it (skipped on machines with no candidate system font).
  - `mounted_volumes.rs` — `Volume::parse_mounts` fixture tests (injected label maps, hermetic): local block partitions only, pseudo-fs/boot/ESP/network excluded, pretty disk labels win over UUID mountpoint names, sorted by label.
  - `gui_tests.rs` also carries the sidebar-layout regression (`sidebar_column_and_scroll_content_ignore_textedit_overflow`): asserts the Places/Favorites column stays 120px for any address text, that the file-list sibling's `min.x` is beside it, **and** that the scroll content width does not grow — with premise guards for all three original failures. **Three independent egui traps, all hit here:** (1) *width* — a `ui.vertical` child is sized by its own `min_rect`, and egui's `TextEdit` **deliberately** grows that by the text overflow (`builder.rs`: "allocate additional space … so a ScrollArea can properly scroll to the cursor"). Inside a vertical-only ScrollArea, `auto_shrink([true, false])` means `scroll_enabled[0] == false`, so the column width **is** `content_size.x` and typing a long address grew the sidebar. `set_max_width`/`add_sized`/`clip_text` all fail: caps bound painting, but the overflow grows `min_rect`, and `min_rect` wins the layout. (2) *placement* — `ui.new_child` does **not** advance the parent's cursor (only `allocate_new_ui` does), so using it alone put the file-list sibling at the same x, drawing on top of the sidebar. (3) *scrollbar* — fixing the column alone still left the scroll **content** growing (100→258px), which kept dragging the scrollbar; the form needs the same fixed-rect treatment. The working shape at both levels is `ui.allocate_space(vec2(w, h))` — reserves the rect *and* advances the cursor — followed by `ui.new_child(UiBuilder::new().max_rect(that_rect))`, whose min_rect does not propagate upward. `SIDEBAR_W`, `FORM_W` and `FORM_FIELD_H` in `gui/panes/library.rs` are the single sources for those sizes. It also carries a fourth, unrelated trap: `right_to_left_center_does_not_swallow_the_column` — a horizontal `with_layout` with cross-axis `Center`/`Max` hands its child a `min_rect` spanning the parent's whole remaining height, and `scope_dyn` advances the parent cursor by it, so a one-row header consumed a 444px column and pushed the file list below the pane. `Align::Min` is the fix; the assert is on geometry (is the list still on screen), not on the align token.
  - `tracks_tests.rs` — hermetic tests for `tplay::tracks` (no network, no audio device): `local_file`/`local_file_now` (injected spool dir) incl. the local-missing case where they deliberately disagree, `pre_availability_decides_pre_buffering_not_the_source` (pins the gapless-on-cached-remote change), `normalize`, `split_for_tags`, `TagReader::request`/`drain_into` (real `scan_files` thread, `Network::new(vec![])` sends the worker nothing) and `TagReader::absorb`'s cache-`Ok`-not-`Err` asymmetry. `tag_cache.rs` carries the two cross-scan behaviours that cannot be seen from inside one scan (idle reader, and a new request replacing an in-flight one).
  - `smb_helpers.rs` — hermetic URI/spool tests for `tplay::network` (no network, no worker): `is_remote`, `split_uri` across server/share/dir/file shapes, `parse_server_input` (bare host, scheme-less `host/share`, full `smb://host/share[/dir]`, whitespace, empty), `add_server` dedup-by-host + username update (the login prompt re-adds the same host, so it must update in place), uri-builder round-trips, `nav_uri_round_trips_but_renaming_one_does_not` (a share row's path is a full URI — splitting round-trips, re-joining it as a name yields `music/smb://nas/media/music/Rock` and a server-side PATH_NOT_FOUND), `uri_parent` as the inverse of `child_uri` (incl. the bare-`smb://host` case that splits into a garbage `"smb:/"`), `is_playlist` on a share filename, `is_self_or_parent` (`.`/`..` dropped, `.config` kept), FNV-1a 64 vectors + `spool_key` stability/distinctness, `cache_path_in` extension handling.

## Build / verify

- `cargo check` — compiles fast with cached target/.
- `cargo test` — runs all tests.
- `[profile.release] opt-level` is deliberately absent — 3 is Cargo's default.

## Dependencies — each one is load-bearing

| dep | why |
|---|---|
| eframe | GUI + windowing (egui) |
| rodio (`symphonia-all`) | decode + playback: mp3, flac, ogg, wav, m4a. Version must stay ≥ 0.21 — `Decoder::try_from(File)` sets the stream `byte_len`, without which symphonia's FLAC reader refuses to seek (`Unseekable`), and every FLAC seek fell back to slow decode-from-zero |
| symphonia (direct) | MP3 duration probing rodio can't do |
| rfd | native file dialog |
| serde / serde_json | playlist save/load (JSON) + dock layout (JSON via serde) |
| dirs | cross-platform config directory (`~/.config/tplay/`) |
| egui_dock | docking layout: tab drag/split/tear-off; serde for layout persistence |
| image (png+jpeg) | decodes theme icons (PNG → egui texture) synchronously at startup / skin switch, and album covers (PNG/JPEG, format sniffed from bytes) in the Album Cover pane |
| lofty | audio tag reading for every pane's track display — ID3v2, Vorbis comments, MP4 ilst, RIFF INFO (artist/album/year/genre/track + duration) |
| smb2 | built-in SMB client for the Network pane (Phase 2) — `list_shares`/`connect_share`/`list_directory` for browsing, `read_file_pipelined` for spooling, `write_file_pipelined` for saving a playlist to a share. Pure Rust, no external `mount` helper. LAN-only by design — no internet services |
| tokio (`rt`,`net`,`time`) | async runtime for the smb2 client — a single current-thread runtime owned by the SMB worker thread (one operation at a time, sequential `rx.recv()`) |

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
- **Remote tracks** (`smb://` URIs in the playlist): there is no separate code path. Every play goes through the one `play_now` entry point, which asks `tracks::local_file_now` — a cache hit plays the local copy via `load_file_as(local, uri)` (tags keyed by URI); a miss tears down the sink, clears `current_path`/`total_duration`, and asks the worker to spool it (`Network::spool` also marks it pending, and marks the track played). `Network::drain` returns the `Spooled` event when the download lands for the still-pending track (a stale reply for a superseded track is dropped) and `update()` promotes it into the sink, so Now Playing shows "Loading from server…" while `network().pending()` is set, and `advance()`'s `current_path.is_none()` guard can't cascade past the pending track. **Gapless/crossfade apply to any track whose bytes are on hand** (the xf arm asks `tracks::local_file_now`, not `is_remote`), so an already-spooled network track crossfades like a local one; `stop()` calls `discard_pending()`. A re-requested track hits the disk cache instantly. The cache is bounded only by `Config::spool_cache_mb` (see **Spool cache eviction**).
- **Save Playlist** — first save opens a native save dialog (defaults to the Library's current folder, suggested `playlist.tplay`) and writes the current playlist as a `.tplay` file (`{"paths": [...]}` only — shuffle/repeat are appwide, never playlist content; `library::write_playlist`, which is now `fs::write` over the pure `library::playlist_json`). The pane remembers the file (`playlist_file`), so later saves overwrite it directly — no dialog. Loading a `.tplay` from the Library also sets the tracked file (save writes back to the source) and **stops playback** (same as **Create Playlist** — the swap is silent); **Create Playlist** clears the tracked file so a fresh playlist asks where to go. Button hover shows the overwrite target ("Overwrite playlist: …"). A **failed** write no longer marks the playlist clean or tracks the file — that was a bug: both were set unconditionally, so a failed save claimed success and every later save overwrote a file that was never written.
- **Playlists on SMB shares** (load *and* save, no mount):
  - **Load.** A `.tplay` on a share gets a row in the remote browser (see the shared file list below). Clicking it confirms first when the playlist is dirty — the same gate as a local `.tplay` click — then `Network::fetch` spools the file. That uses `SmbCmd::Spool` but a **second slot** (`fetch_req`, separate from the playback `pending`) so reading a playlist can never displace a track that is still spooling, or vice versa; `drain` returns `Event::Fetched` and `update()` applies it via `load_fetched_playlist`. Stale replies drop against the slot.
  - **`read_playlist(path, base)` takes an explicit base, not `path.parent()`.** A remote playlist is read from its **spool cache copy**, so resolving against that file's own parent would silently drop every relative track. Local callers pass the file's directory; remote callers pass `network::uri_parent(&uri)` — the share directory the playlist was browsed at.
  - **`smb://` entries are never joined onto the base.** They only *look* relative: `Path::is_relative` is true for anything without a leading `/`, so keying off it turns `smb://nas/m/x.mp3` into `smb://nas/m/smb://nas/m/x.mp3`. The mapper branches on `is_remote` first, then `is_absolute`. Covered by `smb_uris_are_not_joined_onto_the_base`.
  - **Save.** `save_playlist_to` branches on `is_remote` → `Network::save` → `SmbCmd::Save` → `run_save` (`client.write_file_pipelined`, one compound CREATE+WRITE+FLUSH+CLOSE with `FileOverwriteIf`, so no file lifecycle to manage). The write is async, so `playlist_file`/`playlist_dirty` are only updated when `Event::Saved` confirms — dispatching is not success. On success the app also calls `refresh_network_dir` so the new file appears in the listing without navigating away and back. Entries are written as **full `smb://` URIs** (no relative rewriting), so a share playlist is portable only in the sense that it always resolves here.
  - **Where Save writes** (a native dialog cannot target a URI, so the target is a rule): `playlist_file` is already a remote URI → overwrite in place, no UI; otherwise, if a share directory is open in the Library's remote browser → an **`egui::Modal`** name field (`share_save_modal` in `gui/panes/playlist.rs`) writes into the directory on screen, mirroring how local Save defaults to the Library's current folder; otherwise the native local dialog. This is the app's **only non-native dialog** — `rfd::MessageDialog` is Yes/No with no text field, and this is the one prompt that needs input. The suggested name is `library::default_playlist_name` (the tracked file's stem) and typed input goes through `library::playlist_file_name` (appends `.tplay`, strips path separators rather than sending a bogus server path, blank cancels).
  - A remote `last_playlist` restores through the same async `fetch`, because `Path::exists()` is always false for an `smb://` path and would skip the restore silently. It fails until the user logs in again — passwords are session-memory only, so there is nothing to reuse.
- **New Playlist** — clears the current playlist (replaced the old Load button); the button label reads **Create Playlist**. Loading now happens in the Library by clicking a `.tplay` file. New and Load confirm (native Yes/No, `TPlayApp::confirm`) only when the playlist has unsaved edits (`playlist_dirty`) — a clean switch is silent. The ✕ per-row remove always confirms.
- Action buttons (Create Playlist / Save Playlist) live in a `horizontal_wrapped` row at the bottom of the pane (`item_spacing.x = 12` between them).
- **Shuffle algorithm** (in `app.rs`): `next_track_index` picks a random unplayed index (XorShift64); `prev_track_index` pops from `played` history; `repeat=true` restarts the cycle when the pool is exhausted. `reset_shuffle()` clears `played` — called on toggle, play_track, remove/move, load, stop.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory (`tplay.drag_from`, `tplay.drag_hover`) and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.

---

## Tracks: one surface per source

A local track and an `smb://` track differ in exactly one way: **timing**. A local track's bytes
are on disk now; a remote track's may be in the spool cache, or may need a fetch first. Everything
else — the id, its tags, "does it exist", "give me the file to decode" — is the same operation
over a different transport. `src/tracks.rs` is where that is expressed, so **no part of the app
branches on where a track came from**.

- **`PathBuf` is the id, deliberately.** A `smb://` URI is stored as a `PathBuf` holding the URI,
  and a URI's last segment is the filename, so it works as an id with no help: the `tag_cache` is
  keyed by it, and `title_or_stem` / `sort_key` / `is_playlist` and the search haystack all treat a
  URI and a local path alike — which is exactly what makes one shared `file_list_ui` possible for
  both browsers. A `Track` newtype would be a rename-only diff across `playlist`, `.tplay` JSON,
  `config.json` and ~20 comparisons, removing no divergence.
- **`is_remote` is called from two places only**: `tracks.rs` (every track operation) and
  `library.rs`'s `read_playlist` mapper, where the question genuinely is about path semantics.
  **Never** from `app.rs` or `src/gui/`. Two `is_remote` branches survive in `app.rs`, both
  *playlist* concerns rather than track ones: the remote `last_playlist` restore and
  `save_playlist_to`.
- **`local_file` vs `local_file_now`** — two functions, not one, and the split is load-bearing.
  They agree everywhere except a *local* track with no file on disk, where `local_file` still
  returns the path (an art read should be attempted, and reports its own "no art") and
  `local_file_now` returns `None` (there is nothing to pre-buffer). Collapsing them either blanks
  the album art of every track that fails to open, or makes `advance()` read "no file to
  pre-buffer" as "this is a remote track". Each has an `_in(dir)` variant so tests inject a spool
  dir and stay hermetic.
- **`play_now(track)` is the only way a track starts playing** — bytes on hand are loaded
  directly, otherwise it is requested and played on arrival. `play_file` (direct open,
  `current_index = None`) and `start` (playlist flow) differ *only* in the `current_index` they
  set, which is why that stays with the caller. **The trap:** calling `load_file_as` on a remote
  track does `File::open("smb://…")`, which always fails — and it fails *silently*, clearing
  `current_path` rather than erroring, so the transport play button looked fine while breaking
  network playback entirely. Route every play through `play_now`. (`play_now` is deliberately not
  named `play_track`; that name is the Playlist pane's "play row N" verb, which also resets
  shuffle.)
- **`normalize`** turns a playlist entry into the stored id: a local path is canonicalized and
  dropped if it no longer exists (a playlist must not resurrect a deleted file), and a remote URI
  is kept **verbatim** — rewriting it would break the spool-cache key that both playback and tag
  lookup use.
- **Gapless/crossfade follow availability, not source.** The xf arm asks `local_file_now`, so an
  already-spooled network track crossfades like a local one; an uncached one cannot pre-buffer and
  falls through to a natural-advance gap — the same thing a local track with a missing file
  already did. Pinned by `pre_availability_decides_pre_buffering_not_the_source`.
- **The pending state was already uniform.** `current_path.is_none()` plus `Network::pending()`
  is how the app knows a track is in flight, and that predates this section. Only the *entry
  point* used to fork.
- **The two tag transports stay two transports** — a thread with an mpsc, and a worker command
  with a reply `Event`. `TagReader` hides the split from callers without pretending the
  underlying machine is one thing.

---

## Equalizer

- 10 bands at `audio::eq::EQ_FREQUENCIES` (20, 100, 300, 600, 1K, 3K, 5K, 8K, 12K, 16K — the reference UI's labels; the same constant drives the filters *and* the pane labels, so they can't drift).
- Pane header: left `ON` toggle, then `Reset` button and preset dropdown — all three grouped in one row with matching button chrome. Presets: `EQ_PRESETS` in app.rs — Flat, Rock, Pop, Jazz, Classical, Electronic, Vocal. Curves follow sfxengine.com/blog/best-equalizer-settings-for-music (Rock ⇐ Rock/Metal, Pop ⇐ V-Shape, Jazz ⇐ Treble Boost, Electronic ⇐ Bass Boost, Vocal ⇐ Vocal Enhancement). Selecting one applies its gains immediately; any manual slider tweak switches the selection to `EQ_PRESET_CUSTOM`. Preset is derived from gains (never stored — one source of truth) in `~/.config/tplay/config.json`.
- **Live EQ**: `EqSource` chains 10 biquad filters (Direct Form II transposed) in series. `refresh()` runs per sample frame; reads shared gains under a read lock; rebuilds only changed bands (fresh `Biquad` with zeroed state — no click from stale history). `enabled=false` skips processing entirely.
- **Sliders**: vertical, trailing fill (theme `--progress-fill`), height clamped to theme layout tokens (`eq_slider_min_h`..`eq_slider_max_h`). Band width dynamic: max 80px, shrinks with spacing to fit the pane; the pane width is floored at `10 × eq_band_w_min` (no scrollbars on the EQ pane). Frequency labels use the `text_meta` size. (A "scale the slider on press" effect isn't implemented — egui has no transform API for widgets; the drag already responds instantly with `animation_time = 0`.)

---
 
## Visualizer
 
- Dockable pane (**Visualizer**), closed by default, auto-listed in the ☰ menu. No new theme tokens, no new icons.
- Header: the view selector is a dropdown (`ComboBox`) iterating `VizView::ALL`, persisted in `config.json` (`viz_view`) so the choice survives restarts.
- **Views are modular, mirroring the panes pattern**: one file per view under `src/gui/panes/visualizer/views/`, each exposing a single `pub fn draw(painter, rect, viz, palette)`; `visualizer.rs` is the dispatcher (header + `match`). Views own their per-frame state privately in egui memory (bars keeps its smoothing buffer under its own key). **Adding a view** = a new `views/<name>.rs` + a `VizView` variant in `app.rs` (`ALL` + `name` + one match arm) — no coordinator/config changes.
- Audio pipeline: `TapSource` wraps `EqSource` (post-EQ → visualizes exactly what you hear). Mono-downmixes, pushes into a capped ring buffer (`VizBuf`, ~100 ms / 4096 samples). `load_file_as` and `seek` append the tap; `load_file_as`/`stop()` clear the buffer.
- **Bars view**: 1024-point radix-2 FFT with Hann window → 32 log-spaced bands → dB-normalized (-60..0) → per-bin attack/release smoothing (classic WMP feel).
- **Wave view**: mirrored time-domain **peak envelope** downsampled from the ring buffer (one `|sample|` peak per ~2px display column, from a ~23 ms window), rendered as adjacent filled columns + outline strokes — column fills use `rect_filled` (polygon tessellation of raw sample waves rendered as garbage).
- Drawing: full-rect `ui.painter()`, palette colors only (`accent`, `progress_fill`, `bg`), `scroll_bars = [false, false]` like the EQ pane.
- No new dependencies — hand-rolled FFT and smoothing in `src/audio/viz.rs` (pure logic, tested without an audio device).
 
---
 
## Album Cover
 
- Dockable pane (**Album Cover**), closed by default, auto-listed in the ☰ menu. Library `read_cover` helper plus the theme `nocover` placeholder icon — no new theme tokens, no new app state (the texture cache lives in egui memory).
- **Art source** (`library::read_cover`): the currently playing track's embedded tag picture (lofty `tag.pictures()`, first one), else a cover file beside the track (`folder.jpg`/`Folder.jpg`/`cover.jpg`/`Cover.jpg`/`folder.png`/`cover.png` — `COVER_FILES`, same house-constant style as `AUDIO_EXTENSIONS`). **A remote track resolves through `tracks::local_file` first**: `read_cover` does a filesystem read, and there is no file on disk named `smb://…`, so passing the URI directly always failed and the pane showed the placeholder. `local_file` maps a remote URI to its spool-cache copy (`None` if not spooled) and returns a local path unchanged — *not* `local_file_now`, because a local track whose open failed should still be attempted here so this pane reports its own "no art". The pane's texture cache stays keyed by the URI, and no re-resolve case is needed: a remote track can only become *current* after being spooled, since playback uses the same cache. Reads on demand when the track changes — **not** part of the folder tag scan, so no MBs of art bytes in the background cache.
- **Decode**: `image::load_from_memory` (PNG+JPEG features) sniffs the format from magic bytes — no mime plumbing. Synchronous on the UI thread, one decode per track change (small, single-digit ms).
- **Cache**: egui memory under `tplay.cover` holds `(path, Option<TextureHandle>)` — keyed by the playing track's path, so a track with no art re-reads nothing until the track changes (the `None` half caches "checked, no art").
- **Drawing**: full-rect `ui.painter()`, `p.bg` fill like the Visualizer, art letterboxed centered with aspect ratio preserved; no art → the theme's `nocover.png` placeholder (128×128, panel_bg rounded square + text_secondary note, baked at generation; fallback chain: own → default theme → 🎵 glyph via `painter.text`), centered at pane-relative size. `scroll_bars = [false, false]` like the EQ/Visualizer panes.
 
---
 
## Library

- Filesystem browser (`TPlayApp::navigate_to`): the current folder's subfolders + audio files rendered as one combined row list (`library::Entry::Dir`/`File`). Subfolders are `📁` rows, audio files are tag-table rows. **There is no `..` row** — the breadcrumb is the only way up (it walks every ancestor and makes each non-last segment a jump target, so one level up is the second-to-last segment). Dot-prefixed (hidden) subfolders are skipped by default — a "Show hidden folders" checkbox in the ☰ menu toggles it (`TPlayApp::show_hidden`/`set_show_hidden`, persisted in `config.json`).
- **Header**: clickable breadcrumb to the current folder (root → current, every ancestor a jump target, middle segments collapsed to `…` beyond depth 3, full path on hover) + ★ toggle. The "N tracks · M folders · K playlists" count and the **Add All** button are *not* here — they moved into `list_header_right`, drawn just above the list so the SMB share browser gets them too. The 6-column sortable header renders **only when the folder has audio files** — folders/playlists-only folders skip it (no tag columns to align to).
- **One file list, two sources** (`file_list_ui` in `gui/panes/library.rs`): the search box, the sortable 6-column header, the folder/track/playlist rows and the "Scanning…" note are drawn once for `&[library::Entry]`. The local browser passes `library_entries()`; the share browser passes entries whose `path` is the file's `smb://` URI. That works because a URI is just a path whose last segment is the filename, so `title_or_stem`, `sort_key`, `is_playlist` and the search haystack all behave identically — which is what gives a share real tag columns, column sorting and search instead of the name+size list it started with. `parent_dir` used to draw the `..` row and is gone — the breadcrumb covers it (see the filesystem-browser bullet). Rows return an `Act` (`Nav`/`Play`/`Add`/`LoadPlaylist`) and each caller carries it out, since only it knows that `Nav` means `navigate_to` or `browse_open`, and that `LoadPlaylist` is synchronous locally but a spool-then-load on a share. `library::sort_entries` is the one sorter both use; descending is a `reverse()` after the ascending sort so ties don't reshuffle on every toggle.
- **`list_header_right` must use `Layout::right_to_left(Align::Min)` — the align token is load-bearing, not cosmetic.** A horizontal `with_layout` whose cross-axis align is `Center` or `Max` gives its child a `min_rect` spanning the parent's **whole remaining height** rather than the height actually used, and `Ui::scope_dyn` ends with `advance_cursor_after_rect(child.min_rect())` — so the parent cursor jumps by that whole span and `available_height()` collapses to 0. Measured in a 460px window against this pane's real shape: `Center` consumed **430px of a 444px column**, `Min` consumed the **21px** it used. With `Center` the list's ScrollArea was positioned at y≈497 — off the bottom of the pane — leaving only the Add All row visible beside the sidebar. **The damage scales with the parent's available height**, which is why it is invisible elsewhere: every other `with_layout` in the app sits under a *horizontal* parent (`coordinator.rs` top bar, `now_playing.rs` balance row, `playlist.rs` rows, `visualizer.rs` header), where the child's available height is one row; the one other vertical-parent site is `now_playing.rs`'s transport block, whose pane is ~15% of the window. Regression test: `right_to_left_center_does_not_swallow_the_column` in `gui_tests.rs` (premise-guards the trap, then asserts the geometry).
- **Playlist files as rows**: `.tplay` files (see Playlist) are listed interleaved with the songs (`library::is_playlist` widens `list_dir`'s filter). A playlist row shows its full filename plus a "Playlist" tag right after the name (not floated to the far right), no tag cells and no `+`. Click → confirm (only if the current playlist has unsaved edits) → `load_playlist_from`, dumping the saved tracks into the Playlist pane for editing. Playlists are **not** tag-scanned (`navigate_to`/`library_scanning` exclude them, or "Scanning…" would stick) and **Add All** skips them — a playlist file isn't audio.
- **Places**: a fixed section above Favorites listing common user folders (`TPlayApp::quick_folders` — Home + XDG Music/Downloads/Desktop, missing dirs skipped, dupes dropped); same row style as favorites without the ✕.
- **Volumes**: local block partitions from `/proc/self/mounts` (`library::Volume::mounted_volumes`) — pretty disk labels win over UUID mountpoint names; click to jump (same row style as Places, no ✕).
- **Network**: built-in SMB browsing (no mount). The sidebar's "+" opens a **host-only** add-server form (a bare host **or** a full `smb://host/share[/dir]` URI via `network::parse_server_input`, scheme optional): the host alone is saved and listed, while a named share drops straight into that share via `browse_open` — the GNOME-Files path that skips `list_shares` (IPC$ + srvsvc RPC) entirely, so servers that need it are still reachable. **Credentials are prompted for in the main pane, not the sidebar** (`login_form_ui`, rendered by `remote_list_ui` whenever the error is `LOGON_FAILURE`/`ACCESS_DENIED`), so a saved address is reused as-is and only the login is retyped. The prompt is keyed per host in egui memory (`(NET_LOGIN, host)`) and prefills the saved username; Connect calls `add_network_server` (username is persisted) + `set_network_password` (**password is session-memory only, never written to config**) and then retries **the stage it was on** (`browse_server`, or `browse_open` back into the share/dir) rather than bouncing out to the share list. A **blank username is not a guest account** — most NAS boxes reject it at SessionSetup, which is exactly what the prompt exists for. Cancel returns to the local browser (`leave_network`). Clicking a saved server swaps the main column for `remote_list_ui`: a breadcrumb (Local / host / share / dir segments — Local exits back to the folder browser), a "Connecting…" spinner or an error + hint line, then the **shared file list** — search box, sortable 6-column tag header, and folder/track/playlist rows — over entries whose `path` is each file's `smb://` URI (see **One file list, two sources**). Rows that are neither audio nor a `.tplay` are not listed, same as a local folder. `.` and `..` are dropped in `run_list_dir` (`network::is_self_or_parent`), because servers routinely include them and they are navigation artifacts, not content: they pass the `is_dir` filter, become real folder rows, inflate the "N folders" count, and produce a URI with a `.` segment the server rejects. This is **not** a hidden-file filter — a real `.config`-style folder stays reachable, because the share browser has no "show hidden" toggle to reveal it with, unlike the local browser's. This is a *different* `.`/`..` from the locally-drawn up-row that `file_list_ui` no longer has. At the share-list stage there is no directory, so the breadcrumb's "Local" plus a flat list of share names is all that's drawn. Add All, the composition counts and the search box all work here too, and add `smb://` URIs to the playlist unchanged. **Tags for a share are downloaded, not guessed** (see **Remote tags** below), so rows start as filenames and fill in — with a live count — as the files arrive. **Exactly one sidebar row is active at a time.** The server row highlights from `browse().host`, but the local rows (Places / Volumes / Favorites) must be gated on `!network_mode` — entering network mode does *not* change `library_dir`, so comparing against it directly left the last local folder lit *alongside* the server. That gate is computed once as `current_local: Option<PathBuf>` and compared by the three local rows. The reverse direction already worked: clicking a local folder calls `leave_network`, so the server row un-highlights. The server form state (`NET_FORM`) and remote browse state (`network::NetworkBrowse`) support the same click navigation as the folder browser. All network state (servers, session passwords, browse position, pending spool, in-flight tags, worker channels) lives in one `network::Network` object; the app exposes it via `TPlayApp::network()`/`network_mut()` (the only SMB methods left in app.rs are the two persistence-aware server CRUD wrappers).
  - **A share row's `path` is a full URI, never a bare name** — the single most bug-prone thing in this pane, and it shipped broken once. `remote_list_ui` builds entries with `path: child_uri(&dir, &e.name)`, so `Act::Nav` arrives carrying `smb://nas/media/music/Rock`. **Split it** (`split_uri` → `browse_open(uri, share, rel)`, a no-op round trip); never feed it to anything that appends a name. Doing that produced a `rel` of `music/smb://nas/media/music/Rock` and a **PATH_NOT_FOUND** from the server. The `child_browse(host, share, rel, name)` helper that invited the mistake — four loosely-typed positional args where a URI in the `name` slot compiles fine and only fails server-side — has been **deleted**; `child_uri(parent, name)` is the only way to build a child URI, and its argument types make the confusion impossible. Guarded by `nav_uri_round_trips_but_renaming_one_does_not`, which asserts both that splitting round-trips exactly and that the wrong construction is detectably different. The one deliberate exception is the share-list stage, whose rows are built from `RemoteEntry::name` and so really are bare names — it is the only place that calls `share_uri` directly, and its variable is named `share_name` for exactly this reason.
- **Remote tags** — how a share's rows get titles, and why it downloads the files. `ensure_tags` splits its missing set: local paths to the `scan_files` thread, `smb://` URIs to `Network::fetch_tags`, which batches by host (credentials are per-command, so a playlist mixing two NAS boxes must not send one box's password to the other) and sends `SmbCmd::Tags` → `run_tags`. **`run_tags` spools the WHOLE file and reads tags from the cache copy** via `run_spool` + `library::read_info`. That is deliberate, and it replaced a header-prefix read that could not deliver parity:
  - A prefix cannot serve every format. WAV needs its complete `data` chunk; M4A's `moov` atom usually sits at the end; FLAC and MP3 tag blocks vary in size. Measured against a real share: FLAC never tagged, and MP3 tagged only sometimes — "some worked, some only loaded when played", which is the signature of a tag block falling outside the prefix.
  - A prefix cannot give a true **duration** either. lofty derives duration from the bytes it is handed, so a 64 KB prefix of a 7.8 s track reported **4.1 s** — silently wrong, which is worse than absent.
  - The download is not wasted: `run_spool` writes into the same cache playback reads, so playing the track afterwards is a cache hit, and an already-cached file skips the network entirely.
  - The trade is bandwidth and disk for correctness: browsing a 300-file share downloads it. That is the only way a remote row can match a local one, which is the stated goal. The share browser shows a live count (`Network::pending_tags`) so the wait reads as progress.
  - Results arrive as `Event::Tagged(Vec<(uri, Result<TrackInfo, String>)>)` and land in the **same `tag_cache`**, keyed by URI, so every pane fills in with no UI work. **`Ok` is cached even when every field is blank** — that is the definitive "read it, it has no tags" answer, and `title_or_stem` falls back to the stem on an empty title, so it displays exactly like no cache entry. **`Err` is NOT cached**: a failed transfer says nothing about the file's tags, so caching it would blank the row for the rest of the session. Leaving it out is what makes it retryable.
- **Two separate tag counters, and why** (`network.rs`): `tagging: HashSet<String>` is what is **in flight** — it is what `busy()` reads (so `request_repaint` keeps coming) and what `pending_tags()` counts for the progress line. `tag_attempts: HashMap<String, u8>` counts **failures**, cleared on success, and a track that reaches `TAG_ATTEMPTS_MAX` is not asked again this session. They must not be merged: the browser asks for every uncached track *every frame*, so an unbounded retry re-queues a dead server 60×/second forever, while a retained failure mark inside the in-flight set would pin `busy()` true and spin the event loop forever. Both directions are guarded by `tag_attempts_are_bounded_and_cleared_on_success`.
- **Spool cache eviction** — the cache has no TTL and no other reason to shrink, and tagging now fills it. `Config::spool_cache_mb` (default 2048, i.e. 2 GiB, hand-editable in `config.json` and written back by `save_config` so a hand-set value survives) is the ceiling. `Network::evict_unplayed` runs after a tag batch lands — not per frame, since it walks the directory — and deletes via the pure, testable `select_evictions`: **largest unmarked first, never a played file**. `Network::mark_played` is called from inside `Network::spool` itself — a user-requested spool is one they asked to *hear*, so it is exempt from the moment they ask, including while the download is still in flight. The failure mode is therefore a re-download, never a broken play. Unmarked-ness is session state: after a restart every file looks unplayed and is therefore evictable, which is the safe direction.
- **Favorites** ★ toggles the current folder; persisted with the last browsed dir to `~/.config/tplay/config.json` (`{library: {favorites, last_dir, show_hidden}}`), restored on startup. The Places/Favorites column is a **fixed 120px** (`SIDEBAR_W` in `gui/panes/library.rs`, fits the 320px minimum window alongside the file list) and scrolls vertically when the stack outgrows the pane. It is allocated as an exact rect via `ui.allocate_space` + `ui.new_child` — *not* `ui.vertical` + `set_min_width` (which sizes the column by its `min_rect`, and egui's `TextEdit` grows `min_rect` by the text overflow, widening the sidebar while typing) and *not* a bare `new_child` (which never advances the horizontal cursor, so the file-list column landed on top of it). The **add-server form is wrapped the same way inside the scroll content** — without that, the TextEdit overflow kept growing `content_size.x` and dragged the sidebar's scrollbar around. See the sidebar-layout regression in `gui_tests.rs`. Its ScrollArea carries `id_salt("places_favorites")` (sibling-column uis resolve to the same stable `ui.id`, so a plain `ScrollArea::vertical()` would clash with the file list's and trigger egui's ID-collision overlay) and `auto_shrink([true, false])` so it follows its content's width *within* the fixed column instead of claiming the whole row and squeezing the file list to nothing.
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
- Icons: `themes/<id>/icons/<name>.png` (18 fixed names). Resolution: own file → default theme's file → glyph. Preloaded synchronously into `egui::TextureHandle`s (`TPlayApp.icons`, one per `Icon::ALL` slot) at startup and in `set_theme`; `app.theme_icon(Icon)` hands them to `theme::icon_button`/`theme::icon`. The 7 newer icons (sort/star/folder/window chrome) are generated 20×20 PNGs with colors baked from each theme's `theme.json` palette tokens at generation time (accent / text_secondary / text_primary) — no runtime tinting, matching the hand-drawn transport icons' convention. `nocover.png` (the Album Cover pane's placeholder) is generated the same way but at 128×128 so it stays crisp scaled to pane size.
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

- Six panes as tabs: **Now Playing**, **Playlist**, **Equalizer**, **Library**, **Visualizer**, **Album Cover**
- **Pane sizing**: all panes are `Fill` — they take their dock share and resize via separator drag; `apply_min_pane_sizes` only *floors* every pane at its measured content size (Playlist/Equalizer/Library measure their bodies; Now Playing records its content height under `tplay.pane_content_h` and draws its controls **vertically centered** — the top pad is half of `available_height() - last frame's content height`, so it converges one frame after a resize). No pane is pinned; the old Now Playing "Fixed" behavior was removed. Minimum floors are enforced twice per frame: `apply_min_pane_sizes` runs before `DockArea::show`, and again **after** it (the splitter drag and floating-window resize happen inside `show()` and ignore the floors), then `ctx.request_repaint()` lands the corrected fractions next frame - so dragging below the EQ sliders' minimum height snaps back instead of sticking.
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

- **Seek fast/slow path** (`seek` in app.rs): try `sink.try_seek()`; if it fails, reopen the file and play through with `source.skip_duration(target)`. `try_seek` reaches the decoder because `EqSource` and `TapSource` forward it; `TrackPosition` then reports the new position in-place, so the whole file-based slow path is a fallback. FLAC needs rodio ≥ 0.21: `Decoder::try_from(File)` supplies the stream `byte_len` that symphonia's FLAC reader requires for its binary-search range (rodio 0.19 hardcoded it `None`, so every FLAC seek failed with `Unseekable` and fell back to decode-from-zero — no seektable helps, the range check runs first). Symphonia's FLAC binary search is ~40µs with or without a seektable, so no seektable is built anymore.
- **EQ Source wrapper**: Custom `rodio::Source` that chains 10 biquad filters in series; applies per-sample in `next()`. Uses RBJ peaking EQ coefficients. Sample rate from inner source. Decoder outputs f32 (`Sample` = f32 in rodio ≥ 0.21, no `convert_samples` needed). **EQ settings are live**: gains live in `Arc<RwLock<EqShared>>` shared with the GUI; `EqSource::refresh()` uses read locks (concurrent), UI uses write locks — no sink rebuild, no audio restart. 0 dB is an exact identity filter (RBJ peaking at A=1), so enabled/disabled needs no bypass path.
- **Visualization tap source** (`src/audio/viz.rs`): `TapSource` wraps `EqSource` (post-EQ) and mono-downmixes into a capped ring buffer (`VizBuf`, 4096 samples ≈ 93 ms). The GUI snapshots the tail each frame, runs a 1024-point radix-2 FFT with Hann window, maps to 32 log-spaced bands, dB-normalizes, and applies per-bin attack/release smoothing. Both `load_file_as` and `seek` append the tap; `load_file_as`/`stop()` clear the buffer so stopped playback shows flat idle. Pure logic — no audio device needed for tests.
- **Slider pinning + position offset**: the slow-path seek rebuilds the sink with a `skip_duration(target)` source; rodio's `get_pos()` counts only post-skip samples, so the fresh sink under-reports by exactly the skip amount. `position_offset: Duration` compensates in `playback_position()`/`playback_position_secs()`: set to the seek target on slow seek, zeroed after a fast `try_seek` (which already reports the new position), cleared on load/stop. The time label shows the new position immediately and the slider pin (`seek_target`, held until `get_pos` catches up) releases within a frame. The bar's resting state follows `playback_position()` every frame — the `seek_target` branch only holds during catch-up, and the plain-else branch tracks playback when no seek is in flight, so a track change (`load_file_as` clears `seek_target`) resets the bar to 0:00.
- **Sink replacement on load/seek**: replacing the `Sink` (via `Sink::connect_new(self.output.mixer())` — one shared mixer, old sink's Drop stops its queue) drops the old decoder and releases the file handle.
- **Playlist auto-advance** (`advance` in app.rs): runs every frame; fires only when a track drained naturally — sink empty, unpaused, and `current_path` set. The `current_path.is_none()` guard is what stops a failed load from cascading through the whole list one entry per frame.
- **System font fallback** (`theme::install_fallback_fonts` + `SYSTEM_FONT_CANDIDATES`): egui's bundled fonts (Ubuntu-Light subset etc.) lack some Unicode — e.g. U+2010 HYPHEN in "Ne‐Yo" tofu'd because the subset's punctuation coverage starts at U+2013. At startup the first existing system font (DejaVu/Liberation/FreeSans/Arial/Segoe paths; `.ttc` collections skipped via a magic-byte check) is appended last to both font families, filling only missing glyphs. App-wide, not per-theme.
- **Per-frame UI state in egui Memory**: `TPlayApp` has zero UI state. The seek slider position (`tplay.seek` in now_playing.rs), the drag-and-drop state (`tplay.drag_from` / `tplay.drag_hover` in playlist.rs), the library init flag and search query (`tplay.library.init` / `tplay.library.query` in library.rs) are stored via `ui.ctx().memory_mut` so they survive across frames and between panes.
- **Drag-and-drop reorder**: `drag_from` / `drag_hover` live in egui Memory and persist across frames. On drop, item is removed from `from` and inserted at `to` (no `-1` adjustment), so dragging item 1 over item 3 puts it at position 3. `current_index` updated to follow the moved track.
- **Shuffle order**: `played: Vec<usize>` is the history of indices played in this shuffle cycle. `next_track_index` picks random from the unplayed pool; `prev_track_index` pops from `played`. `repeat=true` restarts the cycle when the pool is exhausted. Clicking a track directly resets shuffle (`played.clear()`). Reordering/deleting calls `reset_shuffle()`.
- **Dock state in egui Memory**: `DockState<Pane>` lives in `ctx.data()` under `tplay.dock_state`. Pure UI state — not in `TPlayApp`. Layout persists to disk each frame.
- **Shared tag scan**: one cache (`tag_cache`) serves every pane, fed by two transports, and **`tracks::TagReader` owns the split** so the app holds no mpsc plumbing and no `tag_scan_rx` field. `TPlayApp::ensure_tags(paths)` forwards to `TagReader::request(cache, network, paths)`, which skips cached tracks and hands the local half to a background `scan_files` thread (replacing any in-flight receiver — per-track cache means a dropped scan just restarts next request) and the remote half to `Network::fetch_tags` (see **Remote tags**). Results drain per frame via `TagReader::drain_into` (local, including the mpsc `Empty`/`Disconnected` branches that clear "Scanning…") and `TagReader::absorb` from the `Event::Tagged` arm of `update()` (remote, including `MAX_TAG_FAILURES_LOGGED` which moved with it). Triggers: library `navigate_to` and the share browser, playlist `add_files`, `load_playlist_from`. `load_file_as` reads the playing track's tags inline (one file) so Now Playing shows title · artist · album immediately. `library_scanning()` recomputes as "any current-folder file missing from the cache" — a **local** question by construction, since `library_entries` only ever comes from `navigate_to` over a `dir.is_dir()`-checked `library_dir` and an `smb://` URI never passes that; the share browser computes the same thing over its own entries.
- **SMB worker** (`network::spawn_worker`, spawned by `Network::new`): a single `std::thread` owns a tokio current-thread runtime and processes `SmbCmd`s sequentially (`ListShares`/`ListDir`/`Spool`/`Save`/`Tags` — paced by user clicks, spool is the one long op). Replies come back over a std mpsc drained every frame by `Network::drain` (same pattern as `drain_tag_scan`; `update()` repaints while `busy()` or playing). Each command carries its own `SmbCreds` snapshot (saved username + session password) — passwords exist only inside the command and are dropped after use. Stale replies are dropped by the `host`/`uri` match against the current browse/pending state. Sequential processing means a `Save` queues behind an in-flight track spool — fine for a few KB of JSON, but a large download makes Save feel slow. A `Tags` batch is likewise a long pole, and now a much heavier one: browsing into a 300-file directory queues one command that **downloads 300 whole files**, and browse clicks wait behind all of it. That is the price of remote rows matching local ones (see **Remote tags**); the share browser shows a live count so the wait is legible, and a revisit is instant because the files are already cached.
- **Four in-flight slots, one per concern** (`network.rs`): `pending` (playback spool, read by Now Playing for "Loading from server…" and by `advance()`'s cascade guard), `fetch_req` (a remote `.tplay` download), `saving` (a share write), `tagging` (the set of URIs with a tag request in flight — see **Remote tags** for why it is not the same map as the failure counts). `drain` returns the **first actionable** event and leaves the rest queued, so `update()` drains in a `while let` loop — a save reply must not be stranded behind an unrelated one, and the loop can't spin (each `Some` consumed a reply). All four feed `busy()`, which gates `request_repaint`: anything in flight that `busy()` doesn't know about sits undrained until some unrelated repaint happens.
- **The tag cache holds `TrackInfo`, not `Option<TrackInfo>`.** "Checked, no tags" is therefore recorded as an **empty** `TrackInfo`, which is what `scan_files` already stores for an untagged local file. Every reader treats that identically to no entry — `title_or_stem` falls back to the stem on an empty title, and the search haystack and Now Playing labels see empty strings — so nothing needs to distinguish the two. The one thing that *does* need the empty entry is the "don't re-request" rule: skipping unparseable files entirely (as `scan_files` does) means `library_scanning()` never clears for a file that can't be parsed, leaving a permanent "Scanning…". That remains a latent bug on the **local** side. The share browser avoids it by caching an empty `TrackInfo` for a *successfully read* untagged file, and by showing "Reading tags… (N unavailable)" once in-flight work hits zero — so a track that gave up after its retries cannot pin a count on screen forever.
- **`Path::is_relative` is true for `smb://` URIs** (anything without a leading `/` counts as relative on Unix). Remote paths are stored as `PathBuf`s holding URIs, so any code keying off `is_relative`/`is_absolute` must check `network::is_remote` **first** — otherwise an `smb://nas/m/x.mp3` gets joined onto whatever base and becomes `smb://nas/m/smb://nas/m/x.mp3`. `read_playlist` is the place this bites; `tests/smb_helpers.rs` and `playlist_tests.rs` guard it. `is_remote` is now called from only two places: `tracks.rs` (all track operations) and `library.rs`'s `read_playlist` mapper — **never** from `app.rs` or `src/gui/`.
- **Appwide settings**: one `~/.config/tplay/config.json` holds everything — theme, EQ (enabled + gains), shuffle/repeat, visualizer view (`viz_view`), volume, last loaded playlist, saved SMB servers (`servers`), and the library state (favorites, last dir, show hidden). Saved via `TPlayApp::save_config()` on any change, loaded once in `new()`. Shuffle/repeat are global playback modes — loading a playlist never touches them. Volume is applied to the sink at startup. The last loaded `.tplay` (`last_playlist`) is re-loaded on startup (Library + tag scan + tracked file all restored), so Save Playlist keeps overwriting the same file across sessions.
  - **Config JSON structure**:
    ```json
    {
      "theme": "dark",
      "eq": { "enabled": false, "gains": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
      "shuffle": false,
      "repeat": false,
      "viz_view": "Bars",
      "volume": 1.0,
      "buffer_size": 8192,
      "last_playlist": "/path/to/playlist.tplay",
      "library": { "favorites": ["/path/to/fav"], "last_dir": "/path/to/music", "show_hidden": false },
      "servers": [{ "host": "192.168.1.50", "username": "guest" }]
    }
    ```
- **Confirm dialogs**: `TPlayApp::confirm(title, desc, at_risk)` wraps `rfd::MessageDialog` (Yes/No). Used for New Playlist, loading a playlist over the current one, and per-track ✕ removal. `at_risk = false` (e.g. an empty playlist) skips the dialog. Save-overwrite is already confirmed by the native save dialog.
- **XorShift64 RNG** (`app.rs`): inline 64-bit XorShift (`state ^= state << 13; state ^= state >> 7; state ^= state << 17`) replaces `fastrand`. Used for shuffle track selection. Fixed seed `0xC0FFEE`.

---

## Config & data files

| file | purpose |
|---|---|
| `~/.config/tplay/config.json` | unified app settings (theme, EQ, shuffle, repeat, volume, **buffer_size**, **spool_cache_mb**, last playlist, library state, balance, remaining, gapless, crossfade, crossfade_secs) |
| `~/.config/tplay/dock_layout.json` | egui_dock layout (tabs, splits, floating windows) |
| `~/.config/tplay/layouts/<name>.json` | named dock layout (tabs, splits, floating windows) — created via ☰ menu |
| `~/.config/tplay/themes/<id>/theme.json` | user theme override (wins on id clash) |
| `<exe_dir>/themes/<id>/theme.json` | shipped themes (dark, retro, neon) |
| `./themes/<id>/theme.json` | dev themes (cargo run from repo root) |
| `<playlist>.tplay` | playlist file: `{"paths": ["/abs/path", "relative/path"]}` — relative paths resolve against the playlist's own directory, or against the share directory URI for one loaded off an SMB share. A `smb://` path is stored in full and never resolved against that base |

**Config JSON structure** (added fields in **bold**):
```json
{
  "theme": "dark",
  "eq": { "enabled": false, "gains": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
  "shuffle": false,
  "repeat": false,
  "viz_view": "Bars",
  "volume": 1.0,
  "buffer_size": 8192,
  "spool_cache_mb": 2048,
  "last_playlist": "/path/to/playlist.tplay",
  "library": { "favorites": ["/path/to/fav"], "last_dir": "/path/to/music", "show_hidden": false },
  "servers": [{ "host": "192.168.1.50", "username": "guest" }],
  "**balance**": 0.0,
  "**remaining**": false,
  "**gapless**": false,
  "**crossfade**": false,
  "**crossfade_secs**": 3.0
}
```

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
| `COVER_ID` (fn `cover_id`) | gui/panes/album_cover.rs | `"tplay.cover"` (egui memory key — decoded cover cache) |
| `SEEK_ID` | gui/panes/now_playing.rs | `"tplay.seek"` (egui memory key) |
| `LIB_INIT` | gui/panes/library.rs | `"tplay.library.init"` (egui memory key) |
| `LIB_QUERY` | gui/panes/library.rs | `"tplay.library.query"` (egui memory key) |
| `NET_FORM` | gui/panes/library.rs | `"tplay.network.form"` (egui memory key — sidebar add-server host form) |
| `NET_LOGIN` | gui/panes/library.rs | `"tplay.network.login"` (egui memory key — main-pane credential prompt, keyed `(NET_LOGIN, host)`) |
| `SAVE_SHARE_ID` | gui/panes/playlist.rs | `"tplay.playlist.save_share"` (egui memory key — the share-save name modal, `Option<(dir_uri, filename)>`) |
| `DEFAULT_PLAYLIST_NAME` | library.rs | `"playlist.tplay"` (fallback save name when nothing has a usable stem) |
| `uri_parent` | network.rs | inverse of `child_uri` — the share dir a remote playlist resolves against |
| `local_file` | tracks.rs | a track → the local file to read (remote → spool-cache copy, `None` if not spooled; local → unchanged, existence not required) |
| `local_file_now` | tracks.rs | the same, but only if readable **right now** — the pre-buffer question |
| `normalize` | tracks.rs | playlist entry → stored id (local canonicalized & dropped if gone, remote verbatim) |
| `MAX_TAG_FAILURES_LOGGED` | tracks.rs | `3` — per-track tag failures printed before switching to a count |
| `AUDIO_EXTENSIONS` | library.rs | `["mp3","wav","ogg","flac","m4a"]` |
| `PREROLL_SECS` | app.rs | `2.0` (gapless/crossfade arm trigger, seconds before track end) |
| `SMB_PORT` | network.rs | `445` — SMB protocol default port |
| `TAG_ATTEMPTS_MAX` | network.rs | `3` — tag-fetch attempts before a remote track is given up on for the session |
| `spool_cache_mb` | app.rs (`Config`) | `2048` — MiB ceiling for never-played spool; eviction never touches a played file |
| `spool_key` / `cache_path` | network.rs | FNV-1a 64 hex key + cache path for a `smb://` URI (`<cache>/tplay/smb/<key>.<ext>`) |

---

## Test conventions

- **All tests live in `tests/`** — no `#[cfg(test)]` modules in src. Cargo auto-discovers each `tests/<suite>.rs` as its own binary, so every suite runs exactly once (`tests/main.rs` was removed — it re-declared the suites and doubled every run).
- `tests/common.rs` helpers: `temp_dir()`, `test_dir(name)`, `write_wav(path)` (1s 8kHz mono PCM), `write_minimal_flac(path)`, `assert_duration_approx`.
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
| Change the file list (rows, columns, sorting, search) | `file_list_ui` + `list_header_right` in `gui/panes/library.rs` — shared by the local folder browser AND the SMB share browser, so a change lands in both |
| Change how remote tracks get tags | `Network::fetch_tags` + `run_tags` (network.rs), the `Event::Tagged` arm in `update()` |
| Change spool cache retention | `Config::spool_cache_mb` + `Network::evict_unplayed`/`mark_played`, policy in the pure `select_evictions` |
| Change visualizer rendering | `gui/panes/visualizer.rs` (bars/wave draw), `audio/viz.rs` (FFT/smoothing) |
| Change docking behavior | `gui/coordinator.rs` (`apply_min_pane_sizes`, `default_tree`, `save_layout`/`load_layout`/`list_layouts`, Layouts menu section) |
| Add a config field | `app.rs` — the `Config` struct, a `TPlayApp` field, and both `new()` and `save_config`; a value hand-edited into `config.json` is only preserved if `save_config` writes it back |
| Change tag fields | `library.rs` (`TrackInfo`, `read_info`, `SORT_OPTIONS`, `sort_key`) |
| Adjust seek behavior | `app.rs` (`seek`, `advance`, `load_file_as`) |

---

## Future milestones

Not yet built — roadmap only. Each milestone is scoped to the existing
architecture (the "File to touch" pointers below); nothing here is live until
implemented and this section rewritten.

### 1. Media Library database

Turn the folder browser into a persistent library: the `tag_cache`
(`HashMap<PathBuf, TrackInfo>` in `app.rs`, currently session-scoped) is the
seed — persist it to `~/.config/tplay/` on scan completion (`drain_tag_scan`
already has the hook; same pattern as `save_config`/`load_config`) so
re-launch needs no re-scan. Add per-track metadata Winamp had that `TrackInfo`
doesn't: **rating (1–5 ★)**, **play count**, **last-played date** (increment in
`load_file_as`). Library-data fields go into `library.rs` (`TrackInfo` +
`read_info` + `SORT_OPTIONS` + `sort_key`) and/or the `Config`/`LibraryData`
structs — the same edit pattern as adding a config field. The headline:
**Smart Views / auto playlists** — saved rule queries ("rating ≥ 4", "top 25
played", "added last 90 days") evaluated against the persisted cache, rendered
as a playlist (reuse the Playlist pane's load machinery; the result is just a
`Vec<PathBuf>`). No new dependencies; lofty/scan_files already read everything.
Out of scope: matching Winamp's network features (CDDB, online stores) — this
app is no-network.

### 2. Playlist editor depth

Add Winamp Playlist Editor operations to `gui/panes/playlist.rs` + `app.rs`:

- **Sort playlist** (by title/path/track #/album), **reverse**, **randomize**
  — reuse `library::sort_key` (needs a `sort_key`-style call for the playlist's
  untagged pre-scan rows too: `app.rs` already owns `tag_cache`); randomize
  reuses the XorShift64 RNG (`rand_usize`).
- **Remove duplicates** — a one-pass cleanup over `playlist: Vec<PathBuf>`
  (dedup happens on add today, but not retroactively).
- **Jump-to-file** — Winamp's `J` quick search over the playlist; same
  egui-memory pattern as the Library's search (`LIB_QUERY` in
  `gui/panes/library.rs`).
- **Multiple concurrent playlists** — a list of named playlists instead of one
  `playlist: Vec<PathBuf>`. Bigger change: touches `playlist_file`/
  `playlist_dirty` semantics and the Playlist pane header. Defer until the
  single-playlist ops above land.

Sort/randomize/dups are pure-logic in scope for the `tests/` mirror-branch
convention (`playlist_tests.rs` pattern).

### 3. Interop

- **`.m3u` / `.pls` import & export** — touch `library.rs` only:
  `read_playlist`/`write_playlist` branch on extension (`.tplay` JSON today;
  `.m3u` is plain text, one path per line), and `is_playlist` widens to the new
  extensions so they appear in the Library + are picked up by the `.tplay`
  row handling. Relative `.m3u` paths resolve against the same `base` argument
  `read_playlist` already takes, so share-hosted `.m3u` files keep working for
  free — and the `is_remote`-before-`is_relative` order in that mapper becomes
  load-bearing here, since `.m3u` files are relative-path-heavy by nature.
- **More audio formats** — Winamp played AIFF, WMA, APE, MOD/S3M/XM/IT
  (tracked music is a Nullsoft hallmark), MIDI. Two paths: formats rodio can
  already decode → extend `AUDIO_EXTENSIONS` (`library.rs`) + `audio_dialog`
  filter (`app.rs`) + rodio features (`Cargo.toml`); MOD/tracked + MIDI need a
  new decoder dependency — each new dep must justify itself per the deps
  table. Several rodio-backed decoders are still considered future work, not a
  milestone of their own.

### 4. Playback-control niceties — IMPLEMENTED

- **Balance (L/R)** — `balance: f32` field in `Config` (default 0.0, range -1..1).
  Live, no sink rebuild: `src/audio/balance.rs` `BalanceSource` reads
  `Arc<RwLock<f32>>` per audio frame (same pattern as `EqSource`/`EqShared`).
  Slider in Now Playing pane next to volume.
  **Note**: rodio 0.21 has no `Sink::set_pan`; implemented as a source wrapper
  (`BalanceSource`) instead of sink-level panning.
- **Elapsed ↔ remaining toggle** — persisted `Config.remaining: bool` + 
  `TPlayApp::set_remaining()` (the `viz_view` pattern). Clickable elapsed/remaining
  label in Now Playing pane toggles the mode.
- **Gapless playback** — `Config.gapless: bool`. When enabled and within
  `PREROLL_SECS` (2s) of track end, the **xf sink** pre-buffers the next track
  at volume 0, holds it silent, and swaps on the current track's drain — zero
  gap, zero overlap. No queue fusion.
- **Crossfade** — `Config.crossfade: bool` + `Config.crossfade_secs: f32` (default 3s).
  **Real two-sink crossfader** (see implementation notes): the outgoing track plays
  full-length in the main sink while the incoming track plays simultaneously in a
  second sink on the same mixer, faded in with equal-power gains per frame. Each
  track is heard exactly once — no repeated tail. Crossfade supersedes gapless when
  both on. ☰ menu: Crossfade checkbox + duration slider (0–10s).
- **ReplayGain / loudness normalization** — read the `REPLAYGAIN_TRACK_GAIN`
  tag (lofty exposes it) and apply as a per-track volume offset at `load_file_as`
  time; or a pre-scan loudness pass. Speculative until users ask.
- **Per-track EQ** — conflicts with the current single `EqShared` source of
  truth (`audio/eq.rs`) and "EQ settings are live" design; would need a
  per-track settings map consulted on `load_file_as`. Not recommended without a
  product reason.

Milestones 1–2 ship user-visible Winamp parity on the existing architecture;
3–4 are smaller, mostly self-contained (3 = one file + Cargo.toml, 4 = config
field + pane tweak). No new milestone may hardcode constants — theme tokens /
shared-constant rules apply to milestones as much as to shipped code.

---

### 4. Playback-control niceties — implementation notes

**New source modules** (pure, headless-testable, mirror `audio/eq.rs` pattern):
- `src/audio/balance.rs` — `BalanceSource<S>` wraps any `Source<Item=f32>`,
  applies per-channel gains from `Arc<RwLock<f32>>`, outputs stereo.
  `balance_gains(f32) -> (f32,f32)` linear curve (center = 1.0/1.0 exact passthrough).
  Tests: `tests/balance_tests.rs` (8 tests, identical harness to `eq_live.rs`).

- `src/audio/transition.rs` — free functions `build_gapless_next()` (a full,
  buffered EQ → Tap → Balance source — the unit both sinks play) and
  `fade_gains(p) -> (f32, f32)` (equal-power cos/sin crossfade curve).
  **Seek-first helper** `seek_or_skip()`: attempts `try_seek()` (fast path for
  FLAC/WAV/OGG/M4A) before falling back to `skip_duration()` (eager decode for
  unseekable streams). No UI code, pure logic. Tests:
  `tests/gapless_crossfade.rs`.

**App state additions** (`src/app.rs`):
- `Config`: `balance`, `remaining`, `gapless`, `crossfade`, `crossfade_secs`.
- `TPlayApp` fields: `balance: Arc<RwLock<f32>>`, `remaining: bool`,
  `gapless: bool`, `crossfade: bool`, `crossfade_secs: f32`,
  `xf_sink: Option<Sink>` (incoming track, second sink on the same mixer) and
  `xf_out_total: Option<Duration>` (the outgoing track's duration, captured at
  arm time — the arm flips `total_duration` to the incoming track for the seek
  bar, so the fade math needs the outgoing total saved separately).
- Getters/setters: `balance()/set_balance()`, `remaining()/set_remaining()`,
  `gapless()/toggle_gapless()`, `crossfade()/toggle_crossfade()`,
  `crossfade_secs()/set_crossfade_secs()`.

**UI additions** (`src/gui/panes/now_playing.rs`, `src/gui/coordinator.rs`):
- Now Playing: new second row under transport controls — Balance slider with "Balance" meta label (left-aligned, double-click resets to center 0.0) + Gapless/Crossfade **icon buttons** (18px, lit via `selected()` visual, same convention as lit shuffle/repeat icons).
- Volume slider now stands alone in the transport row (no balance crowding).
- ☰ menu: "Crossfade" section (constant height) — crossfade duration slider **always visible and editable** (no `ui.add_enabled` guard), Gapless/Crossfade checkboxes removed (now icon buttons in Now Playing row).
- Icon set expanded to 20: `gapless.png` + `crossfade.png` (20×20, text_primary baked per theme, glyph fallbacks ⏩/🔗).

**Real two-sink crossfader** (`src/app.rs`, `src/audio/transition.rs`):
- **Two sinks, one mixer**: `self.sink` plays the current track full-length
  (never truncated); the arm creates a second `Sink::connect_new(self.output.mixer())`
  (`xf_sink`) holding the buffered incoming track. Both play simultaneously;
  rodio mixes them. Each track is heard exactly once — the old three-piece
  queue (`A_body` + `mix(A_tail,B_head)` + `B_body`) is deleted, which also
  killed B's repeated tail (B_tail re-read at the end of B_body) and the
  multi-second UI freeze from eager `skip_duration()`.
- **Arm** (`advance()` step 2): within `crossfade_secs` of the outgoing track's
  end (or `PREROLL_SECS` with gapless), `next_track_index()` is called ONCE and
  the incoming source is built synchronously (buffered decode — header-only,
  ms). `xf_out_total` = outgoing duration is captured, then
  `current_index`/`current_path`/`total_duration` flip to the incoming track so
  Now Playing and the seek bar already show it. `playback_position()`/
  `playback_position_secs()` prefer `xf_sink` while present. Missing next file →
  skip the arm; natural advance fails it gracefully via `play_now`.
  **Short-incoming guard**: a track whose tagged duration is shorter than the
  hold/fade window would drain muted while "playing" then be promoted empty
  (silent skip). The arm consults `tag_cache` and bails when `duration < hold`
  (gapless hold / crossfade window) — such tracks play via natural advance
  with a gap instead. Untagged tracks are allowed through.
- **Settle** (`advance()` step 1): fade progress `p = 1 − remaining/cf` from
  `sink.get_pos()` vs `xf_out_total` (pause/seek-safe, no wall clock);
  `fade_gains(p)` sets `sink`/`xf_sink` volumes each frame (equal-power, no
  midpoint dip). Gapless: xf held at volume 0 until the swap — zero gap. When
  the outgoing sink empties → `finish_xf()`: stop the outgoing sink, promote
  `xf_sink` to `self.sink`, restore full volume, zero `position_offset`.
- **Seek-first builders**: `transition::seek_or_skip()` tries `try_seek()`
  (FLAC binary search ~µs, WAV/OGG/M4A fast) before falling back to
  `skip_duration()` — turns the freeze into an instant seek for seekable formats.
- **Cancellation** (`cancel_xf()`): stops + drops `xf_sink`, restores main volume. Called from `load_file_as`, `seek`,
  `stop`, `remove_track`, `move_track` (seek/stop/remove/move/start invalidation
  sites). `pause`/`play` drive both sinks.

**Tests added**:
- `tests/balance_tests.rs` — 8 tests: curve, mono/stereo routing, bit-transparent passthrough, live change mid-stream, seek clears half-frame.
- `tests/gapless_crossfade.rs` — 10 tests: `fade_gains` (endpoints, midpoint ≈ 0.707, constant-power a²+b²=1, monotonicity, clamping) + `build_gapless_next` on a real WAV (full-length drain, 0 dB/center passthrough of a mid-file sample).
- `tests/config_persistence.rs` — round-trip, defaults, JSON keys for all 5 new fields.

**Deferred** (unchanged from roadmap): ReplayGain, per-track EQ. Accepted edge:
duration-probe inaccuracy can make the swap (or fade completion) land a few
samples early/late → a tiny volume pop on the incoming track; correction is a
more accurate MP3 probe, not a design change.

---

## Remember

- Update this file after every significant change.
- The codebase is intentionally small — prefer deletion over addition.
- All constants come from theme JSON or shared constants; no magic numbers in pane code.