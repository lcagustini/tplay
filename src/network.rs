//! Built-in SMB browsing client — pure Rust (smb2 crate), no system mount.
//! Remote tracks are modelled as `smb://host/share/rel/path` URIs carried in
//! the playlist as PathBufs. Playback is **spool-then-play** ("materialize"):
//! the file is downloaded to the spool cache dir before playback, so the
//! audio pipeline (rodio) never touches the network.
//!
//! A single background worker thread owns a tokio current-thread runtime and
//! processes `SmbCmd`s sequentially (shares list / dir list / spool). Replies
//! come back over a std mpsc channel the app drains every frame — the same
//! pattern as the library tag scan. Passwords live only inside a command and
//! are dropped after use; nothing is persisted.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

/// SMB default port (protocol constant, like HTTP's 80).
pub const SMB_PORT: u16 = 445;

/// One browsable SMB entry — mirrors `smb2::client::tree::DirectoryEntry`.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Credentials for one SMB connection. Empty username/password = guest.
/// There is deliberately no `Default`: passwords must be explicit.
#[derive(Clone, Debug)]
pub struct SmbCreds {
    pub username: String,
    pub password: String,
}

/// A saved server in `Config.servers` — host + username only. Passwords are
/// never persisted; they live in per-session memory keyed by host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCfg {
    pub host: String,
    pub username: String,
}

/// Commands sent to the worker thread.
pub enum SmbCmd {
    /// Enumerate the server's disk shares. `host` is the bare hostname/IP.
    ListShares { host: String, creds: SmbCreds, reply: Sender<SmbReply> },
    /// List a share or directory. `uri` = `smb://host/share[/rel/dir]`.
    ListDir { uri: String, creds: SmbCreds, reply: Sender<SmbReply> },
    /// Download `uri` into the spool cache, replying the local path. Used for
    /// playback AND for reading a remote `.tplay` off the server.
    Spool { uri: String, creds: SmbCreds, reply: Sender<SmbReply> },
    /// Upload `data` to `uri`, overwriting it if it exists.
    Save { uri: String, data: Vec<u8>, creds: SmbCreds, reply: Sender<SmbReply> },
}

/// Replies routed back to the app (drained like the tag scan). The `uri`/
/// `host` fields let the app drop stale replies (the user navigated away).
pub enum SmbReply {
    Shares { host: String, result: Result<Vec<RemoteEntry>, String> },
    Dir { uri: String, result: Result<Vec<RemoteEntry>, String> },
    Spooled { uri: String, result: Result<PathBuf, String> },
    Saved { uri: String, result: Result<(), String> },
}

/// An event the app must act on, yielded by `Network::drain`. Browse replies
/// (ListShares/ListDir) are applied to the browse state internally; the rest
/// land here because the app must act on them.
pub enum Event {
    /// A requested spool finished for the still-pending track. The app plays
    /// the local spooled copy in place of the `smb://` URI it was told to play.
    Spooled { uri: String, result: Result<PathBuf, String> },
    /// A remote `.tplay` finished downloading; `local` is its spooled copy.
    Fetched { uri: String, result: Result<PathBuf, String> },
    /// A playlist write to a share completed.
    Saved { uri: String, result: Result<(), String> },
}

/// Remote-browse position inside the Library pane — which server/share/dir is
/// open, the last listing, and its in-flight/error status.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkBrowse {
    /// Bare hostname/IP of the selected server.
    pub host: String,
    /// Current share (None = browsing the server's shares).
    pub share: Option<String>,
    /// Share-relative directory path ("" = share root).
    pub rel: String,
    /// Last listing for the current share/dir.
    pub entries: Vec<RemoteEntry>,
    /// True while a listing request is in flight.
    pub busy: bool,
    /// Last listing error, if any.
    pub error: Option<String>,
}

/// All SMB network state + worker channels, owned by the app as one object.
/// Pure state — persistence (`config.json` servers) and playback are the app's
/// job; the GUI reaches in through `TPlayApp::network()`/`network_mut()`.
pub struct Network {
    servers: Vec<ServerCfg>,
    /// Session-memory passwords keyed by host — never persisted.
    passwords: HashMap<String, String>,
    browse: Option<NetworkBrowse>,
    /// A remote track waiting on its spool (its `smb://` URI). The app set
    /// `current_index` before requesting, so the spooled file plays into it.
    pending: Option<PathBuf>,
    /// A remote `.tplay` waiting on its download. Separate from `pending` so
    /// reading a playlist can never displace a track that is still spooling
    /// (and vice versa) — one slot each, one request each.
    fetch_req: Option<String>,
    /// A playlist write to a share is in flight. No stale-check needed (the
    /// app acts on every reply), but it must keep frames coming so the reply
    /// is drained and the share listing refreshes.
    saving: bool,
    cmd_tx: Sender<SmbCmd>,
    /// Cloned into every command so replies flow back to `reply_rx`.
    reply_tx: Sender<SmbReply>,
    reply_rx: Receiver<SmbReply>,
}

impl Network {
    /// Create the channels, spawn the worker thread, and take the saved
    /// servers. The worker exits when the app drops this (channel disconnect).
    pub fn new(servers: Vec<ServerCfg>) -> Self {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        spawn_worker(cmd_rx);
        Self {
            servers,
            passwords: HashMap::new(),
            browse: None,
            pending: None,
            fetch_req: None,
            saving: false,
            cmd_tx,
            reply_tx,
            reply_rx,
        }
    }

    /// Saved servers (host + username) from config; passwords are separate.
    pub fn servers(&self) -> &[ServerCfg] {
        &self.servers
    }

    /// Add a server (dedup by host; re-adding updates the username).
    pub fn add_server(&mut self, host: String, username: String) {
        let host = host.trim().to_string();
        if host.is_empty() {
            return;
        }
        match self.servers.iter_mut().find(|s| s.host == host) {
            Some(s) => s.username = username,
            None => self.servers.push(ServerCfg { host, username }),
        }
    }

    /// Remove a saved server. Passwords stay in the session map — short-lived,
    /// cleared on exit.
    pub fn remove_server(&mut self, host: &str) {
        self.servers.retain(|s| s.host != host);
    }

    /// Session-memory password for a host (never persisted) — set at add-time
    /// via the Library's add-server form; consumed by `creds_for` on connect.
    pub fn set_password(&mut self, host: String, password: String) {
        self.passwords.insert(host, password);
    }

    /// Current remote-browse position (Some = Library shows network mode).
    pub fn browse(&self) -> Option<&NetworkBrowse> {
        self.browse.as_ref()
    }

    /// A remote track waiting on its spool, if any — Now Playing shows
    /// "Loading from server…" while set.
    pub fn pending(&self) -> Option<&Path> {
        self.pending.as_ref().map(|p| p.as_path())
    }

    /// True while the worker owes us something (a spool, a playlist fetch or
    /// save, or a listing). Drives `request_repaint`, so anything in flight
    /// must show up here or its reply sits undrained.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
            || self.fetch_req.is_some()
            || self.saving
            || self.browse.as_ref().is_some_and(|b| b.busy)
    }

    /// Saved username for a host (or blank → guest) + session password.
    fn creds_for(&self, host: &str) -> SmbCreds {
        SmbCreds {
            username: self
                .servers
                .iter()
                .find(|s| s.host == host)
                .map(|s| s.username.clone())
                .unwrap_or_default(),
            password: self.passwords.get(host).cloned().unwrap_or_default(),
        }
    }

    fn send(&self, cmd: SmbCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Select a server: enter network mode at the shares stage.
    pub fn browse_server(&mut self, host: String) {
        self.browse = Some(NetworkBrowse {
            host: host.clone(),
            share: None,
            rel: String::new(),
            entries: Vec::new(),
            busy: true,
            error: None,
        });
        let creds = self.creds_for(&host);
        self.send(SmbCmd::ListShares {
            host,
            creds,
            reply: self.reply_tx.clone(),
        });
    }

    /// Navigate to a share or directory within the browsed server.
    /// `share` None goes back to the shares stage; `rel` is share-relative.
    pub fn browse_open(&mut self, uri: String, share: Option<String>, rel: String) {
        if let Some(b) = self.browse.as_mut() {
            b.share = share;
            b.rel = rel;
            b.entries = Vec::new();
            b.busy = true;
            b.error = None;
        }
        let host = split_uri(&uri).map(|(h, _, _)| h).unwrap_or_default();
        self.send(SmbCmd::ListDir {
            uri,
            creds: self.creds_for(&host),
            reply: self.reply_tx.clone(),
        });
    }

    /// Exit network mode — the Library back to its local folder browser.
    pub fn leave_network(&mut self) {
        self.browse = None;
    }

    /// Request playback of `uri`: spool it now and mark it pending (the app
    /// already tore down the current track). The spooled file is delivered via
    /// a `Spooled` event from `drain`.
    pub fn spool(&mut self, uri: PathBuf) {
        self.pending = Some(uri.clone());
        let uri_str = uri.to_string_lossy().into_owned();
        let host = split_uri(&uri_str).map(|(h, _, _)| h).unwrap_or_default();
        self.send(SmbCmd::Spool {
            uri: uri_str,
            creds: self.creds_for(&host),
            reply: self.reply_tx.clone(),
        });
    }

    /// Forget a pending spool — used when the user stops playback. The worker
    /// still downloads, but its reply lands as stale and `drain` drops it.
    pub fn discard_pending(&mut self) {
        self.pending = None;
    }

    /// Download a remote `.tplay` so the app can read it. Deliberately does
    /// NOT touch `pending`: that slot belongs to playback, and reading a
    /// playlist must not cancel (or be cancelled by) a track download.
    /// Re-requesting the same or another playlist supersedes the first.
    pub fn fetch(&mut self, uri: String) {
        self.fetch_req = Some(uri.clone());
        let host = split_uri(&uri).map(|(h, _, _)| h).unwrap_or_default();
        self.send(SmbCmd::Spool {
            uri,
            creds: self.creds_for(&host),
            reply: self.reply_tx.clone(),
        });
    }

    /// Write a playlist to a share. Async: the bytes go to the worker and
    /// `drain` reports the outcome, so the UI thread never blocks on the LAN.
    pub fn save(&mut self, uri: String, data: String) {
        let host = split_uri(&uri).map(|(h, _, _)| h).unwrap_or_default();
        self.saving = true;
        self.send(SmbCmd::Save {
            uri,
            data: data.into_bytes(),
            creds: self.creds_for(&host),
            reply: self.reply_tx.clone(),
        });
    }

    /// Drain worker replies into the browse state. Returns the one event the
    /// app must act on (a spool completing for the still-pending track);
    /// browse listings are applied internally and stale replies are dropped
    /// by host/uri match, so nothing else leaks out.
    pub fn drain(&mut self) -> Option<Event> {
        while let Ok(reply) = self.reply_rx.try_recv() {
            match reply {
                SmbReply::Shares { host, result } => {
                    if let Some(b) = self.browse.as_mut() {
                        if b.host == host && b.share.is_none() {
                            b.busy = false;
                            match result {
                                Ok(entries) => { b.entries = entries; b.error = None; }
                                Err(e) => { b.entries = Vec::new(); b.error = Some(e); }
                            }
                        }
                    }
                }
                SmbReply::Dir { uri, result } => {
                    if let Some(b) = self.browse.as_mut() {
                        let current = b.share.as_ref()
                            .map(|s| dir_uri(&b.host, s, &b.rel))
                            .unwrap_or_default();
                        if current == uri {
                            b.busy = false;
                            match result {
                                Ok(entries) => { b.entries = entries; b.error = None; }
                                Err(e) => { b.entries = Vec::new(); b.error = Some(e); }
                            }
                        }
                    }
                }
                SmbReply::Spooled { uri, result } => {
                    // Playback wins: a track download is promoted to the sink
                    // the moment it lands, so it must not wait behind a fetch.
                    let is_pending = self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.to_string_lossy() == uri);
                    if is_pending {
                        self.pending = None;
                        return Some(Event::Spooled { uri, result });
                    }
                    // Otherwise it may be a playlist download. A stale reply —
                    // the user clicked another `.tplay` in the meantime — is
                    // dropped against the slot.
                    if self.fetch_req.as_deref() == Some(uri.as_str()) {
                        self.fetch_req = None;
                        return Some(Event::Fetched { uri, result });
                    }
                }
                SmbReply::Saved { uri, result } => {
                    self.saving = false;
                    return Some(Event::Saved { uri, result });
                }
            }
        }
        None
    }
}

// ── URI model ────────────────────────────────────────────────────────────────

/// Is this playlist/library path a remote `smb://` track?
pub fn is_remote(path: &Path) -> bool {
    path.to_string_lossy().starts_with("smb://")
}

/// Parse `smb://host/share/rel/path` into `(host, share, rel)`.
/// `share` and `rel` are None/"" for a bare server / share-root URI.
pub fn split_uri(uri: &str) -> Option<(String, Option<String>, String)> {
    let rest = uri.strip_prefix("smb://")?;
    let (host, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        return None;
    }
    let (share, rel) = tail
        .split_once('/')
        .map(|(s, r)| (Some(s.to_string()), r.to_string()))
        .unwrap_or_else(|| {
            if tail.is_empty() { (None, String::new()) } else { (Some(tail.to_string()), String::new()) }
        });
    Some((host.to_string(), share, rel))
}

/// Parse what the add-server field accepts: a bare `host`, `host/share`, or a
/// full `smb://host/share[/dir]` URI (the scheme is optional). Returns the host
/// to save, plus the share/relative path to open directly when the input named
/// one — jumping straight into a share skips share enumeration entirely, which
/// is what GNOME Files does and is the only way in on servers that need it.
///
/// `None` for empty input.
pub fn parse_server_input(input: &str) -> Option<(String, Option<String>, String)> {
    let raw = input.trim().trim_start_matches("smb://").trim();
    if raw.is_empty() {
        return None;
    }
    split_uri(&format!("smb://{raw}"))
}

/// `smb://host`
pub fn server_uri(host: &str) -> String {
    format!("smb://{host}")
}

/// `smb://host/share` — share root.
pub fn share_uri(host: &str, share: &str) -> String {
    format!("smb://{host}/{share}")
}

/// `smb://host/share/rel` with `rel` optionally empty (share root).
pub fn dir_uri(host: &str, share: &str, rel: &str) -> String {
    let base = share_uri(host, share);
    if rel.is_empty() { base } else { format!("{base}/{rel}") }
}

/// `parent_uri/name` — descend one level from a share or directory URI.
pub fn child_uri(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

/// The directory portion of a file URI: `smb://h/share/dir/f.tplay` ->
/// `smb://h/share/dir`. Inverse of `child_uri`; the base a remote playlist's
/// relative entries resolve against.
///
/// Returns the input unchanged when there is no directory part to take. The
/// guard is that the result is still a host-bearing URI, not merely non-empty:
/// a bare `smb://host` splits at its last slash into `"smb:/"`, which is
/// non-empty but would resolve relative entries onto garbage.
pub fn uri_parent(uri: &str) -> &str {
    match uri.rsplit_once('/') {
        Some((parent, _)) if parent.strip_prefix("smb://").is_some_and(|r| !r.is_empty()) => {
            parent
        }
        _ => uri,
    }
}

/// Resolve a click on a share-list entry or a directory entry.
///
/// A `None` share means the server's share list is showing, so the clicked name
/// is the share itself. Once a share is selected, the same click extends its
/// share-relative path.
pub fn child_browse(
    host: &str,
    share: Option<&str>,
    rel: &str,
    name: &str,
) -> (String, Option<String>, String) {
    match share {
        None => {
            let share = name.to_owned();
            (share_uri(host, &share), Some(share), String::new())
        }
        Some(share) => {
            let new_rel = if rel.is_empty() {
                name.to_owned()
            } else {
                format!("{rel}/{name}")
            };
            (dir_uri(host, share, &new_rel), Some(share.to_owned()), new_rel)
        }
    }
}

/// Human size for the remote file rows ("3.2 MB", "45 KB"). 1024-based, MB/GB.
pub fn fmt_size(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

// ── Spool cache ──────────────────────────────────────────────────────────────

/// FNV-1a 64-bit — hand-rolled because `DefaultHasher`'s algorithm is
/// unspecified across Rust releases (a cache filename must be stable). Same
/// "no dependency for a tiny hash" convention as app.rs's XorShift64 RNG.
pub fn fnv1a64(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Stable hex key for a remote URI — the spooled filename stem.
pub fn spool_key(uri: &str) -> String {
    format!("{:016x}", fnv1a64(uri))
}

/// Spool cache dir: `<cache>/tplay/smb/`.
pub fn spool_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tplay")
        .join("smb")
}

/// Pure: cache path for `uri` inside a given spool dir (tests inject the dir).
/// Extension is kept from the URI so rodio's decoder can sniff the format.
pub fn cache_path_in(uri: &str, dir: &Path) -> PathBuf {
    let key = spool_key(uri);
    match Path::new(uri).extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty()) {
        Some(ext) => dir.join(format!("{key}.{ext}")),
        None => dir.join(key),
    }
}

/// Cache path for `uri` in the real spool dir.
pub fn cache_path(uri: &str) -> PathBuf {
    cache_path_in(uri, &spool_dir())
}

// ── Worker thread ────────────────────────────────────────────────────────────

/// Spawn the single SMB worker thread. It exits when the app drops its
/// `Sender<SmbCmd>` (channel disconnect on `rx.recv()` error).
fn spawn_worker(rx: Receiver<SmbCmd>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("tplay: smb worker runtime failed: {e}");
                return;
            }
        };
        rt.block_on(async move {
            // Sequential processing: browse clicks are paced by the user and a
            // spool is the one long op, so a per-command connection is fine.
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    SmbCmd::ListShares { host, creds, reply } => {
                        let res = run_list_shares(&host, &creds).await;
                        let _ = reply.send(SmbReply::Shares { host, result: res });
                    }
                    SmbCmd::ListDir { uri, creds, reply } => {
                        let res = run_list_dir(&uri, &creds).await;
                        let _ = reply.send(SmbReply::Dir { uri, result: res });
                    }
                    SmbCmd::Spool { uri, creds, reply } => {
                        let res = run_spool(&uri, &creds).await;
                        let _ = reply.send(SmbReply::Spooled { uri, result: res });
                    }
                    SmbCmd::Save { uri, data, creds, reply } => {
                        let res = run_save(&uri, &data, &creds).await;
                        let _ = reply.send(SmbReply::Saved { uri, result: res });
                    }
                }
            }
        });
    })
}

fn addr(host: &str) -> String {
    format!("{host}:{SMB_PORT}")
}

/// Errors are surfaced as strings — enough for the pane's status line.
type CmdResult<T> = Result<T, String>;

async fn connect(host: &str, creds: &SmbCreds) -> CmdResult<smb2::SmbClient> {
    smb2::connect(&addr(host), &creds.username, &creds.password)
        .await
        .map_err(|e| e.to_string())
}

async fn run_list_shares(host: &str, creds: &SmbCreds) -> CmdResult<Vec<RemoteEntry>> {
    let mut client = connect(host, creds).await?;
    let shares = client.list_shares().await.map_err(|e| e.to_string())?;
    // Disk shares (STYPE_DISKTREE) only — skip IPC$, print queues, and the
    // hidden ADMIN$/C$ special shares a server can expose.
    Ok(shares
        .into_iter()
        .filter(|s| s.share_type & 0x0000_FFFF == 0)
        .map(|s| RemoteEntry { name: s.name, size: 0, is_dir: true })
        .collect())
}

async fn run_list_dir(uri: &str, creds: &SmbCreds) -> CmdResult<Vec<RemoteEntry>> {
    let (host, share, rel) = split_uri(uri).ok_or_else(|| format!("bad uri: {uri}"))?;
    let share = share.ok_or_else(|| format!("no share in uri: {uri}"))?;
    let mut client = connect(&host, creds).await?;
    let mut tree = client.connect_share(&share).await.map_err(|e| e.to_string())?;
    let entries = client.list_directory(&mut tree, &rel).await.map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|e| RemoteEntry { name: e.name, size: e.size, is_dir: e.is_directory })
        .collect())
}

/// Download `uri` to its spool cache path. Already cached → return immediately.
/// Full-file pipelined read, then one write.
/// ponytail: whole file is buffered in RAM — fine for tracks (≤ a few hundred
/// MB); switch to the streaming `FileDownload`/write-behind path if ever
/// spooling multi-GB files.
async fn run_spool(uri: &str, creds: &SmbCreds) -> CmdResult<PathBuf> {
    let dest = cache_path(uri);
    if dest.is_file() {
        return Ok(dest);
    }
    let (host, share, rel) = split_uri(uri).ok_or_else(|| format!("bad uri: {uri}"))?;
    let share = share.ok_or_else(|| format!("no share in uri: {uri}"))?;
    let mut client = connect(&host, creds).await?;
    let mut tree = client.connect_share(&share).await.map_err(|e| e.to_string())?;
    let bytes = client.read_file_pipelined(&mut tree, &rel).await.map_err(|e| e.to_string())?;
    if let Some(p) = dest.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Err(e) = std::fs::write(&dest, &bytes) {
        let _ = std::fs::remove_file(&dest);
        return Err(e.to_string());
    }
    Ok(dest)
}

/// `run_spool`'s twin: upload bytes to a share, overwriting if it exists.
/// `write_file_pipelined` is one compound CREATE+WRITE+FLUSH+CLOSE with
/// `FileOverwriteIf`, so there is no file lifecycle to manage here.
async fn run_save(uri: &str, data: &[u8], creds: &SmbCreds) -> CmdResult<()> {
    let (host, share, rel) = split_uri(uri).ok_or_else(|| format!("bad uri: {uri}"))?;
    let share = share.ok_or_else(|| format!("no share in uri: {uri}"))?;
    let mut client = connect(&host, creds).await?;
    let mut tree = client.connect_share(&share).await.map_err(|e| e.to_string())?;
    client
        .write_file_pipelined(&mut tree, &rel, data)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}