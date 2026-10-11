use crate::VideoReader;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn narrow_native_picture_conversion_preserves_output_boundaries() {
    let directory = tempdir().unwrap();
    for width in [8, 18, 258] {
        let source = directory.path().join(format!("{width}.mkv"));
        let fixture = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("color=blue:size={width}x8:rate=1"))
            .args(["-frames:v", "1", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            fixture.status.success(),
            "{}",
            String::from_utf8_lossy(&fixture.stderr)
        );
        let mut reader = VideoReader::open(&source).unwrap();
        assert_eq!(reader.info.width, width);
        let bytes = width as usize * 8 * 4;
        let mut guarded = vec![0xa5; bytes + 128];
        assert!(
            reader
                .read_picture(&mut guarded[64..64 + bytes], None)
                .unwrap()
                .is_some()
        );
        assert_eq!(&guarded[..64], &[0xa5; 64], "width {width}");
        assert_eq!(&guarded[64 + bytes..], &[0xa5; 64], "width {width}");
        let independent = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&source)
            .args(["-frames:v", "1", "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(independent.status.success());
        assert_eq!(&guarded[64..64 + bytes], independent.stdout);
    }
}
