use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub autostart: bool,
    pub hotkey_enabled: bool,
    pub hotkey: String,
    pub notifications: bool,
    /// Display zoom for the WhatsApp webview, 1.0 = the site's own sizing.
    pub zoom: f64,
    pub oled_theme: bool,
    pub chat_background: String,
    pub chat_background_color: String,
    pub chat_background_image: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_minimized: false,
            autostart: false,
            hotkey_enabled: true,
            hotkey: "CmdOrCtrl+Shift+W".to_string(),
            notifications: true,
            zoom: 1.0,
            oled_theme: true,
            chat_background: "pure-black".to_string(),
            chat_background_color: "#000000".to_string(),
            chat_background_image: String::new(),
        }
    }
}

/// Zoom bounds. The settings UI only offers four presets inside this range; the
/// clamp exists so a hand-edited settings.json cannot leave the app at a zoom
/// level from which the settings window is unreadable.
pub const ZOOM_MIN: f64 = 0.5;
pub const ZOOM_MAX: f64 = 2.0;
pub const MAX_CHAT_BACKGROUND_IMAGE_CHARS: usize = 2_200_000;

/// Bring a zoom factor into range. A non-finite value (a `null` or a string in
/// the JSON deserialises to the default, but arithmetic in an older build could
/// still have written a NaN) falls back to 1.0 rather than to a bound.
pub fn sanitize_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_MIN, ZOOM_MAX)
    } else {
        1.0
    }
}

pub fn sanitize_chat_background(value: &str) -> String {
    match value {
        "pure-black" | "graphite-grid" | "emerald-glow" | "custom-color" | "custom-image" => {
            value.to_string()
        }
        _ => "pure-black".to_string(),
    }
}

pub fn sanitize_chat_background_color(value: &str) -> String {
    let valid = value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if valid {
        value.to_ascii_uppercase()
    } else {
        "#000000".to_string()
    }
}

pub fn sanitize_chat_background_image(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let supported = [
        "data:image/jpeg;base64,",
        "data:image/png;base64,",
        "data:image/webp;base64,",
    ];
    let Some(prefix) = supported.iter().find(|prefix| value.starts_with(**prefix)) else {
        return String::new();
    };
    let payload = &value[prefix.len()..];
    let valid_payload = !payload.is_empty()
        && payload
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
    if value.len() <= MAX_CHAT_BACKGROUND_IMAGE_CHARS && valid_payload {
        value.to_string()
    } else {
        String::new()
    }
}

impl Settings {
    /// Repair values an older or hand-edited settings.json can carry.
    pub fn sanitized(mut self) -> Self {
        self.zoom = sanitize_zoom(self.zoom);
        self.chat_background = sanitize_chat_background(&self.chat_background);
        self.chat_background_color = sanitize_chat_background_color(&self.chat_background_color);
        self.chat_background_image = sanitize_chat_background_image(&self.chat_background_image);
        if self.chat_background == "custom-image" && self.chat_background_image.is_empty() {
            self.chat_background = "pure-black".to_string();
        }
        self
    }
}

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

fn settings_path(app: &AppHandle) -> tauri::Result<PathBuf> {
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| crate::persist::load_json::<Settings>(&p))
        .map(Settings::sanitized)
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, s: &Settings) -> tauri::Result<()> {
    let path = settings_path(app)?;
    crate::persist::save_json(&path, s)?;
    Ok(())
}

/// Apply side effects of settings (autostart + global shortcut). Returns the global-
/// shortcut registration error as `Some(msg)` if registering failed; `None` if it
/// registered successfully, or the shortcut is disabled/empty, or on non-desktop.
pub fn apply(app: &AppHandle, s: &Settings) -> Option<String> {
    // Zoom is a webview property, so it applies on every platform and takes
    // effect on the open account windows without a reload.
    crate::window::apply_zoom_all(app, s.zoom);

    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autostart = app.autolaunch();
        if s.autostart {
            let _ = autostart.enable();
        } else {
            let _ = autostart.disable();
        }

        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let gs = app.global_shortcut();
        let _ = gs.unregister_all();
        if s.hotkey_enabled && !s.hotkey.trim().is_empty() {
            return match gs.register(s.hotkey.as_str()) {
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            };
        }
        None
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, s);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        sanitize_chat_background, sanitize_chat_background_color, sanitize_chat_background_image,
        sanitize_zoom, Settings, ZOOM_MAX, ZOOM_MIN,
    };

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert!(s.close_to_tray);
        assert!(s.notifications);
        assert_eq!(s.hotkey, "CmdOrCtrl+Shift+W");
        assert!(!s.autostart);
        assert_eq!(s.zoom, 1.0);
        assert!(s.oled_theme);
        assert_eq!(s.chat_background, "pure-black");
        assert_eq!(s.chat_background_color, "#000000");
        assert!(s.chat_background_image.is_empty());
    }

    #[test]
    fn zoom_presets_survive_sanitizing() {
        for z in [0.75, 0.85, 1.0, 1.15] {
            assert_eq!(sanitize_zoom(z), z);
        }
    }

    #[test]
    fn out_of_range_zoom_is_clamped() {
        assert_eq!(sanitize_zoom(0.01), ZOOM_MIN);
        assert_eq!(sanitize_zoom(9.0), ZOOM_MAX);
        assert_eq!(sanitize_zoom(-1.0), ZOOM_MIN);
    }

    #[test]
    fn non_finite_zoom_falls_back_to_unscaled() {
        assert_eq!(sanitize_zoom(f64::NAN), 1.0);
        assert_eq!(sanitize_zoom(f64::INFINITY), 1.0);
    }

    #[test]
    fn zoom_from_json_is_sanitized() {
        let s: Settings = serde_json::from_str(r#"{"zoom": 12}"#).unwrap();
        assert_eq!(s.sanitized().zoom, ZOOM_MAX);
        // Absent in a settings.json written by an older build.
        let s: Settings = serde_json::from_str(r#"{"autostart": true}"#).unwrap();
        assert_eq!(s.sanitized().zoom, 1.0);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"autostart": true}"#).unwrap();
        assert!(s.autostart);
        assert!(s.close_to_tray);
        assert_eq!(s.hotkey, "CmdOrCtrl+Shift+W");
    }

    #[test]
    fn empty_json_is_all_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn roundtrip() {
        let s = Settings {
            autostart: true,
            hotkey: "Ctrl+Alt+W".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn appearance_values_are_restricted_to_safe_css_inputs() {
        assert_eq!(sanitize_chat_background("emerald-glow"), "emerald-glow");
        assert_eq!(
            sanitize_chat_background("url(javascript:alert(1))"),
            "pure-black"
        );
        assert_eq!(sanitize_chat_background_color("#1a2B3c"), "#1A2B3C");
        assert_eq!(
            sanitize_chat_background_color("red; color: white"),
            "#000000"
        );
    }

    #[test]
    fn custom_wallpaper_accepts_only_bounded_image_data_urls() {
        let valid = "data:image/webp;base64,UklGRg==";
        assert_eq!(sanitize_chat_background_image(valid), valid);
        assert!(sanitize_chat_background_image("https://example.com/a.png").is_empty());
        assert!(sanitize_chat_background_image("data:text/html;base64,PHNjcmlwdD4=").is_empty());
        assert!(sanitize_chat_background_image("data:image/png;base64,not base64").is_empty());
        let oversized = format!("data:image/jpeg;base64,{}", "A".repeat(2_200_001));
        assert!(sanitize_chat_background_image(&oversized).is_empty());
    }
}
