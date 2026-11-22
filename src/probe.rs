//! Разбор медиафайлов через ffprobe.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::ffmpeg::{ffprobe, hide_window};
use crate::i18n;
use crate::tf;

#[derive(Debug, Deserialize)]
struct FfprobeOut {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
    #[serde(default)]
    chapters: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Format {
    duration: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    attached_pic: i32,
}

#[derive(Debug, Deserialize)]
struct SideData {
    #[serde(default)]
    rotation: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_space: Option<String>,
    field_order: Option<String>,
    nb_frames: Option<String>,
    duration: Option<String>,
    sample_aspect_ratio: Option<String>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
    disposition: Option<Disposition>,
}

#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub frames: u64,
    pub vcodec: String,
    pub pix_fmt: String,
    pub bit_depth: u8,
    pub color_transfer: String,
    pub color_primaries: String,
    pub color_space: String,
    pub is_hdr: bool,
    pub interlaced: bool,
    pub sar: String,
    pub rotation: i32,
    pub vfr: bool,
    pub audio_codecs: Vec<String>,
    pub sub_codecs: Vec<String>,
    pub has_chapters: bool,
    pub size_bytes: u64,
}

impl MediaInfo {
    pub fn resolution(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    /// Соотношение сторон с учётом анаморфного SAR.
    pub fn display_aspect(&self) -> f64 {
        let w = self.width.max(1) as f64;
        let h = self.height.max(1) as f64;
        let sar = self
            .sar
            .split_once(':')
            .and_then(|(n, d)| {
                let n: f64 = n.parse().ok()?;
                let d: f64 = d.parse().ok()?;
                (d > 0.0 && n > 0.0).then_some(n / d)
            })
            .unwrap_or(1.0);
        (w * sar) / h
    }

    pub fn summary(&self) -> String {
        format!(
            "{} · {:.4} fps · {} {} · {}",
            self.resolution(),
            self.fps,
            self.vcodec,
            if self.bit_depth > 8 {
                format!("{}bit", self.bit_depth)
            } else {
                String::new()
            },
            if self.is_hdr { "HDR" } else { "SDR" }
        )
        .replace("  ", " ")
    }

    pub fn duration_hms(&self) -> String {
        let total = self.duration.max(0.0).round() as u64;
        format!(
            "{:02}:{:02}:{:02}",
            total / 3600,
            (total % 3600) / 60,
            total % 60
        )
    }
}

fn parse_fraction(value: &Option<String>) -> f64 {
    let Some(v) = value else { return 0.0 };
    if let Some((num, den)) = v.split_once('/') {
        let n: f64 = num.parse().unwrap_or(0.0);
        let d: f64 = den.parse().unwrap_or(0.0);
        if d != 0.0 {
            return n / d;
        }
        0.0
    } else {
        v.parse().unwrap_or(0.0)
    }
}

pub fn probe(path: &Path) -> anyhow::Result<MediaInfo> {
    let mut cmd = Command::new(ffprobe());
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
    ]);
    cmd.arg(path);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    hide_window(&mut cmd);

    let out = cmd.output().map_err(|e| {
        anyhow::anyhow!(tf!(
            "Не удалось запустить ffprobe: {e}",
            "Failed to start ffprobe: {e}"
        ))
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .last()
            .unwrap_or(i18n::s("неизвестная ошибка", "unknown error"));
        anyhow::bail!(tf!(
            "ffprobe не смог прочитать файл: {last}",
            "ffprobe could not read the file: {last}"
        ));
    }

    let parsed: FfprobeOut = serde_json::from_slice(&out.stdout).map_err(|e| {
        anyhow::anyhow!(tf!(
            "Некорректный ответ ffprobe: {e}",
            "Invalid ffprobe response: {e}"
        ))
    })?;

    let video = parsed
        .streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video")
                && s.disposition
                    .as_ref()
                    .map(|d| d.attached_pic == 0)
                    .unwrap_or(true)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(i18n::s(
                "В файле не найдена видеодорожка",
                "No video track found in the file"
            ))
        })?;

    let mut fps = parse_fraction(&video.r_frame_rate);
    let avg = parse_fraction(&video.avg_frame_rate);
    let vfr = fps > 0.0 && avg > 0.0 && (fps - avg).abs() > 0.01;
    if fps <= 0.0 {
        fps = if avg > 0.0 { avg } else { 25.0 };
    }

    let mut duration = 0.0f64;
    for src in [video.duration.as_deref(), parsed.format.as_ref().and_then(|f| f.duration.as_deref())] {
        if let Some(v) = src {
            if let Ok(d) = v.parse::<f64>() {
                if d > 0.0 {
                    duration = d;
                    break;
                }
            }
        }
    }

    let mut frames: u64 = video
        .nb_frames
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if frames == 0 && duration > 0.0 {
        frames = (duration * fps).round() as u64;
    }

    let pix_fmt = video.pix_fmt.clone().unwrap_or_default();
    let bit_depth = if pix_fmt.contains("12") {
        12
    } else if pix_fmt.contains("10") || pix_fmt.contains("p010") || pix_fmt.contains("p216") {
        10
    } else {
        8
    };

    let color_transfer = video
        .color_transfer
        .clone()
        .unwrap_or_else(|| "unknown".into())
        .to_lowercase();
    let color_primaries = video
        .color_primaries
        .clone()
        .unwrap_or_else(|| "unknown".into())
        .to_lowercase();
    let color_space = video
        .color_space
        .clone()
        .unwrap_or_else(|| "unknown".into())
        .to_lowercase();

    let is_hdr = matches!(color_transfer.as_str(), "smpte2084" | "arib-std-b67" | "smpte428")
        || color_primaries == "bt2020";

    let field_order = video
        .field_order
        .clone()
        .unwrap_or_else(|| "progressive".into())
        .to_lowercase();
    let interlaced = !matches!(field_order.as_str(), "progressive" | "unknown" | "");

    let rotation = video
        .side_data_list
        .iter()
        .find_map(|sd| sd.rotation)
        .map(|r| r as i32)
        .unwrap_or(0);

    // Поворот из метаданных (телефон): ffmpeg кадры автоматически разворачивает,
    // поэтому и разрешение считаем уже в «экранной» ориентации.
    let mut width = video.width.unwrap_or(0);
    let mut height = video.height.unwrap_or(0);
    if rotation.rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }

    let sar = video
        .sample_aspect_ratio
        .clone()
        .filter(|s| {
            let s = s.trim();
            !s.is_empty() && s != "0:1" && s != "N/A" && s != "1:1"
        })
        .unwrap_or_else(|| "1:1".into());

    let audio_codecs: Vec<String> = parsed
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .filter_map(|s| s.codec_name.clone())
        .collect();

    let sub_codecs: Vec<String> = parsed
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("subtitle"))
        .filter_map(|s| s.codec_name.clone())
        .collect();

    let size_bytes = parsed
        .format
        .as_ref()
        .and_then(|f| f.size.as_deref())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(MediaInfo {
        path: path.to_path_buf(),
        width,
        height,
        fps,
        duration,
        frames,
        vcodec: video.codec_name.clone().unwrap_or_else(|| "?".into()),
        pix_fmt,
        bit_depth,
        color_transfer,
        color_primaries,
        color_space,
        is_hdr,
        interlaced,
        sar,
        rotation,
        vfr,
        audio_codecs,
        sub_codecs,
        has_chapters: !parsed.chapters.is_empty(),
        size_bytes,
    })
}
