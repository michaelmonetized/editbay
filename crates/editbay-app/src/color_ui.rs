use editbay_core::{ColorConfiguration, OutputTransfer, WorkingGamut};
use editbay_render::scopes::{
    ScopeGrid, ScopeIdentity, ScopeRange, ScopeReport, ScopeScale, ScopeSettings, ScopeUnit,
};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Rect, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, pos2, vec2,
};

const TINTS: [Color32; 5] = [
    Color32::from_rgb(140, 235, 210),
    Color32::from_rgb(255, 115, 115),
    Color32::from_rgb(115, 245, 155),
    Color32::from_rgb(125, 165, 255),
    Color32::from_rgb(220, 220, 245),
];

#[derive(PartialEq)]
struct CacheKey {
    identity: ScopeIdentity,
    dimensions: [u32; 2],
    color: ColorConfiguration,
    settings: ScopeSettings,
}

struct CachedTextures {
    key: CacheKey,
    textures: Vec<TextureHandle>,
}

/// Native color-workspace scopes with one bounded accepted-frame texture cache.
#[derive(Default)]
pub struct ScopePanel {
    pub enabled: bool,
    pub settings: ScopeSettings,
    cached: Option<CachedTextures>,
}

impl ScopePanel {
    /// Draw scopes from a report already accepted by the program-frame owner.
    /// `ui` supplies native drawing and `report` supplies real captured pixels'
    /// numeric results. Returns true when enabled state or scope settings change;
    /// the caller must restart its asynchronous capture with those new settings.
    /// Reports for previous settings are hidden while the next capture completes.
    pub fn show(&mut self, ui: &mut Ui, report: Option<&ScopeReport>) -> bool {
        let before = (self.enabled, self.settings);
        ui.horizontal(|ui| {
            ui.heading("Color");
            ui.checkbox(&mut self.enabled, "Live scopes");
        });
        ui.add_enabled_ui(self.enabled, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Levels");
                egui::ComboBox::from_id_salt("scope-level-units")
                    .selected_text(scale_label(self.settings.scale))
                    .show_ui(ui, |ui| {
                        for scale in [ScopeScale::OutputCode, ScopeScale::Linear, ScopeScale::Nits] {
                            ui.selectable_value(
                                &mut self.settings.scale,
                                scale,
                                scale_label(scale),
                            );
                        }
                    });
                ui.label("Sample limit");
                egui::ComboBox::from_id_salt("scope-sample-limit")
                    .selected_text(format!("{} pixels", self.settings.maximum_samples))
                    .show_ui(ui, |ui| {
                        for samples in [16_384, 65_536, 262_144] {
                            ui.selectable_value(
                                &mut self.settings.maximum_samples,
                                samples,
                                format!("{samples} pixels"),
                            );
                        }
                    });
            });
            ui.collapsing("Luminance settings", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Reference white");
                    ui.add(
                        egui::DragValue::new(&mut self.settings.reference_white_nits)
                            .range(1.0..=self.settings.peak_nits)
                            .speed(1.0)
                            .suffix(" nits"),
                    );
                    ui.label("Peak");
                    ui.add(
                        egui::DragValue::new(&mut self.settings.peak_nits)
                            .range(self.settings.reference_white_nits.max(100.0)..=10_000.0)
                            .speed(10.0)
                            .suffix(" nits"),
                    );
                });
                ui.weak("Nits follow this explicit reference white. Peak sets the nits plot and HLG reference display.");
            });
        });
        let changed = before != (self.enabled, self.settings);
        if !self.enabled {
            self.cached = None;
            ui.weak("Enable live scopes to inspect the program frame.");
            return changed;
        }
        let Some(report) = report.filter(|report| report.settings == self.settings) else {
            self.cached = None;
            ui.weak("Waiting for the program frame…");
            return changed;
        };
        if !self.prepare(ui.ctx(), report) {
            self.cached = None;
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Scope plot exceeds its size limit",
            );
            return changed;
        }
        let textures = &self.cached.as_ref().unwrap().textures;
        egui::ScrollArea::vertical()
            .id_salt("color-scopes-content")
            .show(ui, |ui| {
                ui.label(format!(
                    "Program frame {} · revision {} · {} × {}",
                    report.identity.frame,
                    report.identity.version.revision,
                    report.dimensions[0],
                    report.dimensions[1]
                ));
                ui.label(format!(
                    "Working: linear {} · Display: {} {}",
                    gamut_label(report.color.working_gamut),
                    gamut_label(report.color.display_gamut),
                    transfer_label(report.color.display_transfer)
                ));
                ui.label(format!(
                    "Output signal: {} {} · Waveform/parade: {} {}",
                    gamut_label(report.vectorscope_gamut),
                    transfer_label(report.vectorscope_transfer),
                    gamut_label(report.signal_gamut),
                    transfer_label(report.signal_transfer)
                ));
                ui.weak(format!(
                    "Sampled {} / {} pixels · {} valid · {} transparent · {} invalid",
                    report.sampled_pixels,
                    u64::from(report.dimensions[0]) * u64::from(report.dimensions[1]),
                    report.valid_pixels,
                    report.transparent_pixels,
                    report.invalid_pixels
                ));
                ui.separator();
                draw_scope(ui, &textures[0], &report.waveform, "Waveform", 190.0, false);
                if ui.available_width() >= 600.0 {
                    ui.columns(3, |columns| {
                        for (channel, column) in columns.iter_mut().enumerate() {
                            draw_scope(
                                column,
                                &textures[channel + 1],
                                &report.parade[channel],
                                ["Red parade", "Green parade", "Blue parade"][channel],
                                160.0,
                                false,
                            );
                        }
                    });
                } else {
                    for channel in 0..3 {
                        draw_scope(
                            ui,
                            &textures[channel + 1],
                            &report.parade[channel],
                            ["Red parade", "Green parade", "Blue parade"][channel],
                            130.0,
                            false,
                        );
                    }
                }
                if ui.available_width() >= 620.0 {
                    ui.columns(2, |columns| {
                        draw_scope(
                            &mut columns[0],
                            &textures[4],
                            &report.vectorscope,
                            "Vectorscope",
                            260.0,
                            true,
                        );
                        draw_statistics(&mut columns[1], report);
                    });
                } else {
                    draw_scope(
                        ui,
                        &textures[4],
                        &report.vectorscope,
                        "Vectorscope",
                        260.0,
                        true,
                    );
                    draw_statistics(ui, report);
                }
                ui.collapsing("Signal details", |ui| {
                    ui.label(format!(
                        "Reference white: {:.1} nits · peak: {:.1} nits",
                        report.settings.reference_white_nits, report.settings.peak_nits
                    ));
                    ui.label("Partial alpha is unpremultiplied; fully transparent pixels are excluded.");
                    ui.label("Output code values are calculated from the declared output gamut and transfer.");
                    if matches!(
                        report.vectorscope_transfer,
                        OutputTransfer::Pq | OutputTransfer::Hlg
                    ) {
                        ui.label("HDR code analysis floors negative light; linear and nits levels retain negative values.");
                        if report.vectorscope_transfer == OutputTransfer::Hlg {
                            ui.label("HLG uses the selected peak, zero black and the reference viewing environment.");
                        }
                    }
                    ui.label("Range flags count sampled levels. They do not determine whether an earlier stage clipped.");
                    ui.label("Plot edges retain values outside the visible axes. Numeric ranges remain unclamped.");
                    ui.label(format!(
                        "Output code below 0 (R/G/B): {} · above 1: {}",
                        channel_counts(report.clipping.output_below_zero_channels),
                        channel_counts(report.clipping.output_above_one_channels)
                    ));
                    ui.label(format!(
                        "Off-plot samples: waveform {} · parade {} / {} / {} · vectorscope {}",
                        report.waveform.outside,
                        report.parade[0].outside,
                        report.parade[1].outside,
                        report.parade[2].outside,
                        report.vectorscope.outside
                    ));
                    egui::Grid::new("scope-output-ranges")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Output code");
                            ui.strong("Minimum");
                            ui.strong("Maximum");
                            ui.end_row();
                            for channel in 0..3 {
                                range_row(
                                    ui,
                                    ["Red", "Green", "Blue"][channel],
                                    report.statistics.output_rgb[channel],
                                );
                            }
                        });
                });
            });
        changed
    }

    /// Refresh only a changed accepted-frame cache using bounded plot images.
    /// `ctx` owns textures and `report` supplies the captured scope grids. Returns
    /// false for malformed or oversized plots; one frame's five textures remain.
    fn prepare(&mut self, ctx: &egui::Context, report: &ScopeReport) -> bool {
        let grids = [
            &report.waveform,
            &report.parade[0],
            &report.parade[1],
            &report.parade[2],
            &report.vectorscope,
        ];
        if grids.iter().any(|grid| {
            grid.width == 0
                || grid.height == 0
                || grid.width > 512
                || grid.height > 512
                || grid.counts.len() != grid.width as usize * grid.height as usize
        }) {
            return false;
        }
        let key = CacheKey {
            identity: report.identity,
            dimensions: report.dimensions,
            color: report.color,
            settings: report.settings,
        };
        if self.cached.as_ref().is_some_and(|cached| cached.key == key) {
            return true;
        }
        if let Some(cached) = &mut self.cached {
            for (index, grid) in grids.into_iter().enumerate() {
                cached.textures[index].set(heatmap(grid, TINTS[index]), TextureOptions::NEAREST);
            }
            cached.key = key;
        } else {
            self.cached = Some(CachedTextures {
                key,
                textures: grids
                    .into_iter()
                    .enumerate()
                    .map(|(index, grid)| {
                        ctx.load_texture(
                            format!("editbay-scope-{index}"),
                            heatmap(grid, TINTS[index]),
                            TextureOptions::NEAREST,
                        )
                    })
                    .collect(),
            });
        }
        true
    }
}

/// Convert a validated density grid into an upright logarithmic heatmap.
/// `grid` contains row-zero minimum values and `tint` identifies its channel.
/// Returns one opaque plot image with black empty bins and bright occupied bins.
fn heatmap(grid: &ScopeGrid, tint: Color32) -> ColorImage {
    let maximum = f64::from(grid.counts.iter().copied().max().unwrap_or(0)).ln_1p();
    let mut pixels = Vec::with_capacity(grid.counts.len());
    for row in (0..grid.height as usize).rev() {
        for &count in &grid.counts[row * grid.width as usize..(row + 1) * grid.width as usize] {
            if count == 0 {
                pixels.push(Color32::BLACK);
            } else {
                let intensity = 0.2 + 0.8 * f64::from(count).ln_1p() / maximum;
                pixels.push(Color32::from_rgb(
                    (f64::from(tint.r()) * intensity) as u8,
                    (f64::from(tint.g()) * intensity) as u8,
                    (f64::from(tint.b()) * intensity) as u8,
                ));
            }
        }
    }
    ColorImage::new([grid.width as usize, grid.height as usize], pixels)
}

/// Paint a cached scope image and its declared numeric axes.
/// `ui`, `texture`, `grid`, `title`, `height` and `vector` define the chart.
/// Returns no value; vectors use a square Cb/Cr graticule with a zero crosshair.
fn draw_scope(
    ui: &mut Ui,
    texture: &TextureHandle,
    grid: &ScopeGrid,
    title: &str,
    height: f32,
    vector: bool,
) {
    let width = ui.available_width();
    if width < 100.0 {
        ui.weak("Widen the color panel to view scopes.");
        return;
    }
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if !rect.intersects(ui.clip_rect()) {
        return;
    }
    let painter = ui.painter();
    let mut plot = Rect::from_min_max(rect.min + vec2(50.0, 25.0), rect.max - vec2(8.0, 24.0));
    if vector {
        let side = plot.width().min(plot.height());
        plot = Rect::from_center_size(plot.center(), vec2(side, side));
    }
    painter.text(
        rect.min + vec2(0.0, 3.0),
        Align2::LEFT_TOP,
        format!("{title} · {}", unit_label(grid.vertical.unit)),
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    painter.image(
        texture.id(),
        plot,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    let line = Stroke::new(1.0, Color32::from_white_alpha(35));
    let muted = ui.visuals().weak_text_color();
    for index in 0..=4 {
        let fraction = index as f32 / 4.0;
        let y = plot.top() + fraction * plot.height();
        let value = grid.vertical.maximum
            - f64::from(fraction) * (grid.vertical.maximum - grid.vertical.minimum);
        painter.hline(plot.x_range(), y, line);
        painter.text(
            pos2(plot.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            level_label(value),
            FontId::monospace(10.0),
            muted,
        );
    }
    for (fraction, align) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        let x = plot.left() + fraction * plot.width();
        let value = grid.horizontal.minimum
            + f64::from(fraction) * (grid.horizontal.maximum - grid.horizontal.minimum);
        painter.vline(x, plot.y_range(), line);
        painter.text(
            pos2(x, plot.bottom() + 4.0),
            align,
            format!(
                "{} {}",
                level_label(value),
                unit_label(grid.horizontal.unit)
            ),
            FontId::monospace(9.0),
            muted,
        );
    }
    if vector {
        painter.circle_stroke(plot.center(), plot.width() * 0.5, line);
        painter.hline(plot.x_range(), plot.center().y, line);
        painter.vline(plot.center().x, plot.y_range(), line);
    }
    painter.rect_stroke(plot, 0.0, line, egui::StrokeKind::Inside);
    response.on_hover_text(format!(
        "Density from the captured program frame; {} sampled values lie outside these axes and remain in edge bins.",
        grid.outside
    ));
}

/// Draw unclamped working-signal ranges and sampled range flags.
/// `ui` owns layout and `report` supplies accepted measurements. Returns no value.
fn draw_statistics(ui: &mut Ui, report: &ScopeReport) {
    ui.strong("Sampled working levels");
    match report.statistics.luminance_nits {
        Some(range) => {
            ui.label(format!(
                "Luminance: {}–{} nits",
                level_label(range.minimum),
                level_label(range.maximum)
            ));
        }
        None => {
            ui.weak("No visible valid pixels in this sample.");
        }
    }
    egui::Grid::new("scope-working-ranges")
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Linear");
            ui.strong("Minimum");
            ui.strong("Maximum");
            ui.end_row();
            for channel in 0..3 {
                range_row(
                    ui,
                    ["Red", "Green", "Blue"][channel],
                    report.statistics.linear_rgb[channel],
                );
            }
        });
    ui.add_space(8.0);
    ui.strong("Sampled range flags · R / G / B");
    for (label, counts) in [
        ("Below zero", report.clipping.negative_channels),
        (
            "Above white",
            report.clipping.above_reference_white_channels,
        ),
        ("Above peak", report.clipping.above_peak_channels),
    ] {
        ui.label(format!("{label}: {}", channel_counts(counts)));
    }
    ui.weak(format!(
        "White {:.1} nits · peak {:.1} nits",
        report.settings.reference_white_nits, report.settings.peak_nits
    ));
}

/// Append one measured range to a three-column table.
/// `ui`, `label` and `range` define the row. Returns no value; absent data is marked.
fn range_row(ui: &mut Ui, label: &str, range: Option<ScopeRange>) {
    ui.label(label);
    ui.monospace(range.map_or_else(|| "—".into(), |range| level_label(range.minimum)));
    ui.monospace(range.map_or_else(|| "—".into(), |range| level_label(range.maximum)));
    ui.end_row();
}

/// Format three sampled channel counters in fixed RGB order.
/// `counts` holds red, green and blue counts. Returns a compact display string.
fn channel_counts(counts: [u64; 3]) -> String {
    format!("{} / {} / {}", counts[0], counts[1], counts[2])
}

/// Format a finite scope level without hiding useful fractional code values.
/// `value` uses the chart's declared units. Returns a compact numeric label.
fn level_label(value: f64) -> String {
    if value.abs() >= 1000.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.3}")
    }
}

/// Name a scope plot's selected scale.
/// `scale` selects the interpretation. Returns the native control's label.
fn scale_label(scale: ScopeScale) -> &'static str {
    match scale {
        ScopeScale::Linear => "Working linear",
        ScopeScale::Nits => "Nits",
        ScopeScale::OutputCode => "Output code",
    }
}

/// Name the units recorded on a numeric axis.
/// `unit` declares the quantity. Returns a short visible unit label.
fn unit_label(unit: ScopeUnit) -> &'static str {
    match unit {
        ScopeUnit::ImageColumn => "px",
        ScopeUnit::RelativeLinear => "linear",
        ScopeUnit::Nits => "nits",
        ScopeUnit::NormalizedCode => "code",
        ScopeUnit::Cb => "Cb",
        ScopeUnit::Cr => "Cr",
    }
}

/// Name declared RGB primaries.
/// `gamut` selects the color space. Returns its visible standard name.
fn gamut_label(gamut: WorkingGamut) -> &'static str {
    match gamut {
        WorkingGamut::Bt709 => "BT.709",
        WorkingGamut::Bt2020 => "BT.2020",
        WorkingGamut::DisplayP3 => "Display P3",
    }
}

/// Name a declared transfer function without inferring display support.
/// `transfer` selects the encoding. Returns its visible standard name.
fn transfer_label(transfer: OutputTransfer) -> &'static str {
    match transfer {
        OutputTransfer::Linear => "linear",
        OutputTransfer::Srgb => "sRGB",
        OutputTransfer::Bt709 => "BT.709",
        OutputTransfer::Pq => "PQ",
        OutputTransfer::Hlg => "HLG",
    }
}
