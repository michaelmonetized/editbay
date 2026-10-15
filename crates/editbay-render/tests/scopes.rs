use editbay_core::{ColorConfiguration, DocumentVersion, OutputTransfer, WorkingGamut};
use editbay_render::scopes::{
    ScopeError, ScopeIdentity, ScopeReport, ScopeScale, ScopeSettings, ScopeUnit,
    sample_program_frame,
};
use uuid::Uuid;

fn identity() -> ScopeIdentity {
    ScopeIdentity {
        version: DocumentVersion {
            project_id: Uuid::from_u128(1),
            revision: 7,
        },
        composition: Uuid::from_u128(2),
        frame: 19,
    }
}

fn settings() -> ScopeSettings {
    ScopeSettings {
        columns: 5,
        rows: 10,
        vector_size: 10,
        scale: ScopeScale::Linear,
        ..ScopeSettings::default()
    }
}

fn linear_color(gamut: WorkingGamut) -> ColorConfiguration {
    ColorConfiguration {
        working_gamut: gamut,
        output_gamut: gamut,
        output_transfer: OutputTransfer::Linear,
        ..ColorConfiguration::default()
    }
}

fn measure(pixels: &[[f32; 4]], color: ColorConfiguration, settings: ScopeSettings) -> ScopeReport {
    sample_program_frame(
        pixels.as_flattened(),
        [pixels.len() as u32, 1],
        identity(),
        color,
        settings,
    )
    .unwrap()
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "{actual} != {expected} within {tolerance}"
    );
}

#[test]
fn known_linear_ramp_retains_horizontal_position_and_numeric_levels() {
    let pixels = [0.0, 0.25, 0.5, 0.75, 1.0].map(|level| [level, level, level, 1.0]);
    let report = measure(&pixels, linear_color(WorkingGamut::Bt709), settings());
    assert_eq!(report.sampled_pixels, 5);
    assert_eq!(report.valid_pixels, 5);
    assert_eq!(report.waveform.vertical.unit, ScopeUnit::RelativeLinear);
    assert_eq!(report.waveform.horizontal.unit, ScopeUnit::ImageColumn);
    for (column, row) in [(0, 1), (1, 3), (2, 5), (3, 7), (4, 9)] {
        assert_eq!(report.waveform.count(column, row), Some(1));
        for channel in &report.parade {
            assert_eq!(channel.count(column, row), Some(1));
        }
    }
    assert_eq!(report.waveform.count(5, 0), None);
    assert_eq!(report.waveform.count(0, 10), None);
    close(report.statistics.luminance_nits.unwrap().minimum, 0.0, 0.0);
    close(
        report.statistics.luminance_nits.unwrap().maximum,
        100.0,
        1e-12,
    );
    assert!(report.clipping.above_peak_channels.iter().all(|n| *n == 0));
}

#[test]
fn bt709_primaries_match_independent_ncl_ycbcr_constants() {
    let report = measure(
        &[
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ],
        linear_color(WorkingGamut::Bt709),
        settings(),
    );
    close(
        report.statistics.output_luma.unwrap().minimum,
        0.0722,
        1e-12,
    );
    close(
        report.statistics.output_luma.unwrap().maximum,
        0.7152,
        1e-12,
    );
    close(
        report.statistics.chroma[0].unwrap().minimum,
        -0.38542789394266,
        1e-12,
    );
    close(report.statistics.chroma[0].unwrap().maximum, 0.5, 1e-12);
    close(
        report.statistics.chroma[1].unwrap().minimum,
        -0.45415290830581656,
        1e-12,
    );
    close(report.statistics.chroma[1].unwrap().maximum, 0.5, 1e-12);
    for (column, row) in [(3, 9), (1, 0), (9, 4)] {
        assert_eq!(report.vectorscope.count(column, row), Some(1));
    }
    assert_eq!(report.vectorscope.counts.iter().sum::<u32>(), 3);
}

#[test]
fn working_gamut_determines_absolute_luminance_instead_of_assuming_bt709() {
    for (gamut, expected) in [
        (WorkingGamut::Bt709, 21.26),
        (WorkingGamut::Bt2020, 26.27),
        (WorkingGamut::DisplayP3, 22.89745641),
    ] {
        let report = measure(&[[1.0, 0.0, 0.0, 1.0]], linear_color(gamut), settings());
        assert_eq!(report.signal_gamut, gamut);
        close(
            report.statistics.luminance_nits.unwrap().minimum,
            expected,
            1e-8,
        );
    }
}

#[test]
fn nits_preserve_highlights_and_use_the_explicit_reference_white() {
    let settings = ScopeSettings {
        scale: ScopeScale::Nits,
        reference_white_nits: 203.0,
        peak_nits: 1000.0,
        ..settings()
    };
    let report = measure(
        &[
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [5.0, 5.0, 5.0, 1.0],
        ],
        linear_color(WorkingGamut::Bt2020),
        settings,
    );
    assert_eq!(report.waveform.vertical.unit, ScopeUnit::Nits);
    close(report.waveform.vertical.minimum, -25.375, 0.0);
    close(report.waveform.vertical.maximum, 1125.0, 0.0);
    close(
        report.statistics.luminance_nits.unwrap().maximum,
        1015.0,
        1e-10,
    );
    assert_eq!(report.clipping.above_reference_white_channels, [1; 3]);
    assert_eq!(report.clipping.above_peak_channels, [1; 3]);
    assert_eq!(report.waveform.outside, 0);
}

#[test]
fn premultiplied_partial_alpha_matches_straight_color_and_zero_alpha_is_excluded() {
    let partial = measure(
        &[[0.1, 0.2, 0.4, 0.5], [100.0, -20.0, 8.0, 0.0]],
        linear_color(WorkingGamut::Bt709),
        settings(),
    );
    let straight = measure(
        &[[0.2, 0.4, 0.8, 1.0]],
        linear_color(WorkingGamut::Bt709),
        settings(),
    );
    assert_eq!(partial.statistics, straight.statistics);
    assert_eq!(partial.clipping, straight.clipping);
    assert_eq!(partial.vectorscope, straight.vectorscope);
    assert_eq!(partial.sampled_pixels, 2);
    assert_eq!(partial.valid_pixels, 1);
    assert_eq!(partial.transparent_pixels, 1);
}

#[test]
fn negatives_overrange_and_plot_clamping_keep_unclamped_numeric_evidence() {
    let report = measure(
        &[[-0.25, 1.5, 120.0, 1.0]],
        ColorConfiguration::default(),
        settings(),
    );
    assert_eq!(report.clipping.negative_channels, [1, 0, 0]);
    assert_eq!(report.clipping.above_reference_white_channels, [0, 1, 1]);
    assert_eq!(report.clipping.above_peak_channels, [0, 0, 1]);
    assert_eq!(report.clipping.output_below_zero_channels, [1, 0, 0]);
    assert_eq!(report.clipping.output_above_one_channels, [0, 1, 1]);
    close(report.statistics.linear_rgb[0].unwrap().minimum, -0.25, 0.0);
    close(report.statistics.linear_rgb[2].unwrap().maximum, 120.0, 0.0);
    close(
        report.statistics.output_rgb[0].unwrap().minimum,
        -3.23,
        1e-12,
    );
    assert_eq!(report.waveform.outside, 1);
    assert!(report.parade.iter().all(|grid| grid.outside == 1));
    assert_eq!(report.waveform.counts.iter().sum::<u32>(), 1);
}

#[test]
fn invalid_pixels_do_not_poison_ranges_or_density_bins() {
    let report = measure(
        &[
            [f32::NAN, 0.0, 0.0, 1.0],
            [0.0, f32::INFINITY, 0.0, 1.0],
            [0.0, 0.0, 0.0, -0.1],
            [0.0, 0.0, 0.0, 1.1],
            [0.0, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.5, 1.0],
        ],
        linear_color(WorkingGamut::Bt709),
        settings(),
    );
    assert_eq!(report.sampled_pixels, 6);
    assert_eq!(report.invalid_pixels, 4);
    assert_eq!(report.transparent_pixels, 1);
    assert_eq!(report.valid_pixels, 1);
    assert_eq!(report.waveform.counts.iter().sum::<u32>(), 1);
    close(
        report.statistics.luminance_nits.unwrap().maximum,
        50.0,
        1e-12,
    );
    let empty = measure(
        &[[0.0, 0.0, 0.0, 0.0]],
        linear_color(WorkingGamut::Bt709),
        settings(),
    );
    assert!(empty.statistics.luminance_nits.is_none());
    assert_eq!(empty.waveform.counts.iter().sum::<u32>(), 0);
}

#[test]
fn pq_absolute_code_values_match_fixed_st2084_reference_points() {
    let color = ColorConfiguration {
        output_transfer: OutputTransfer::Pq,
        ..linear_color(WorkingGamut::Bt2020)
    };
    let settings = ScopeSettings {
        scale: ScopeScale::OutputCode,
        ..settings()
    };
    for (linear, nits, code) in [
        (0.0, 0.0, 0.0000007309559025783966),
        (1.0, 100.0, 0.508078421517399),
        (10.0, 1000.0, 0.751827096247041),
        (100.0, 10_000.0, 1.0),
    ] {
        let report = measure(&[[linear, linear, linear, 1.0]], color, settings);
        assert_eq!(report.signal_transfer, OutputTransfer::Pq);
        assert_eq!(report.signal_gamut, WorkingGamut::Bt2020);
        assert_eq!(report.waveform.vertical.unit, ScopeUnit::NormalizedCode);
        close(
            report.statistics.output_rgb[0].unwrap().minimum,
            code,
            1e-10,
        );
        close(
            report.statistics.luminance_nits.unwrap().minimum,
            nits,
            1e-9,
        );
    }
}

#[test]
fn hlg_reference_ootf_is_inverted_before_the_oetf() {
    let color = ColorConfiguration {
        output_transfer: OutputTransfer::Hlg,
        ..linear_color(WorkingGamut::Bt2020)
    };
    let settings = ScopeSettings {
        peak_nits: 1000.0,
        scale: ScopeScale::OutputCode,
        ..settings()
    };
    for (linear, expected_code) in [
        (0.0, 0.0),
        (0.506_970_3, 0.5),
        (1.277_400_3, 0.6723581321276545),
        (10.0, 0.9999999950661305),
    ] {
        let report = measure(&[[linear, linear, linear, 1.0]], color, settings);
        assert_eq!(report.vectorscope_transfer, OutputTransfer::Hlg);
        close(
            report.statistics.output_rgb[0].unwrap().minimum,
            expected_code,
            2e-8,
        );
    }
}

#[test]
fn output_code_scopes_convert_primaries_and_retain_working_statistics() {
    let color = ColorConfiguration {
        output_gamut: WorkingGamut::Bt2020,
        ..linear_color(WorkingGamut::Bt709)
    };
    let report = measure(
        &[[1.0, 0.0, 0.0, 1.0]],
        color,
        ScopeSettings {
            scale: ScopeScale::OutputCode,
            ..settings()
        },
    );
    assert_eq!(report.signal_gamut, WorkingGamut::Bt2020);
    assert_eq!(report.vectorscope_gamut, WorkingGamut::Bt2020);
    for (range, expected) in
        report
            .statistics
            .output_rgb
            .iter()
            .zip([0.6274038959, 0.0690972894, 0.0163914389])
    {
        close(range.unwrap().minimum, expected, 1e-8);
    }
    close(report.statistics.linear_rgb[0].unwrap().minimum, 1.0, 0.0);
    close(
        report.statistics.luminance_nits.unwrap().minimum,
        21.26,
        1e-10,
    );
}

#[test]
fn bounded_sampling_is_deterministic_and_includes_the_last_pixel() {
    let pixels =
        [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9].map(|level| [level, level, level, 1.0]);
    let settings = ScopeSettings {
        maximum_samples: 3,
        ..settings()
    };
    let report = measure(&pixels, linear_color(WorkingGamut::Bt709), settings);
    assert_eq!(report.sampled_pixels, 3);
    assert_eq!(report.waveform.counts.iter().sum::<u32>(), 3);
    close(report.statistics.linear_rgb[0].unwrap().minimum, 0.0, 0.0);
    close(report.statistics.linear_rgb[0].unwrap().maximum, 0.9, 3e-8);
    assert_eq!(
        report,
        measure(&pixels, linear_color(WorkingGamut::Bt709), settings)
    );
    let one = measure(
        &pixels,
        linear_color(WorkingGamut::Bt709),
        ScopeSettings {
            maximum_samples: 1,
            ..settings
        },
    );
    close(one.statistics.linear_rgb[0].unwrap().minimum, 0.5, 0.0);
}

#[test]
fn report_identity_rejects_stale_revision_frame_and_color_settings() {
    let color = linear_color(WorkingGamut::Bt709);
    let settings = settings();
    let report = measure(&[[0.5, 0.5, 0.5, 1.0]], color, settings);
    assert!(report.matches(identity(), color, settings));
    let mut changed = identity();
    changed.version.revision += 1;
    assert!(!report.matches(changed, color, settings));
    changed = identity();
    changed.version.project_id = Uuid::from_u128(3);
    assert!(!report.matches(changed, color, settings));
    changed = identity();
    changed.composition = Uuid::from_u128(4);
    assert!(!report.matches(changed, color, settings));
    changed = identity();
    changed.frame += 1;
    assert!(!report.matches(changed, color, settings));
    assert!(!report.matches(
        identity(),
        ColorConfiguration {
            output_transfer: OutputTransfer::Bt709,
            ..color
        },
        settings
    ));
    assert!(!report.matches(
        identity(),
        color,
        ScopeSettings {
            reference_white_nits: 203.0,
            ..settings
        }
    ));
}

#[test]
fn malformed_inputs_and_unbounded_resources_are_rejected_before_allocation() {
    let color = linear_color(WorkingGamut::Bt709);
    assert_eq!(
        sample_program_frame(&[], [0, 1], identity(), color, settings()).unwrap_err(),
        ScopeError::InvalidDimensions
    );
    assert_eq!(
        sample_program_frame(&[], [8193, 1], identity(), color, settings()).unwrap_err(),
        ScopeError::InvalidDimensions
    );
    assert_eq!(
        sample_program_frame(&[], [1, 1], identity(), color, settings()).unwrap_err(),
        ScopeError::PixelLength {
            expected: 4,
            actual: 0
        }
    );
    for settings in [
        ScopeSettings {
            rows: 513,
            ..settings()
        },
        ScopeSettings {
            columns: 0,
            ..settings()
        },
        ScopeSettings {
            vector_size: 513,
            ..settings()
        },
        ScopeSettings {
            maximum_samples: 0,
            ..settings()
        },
        ScopeSettings {
            maximum_samples: 1_048_577,
            ..settings()
        },
        ScopeSettings {
            reference_white_nits: f64::NAN,
            ..settings()
        },
        ScopeSettings {
            reference_white_nits: 0.0,
            ..settings()
        },
        ScopeSettings {
            reference_white_nits: 1001.0,
            peak_nits: 1000.0,
            ..settings()
        },
        ScopeSettings {
            peak_nits: f64::INFINITY,
            ..settings()
        },
        ScopeSettings {
            peak_nits: 99.0,
            ..settings()
        },
        ScopeSettings {
            peak_nits: 10_001.0,
            ..settings()
        },
    ] {
        assert!(matches!(
            sample_program_frame(&[0.0; 4], [1, 1], identity(), color, settings),
            Err(ScopeError::InvalidSettings(_))
        ));
    }
}
