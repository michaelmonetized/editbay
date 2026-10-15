use editbay_core::{
    DocumentEditor, DocumentVersion, FrameRate, PictureTiming, Project, StreamFormat,
};
use editbay_media::{
    Cancellation, DeliveryFormat, Error, LosslessMovProfile, LosslessMovWriter, NativeAudioReader,
    PixelEncoding, SourceFile, StreamType, VideoReader,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const WIDTH: u32 = 18;
const HEIGHT: u32 = 4;
const FRAMES: u64 = 3;
const SAMPLES_PER_FRAME: u64 = 2000;

struct Fixture {
    directory: PathBuf,
    retained: bool,
    _temporary: Option<tempfile::TempDir>,
}

impl Fixture {
    /// Allocate only generated test-owned assets beneath /home.
    /// `case` names this native qualification; returns storage retained only when requested.
    fn new(case: &str) -> TestResult<Self> {
        let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR").map(PathBuf::from);
        let root = requested
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        assert!(root.starts_with("/home"));
        if requested.is_some() {
            let directory = root.join(format!("precision-{case}-{}", Uuid::new_v4()));
            fs::create_dir(&directory)?;
            Ok(Self {
                directory,
                retained: true,
                _temporary: None,
            })
        } else {
            let temporary = tempfile::Builder::new()
                .prefix("precision-")
                .tempdir_in(root)?;
            Ok(Self {
                directory: temporary.path().into(),
                retained: false,
                _temporary: Some(temporary),
            })
        }
    }

    /// Record actual measured checks without writing an unavailable-test pass.
    /// `case` and `evidence` supply completed assertions; returns after new-file synchronization.
    fn record(&self, case: &str, evidence: Value) -> TestResult<()> {
        if self.retained {
            let receipt = json!({
                "schema":"editbay.native-precision.v1","status":"passed","case":case,
                "license":"MIT; original analytic source-generated fixtures",
                "dimensions":[WIDTH,HEIGHT],"evidence":evidence,
                "proof_boundary":"Native integer16 PNG/MOV and normalized F32 output only. The separate ffmpeg process shares the FFmpeg codec family; original PNG encoding and analytic channel/sample oracles are independent. No GPU, display, sound device, ICC processing, HDR tone mapping, float-origin media or general format-family conformance is claimed."
            });
            let path = self.directory.join("precision.json");
            let mut file = new_file(&path)?;
            file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
            file.sync_all()?;
            eprintln!("Actual native precision evidence: {}", path.display());
        }
        Ok(())
    }
}

/// Create a file only inside this fixture's unique test-owned directory.
/// `path` selects new storage; returns an empty readable/writable file.
fn new_file(path: &Path) -> TestResult<File> {
    Ok(OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .open(path)?)
}

/// Generate straight RGBA channels with significant low bits and every alpha boundary.
/// `frame` selects a distinct analytic picture; returns row-major full-range u16 samples.
fn samples16(frame: u64) -> Vec<u16> {
    (0..u64::from(WIDTH * HEIGHT))
        .flat_map(|index| {
            [
                ((frame * 4096 + index * 257 + 1) % 65536) as u16,
                ((index * 911 + frame * 257 + 0x1234) % 65536) as u16,
                (65535 - (index * 173 + frame * 331) % 65536) as u16,
                [0, 1, 257, 32767, 32768, 65534, 65535][((index + frame) % 7) as usize],
            ]
        })
        .collect()
}

/// Serialize the native integer16 byte contract without host-endian casts.
/// `samples` are straight interleaved channels; returns exact little-endian bytes.
fn little_endian16(samples: &[u16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

/// Compare native F32 bytes with full-range normalized original integer channels.
/// `bytes` and `expected` retain straight source-encoded samples; returns maximum error.
fn normalized_floats(bytes: &[u8], expected: &[u16]) -> TestResult<f32> {
    assert_eq!(bytes.len(), expected.len() * 4);
    let mut maximum = 0_f32;
    for (bytes, sample) in bytes.chunks_exact(4).zip(expected) {
        let value = f32::from_le_bytes(bytes.try_into()?);
        let wanted = f32::from(*sample) / 65535.0;
        let error = (value - wanted).abs();
        assert!(
            value.is_finite() && error <= 2.0e-7,
            "F32 {value} != normalized u16 {wanted}"
        );
        maximum = maximum.max(error);
    }
    Ok(maximum)
}

/// Compute PNG's standard CRC over a chunk's type and data.
/// `bytes` are the exact covered bytes; returns the reflected IEEE CRC-32.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0xedb8_8320 };
        }
    }
    !crc
}

/// Append a genuine PNG chunk with an independently computed checksum.
/// `output`, `kind` and `data` select its bytes; returns no value.
fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());
    output.extend_from_slice(kind);
    output.extend_from_slice(data);
    let mut covered = kind.to_vec();
    covered.extend_from_slice(data);
    output.extend_from_slice(&crc32(&covered).to_be_bytes());
}

/// Encode a tiny original PNG without calling the native encoder or an external codec.
/// `path` and `depth` select a new RGBA8/16 file; returns after writing filter0/stored-DEFLATE rows.
fn independent_png(path: &Path, depth: u8) -> TestResult<()> {
    assert!([8, 16].contains(&depth));
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    let samples = samples16(0);
    let mut rows = Vec::new();
    for row in samples.chunks_exact(WIDTH as usize * 4) {
        rows.push(0);
        for sample in row {
            if depth == 16 {
                rows.extend_from_slice(&sample.to_be_bytes());
            } else {
                rows.push((sample >> 8) as u8);
            }
        }
    }
    let length = u16::try_from(rows.len())?;
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&rows);
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in rows {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&WIDTH.to_be_bytes());
    ihdr.extend_from_slice(&HEIGHT.to_be_bytes());
    ihdr.extend_from_slice(&[depth, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    let mut file = new_file(path)?;
    file.write_all(&png)?;
    file.sync_all()?;
    Ok(())
}

/// Decode raw integer16 pixels through a separately invoked software reference path.
/// `path` and `frames` bound output; returns bytes or fails when reference tooling is unavailable.
fn reference16(path: &Path, frames: u64) -> TestResult<(Vec<u8>, Value)> {
    let executable = std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH is absent")?)
        .map(|directory| directory.join("ffmpeg"))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .ok_or("Independent ffmpeg reference executable is unavailable")?
        .canonicalize()?;
    let cancel = Cancellation::new()?;
    let tool = SourceFile::open(&executable, &cancel)?;
    let version = Command::new(&executable).arg("-version").output()?;
    assert!(version.status.success());
    assert!(version.stdout.len() <= 32768 && version.stderr.len() <= 32768);
    let version_text = String::from_utf8(version.stdout)?;
    let args = vec![
        "-v".to_owned(),
        "error".into(),
        "-nostdin".into(),
        "-hwaccel".into(),
        "none".into(),
        "-threads".into(),
        "2".into(),
        "-filter_threads".into(),
        "1".into(),
        "-filter_complex_threads".into(),
        "1".into(),
        "-i".into(),
        path.to_str()
            .ok_or("Reference receipt requires a UTF-8 fixture path")?
            .to_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-frames:v".into(),
        frames.to_string(),
        "-pix_fmt".into(),
        "rgba64le".into(),
        "-c:v".into(),
        "rawvideo".into(),
        "-threads:v".into(),
        "1".into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ];
    let output = Command::new(&executable).args(&args).output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout.len(),
        WIDTH as usize * HEIGHT as usize * 8 * frames as usize
    );
    tool.verify(&cancel)?;
    let invocation = std::iter::once(executable.to_string_lossy().into_owned())
        .chain(args)
        .collect::<Vec<_>>();
    let evidence = json!({
        "executable":executable,"executable_fingerprint":tool.fingerprint(),
        "version_command":[executable,"-version"],"version":version_text,
        "command":invocation,"exit_code":0,
        "decoded_rgba16le_sha256":format!("{:x}",Sha256::digest(&output.stdout)),
        "boundary":"Separate software process; shares the FFmpeg codec family, not an independent codec implementation"
    });
    Ok((output.stdout, evidence))
}

/// Evaluate exactly representable mono PCM independently of all media readers.
/// `sample` is its original local index; returns an unambiguous analytic float sample.
fn pcm(sample: u64) -> f32 {
    -0.25 + (sample % 31) as f32 / 4096.0
}

/// Write actual native PNG16/float-PCM MOV through the requested delivery format.
/// `path`, `cancel` and `check_lengths` own output and validation; returns its exact profile.
fn native_mov(
    path: &Path,
    cancel: &Cancellation,
    check_lengths: bool,
) -> TestResult<LosslessMovProfile> {
    let profile = LosslessMovProfile {
        width: WIDTH,
        height: HEIGHT,
        frame_rate: FrameRate::new(24, 1)?,
        first_frame: 0,
        frames: FRAMES,
        sample_rate: 48_000,
        channels: vec!["FC".into()],
    };
    let file = new_file(path)?;
    let mut writer = LosslessMovWriter::encoded(
        &file,
        profile.clone(),
        DeliveryFormat::Lossless16Mov,
        cancel.clone(),
    )?;
    let mut first = 0;
    for frame in 0..FRAMES {
        let bytes = little_endian16(&samples16(frame));
        if check_lengths && frame == 0 {
            for length in [0, bytes.len() / 2, bytes.len() - 1, bytes.len() + 1] {
                assert!(matches!(
                    writer.picture(0, &vec![0; length]),
                    Err(Error::Invalid(_))
                ));
            }
        }
        writer.picture(frame, &bytes)?;
        let end = (frame + 1) * SAMPLES_PER_FRAME;
        assert_eq!(profile.samples_through(frame + 1)?, end);
        writer.sound(first, &(first..end).map(pcm).collect::<Vec<_>>())?;
        first = end;
    }
    writer.finish()?;
    file.sync_all()?;
    Ok(profile)
}

/// Validate selected encoding, exact timestamps and every native integer16 channel.
/// `path` and `cancel` own the source; returns captured source and measured readback evidence.
fn inspect_mov(path: &Path, cancel: &Cancellation) -> TestResult<Value> {
    let source = SourceFile::open(path, cancel)?;
    let probe = source.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video)
        .ok_or("Video absent")?;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Audio)
        .ok_or("Audio absent")?;
    assert_eq!(video.codec, "png");
    assert_eq!(audio.codec, "pcm_f32le");
    assert_eq!(audio.channels, ["FC"]);
    let base = video.time_base.ok_or("Video time base absent")?;
    let mut reader = VideoReader::open_stream_with_encoding(
        &source,
        video.index,
        cancel.clone(),
        PixelEncoding::Rgba16Le,
    )?;
    let mut expected_all = Vec::new();
    let mut captured = Vec::new();
    let mut frame = 0;
    while let Some(decoded) = reader.next_frame()? {
        assert!(frame < FRAMES);
        assert_eq!(decoded.encoding, PixelEncoding::Rgba16Le);
        let expected = little_endian16(&samples16(frame));
        assert_eq!(decoded.rgba, expected, "Exact native16 picture {frame}");
        let tick = decoded.source_tick.ok_or("Picture timestamp absent")?;
        assert_eq!(
            i128::from(tick) * i128::from(base.numerator) * 24,
            i128::from(frame) * i128::from(base.denominator)
        );
        assert_eq!(
            decoded.timestamp_ns,
            Some((frame * 1_000_000_000 / 24) as i64)
        );
        captured.push(json!({"frame":frame,"tick":tick,"rgba16le_sha256":format!("{:x}",Sha256::digest(&decoded.rgba))}));
        expected_all.extend_from_slice(&expected);
        frame += 1;
    }
    assert_eq!(frame, FRAMES);
    assert!(reader.next_frame()?.is_none());
    let mut floats = VideoReader::open_stream_with_encoding(
        &source,
        video.index,
        cancel.clone(),
        PixelEncoding::RgbaF32Le,
    )?;
    let mut maximum_float_error = 0_f32;
    for frame in 0..FRAMES {
        let decoded = floats
            .next_frame()?
            .ok_or("Native MOV float picture absent")?;
        assert_eq!(decoded.encoding, PixelEncoding::RgbaF32Le);
        maximum_float_error =
            maximum_float_error.max(normalized_floats(&decoded.rgba, &samples16(frame))?);
    }
    assert!(floats.next_frame()?.is_none());
    let (reference, reference_evidence) = reference16(path, FRAMES)?;
    assert_eq!(
        reference, expected_all,
        "Separate software decoder lost a native16 channel"
    );
    let mut audio_reader = NativeAudioReader::open_stream(&source, audio.index, cancel.clone())?;
    assert_eq!(audio_reader.info.channels, 1);
    assert_eq!(audio_reader.info.sample_rate, 48_000);
    let mut sample = 0;
    while let Some(block) = audio_reader.next_block()? {
        assert_eq!(block.first_sample, Some(sample as i64));
        for value in block.samples {
            assert_eq!(value, pcm(sample), "Exact float PCM sample {sample}");
            sample += 1;
        }
    }
    assert_eq!(sample, FRAMES * SAMPLES_PER_FRAME);
    source.verify(cancel)?;
    Ok(
        json!({"source":source.fingerprint(),"frames":captured,"mono_samples":sample,"reference_rgba16le_sha256":format!("{:x}",Sha256::digest(reference)),"reference":reference_evidence,"every_u16_channel_equal":true,"every_pcm_sample_equal":true,"maximum_normalized_float_error":maximum_float_error}),
    )
}

#[test]
fn craft_video_precision_independent_png16_preserves_low_bits_straight_alpha_and_f32()
-> TestResult<()> {
    assert_eq!(PixelEncoding::Rgba8.bytes_per_pixel(), 4);
    assert_eq!(PixelEncoding::Rgba16Le.bytes_per_pixel(), 8);
    assert_eq!(PixelEncoding::RgbaF32Le.bytes_per_pixel(), 16);
    let fixture = Fixture::new("independent-png")?;
    let path = fixture.directory.join("original-rgba16.png");
    independent_png(&path, 16)?;
    let expected = samples16(0);
    assert!(expected.iter().any(|value| value % 257 != 0));
    for alpha in [0, 1, 257, 32767, 32768, 65534, 65535] {
        assert!(expected.chunks_exact(4).any(|pixel| pixel[3] == alpha));
    }
    let expected_bytes = little_endian16(&expected);
    let mut native = VideoReader::open_with_encoding(&path, PixelEncoding::Rgba16Le)?;
    let decoded = native
        .next_frame()?
        .ok_or("Independent PNG picture absent")?;
    assert_eq!(decoded.encoding, PixelEncoding::Rgba16Le);
    assert_eq!(decoded.rgba, expected_bytes);
    assert!(native.next_frame()?.is_none());
    let (reference, reference_evidence) = reference16(&path, 1)?;
    assert_eq!(reference, expected_bytes);
    let mut floats = VideoReader::open_with_encoding(&path, PixelEncoding::RgbaF32Le)?;
    let decoded_float = floats.next_frame()?.ok_or("Float decoded PNG absent")?;
    assert_eq!(decoded_float.encoding, PixelEncoding::RgbaF32Le);
    let maximum_float_error = normalized_floats(&decoded_float.rgba, &expected)?;
    assert!(floats.next_frame()?.is_none());
    let mut legacy = VideoReader::open(&path)?;
    let legacy = legacy.next_frame()?.ok_or("Legacy picture absent")?;
    let mut explicit = VideoReader::open_with_encoding(&path, PixelEncoding::Rgba8)?;
    let explicit = explicit.next_frame()?.ok_or("Explicit8 picture absent")?;
    assert_eq!(legacy.encoding, PixelEncoding::Rgba8);
    assert_eq!(legacy.rgba.len(), WIDTH as usize * HEIGHT as usize * 4);
    assert_eq!(legacy.rgba, explicit.rgba);
    fixture.record("independent_png16", json!({"input":path,"rgba16le_sha256":format!("{:x}",Sha256::digest(expected_bytes)),"maximum_normalized_float_error":maximum_float_error,"old8_equals_explicit8":true,"straight_alpha_and_low_bits_equal":true,"reference":reference_evidence}))?;
    Ok(())
}

#[test]
fn craft_video_precision_native_mov_retains_16_bits_pcm_clock_and_reopened_metadata()
-> TestResult<()> {
    let fixture = Fixture::new("native-mov")?;
    let path = fixture.directory.join("native-rgba16.mov");
    let cancel = Cancellation::new()?;
    let profile = native_mov(&path, &cancel, true)?;
    let evidence = inspect_mov(&path, &cancel)?;
    let source = SourceFile::open(&path, &cancel)?;
    let probe = source.probe(cancel.clone())?;
    let selected: Vec<_> = probe.streams.iter().map(|stream| stream.index).collect();
    let ingested = source.ingest(
        "Original native16 precision".into(),
        &selected,
        cancel.clone(),
        |_, _| {},
    )?;
    let video = ingested
        .source
        .streams
        .iter()
        .find(|stream| matches!(stream.format, StreamFormat::Video { .. }))
        .ok_or("Ingested video absent")?;
    assert_eq!(
        video
            .metadata
            .get("editbay.pixel_encoding")
            .map(String::as_str),
        Some("rgba16_le")
    );
    let StreamFormat::Video {
        timing:
            PictureTiming::Variable {
                presentation_ticks,
                end_tick,
            },
        ..
    } = &video.format
    else {
        return Err("Native16 ingest did not retain decoded presentation times".into());
    };
    assert_eq!(presentation_ticks.len(), FRAMES as usize);
    for (frame, tick) in presentation_ticks.iter().enumerate() {
        assert_eq!(
            i128::from(*tick) * i128::from(video.time_base.numerator) * 24,
            frame as i128 * i128::from(video.time_base.denominator)
        );
    }
    assert_eq!(
        i128::from(*end_tick) * i128::from(video.time_base.numerator) * 24,
        i128::from(FRAMES) * i128::from(video.time_base.denominator)
    );
    let mut editor = DocumentEditor::new(Project::new("Precision metadata save/reopen")?)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Retain native16 source".into(),
        &ingested.commands(),
    )?;
    let png8 = fixture.directory.join("original-rgba8.png");
    independent_png(&png8, 8)?;
    let source8 = SourceFile::open(&png8, &cancel)?;
    let probe8 = source8.probe(cancel.clone())?;
    let video8 = probe8
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video)
        .ok_or("PNG8 video absent")?;
    let ingested8 = source8.ingest(
        "Original8 compatibility".into(),
        &[video8.index],
        cancel.clone(),
        |_, _| {},
    )?;
    assert_eq!(
        ingested8.source.streams[0]
            .metadata
            .get("editbay.pixel_encoding")
            .map(String::as_str),
        Some("rgba8")
    );
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Retain explicit8 source".into(),
        &ingested8.commands(),
    )?;
    let native = fixture.directory.join("retained.editbay");
    editbay_core::save_new(editor.project(), &native)?;
    assert_eq!(editbay_core::load(&native)?, *editor.project());
    source.verify(&cancel)?;
    source8.verify(&cancel)?;
    fixture.record("native_mov16", json!({"input":path,"profile":profile,"readback":evidence,"native_project":native,"sources":editor.project().sources,"assets":editor.project().assets,"invalid_picture_lengths_rejected_before_valid_frame0":true,"saved_reopened_equal":true,"native_float_source_metadata":"not proven; F32 output tested from original integer16 PNG"}))?;
    Ok(())
}

#[test]
fn craft_video_precision_cancelled_native16_reads_and_writes_fail_before_media_output()
-> TestResult<()> {
    let fixture = Fixture::new("cancellation")?;
    let cancel = Cancellation::new()?;
    let path = fixture.directory.join("source.mov");
    let profile = native_mov(&path, &cancel, false)?;
    let source = SourceFile::open(&path, &cancel)?;
    let probe = source.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.kind == StreamType::Video)
        .ok_or("Source picture absent")?;
    for encoding in [PixelEncoding::Rgba16Le, PixelEncoding::RgbaF32Le] {
        let stopped = Cancellation::new()?;
        stopped.cancel();
        assert!(matches!(
            VideoReader::open_stream_with_encoding(&source, video.index, stopped, encoding),
            Err(Error::Cancelled)
        ));
        let stopped = Cancellation::new()?;
        let mut reader = VideoReader::open_stream_with_encoding(
            &source,
            video.index,
            stopped.clone(),
            encoding,
        )?;
        assert!(reader.next_frame()?.is_some());
        stopped.cancel();
        assert!(matches!(reader.next_frame(), Err(Error::Cancelled)));
        assert!(matches!(reader.next_frame(), Err(Error::Cancelled)));
    }
    let stopped = Cancellation::new()?;
    stopped.cancel();
    assert!(matches!(
        source.ingest(
            "Cancelled source".into(),
            &[video.index],
            stopped.clone(),
            |_, _| {}
        ),
        Err(Error::Cancelled)
    ));
    let unpublished = fixture.directory.join("cancelled.mov");
    let file = new_file(&unpublished)?;
    assert!(matches!(
        LosslessMovWriter::encoded(&file, profile, DeliveryFormat::Lossless16Mov, stopped),
        Err(Error::Cancelled)
    ));
    assert_eq!(file.metadata()?.len(), 0);
    source.verify(&cancel)?;
    fixture.record("native16_cancellation", json!({"input":path,"source":source.fingerprint(),"pre_cancelled_reader_rejected":true,"running_read_cancelled_permanently":true,"cancelled_ingest_rejected":true,"pre_cancelled_writer_bytes":0}))?;
    Ok(())
}
