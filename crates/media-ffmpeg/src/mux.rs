use std::path::PathBuf;
use std::process::Command;

use crate::error::{MediaFfmpegError, Result};
use crate::time::Rational;

/// Request payload for video-only export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoExportRequest {
    pub inputs: Vec<PathBuf>,
    pub segments: Vec<VideoExportSegment>,
    pub output_path: PathBuf,
}

/// One segment to trim and concatenate into the output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoExportSegment {
    pub input_index: usize,
    pub src_in_video: i64,
    pub src_out_video: i64,
    pub src_time_base: Rational,
}

/// Exports timeline segments into an MP4 video by decode -> trim -> re-encode.
pub fn export_video_mp4(request: &VideoExportRequest) -> Result<()> {
    validate_request(request)?;
    let filter_complex = build_filter_complex(request);
    let output_label = if request.segments.len() == 1 {
        "[v0]"
    } else {
        "[vout]"
    };

    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-v", "error", "-y", "-copyts"]);

    for input in &request.inputs {
        command.arg("-i").arg(input);
    }

    command
        .arg("-filter_complex")
        .arg(filter_complex)
        .arg("-map")
        .arg(output_label)
        .args(["-an", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&request.output_path);

    let output = command.output().map_err(|source| MediaFfmpegError::Io {
        context: "run ffmpeg export video",
        source,
    })?;
    if !output.status.success() {
        return Err(MediaFfmpegError::CommandFailed {
            command: command_for_display(request),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

fn build_filter_complex(request: &VideoExportRequest) -> String {
    let mut chains = Vec::<String>::with_capacity(request.segments.len() + 1);
    for (index, segment) in request.segments.iter().enumerate() {
        let chain = format!(
            "[{}:v:0]settb={}/{},trim=start_pts={}:end_pts={},setpts=PTS-STARTPTS[v{}]",
            segment.input_index,
            segment.src_time_base.num,
            segment.src_time_base.den,
            segment.src_in_video,
            segment.src_out_video,
            index
        );
        chains.push(chain);
    }

    if request.segments.len() > 1 {
        let mut concat_inputs = String::new();
        for index in 0..request.segments.len() {
            concat_inputs.push_str(&format!("[v{index}]"));
        }
        chains.push(format!(
            "{concat_inputs}concat=n={}:v=1:a=0[vout]",
            request.segments.len()
        ));
    }

    chains.join(";")
}

fn validate_request(request: &VideoExportRequest) -> Result<()> {
    if request.inputs.is_empty() {
        return Err(MediaFfmpegError::InvalidExportRequest {
            reason: "export inputs are empty",
        });
    }
    if request.segments.is_empty() {
        return Err(MediaFfmpegError::InvalidExportRequest {
            reason: "export segments are empty",
        });
    }

    for segment in &request.segments {
        if segment.input_index >= request.inputs.len() {
            return Err(MediaFfmpegError::InvalidExportRequest {
                reason: "segment input index is out of range",
            });
        }
        if segment.src_out_video <= segment.src_in_video {
            return Err(MediaFfmpegError::InvalidExportRequest {
                reason: "segment source range is not positive",
            });
        }
    }

    Ok(())
}

fn command_for_display(request: &VideoExportRequest) -> String {
    format!("ffmpeg export {}", request.output_path.display())
}

#[cfg(test)]
mod tests {
    use super::{VideoExportRequest, VideoExportSegment, build_filter_complex};
    use crate::Rational;
    use std::path::PathBuf;

    #[test]
    fn build_filter_complex_for_two_segments_uses_trim_setpts_and_concat() {
        let request = VideoExportRequest {
            inputs: vec![PathBuf::from("in.mp4")],
            segments: vec![
                VideoExportSegment {
                    input_index: 0,
                    src_in_video: 90_000,
                    src_out_video: 120_000,
                    src_time_base: Rational::new(1, 90_000).expect("valid"),
                },
                VideoExportSegment {
                    input_index: 0,
                    src_in_video: 120_000,
                    src_out_video: 198_000,
                    src_time_base: Rational::new(1, 90_000).expect("valid"),
                },
            ],
            output_path: PathBuf::from("out.mp4"),
        };

        let filter = build_filter_complex(&request);
        assert_eq!(
            filter,
            "[0:v:0]settb=1/90000,trim=start_pts=90000:end_pts=120000,setpts=PTS-STARTPTS[v0];\
[0:v:0]settb=1/90000,trim=start_pts=120000:end_pts=198000,setpts=PTS-STARTPTS[v1];\
[v0][v1]concat=n=2:v=1:a=0[vout]"
        );
    }
}
