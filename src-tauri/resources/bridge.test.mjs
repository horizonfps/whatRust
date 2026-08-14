// Zero-dependency behavioral tests for bridge.js's drag-drop injector
// (the chunked __whatrustDropFeed protocol). Run: node src-tauri/resources/bridge.test.mjs
// Exits nonzero on failure. Same standalone pattern as settings-ui/comboToAccelerator.test.mjs.
//
// The harness stubs the minimal DOM surface bridge.js touches, evals the real
// script, then drives the feed exactly as window.rs does (begin/chunk/end/commit)
// and asserts which fake <input type=file> received which File objects.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const bridgeSrc = readFileSync(join(here, "bridge.js"), "utf8");

let failures = 0;
function assert(cond, msg) {
  if (cond) {
    console.log("  ok  " + msg);
  } else {
    failures++;
    console.error("FAIL  " + msg);
  }
}
const b64 = (s) => Buffer.from(s).toString("base64");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// --- minimal DOM stubs -------------------------------------------------------
class FakeFile {
  constructor(parts, name, opts) {
    this.name = name;
    this.type = (opts && opts.type) || "";
    let size = 0;
    for (const p of parts) size += p.length ?? 0;
    this.size = size;
    this.parts = parts;
  }
  bytes() {
    const out = Buffer.concat(this.parts.map((p) => Buffer.from(p)));
    return out;
  }
}
class FakeDataTransfer {
  constructor() {
    this._files = [];
    this.items = { add: (f) => this._files.push(f) };
  }
  get files() {
    return this._files.slice();
  }
}
class FakeInput {
  constructor(accept) {
    this.accept = accept;
    this.isConnected = true;
    this.files = [];
    this.batches = []; // one entry per change event: [FakeFile, ...]
  }
  dispatchEvent(ev) {
    if (ev.type === "change") {
      this.batches.push(this.files.slice());
      if (this.onchange) this.onchange();
    }
    return true;
  }
}

function makeHarness({ session = new Map() } = {}) {
  const mediaInput = new FakeInput("image/*,video/mp4,video/3gpp,video/quicktime");
  const docInput = new FakeInput("*");
  const logs = [];
  const invokes = [];
  const listeners = Object.create(null);
  let reloads = 0;
  const body = {
    children: [],
    appendChild(element) {
      element.parentNode = this;
      this.children.push(element);
      return element;
    },
    removeChild(element) {
      this.children = this.children.filter((child) => child !== element);
      element.parentNode = null;
    },
  };
  // Simulate WhatsApp's composer lifecycle: it opens right after an input change
  // and "the user sends" ~80ms later. This exercises injectBatch's real waits
  // (composer detected, then queue held until it closes) without long timeouts.
  const state = { composerUntil: 0 };
  const onChange = () => {
    state.composerUntil = Date.now() + 80;
  };
  mediaInput.onchange = onChange;
  docInput.onchange = onChange;
  const document = {
    title: "WhatsApp",
    readyState: "complete",
    body,
    documentElement: body,
    addEventListener() {},
    createElement() {
      return { id: "", textContent: "", style: {}, parentNode: null, setAttribute() {} };
    },
    getElementById(id) {
      return body.children.find((child) => child.id === id) || null;
    },
    querySelectorAll(sel) {
      return sel === 'input[type="file"]' ? [mediaInput, docInput] : [];
    },
    querySelector(sel) {
      if (sel === "title") return null;
      if (sel.includes("media-caption-input-container")) {
        return Date.now() < state.composerUntil ? {} : null;
      }
      return null;
    },
  };
  const window = {
    location: {
      origin: "https://web.whatsapp.com",
      reload() {
        reloads++;
      },
    },
    sessionStorage: {
      getItem: (key) => session.get(key) ?? null,
      setItem: (key, value) => session.set(key, String(value)),
    },
    __TAURI__: {
      core: {
        invoke(cmd, args) {
          invokes.push({ cmd, args });
          return Promise.resolve();
        },
      },
    },
    addEventListener(name, listener) {
      (listeners[name] ||= []).push(listener);
    },
  };
  const sandboxGlobals = {
    window,
    document,
    navigator: {},
    console: { log: (m) => logs.push(String(m)), error() {} },
    File: FakeFile,
    DataTransfer: FakeDataTransfer,
    Event: class {
      constructor(type) {
        this.type = type;
      }
    },
    MutationObserver: class {
      observe() {}
    },
    atob: (s) => Buffer.from(s, "base64").toString("binary"),
    // unref'd, so bridge.js's 60s stream-GC timer cannot hold node open after the
    // last assertion. The timers still fire — the tests themselves await ref'd
    // sleeps, which keep the loop alive for as long as anything is being checked.
    setTimeout: (fn, ms) => {
      const t = setTimeout(fn, ms);
      if (t && typeof t.unref === "function") t.unref();
      return t;
    },
    clearTimeout,
    setInterval: () => 0,
  };
  // Evaluate bridge.js with our stubs shadowing the real globals.
  const params = Object.keys(sandboxGlobals);
  const fn = new Function(...params, `"use strict";\n${bridgeSrc}`);
  fn(...params.map((k) => sandboxGlobals[k]));
  return {
    window,
    mediaInput,
    docInput,
    logs,
    invokes,
    emit(name, event) {
      for (const listener of listeners[name] || []) listener(event);
    },
    reloadCount: () => reloads,
    recoveryBanners: () => body.children.filter((child) => child.id === "whatrust-chat-recovery"),
  };
}

// Drive the feed the way window.rs stream_drop does.
function feedFile(w, dropId, idx, name, type, content, { chunks = 1 } = {}) {
  const buf = Buffer.from(content);
  w.__whatrustDropFeed({ op: "begin", drop: dropId, file: idx, name, type, size: buf.length });
  const per = Math.max(1, Math.ceil(buf.length / chunks));
  for (let off = 0; off < buf.length || (buf.length === 0 && off === 0); off += per) {
    const slice = buf.subarray(off, Math.min(off + per, buf.length));
    w.__whatrustDropFeed({ op: "chunk", drop: dropId, file: idx, b64: slice.toString("base64") });
    if (buf.length === 0) break;
  }
  w.__whatrustDropFeed({ op: "end", drop: dropId, file: idx });
}

// --- tests -------------------------------------------------------------------
async function testChatTableDesyncRecoversOnce() {
  console.log("chat-table LID failure triggers one visible recovery");
  const h = makeHarness();
  h.emit("unhandledrejection", { reason: new Error("Lid is missing in chat table") });
  await sleep(350);
  assert(h.reloadCount() === 1, "page reloaded after the LID failure");
  assert(
    h.invokes.some((call) => call.cmd === "notify" && /sincroniza/i.test(call.args.body)),
    "native recovery notice dispatched"
  );
  assert(h.recoveryBanners().some((banner) => /sincroniza/i.test(banner.textContent)), "in-page recovery notice shown");
  h.emit("unhandledrejection", { reason: new Error("Lid is missing in chat table") });
  await sleep(350);
  assert(h.reloadCount() === 1, "duplicate failure does not create a reload loop");
}

async function testChatTableRecoveryCooldownSurvivesReload() {
  console.log("chat-table recovery cooldown survives a page reload");
  const session = new Map();
  const first = makeHarness({ session });
  first.emit("unhandledrejection", { reason: new Error("Failed to find row in chat table") });
  await sleep(350);
  assert(first.reloadCount() === 1, "alternate chat-table failure triggers recovery");

  const second = makeHarness({ session });
  second.emit("unhandledrejection", { reason: new Error("Lid is missing in chat table") });
  await sleep(350);
  assert(second.reloadCount() === 0, "fresh page respects the recovery cooldown");
  assert(second.recoveryBanners().some((banner) => /continua/i.test(banner.textContent)), "persistent failure remains visible");
}

async function testMixedDropLosesNothing() {
  console.log("mixed drop routes everything as documents (no silent loss)");
  const { window: w, mediaInput, docInput } = makeHarness();
  feedFile(w, 1, 0, "photo.jpg", "image/jpeg", "jpegbytes");
  feedFile(w, 1, 1, "report.pdf", "application/pdf", "pdfbytes");
  const ack = w.__whatrustDropFeed({ op: "commit", drop: 1, files: 2 });
  assert(ack === "QUEUED:2", `commit acks both files (got ${ack})`);
  await sleep(30);
  assert(mediaInput.batches.length === 0, "media input untouched for a mixed drop");
  assert(docInput.batches.length === 1, "document input received one batch");
  const names = (docInput.batches[0] || []).map((f) => f.name).sort();
  assert(
    JSON.stringify(names) === JSON.stringify(["photo.jpg", "report.pdf"]),
    `both files attached (got ${names})`
  );
}

async function testPureMediaGoesToMediaInput() {
  console.log("pure media drop routes to the Photos & Videos input");
  const { window: w, mediaInput, docInput } = makeHarness();
  feedFile(w, 2, 0, "clip.mp4", "video/mp4", "mp4bytes");
  w.__whatrustDropFeed({ op: "commit", drop: 2, files: 1 });
  await sleep(30);
  assert(mediaInput.batches.length === 1, "media input received the video");
  assert(docInput.batches.length === 0, "document input untouched");
}

async function testDistinctSameNamedFilesBothAttach() {
  console.log("distinct files sharing a name + header prefix both attach");
  const { window: w, mediaInput } = makeHarness();
  const header = "A".repeat(64); // identical first 18+ bytes — old dedupe dropped one
  feedFile(w, 3, 0, "same.mp4", "video/mp4", header + "-first");
  feedFile(w, 3, 1, "same.mp4", "video/mp4", header + "-second");
  const ack = w.__whatrustDropFeed({ op: "commit", drop: 3, files: 2 });
  assert(ack === "QUEUED:2", `both distinct files queued (got ${ack})`);
  await sleep(30);
  assert(
    mediaInput.batches.length === 1 && mediaInput.batches[0].length === 2,
    "one change event carrying both files"
  );
}

async function testChunkReassemblyByteExact() {
  console.log("multi-chunk payload reassembles byte-for-byte");
  const { window: w, mediaInput } = makeHarness();
  const content = Buffer.alloc(10_000);
  for (let i = 0; i < content.length; i++) content[i] = (i * 31 + 7) & 0xff;
  feedFile(w, 4, 0, "big.mp4", "video/mp4", content, { chunks: 7 });
  w.__whatrustDropFeed({ op: "commit", drop: 4, files: 1 });
  await sleep(30);
  const file = mediaInput.batches[0] && mediaInput.batches[0][0];
  assert(!!file, "file arrived");
  assert(file && file.size === content.length, "size preserved across chunks");
  assert(file && file.bytes().equals(content), "bytes identical after reassembly");
}

async function testIncompleteFileSkippedAbortHonored() {
  console.log("incomplete/aborted streams never produce a corrupt File");
  const { window: w, mediaInput, docInput } = makeHarness();
  // File 0: declared 100 bytes but only 10 sent (Rust died mid-stream) — no end op.
  w.__whatrustDropFeed({ op: "begin", drop: 5, file: 0, name: "trunc.mp4", type: "video/mp4", size: 100 });
  w.__whatrustDropFeed({ op: "chunk", drop: 5, file: 0, b64: b64("tenbytes!!") });
  // File 1: explicitly aborted by the sender.
  feedFile(w, 5, 1, "aborted.pdf", "application/pdf", "partial");
  w.__whatrustDropFeed({ op: "abort", drop: 5, file: 1 });
  // File 2: healthy.
  feedFile(w, 5, 2, "good.pdf", "application/pdf", "gooddata");
  const ack = w.__whatrustDropFeed({ op: "commit", drop: 5, files: 3 });
  assert(ack === "QUEUED:1", `only the complete file is queued (got ${ack})`);
  await sleep(30);
  const all = [...mediaInput.batches.flat(), ...docInput.batches.flat()].map((f) => f.name);
  assert(JSON.stringify(all) === JSON.stringify(["good.pdf"]), `only good.pdf attached (got ${all})`);
}

async function testSequentialDropsBothSurvive() {
  console.log("two back-to-back drops are queued, not overwritten");
  const { window: w, mediaInput } = makeHarness();
  feedFile(w, 6, 0, "first.mp4", "video/mp4", "one");
  w.__whatrustDropFeed({ op: "commit", drop: 6, files: 1 });
  feedFile(w, 7, 0, "second.mp4", "video/mp4", "two");
  w.__whatrustDropFeed({ op: "commit", drop: 7, files: 1 });
  // First drop's composer stays "open" ~80ms; the queue must hold drop #7 until
  // it closes, then inject — so wait past both cycles.
  await sleep(400);
  const seen = mediaInput.batches.map((b) => b.map((f) => f.name).join(","));
  assert(
    JSON.stringify(seen) === JSON.stringify(["first.mp4", "second.mp4"]),
    `both drops injected in order (got ${JSON.stringify(seen)})`
  );
}

async function testCommitWithoutDataAcksEmpty() {
  console.log("commit with no completed files acks EMPTY (Rust can toast)");
  const { window: w } = makeHarness();
  const ack = w.__whatrustDropFeed({ op: "commit", drop: 8, files: 0 });
  assert(ack === "EMPTY", `empty commit acks EMPTY (got ${ack})`);
}

async function testMalformedMessagesAreRejected() {
  console.log("malformed feed messages are rejected without throwing");
  const { window: w } = makeHarness();
  assert(w.__whatrustDropFeed(null) === "BADMSG", "null message");
  assert(w.__whatrustDropFeed({ op: "chunk" }) === "BADMSG", "missing drop id");
  assert(
    w.__whatrustDropFeed({ op: "chunk", drop: 9, file: 0, b64: "AA==" }) === "NOFILE",
    "chunk before begin"
  );
  assert(w.__whatrustDropFeed({ op: "wat", drop: 9 }) === "BADOP", "unknown op");
}

const tests = [
  testChatTableDesyncRecoversOnce,
  testChatTableRecoveryCooldownSurvivesReload,
  testMixedDropLosesNothing,
  testPureMediaGoesToMediaInput,
  testDistinctSameNamedFilesBothAttach,
  testChunkReassemblyByteExact,
  testIncompleteFileSkippedAbortHonored,
  testSequentialDropsBothSurvive,
  testCommitWithoutDataAcksEmpty,
  testMalformedMessagesAreRejected,
];

for (const t of tests) {
  await t();
}
if (failures > 0) {
  console.error(`\n${failures} assertion(s) FAILED`);
  process.exit(1);
}
console.log("\nall bridge drop tests passed");
