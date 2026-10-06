use editbay_core::{FrameRate, SourceColor, TimeBase};
use editbay_media::{
    Cancellation, Error, LosslessMovProfile, LosslessMovWriter, NativeAudioReader, SourceFile,
    VideoReader,
};
use std::io::{Seek, SeekFrom, Write};

fn profile(channels: &[&str], rate: u32) -> LosslessMovProfile {
    LosslessMovProfile {
        width: 32,
        height: 18,
        frame_rate: FrameRate::new(30000, 1001).unwrap(),
        first_frame: 0,
        frames: 17,
        sample_rate: rate,
        channels: channels.iter().map(|s| (*s).into()).collect(),
    }
}

fn pixels(frame: u64) -> Vec<u8> {
    (0..32 * 18)
        .flat_map(|index| {
            [
                index as u8,
                frame as u8 * 11,
                (index / 7) as u8,
                (index % 256) as u8,
            ]
        })
        .collect()
}

fn pcm(first: u64, frames: u64, channels: usize) -> Vec<f32> {
    (first..first + frames)
        .flat_map(|sample| {
            (0..channels)
                .map(move |channel| ((sample % 2048) as f32 - 1024.) / 512. + channel as f32 / 16.)
        })
        .collect()
}

#[test]
fn native_lossless_mov_preserves_every_pixel_sample_channel_and_exact_clock() {
    for (channels, rate) in [
        (&["FC"][..], 44100),
        (&["FL", "FR"][..], 48000),
        (&["FL", "FR", "FC", "LFE", "BL", "BR"][..], 48000),
    ] {
        for first_frame in [0, 1, 2, 5] {
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.seek(SeekFrom::Start(17)).unwrap();
            let mut profile = profile(channels, rate);
            profile.first_frame = first_frame;
            let grid = |frame: u64| {
                let numerator = frame * u64::from(rate) * 1001;
                numerator / 30000 + u64::from(!numerator.is_multiple_of(30000))
            };
            let origin = grid(first_frame);
            let total = grid(first_frame + profile.frames) - origin;
            assert_eq!(profile.sample_origin().unwrap(), origin);
            assert_eq!(profile.samples_through(profile.frames).unwrap(), total);
            let cancel = Cancellation::new().unwrap();
            let mut writer =
                LosslessMovWriter::new(file.as_file(), profile.clone(), cancel.clone()).unwrap();
            let mut sound = 0;
            for frame in 0..profile.frames {
                writer.picture(frame, &pixels(frame + first_frame)).unwrap();
                let end = profile.samples_through(frame + 1).unwrap();
                while sound < end {
                    let count = (end - sound).min(4096);
                    writer
                        .sound(sound, &pcm(origin + sound, count, channels.len()))
                        .unwrap();
                    sound += count;
                }
            }
            writer.finish().unwrap();
            file.as_file().sync_all().unwrap();
            let source = SourceFile::open(file.path(), &cancel).unwrap();
            let probe = source.probe(cancel.clone()).unwrap();
            assert_eq!(probe.streams.len(), 2);
            assert_eq!(probe.streams[0].codec, "png");
            assert_eq!(
                probe.streams[0].color,
                SourceColor {
                    primaries: 1,
                    transfer: 13,
                    matrix: 0,
                    range: 2
                }
            );
            assert_eq!(
                probe.streams[0].time_base,
                Some(TimeBase {
                    numerator: 1,
                    denominator: 30000
                })
            );
            assert_eq!(probe.streams[1].codec, "pcm_f32le");
            assert_eq!(probe.streams[1].channels, channels);
            assert_eq!(probe.streams[1].sample_rate, Some(rate));
            assert_eq!(
                probe.streams[1].time_base,
                Some(TimeBase {
                    numerator: 1,
                    denominator: rate
                })
            );
            let mut video = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
            for frame in 0..profile.frames {
                let decoded = video.next_frame().unwrap().unwrap();
                assert_eq!(decoded.source_tick, Some(frame as i64 * 1001));
                assert_eq!(decoded.alpha, editbay_core::AlphaMode::Straight);
                assert_eq!(decoded.rgba, pixels(frame + first_frame));
            }
            assert!(video.next_frame().unwrap().is_none());
            let mut audio = NativeAudioReader::open_stream(&source, 1, cancel.clone()).unwrap();
            let mut actual = Vec::new();
            while let Some(block) = audio.next_block().unwrap() {
                assert_eq!(
                    block.first_sample,
                    Some((actual.len() / channels.len()) as i64)
                );
                actual.extend(block.samples);
            }
            assert_eq!(sound, total);
            assert_eq!(actual, pcm(origin, total, channels.len()));
            source.verify(&cancel).unwrap();
        }
    }
}

#[test]
fn selected_phase_preserves_fractional_edge_samples_and_refuses_overflow() {
    let mut selected = profile(&["FL", "FR"], 48000);
    selected.first_frame = 2;
    selected.frames = 1;
    assert_eq!(selected.sample_origin().unwrap(), 3204);
    assert_eq!(selected.samples_through(0).unwrap(), 0);
    assert_eq!(selected.samples_through(1).unwrap(), 1601);
    assert!(selected.samples_through(2).is_err());
    selected.first_frame = 0;
    assert_eq!(selected.samples_through(1).unwrap(), 1602);
    selected.first_frame = u64::MAX;
    assert!(selected.validate().is_err());
    assert!(selected.samples_through(1).is_err());
    selected.first_frame = i64::MAX as u64;
    assert!(selected.validate().is_err());
    selected.frame_rate.numerator = 0;
    assert!(selected.sample_origin().is_err());
}

#[test]
fn delivery_refuses_incomplete_stale_unbounded_cancelled_and_invalid_work() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let profile = profile(&["FL", "FR"], 48000);
    let token = Cancellation::new().unwrap();
    let mut writer =
        LosslessMovWriter::new(file.as_file(), profile.clone(), token.clone()).unwrap();
    assert!(writer.sound(0, &[0.; 2]).is_err());
    assert!(writer.picture(1, &pixels(0)).is_err());
    assert!(writer.picture(0, &[0; 3]).is_err());
    writer.picture(0, &pixels(0)).unwrap();
    assert!(writer.picture(1, &pixels(1)).is_err());
    assert!(writer.sound(1, &[0.; 2]).is_err());
    assert!(writer.sound(0, &[f32::NAN, 0.]).is_err());
    assert!(writer.sound(0, &[0.; 8194]).is_err());
    writer.sound(0, &[2., -2.]).unwrap();
    token.cancel();
    assert!(matches!(writer.sound(1, &[0.; 2]), Err(Error::Cancelled)));
    assert!(matches!(writer.finish(), Err(Error::Cancelled)));
    file.as_file().set_len(0).unwrap();
    let writer = LosslessMovWriter::new(
        file.as_file(),
        profile.clone(),
        Cancellation::new().unwrap(),
    )
    .unwrap();
    assert!(writer.finish().is_err());
    file.as_file().set_len(0).unwrap();
    file.write_all(b"owned content").unwrap();
    assert!(
        LosslessMovWriter::new(
            file.as_file(),
            profile.clone(),
            Cancellation::new().unwrap()
        )
        .is_err()
    );
    for channels in [
        vec!["FR".into(), "FL".into()],
        vec!["U0".into()],
        Vec::new(),
    ] {
        let mut bad = profile.clone();
        bad.channels = channels;
        assert!(bad.validate().is_err());
    }
    let mut bad = profile;
    bad.frame_rate.numerator = 0;
    assert!(bad.validate().is_err());
}
