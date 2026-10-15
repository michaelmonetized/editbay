//! Bounded scopes over the program renderer's premultiplied linear working image.

use editbay_core::{ColorConfiguration, DocumentVersion, OutputTransfer, WorkingGamut};
use serde::Serialize;
use uuid::Uuid;

/// Document and program position captured by the renderer that supplied the pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScopeIdentity {
    pub version: DocumentVersion,
    pub composition: Uuid,
    pub frame: u64,
}

/// Waveform and parade units; vectors always show the declared output signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeScale {
    Linear,
    Nits,
    #[default]
    OutputCode,
}

/// Scope-only luminance interpretation and bounded plot resources.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScopeSettings {
    pub columns: u32,
    pub rows: u32,
    pub vector_size: u32,
    pub maximum_samples: usize,
    pub scale: ScopeScale,
    pub reference_white_nits: f64,
    pub peak_nits: f64,
}

impl Default for ScopeSettings {
    fn default() -> Self {
        Self {
            columns: 128,
            rows: 256,
            vector_size: 128,
            maximum_samples: 65_536,
            scale: ScopeScale::OutputCode,
            reference_white_nits: 100.0,
            peak_nits: 10_000.0,
        }
    }
}

/// Explicit units for a plot axis, including the source image's horizontal position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeUnit {
    ImageColumn,
    RelativeLinear,
    Nits,
    NormalizedCode,
    Cb,
    Cr,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScopeAxis {
    pub minimum: f64,
    pub maximum: f64,
    pub unit: ScopeUnit,
}

/// Row-major density bins; row zero is the vertical axis minimum.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeGrid {
    pub width: u32,
    pub height: u32,
    pub horizontal: ScopeAxis,
    pub vertical: ScopeAxis,
    pub counts: Vec<u32>,
    pub outside: u64,
}

impl ScopeGrid {
    /// Read a density bin without interpreting its axis units.
    /// `column` and `row` address this grid. Returns no value outside its bounds.
    pub fn count(&self, column: u32, row: u32) -> Option<u32> {
        if column >= self.width || row >= self.height {
            return None;
        }
        self.counts
            .get(row as usize * self.width as usize + column as usize)
            .copied()
    }

    /// Allocate a validated density plot.
    /// `width`, `height` and the axes define its geometry. Returns empty bins.
    fn new(width: u32, height: u32, horizontal: ScopeAxis, vertical: ScopeAxis) -> Self {
        Self {
            width,
            height,
            horizontal,
            vertical,
            counts: vec![0; width as usize * height as usize],
            outside: 0,
        }
    }

    /// Accumulate one finite signal sample.
    /// `x` and `y` use the declared axes. Returns no value; off-plot samples
    /// increment `outside` and remain visible in the nearest edge bin.
    fn add(&mut self, x: f64, y: f64) {
        if x < self.horizontal.minimum
            || x > self.horizontal.maximum
            || y < self.vertical.minimum
            || y > self.vertical.maximum
        {
            self.outside += 1;
        }
        let column = bin(x, self.horizontal, self.width);
        let row = bin(y, self.vertical, self.height);
        self.counts[row * self.width as usize + column] += 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScopeRange {
    pub minimum: f64,
    pub maximum: f64,
}

/// Unclamped sampled ranges; linear RGB is straight-alpha in the working gamut.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeStatistics {
    pub linear_rgb: [Option<ScopeRange>; 3],
    pub luminance_nits: Option<ScopeRange>,
    pub output_rgb: [Option<ScopeRange>; 3],
    pub output_luma: Option<ScopeRange>,
    pub chroma: [Option<ScopeRange>; 2],
}

/// Sample counts describe detected levels, not proof that an earlier stage clipped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ScopeClipping {
    pub negative_channels: [u64; 3],
    pub above_reference_white_channels: [u64; 3],
    pub above_peak_channels: [u64; 3],
    pub output_below_zero_channels: [u64; 3],
    pub output_above_one_channels: [u64; 3],
}

/// A scope result retains the full capture identity and declared interpretations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeReport {
    pub identity: ScopeIdentity,
    pub dimensions: [u32; 2],
    pub color: ColorConfiguration,
    pub settings: ScopeSettings,
    pub signal_gamut: WorkingGamut,
    pub signal_transfer: OutputTransfer,
    pub vectorscope_gamut: WorkingGamut,
    pub vectorscope_transfer: OutputTransfer,
    pub sampled_pixels: u64,
    pub valid_pixels: u64,
    pub transparent_pixels: u64,
    pub invalid_pixels: u64,
    pub statistics: ScopeStatistics,
    pub clipping: ScopeClipping,
    pub waveform: ScopeGrid,
    pub parade: [ScopeGrid; 3],
    pub vectorscope: ScopeGrid,
}

impl ScopeReport {
    /// Check a completed scope job against the receiving program request.
    /// `identity`, `color` and `settings` are the current capture. Returns false
    /// for a different project, revision, composition, frame or interpretation.
    pub fn matches(
        &self,
        identity: ScopeIdentity,
        color: ColorConfiguration,
        settings: ScopeSettings,
    ) -> bool {
        self.identity == identity && self.color == color && self.settings == settings
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScopeError {
    #[error("scope frame dimensions must be between 1 and 8192 pixels")]
    InvalidDimensions,
    #[error("scope frame has {actual} channels; expected {expected}")]
    PixelLength { expected: usize, actual: usize },
    #[error("invalid scope settings: {0}")]
    InvalidSettings(&'static str),
}

/// Measure a captured linear program frame without quantizing to display pixels.
/// `pixels` are row-major premultiplied linear RGBA in `color.working_gamut`;
/// `dimensions`, `identity`, `color` and `settings` describe that same capture.
/// Returns bounded deterministic sampled densities and unclamped numeric ranges.
/// The renderer must validate its receipt before supplying these pixels; the
/// receiving worker must still reject stale session/job ownership. Nits use an
/// explicit scope-only reference white, not measured display luminance. Output
/// code scopes simulate the declared output transform without claiming that the
/// renderer or display supports that output. HLG assumes zero black and the
/// reference viewing environment in ITU-R BT.2100, Table 5 and Note 5i.
pub fn sample_program_frame(
    pixels: &[f32],
    dimensions: [u32; 2],
    identity: ScopeIdentity,
    color: ColorConfiguration,
    settings: ScopeSettings,
) -> Result<ScopeReport, ScopeError> {
    let [width, height] = dimensions;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(ScopeError::InvalidDimensions);
    }
    validate(settings)?;
    let pixel_count = width as usize * height as usize;
    let expected = pixel_count * 4;
    if pixels.len() != expected {
        return Err(ScopeError::PixelLength {
            expected,
            actual: pixels.len(),
        });
    }
    let horizontal = ScopeAxis {
        minimum: 0.0,
        maximum: f64::from(width),
        unit: ScopeUnit::ImageColumn,
    };
    let vertical = match settings.scale {
        ScopeScale::Linear => ScopeAxis {
            minimum: -0.125,
            maximum: 1.125,
            unit: ScopeUnit::RelativeLinear,
        },
        ScopeScale::Nits => ScopeAxis {
            minimum: -0.125 * settings.reference_white_nits,
            maximum: settings.peak_nits * 1.125,
            unit: ScopeUnit::Nits,
        },
        ScopeScale::OutputCode => ScopeAxis {
            minimum: -0.125,
            maximum: 1.125,
            unit: ScopeUnit::NormalizedCode,
        },
    };
    let grid = || ScopeGrid::new(settings.columns, settings.rows, horizontal, vertical);
    let mut report = ScopeReport {
        identity,
        dimensions,
        color,
        settings,
        signal_gamut: if settings.scale == ScopeScale::OutputCode {
            color.output_gamut
        } else {
            color.working_gamut
        },
        signal_transfer: if settings.scale == ScopeScale::OutputCode {
            color.output_transfer
        } else {
            OutputTransfer::Linear
        },
        vectorscope_gamut: color.output_gamut,
        vectorscope_transfer: color.output_transfer,
        sampled_pixels: 0,
        valid_pixels: 0,
        transparent_pixels: 0,
        invalid_pixels: 0,
        statistics: ScopeStatistics {
            linear_rgb: [None; 3],
            luminance_nits: None,
            output_rgb: [None; 3],
            output_luma: None,
            chroma: [None; 2],
        },
        clipping: ScopeClipping::default(),
        waveform: grid(),
        parade: std::array::from_fn(|_| grid()),
        vectorscope: ScopeGrid::new(
            settings.vector_size,
            settings.vector_size,
            ScopeAxis {
                minimum: -0.5,
                maximum: 0.5,
                unit: ScopeUnit::Cb,
            },
            ScopeAxis {
                minimum: -0.5,
                maximum: 0.5,
                unit: ScopeUnit::Cr,
            },
        ),
    };
    let matrix = gamut_matrix(color.working_gamut, color.output_gamut);
    let working_luma = luma_coefficients(color.working_gamut);
    let output_luma = luma_coefficients(color.output_gamut);
    let samples = pixel_count.min(settings.maximum_samples);
    for sample in 0..samples {
        let index = if samples == 1 {
            pixel_count / 2
        } else {
            sample * (pixel_count - 1) / (samples - 1)
        };
        let pixel = &pixels[index * 4..index * 4 + 4];
        report.sampled_pixels += 1;
        if pixel.iter().any(|channel| !channel.is_finite()) || !(0.0..=1.0).contains(&pixel[3]) {
            report.invalid_pixels += 1;
            continue;
        }
        if pixel[3] == 0.0 {
            report.transparent_pixels += 1;
            continue;
        }
        report.valid_pixels += 1;
        let rgb: [f64; 3] =
            std::array::from_fn(|channel| f64::from(pixel[channel]) / f64::from(pixel[3]));
        let luminance = dot(rgb, working_luma);
        let output_linear = matrix.map(|row| dot(rgb, row));
        let output = encode_output(output_linear, color.output_transfer, output_luma, settings);
        let encoded_luma = dot(output, output_luma);
        let cb = (output[2] - encoded_luma) / (2.0 * (1.0 - output_luma[2]));
        let cr = (output[0] - encoded_luma) / (2.0 * (1.0 - output_luma[0]));
        for channel in 0..3 {
            extend(&mut report.statistics.linear_rgb[channel], rgb[channel]);
            extend(&mut report.statistics.output_rgb[channel], output[channel]);
            let nits = rgb[channel] * settings.reference_white_nits;
            report.clipping.negative_channels[channel] += u64::from(rgb[channel] < 0.0);
            report.clipping.above_reference_white_channels[channel] +=
                u64::from(rgb[channel] > 1.0);
            report.clipping.above_peak_channels[channel] += u64::from(nits > settings.peak_nits);
            report.clipping.output_below_zero_channels[channel] += u64::from(output[channel] < 0.0);
            report.clipping.output_above_one_channels[channel] += u64::from(output[channel] > 1.0);
        }
        extend(
            &mut report.statistics.luminance_nits,
            luminance * settings.reference_white_nits,
        );
        extend(&mut report.statistics.output_luma, encoded_luma);
        extend(&mut report.statistics.chroma[0], cb);
        extend(&mut report.statistics.chroma[1], cr);
        let (levels, level) = match settings.scale {
            ScopeScale::Linear => (rgb, luminance),
            ScopeScale::Nits => (
                rgb.map(|channel| channel * settings.reference_white_nits),
                luminance * settings.reference_white_nits,
            ),
            ScopeScale::OutputCode => (output, encoded_luma),
        };
        let x = (index % width as usize) as f64 + 0.5;
        report.waveform.add(x, level);
        for (grid, level) in report.parade.iter_mut().zip(levels) {
            grid.add(x, level);
        }
        report.vectorscope.add(cb, cr);
    }
    Ok(report)
}

/// Validate scope resource and luminance limits.
/// `settings` defines the caller budget. Returns an error before allocation.
fn validate(settings: ScopeSettings) -> Result<(), ScopeError> {
    if !(2..=512).contains(&settings.columns)
        || !(2..=512).contains(&settings.rows)
        || !(2..=512).contains(&settings.vector_size)
        || !(1..=1_048_576).contains(&settings.maximum_samples)
    {
        return Err(ScopeError::InvalidSettings("plot or sampling budget"));
    }
    if !settings.reference_white_nits.is_finite()
        || !settings.peak_nits.is_finite()
        || settings.reference_white_nits <= 0.0
        || !(100.0..=10_000.0).contains(&settings.peak_nits)
        || settings.reference_white_nits > settings.peak_nits
    {
        return Err(ScopeError::InvalidSettings("reference white or peak nits"));
    }
    Ok(())
}

/// Locate a finite signal value in a bounded plot.
/// `value`, `axis` and `size` define the sample. Returns a clamped bin index.
fn bin(value: f64, axis: ScopeAxis, size: u32) -> usize {
    (((value - axis.minimum) / (axis.maximum - axis.minimum) * f64::from(size))
        .floor()
        .clamp(0.0, f64::from(size - 1))) as usize
}

/// Extend an unclamped numeric range.
/// `range` retains prior extrema and `value` is finite. Returns no value.
fn extend(range: &mut Option<ScopeRange>, value: f64) {
    match range {
        Some(range) => {
            range.minimum = range.minimum.min(value);
            range.maximum = range.maximum.max(value);
        }
        None => {
            *range = Some(ScopeRange {
                minimum: value,
                maximum: value,
            });
        }
    }
}

/// Evaluate a three-component weighted sum.
/// `a` and `b` contain components and weights. Returns their scalar product.
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

/// Select luminance weights for declared RGB primaries.
/// `gamut` is the signal color space. Returns red, green and blue coefficients.
fn luma_coefficients(gamut: WorkingGamut) -> [f64; 3] {
    match gamut {
        WorkingGamut::Bt709 => [0.2126, 0.7152, 0.0722],
        WorkingGamut::Bt2020 => [0.2627, 0.6780, 0.0593],
        WorkingGamut::DisplayP3 => [0.2289745641, 0.6917385218, 0.0792869141],
    }
}

/// Simulate a declared output transfer from display-referred linear RGB.
/// `rgb`, `transfer`, `luma` and `settings` define the color and absolute scale.
/// Returns unclamped code values; PQ/HLG cannot encode negative light, so their
/// diagnostic code transform floors negative components while working ranges
/// and negative-channel counts retain the original samples.
fn encode_output(
    rgb: [f64; 3],
    transfer: OutputTransfer,
    luma: [f64; 3],
    settings: ScopeSettings,
) -> [f64; 3] {
    if transfer == OutputTransfer::Hlg {
        let positive = rgb
            .map(|channel| channel.max(0.0) * settings.reference_white_nits / settings.peak_nits);
        let luminance = dot(positive, luma);
        if luminance == 0.0 {
            return [0.0; 3];
        }
        let gamma = if (400.0..=2000.0).contains(&settings.peak_nits) {
            1.2 + 0.42 * (settings.peak_nits / 1000.0).log10()
        } else {
            1.2 * 1.111_f64.powf((settings.peak_nits / 1000.0).log2())
        };
        let factor = luminance.powf((1.0 - gamma) / gamma);
        return positive.map(|channel| {
            let scene = channel * factor;
            if scene <= 1.0 / 12.0 {
                (3.0 * scene).sqrt()
            } else {
                let a = 0.17883277;
                let b = 1.0 - 4.0 * a;
                let c = 0.5 - a * (4.0_f64 * a).ln();
                a * (12.0 * scene - b).ln() + c
            }
        });
    }
    rgb.map(|channel| match transfer {
        OutputTransfer::Linear => channel,
        OutputTransfer::Srgb => {
            if channel <= 0.0031308 {
                channel * 12.92
            } else {
                1.055 * channel.powf(1.0 / 2.4) - 0.055
            }
        }
        OutputTransfer::Bt709 => {
            if channel < 0.018 {
                4.5 * channel
            } else {
                1.099 * channel.powf(0.45) - 0.099
            }
        }
        OutputTransfer::Pq => {
            let level = (channel.max(0.0) * settings.reference_white_nits / 10_000.0)
                .powf(2610.0 / 16_384.0);
            ((3424.0 / 4096.0 + 2413.0 / 128.0 * level) / (1.0 + 2392.0 / 128.0 * level))
                .powf(2523.0 / 32.0)
        }
        OutputTransfer::Hlg => unreachable!(),
    })
}

/// Build a D65 linear RGB conversion.
/// `source` and `destination` select primaries. Returns the row-major matrix.
fn gamut_matrix(source: WorkingGamut, destination: WorkingGamut) -> [[f64; 3]; 3] {
    if source == destination {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let input = xyz(source);
    let output = inverse(xyz(destination));
    std::array::from_fn(|row| {
        std::array::from_fn(|column| (0..3).map(|i| output[row][i] * input[i][column]).sum())
    })
}

/// Select the renderer's D65 RGB-to-XYZ matrix.
/// `gamut` selects primaries. Returns normalized row-major tristimulus weights.
fn xyz(gamut: WorkingGamut) -> [[f64; 3]; 3] {
    match gamut {
        WorkingGamut::Bt709 => [
            [0.4123907993, 0.3575843394, 0.1804807884],
            [0.2126390059, 0.7151686788, 0.0721923154],
            [0.0193308187, 0.1191947798, 0.9505321522],
        ],
        WorkingGamut::Bt2020 => [
            [0.6369580483, 0.1446169036, 0.1688809752],
            [0.2627002120, 0.6779980715, 0.0593017165],
            [0.0, 0.0280726930, 1.0609850577],
        ],
        WorkingGamut::DisplayP3 => [
            [0.4865709486, 0.2656676932, 0.1982172852],
            [0.2289745641, 0.6917385218, 0.0792869141],
            [0.0, 0.0451133819, 1.0439443689],
        ],
    }
}

/// Invert a known nonsingular color matrix.
/// `matrix` is one of the D65 primary matrices. Returns its row-major inverse.
fn inverse(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [a, b, c] = matrix;
    let cofactors = [
        [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ],
        [
            a[2] * c[1] - a[1] * c[2],
            a[0] * c[2] - a[2] * c[0],
            a[1] * c[0] - a[0] * c[1],
        ],
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ],
    ];
    let determinant = dot(a, cofactors[0]);
    std::array::from_fn(|row| std::array::from_fn(|column| cofactors[column][row] / determinant))
}
