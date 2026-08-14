import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "appearance.js"), "utf8");
let failures = 0;

function assert(condition, message) {
  if (condition) console.log("ok   " + message);
  else {
    failures++;
    console.error("FAIL " + message);
  }
}

function harness(initial) {
  const properties = new Map();
  const children = [];
  const navigations = [];
  const root = {
    dataset: {},
    style: { setProperty: (name, value) => properties.set(name, value) },
    appendChild: (child) => children.push(child),
  };
  const surface = { classList: { add: (name) => (surface.className = name) } };
  const createElement = (tag) => ({
    tag,
    id: "",
    textContent: "",
    type: "",
    title: "",
    ariaLabel: "",
    addEventListener: (name, callback) => {
      if (name === "click") createElement.lastClick = callback;
    },
  });
  const document = {
    documentElement: root,
    head: root,
    body: root,
    createElement,
    getElementById: (id) => children.find((child) => child.id === id) || null,
    querySelectorAll: () => [surface],
  };
  const window = {
    location: {
      origin: "https://web.whatsapp.com",
      assign: (url) => navigations.push(url),
    },
    __whatrustInitialAppearance: initial,
  };
  const MutationObserver = class {
    observe() {}
  };
  new Function("window", "document", "MutationObserver", source)(
    window,
    document,
    MutationObserver
  );
  return { window, root, properties, children, surface, navigations, createElement };
}

function earlyDomHarness(initial) {
  const properties = new Map();
  const children = [];
  const listeners = new Map();
  const root = {
    dataset: {},
    style: { setProperty: (name, value) => properties.set(name, value) },
    appendChild: (child) => children.push(child),
  };
  const document = {
    documentElement: null,
    head: null,
    body: null,
    createElement: (tag) => ({ tag, id: "", textContent: "", addEventListener() {} }),
    getElementById: (id) => children.find((child) => child.id === id) || null,
    querySelectorAll: () => [],
    addEventListener: (name, callback) => listeners.set(name, callback),
  };
  const window = {
    location: { origin: "https://web.whatsapp.com" },
    __whatrustInitialAppearance: initial,
    __TAURI__: { core: { invoke() {} } },
  };
  const MutationObserver = class {
    observe() {}
  };
  let error = null;
  try {
    new Function("window", "document", "MutationObserver", source)(
      window,
      document,
      MutationObserver
    );
  } catch (reason) {
    error = reason;
  }
  document.documentElement = root;
  document.head = root;
  document.body = root;
  listeners.get("DOMContentLoaded")?.();
  return { error, listeners, root, properties, children };
}

console.log("document-start injection waits for the DOM root");
{
  const h = earlyDomHarness({ oled: true, background: "pure-black", color: "#000000", image: "" });
  assert(h.error === null, "early injection does not throw");
  assert(h.listeners.has("DOMContentLoaded"), "appearance waits for DOMContentLoaded");
  assert(h.root.dataset.whatrustOled === "true", "deferred appearance is applied");
}

console.log("initial OLED appearance is applied before the page renders");
{
  const h = harness({ oled: true, background: "custom-color", color: "#102030", image: "" });
  assert(typeof h.window.__whatrustApplyAppearance === "function", "live appearance hook exists");
  assert(h.root.dataset.whatrustOled === "true", "OLED marker enabled");
  assert(h.root.dataset.whatrustBackground === "custom-color", "background marker applied");
  assert(h.properties.get("--hrz-chat-base") === "#102030", "custom color applied");
  const style = h.children.find((child) => child.id === "whatrust-hrz-appearance");
  assert(style?.textContent.includes("--background-default:#050505"), "legacy OLED variables installed");
  assert(style?.textContent.includes("--WDS-surface-default:#050505"), "current WhatsApp OLED variables installed");
  assert(h.surface.className === "whatrust-chat-surface", "conversation surface tagged");
  const settingsButton = h.children.find((child) => child.id === "whatrust-settings-button");
  assert(!!settingsButton, "HRZ appearance button installed");
  h.createElement.lastClick?.({ preventDefault() {}, stopPropagation() {} });
  assert(h.navigations.includes("whatrust://settings/appearance"), "appearance button sends the trusted settings navigation");
}

console.log("live presets update without a page reload");
{
  const h = harness({ oled: true, background: "pure-black", color: "#000000", image: "" });
  const result = h.window.__whatrustApplyAppearance({
    oled: true,
    background: "emerald-glow",
    color: "#FFFFFF",
    image: "",
  });
  assert(result === "OK", "live update acknowledged");
  assert(h.root.dataset.whatrustBackground === "emerald-glow", "preset switched");
  assert(h.properties.get("--hrz-chat-wallpaper").includes("radial-gradient"), "emerald texture installed");
  assert(h.children.filter((child) => child.id === "whatrust-hrz-appearance").length === 1, "stylesheet not duplicated");
}

console.log("unsafe or absent custom images fall back to pure black");
{
  const h = harness({
    oled: true,
    background: "custom-image",
    color: "#000000",
    image: "javascript:alert(1)",
  });
  assert(h.root.dataset.whatrustBackground === "pure-black", "unsafe image rejected");
  assert(h.properties.get("--hrz-chat-wallpaper") === "none", "rejected image is not emitted to CSS");
}

if (failures) process.exit(1);
console.log("\nall appearance tests passed");
