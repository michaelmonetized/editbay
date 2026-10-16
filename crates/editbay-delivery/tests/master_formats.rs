use editbay_core::{
    DocumentCommand, DocumentEditor, DocumentVersion, EvaluationSnapshot, FrameRange, FrameRate,
    OutputTransfer, Project, SourcePosition, load, save_new, sequence_from_video_with_audio,
};
use editbay_delivery::{DeliveryControl, DeliveryFormat, DeliveryRequest, Phase, deliver};
use editbay_media::{
    Cancellation, DeliveryColor, LosslessMovProfile, LosslessMovWriter, NativeAudioReader,
    PictureBudget, PixelEncoding, SourceFile, StreamType, VideoReader,
};
use editbay_render::{GraphBudget, GraphRenderer};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use uuid::Uuid;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const WIDTH: u32 = 256;
const HEIGHT: u32 = 128;
const FRAMES: u64 = 6;
const RATE: u32 = 48_000;
const PCM_STEP: f64 = 1. / 8_388_608.;
const DEADLINE: Duration = Duration::from_secs(60);

/// Bind the native CLI to a successful clean serialized build and CPU-only environment.
/// Takes explicit environment paths; returns the actual executable and compiler evidence.
fn bound_cli() -> TestResult<(PathBuf, Value)> {
    assert_eq!(std::env::var("WGPU_BACKEND").as_deref(), Ok("vulkan"));
    assert_eq!(
        std::env::var("VK_DRIVER_FILES").as_deref(),
        Ok("/usr/share/vulkan/icd.d/lvp_icd.json")
    );
    assert!(Path::new("/usr/share/vulkan/icd.d/lvp_icd.json").is_file());
    assert!(matches!(
        std::env::var("LP_NUM_THREADS").as_deref(),
        Ok("1" | "2")
    ));
    assert!(std::env::temp_dir().canonicalize()?.starts_with("/home"));
    let cli = PathBuf::from(
        std::env::var_os("EDITBAY_TEST_CLI")
            .ok_or("EDITBAY_TEST_CLI is required; missing runtime is a failure")?,
    )
    .canonicalize()?;
    assert!(cli.is_file() && cli.metadata()?.permissions().mode() & 0o111 != 0);
    let cancel = Cancellation::new()?;
    let actual = SourceFile::open(&cli, &cancel)?;
    let packet = PathBuf::from(
        std::env::var_os("EDITBAY_TEST_CLI_BUILD_RECEIPT")
            .ok_or("Exact CLI compiler receipt is required")?,
    );
    let mut binding: Value = serde_json::from_slice(&fs::read(&packet)?)?;
    let git = Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()?;
    assert!(git.status.success());
    let revision = String::from_utf8(git.stdout)?;
    let compiled = binding["revision"]
        .as_str()
        .ok_or("Missing compiler revision")?;
    assert!(compiled.len() == 40 && compiled.bytes().all(|c| c.is_ascii_hexdigit()));
    if compiled != revision.trim() {
        let diff = Command::new("git")
            .args([
                "-C",
                env!("CARGO_MANIFEST_DIR"),
                "diff",
                "--name-only",
                compiled,
                revision.trim(),
                "--",
            ])
            .output()?;
        assert!(diff.status.success());
        let changed = String::from_utf8(diff.stdout)?;
        assert!(
            changed
                .lines()
                .all(|p| p.starts_with("crates/editbay-delivery/tests/")
                    || p.starts_with("crates/editbay-media/tests/")),
            "CLI source differs from compiler receipt: {changed}"
        );
    }
    assert_eq!(binding["repository"], "editbay");
    assert_eq!(binding["exit_code"], 0);
    assert_eq!(binding["dirty_before"], false);
    assert_eq!(binding["dirty_after"], false);
    assert!(binding["signal"].is_null());
    assert!(
        binding["receipt_errors"]
            .as_array()
            .ok_or("Missing receipt errors")?
            .is_empty()
    );
    let executable = binding["executables"]
        .as_array()
        .ok_or("Missing executable bindings")?
        .iter()
        .find(|entry| {
            entry["target"]["name"] == "editbay"
                && entry["path"].as_str().is_some_and(|p| Path::new(p) == cli)
        })
        .ok_or("Compiler receipt does not bind this CLI")?;
    assert_eq!(executable["sha256"], actual.fingerprint().sha256);
    assert_eq!(executable["size"], actual.fingerprint().bytes);
    binding["test_source_revision"] = json!(revision.trim());
    binding["actual_cli_fingerprint"] = json!(actual.fingerprint());
    binding["actual_cli_compiler_packet"] = json!(packet);
    Ok((cli, binding))
}

/// Require actual software adapter evidence, rather than inferring it from configuration.
/// `description` comes from the graph or worker receipt; returns only for CPU Vulkan.
fn assert_cpu(description: &str) {
    let text = description.to_ascii_lowercase();
    assert!(
        text.contains("device_type: cpu")
            && text.contains("backend: vulkan")
            && (text.contains("llvmpipe") || text.contains("lavapipe")),
        "Expected CPU lavapipe, got {description}"
    );
}

struct Deadline {
    expired: Arc<AtomicBool>,
    media: Arc<Mutex<Vec<Cancellation>>>,
    jobs: Arc<Mutex<Vec<DeliveryControl>>>,
    done: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}
impl Deadline {
    /// Own bounded cancellation of all real media and delivery work in one case.
    /// Takes no arguments; returns a joined sixty-second watchdog without opening devices.
    fn new() -> TestResult<Self> {
        let expired = Arc::new(AtomicBool::new(false));
        let media = Arc::new(Mutex::new(Vec::<Cancellation>::new()));
        let jobs = Arc::new(Mutex::new(Vec::<DeliveryControl>::new()));
        let (done, completion) = mpsc::channel();
        let (state, tokens, controls) = (expired.clone(), media.clone(), jobs.clone());
        let worker = std::thread::Builder::new()
            .name("master-formats-deadline".into())
            .spawn(move || {
                if completion.recv_timeout(DEADLINE).is_err() {
                    state.store(true, Ordering::Release);
                    for token in tokens.lock().unwrap().iter() {
                        token.cancel();
                    }
                    for control in controls.lock().unwrap().iter() {
                        control.cancel();
                    }
                }
            })?;
        Ok(Self {
            expired,
            media,
            jobs,
            done: Some(done),
            worker: Some(worker),
        })
    }

    /// Register a fresh media generation so dropping a renderer cannot cancel later reads.
    /// Takes no arguments; returns deadline-owned native cancellation.
    fn media(&self) -> TestResult<Cancellation> {
        let token = Cancellation::new()?;
        self.media.lock().unwrap().push(token.clone());
        if self.expired.load(Ordering::Acquire) {
            token.cancel();
        }
        token.check()?;
        Ok(token)
    }

    /// Register a real child lifetime before allowing the supervisor to start it.
    /// Takes no arguments; returns fresh control cancelled immediately after expiry.
    fn control(&self) -> TestResult<DeliveryControl> {
        let control = DeliveryControl::new()?;
        self.jobs.lock().unwrap().push(control.clone());
        if self.expired.load(Ordering::Acquire) {
            control.cancel();
        }
        Ok(control)
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Allocate unique test-only storage beneath /home, retaining it only when requested.
/// `label` selects evidence names; returns the directory and its optional cleanup owner.
fn directory(label: &str) -> TestResult<(PathBuf, Option<tempfile::TempDir>)> {
    let requested = std::env::var_os("EDITBAY_WORKFLOW_RECEIPTS_DIR");
    let parent = requested
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts"));
    assert!(parent.starts_with("/home"));
    fs::create_dir_all(&parent)?;
    let parent = parent.canonicalize()?;
    assert!(parent.starts_with("/home"));
    if requested.is_some() {
        let path = parent.join(format!("master-{label}-{}", Uuid::new_v4()));
        fs::create_dir(&path)?;
        eprintln!("Master format evidence: {}", path.display());
        Ok((path, None))
    } else {
        let temporary = tempfile::Builder::new()
            .prefix("master-formats-")
            .tempdir_in(parent)?;
        Ok((temporary.path().into(), Some(temporary)))
    }
}

/// Compute the unchanged composition-grid sample boundary independently of media helpers.
/// `frame` and `rate` describe the source clock; returns the exclusive 48 kHz boundary.
fn boundary(frame: u64, rate: FrameRate) -> u64 {
    (u128::from(frame) * u128::from(RATE) * u128::from(rate.denominator))
        .div_ceil(u128::from(rate.numerator)) as u64
}

/// Generate original, opaque, low-frequency BT.709 bars and a gray ramp with low bits.
/// `frame`, `x`, `y` select a known sample; returns exact straight source RGBA16 codes.
fn pixel(frame: u64, x: u32, y: u32, opaque: bool) -> [u16; 4] {
    let base = if x < 128 {
        [
            [0.15, 0.25, 0.40],
            [0.40, 0.20, 0.20],
            [0.20, 0.50, 0.30],
            [0.50, 0.50, 0.50],
        ][(x / 32) as usize]
    } else {
        [0.12 + 0.50 * f64::from(x - 128) / 127.; 3]
    };
    let lift = frame as f64 * 0.04 + f64::from(y) / 127. * 0.025;
    let rgb = base.map(|v| ((v + lift) * 65535.).round() as u16);
    [
        rgb[0],
        rgb[1],
        rgb[2],
        if opaque { u16::MAX } else { 32768 },
    ]
}

/// Generate independently indexed float tones with both full-scale PCM endpoints.
/// `sample` and `channel` are absolute source ordinals; returns finite unquantized float PCM.
fn tone(sample: u64, channel: usize) -> f32 {
    if sample.is_multiple_of(257) {
        return 1.;
    }
    if sample.is_multiple_of(263) {
        return -1.;
    }
    let phase = sample as f64 * std::f64::consts::TAU;
    (if channel == 0 {
        0.18 * (phase / 96.).sin() + 0.025 * (phase / 31.).cos()
    } else {
        -0.22 * (phase / 79.).cos() + 0.017 * (phase / 43.).sin()
    }) as f32
}

fn aac_tone(sample: u64, channel: usize) -> f32 {
    let phase = sample as f64 * std::f64::consts::TAU;
    (if channel == 0 {
        0.18 * (phase / 96.).sin()
    } else {
        -0.22 * (phase / 79.).cos()
    }) as f32
}

fn hdr_gray(code: u16, transfer: i32) -> u16 {
    let source = f64::from(code) / 65535.;
    let linear = if source < 0.081 {
        source / 4.5
    } else {
        ((source + 0.099) / 1.099).powf(1. / 0.45)
    };
    let output = if transfer == 16 {
        let power = (linear * 100. / 10000.).powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * power) / (1. + 2392. / 128. * power)).powf(2523. / 32.)
    } else {
        let scene = (linear * 100. / 1000.).powf(1. / 1.2);
        if scene <= 1. / 12. {
            (3. * scene).sqrt()
        } else {
            let a = 0.17883277_f64;
            let b = 1. - 4. * a;
            a * (12. * scene - b).ln() + 0.5 - a * (4. * a).ln()
        }
    };
    (output * 65535.).round() as u16
}

/// Author a real high-depth source and a saved linked native sequence through shared commands.
/// `path`, rate/layout and failure switches define original input; returns reopened project and root.
fn author(
    path: &Path,
    rate: FrameRate,
    channels: usize,
    opaque: bool,
    headroom: Option<f32>,
    cancel: &Cancellation,
) -> TestResult<(Arc<Project>, Uuid)> {
    author_signal(path, rate, channels, opaque, headroom, cancel, false)
}

fn author_signal(
    path: &Path,
    rate: FrameRate,
    channels: usize,
    opaque: bool,
    headroom: Option<f32>,
    cancel: &Cancellation,
    aac: bool,
) -> TestResult<(Arc<Project>, Uuid)> {
    let profile = LosslessMovProfile {
        width: WIDTH,
        height: HEIGHT,
        frame_rate: rate,
        first_frame: 0,
        frames: FRAMES,
        sample_rate: RATE,
        channels: if channels == 1 {
            vec!["FC".into()]
        } else {
            vec!["FL".into(), "FR".into()]
        },
    };
    let color = DeliveryColor {
        primaries: 1,
        transfer: 1,
        hdr_metadata: Default::default(),
    };
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)?;
    let mut writer = LosslessMovWriter::encoded_color(
        &file,
        profile.clone(),
        DeliveryFormat::Lossless16Mov,
        color,
        cancel.clone(),
    )?;
    let mut first = 0;
    for frame in 0..FRAMES {
        let pixels: Vec<u8> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).flat_map(move |x| pixel(frame, x, y, opaque)))
            .flat_map(u16::to_le_bytes)
            .collect();
        writer.picture(frame, &pixels)?;
        let end = boundary(frame + 1, rate);
        let samples: Vec<f32> = (first..end)
            .flat_map(|n| {
                (0..channels).map(move |channel| {
                    headroom.unwrap_or_else(|| {
                        if aac {
                            aac_tone(n, channel)
                        } else {
                            tone(n, channel)
                        }
                    })
                })
            })
            .collect();
        writer.sound(first, &samples)?;
        first = end;
    }
    writer.finish()?;
    file.sync_all()?;
    let source = SourceFile::open(path, cancel)?;
    let imported = source.ingest(
        "Original master-format bars and tones".into(),
        &[0, 1],
        cancel.clone(),
        |_, _| {},
    )?;
    let mut editor = DocumentEditor::new(Project::new("Native DNxHR master acceptance")?)?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Retain actual source".into(),
        &imported.commands(),
    )?;
    let commands =
        sequence_from_video_with_audio(editor.project(), imported.source.id, 0, 1, rate)?;
    let root = commands
        .iter()
        .find_map(|command| match command {
            DocumentCommand::SetComposition { composition } if composition.picture.is_some() => {
                Some(composition.id)
            }
            _ => None,
        })
        .ok_or("Authored picture root absent")?;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Create linked sequence".into(),
        &commands,
    )?;
    let mut configuration = editor.project().color;
    configuration.output_transfer = OutputTransfer::Bt709;
    editor.apply(
        DocumentVersion::of(editor.project()),
        "Declare actual BT.709 output".into(),
        &[DocumentCommand::SetColor { configuration }],
    )?;
    let native = path.with_extension("editbay");
    save_new(editor.project(), &native)?;
    let reopened = load(&native)?;
    assert_eq!(reopened, *editor.project());
    assert_eq!(
        reopened.sources[0].streams[0].metadata["editbay.pixel_encoding"],
        "rgba16_le"
    );
    assert_eq!(reopened.color.output_transfer, OutputTransfer::Bt709);
    Ok((Arc::new(reopened), root))
}

/// Inspect actual decoded clocks, opaque high-depth picture and quantized sample endpoints.
/// `path`, selected range/rate/layout and native cancellation define the exact expected master.
fn inspect(
    path: &Path,
    format: DeliveryFormat,
    rate: FrameRate,
    range: FrameRange,
    channels: usize,
    cancel: &Cancellation,
    color: DeliveryColor,
) -> TestResult<Value> {
    let source = SourceFile::open(path, cancel)?;
    let probe = source.probe(cancel.clone())?;
    let video = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamType::Video)
        .ok_or("Delivered picture absent")?;
    let audio = probe
        .streams
        .iter()
        .find(|s| s.kind == StreamType::Audio)
        .ok_or("Delivered sound absent")?;
    assert_eq!(video.codec, if format.aac() { "hevc" } else { "dnxhd" });
    assert_eq!(
        video.metadata["editbay.codec_profile_name"],
        if format.aac() { "Main 10" } else { "DNXHR HQX" }
    );
    assert_eq!(
        video.metadata["editbay.pixel_format_name"],
        if format.aac() {
            "yuv420p10le"
        } else {
            "yuv422p10le"
        }
    );
    assert_eq!(video.metadata["editbay.component_depth"], "10");
    assert_eq!((video.width, video.height), (Some(WIDTH), Some(HEIGHT)));
    assert_eq!(
        (
            video.color.primaries,
            video.color.transfer,
            video.color.matrix,
            video.color.range
        ),
        (
            color.primaries,
            color.transfer,
            if color.primaries == 9 { 9 } else { 1 },
            1
        )
    );
    assert_eq!(video.nominal_rate, Some(rate));
    assert_eq!(audio.codec, if format.aac() { "aac" } else { "pcm_s24le" });
    assert_eq!(audio.sample_rate, Some(RATE));
    assert_eq!(
        audio.channels,
        if channels == 1 {
            vec!["FC"]
        } else {
            vec!["FL", "FR"]
        }
    );
    let base = video
        .time_base
        .ok_or("Delivered picture time base absent")?;
    let mut reader = VideoReader::open_stream_with_encoding(
        &source,
        video.index,
        cancel.clone(),
        PixelEncoding::Rgba16Le,
    )?;
    let (mut count, mut maximum, mut squared, mut measured, mut non8bit, mut maximum_luma) =
        (0_u64, 0_u16, 0_f64, 0_u64, 0_u64, 0_f64);
    while let Some(decoded) = reader.next_frame()? {
        assert!(count < range.end - range.start, "Extra delivered picture");
        let tick = decoded.source_tick.ok_or("Picture timestamp absent")?;
        assert_eq!(
            i128::from(tick) * i128::from(base.numerator) * i128::from(rate.numerator),
            i128::from(count) * i128::from(rate.denominator) * i128::from(base.denominator)
        );
        assert_eq!(decoded.encoding, PixelEncoding::Rgba16Le);
        assert_eq!(decoded.rgba.len(), (WIDTH * HEIGHT * 8) as usize);
        for (index, bytes) in decoded.rgba.as_chunks::<8>().0.iter().enumerate() {
            let x = index as u32 % WIDTH;
            let mut expected = pixel(range.start + count, x, index as u32 / WIDTH, true);
            let actual: [u16; 4] =
                std::array::from_fn(|c| u16::from_le_bytes([bytes[c * 2], bytes[c * 2 + 1]]));
            assert_eq!(actual[3], u16::MAX);
            if [16, 18].contains(&color.transfer) {
                if x < 136 {
                    continue;
                }
                let gray = hdr_gray(expected[0], color.transfer);
                expected = [gray, gray, gray, u16::MAX];
            }
            let error =
                std::array::from_fn::<_, 3, _>(|c| f64::from(actual[c]) - f64::from(expected[c]));
            let luma = (error[0] * 0.2126 + error[1] * 0.7152 + error[2] * 0.0722).abs() / 65535.;
            maximum_luma = maximum_luma.max(luma);
            // 4:2:2 mixes chroma at bar edges; interior samples still need tight RGB bounds.
            let interior = [32_i64, 64, 96, 128]
                .into_iter()
                .all(|edge| (i64::from(x) - edge).abs() > 3);
            if interior {
                for (actual, expected) in actual[..3].iter().zip(&expected[..3]) {
                    maximum = maximum.max(actual.abs_diff(*expected));
                    let error = f64::from(*actual) - f64::from(*expected);
                    squared += error * error;
                    measured += 1;
                    non8bit += u64::from(!actual.is_multiple_of(257));
                }
            }
        }
        count += 1;
    }
    assert_eq!(count, range.end - range.start);
    let rms = (squared / measured as f64).sqrt();
    assert!(
        maximum <= if format.aac() { 1024 } else { 512 }
            && rms <= if format.aac() { 256. } else { 128. }
            && maximum_luma <= if format.aac() { 0.02 } else { 0.01 },
        "RGB max {maximum}, RMS {rms}, luma {maximum_luma}"
    );
    assert!(
        non8bit > measured / 2,
        "No meaningful high-depth delivered component distribution"
    );
    let mut reader = NativeAudioReader::open_stream(&source, audio.index, cancel.clone())?;
    assert_eq!(
        (reader.info.sample_rate, reader.info.channels),
        (RATE as i32, channels as i32)
    );
    let start = boundary(range.start, rate);
    let total = boundary(range.end, rate) - start;
    let (mut samples, mut maximum_pcm_error, mut positive_endpoints, mut negative_endpoints) =
        (0_u64, 0_f64, 0_u64, 0_u64);
    let (mut padding, mut pcm_squared) = (0_u64, 0_f64);
    let mut lag_squared = [0_f64; 17];
    let mut lag_samples = 0_u64;
    let mut endpoints = Vec::new();
    while let Some(block) = reader.next_block()? {
        assert_eq!(block.first_sample, Some((samples + padding) as i64));
        assert!(block.samples.len().is_multiple_of(channels));
        for frame in block.samples.chunks_exact(channels) {
            if samples >= total {
                assert!(format.aac(), "Unexpected MXF/PCM padding");
                padding += 1;
                assert!(padding <= 1023, "Excess AAC padding");
                continue;
            }
            for (channel, actual) in frame.iter().enumerate() {
                let original = f64::from(if format.aac() {
                    aac_tone(start + samples, channel)
                } else {
                    tone(start + samples, channel)
                });
                let expected = if format.aac() {
                    original
                } else {
                    (original / PCM_STEP).round().clamp(-8_388_608., 8_388_607.) * PCM_STEP
                };
                let error = (f64::from(*actual) - expected).abs();
                assert!(
                    actual.is_finite() && error <= if format.aac() { 0.08 } else { PCM_STEP },
                    "Sample {samples}/{channel}: {actual} != PCM24 {expected}"
                );
                maximum_pcm_error = maximum_pcm_error.max(error);
                pcm_squared += error * error;
                if format.aac() && samples >= 128 && samples + 128 < total {
                    for (index, squared) in lag_squared.iter_mut().enumerate() {
                        let shifted = (start + samples) as i64 + index as i64 - 8;
                        let reference = f64::from(aac_tone(u64::try_from(shifted)?, channel));
                        *squared += (f64::from(*actual) - reference).powi(2);
                    }
                    lag_samples += 1;
                }
                positive_endpoints += u64::from(original == 1.);
                negative_endpoints += u64::from(original == -1.);
                if samples == 0 || samples + 1 == total {
                    endpoints.push(json!({"sample":samples,"source_sample":start+samples,"channel":channel,"original":original,"quantized":expected,"actual":actual}));
                }
            }
            samples += 1;
        }
    }
    assert_eq!(samples, total);
    if format.pcm24() {
        assert!(positive_endpoints > 0 && negative_endpoints > 0);
    }
    let pcm_rms = (pcm_squared / (samples as f64 * channels as f64)).sqrt();
    assert!(pcm_rms <= if format.aac() { 0.02 } else { PCM_STEP });
    let best_lag = if format.aac() {
        assert!(lag_samples > 512);
        let best = lag_squared
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0 as i64
            - 8;
        assert_eq!(
            best, 0,
            "AAC analytic sample phase shifted; squared errors {lag_squared:?}"
        );
        Some(best)
    } else {
        None
    };
    source.verify(cancel)?;
    Ok(
        json!({"probe":probe,"frames":count,"samples":samples,"padding":padding,"pcm_rms_error":pcm_rms,"analytic_best_lag_samples":best_lag,"lag_squared_errors":if format.aac() {Some(lag_squared)} else {None},"lag_samples":lag_samples,"source_sample_start":start,"source_sample_end":start+total,"maximum_interior_code_error":maximum,"interior_rms_code_error":rms,"maximum_luma_error":maximum_luma,"interior_non8bit_components":non8bit,"maximum_pcm_error":maximum_pcm_error,"pcm24_step":if format.pcm24() {Some(PCM_STEP)} else {None},"positive_endpoints":positive_endpoints,"negative_endpoints":negative_endpoints,"endpoints":endpoints}),
    )
}

/// Exercise real native source, saved float graph and supervised DNxHR delivery in one case.
/// `format`, layout and rate/range select a bounded profile; errors retain honest failed evidence.
fn run_case(
    format: DeliveryFormat,
    label: &str,
    channels: usize,
    rate: FrameRate,
    range: FrameRange,
) -> TestResult<()> {
    run_case_color(format, label, channels, rate, range, None)
}

fn run_case_color(
    format: DeliveryFormat,
    label: &str,
    channels: usize,
    rate: FrameRate,
    range: FrameRange,
    hdr: Option<OutputTransfer>,
) -> TestResult<()> {
    let (directory, _cleanup) = directory(label)?;
    let (cli, binding) = bound_cli()?;
    let deadline = Deadline::new()?;
    let result = (|| -> TestResult<Value> {
        let cancel = deadline.media()?;
        let input = directory.join("original.mov");
        let (mut project, composition) =
            author_signal(&input, rate, channels, true, None, &cancel, format.aac())?;
        if let Some(transfer) = hdr {
            let mut editor = DocumentEditor::new((*project).clone())?;
            let mut configuration = project.color;
            configuration.output_gamut = editbay_core::WorkingGamut::Bt2020;
            configuration.output_transfer = transfer;
            editor.apply(
                DocumentVersion::of(editor.project()),
                "Declare HEVC HDR output".into(),
                &[DocumentCommand::SetColor { configuration }],
            )?;
            let edited = editor.project().clone();
            editor.undo(DocumentVersion::of(editor.project()))?;
            assert_eq!(editor.project().color, project.color);
            editor.redo(DocumentVersion::of(editor.project()))?;
            assert_eq!(editor.project().color, edited.color);
            let path = directory.join("hdr.editbay");
            save_new(editor.project(), &path)?;
            project = Arc::new(load(&path)?);
            assert_eq!(*project, *editor.project());
        }
        let input_identity = SourceFile::open(&input, &cancel)?.fingerprint().clone();
        let mut graph = GraphRenderer::new(
            Arc::new(EvaluationSnapshot::new(project.clone())?),
            PictureBudget::default(),
            GraphBudget::default(),
            deadline.media()?,
        )?;
        assert_cpu(&format!("{:?}", graph.adapter));
        let frame = graph.render(
            composition,
            SourcePosition::new(range.start as i64, 1)?,
            false,
        )?;
        let actual = graph.readback(&frame)?;
        assert_eq!(actual.len(), (WIDTH * HEIGHT * 4) as usize);
        let mut maximum_working_error = 0_f64;
        for (index, actual) in actual.as_chunks::<4>().0.iter().enumerate() {
            let codes = pixel(
                range.start,
                index as u32 % WIDTH,
                index as u32 / WIDTH,
                true,
            );
            for (actual, code) in actual[..3].iter().zip(&codes[..3]) {
                let encoded = f64::from(*code) / 65535.;
                let expected = if encoded < 0.081 {
                    encoded / 4.5
                } else {
                    ((encoded + 0.099) / 1.099).powf(1. / 0.45)
                };
                maximum_working_error =
                    maximum_working_error.max((f64::from(*actual) - expected).abs());
            }
            assert_eq!(actual[3], 1.);
        }
        assert!(
            maximum_working_error < 0.0002,
            "BT.709 linear error {maximum_working_error}"
        );
        drop(graph);
        let request = DeliveryRequest {
            format,
            composition,
            sample_rate: RATE,
            range: Some(range),
        };
        let master = directory.join(format!("master.{}", format.extension()));
        let receipt = deliver(
            &cli,
            project.clone(),
            request,
            &master,
            &deadline.control()?,
            |_, _| {},
        )?;
        assert_cpu(&receipt.adapter);
        assert_eq!(receipt.format, format);
        assert_eq!(receipt.version, DocumentVersion::of(&project));
        assert_eq!(receipt.profile.first_frame, range.start);
        assert_eq!(receipt.profile.frames, range.end - range.start);
        assert_eq!(receipt.profile.channels.len(), channels);
        assert_eq!(
            (receipt.color.primaries, receipt.color.transfer),
            if hdr.is_some() {
                (
                    9,
                    if hdr == Some(OutputTransfer::Pq) {
                        16
                    } else {
                        18
                    },
                )
            } else {
                (1, 1)
            }
        );
        assert!(
            receipt.picture_quality.samples > 0 && receipt.sound_quality.samples > 0,
            "Lossy picture/PCM24 QC must be measured"
        );
        let observed = inspect(
            &master,
            format,
            rate,
            range,
            channels,
            &cancel,
            receipt.color,
        )?;
        let owned = SourceFile::open(&master, &cancel)?.fingerprint().clone();
        assert_eq!(
            (receipt.file_sha256.as_str(), receipt.file_bytes),
            (owned.sha256.as_str(), owned.bytes)
        );
        let error = deliver(
            &cli,
            project.clone(),
            request,
            &master,
            &deadline.control()?,
            |_, _| {},
        )
        .expect_err("Existing master must be preserved");
        assert!(error.to_string().contains("destination already exists"));
        assert_eq!(*SourceFile::open(&master, &cancel)?.fingerprint(), owned);
        let cancelled = directory.join(format!("cancelled.{}", format.extension()));
        let control = deadline.control()?;
        let mut accepted = false;
        let result = deliver(&cli, project, request, &cancelled, &control, |status, _| {
            if (status.phase == Phase::Rendering && status.pictures >= 1)
                || status.phase == Phase::VerifyingPictures
            {
                accepted |= control.cancel();
            }
        });
        assert!(accepted, "Must cancel actual private rendered output");
        assert!(
            result
                .expect_err("Accepted cancellation must block publication")
                .to_string()
                .to_ascii_lowercase()
                .contains("cancel")
        );
        assert!(!cancelled.exists());
        assert_eq!(
            *SourceFile::open(&input, &cancel)?.fingerprint(),
            input_identity
        );
        assert!(!deadline.expired.load(Ordering::Acquire));
        Ok(
            json!({"input":input,"input_fingerprint":input_identity,"master":master,"receipt":receipt,"observed":observed,"maximum_working_error":maximum_working_error,"existing_master_preserved":true,"cancelled_private_render_unpublished":true}),
        )
    })();
    let receipt = match &result {
        Ok(evidence) => json!({"status":"passed","evidence":evidence}),
        Err(error) => {
            let text = error.to_string();
            let unavailable = text.to_ascii_lowercase().contains("encoder")
                && text.to_ascii_lowercase().contains("not found");
            json!({"status":if unavailable { "unavailable" } else { "failed" },"error":text,"unavailable_is_not_passed":true})
        }
    };
    editbay_core::write_new_bytes(
        directory.join("master-format.json"),
        &serde_json::to_vec_pretty(
            &json!({"schema":"editbay.master-formats.v1","case":label,"format":format,"binding":binding,"result":receipt,"fixture_source_sha256":format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(include_bytes!("master_formats.rs"))),"proof_boundary":"Actual native source/writer, shared float graph, clean-bound CLI supervisor and decode/QC on CPU Vulkan. Original analytic BT.709 picture/PCM oracles include selected-range endpoints and PCM24 saturation. Encode/decode share FFmpeg; no independent DNxHR/MXF conformance, external NLE, desktop/audio device or physical proof."}),
        )?,
    )?;
    result.map(|_| ())
}

#[test]
fn craft_video_master_formats_dnxhr_mov_mono() -> TestResult<()> {
    run_case(
        DeliveryFormat::DnxhrHqxMov,
        "mov-mono",
        1,
        FrameRate::new(24, 1)?,
        FrameRange { start: 1, end: 4 },
    )
}

#[test]
fn craft_video_hevc_main10_mp4_mono() -> TestResult<()> {
    run_case(
        DeliveryFormat::HevcMain10Mp4,
        "hevc-mono",
        1,
        FrameRate::new(25, 1)?,
        FrameRange { start: 1, end: 4 },
    )
}

#[test]
fn craft_video_hevc_main10_mp4_pq() -> TestResult<()> {
    run_case_color(
        DeliveryFormat::HevcMain10Mp4,
        "hevc-pq",
        1,
        FrameRate::new(25, 1)?,
        FrameRange { start: 1, end: 4 },
        Some(OutputTransfer::Pq),
    )
}

#[test]
fn craft_video_hevc_main10_mp4_hlg() -> TestResult<()> {
    run_case_color(
        DeliveryFormat::HevcMain10Mp4,
        "hevc-hlg",
        1,
        FrameRate::new(25, 1)?,
        FrameRange { start: 1, end: 4 },
        Some(OutputTransfer::Hlg),
    )
}

#[test]
fn craft_video_hevc_main10_mp4_stereo_fractional_phase() -> TestResult<()> {
    run_case(
        DeliveryFormat::HevcMain10Mp4,
        "hevc-stereo-fractional",
        2,
        FrameRate::new(30_000, 1001)?,
        FrameRange { start: 2, end: 5 },
    )
}
#[test]
fn craft_video_master_formats_dnxhr_mov_stereo() -> TestResult<()> {
    run_case(
        DeliveryFormat::DnxhrHqxMov,
        "mov-stereo",
        2,
        FrameRate::new(24, 1)?,
        FrameRange { start: 1, end: 4 },
    )
}
#[test]
fn craft_video_master_formats_dnxhr_mxf_mono() -> TestResult<()> {
    run_case(
        DeliveryFormat::DnxhrHqxMxf,
        "mxf-mono",
        1,
        FrameRate::new(24, 1)?,
        FrameRange { start: 1, end: 4 },
    )
}
#[test]
fn craft_video_master_formats_dnxhr_mxf_stereo() -> TestResult<()> {
    run_case(
        DeliveryFormat::DnxhrHqxMxf,
        "mxf-stereo",
        2,
        FrameRate::new(24, 1)?,
        FrameRange { start: 1, end: 4 },
    )
}
#[test]
fn craft_video_master_formats_mov_preserves_fractional_selected_sample_phase() -> TestResult<()> {
    run_case(
        DeliveryFormat::DnxhrHqxMov,
        "mov-fractional-phase",
        1,
        FrameRate::new(30_000, 1001)?,
        FrameRange { start: 2, end: 3 },
    )
}

#[test]
fn craft_video_master_formats_mxf_refuses_fractional_pcm_edit_before_publication() -> TestResult<()>
{
    // FFmpeg n9.0.1 mxfenc.c:2875 injects pcm_rechunk r=rate; its default pad=1
    // and round-nearest origin conflict with the native ceil/absolute source grid.
    // https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.1/libavcodec/bsf/pcm_rechunk.c
    let (directory, _cleanup) = directory("mxf-fractional-refusal")?;
    let (cli, binding) = bound_cli()?;
    let deadline = Deadline::new()?;
    let cancel = deadline.media()?;
    let rate = FrameRate::new(30_000, 1001)?;
    assert_eq!(boundary(3, rate) - boundary(2, rate), 1601);
    let (project, composition) = author(
        &directory.join("original.mov"),
        rate,
        1,
        true,
        None,
        &cancel,
    )?;
    let destination = directory.join("refused.mxf");
    let mut worker_started = false;
    let error = deliver(
        &cli,
        project,
        DeliveryRequest {
            format: DeliveryFormat::DnxhrHqxMxf,
            composition,
            sample_rate: RATE,
            range: Some(FrameRange { start: 2, end: 3 }),
        },
        &destination,
        &deadline.control()?,
        |_, _| worker_started = true,
    )
    .expect_err("Fractional MXF sample/edit phase must be refused");
    assert!(!destination.exists());
    assert!(
        !worker_started,
        "Refuse the clock before starting an output worker"
    );
    let reason = error.to_string();
    assert!(
        reason.to_ascii_lowercase().contains("mxf")
            && reason.to_ascii_lowercase().contains("fractional"),
        "Expected explicit MXF policy refusal: {reason}"
    );
    editbay_core::write_new_bytes(
        directory.join("mxf-fractional-refusal.json"),
        &serde_json::to_vec_pretty(
            &json!({"status":"passed","binding":binding,"refused_rate":rate,"range":[2,3],"source_samples":1601,"reason":reason,"not_qualified":"30000/1001 MXF","before_worker":true,"before_publication":true}),
        )?,
    )?;
    Ok(())
}

#[test]
fn craft_video_master_formats_refuse_nonopaque_and_pcm_headroom() -> TestResult<()> {
    let (directory, _cleanup) = directory("unsupported-signal")?;
    let (cli, binding) = bound_cli()?;
    let deadline = Deadline::new()?;
    let cancel = deadline.media()?;
    let mut cases = Vec::new();
    for (name, opaque, headroom) in [
        ("alpha", false, None),
        ("positive-headroom", true, Some(1.01)),
        ("negative-headroom", true, Some(-1.01)),
    ] {
        let input = directory.join(format!("{name}.mov"));
        let (project, composition) =
            author(&input, FrameRate::new(24, 1)?, 1, opaque, headroom, &cancel)?;
        for format in [DeliveryFormat::DnxhrHqxMov, DeliveryFormat::DnxhrHqxMxf] {
            let destination = directory.join(format!("refused-{name}.{}", format.extension()));
            let error = deliver(
                &cli,
                project.clone(),
                DeliveryRequest {
                    format,
                    composition,
                    sample_rate: RATE,
                    range: Some(FrameRange { start: 1, end: 2 }),
                },
                &destination,
                &deadline.control()?,
                |_, _| {},
            )
            .expect_err("Unsupported signal must fail visibly rather than flatten or clip");
            let reason = error.to_string();
            let lower = reason.to_ascii_lowercase();
            assert!(
                if headroom.is_some() {
                    lower.contains("full scale")
                        || lower.contains("headroom")
                        || lower.contains("pcm24")
                } else {
                    lower.contains("opaque") || lower.contains("alpha")
                },
                "Wrong failure: {reason}"
            );
            assert!(!destination.exists());
            cases.push(
                json!({"signal":name,"format":format,"reason":reason,"before_publication":true}),
            );
        }
    }
    editbay_core::write_new_bytes(
        directory.join("signal-refusals.json"),
        &serde_json::to_vec_pretty(&json!({"status":"passed","binding":binding,"cases":cases}))?,
    )?;
    Ok(())
}
