//! Follows the Omarchy theme live: colors from colors.toml, corners from Hyprland's rounding.
//! Without Omarchy it falls back to egui's dark theme with a yellow accent.

use eframe::egui::{self, Color32, CornerRadius, Visuals};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const RECHECK: Duration = Duration::from_secs(2);

pub struct Theme {
    path: PathBuf,
    applied: Option<String>,
    checked: Instant,
    radius: u8,
}

impl Theme {
    pub fn new() -> Self {
        Self {
            path: crate::backend::state_home().join("omarchy/current/theme/colors.toml"),
            applied: None,
            checked: Instant::now(),
            radius: hypr_rounding(),
        }
    }

    /// Re-reads colors.toml every couple of seconds; a theme switch swaps the file, so polling beats a file watcher here.
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.applied.is_some() && self.checked.elapsed() < RECHECK {
            return;
        }
        self.checked = Instant::now();
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        if self.applied.as_ref() != Some(&text) {
            ctx.set_visuals(visuals(&text, self.radius));
            self.applied = Some(text);
        }
        ctx.request_repaint_after(RECHECK);
    }
}

/// Uses the desktop's interface font (Adwaita Sans on GNOME and Omarchy) ahead of egui's
/// own, which stays as the fallback for symbols, and sets larger text sizes than egui's.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = desktop_font().and_then(|path| std::fs::read(path).ok()) {
        fonts.font_data.insert("desktop".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "desktop".into());
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily::*, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, Proportional)),
            (TextStyle::Body, FontId::new(15.0, Proportional)),
            (TextStyle::Button, FontId::new(15.0, Proportional)),
            (TextStyle::Heading, FontId::new(21.0, Proportional)),
            (TextStyle::Monospace, FontId::new(14.0, Monospace)),
        ]
        .into();
    });
}

/// The font file for the desktop's interface font, from GNOME's setting or fontconfig's sans-serif.
fn desktop_font() -> Option<String> {
    let setting = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "font-name"]).output().ok();
    let family = setting.map(|o| font_family(&String::from_utf8_lossy(&o.stdout))).filter(|f| !f.is_empty()).unwrap_or("sans-serif".into());
    let out = std::process::Command::new("fc-match").arg(&family).arg("--format=%{file}").output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// "'Adwaita Sans 11'" -> "Adwaita Sans"
fn font_family(setting: &str) -> String {
    let name = setting.trim().trim_matches('\'');
    match name.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f32>().is_ok() => family.to_string(),
        _ => name.to_string(),
    }
}

fn hypr_rounding() -> u8 {
    let out = std::process::Command::new("hyprctl").args(["getoption", "decoration:rounding", "-j"]).output();
    out.ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["int"].as_u64())
        .map_or(0, |r| r.min(24) as u8)
}

fn color(toml: &str, name: &str) -> Option<Color32> {
    toml.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        if k.trim() != name {
            return None;
        }
        Color32::from_hex(v.trim().trim_matches('"')).ok()
    })
}

fn visuals(toml: &str, radius: u8) -> Visuals {
    let light = toml.lines().any(|l| l.replace(' ', "") == "mode=\"light\"");
    let mut v = if light { Visuals::light() } else { Visuals::dark() };
    let c = |name| color(toml, name);
    let accent = c("accent").unwrap_or(Color32::from_rgb(0xFF, 0xD6, 0x0A));
    v.selection.bg_fill = accent;
    v.selection.stroke.color = c("foreground").unwrap_or(v.selection.stroke.color);
    v.hyperlink_color = accent;
    v.widgets.hovered.bg_stroke.color = accent;
    v.widgets.active.bg_stroke.color = accent;
    if let Some(bg) = c("background") {
        v.panel_fill = bg;
        v.window_fill = bg;
    }
    if let Some(dark) = c("dark_background") {
        v.extreme_bg_color = dark;
    }
    if let Some(lighter) = c("lighter_background") {
        v.faint_bg_color = lighter;
        v.widgets.inactive.bg_fill = lighter;
        v.widgets.inactive.weak_bg_fill = lighter;
    }
    if let Some(sel) = c("selection") {
        v.widgets.hovered.bg_fill = sel;
        v.widgets.hovered.weak_bg_fill = sel;
    }
    if let Some(fg) = c("foreground") {
        v.override_text_color = Some(fg);
    }
    v.error_fg_color = c("red").unwrap_or(v.error_fg_color);
    v.warn_fg_color = c("yellow").unwrap_or(v.warn_fg_color);
    let small = CornerRadius::same(radius.min(4));
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = small;
    }
    v.window_corner_radius = CornerRadius::same(radius);
    v.menu_corner_radius = small;
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_desktop_font_setting() {
        assert_eq!(super::font_family("'Adwaita Sans 11'\n"), "Adwaita Sans");
        assert_eq!(super::font_family("'Cantarell'"), "Cantarell");
        assert_eq!(super::font_family(""), "");
    }

    #[test]
    fn reads_omarchy_colors() {
        let toml = "mode = \"dark\"\naccent = \"#0A84FF\"\nbackground = \"#1E1E1E\"\n";
        assert_eq!(super::color(toml, "accent"), Some(super::Color32::from_rgb(0x0A, 0x84, 0xFF)));
        assert_eq!(super::color(toml, "red"), None);
        assert_eq!(super::visuals(toml, 12).panel_fill, super::Color32::from_rgb(0x1E, 0x1E, 0x1E));
    }
}
