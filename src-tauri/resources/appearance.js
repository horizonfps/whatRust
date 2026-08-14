(function () {
  "use strict";
  if (window.location.origin !== "https://web.whatsapp.com") return;

  var STYLE_ID = "whatrust-hrz-appearance";
  var BACKGROUNDS = {
    "pure-black": true,
    "graphite-grid": true,
    "emerald-glow": true,
    "custom-color": true,
    "custom-image": true,
  };

  var OLED_CSS = [
    ":root{color-scheme:dark;--hrz-chat-base:#000;--hrz-chat-wallpaper:none}",
    "html[data-whatrust-oled='true']{",
    "--background-default:#050505!important;--background-default-hover:#101211!important;",
    "--background-default-active:#151816!important;--background-secondary:#090a0a!important;",
    "--background-secondary-alt:#0d0f0e!important;--background-panel-header:#080909!important;",
    "--panel-background:#050505!important;--panel-background-colored:#07110e!important;",
    "--conversation-panel-background:#000!important;--compose-panel-background:#080a09!important;",
    "--rich-text-panel-background:#101311!important;--incoming-background:#111412!important;",
    "--incoming-background-deeper:#171b18!important;--outgoing-background:#09251c!important;",
    "--outgoing-background-deeper:#0b3024!important;--primary:#f2f7f4!important;",
    "--secondary:#8b9991!important;--primary-strong:#12d991!important;",
    "--icon:#8e9c94!important;--icon-lighter:#68756e!important;--border-strong:#1d2420!important;",
    "--border-list:#121714!important;--unread-marker-background:#12d991!important}",
    "html[data-whatrust-oled='true'],html[data-whatrust-oled='true'] body,",
    "html[data-whatrust-oled='true'] #app{background:#000!important}",
    "html[data-whatrust-oled='true'] header,html[data-whatrust-oled='true'] footer{",
    "box-shadow:none!important;border-color:#151b17!important}",
    "html[data-whatrust-oled='true'] [data-testid='cell-frame-container']{",
    "border-bottom-color:#101411!important}",
    "html[data-whatrust-oled='true'] [data-testid='msg-container']{filter:saturate(.86)}",
    "html[data-whatrust-oled='true'] ::-webkit-scrollbar{width:7px;height:7px}",
    "html[data-whatrust-oled='true'] ::-webkit-scrollbar-track{background:#050505}",
    "html[data-whatrust-oled='true'] ::-webkit-scrollbar-thumb{background:#263029;border-radius:999px}",
    ".whatrust-chat-surface,html[data-whatrust-background] [data-testid='conversation-panel-body'],",
    "html[data-whatrust-background] [data-testid='conversation-panel-messages'],",
    "html[data-whatrust-background] main [role='application']{",
    "background-color:var(--hrz-chat-base)!important;background-image:var(--hrz-chat-wallpaper)!important;",
    "background-position:center!important;background-repeat:repeat!important;background-size:auto!important}",
    "html[data-whatrust-background='custom-image'] .whatrust-chat-surface,",
    "html[data-whatrust-background='custom-image'] [data-testid='conversation-panel-body'],",
    "html[data-whatrust-background='custom-image'] [data-testid='conversation-panel-messages'],",
    "html[data-whatrust-background='custom-image'] main [role='application']{",
    "background-position:center!important;background-repeat:no-repeat!important;background-size:cover!important}",
    ".whatrust-chat-surface::before,html[data-whatrust-background] [data-testid='conversation-panel-body']::before{",
    "background-image:none!important;opacity:0!important}",
  ].join("");

  function safeColor(value) {
    value = String(value || "").toUpperCase();
    return /^#[0-9A-F]{6}$/.test(value) ? value : "#000000";
  }

  function safeImage(value) {
    value = String(value || "");
    if (value.length > 2200000) return "";
    return /^data:image\/(?:jpeg|png|webp);base64,[A-Za-z0-9+/=]+$/.test(value) ? value : "";
  }

  function ensureStyle() {
    var style = document.getElementById && document.getElementById(STYLE_ID);
    if (style) return style;
    style = document.createElement("style");
    style.id = STYLE_ID;
    style.textContent = OLED_CSS;
    (document.head || document.documentElement).appendChild(style);
    return style;
  }

  function markChatSurfaces() {
    if (!document.querySelectorAll) return;
    var surfaces = document.querySelectorAll(
      "[data-testid='conversation-panel-body'],[data-testid='conversation-panel-messages'],main [role='application']"
    );
    for (var i = 0; i < surfaces.length; i++) {
      if (surfaces[i].classList) surfaces[i].classList.add("whatrust-chat-surface");
    }
  }

  function apply(config) {
    config = config || {};
    ensureStyle();
    var root = document.documentElement;
    var background = BACKGROUNDS[config.background] ? config.background : "pure-black";
    var color = safeColor(config.color);
    var image = safeImage(config.image);
    if (background === "custom-image" && !image) background = "pure-black";

    root.dataset.whatrustOled = config.oled === false ? "false" : "true";
    root.dataset.whatrustBackground = background;
    root.style.setProperty("--hrz-chat-base", background === "custom-color" ? color : "#000000");

    var wallpaper = "none";
    if (background === "graphite-grid") {
      wallpaper = "linear-gradient(rgba(32,43,37,.26) 1px,transparent 1px),linear-gradient(90deg,rgba(32,43,37,.26) 1px,transparent 1px)";
      root.style.setProperty("--hrz-chat-base", "#020303");
    } else if (background === "emerald-glow") {
      wallpaper = "radial-gradient(circle at 72% 18%,rgba(18,217,145,.12),transparent 34%),radial-gradient(circle at 18% 82%,rgba(8,88,60,.16),transparent 38%)";
      root.style.setProperty("--hrz-chat-base", "#000000");
    } else if (background === "custom-image") {
      wallpaper = "linear-gradient(rgba(0,0,0,.58),rgba(0,0,0,.58)),url(\"" + image + "\")";
    }
    root.style.setProperty("--hrz-chat-wallpaper", wallpaper);
    markChatSurfaces();
    return "OK";
  }

  window.__whatrustApplyAppearance = apply;
  apply(window.__whatrustInitialAppearance || {});

  try {
    var observer = new MutationObserver(markChatSurfaces);
    observer.observe(document.documentElement, { childList: true, subtree: true });
  } catch (e) {}
})();
