use std::path::PathBuf;
use std::process::Command;

use media_ffmpeg::{Rational, decode_video_frame_near_seconds, probe_media, rescale};

fn make_sample_video() -> PathBuf {
    let output = std::env::temp_dir().join(format!(
        "cutit-step1-{}-{}.mp4",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos()
    ));

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "1.2",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&output)
        .output()
        .expect("ffmpeg must be installed to run tests");

    assert!(
        status.status.success(),
        "ffmpeg command must succeed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    output
}

#[test]
fn probe_media_finds_video_audio_and_time_base() {
    let sample = make_sample_video();

    let info = probe_media(&sample).expect("probe should succeed");

    assert_eq!(info.streams.len(), 2);

    let video = info.first_video().expect("video stream should exist");
    assert_eq!(video.width, Some(160));
    assert_eq!(video.height, Some(90));
    assert!(video.time_base.den > 0);

    let audio = info.first_audio().expect("audio stream should exist");
    assert_eq!(audio.sample_rate, Some(48_000));
    assert_eq!(audio.channels, Some(1));
}

#[test]
fn decode_video_frame_uses_best_effort_timestamp_and_returns_rgba() {
    let sample = make_sample_video();
    let at_seconds: f64 = 0.5;

    let frame =
        decode_video_frame_near_seconds(&sample, at_seconds).expect("frame decode should succeed");

    assert_eq!(frame.width, 160);
    assert_eq!(frame.height, 90);
    assert_eq!(frame.rgba.len(), (160 * 90 * 4) as usize);

    let target_tl = (at_seconds * 1_000_000.0).round() as i64;
    let target_video_ts = rescale(target_tl, Rational::MICROS, frame.time_base);
    assert!(
        frame.best_effort_timestamp >= target_video_ts,
        "decoded frame must be at-or-after requested timestamp"
    );
}

#[test]
fn rescale_preserves_non_integer_rate_precision() {
    let one_second_in_90k = 90_000;
    let micros = rescale(
        one_second_in_90k,
        Rational::new(1, 90_000).expect("valid rational"),
        Rational::MICROS,
    );
    assert_eq!(micros, 1_000_000);

    let ntsc_frame = 1_001;
    let micros = rescale(
        ntsc_frame,
        Rational::new(1, 30_000).expect("valid rational"),
        Rational::MICROS,
    );
    assert_eq!(micros, 33_367);
}
