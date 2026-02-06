use std::path::PathBuf;
use std::process::Command;

use crate::error::{MediaFfmpegError, Result};
use crate::time::Rational;

/// Request payload for MP4 export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoExportRequest {
    pub inputs: Vec<PathBuf>,
    pub segments: Vec<VideoExportSegment>,
    pub audio: Option<AudioExportSettings>,
    pub output_path: PathBuf,
}

/// Audio output settings used when audio export is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioExportSettings {
    pub sample_rate: u32,
    pub channels: u16,
}

/// One segment to trim and concatenate into the output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoExportSegment {
    pub input_index: usize,
    pub src_in_video: i64,
    pub src_out_video: i64,
    pub src_video_time_base: Rational,
    pub src_in_audio: Option<i64>,
    pub src_out_audio: Option<i64>,
    pub src_audio_time_base: Option<Rational>,
}

/// Exports timeline segments into an MP4 by decode -> trim -> re-encode.
pub fn export_video_mp4(request: &VideoExportRequest) -> Result<()> {
    validate_request(request)?;
    let filter_complex = build_filter_complex(request);
    let has_audio = request.audio.is_some();
    let output_video_label = if request.segments.len() == 1 {
        "[v0]"
    } else {
        "[vout]"
    };
    let output_audio_label = if has_audio {
        Some(if request.segments.len() == 1 {
            "[a0]"
        } else {
            "[aout]"
        })
    } else {
        None
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
        .arg(output_video_label)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"]);

    if let (Some(audio), Some(output_audio_label)) = (request.audio.as_ref(), output_audio_label) {
        command
            .arg("-map")
            .arg(output_audio_label)
            .args(["-c:a", "aac", "-ar"])
            .arg(audio.sample_rate.to_string())
            .args(["-ac"])
            .arg(audio.channels.to_string());
    } else {
        command.arg("-an");
    }

    command.arg(&request.output_path);

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
    let has_audio = request.audio.is_some();
    let mut chains = Vec::<String>::with_capacity(request.segments.len() * 2 + 1);
    for (index, segment) in request.segments.iter().enumerate() {
        let video_chain = format!(
            "[{}:v:0]settb={}/{},trim=start_pts={}:end_pts={},setpts=PTS-STARTPTS[v{}]",
            segment.input_index,
            segment.src_video_time_base.num,
            segment.src_video_time_base.den,
            segment.src_in_video,
            segment.src_out_video,
            index
        );
        chains.push(video_chain);

        if has_audio {
            let output_audio = request
                .audio
                .expect("audio settings must exist when audio export is enabled");
            let output_channel_layout = channel_layout_for_channels(output_audio.channels)
                .expect("audio channels must map to a channel layout");
            let audio_tb = segment
                .src_audio_time_base
                .expect("audio time base must exist when audio export is enabled");
            let audio_chain = format!(
                "[{}:a:0]asettb={}/{},atrim=start_pts={}:end_pts={},asetpts=PTS-STARTPTS,aresample={}:async=1:first_pts=0,aformat=sample_rates={}:channel_layouts={}[a{}]",
                segment.input_index,
                audio_tb.num,
                audio_tb.den,
                segment
                    .src_in_audio
                    .expect("audio range start must exist when audio export is enabled"),
                segment
                    .src_out_audio
                    .expect("audio range end must exist when audio export is enabled"),
                output_audio.sample_rate,
                output_audio.sample_rate,
                output_channel_layout,
                index
            );
            chains.push(audio_chain);
        }
    }

    if request.segments.len() > 1 {
        let mut concat_inputs = String::new();
        for index in 0..request.segments.len() {
            if has_audio {
                concat_inputs.push_str(&format!("[v{index}][a{index}]"));
            } else {
                concat_inputs.push_str(&format!("[v{index}]"));
            }
        }
        if has_audio {
            chains.push(format!(
                "{concat_inputs}concat=n={}:v=1:a=1[vout][aout]",
                request.segments.len()
            ));
        } else {
            chains.push(format!(
                "{concat_inputs}concat=n={}:v=1:a=0[vout]",
                request.segments.len()
            ));
        }
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

    if let Some(audio) = request.audio {
        if audio.sample_rate == 0 {
            return Err(MediaFfmpegError::InvalidExportRequest {
                reason: "audio sample rate must be positive",
            });
        }
        if audio.channels == 0 {
            return Err(MediaFfmpegError::InvalidExportRequest {
                reason: "audio channels must be positive",
            });
        }
        if channel_layout_for_channels(audio.channels).is_none() {
            return Err(MediaFfmpegError::InvalidExportRequest {
                reason: "audio channel layout is unsupported",
            });
        }
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
        if request.audio.is_some() {
            let Some(src_in_audio) = segment.src_in_audio else {
                return Err(MediaFfmpegError::InvalidExportRequest {
                    reason: "audio range start is missing",
                });
            };
            let Some(src_out_audio) = segment.src_out_audio else {
                return Err(MediaFfmpegError::InvalidExportRequest {
                    reason: "audio range end is missing",
                });
            };
            if segment.src_audio_time_base.is_none() {
                return Err(MediaFfmpegError::InvalidExportRequest {
                    reason: "audio time base is missing",
                });
            }
            if src_out_audio <= src_in_audio {
                return Err(MediaFfmpegError::InvalidExportRequest {
                    reason: "audio source range is not positive",
                });
            }
        }
    }

    Ok(())
}

fn command_for_display(request: &VideoExportRequest) -> String {
    format!("ffmpeg export {}", request.output_path.display())
}

fn channel_layout_for_channels(channels: u16) -> Option<&'static str> {
    match channels {
        1 => Some("mono"),
        2 => Some("stereo"),
        3 => Some("2.1"),
        4 => Some("quad"),
        5 => Some("5.0"),
        6 => Some("5.1"),
        7 => Some("6.1"),
        8 => Some("7.1"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioExportSettings, VideoExportRequest, VideoExportSegment, build_filter_complex,
        validate_request,
    };
    use crate::{MediaFfmpegError, Rational};
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
                    src_video_time_base: Rational::new(1, 90_000).expect("valid"),
                    src_in_audio: None,
                    src_out_audio: None,
                    src_audio_time_base: None,
                },
                VideoExportSegment {
                    input_index: 0,
                    src_in_video: 120_000,
                    src_out_video: 198_000,
                    src_video_time_base: Rational::new(1, 90_000).expect("valid"),
                    src_in_audio: None,
                    src_out_audio: None,
                    src_audio_time_base: None,
                },
            ],
            audio: None,
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

    #[test]
    fn build_filter_complex_for_two_segments_with_audio_uses_av_concat() {
        let request = VideoExportRequest {
            inputs: vec![PathBuf::from("in.mp4")],
            segments: vec![
                VideoExportSegment {
                    input_index: 0,
                    src_in_video: 90_000,
                    src_out_video: 120_000,
                    src_video_time_base: Rational::new(1, 90_000).expect("valid"),
                    src_in_audio: Some(48_000),
                    src_out_audio: Some(64_000),
                    src_audio_time_base: Some(Rational::new(1, 48_000).expect("valid")),
                },
                VideoExportSegment {
                    input_index: 0,
                    src_in_video: 120_000,
                    src_out_video: 198_000,
                    src_video_time_base: Rational::new(1, 90_000).expect("valid"),
                    src_in_audio: Some(64_000),
                    src_out_audio: Some(105_600),
                    src_audio_time_base: Some(Rational::new(1, 48_000).expect("valid")),
                },
            ],
            audio: Some(AudioExportSettings {
                sample_rate: 48_000,
                channels: 2,
            }),
            output_path: PathBuf::from("out.mp4"),
        };

        let filter = build_filter_complex(&request);
        assert_eq!(
            filter,
            "[0:v:0]settb=1/90000,trim=start_pts=90000:end_pts=120000,setpts=PTS-STARTPTS[v0];\
[0:a:0]asettb=1/48000,atrim=start_pts=48000:end_pts=64000,asetpts=PTS-STARTPTS,aresample=48000:async=1:first_pts=0,aformat=sample_rates=48000:channel_layouts=stereo[a0];\
[0:v:0]settb=1/90000,trim=start_pts=120000:end_pts=198000,setpts=PTS-STARTPTS[v1];\
[0:a:0]asettb=1/48000,atrim=start_pts=64000:end_pts=105600,asetpts=PTS-STARTPTS,aresample=48000:async=1:first_pts=0,aformat=sample_rates=48000:channel_layouts=stereo[a1];\
[v0][a0][v1][a1]concat=n=2:v=1:a=1[vout][aout]"
        );
    }

    #[test]
    fn validate_request_rejects_unsupported_audio_channel_layout() {
        let request = VideoExportRequest {
            inputs: vec![PathBuf::from("in.mp4")],
            segments: vec![VideoExportSegment {
                input_index: 0,
                src_in_video: 90_000,
                src_out_video: 120_000,
                src_video_time_base: Rational::new(1, 90_000).expect("valid"),
                src_in_audio: Some(48_000),
                src_out_audio: Some(64_000),
                src_audio_time_base: Some(Rational::new(1, 48_000).expect("valid")),
            }],
            audio: Some(AudioExportSettings {
                sample_rate: 48_000,
                channels: 9,
            }),
            output_path: PathBuf::from("out.mp4"),
        };

        let result = validate_request(&request);
        assert!(matches!(
            result,
            Err(MediaFfmpegError::InvalidExportRequest {
                reason: "audio channel layout is unsupported"
            })
        ));
    }
}
