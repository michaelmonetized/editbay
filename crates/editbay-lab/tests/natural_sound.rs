use serde_json::Value;
use std::process::Command;

#[test]
fn real_source_sequences_preserve_pcm_delays_preroll_and_fractional_tails() {
    let dir = tempfile::tempdir().unwrap();
    for video_rate in ["24", "25", "30", "30000/1001", "24000/1001"] {
        for (rate, codec, layout, video_delay, audio_delay) in [
            (44100, "pcm_f32le", "stereo", "0", "0"),
            (48000, "pcm_f32le", "stereo", "0", "0.125"),
            (48000, "pcm_f32le", "stereo", "0.166833333333", "0"),
            (48000, "aac", "5.1", "0", "0"),
        ] {
            let path = dir.path().join(format!(
                "{}-{rate}-{codec}-{video_delay}-{audio_delay}.mov",
                video_rate.replace('/', "-")
            ));
            let expression = if layout == "stereo" {
                "sin(2*PI*137*t)|0.5*cos(2*PI*701*t)"
            } else {
                "sin(2*PI*137*t)|0.5*cos(2*PI*701*t)|0.2*sin(2*PI*463*t)|0.1*cos(2*PI*37*t)|0.3*sin(2*PI*997*t)|0.4*cos(2*PI*1103*t)"
            };
            let output = Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-itsoffset",
                    video_delay,
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc2=size=64x48:rate={video_rate}:duration=0.9"),
                    "-itsoffset",
                    audio_delay,
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("aevalsrc={expression}:s={rate}:d=1.123:c={layout}"),
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    codec,
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result = Command::new(env!("CARGO_BIN_EXE_editbay-lab"))
                .arg("natural-sound")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr)
            );
            let receipt: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(receipt["qualified"], true);
            assert_eq!(receipt["maximum_absolute_pcm_error"], 0.);
            assert!(receipt["silence_frames"].as_u64().unwrap() > 0);
            assert!(receipt["active_frames"].as_u64().unwrap() > 30000);
            if audio_delay != "0" {
                assert!(receipt["source_sample_offset"].as_i64().unwrap() < 0);
            }
            if video_delay != "0" {
                assert!(receipt["source_sample_offset"].as_i64().unwrap() > 0);
            }
        }
    }
}
