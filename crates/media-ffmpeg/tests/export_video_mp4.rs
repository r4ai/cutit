use std::path::PathBuf;
use std::process::Command;

use media_ffmpeg::{
    Rational, VideoExportRequest, VideoExportSegment, export_video_mp4, probe_media, rescale,
};

fn make_sample_video() -> PathBuf {
    let output = std::env::temp_dir().join(format!(
        "cutit-step3-{}-{}.mp4",
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

fn make_sample_video_with_non_zero_start_pts() -> PathBuf {
    let base = make_sample_video();
    let output = std::env::temp_dir().join(format!(
        "cutit-step3-shifted-{}-{}.mp4",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos()
    ));

    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-itsoffset", "2"])
        .arg("-i")
        .arg(&base)
        .args(["-map", "0:v", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
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
fn export_video_mp4_trims_and_concatenates_video_only_segments() {
    let sample = make_sample_video();
    let probe = probe_media(&sample).expect("probe should succeed");
    let video = probe.first_video().expect("video stream should exist");
    let src_in = video.start_pts.unwrap_or(0);
    let tb = video.time_base;

    let seg0_start = src_in + rescale(200_000, Rational::MICROS, tb);
    let seg0_end = src_in + rescale(500_000, Rational::MICROS, tb);
    let seg1_start = src_in + rescale(700_000, Rational::MICROS, tb);
    let seg1_end = src_in + rescale(1_000_000, Rational::MICROS, tb);

    let output = std::env::temp_dir().join(format!(
        "cutit-step3-exported-{}-{}.mp4",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos()
    ));

    let request = VideoExportRequest {
        inputs: vec![sample],
        segments: vec![
            VideoExportSegment {
                input_index: 0,
                src_in_video: seg0_start,
                src_out_video: seg0_end,
                src_time_base: tb,
            },
            VideoExportSegment {
                input_index: 0,
                src_in_video: seg1_start,
                src_out_video: seg1_end,
                src_time_base: tb,
            },
        ],
        output_path: output.clone(),
    };

    export_video_mp4(&request).expect("export should succeed");

    let exported = probe_media(&output).expect("probe exported media should succeed");
    assert!(
        exported.first_video().is_some(),
        "video stream should exist"
    );
    assert!(
        exported.first_audio().is_none(),
        "audio stream should not exist"
    );

    let duration = exported
        .duration_seconds
        .expect("exported duration should exist");
    assert!(
        (duration - 0.6).abs() < 0.12,
        "duration must be around 0.6 sec, got {duration}"
    );
}

#[test]
fn export_video_mp4_handles_input_with_non_zero_start_pts() {
    let sample = make_sample_video_with_non_zero_start_pts();
    let probe = probe_media(&sample).expect("probe should succeed");
    let video = probe.first_video().expect("video stream should exist");
    assert_ne!(
        video.start_pts,
        Some(0),
        "test input must have non-zero start_pts"
    );
    let src_in = video.start_pts.unwrap_or(0);
    let src_out = src_in + video.duration_ts.expect("video duration should exist");

    let output = std::env::temp_dir().join(format!(
        "cutit-step3-nonzero-start-exported-{}-{}.mp4",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock must be after unix epoch")
            .as_nanos()
    ));

    let request = VideoExportRequest {
        inputs: vec![sample],
        segments: vec![VideoExportSegment {
            input_index: 0,
            src_in_video: src_in,
            src_out_video: src_out,
            src_time_base: video.time_base,
        }],
        output_path: output.clone(),
    };

    export_video_mp4(&request).expect("export should succeed");

    let exported = probe_media(&output).expect("probe exported media should succeed");
    assert!(
        exported.first_video().is_some(),
        "video stream should exist in output"
    );
}
