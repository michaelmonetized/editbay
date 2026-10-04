use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    pub widget: Color32,
    pub hover: Color32,
    pub canvas: Color32,
    pub foreground: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub dark: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self::parse("background='#1e1e2e'\nforeground='#cdd6f4'\naccent='#89b4fa'").unwrap()
    }
}

impl Palette {
    /// Derive quiet application surfaces from an Omarchy color file.
    /// `text` contains desktop key/value colors. Returns no palette when its
    /// required background or foreground is invalid, retaining the last good theme.
    pub fn parse(text: &str) -> Option<Self> {
        let colors: HashMap<_, _> = text
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once('=')?;
                let value = value.trim().trim_matches(['\'', '"']);
                let value = value.strip_prefix('#')?;
                let rgb = u32::from_str_radix(value.get(..6)?, 16).ok()?;
                Some((
                    key.trim(),
                    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
                ))
            })
            .collect();
        let get = |keys: &[&str]| keys.iter().find_map(|key| colors.get(key).copied());
        let background = get(&["background", "base", "bg"])?;
        let foreground = get(&["foreground", "text", "fg"])?;
        let accent = get(&["accent", "blue", "color4"]).unwrap_or(foreground);
        let dark = 0.2126 * f32::from(background.r())
            + 0.7152 * f32::from(background.g())
            + 0.0722 * f32::from(background.b())
            < 140.0;
        let toward = if dark { Color32::WHITE } else { Color32::BLACK };
        let panel = mix(
            background,
            get(&["dark_background", "mantle", "surface0"])
                .unwrap_or(mix(background, toward, 0.04)),
            0.45,
        );
        let widget = mix(
            background,
            get(&["lighter_background", "surface1"]).unwrap_or(mix(
                background,
                toward,
                if dark { 0.10 } else { 0.08 },
            )),
            0.55,
        );
        let extreme = get(&["darker_background", "crust"]).unwrap_or(mix(
            background,
            if dark { Color32::BLACK } else { Color32::WHITE },
            if dark { 0.25 } else { 0.12 },
        ));
        Some(Self {
            background,
            panel,
            widget,
            hover: mix(widget, toward, 0.08),
            canvas: mix(background, extreme, 0.6),
            foreground,
            muted: mix(foreground, background, 0.38),
            border: mix(panel, foreground, 0.10),
            accent,
            error: get(&["red", "color1"]).unwrap_or(Color32::from_rgb(243, 139, 168)),
            warning: get(&["yellow", "orange", "color3"])
                .unwrap_or(Color32::from_rgb(249, 226, 175)),
            dark,
        })
    }

    /// Apply the native hierarchy shared with Omadesign.
    /// `ctx` receives palette, compact spacing and typography. Returns no value;
    /// font bytes are installed separately after their background lookup completes.
    pub fn apply(self, ctx: &egui::Context) {
        let mut visuals = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.override_text_color = Some(self.foreground);
        visuals.weak_text_color = Some(self.muted);
        visuals.window_fill = self.background;
        visuals.panel_fill = self.panel;
        visuals.extreme_bg_color = self.canvas;
        visuals.text_edit_bg_color = Some(self.canvas);
        visuals.faint_bg_color = self.widget;
        visuals.window_stroke = Stroke::new(1., self.border);
        visuals.window_corner_radius = egui::CornerRadius::same(10);
        visuals.selection.bg_fill =
            Color32::from_rgba_unmultiplied(self.accent.r(), self.accent.g(), self.accent.b(), 25);
        visuals.selection.stroke = Stroke::new(1., self.accent);
        visuals.hyperlink_color = self.accent;
        visuals.warn_fg_color = self.warning;
        visuals.error_fg_color = self.error;
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.open,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.noninteractive,
        ] {
            widget.bg_fill = self.widget;
            widget.weak_bg_fill = self.widget;
            widget.bg_stroke = Stroke::NONE;
            widget.fg_stroke = Stroke::new(1., self.foreground);
            widget.corner_radius = egui::CornerRadius::same(6);
        }
        visuals.widgets.hovered.bg_fill = self.hover;
        visuals.widgets.active.bg_fill = mix(self.widget, self.accent, 0.18);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., self.border);
        ctx.set_theme(if self.dark {
            egui::ThemePreference::Dark
        } else {
            egui::ThemePreference::Light
        });
        ctx.set_visuals(visuals);
        ctx.style_mut_of(
            if self.dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
            |style| {
                style.animation_time = 0.10;
                style.spacing.item_spacing = egui::vec2(6., 6.);
                style.spacing.button_padding = egui::vec2(9., 5.);
                style.spacing.interact_size = egui::vec2(26., 26.);
                style.spacing.icon_width = 14.;
                style.spacing.indent = 12.;
                style.spacing.scroll = egui::style::ScrollStyle::thin();
                for (kind, size, family) in [
                    (TextStyle::Heading, 16., FontFamily::Proportional),
                    (TextStyle::Body, 13., FontFamily::Proportional),
                    (TextStyle::Button, 12.5, FontFamily::Proportional),
                    (TextStyle::Small, 11., FontFamily::Proportional),
                    (TextStyle::Monospace, 12., FontFamily::Monospace),
                ] {
                    style.text_styles.insert(kind, FontId::new(size, family));
                }
            },
        );
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
    }
}

fn mix(a: Color32, b: Color32, amount: f32) -> Color32 {
    let component = |a: u8, b: u8| (f32::from(a) * (1. - amount) + f32::from(b) * amount) as u8;
    Color32::from_rgb(
        component(a.r(), b.r()),
        component(a.g(), b.g()),
        component(a.b(), b.b()),
    )
}

struct PreparedTheme {
    palette: Option<Palette>,
    font: Option<(PathBuf, Vec<u8>)>,
}

pub struct LiveTheme {
    pub palette: Palette,
    pub font_path: Option<PathBuf>,
    home: PathBuf,
    colors: PathBuf,
    next_poll: Instant,
    pending: Option<Receiver<PreparedTheme>>,
}

impl LiveTheme {
    /// Watch the desktop without reading files or running commands in a UI frame.
    /// `home` selects Omarchy settings; `ctx` receives the initial fallback and
    /// background wakeups. Returns an independently owned theme watcher.
    pub fn new(home: PathBuf, ctx: &egui::Context) -> Self {
        let palette = Palette::default();
        palette.apply(ctx);
        install_fonts(ctx, None);
        let colors = std::env::var_os("EDITBAY_QA_THEME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state/omarchy/current/theme/colors.toml"));
        Self {
            palette,
            font_path: None,
            home,
            colors,
            next_poll: Instant::now(),
            pending: None,
        }
    }

    /// Accept ready theme data and schedule the next background lookup.
    /// `ctx` is the current native context. Returns immediately; invalid color
    /// files keep the previous palette and unchanged fonts are not reinstalled.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(update) => {
                    self.pending = None;
                    if let Some(palette) = update.palette
                        && palette != self.palette
                    {
                        self.palette = palette;
                        palette.apply(ctx);
                    }
                    if let Some((path, bytes)) = update.font
                        && self.font_path.as_ref() != Some(&path)
                    {
                        self.font_path = Some(path);
                        install_fonts(ctx, Some(bytes));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let now = Instant::now();
        if now >= self.next_poll && self.pending.is_none() {
            self.next_poll = now + Duration::from_secs(2);
            let (sender, receiver) = mpsc::channel();
            let colors = self.colors.clone();
            let home = self.home.clone();
            let wake = ctx.clone();
            match std::thread::Builder::new()
                .name("editbay-theme".into())
                .spawn(move || {
                    let palette = fs::read_to_string(colors)
                        .ok()
                        .and_then(|text| Palette::parse(&text));
                    let font = font_file(&home)
                        .and_then(|path| fs::read(&path).ok().map(|bytes| (path, bytes)));
                    let _ = sender.send(PreparedTheme { palette, font });
                    wake.request_repaint();
                }) {
                Ok(_) => self.pending = Some(receiver),
                Err(error) => eprintln!("editbay: theme worker: {error}"),
            }
        }
        ctx.request_repaint_after(self.next_poll.saturating_duration_since(now));
    }
}

fn font_file(home: &Path) -> Option<PathBuf> {
    let output = Command::new("omarchy")
        .args(["font", "current"])
        .env("HOME", home)
        .output()
        .ok();
    let named = output
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "sans-serif".into());
    let output = Command::new("fc-match")
        .args(["-f", "%{file}", &named])
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    (output.status.success()
        && fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 32 * 1024 * 1024))
    .then_some(path)
}

fn install_fonts(ctx: &egui::Context, bytes: Option<Vec<u8>>) {
    let mut fonts = FontDefinitions::default();
    if let Some(bytes) = bytes {
        fonts
            .font_data
            .insert("desktop".into(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "desktop".into());
        }
    }
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/phosphor/Phosphor-Light.ttf"
        ))),
    );
    fonts
        .families
        .insert(FontFamily::Name("phosphor".into()), vec!["phosphor".into()]);
    ctx.set_fonts(fonts);
}
