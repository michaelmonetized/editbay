use editbay_media::{Cancellation, SourceFile, VideoReader};
use std::process::Command;

#[test]
fn common_client_codecs_ingest_and_decode_against_independent_rgba() {
    let directory = tempfile::tempdir().unwrap();
    for (codec, extension, encoding) in [
        (
            "h264",
            "mp4",
            vec!["-c:v", "libx264", "-pix_fmt", "yuv420p"],
        ),
        (
            "hevc",
            "mp4",
            vec![
                "-c:v",
                "libx265",
                "-pix_fmt",
                "yuv420p",
                "-x265-params",
                "log-level=error:pools=1",
            ],
        ),
        (
            "prores",
            "mov",
            vec![
                "-c:v",
                "prores_ks",
                "-profile:v",
                "3",
                "-pix_fmt",
                "yuv422p10le",
            ],
        ),
        (
            "dnxhd",
            "mov",
            vec![
                "-c:v",
                "dnxhd",
                "-profile:v",
                "dnxhr_hq",
                "-pix_fmt",
                "yuv422p",
            ],
        ),
    ] {
        let path = directory.path().join(format!("{codec}.{extension}"));
        let encoded = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=256x144:rate=12:duration=0.5",
                "-vf",
                "scale=in_color_matrix=bt709:out_color_matrix=bt709",
                "-colorspace",
                "bt709",
                "-color_primaries",
                "bt709",
                "-color_trc",
                "bt709",
            ])
            .args(encoding)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            encoded.status.success(),
            "{codec}: {}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        let cancel = Cancellation::new().unwrap();
        let source = SourceFile::open(&path, &cancel).unwrap();
        let imported = source
            .ingest(codec.into(), &[0], cancel.clone(), |_, _| {})
            .unwrap();
        assert_eq!(imported.source.streams[0].codec, codec);
        let mut reader = VideoReader::open_stream(&source, 0, cancel.clone()).unwrap();
        let mut actual = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            actual.extend(frame.rgba);
        }
        assert_eq!(actual.len(), 6 * 256 * 144 * 4);
        let reference = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args([
                "-vf",
                "scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=bilinear",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(reference.status.success());
        assert_eq!(actual, reference.stdout, "{codec} native decode differs");
        source.verify(&cancel).unwrap();
    }
}
