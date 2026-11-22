//! Подбор видеокодеков и параметров кодирования.

use crate::config::Settings;
use crate::ffmpeg::available_encoders;
use crate::probe::MediaInfo;

pub struct QualityProfile {
    pub nvenc_cq: u32,
    pub x265_crf: u32,
    pub x264_crf: u32,
    pub av1_crf: u32,
    pub nvenc_preset: &'static str,
    pub x265_preset: &'static str,
    pub x264_preset: &'static str,
    pub av1_preset: &'static str,
}

pub fn quality_profile(name: &str) -> QualityProfile {
    match name {
        "max" => QualityProfile {
            nvenc_cq: 18,
            x265_crf: 17,
            x264_crf: 15,
            av1_crf: 26,
            nvenc_preset: "p7",
            x265_preset: "slower",
            x264_preset: "slower",
            av1_preset: "4",
        },
        "balanced" => QualityProfile {
            nvenc_cq: 24,
            x265_crf: 23,
            x264_crf: 21,
            av1_crf: 34,
            nvenc_preset: "p5",
            x265_preset: "medium",
            x264_preset: "medium",
            av1_preset: "7",
        },
        _ => QualityProfile {
            nvenc_cq: 21,
            x265_crf: 20,
            x264_crf: 18,
            av1_crf: 30,
            nvenc_preset: "p6",
            x265_preset: "slow",
            x264_preset: "slow",
            av1_preset: "6",
        },
    }
}

const AUTO_ORDER: [&str; 5] = ["hevc_nvenc", "h264_nvenc", "libx265", "libx264", "libsvtav1"];

pub fn resolve_encoder(preferred: &str) -> String {
    let encs = available_encoders();
    if preferred != "auto" && !preferred.is_empty() {
        if encs.contains(preferred) {
            return preferred.to_string();
        }
        let fallback = match preferred {
            "hevc_nvenc" => "libx265",
            "h264_nvenc" => "libx264",
            "libsvtav1" => "libx265",
            _ => "libx264",
        };
        if encs.contains(fallback) {
            return fallback.to_string();
        }
    }
    for name in AUTO_ORDER {
        if encs.contains(name) {
            return name.to_string();
        }
    }
    "libx264".to_string()
}

pub fn is_10bit_capable(encoder: &str) -> bool {
    matches!(encoder, "hevc_nvenc" | "libx265" | "libsvtav1")
}

pub fn video_args(encoder: &str, quality: &str, ten_bit: bool) -> Vec<String> {
    let q = quality_profile(quality);
    let mut a: Vec<String> = vec!["-c:v".to_string(), encoder.to_string()];
    // (флаг, значение)
    let mut pairs: Vec<(&str, String)> = Vec::new();

    match encoder {
        "hevc_nvenc" => {
            pairs.push(("-preset", q.nvenc_preset.into()));
            pairs.push(("-tune", "hq".into()));
            pairs.push(("-rc", "vbr".into()));
            pairs.push(("-cq", q.nvenc_cq.to_string()));
            pairs.push(("-b:v", "0".into()));
            pairs.push(("-multipass", "fullres".into()));
            pairs.push(("-spatial-aq", "1".into()));
            pairs.push(("-temporal-aq", "1".into()));
            pairs.push(("-aq-strength", "8".into()));
            pairs.push(("-b_ref_mode", "middle".into()));
            pairs.push(("-profile:v", if ten_bit { "main10" } else { "main" }.into()));
            pairs.push(("-pix_fmt", if ten_bit { "p010le" } else { "yuv420p" }.into()));
        }
        "h264_nvenc" => {
            pairs.push(("-preset", q.nvenc_preset.into()));
            pairs.push(("-tune", "hq".into()));
            pairs.push(("-rc", "vbr".into()));
            pairs.push(("-cq", q.nvenc_cq.to_string()));
            pairs.push(("-b:v", "0".into()));
            pairs.push(("-multipass", "fullres".into()));
            pairs.push(("-spatial-aq", "1".into()));
            pairs.push(("-temporal-aq", "1".into()));
            pairs.push(("-aq-strength", "8".into()));
            pairs.push(("-profile:v", "high".into()));
            pairs.push(("-pix_fmt", "yuv420p".into()));
        }
        "libx265" => {
            pairs.push(("-preset", q.x265_preset.into()));
            pairs.push(("-crf", q.x265_crf.to_string()));
            pairs.push((
                "-pix_fmt",
                if ten_bit { "yuv420p10le" } else { "yuv420p" }.into(),
            ));
            pairs.push((
                "-x265-params",
                "aq-mode=3:psy-rd=2.0:psy-rdoq=2.0:rdoq-level=2:deblock=-1,-1:strong-intra-smoothing=0:limit-sao=1"
                    .into(),
            ));
        }
        "libx264" => {
            pairs.push(("-preset", q.x264_preset.into()));
            pairs.push(("-crf", q.x264_crf.to_string()));
            pairs.push(("-pix_fmt", "yuv420p".into()));
            pairs.push(("-profile:v", "high".into()));
            pairs.push(("-x264-params", "aq-mode=3:deblock=-1,-1:ref=4".into()));
        }
        "libsvtav1" => {
            pairs.push(("-preset", q.av1_preset.into()));
            pairs.push(("-crf", q.av1_crf.to_string()));
            pairs.push((
                "-pix_fmt",
                if ten_bit { "yuv420p10le" } else { "yuv420p" }.into(),
            ));
        }
        _ => {
            pairs.push(("-crf", "20".into()));
        }
    }

    for (flag, value) in pairs {
        a.push(flag.to_string());
        a.push(value);
    }
    a
}

const MP4_AUDIO_OK: [&str; 7] = ["aac", "mp3", "ac3", "eac3", "alac", "opus", "flac"];
const TEXT_SUBS: [&str; 7] = ["subrip", "srt", "ass", "ssa", "mov_text", "webvtt", "text"];

/// Турбо-профиль NVENC: самый быстрый пресет без многопроходности.
/// Нужен движку «Турбо», где важна скорость, а не последние проценты качества.
pub fn video_args_turbo(encoder: &str, quality: &str, ten_bit: bool) -> Vec<String> {
    let mut a = video_args(encoder, quality, ten_bit);
    let mut i = 0;
    while i + 1 < a.len() {
        match a[i].as_str() {
            "-preset" => {
                a[i + 1] = "p1".into();
                i += 2;
            }
            "-multipass" => {
                a.remove(i);
                a.remove(i);
            }
            _ => i += 1,
        }
    }
    a
}

/// Аргументы мультиплексирования.
/// `src_input` — индекс входа с оригиналом (0 для быстрого пути, 1 для AI-пути).
pub fn mux_args(st: &Settings, info: &MediaInfo, src_input: usize) -> Vec<String> {
    let src = src_input;
    let mut a: Vec<String> = vec!["-map".into(), "0:v:0".into()];
    let mp4 = st.container == "mp4";

    if !info.audio_codecs.is_empty() {
        a.extend(["-map".into(), format!("{src}:a?")]);
        if mp4
            && info
                .audio_codecs
                .iter()
                .any(|c| !MP4_AUDIO_OK.contains(&c.as_str()))
        {
            a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "320k".into()]);
        } else {
            a.extend(["-c:a".into(), "copy".into()]);
        }
    }

    if !info.sub_codecs.is_empty() {
        if !mp4 {
            a.extend(["-map".into(), format!("{src}:s?"), "-c:s".into(), "copy".into()]);
        } else if let Some(pos) = info
            .sub_codecs
            .iter()
            .position(|c| TEXT_SUBS.contains(&c.as_str()))
        {
            a.extend([
                "-map".into(),
                format!("{src}:s:{pos}?"),
                "-c:s".into(),
                "mov_text".into(),
            ]);
        }
    }

    if !mp4 {
        a.extend(["-map".into(), format!("{src}:t?"), "-c:t".into(), "copy".into()]);
        a.extend(["-map_chapters".into(), src.to_string()]);
        a.extend(["-map_metadata".into(), src.to_string()]);
    } else {
        a.extend(["-map_metadata".into(), src.to_string()]);
        a.extend(["-movflags".into(), "+faststart".into()]);
        if info.has_chapters {
            a.extend(["-map_chapters".into(), src.to_string()]);
        }
    }

    a
}

/// Цветовые метки результата.
pub fn color_args(info: &MediaInfo, st: &Settings) -> Vec<String> {
    let keep_hdr = info.is_hdr && st.hdr_policy == "preserve";
    let mut a: Vec<String> = Vec::new();
    if keep_hdr {
        let trc = if info.color_transfer == "unknown" {
            "smpte2084"
        } else {
            &info.color_transfer
        };
        a.extend([
            "-colorspace".into(),
            "bt2020nc".into(),
            "-color_primaries".into(),
            "bt2020".into(),
            "-color_trc".into(),
            trc.into(),
            "-color_range".into(),
            "tv".into(),
        ]);
    } else {
        a.extend([
            "-colorspace".into(),
            "bt709".into(),
            "-color_primaries".into(),
            "bt709".into(),
            "-color_trc".into(),
            "bt709".into(),
            "-color_range".into(),
            "tv".into(),
        ]);
    }
    a
}


