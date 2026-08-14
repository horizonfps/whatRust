(function () {
  "use strict";
  if (window.location.origin !== "https://web.whatsapp.com") return;

  function invoke(cmd, args) {
    var t = window.__TAURI__;
    if (t && t.core && typeof t.core.invoke === "function") {
      return t.core.invoke(cmd, args).catch(function () {});
    }
    return Promise.resolve();
  }

  // Native OS toast with short-window de-duplication. WhatsApp can raise the same alert
  // more than once in a burst (a React re-render, or one message surfacing through two
  // notification code paths), and unlike a browser (which collapses repeats by tag) a
  // native toast stacks every call as a separate visible popup. Suppress an identical
  // title+body seen within DEDUP_MS; genuinely different messages are never affected.
  // Returns true if forwarded, false if suppressed as a duplicate.
  var DEDUP_MS = 3500;
  var recentNotif = Object.create(null);
  function nativeNotify(title, body) {
    title = String(title || "WhatsApp");
    body = String(body || "");
    var now = Date.now();
    for (var k in recentNotif) {
      if (now - recentNotif[k] > DEDUP_MS) delete recentNotif[k]; // prune stale keys
    }
    var key = title + " " + body;
    if (recentNotif[key] && now - recentNotif[key] <= DEDUP_MS) return false;
    recentNotif[key] = now;
    invoke("notify", { title: title, body: body });
    return true;
  }

  // 0) Page-error tap -> diagnostic log. WhatsApp failures (e.g. a media worker
  //    dying) are invisible at the process level; forward the page's error
  //    surface to dlog so a hang can be diagnosed from the log alone. Capped and
  //    truncated; technical error text only.
  try {
    var errCount = 0;
    var elog = function (m) {
      if (errCount >= 40) return;
      errCount++;
      try { invoke("dlog", { msg: ("pageerr: " + m).slice(0, 300) }); } catch (e) {}
    };
    window.addEventListener("error", function (e) {
      if (e && e.message) elog("onerror: " + e.message + " @" + (e.filename || "") + ":" + (e.lineno || 0));
    }, true);
    window.addEventListener("unhandledrejection", function (e) {
      var r = e && e.reason;
      elog("unhandledrejection: " + ((r && (r.stack || r.message)) || r));
    });
    // NOTE: we deliberately do NOT hook console.error — WhatsApp routes routine,
    // message-adjacent logs through it, which would risk leaking chat content into
    // the diagnostic log (dlog's no-PII rule). window.onerror / unhandledrejection /
    // worker-onerror capture genuine faults without that exposure.
    // Unhandled errors inside dedicated workers (WhatsApp's wasm media pipeline
    // runs there) never reach window.onerror; wrap the constructor to listen.
    var NativeWorker = window.Worker;
    if (typeof NativeWorker === "function") {
      var WrappedWorker = function Worker(url, opts) {
        var w = opts !== undefined ? new NativeWorker(url, opts) : new NativeWorker(url);
        try {
          w.addEventListener("error", function (e) {
            elog("worker onerror: " + ((e && e.message) || "?") + " @" + ((e && e.filename) || "") + ":" + ((e && e.lineno) || 0));
          });
        } catch (e) {}
        return w;
      };
      WrappedWorker.prototype = NativeWorker.prototype;
      window.Worker = WrappedWorker;
    }
  } catch (e) {}

  // 1) Client Hints shim — navigator.userAgentData is undefined in WebKit
  //    (WebKitGTK on Linux, WKWebView on macOS), which WhatsApp's capability
  //    check can trip over. On Windows WebView2 is Chromium: userAgentData is
  //    real and this shim is skipped. Windows also keeps WebView2's native UA;
  //    WebKit platforms derive the platform token from Rust's CHROME_UA.
  try {
    var uaPlatform = /Macintosh|Mac OS X/.test(navigator.userAgent || "")
      ? "macOS"
      : /Windows/.test(navigator.userAgent || "")
        ? "Windows"
        : "Linux";
    var uaPlatformVersion = uaPlatform === "macOS" ? "13.0.0" : uaPlatform === "Windows" ? "10.0.0" : "6.0.0";
    if (!navigator.userAgentData) {
      Object.defineProperty(navigator, "userAgentData", {
        configurable: true,
        value: {
          brands: [
            { brand: "Chromium", version: "143" },
            { brand: "Google Chrome", version: "143" },
            { brand: "Not_A Brand", version: "24" },
          ],
          mobile: false,
          platform: uaPlatform,
          // Real Chrome returns the requested hints (and these fields are what WhatsApp's
          // capability/calling check reads). The previous shim omitted bitness,
          // fullVersionList and wow64 and left platformVersion empty, which a strict check
          // can treat as "not Chrome". Return the full, internally-consistent set; returning
          // hints that weren't asked for is harmless.
          getHighEntropyValues: function () {
            return Promise.resolve({
              architecture: "x86",
              bitness: "64",
              brands: [
                { brand: "Chromium", version: "143" },
                { brand: "Google Chrome", version: "143" },
                { brand: "Not_A Brand", version: "24" },
              ],
              fullVersionList: [
                { brand: "Chromium", version: "143.0.0.0" },
                { brand: "Google Chrome", version: "143.0.0.0" },
                { brand: "Not_A Brand", version: "24.0.0.0" },
              ],
              mobile: false,
              model: "",
              platform: uaPlatform,
              platformVersion: uaPlatformVersion,
              uaFullVersion: "143.0.0.0",
              wow64: false,
            });
          },
        },
      });
    }
  } catch (e) {}

  // 1b) Chrome environment marker. WhatsApp Web's eligibility checks probe for
  //     `window.chrome` beyond the UA and userAgentData (both spoofed above). Add a
  //     minimal, idempotent stand-in matching what a real Chrome minimally exposes;
  //     never clobber a genuine `window.chrome` (WebView2 on Windows has a real one).
  //     NOTE: this makes the *presentation* consistent; it cannot conjure missing
  //     engine APIs. Linux distro WebKitGTK ships no WebRTC backend, so calling
  //     stays unsupported there regardless (verified: RTCPeerConnection undefined
  //     with enable-webrtc on, webkit2gtk 2.52.3).
  try {
    if (!window.chrome) {
      Object.defineProperty(window, "chrome", {
        configurable: true,
        enumerable: true,
        writable: true,
        value: { app: { isInstalled: false }, runtime: {} },
      });
    }
  } catch (e) {}

  // 2) Notification override — forward to a native OS notification via Rust.
  try {
    function ShimNotification(title, options) {
      options = options || {};
      this.title = title;
      this.body = options.body || "";
      this.onclick = null;
      this.onclose = null;
      this.onerror = null;
      this.onshow = null;
      nativeNotify(title, options.body);
    }
    ShimNotification.prototype.close = function () {
      if (typeof this.onclose === "function") this.onclose();
    };
    ShimNotification.prototype.addEventListener = function () {};
    ShimNotification.prototype.removeEventListener = function () {};
    ShimNotification.permission = "granted";
    ShimNotification.requestPermission = function (cb) {
      if (typeof cb === "function") cb("granted");
      return Promise.resolve("granted");
    };
    window.Notification = ShimNotification;
  } catch (e) {}

  // 2b) Service-worker notification path. Modern WhatsApp Web also raises notifications
  //     via ServiceWorkerRegistration.showNotification (e.g. when the tab is backgrounded),
  //     which the window.Notification shim above does NOT intercept — so those toasts never
  //     reached the OS. Override the page-side prototype method to forward through the same
  //     native `notify` command, and report an empty list from getNotifications (we render a
  //     native toast, so there is no in-page Notification object to hand back). We can only
  //     reach the page's prototype here; notifications fired from inside the service worker's
  //     own context are out of scope for a page-injected script.
  try {
    var SWR = window.ServiceWorkerRegistration;
    if (SWR && SWR.prototype && typeof SWR.prototype.showNotification === "function") {
      SWR.prototype.showNotification = function (title, options) {
        options = options || {};
        // Route through nativeNotify so the service-worker path shares the same de-dup
        // window as window.Notification (an alert that fires on both paths shows once).
        nativeNotify(title, options.body);
        // Real API resolves Promise<undefined>; match it so callers awaiting it don't break.
        return Promise.resolve();
      };
      if (typeof SWR.prototype.getNotifications === "function") {
        SWR.prototype.getNotifications = function () {
          return Promise.resolve([]);
        };
      }
    }
  } catch (e) {}

  // 3) Unread count — forward the raw <title> string on change; Rust parses it.
  var lastTitle = "";
  function report() {
    if (document.title === lastTitle) return;
    lastTitle = document.title;
    invoke("set_unread", { title: document.title });
  }
  function start() {
    try {
      var el = document.querySelector("title");
      if (el) {
        new MutationObserver(report).observe(el, {
          childList: true,
          characterData: true,
          subtree: true,
        });
      }
      setInterval(report, 2000); // fallback if <title> node is swapped
      report();
    } catch (e) {}
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }

  // 4) Drag-and-drop file injection.
  //    The native webview never delivers an OS file drop into this page on any platform
  //    (Tauri's drop handler consumes it), so Rust captures the drop (window.rs
  //    `register_drop_handler`) and STREAMS the file bytes here through
  //    `__whatrustDropFeed` as begin/chunk/end/commit messages keyed by a drop id.
  //    Chunked transport (vs the old single giant eval) keeps peak memory at one
  //    ~5.6 MB base64 chunk per step and stays far below WebView2's cross-process
  //    message ceiling; each chunk is standalone base64 (Rust emits multiple-of-3
  //    byte chunks) decoded to bytes on arrival.
  //
  //    WhatsApp Web keeps a sticker-creator <input type=file> (image-only accept) ALWAYS
  //    mounted, but mounts the real "Photos & videos" input (accept includes video) and
  //    the "Document" input (accept "*") only when the attach (+) menu is opened. Targeting
  //    a file input blindly therefore lands on the sticker input — which turns photos into
  //    stickers and rejects video/documents. So we ROUTE BY TYPE: open the attach menu to
  //    mount the right input, then set its .files and fire change (React's onChange fires
  //    for synthetic bubbling events; it doesn't check isTrusted, and input.files is
  //    settable in WebKit). Images + native videos -> media input; everything else (zip,
  //    pdf, docs, webm/mkv/avi) -> document input. We never use the sticker input.
  //
  //    A MIXED drop (media + documents) routes EVERYTHING through the Document input:
  //    one composer per drop, and the document input accepts every file — a photo sent
  //    as a file still arrives, whereas the old "media wins" policy silently discarded
  //    the documents. Drops are queued and injected ONE AT A TIME (a later drop waits
  //    for the earlier composer), so two quick drops can't overwrite each other's
  //    input.files. There is no content dedupe: distinct same-named files both attach;
  //    replays are impossible because every drop carries a unique Rust-side id.
  try {
    var drop = {};
    drop.log = function (m) {
      try { console.log("[whatRust drop] " + m); } catch (e) {}
      try { invoke("dlog", { msg: String(m).slice(0, 280) }); } catch (e) {}
    };
    drop.b64ToBytes = function (b64) {
      var bin = atob(b64),
        n = bin.length,
        u = new Uint8Array(n);
      for (var i = 0; i < n; i++) u[i] = bin.charCodeAt(i);
      return u;
    };
    drop.dataTransfer = function (files) {
      var dt = new DataTransfer();
      for (var i = 0; i < files.length; i++) dt.items.add(files[i]);
      return dt;
    };
    // Only these video types are accepted by WhatsApp's Photos & Videos input; other
    // video containers (webm/mkv/avi) are rejected there and must go as a document.
    drop.NATIVE_VIDEO = { "video/mp4": 1, "video/3gpp": 1, "video/quicktime": 1 };
    drop.isMedia = function (type) {
      return /^image\//.test(type || "") || drop.NATIVE_VIDEO[type] === 1;
    };
    drop.qs = function (sels) {
      for (var i = 0; i < sels.length; i++) {
        var e = document.querySelector(sels[i]);
        if (e) return e;
      }
      return null;
    };
    // The media input is the only file input whose accept lists a video type; the sticker
    // input is image-only, so it can never match this. isConnected guards against a
    // stale input left over from a torn-down composer.
    drop.findMediaInput = function () {
      var ins = document.querySelectorAll('input[type="file"]');
      for (var i = 0; i < ins.length; i++) {
        if (ins[i].isConnected !== false && /video/i.test(ins[i].accept || "")) return ins[i];
      }
      return null;
    };
    // The document input accepts everything (accept "*"/""/no image+video). The sticker
    // input (image-only) and media input (has video) are both excluded.
    drop.findDocInput = function () {
      var ins = document.querySelectorAll('input[type="file"]');
      for (var i = 0; i < ins.length; i++) {
        if (ins[i].isConnected === false) continue;
        var a = (ins[i].accept || "").trim();
        if (a === "" || a === "*" || a === "*/*") return ins[i];
        if (!/image/i.test(a) && !/video/i.test(a)) return ins[i];
      }
      return null;
    };
    drop.openMenu = function () {
      var b = drop.qs([
        '[data-icon="plus"]',
        '[data-icon="attach-menu-plus"]',
        '[data-icon="clip"]',
        '[data-testid="clip"]',
        'button[title="Attach"]',
        '[aria-label="Attach"]',
        '[title="Attach"]',
      ]);
      if (!b) return false;
      (b.closest('button,[role="button"],div[role="button"]') || b).click();
      return true;
    };
    drop.clickMenuItem = function (kind) {
      var sels =
        kind === "media"
          ? ['[data-testid="attach-image"]', '[data-icon="media-multiple"]', '[aria-label*="Photos"]', '[aria-label*="hoto"]']
          : ['[data-testid="attach-document"]', '[data-icon="document"]', '[aria-label*="Document"]', '[aria-label*="ocument"]'];
      var e = drop.qs(sels);
      if (!e) return false;
      (e.closest('li,button,[role="button"],div[role="button"]') || e).click();
      return true;
    };
    drop.poll = function (fn, ms) {
      return new Promise(function (resolve) {
        var t0 = Date.now();
        (function p() {
          var r = fn();
          if (r) return resolve(r);
          if (Date.now() - t0 >= ms) return resolve(null);
          setTimeout(p, 60);
        })();
      });
    };
    drop.inject = function (input, files) {
      var dt = drop.dataTransfer(files);
      try {
        input.files = dt.files; // settable in WebKit + Blink (WHATWG html#2861)
      } catch (e) {
        drop.log("input.files assign threw: " + e);
        return false;
      }
      input.dispatchEvent(new Event("change", { bubbles: true }));
      input.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    };
    // Open the attach menu (which mounts the lazily-rendered inputs) and inject into the
    // one matching `kind`. If opening the menu alone doesn't mount it, click the matching
    // submenu item to force it. No user gesture exists here, so the OS file picker can't
    // open — only the input element is mounted, which is all we need.
    drop.mountAndInject = function (kind, files) {
      var find = kind === "media" ? drop.findMediaInput : drop.findDocInput;
      var existing = find();
      if (existing) {
        drop.log(kind + " input already present");
        return Promise.resolve(drop.inject(existing, files));
      }
      drop.openMenu();
      return drop.poll(find, 1000).then(function (input) {
        if (input) return drop.inject(input, files);
        drop.clickMenuItem(kind);
        return drop.poll(find, 1000).then(function (input2) {
          if (input2) return drop.inject(input2, files);
          drop.log(kind + " input NOT found after opening attach menu");
          return false;
        });
      });
    };
    // WhatsApp opens a media/document preview composer once a file is attached; used both
    // as the success signal and to hold the NEXT queued drop until the current composer
    // is closed. Best-effort selectors across WA Web versions.
    drop.composerOpen = function () {
      return !!(
        document.querySelector('[data-testid="media-caption-input-container"]') ||
        document.querySelector('[data-testid="media-editor"]') ||
        document.querySelector('[data-animate-modal-body="true"]') ||
        document.querySelector('span[data-icon="send"]') ||
        document.querySelector('span[data-icon="media-cancel"]')
      );
    };
    drop.waitFor = function (pred, ms) {
      return new Promise(function (resolve) {
        var t0 = Date.now();
        (function poll() {
          if (pred()) return resolve(true);
          if (Date.now() - t0 >= ms) return resolve(false);
          setTimeout(poll, 80);
        })();
      });
    };

    // --- chunked receive state ---
    // pending[dropId] = { files: { idx: {name,type,size,parts:[Uint8Array],got} } }
    drop.pending = Object.create(null);
    // Discard a stream that never commits (e.g. Rust died mid-drop) after 60s.
    drop.touch = function (st) {
      if (st.gc) clearTimeout(st.gc);
      st.gc = setTimeout(function () {
        delete drop.pending[st.id];
        drop.log("drop #" + st.id + " expired uncommitted");
      }, 60000);
    };
    drop.state = function (id) {
      var st = drop.pending[id];
      if (!st) {
        st = drop.pending[id] = { id: id, files: Object.create(null) };
      }
      drop.touch(st);
      return st;
    };

    // Serialized injection queue: one drop's composer at a time.
    drop.queue = Promise.resolve();
    drop.runQueued = function (job) {
      drop.queue = drop.queue.then(job, job);
    };

    drop.injectBatch = function (files) {
      var media = files.filter(function (f) { return drop.isMedia(f.type); });
      var docs = files.filter(function (f) { return !drop.isMedia(f.type); });
      // One composer per drop. A mixed batch goes WHOLLY through the document
      // input (it accepts everything) so no file is ever silently discarded.
      var kind, batch;
      if (media.length && docs.length) {
        kind = "document";
        batch = files;
        drop.log("mixed drop: routing all " + files.length + " file(s) as documents");
      } else if (media.length) {
        kind = "media";
        batch = media;
      } else {
        kind = "document";
        batch = docs;
      }
      drop.log("drop: injecting " + batch.length + " file(s) via " + kind + " input");
      return drop.mountAndInject(kind, batch).then(function (ok) {
        drop.log(kind + " inject " + (ok ? "dispatched" : "FAILED"));
        return drop.waitFor(drop.composerOpen, 1800).then(function (open) {
          drop.log("composer " + (open ? "opened" : "NOT detected") + " (" + kind + ")");
          if (!open) return;
          // Hold the queue until this composer closes (sent/cancelled), so the
          // next drop doesn't fight it for the attach flow. Bounded wait.
          return drop.waitFor(function () { return !drop.composerOpen(); }, 120000);
        });
      });
    };

    // Rust-side feed. Messages: {op:"begin",drop,file,name,type,size} ->
    // {op:"chunk",drop,file,b64}* -> {op:"end",drop,file} | {op:"abort",drop,file},
    // then one {op:"commit",drop,files}. Returns a string ack for commit (read by
    // eval_with_callback in Rust, proving the handler executed).
    window.__whatrustDropFeed = function (msg) {
      try {
        if (!msg || typeof msg.drop !== "number") return "BADMSG";
        var st = drop.state(msg.drop);
        if (msg.op === "begin") {
          st.files[msg.file] = {
            name: String(msg.name || "file"),
            type: String(msg.type || "application/octet-stream"),
            size: msg.size >>> 0,
            parts: [],
            got: 0,
            done: false,
          };
          return "OK";
        }
        var f = st.files[msg.file];
        if (msg.op === "chunk") {
          if (!f || f.done) return "NOFILE";
          var bytes = drop.b64ToBytes(msg.b64 || "");
          f.parts.push(bytes);
          f.got += bytes.length;
          return "OK";
        }
        if (msg.op === "end") {
          if (f) f.done = true;
          return "OK";
        }
        if (msg.op === "abort") {
          delete st.files[msg.file];
          drop.log("drop #" + st.id + " file " + msg.file + " aborted by sender");
          return "OK";
        }
        if (msg.op === "commit") {
          if (st.gc) clearTimeout(st.gc);
          delete drop.pending[st.id];
          var files = [];
          for (var k in st.files) {
            var e = st.files[k];
            if (!e.done || e.got !== e.size) {
              drop.log("drop #" + st.id + " file " + k + " incomplete (" + e.got + "/" + e.size + "), skipped");
              continue;
            }
            files.push(new File(e.parts, e.name, { type: e.type }));
            e.parts = null; // release chunk references once the File owns the data
          }
          if (files.length === 0) return "EMPTY";
          drop.runQueued(function () {
            return drop.injectBatch(files).catch(function (e) {
              drop.log("inject error: " + e);
            });
          });
          return "QUEUED:" + files.length;
        }
        return "BADOP";
      } catch (e) {
        drop.log("feed error: " + e);
        return "ERR";
      }
    };
    drop.log("drop injector v3 (chunked, queued, no-loss routing) ready");
  } catch (e) {}
})();
