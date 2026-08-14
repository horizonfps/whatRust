import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "appearance.js"), "utf8");
new Function(source)();

let failures = 0;
function check(condition, message) {
  if (condition) console.log("ok   " + message);
  else {
    failures++;
    console.error("FAIL " + message);
  }
}

const fit = globalThis.AppearanceFmt.fitDimensions;
check(JSON.stringify(fit(3840, 2160)) === JSON.stringify({ width: 1920, height: 1080 }), "4K wallpaper scales to 1080p");
check(JSON.stringify(fit(800, 600)) === JSON.stringify({ width: 800, height: 600 }), "small wallpaper is not enlarged");
check(JSON.stringify(fit(1000, 3000)) === JSON.stringify({ width: 360, height: 1080 }), "portrait aspect ratio is preserved");
check(globalThis.AppearanceFmt.validateFile({ type: "image/webp", size: 1024 }) === "", "WebP is accepted");
check(globalThis.AppearanceFmt.validateFile({ type: "image/gif", size: 1024 }).includes("JPEG"), "animated and unsupported formats are rejected");
check(globalThis.AppearanceFmt.validateFile({ type: "image/png", size: 13 * 1024 * 1024 }).includes("12 MB"), "oversized sources are rejected");

if (failures) process.exit(1);
console.log("\nall appearance helper tests passed");
