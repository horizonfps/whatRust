# Latency and Stability Audit

- Audit date: 2026-08-14
- Baseline: `v0.6.0` (`2138618`)
- Target: Windows 10/11 with WebView2, with cross-platform regressions covered by the existing tests

## Outcome

| Area | Evidence in the baseline | Mitigation | Status |
| --- | --- | --- | --- |
| State persistence | Accounts, settings, and lock state used direct writes. An interrupted write could leave invalid JSON and silently select defaults on the next launch. | Same-directory temporary write, file flush, previous-file backup, atomic rename, and backup recovery. | Fixed |
| Unread updates | Every page-title report rebuilt the tray menu and re-read state, even when the unread count had not changed. | Ignore unchanged counts and use saturating aggregation. | Fixed |
| Windows browser identity | The app replaced WebView2's native Edge identity with a hard-coded Chrome version. Microsoft documents that overriding the UA can also clear native client hints. | Keep the native WebView2 UA and client hints on Windows. | Fixed |
| WebView2 process failures | No `ProcessFailed` handler was registered. A dead renderer could remain on an error page, and a browser-process exit did not recreate the webviews. | Reload an exited renderer, request one controlled app restart after a browser-process exit, and log auto-recoverable failures. | Fixed |
| Multi-account startup | One account-window error escaped through `?`, failed the whole setup hook, and reached a release build configured with `panic = "abort"`. | Continue opening all accounts and fail setup only if none can open. | Fixed |
| Drag-and-drop transport | A background thread could queue the full 300 MB drop through `eval()` without waiting for the page. `Ok(())` only proved that a script was queued. | Require an acknowledgement for every begin/chunk/end step, allow one in-flight chunk, apply a 15-second deadline, discard partial streams, and serialize drops per window. | Fixed |

## Session integrity

WebView2 cookies, IndexedDB, and other WhatsApp session data live in persistent webview profile directories. None of the crash-recovery paths added by this audit deletes or replaces those directories. A renderer reload or controlled application restart reopens the same profile.

The production code intentionally deletes session data only in two user-triggered flows:

- removing an account deletes that account's isolated profile;
- resetting a forgotten app-lock password deletes all profiles and logs every account out.

The audit found no ordinary crash path that explicitly unlinks the desktop session. The state-file fix also prevents a torn `accounts.json` write from temporarily hiding additional profiles from the account list; the profile data itself was never stored in that JSON file.

This does not prove that WhatsApp will never revoke a linked device. The upstream [account-review report](https://github.com/karem505/whatRust/issues/17) describes a server-side suspension immediately after linking. Removing the synthetic Windows UA reduces one avoidable identity mismatch, but it is not proof of causation or a confirmed resolution. No live account was linked during this audit.

## Remaining work

1. **P1: bound decoded drop-batch memory dynamically.** IPC transport is now bounded to one chunk, but the page may still retain up to the configured 300 MB raw batch while constructing `File` objects. Use a lower or memory-aware batch budget, or commit files in smaller groups.
2. **P1: coalesce tray work and cache configuration.** No-op unread reports are eliminated, but a real count change still synchronously loads account and lock state and rebuilds the entire native menu. Cache immutable account metadata and debounce bursts of real changes.
3. **P1: handle sustained renderer hangs without destroying drafts.** `RenderProcessUnresponsive` is logged and observed. Add a bounded health check and user choice to reload after a sustained hang; automatic reload risks losing an unsent message.
4. **P2: validate persisted account identifiers.** Generated identifiers are safe, but hand-edited semantically invalid JSON can still contain duplicate or path-like IDs. Normalize records on load before using IDs as labels or profile-directory components.
5. **P2: add privacy-preserving crash diagnostics.** Record a local launch marker, clean-shutdown marker, WebView2 failure kind, and build version. Keep message content, account names, and phone numbers out of logs. Optional local minidumps would make native crashes actionable.
6. **P2: add an automated Windows fault-injection smoke test.** Terminate a renderer, verify reload, terminate the browser process, verify controlled restart, and confirm the profile remains logged in. Run this only with a dedicated test account.

## Validation performed

- `cargo test --manifest-path src-tauri/Cargo.toml`: 78 passed.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: passed.
- `cargo build --manifest-path src-tauri/Cargo.toml --release`: passed on Windows.
- `node settings-ui/comboToAccelerator.test.mjs`: passed.
- `node settings-ui/dialog.test.mjs`: passed.
- `node src-tauri/resources/bridge.test.mjs`: passed.

The tests cover persistence recovery, startup isolation, the WebView2 recovery policy, unread no-op suppression, drag-message acknowledgement, chunk reconstruction, file-size race detection, and existing UI behavior. They do not substitute for a long-running test against the live WhatsApp service.

## References

- [Microsoft: WebView2 process-related events](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-related-events)
- [Microsoft: WebView2 UserAgent property](https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2settings.useragent)
- [Microsoft: User-Agent guidance](https://learn.microsoft.com/en-us/microsoft-edge/web-platform/user-agent-guidance)
- [Tauri: JavaScript evaluation API](https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindow.html)
