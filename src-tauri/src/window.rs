use crate::accounts::{self, Account, ActiveAccount};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Recent desktop Chrome UA for WebKit. WhatsApp Web rejects the default WebKitGTK UA.
/// Bump the major version occasionally, and keep it in sync with the client-hints
/// shim in `resources/bridge.js` (brands/fullVersionList/uaFullVersion).
///
/// Windows keeps WebView2's native Edge UA and client hints. Overriding them can
/// clear Chromium's real `Sec-CH-UA-*` values and creates an unnecessary synthetic
/// browser identity. macOS still needs a Chrome-shaped UA for WKWebView.
///
/// NOTE (Linux): setting this alone is NOT enough — WebKitGTK's site-specific
/// quirks override the embedder UA for web.whatsapp.com with a fake macOS Safari
/// string. `enable_webview_media` turns quirks off so this UA actually reaches
/// the site.
#[cfg(target_os = "linux")]
pub const CHROME_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
#[cfg(target_os = "macos")]
pub const CHROME_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

#[cfg(windows)]
fn user_agent_override() -> Option<&'static str> {
    None
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
enum Webview2Recovery {
    Reload,
    Restart,
    Observe,
}

#[cfg(windows)]
fn webview2_recovery(
    kind: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_PROCESS_FAILED_KIND,
) -> Webview2Recovery {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    };

    match kind {
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => Webview2Recovery::Restart,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => Webview2Recovery::Reload,
        _ => Webview2Recovery::Observe,
    }
}

#[cfg(not(windows))]
fn user_agent_override() -> Option<&'static str> {
    Some(CHROME_UA)
}

const BRIDGE_JS: &str = include_str!("../resources/bridge.js");
const APP_ICON: &[u8] = include_bytes!("../icons/128x128.png");

/// Open (or focus, if it already exists) the window for `account`. The label is
/// `wa-<id>`; everything the single-account window carried is preserved (Chrome UA,
/// `bridge.js`, app icon, sizes, close-to-tray), plus per-account session isolation
/// for non-default accounts.
pub fn open_account_window(
    app: &AppHandle,
    account: &Account,
    start_hidden: bool,
) -> tauri::Result<WebviewWindow> {
    let label = accounts::window_label(&account.id);

    // Reuse the existing window if it is already open.
    if let Some(w) = app.get_webview_window(&label) {
        return Ok(w);
    }

    let url = "https://web.whatsapp.com/".parse().expect("valid url");
    let icon = tauri::image::Image::from_bytes(APP_ICON)?;

    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(format!("whatRust — {}", account.name))
        .inner_size(1100.0, 800.0)
        .min_inner_size(560.0, 480.0)
        .icon(icon)?;
    let builder = match user_agent_override() {
        Some(user_agent) => builder.user_agent(user_agent),
        None => builder,
    };
    let builder = builder
        .initialization_script(BRIDGE_JS)
        // Drag-and-drop is done by capturing the OS drop in Rust and streaming the file
        // into the page (see `register_drop_handler` + bridge.js `__whatrustDropFeed`).
        // We deliberately KEEP Tauri's drag-drop handler enabled: on Linux/webkit2gtk the
        // native webview never delivers a file drop into the page DOM (broken on Wayland;
        // on X11 the GTK drop is accepted — the "+" cursor shows — but it still never
        // reaches WhatsApp Web), so relying on in-page HTML5 drop simply does not work
        // there. The handler is what hands us the dropped paths via `WindowEvent::DragDrop`.
        // Belt-and-braces: cancel any `file://` navigation so a stray drop can never
        // navigate the window away and tear down the live WhatsApp session.
        .on_navigation(|url| url.scheme() != "file")
        // Downloads: with NO handler registered, wry never wires up the platform's
        // download machinery at all — on Linux nobody answers WebKit's
        // `decide-destination` and the engine cancels every download, so WhatsApp's
        // "Download" button (videos, images, documents) silently did nothing.
        // Accept every download into the user's Downloads folder (wry pre-fills a
        // de-duplicated absolute path on Linux/Windows; the fallback covers a
        // platform handing us an empty/relative destination) and toast on finish.
        .on_download(|webview, event| {
            match event {
                tauri::webview::DownloadEvent::Requested { url, destination } => {
                    ensure_download_destination(
                        destination,
                        webview.app_handle().path().download_dir().ok(),
                    );
                    // Log routing only — never the file name (matches dlog's no-PII rule).
                    crate::dlog::log(&format!(
                        "download: requested scheme={} dest_abs={}",
                        url.scheme(),
                        destination.is_absolute()
                    ));
                }
                tauri::webview::DownloadEvent::Finished { path, success, .. } => {
                    crate::dlog::log(&format!(
                        "download: finished success={success} path_known={}",
                        path.is_some()
                    ));
                    let app = webview.app_handle();
                    if success {
                        let body = match path.as_ref().and_then(|p| p.file_name()) {
                            Some(n) => {
                                format!("{} — saved to your Downloads folder.", n.to_string_lossy())
                            }
                            // macOS never reports the final path; the folder is still right.
                            None => "Saved to your Downloads folder.".to_string(),
                        };
                        crate::notify::show(app, "Download complete", &body);
                    } else {
                        crate::notify::show(
                            app,
                            "Download failed",
                            "The file could not be downloaded. Please try again.",
                        );
                    }
                }
                _ => {}
            }
            true
        })
        .visible(!start_hidden);

    let builder = apply_isolation(builder, account, app);
    let win = builder.build()?;

    // Close-to-tray (reads the live setting so the toggle takes effect without a restart).
    let app_handle = app.clone();
    let label_for_close = label.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            if crate::settings::load(&app_handle).close_to_tray {
                let lc = crate::applock::load(&app_handle);
                if lc.is_active() && lc.lock_on_hide {
                    crate::lock::lock_now(&app_handle);
                } else if let Some(w) = app_handle.get_webview_window(&label_for_close) {
                    let _ = w.hide();
                }
                api.prevent_close();
            }
        }
    });

    apply_zoom(&win, crate::settings::load(app).zoom);
    register_focus_listener(app, &win);
    register_drop_handler(&win);
    enable_webview_media(&win);
    Ok(win)
}

/// Set the display zoom on one account webview.
///
/// This is a page zoom rather than a font-size override, because WhatsApp Web
/// sizes nearly everything in px: scaling the page is what moves the chat list
/// and the conversation pane together. Zooming out is also what buys back
/// horizontal space on a high-DPI display, where the chat list otherwise takes a
/// disproportionate share of the window.
fn apply_zoom(win: &WebviewWindow, zoom: f64) {
    // Unsupported below macOS 11; there the window keeps the site's own sizing.
    let _ = win.set_zoom(crate::settings::sanitize_zoom(zoom));
}

/// Re-apply the display zoom to every open account window, so changing the
/// setting takes effect immediately instead of at the next launch.
pub fn apply_zoom_all(app: &AppHandle, zoom: f64) {
    for a in accounts::load(app).accounts {
        if let Some(w) = app.get_webview_window(&accounts::window_label(&a.id)) {
            apply_zoom(&w, zoom);
        }
    }
}

/// Largest single dropped file we will inline-inject into the page. The page must
/// hold the decoded bytes to build the `File`, so keep it bounded; larger files are
/// skipped (with a user-visible toast) rather than risking an OOM or a long UI stall.
const MAX_DROP_FILE_BYTES: u64 = 100 * 1024 * 1024;
/// Aggregate raw-byte budget for ONE drop. Files are individually capped, but the
/// page holds every accepted file of a batch in memory at once — without a batch
/// budget, 30 near-cap files still meant multiple GiB in flight.
const MAX_DROP_TOTAL_BYTES: u64 = 300 * 1024 * 1024;
/// Cap how many files one drop can inject, counted over files that actually pass
/// the checks (a folder or an oversized file does not use up a slot).
const MAX_DROP_FILES: usize = 30;
/// Raw bytes per streamed chunk eval. A multiple of 3, so every non-final chunk
/// base64-encodes standalone without padding and the page can decode chunks
/// independently. ~4 MiB raw ≈ 5.6 MiB base64 per eval — far below WebView2's
/// cross-process message ceiling (the old single-eval transport marshaled a
/// 100 MiB file as ~266 MiB of UTF-16 in one ExecuteScript and could die
/// silently), and it keeps peak memory at one chunk instead of one file.
const DROP_CHUNK_BYTES: usize = 85 * 48 * 1024; // 4_177_920, multiple of 3

/// Capture OS file drops and inject them into WhatsApp Web.
///
/// On Linux the webview never delivers the drop into the page DOM, so Tauri's
/// drag-drop handler (kept enabled in the builder) is our only source of the dropped
/// paths — and because that handler consumes the drop on every platform, Windows and
/// macOS drops arrive here too. Files are read off the UI thread and streamed to the
/// page-side `__whatrustDropFeed` (bridge.js) as begin/chunk/end messages keyed by a
/// process-unique drop id, then committed; bridge.js rebuilds the `File`s and hands
/// them to WhatsApp's own attach flow. The commit runs through `eval_with_callback`,
/// so "the page actually executed the handler" is confirmed instead of assumed, and
/// every skipped file is surfaced to the user as a toast (not just a log line).
fn register_drop_handler(win: &WebviewWindow) {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Process-wide drop id: lets the page key concurrent streams from several
    // windows/drops without guessing by file name or content.
    static DROP_SEQ: AtomicU64 = AtomicU64::new(1);
    let stream_lock = std::sync::Arc::new(std::sync::Mutex::new(()));
    let win = win.clone();
    win.clone().on_window_event(move |event| {
        let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, position }) = event
        else {
            return;
        };
        let drop_id = DROP_SEQ.fetch_add(1, Ordering::Relaxed);
        crate::dlog::log(&format!(
            "dragdrop: drop #{drop_id}: {} path(s) at ({:.0},{:.0})",
            paths.len(),
            position.x,
            position.y
        ));
        if paths.is_empty() {
            // Seen on macOS for promise-only drag sources (e.g. dragging straight out
            // of Photos.app): wry reads only NSFilenamesPboardType, so the drop lands
            // with zero paths. Tell the user instead of silently doing nothing.
            crate::notify::show(
                win.app_handle(),
                "Nothing was attached",
                "That item can't be dropped directly. Drag the file from your file manager, or use the attach (+) button.",
            );
            return;
        }
        let paths = paths.clone();
        let w = win.clone();
        let stream_lock = stream_lock.clone();
        // Read + stream off the UI thread: a large video would otherwise stall the window.
        std::thread::spawn(move || {
            let _guard = stream_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stream_drop(&w, drop_id, &paths);
        });
    });
}

/// Why a file in a drop was skipped. Counters only — never file names (the summary
/// feeds a toast and the no-PII diagnostic log).
#[derive(Default, Debug, PartialEq, Eq)]
struct DropSkips {
    too_large: usize,
    over_budget: usize,
    over_count: usize,
    not_file: usize,
    unreadable: usize,
    changed: usize,
}

impl DropSkips {
    fn total(&self) -> usize {
        self.too_large
            + self.over_budget
            + self.over_count
            + self.not_file
            + self.unreadable
            + self.changed
    }
}

/// One accepted file of a drop, decided by [`plan_drop`] before any bytes move.
struct PlannedFile {
    path: std::path::PathBuf,
    name: String,
    mime: &'static str,
    len: u64,
}

/// Decide which dropped paths are injectable, applying the per-file cap, the batch
/// byte budget, and the file-count cap — counting only files that actually qualify
/// (30 unreadable paths must not starve a valid 31st). Pure planning: no page I/O.
fn plan_drop(paths: &[std::path::PathBuf]) -> (Vec<PlannedFile>, DropSkips) {
    let mut planned = Vec::new();
    let mut skips = DropSkips::default();
    let mut budget = MAX_DROP_TOTAL_BYTES;
    for (idx, p) in paths.iter().enumerate() {
        if planned.len() == MAX_DROP_FILES {
            skips.over_count += 1;
            continue;
        }
        let meta = match std::fs::metadata(p) {
            Ok(m) => m,
            Err(e) => {
                crate::dlog::log(&format!("dragdrop: file {idx}: stat failed: {e}"));
                skips.unreadable += 1;
                continue;
            }
        };
        if !meta.is_file() {
            crate::dlog::log(&format!("dragdrop: file {idx}: skip, not a regular file"));
            skips.not_file += 1;
            continue;
        }
        let len = meta.len();
        if len > MAX_DROP_FILE_BYTES {
            crate::dlog::log(&format!(
                "dragdrop: file {idx}: skip, {len} bytes over the {MAX_DROP_FILE_BYTES} cap"
            ));
            skips.too_large += 1;
            continue;
        }
        if len > budget {
            crate::dlog::log(&format!(
                "dragdrop: file {idx}: skip, {len} bytes over the remaining batch budget"
            ));
            skips.over_budget += 1;
            continue;
        }
        budget -= len;
        // to_string_lossy (not to_str+"file"): a name with invalid Unicode keeps its
        // (usually ASCII) extension, so MIME routing still works.
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let mime = mime_for(&name);
        planned.push(PlannedFile {
            path: p.clone(),
            name,
            mime,
            len,
        });
    }
    (planned, skips)
}

/// Human summary of skipped files for the toast. Counts and reasons only — no names.
fn summarize_skips(s: &DropSkips) -> String {
    let mut parts: Vec<String> = Vec::new();
    let plural = |n: usize| if n == 1 { "file" } else { "files" };
    if s.too_large > 0 {
        parts.push(format!(
            "{} {} over the 100 MB size limit",
            s.too_large,
            plural(s.too_large)
        ));
    }
    if s.over_budget > 0 {
        parts.push(format!(
            "{} {} over the 300 MB per-drop total",
            s.over_budget,
            plural(s.over_budget)
        ));
    }
    if s.over_count > 0 {
        parts.push(format!(
            "{} {} over the 30-file limit",
            s.over_count,
            plural(s.over_count)
        ));
    }
    if s.not_file > 0 {
        parts.push(format!(
            "{} folder(s) or special {}",
            s.not_file,
            plural(s.not_file)
        ));
    }
    if s.unreadable > 0 {
        parts.push(format!(
            "{} unreadable {}",
            s.unreadable,
            plural(s.unreadable)
        ));
    }
    if s.changed > 0 {
        parts.push(format!(
            "{} {} that changed while reading",
            s.changed,
            plural(s.changed)
        ));
    }
    format!("Not attached: {}.", parts.join(", "))
}

/// Why streaming one file to the page stopped.
enum StreamAbort {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file's size changed between planning and reading (TOCTOU guard) — a
    /// truncated or overgrown snapshot would inject a corrupt file, so it is dropped.
    Changed,
    /// `eval` into the webview failed; the whole drop is abandoned.
    Eval(tauri::Error),
    /// The page did not acknowledge a message before the bounded deadline.
    AckTimeout,
    /// The callback channel closed before the page acknowledged the message.
    AckDisconnected,
    /// The page rejected a begin, chunk, or end message.
    AckRejected,
}

// --- page-message builders (pure, unit-tested) ---

fn drop_msg_begin(drop_id: u64, idx: usize, name: &str, mime: &str, len: u64) -> String {
    let name_json = serde_json::to_string(name).unwrap_or_else(|_| "\"file\"".into());
    format!(
        "window.__whatrustDropFeed?window.__whatrustDropFeed({{op:\"begin\",drop:{drop_id},file:{idx},name:{name_json},type:\"{mime}\",size:{len}}}):\"NOHANDLER\""
    )
}

fn drop_msg_chunk_prefix(drop_id: u64, idx: usize) -> String {
    format!(
        "window.__whatrustDropFeed?window.__whatrustDropFeed({{op:\"chunk\",drop:{drop_id},file:{idx},b64:\""
    )
}

const DROP_MSG_CHUNK_SUFFIX: &str = "\"}):\"NOHANDLER\"";

fn drop_msg_end(drop_id: u64, idx: usize) -> String {
    format!(
        "window.__whatrustDropFeed?window.__whatrustDropFeed({{op:\"end\",drop:{drop_id},file:{idx}}}):\"NOHANDLER\""
    )
}

fn drop_msg_abort(drop_id: u64, idx: usize) -> String {
    format!(
        "window.__whatrustDropFeed?window.__whatrustDropFeed({{op:\"abort\",drop:{drop_id},file:{idx}}}):\"NOHANDLER\""
    )
}

fn drop_msg_commit(drop_id: u64, files: usize) -> String {
    // Ternary (not &&) so a missing handler yields a distinguishable "NOHANDLER"
    // ack through eval_with_callback instead of silently evaluating to undefined.
    format!(
        "window.__whatrustDropFeed?window.__whatrustDropFeed({{op:\"commit\",drop:{drop_id},files:{files}}}):\"NOHANDLER\""
    )
}

fn drop_ack_value(ack: &str) -> String {
    serde_json::from_str::<String>(ack).unwrap_or_else(|_| ack.trim().to_owned())
}

fn drop_ack_is(ack: &str, expected: &str) -> bool {
    drop_ack_value(ack) == expected
}

fn eval_drop_message(w: &WebviewWindow, js: String) -> Result<String, StreamAbort> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    w.eval_with_callback(js, move |ack| {
        let _ = sender.try_send(ack);
    })
    .map_err(StreamAbort::Eval)?;

    match receiver.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(ack) => Ok(ack),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(StreamAbort::AckTimeout),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(StreamAbort::AckDisconnected),
    }
}

fn eval_drop_step(w: &WebviewWindow, js: String) -> Result<(), StreamAbort> {
    let ack = eval_drop_message(w, js)?;
    if drop_ack_is(&ack, "OK") {
        Ok(())
    } else {
        Err(StreamAbort::AckRejected)
    }
}

/// Read exactly `expected` bytes from `r`, emitting standalone base64 chunks of
/// [`DROP_CHUNK_BYTES`] raw bytes each (final chunk shorter, padded). Returns
/// [`StreamAbort::Changed`] if the stream ends early or still has data past
/// `expected` — the size was validated at plan time, so a mismatch means the file
/// was modified in between and its snapshot cannot be trusted.
fn stream_chunks<R: std::io::Read>(
    r: &mut R,
    expected: u64,
    mut emit: impl FnMut(&str) -> Result<(), StreamAbort>,
) -> Result<(), StreamAbort> {
    let mut buf = [0u8; 48 * 1024];
    let mut chunk: Vec<u8> = Vec::with_capacity(DROP_CHUNK_BYTES.min(expected as usize + 2));
    let mut b64 = String::new();
    let mut total: u64 = 0;
    loop {
        let want = std::cmp::min(buf.len() as u64, expected - total) as usize;
        if want == 0 {
            break;
        }
        let n = r.read(&mut buf[..want]).map_err(StreamAbort::Io)?;
        if n == 0 {
            return Err(StreamAbort::Changed); // shrank below the planned size
        }
        total += n as u64;
        chunk.extend_from_slice(&buf[..n]);
        while chunk.len() >= DROP_CHUNK_BYTES {
            b64.clear();
            base64_encode_into(&mut b64, &chunk[..DROP_CHUNK_BYTES]);
            emit(&b64)?;
            chunk.drain(..DROP_CHUNK_BYTES);
        }
    }
    // One extra read probes for growth past the planned size.
    if r.read(&mut buf[..1]).map_err(StreamAbort::Io)? != 0 {
        return Err(StreamAbort::Changed);
    }
    if !chunk.is_empty() || expected == 0 {
        b64.clear();
        base64_encode_into(&mut b64, &chunk);
        emit(&b64)?;
    }
    Ok(())
}

/// Stream one planned file to the page as begin + chunk(s) + end.
fn stream_one_file(
    w: &WebviewWindow,
    drop_id: u64,
    idx: usize,
    f: &PlannedFile,
) -> Result<(), StreamAbort> {
    let mut file = std::fs::File::open(&f.path).map_err(StreamAbort::Io)?;
    // fstat on the open handle: the authoritative size for the bytes we will read
    // (the plan-time stat raced against renames/writes).
    let len = file.metadata().map_err(StreamAbort::Io)?.len();
    if len != f.len || len > MAX_DROP_FILE_BYTES {
        return Err(StreamAbort::Changed);
    }
    eval_drop_step(w, drop_msg_begin(drop_id, idx, &f.name, f.mime, len))?;
    let prefix = drop_msg_chunk_prefix(drop_id, idx);
    let result = stream_chunks(&mut file, len, |b64| {
        let mut js = String::with_capacity(prefix.len() + b64.len() + DROP_MSG_CHUNK_SUFFIX.len());
        js.push_str(&prefix);
        js.push_str(b64); // base64 alphabet needs no JS-string escaping
        js.push_str(DROP_MSG_CHUNK_SUFFIX);
        eval_drop_step(w, js)
    });
    let result = match result {
        Ok(()) => eval_drop_step(w, drop_msg_end(drop_id, idx)),
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        // Best-effort: release page-side chunks after any partial stream failure.
        if !matches!(error, StreamAbort::Eval(_)) {
            let _ = w.eval(drop_msg_abort(drop_id, idx));
        }
    }
    result
}

/// Plan, stream, and commit one OS drop, then surface the outcome to the user.
fn stream_drop(w: &WebviewWindow, drop_id: u64, paths: &[std::path::PathBuf]) {
    let (planned, mut skips) = plan_drop(paths);
    let mut streamed = 0usize;
    for (idx, f) in planned.iter().enumerate() {
        match stream_one_file(w, drop_id, idx, f) {
            Ok(()) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id} file {idx}: streamed {} bytes ({})",
                    f.len, f.mime
                ));
                streamed += 1;
            }
            Err(StreamAbort::Io(e)) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id} file {idx}: read failed: {e}"
                ));
                skips.unreadable += 1;
            }
            Err(StreamAbort::Changed) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id} file {idx}: skip, changed while reading"
                ));
                skips.changed += 1;
            }
            Err(StreamAbort::Eval(e)) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id}: eval failed, abandoning drop: {e}"
                ));
                crate::notify::show(
                    w.app_handle(),
                    "Files not attached",
                    "The dropped files couldn't be handed to WhatsApp. Please try again.",
                );
                return;
            }
            Err(StreamAbort::AckTimeout) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id}: page ack timed out, abandoning drop"
                ));
                crate::notify::show(
                    w.app_handle(),
                    "Files not attached",
                    "WhatsApp stopped responding while receiving the dropped files. Please try again.",
                );
                return;
            }
            Err(StreamAbort::AckDisconnected | StreamAbort::AckRejected) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id}: page rejected or lost the stream"
                ));
                crate::notify::show(
                    w.app_handle(),
                    "Files not attached",
                    "WhatsApp wasn't ready to receive the dropped files. Please try again.",
                );
                return;
            }
        }
    }
    if streamed > 0 {
        let commit = drop_msg_commit(drop_id, streamed);
        match eval_drop_message(w, commit) {
            Ok(ack) => {
                let ack = drop_ack_value(&ack);
                crate::dlog::log(&format!("dragdrop: drop #{drop_id} commit ack: {ack}"));
                if !ack.starts_with("QUEUED:") {
                    crate::notify::show(
                        w.app_handle(),
                        "Files not attached",
                        "WhatsApp wasn't ready to receive the dropped files. Please try again.",
                    );
                }
            }
            Err(_) => {
                crate::dlog::log(&format!(
                    "dragdrop: drop #{drop_id}: commit acknowledgement failed"
                ));
                crate::notify::show(
                    w.app_handle(),
                    "Files not attached",
                    "WhatsApp wasn't ready to receive the dropped files. Please try again.",
                );
            }
        }
    }
    if skips.total() > 0 {
        crate::notify::show(
            w.app_handle(),
            "Some files were not attached",
            &summarize_skips(&skips),
        );
    }
}

/// Best-effort MIME from the file extension, so WhatsApp routes images/videos/docs to
/// the right composer. Unknown types fall back to a generic binary type (still sends).
///
/// Why the image list matters: bridge.js routes anything whose MIME starts with `image/`
/// to the Photos & Videos composer (a photo); anything else goes to the Document composer.
/// A modern phone photo (AVIF, HEIF/HEIC) that fell through to `application/octet-stream`
/// was therefore attached as a *file* instead of a *photo* — covering those extensions
/// fixes the routing. We deliberately do NOT route niche raster formats (TIFF, ICO, APNG)
/// as images: WhatsApp's photo composer may reject them, which would be worse than the
/// current behaviour of sending them as a document — so they stay documents. Non-native
/// video containers also still go as a document (only mp4/3gpp/quicktime are accepted by
/// the media input), but get a correct label rather than a generic one.
fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        // Images (image/* -> routed to the Photos & Videos composer by bridge.js). Limited to
        // formats WhatsApp's photo composer accepts, so nothing regresses to "not supported".
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "heif" | "hif" => "image/heif",
        // Video. Only mp4/3gpp/quicktime are accepted by WhatsApp's media input (bridge.js
        // NATIVE_VIDEO); the rest still send, as a document, but with a correct label.
        "mp4" | "m4v" => "video/mp4",
        "mov" | "qt" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "3gp" | "3gpp" => "video/3gpp",
        "3g2" | "3gp2" | "3gpp2" => "video/3gpp2",
        "avi" => "video/x-msvideo",
        "mpeg" | "mpg" => "video/mpeg",
        "mts" | "m2ts" => "video/mp2t",
        "ogv" => "video/ogg",
        "flv" => "video/x-flv",
        // Audio (sent as a document; correct labels help WhatsApp render an audio preview).
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "weba" => "audio/webm",
        "amr" => "audio/amr",
        "mid" | "midi" => "audio/midi",
        // Documents / archives / text.
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rtf" => "application/rtf",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "epub" => "application/epub+zip",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

/// Append the standard base64 (RFC 4648, with `=` padding) of `data` to `out`. Hand-rolled
/// to avoid pulling a crate into this otherwise lean dependency tree.
///
/// Encodes per 3-byte group, padding only a final partial group. Callers that feed data
/// across multiple calls (streaming) MUST pass whole 3-byte groups on every call except the
/// last — otherwise an interior partial group would be padded mid-stream. [`stream_chunks`]
/// upholds that contract by emitting whole [`DROP_CHUNK_BYTES`] (multiple-of-3) chunks.
fn base64_encode_into(out: &mut String, data: &[u8]) {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    out.reserve(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
}

/// Standard base64 of `data` as an owned `String`. Thin wrapper over [`base64_encode_into`].
/// Only the chunked [`stream_chunks`] is used in production now, so this whole-buffer
/// form is exercised by the tests (as the parity oracle) — hence `cfg(test)`.
#[cfg(test)]
fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    base64_encode_into(&mut out, data);
    out
}

/// Make sure a platform-suggested download `destination` is usable: keep an
/// absolute path with a file name as-is; otherwise rebuild it as
/// `<download_dir>/<file name>` (file name defaulting to "download", directory
/// defaulting to the temp dir if the platform can't name a Downloads folder).
fn ensure_download_destination(
    destination: &mut std::path::PathBuf,
    download_dir: Option<std::path::PathBuf>,
) {
    if destination.is_absolute() && destination.file_name().is_some() {
        return;
    }
    let name = destination
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "download".into());
    let mut dir = download_dir.unwrap_or_else(std::env::temp_dir);
    dir.push(name);
    *destination = dir;
}

/// Track the last-focused account window in `ActiveAccount`. Registered once per
/// window inside `open_account_window`, so startup *and* dynamically-added windows
/// get it exactly once.
fn register_focus_listener(app: &AppHandle, win: &WebviewWindow) {
    let app_handle = app.clone();
    let label = win.label().to_string();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(true) = event {
            if let Some(active) = app_handle.try_state::<ActiveAccount>() {
                *active.lock().unwrap() = label.clone();
            }
        }
    });
}

/// Apply per-account session isolation to a window builder.
///
/// The `default` account uses the default webview store (no override) so the
/// pre-multi-account login is preserved. Additional accounts get an isolated store:
/// `data_directory` on Linux/Windows, `data_store_identifier` (the persisted v4 UUID)
/// on macOS. `data_directory`/`data_store_identifier` compile on every platform in
/// tauri; only *which* one is called is cfg-gated, with a `compile_error!()` catch-all
/// so a future platform can't silently skip isolation.
fn apply_isolation<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle<tauri::Wry>>,
    account: &Account,
    app: &AppHandle,
) -> WebviewWindowBuilder<'a, tauri::Wry, tauri::AppHandle<tauri::Wry>> {
    // The default account always uses the shared default store.
    if account.id == "default" {
        return builder;
    }

    #[cfg(any(target_os = "linux", windows))]
    {
        let _ = app;
        if let Ok(dir) = accounts::profile_dir(app, &account.id) {
            let _ = std::fs::create_dir_all(&dir);
            return builder.data_directory(dir);
        }
        builder
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        // Non-default accounts carry a persisted, non-nil v4 UUID. Fall back to a
        // freshly generated non-nil UUID rather than risk the nil-UUID exception.
        let uuid = account.store_uuid.unwrap_or_else(accounts::gen_store_uuid);
        builder.data_store_identifier(uuid)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (builder, account, app);
        compile_error!("per-account session isolation is not implemented for this platform");
    }
}

/// Whether the platform can isolate additional accounts. macOS needs >= 14 for
/// `data_store_identifier`. Linux/Windows always can. Returns an `Err` message
/// suitable for surfacing in the Accounts UI.
pub fn ensure_isolation_supported() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
        // data_store_identifier requires macOS >= 14.
        let required = NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 0,
            patchVersion: 0,
        };
        let ok = NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(required);
        if ok {
            Ok(())
        } else {
            Err("Multiple accounts require macOS 14 or newer.".into())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// Grant microphone/camera + media settings to WhatsApp Web, and make sure the
/// site actually sees our Chrome UA. The system webview denies getUserMedia by
/// default, which blocks voice messages (the "Allow microphone" prompt). We
/// enable the media settings and auto-approve the webview's permission requests
/// for the WhatsApp window.
///
/// Reality check (verified against webkit2gtk 2.52.3 on this distro): the
/// library is built WITHOUT a WebRTC backend — `RTCPeerConnection` stays
/// undefined even with `enable-webrtc` on (the setting is kept as a harmless
/// forward-compat no-op). Voice/video CALLS therefore cannot work in the Linux
/// system webview no matter what we spoof; WhatsApp's "your browser doesn't
/// support calling" is, on Linux, literally true. Calls can work on Windows
/// (WebView2 is Chromium and ships WebRTC).
fn enable_webview_media(win: &WebviewWindow) {
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::glib::prelude::ObjectExt;
        use webkit2gtk::{PermissionRequestExt, WebViewExt};
        let _ = win.with_webview(|webview| {
            let wv = webview.inner();
            if let Some(settings) = WebViewExt::settings(&wv) {
                settings.set_property("enable-media-stream", true);
                settings.set_property("enable-mediasource", true);
                settings.set_property("enable-webrtc", true);
                settings.set_property("enable-encrypted-media", true);
                // CRITICAL: WebKitGTK hardcodes per-site UA quirks, and
                // web.whatsapp.com is on the list — with quirks enabled (the
                // default) the engine REPLACES our Chrome UA with a fake macOS
                // Safari string ("... Version/60.5 Safari/605.1.15"), which
                // WhatsApp reads as an ancient Safari and answers by disabling
                // video sending and calling ("please update your browser").
                // Turning quirks off lets CHROME_UA through unmodified.
                settings.set_property("enable-site-specific-quirks", false);
                // Opt-in inspector for live diagnosis: run `WHATRUST_DEVTOOLS=1
                // whatrust` and right-click > Inspect Element.
                if std::env::var_os("WHATRUST_DEVTOOLS").is_some() {
                    settings.set_property("enable-developer-extras", true);
                }
            }
            wv.connect_permission_request(|_wv, req| {
                req.allow();
                true
            });
            // The initial navigation was issued before this callback ran, i.e.
            // with the quirked UA still active. Reload once so the session is
            // consistently Chrome from the very first request WhatsApp sees.
            wv.reload();
            crate::dlog::log(
                "webview: media settings applied, site-specific quirks OFF, reloaded with Chrome UA",
            );
        });
    }
    #[cfg(target_os = "windows")]
    {
        let app = win.app_handle().clone();
        let _ = win.with_webview(move |webview| enable_media_windows(webview, app));
    }
    #[cfg(target_os = "macos")]
    {
        // wry already installs a WKUIDelegate that auto-grants requestMediaCapturePermission;
        // mic/camera are gated only by the Info.plist usage-description keys (see src-tauri/Info.plist).
        let _ = win;
    }
}

/// Windows (WebView2): auto-allow microphone/camera permission requests so WhatsApp
/// voice messages and calls work without a prompt (and can't be wedged by a prior "Block").
#[cfg(target_os = "windows")]
fn enable_media_windows(webview: tauri::webview::PlatformWebview, app: AppHandle) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2PermissionRequestedEventArgs,
        ICoreWebView2ProcessFailedEventArgs, COREWEBVIEW2_PERMISSION_KIND,
        COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PROCESS_FAILED_KIND,
    };
    use webview2_com::{PermissionRequestedEventHandler, ProcessFailedEventHandler};

    static RESTARTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    // SAFETY: with_webview runs on the UI thread where the WebView2 controller lives;
    // these are standard WebView2 COM calls.
    unsafe {
        let controller = webview.controller();
        let core: ICoreWebView2 = match controller.CoreWebView2() {
            Ok(c) => c,
            Err(_) => return,
        };
        let handler = PermissionRequestedEventHandler::create(Box::new(
            move |_wv: Option<ICoreWebView2>,
                  args: Option<ICoreWebView2PermissionRequestedEventArgs>|
                  -> windows_core::Result<()> {
                if let Some(args) = args {
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                    args.PermissionKind(&mut kind)?;
                    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                    {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    }
                }
                Ok(())
            },
        ));
        let mut token: i64 = 0;
        let _ = core.add_PermissionRequested(&handler, &mut token);

        let failure_handler = ProcessFailedEventHandler::create(Box::new(
            move |sender: Option<ICoreWebView2>,
                  args: Option<ICoreWebView2ProcessFailedEventArgs>|
                  -> windows_core::Result<()> {
                let Some(args) = args else {
                    crate::dlog::log("webview: process failure reported without details");
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                args.ProcessFailedKind(&mut kind)?;
                crate::dlog::log(&format!("webview: process failure kind={}", kind.0));

                match webview2_recovery(kind) {
                    Webview2Recovery::Reload => {
                        if let Some(sender) = sender {
                            match sender.Reload() {
                                Ok(()) => crate::dlog::log("webview: renderer reload requested"),
                                Err(_) => crate::dlog::log("webview: renderer reload failed"),
                            }
                        }
                    }
                    Webview2Recovery::Restart => {
                        if !RESTARTING.swap(true, std::sync::atomic::Ordering::AcqRel) {
                            crate::dlog::log("webview: browser process exited; restart requested");
                            app.request_restart();
                        }
                    }
                    Webview2Recovery::Observe => {}
                }
                Ok(())
            },
        ));
        let mut failure_token: i64 = 0;
        if core
            .add_ProcessFailed(&failure_handler, &mut failure_token)
            .is_err()
        {
            crate::dlog::log("webview: failed to register process recovery");
        }
    }
}

/// Show + unminimize + focus an account window by its `wa-<id>` label.
pub fn show_account(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Show the active (last-focused) account window. Falls back to the first existing
/// account window, then the settings window.
pub fn show_active(app: &AppHandle) {
    // If the app is locked, any "reveal" request shows the lock screen, never an
    // account window. Covers tray click, global shortcut, single-instance, macOS Reopen.
    if !crate::lock::is_unlocked(app) {
        crate::lock::show_lock_window(app);
        return;
    }
    if let Some(active) = app.try_state::<ActiveAccount>() {
        let label = active.lock().unwrap().clone();
        if app.get_webview_window(&label).is_some() {
            show_account(app, &label);
            return;
        }
    }
    // Fall back to any account window.
    if let Some(label) = app
        .webview_windows()
        .keys()
        .find(|l| l.starts_with("wa-"))
        .cloned()
    {
        show_account(app, &label);
        return;
    }
    // Last resort: the settings window.
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Backwards-compatible shim: single-instance + macOS Reopen call this; it now
/// targets the active account.
pub fn show_main(app: &AppHandle) {
    show_active(app);
}

/// What a toggle should do given the active window's visibility.
#[derive(Debug, PartialEq, Eq)]
pub enum ToggleAct {
    Hide,
    Show,
}

/// Pure toggle decision: a visible active window is hidden; anything else (a hidden
/// window, or no active window at all → `None`) is shown.
pub fn toggle_decision(active_visible: Option<bool>) -> ToggleAct {
    match active_visible {
        Some(true) => ToggleAct::Hide,
        _ => ToggleAct::Show,
    }
}

/// Toggle the active account window: hide it if visible, otherwise show + focus it.
/// The "show" path goes through `show_active`, which defers to the lock screen when
/// the app is locked — so a toggle (e.g. an OS-bound `whatrust --toggle` on Wayland,
/// where in-process global hotkeys can't fire) can NEVER reveal an account window
/// while locked. The "hide" path only triggers when an account window is visible,
/// which cannot happen while locked.
pub fn toggle_active(app: &AppHandle) {
    let label = app
        .try_state::<ActiveAccount>()
        .map(|a| a.lock().unwrap().clone());
    let visible = label
        .as_ref()
        .and_then(|l| app.get_webview_window(l))
        .map(|w| w.is_visible().unwrap_or(false));
    match toggle_decision(visible) {
        ToggleAct::Hide => {
            if let Some(l) = label {
                if let Some(w) = app.get_webview_window(&l) {
                    let _ = w.hide();
                }
            }
        }
        ToggleAct::Show => show_active(app),
    }
}

/// Opens (or focuses) the local settings window.
pub fn open_settings_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
        .title("whatRust — Settings")
        .inner_size(440.0, 680.0)
        .resizable(false)
        .build();
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use super::CHROME_UA;
    use super::{
        base64_encode, drop_ack_is, drop_msg_begin, drop_msg_chunk_prefix, drop_msg_commit,
        drop_msg_end, ensure_download_destination, mime_for, plan_drop, stream_chunks,
        summarize_skips, toggle_decision, user_agent_override, DropSkips, StreamAbort, ToggleAct,
        DROP_CHUNK_BYTES, DROP_MSG_CHUNK_SUFFIX, MAX_DROP_FILES,
    };

    #[cfg(windows)]
    #[test]
    fn windows_keeps_the_native_webview2_user_agent() {
        assert_eq!(user_agent_override(), None);
    }

    #[cfg(windows)]
    #[test]
    fn webview2_failure_policy_recovers_only_fatal_content_failures() {
        use super::{webview2_recovery, Webview2Recovery};
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
            COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
        };

        assert_eq!(
            webview2_recovery(COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED),
            Webview2Recovery::Restart
        );
        assert_eq!(
            webview2_recovery(COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED),
            Webview2Recovery::Reload
        );
        assert_eq!(
            webview2_recovery(COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE),
            Webview2Recovery::Observe
        );
        assert_eq!(
            webview2_recovery(COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED),
            Webview2Recovery::Observe
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn chrome_ua_and_bridge_client_hints_agree_on_the_version() {
        // The UA header (Rust) and the client-hints shim (bridge.js) must present the
        // same Chrome major version, or WhatsApp sees an inconsistent browser.
        let major = CHROME_UA
            .split("Chrome/")
            .nth(1)
            .and_then(|s| s.split('.').next())
            .expect("CHROME_UA carries a Chrome/<major> token");
        let bridge = include_str!("../resources/bridge.js");
        assert!(
            bridge.contains(&format!("version: \"{major}\"")),
            "bridge.js client hints must advertise Chrome {major}"
        );
        assert!(
            bridge.contains(&format!("uaFullVersion: \"{major}.0.0.0\"")),
            "bridge.js uaFullVersion must advertise Chrome {major}"
        );
    }

    #[test]
    fn absolute_download_destination_is_kept() {
        // A Unix "/..." path is NOT absolute on Windows (no drive letter), so give
        // each platform a path that is genuinely absolute there.
        #[cfg(windows)]
        let abs = "C:\\Users\\user\\Downloads\\video.mp4";
        #[cfg(not(windows))]
        let abs = "/home/user/Downloads/video.mp4";
        let mut d = std::path::PathBuf::from(abs);
        ensure_download_destination(&mut d, Some(std::path::PathBuf::from("/elsewhere")));
        assert_eq!(d, std::path::PathBuf::from(abs));
    }

    #[test]
    fn empty_download_destination_falls_back_to_download_dir() {
        let mut d = std::path::PathBuf::new();
        ensure_download_destination(&mut d, Some(std::path::PathBuf::from("/dl")));
        assert_eq!(d, std::path::PathBuf::from("/dl/download"));
    }

    #[test]
    fn relative_download_destination_moves_into_download_dir() {
        let mut d = std::path::PathBuf::from("clip.mp4");
        ensure_download_destination(&mut d, Some(std::path::PathBuf::from("/dl")));
        assert_eq!(d, std::path::PathBuf::from("/dl/clip.mp4"));
    }

    #[test]
    fn missing_download_dir_falls_back_to_temp() {
        let mut d = std::path::PathBuf::from("clip.mp4");
        ensure_download_destination(&mut d, None);
        assert_eq!(d, std::env::temp_dir().join("clip.mp4"));
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        // The canonical RFC 4648 test vectors, including every padding case.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes() {
        assert_eq!(base64_encode(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(base64_encode(&[0x00]), "AA==");
    }

    // Write `bytes` to a unique temp file and return its path. Caller removes it.
    fn write_temp(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        use std::io::Write;
        let p = std::env::temp_dir().join(format!(
            "whatrust_test_{}_{}_{tag}",
            std::process::id(),
            bytes.len()
        ));
        std::fs::File::create(&p).unwrap().write_all(bytes).unwrap();
        p
    }

    #[test]
    fn stream_chunks_matches_oneshot_encoding() {
        // Sizes covering: below one chunk (every length-mod-3 case), exactly one
        // chunk, and just past a chunk boundary — chunk concatenation must equal
        // the one-shot base64 of the whole input (non-final chunks are unpadded
        // because DROP_CHUNK_BYTES is a multiple of 3).
        for len in [
            0usize,
            1,
            2,
            3,
            48 * 1024 + 1,
            DROP_CHUNK_BYTES,
            DROP_CHUNK_BYTES + 1,
            DROP_CHUNK_BYTES + 2,
        ] {
            let data: Vec<u8> = (0..len)
                .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
                .collect();
            let mut got = String::new();
            let mut chunks = 0usize;
            stream_chunks(&mut &data[..], len as u64, |b64| {
                chunks += 1;
                got.push_str(b64);
                Ok(())
            })
            .unwrap_or_else(|_| panic!("stream failed at len={len}"));
            assert_eq!(got, base64_encode(&data), "mismatch at len={len}");
            let expect_chunks = std::cmp::max(1, len.div_ceil(DROP_CHUNK_BYTES));
            assert_eq!(chunks, expect_chunks, "chunk count at len={len}");
        }
    }

    #[test]
    fn stream_chunks_rejects_shrunk_and_grown_files() {
        // The reader enforces the planned size: fewer bytes than promised (file
        // truncated mid-read) or extra bytes past it (file grew) must abort with
        // Changed — the old code silently read to EOF, bypassing the size cap.
        let data = [7u8; 100];
        let shrunk = stream_chunks(&mut &data[..], 200, |_| Ok(()));
        assert!(matches!(shrunk, Err(StreamAbort::Changed)), "shrunk file");
        let grown = stream_chunks(&mut &data[..], 50, |_| Ok(()));
        assert!(matches!(grown, Err(StreamAbort::Changed)), "grown file");
    }

    #[test]
    fn plan_drop_applies_per_file_and_total_caps_with_feedback() {
        let small = write_temp("ok.mp4", &[1u8; 32]);
        let dir = std::env::temp_dir();
        let missing = std::path::PathBuf::from("/nonexistent/whatrust/gone.bin");
        let (planned, skips) = plan_drop(&[small.clone(), dir, missing]);
        let _ = std::fs::remove_file(&small);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].mime, "video/mp4");
        assert_eq!(planned[0].len, 32);
        assert_eq!(skips.not_file, 1);
        assert_eq!(skips.unreadable, 1);
        assert_eq!(skips.total(), 2);
        // The toast summary names both reasons, never file names.
        let s = summarize_skips(&skips);
        assert!(s.contains("folder"), "{s}");
        assert!(s.contains("unreadable"), "{s}");
        assert!(!s.contains("ok.mp4"), "{s}");
    }

    #[test]
    fn plan_drop_counts_only_valid_files_toward_the_cap() {
        // 30 invalid paths followed by one valid file: the valid file must still be
        // planned (the old `take(30)` burned slots on the invalid entries).
        let valid = write_temp("late.png", &[9u8; 8]);
        let mut paths: Vec<std::path::PathBuf> = (0..MAX_DROP_FILES)
            .map(|i| std::path::PathBuf::from(format!("/nonexistent/whatrust/{i}.bin")))
            .collect();
        paths.push(valid.clone());
        let (planned, skips) = plan_drop(&paths);
        let _ = std::fs::remove_file(&valid);
        assert_eq!(planned.len(), 1, "valid 31st file must survive");
        assert_eq!(skips.unreadable, MAX_DROP_FILES);
        assert_eq!(skips.over_count, 0);
    }

    #[test]
    fn plan_drop_enforces_the_aggregate_budget() {
        use std::io::Write;
        // Three sparse-ish files of 150 MB nominal size would blow the 300 MB batch
        // budget on the third. Use set_len to avoid writing real gigabytes.
        let mut paths = Vec::new();
        for i in 0..3 {
            let p = std::env::temp_dir().join(format!(
                "whatrust_test_{}_budget_{i}.bin",
                std::process::id()
            ));
            let f = std::fs::File::create(&p).unwrap();
            f.set_len(90 * 1024 * 1024).unwrap();
            drop(f);
            // touch so metadata is fresh
            std::fs::OpenOptions::new()
                .append(true)
                .open(&p)
                .unwrap()
                .flush()
                .unwrap();
            paths.push(p);
        }
        // 90+90+90 = 270 MB fits; add a 4th to cross 300 MB.
        let extra = std::env::temp_dir().join(format!(
            "whatrust_test_{}_budget_extra.bin",
            std::process::id()
        ));
        let f = std::fs::File::create(&extra).unwrap();
        f.set_len(90 * 1024 * 1024).unwrap();
        drop(f);
        paths.push(extra.clone());
        let (planned, skips) = plan_drop(&paths);
        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
        assert_eq!(planned.len(), 3, "first 270 MB fit the 300 MB budget");
        assert_eq!(
            skips.over_budget, 1,
            "the 4th file exceeds the batch budget"
        );
    }

    #[test]
    fn drop_messages_are_well_formed_and_guarded() {
        let begin = drop_msg_begin(7, 0, "a \"quoted\" name.mp4", "video/mp4", 42);
        assert!(begin.starts_with("window.__whatrustDropFeed?"));
        assert!(begin.ends_with(":\"NOHANDLER\""));
        assert!(
            begin.contains("\\\"quoted\\\""),
            "name must be JSON-escaped"
        );
        assert!(begin.contains("size:42"));
        let prefix = drop_msg_chunk_prefix(7, 0);
        assert!(prefix.ends_with("b64:\""));
        assert!(DROP_MSG_CHUNK_SUFFIX.starts_with('"'));
        assert!(DROP_MSG_CHUNK_SUFFIX.ends_with(":\"NOHANDLER\""));
        assert!(drop_msg_end(7, 0).contains("\"end\""));
        // Commit uses a ternary so a missing handler acks "NOHANDLER" instead of
        // silently evaluating to undefined.
        let commit = drop_msg_commit(7, 3);
        assert!(commit.contains("?window.__whatrustDropFeed("));
        assert!(commit.ends_with(":\"NOHANDLER\""));
    }

    #[test]
    fn drop_ack_requires_the_exact_expected_value() {
        assert!(drop_ack_is("\"OK\"", "OK"));
        assert!(drop_ack_is("QUEUED:2", "QUEUED:2"));
        assert!(!drop_ack_is("\"NOHANDLER\"", "OK"));
        assert!(!drop_ack_is("\"NOT_OK\"", "OK"));
    }

    #[test]
    fn skip_summary_reads_naturally_for_single_reasons() {
        let s = summarize_skips(&DropSkips {
            too_large: 1,
            ..Default::default()
        });
        assert_eq!(s, "Not attached: 1 file over the 100 MB size limit.");
    }

    #[test]
    fn mime_is_extension_and_case_insensitive() {
        assert_eq!(mime_for("Photo.JPG"), "image/jpeg");
        assert_eq!(mime_for("clip.mp4"), "video/mp4");
        assert_eq!(mime_for("doc.pdf"), "application/pdf");
        assert_eq!(mime_for("noext"), "application/octet-stream");
        assert_eq!(mime_for("archive.unknownext"), "application/octet-stream");
    }

    #[test]
    fn modern_image_types_resolve_to_image_so_they_route_as_photos() {
        // The routing fix: bridge.js sends anything `image/*` to the Photos composer. These
        // used to fall through to octet-stream and were mis-attached as documents.
        for n in ["pic.avif", "IMG_1.HEIF", "shot.heic"] {
            assert!(
                mime_for(n).starts_with("image/"),
                "{n} should resolve to an image/* type, got {}",
                mime_for(n)
            );
        }
        // Niche raster formats are deliberately NOT routed as photos (WhatsApp's photo
        // composer may reject them) — they stay documents, which always sends.
        for n in ["scan.tiff", "icon.ico", "frames.apng"] {
            assert_eq!(
                mime_for(n),
                "application/octet-stream",
                "{n} should stay a document"
            );
        }
    }

    #[test]
    fn new_av_and_doc_types_have_specific_labels() {
        assert_eq!(mime_for("song.flac"), "audio/flac");
        assert_eq!(mime_for("clip.aac"), "audio/aac");
        assert_eq!(mime_for("movie.mpeg"), "video/mpeg");
        assert_eq!(mime_for("notes.md"), "text/markdown");
        assert_eq!(mime_for("data.json"), "application/json");
        assert_eq!(mime_for("Archive.7Z"), "application/x-7z-compressed");
        assert_eq!(mime_for("book.epub"), "application/epub+zip");
        // Unknown extensions still fall back so the file always sends.
        assert_eq!(mime_for("mystery.qwerty"), "application/octet-stream");
    }

    #[test]
    fn standard_extension_aliases_route_like_their_canonical_forms() {
        // Aliases straight out of the freedesktop MIME database; these used to fall
        // through to octet-stream and mis-route photos/videos as documents.
        assert_eq!(mime_for("shot.jpe"), "image/jpeg");
        assert_eq!(mime_for("clip.3gpp"), "video/3gpp");
        assert_eq!(mime_for("movie.qt"), "video/quicktime");
        assert_eq!(mime_for("pic.hif"), "image/heif");
        assert_eq!(mime_for("clip.3gp2"), "video/3gpp2");
    }

    #[test]
    fn visible_active_window_is_hidden() {
        assert_eq!(toggle_decision(Some(true)), ToggleAct::Hide);
    }

    #[test]
    fn hidden_active_window_is_shown() {
        assert_eq!(toggle_decision(Some(false)), ToggleAct::Show);
    }

    #[test]
    fn no_active_window_is_shown() {
        assert_eq!(toggle_decision(None), ToggleAct::Show);
    }
}
