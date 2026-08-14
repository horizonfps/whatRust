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
  const root = {
    dataset: {},
    style: { setProperty: (name, value) => properties.set(name, value) },
    appendChild: (child) => children.push(child),
  };
  const surface = { classList: { add: (name) => (surface.className = name) } };
  const document = {
    documentElement: root,
    head: root,
    createElement: (tag) => ({ tag, id: "", textContent: "" }),
    getElementById: (id) => children.find((child) => child.id === id) || null,
    querySelectorAll: () => [surface],
  };
  const window = {
    location: { origin: "https://web.whatsapp.com" },
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
  return { window, root, properties, children, surface };
}

console.log("initial OLED appearance is applied before the page renders");
{
  const h = harness({ oled: true, background: "custom-color", color: "#102030", image: "" });
  assert(typeof h.window.__whatrustApplyAppearance === "function", "live appearance hook exists");
  assert(h.root.dataset.whatrustOled === "true", "OLED marker enabled");
  assert(h.root.dataset.whatrustBackground === "custom-color", "background marker applied");
  assert(h.properties.get("--hrz-chat-base") === "#102030", "custom color applied");
  assert(h.children.length === 1 && h.children[0].textContent.includes("--background-default:#050505"), "OLED stylesheet installed once");
  assert(h.surface.className === "whatrust-chat-surface", "conversation surface tagged");
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
  assert(h.children.length === 1, "stylesheet not duplicated");
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
