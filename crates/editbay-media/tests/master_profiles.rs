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
