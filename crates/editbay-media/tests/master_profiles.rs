use editbay_core::FrameRate;
use editbay_media::{DeliveryColor, DeliveryFormat, LosslessMovProfile, PixelEncoding};

fn profile() -> LosslessMovProfile {
    LosslessMovProfile {
        width: 256,
        height: 128,
        frame_rate: FrameRate {
            numerator: 25,
            denominator: 1,
        },
        first_frame: 2,
        frames: 1,
        sample_rate: 48_000,
        channels: vec!["FL".into(), "FR".into()],
    }
}

#[test]
fn fractional_selected_phase_is_preserved_for_mov_and_refused_for_mxf() {
    let mut value = profile();
    value.frame_rate = FrameRate {
        numerator: 30_000,
        denominator: 1001,
    };
    assert_eq!(value.sample_origin().unwrap(), 3204);
    assert_eq!(value.samples_through(1).unwrap(), 1601);
    DeliveryFormat::DnxhrHqxMov.validate(&value).unwrap();
    assert!(
        DeliveryFormat::DnxhrHqxMxf
            .validate(&value)
            .unwrap_err()
            .to_string()
            .contains("fractional")
    );
    value.frame_rate = FrameRate {
        numerator: 24_000,
        denominator: 1001,
    };
    assert_eq!(value.samples_through(1).unwrap(), 2002);
    DeliveryFormat::DnxhrHqxMxf.validate(&value).unwrap();
}

#[test]
fn masters_require_actual_bt709_opaque_high_depth_and_declared_channels() {
    for format in [DeliveryFormat::DnxhrHqxMov, DeliveryFormat::DnxhrHqxMxf] {
        assert_eq!(format.pixel_encoding(), PixelEncoding::Rgba16Le);
        assert!(format.requires_opaque() && format.pcm24() && !format.lossless());
        assert!(DeliveryColor::default().validate(format).is_err());
        DeliveryColor {
            primaries: 1,
            transfer: 1,
            hdr_metadata: Default::default(),
        }
        .validate(format)
        .unwrap();
        for invalid in [
            LosslessMovProfile {
                width: 255,
                ..profile()
            },
            LosslessMovProfile {
                height: 118,
                ..profile()
            },
            LosslessMovProfile {
                sample_rate: 44_100,
                ..profile()
            },
            LosslessMovProfile {
                channels: vec![
                    "FL".into(),
                    "FR".into(),
                    "FC".into(),
                    "LFE".into(),
                    "BL".into(),
                    "BR".into(),
                ],
                ..profile()
            },
            LosslessMovProfile {
                frame_rate: FrameRate {
                    numerator: 27,
                    denominator: 1,
                },
                ..profile()
            },
        ] {
            assert!(format.validate(&invalid).is_err());
        }
    }
}

#[test]
fn hevc_hdr_negotiation_and_registration_never_claim_qualification() {
    let format = DeliveryFormat::HevcMain10Mp4;
    assert!(format.requires_opaque() && format.aac());
    assert_eq!(format.pixel_encoding(), PixelEncoding::Rgba16Le);
    let hdr = DeliveryColor {
        primaries: 9,
        transfer: 16,
        hdr_metadata: Default::default(),
    };
    hdr.validate(format).unwrap();
    assert!(
        DeliveryColor {
            primaries: 1,
            ..hdr
        }
        .validate(format)
        .is_err()
    );
    assert!(
        format
            .validate(&LosslessMovProfile {
                width: 15,
                ..profile()
            })
            .is_err()
    );
    for available in DeliveryFormat::ALL.map(|format| format.availability()) {
        assert!(matches!(
            available.status,
            "available" | "unavailable" | "undetermined"
        ));
        assert_eq!(available.qualification, "not_run_by_capability_query");
        assert!(!available.reason.is_empty());
    }
}

#[test]
fn webm_exact_picture_clock_is_explicit_and_sound_is_48k() {
    let format = DeliveryFormat::Vp9Profile2Webm;
    format.validate(&profile()).unwrap();
    assert!(format.lossy_sound() && format.yuv10() && format.requires_opaque());
    assert_eq!(format.extension(), "webm");
    for invalid in [
        LosslessMovProfile {
            frame_rate: FrameRate {
                numerator: 30_000,
                denominator: 1001,
            },
            ..profile()
        },
        LosslessMovProfile {
            frame_rate: FrameRate {
                numerator: 24,
                denominator: 1,
            },
            ..profile()
        },
        LosslessMovProfile {
            sample_rate: 44100,
            ..profile()
        },
    ] {
        assert!(
            format
                .validate(&invalid)
                .unwrap_err()
                .to_string()
                .contains("WebM")
        );
    }
}

#[test]
fn hevc_main8_is_a_separate_declared_lossy_sdr_profile() {
    let format = DeliveryFormat::HevcMainMp4;
    assert!(format.hevc() && format.aac() && format.requires_opaque());
    assert_eq!(format.pixel_encoding(), PixelEncoding::Rgba16Le);
    assert_eq!(format.availability().pixel_format, "yuv420p");
    DeliveryColor::default().validate(format).unwrap();
    assert!(
        DeliveryColor {
            primaries: 9,
            transfer: 16,
            hdr_metadata: Default::default()
        }
        .validate(format)
        .is_err()
    );
    assert_eq!(
        DeliveryFormat::HevcMain10Mp4.availability().pixel_format,
        "yuv420p10le"
    );
}
