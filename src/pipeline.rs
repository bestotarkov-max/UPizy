//! Основной конвейер: апскейл AI-движком или GPU-шейдерами и финальное кодирование.

use anyhow::{anyhow, bail, Result};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ai;
use crate::anime4k;
use crate::config::{self, Settings};
use crate::i18n;
use crate::tf;
use crate::encoders;
use crate::ffmpeg::{ffmpeg, has_filter, hide_window};
use crate::probe::{self, MediaInfo};
use crate::rife;

#[derive(Debug, Clone)]
pub enum Event {
    Log(String),
    Progress { fraction: f32, stage: String },
    FileStarted { path: PathBuf, info: String },
    FileFinished { path: PathBuf, ok: bool, message: String },
    AllFinished { ok: usize, failed: usize },
    Probed { path: PathBuf, summary: String },
    AiReady(bool),
    RifeReady(bool),
}

pub struct Ctx {
    pub tx: Sender<Event>,
    pub cancel: Arc<AtomicBool>,
}

impl Ctx {
    pub fn new(tx: Sender<Event>, cancel: Arc<AtomicBool>) -> Self {
        Self { tx, cancel }
    }

    pub fn log(&self, msg: impl Into<String>) {
        let _ = self.tx.send(Event::Log(msg.into()));
    }

    pub fn progress(&self, fraction: f32, stage: impl Into<String>) {
        let _ = self.tx.send(Event::Progress {
            fraction: fraction.clamp(0.0, 1.0),
            stage: stage.into(),
        });
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            bail!(i18n::s("Отменено пользователем", "Cancelled by user"));
        }
        Ok(())
    }
}

fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

/// Кратность интерполяции кадров (1 = выключена).
pub fn interp_mult(st: &Settings) -> u32 {
    match st.interp {
        4 => 4,
        2 => 2,
        _ => 1,
    }
}

/// Итоговое разрешение для выбранной цели.
pub fn target_size(info: &MediaInfo, st: &Settings) -> (u32, u32) {
    let h = info.height.max(2);

    if st.target == "custom" {
        return (even(st.custom_w.max(2)), even(st.custom_h.max(2)));
    }
    // Режим «только FPS»: размер не меняем.
    if st.target == "keep" {
        return (even(info.width.max(2)), even(info.height.max(2)));
    }

    let ar = info.display_aspect();
    let target_h = match st.target.as_str() {
        "2K" => 1440.0,
        "4K" => 2160.0,
        "1.5x" => h as f64 * 1.5,
        "2x" => h as f64 * 2.0,
        "3x" => h as f64 * 3.0,
        "4x" => h as f64 * 4.0,
        _ => 2160.0,
    };
    let mut th = target_h.round().max(2.0);
    let mut tw = (th * ar).round().max(2.0);
    // Не выходим за 8K по ширине
    if tw > 7680.0 {
        let k = 7680.0 / tw;
        tw = 7680.0;
        th = (th * k).round();
    }
    (even(tw as u32), even(th as u32))
}

pub fn target_label(st: &Settings, size: (u32, u32)) -> String {
    match st.target.as_str() {
        "2K" => "2K".into(),
        "4K" => "4K".into(),
        "custom" => format!("{}x{}", size.0, size.1),
        other => other.to_string(),
    }
}

/// Имя результата: разрешение и/или fps.
fn output_label(st: &Settings, info: &MediaInfo, size: (u32, u32), mult: u32) -> String {
    let fps = (info.fps.max(1.0) * mult as f64).round() as u32;
    let label = if st.target == "keep" {
        format!("{fps}fps")
    } else if mult > 1 {
        format!("{}_{fps}fps", target_label(st, size))
    } else {
        target_label(st, size)
    };
    // Режим подписываем в имени файла: иначе результаты разных движков
    // отличаются только суффиксом _1, _2 и их не сравнить.
    if st.engine == "anime4k" {
        format!("{label}_anime4k")
    } else {
        label
    }
}

pub fn output_path(src: &Path, st: &Settings, label: &str) -> Result<PathBuf> {
    let dir = if st.output_dir.trim().is_empty() {
        src.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    } else {
        PathBuf::from(st.output_dir.trim())
    };
    fs::create_dir_all(&dir)?;

    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video")
        .to_string();
    let ext = if st.container == "mp4" { "mp4" } else { "mkv" };
    let base = format!("{stem}_{label}");
    let mut candidate = dir.join(format!("{base}.{ext}"));
    if !st.overwrite {
        let mut n = 1;
        while candidate.exists() && n < 1000 {
            candidate = dir.join(format!("{base}_{n}.{ext}"));
            n += 1;
        }
    }
    Ok(candidate)
}

/// Признаки того, что видеокарта «отвалилась» на уровне драйвера (TDR),
/// а не того, что программа сделала что-то не так. Возвращает подсказку для лога.
fn tdr_hint(e: &anyhow::Error) -> Option<&'static str> {
    let text = format!("{e:#}");
    let markers = [
        "GPU is lost",
        "device is lost",
        "DEVICE_LOST",
        "CUDA_ERROR",
        "No device available",
        "no suitable device",
        "Failed creating Vulkan device",
        "Device creation failed",
        "cuInit",
    ];
    if markers.iter().any(|m| text.contains(m)) {
        Some(i18n::s(
            "Видеокарта перестала отвечать на запросы драйвера (TDR) — это сбой железа или \
драйвера, а не программы. Перезагрузите ПК; если повторяется под нагрузкой, см. раздел \
«Нестабильная видеокарта» в README.",
            "The GPU stopped responding to the driver (TDR) — this is a hardware or driver \
failure, not a program bug. Reboot the PC; if it repeats under load, see the \
“Unstable GPU” section in the README.",
        ))
    } else {
        None
    }
}

/// Полный цикл обработки одного файла.
pub fn upscale_file(src: &Path, st: &Settings, ctx: &Ctx) -> Result<PathBuf> {
    let started = Instant::now();
    let info = probe::probe(src)?;
    if info.width == 0 || info.height == 0 {
        bail!(i18n::s(
            "Не удалось определить разрешение видео",
            "Could not determine the video resolution"
        ));
    }
    let size = target_size(&info, st);
    let mult = interp_mult(st);
    let _ = ctx.tx.send(Event::FileStarted {
        path: src.to_path_buf(),
        info: info.summary(),
    });

    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(i18n::s("видео", "video"));
    if st.target == "keep" {
        ctx.log(tf!(
            "▶ {name}: {} · только FPS, {} ({} → {:.0} fps)",
            "▶ {name}: {} · FPS only, {} ({} → {:.0} fps)",
            info.resolution(),
            info.duration_hms(),
            info.fps,
            info.fps * mult as f64
        ));
    } else {
        ctx.log(format!(
            "▶ {name}: {} → {}x{} ({})",
            info.resolution(),
            size.0,
            size.1,
            info.duration_hms()
        ));
    }
    if info.vfr {
        ctx.log(i18n::s(
            "  Переменный fps: видео будет приведено к постоянному.",
            "  Variable fps: video will be converted to constant frame rate.",
        ));
    }
    if info.is_hdr && st.engine == "ai" && st.hdr_policy != "preserve" {
        ctx.log(i18n::s(
            "  HDR-источник: выполняется тональное отображение в SDR (bt.709).",
            "  HDR source: tone-mapping to SDR (bt.709).",
        ));
    }

    let label = output_label(st, &info, size, mult);
    let out = output_path(src, st, &label)?;
    let upscaling = size.1 > info.height || size.0 > info.width;
    let use_ai = st.engine == "ai" && upscaling;
    let use_rife = mult > 1 && rife::installed();

    let result = if use_ai || use_rife {
        run_frames(&info, st, size, &out, use_ai, ctx)
    } else {
        run_fast(&info, st, size, &out, ctx)
    };

    if let Err(e) = result {
        let _ = fs::remove_file(&out);
        if let Some(hint) = tdr_hint(&e) {
            ctx.log(format!("  ⚠ {hint}"));
        }
        return Err(e);
    }

    ctx.progress(1.0, i18n::s("Готово", "Done"));
    ctx.log(format!(
        "✔ {name}: {} ({:.0} c)",
        out.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        started.elapsed().as_secs_f64()
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Быстрый путь: libplacebo (ewa_lanczossharp + deband) прямо в ffmpeg
// ---------------------------------------------------------------------------

fn fast_filter(info: &MediaInfo, st: &Settings, tw: u32, th: u32, a4k: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if info.interlaced {
        parts.push("bwdif=mode=send_frame:parity=auto:deint=all".into());
    }
    if st.denoise {
        parts.push("hqdn3d=1.5:1.5:6:6".into());
    }
    let mult = interp_mult(st);
    if mult > 1 {
        // Запасной вариант без RIFE: motion-compensated интерполяция ffmpeg.
        parts.push(format!(
            "minterpolate=fps={:.6}:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1:scd=none",
            info.fps.max(1.0) * mult as f64
        ));
    }
    let hdr_tonemap = info.is_hdr && st.hdr_policy != "preserve";
    if let Some(lp) = a4k {
        // Нейросетевые шейдеры Anime4K: сами делают ×2 и восстанавливают линии.
        let mut lp = lp.to_string();
        if hdr_tonemap {
            lp.push_str(
                ":tonemapping=bt.2390:colorspace=bt709:color_primaries=bt709:color_trc=bt709",
            );
        }
        parts.push(lp);
    } else if has_filter("libplacebo") {
        let mut lp = format!(
            "libplacebo=w={tw}:h={th}:upscaler=ewa_lanczossharp:downscaler=mitchell:\
sigmoid=true:deband=true:deband_iterations=2:deband_threshold=6:deband_radius=16:dithering=blue"
        );
        if hdr_tonemap {
            lp.push_str(
                ":tonemapping=bt.2390:colorspace=bt709:color_primaries=bt709:color_trc=bt709",
            );
        }
        parts.push(lp);
    } else if hdr_tonemap && has_filter("zscale") {
        parts.push(format!("scale={tw}:{th}:flags=lanczos"));
        parts.push(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv"
                .into(),
        );
    } else {
        parts.push(format!("scale={tw}:{th}:flags=lanczos"));
    }
    if st.sharpen > 0.01 {
        parts.push(format!("cas=strength={:.2}", st.sharpen.clamp(0.0, 1.0)));
    }
    parts.join(",")
}

/// Фильтр Anime4K для выбранного движка: `None`, если режим не выбран,
/// апскейл не нужен или в ffmpeg нет libplacebo.
fn anime4k_filter(info: &MediaInfo, st: &Settings, size: (u32, u32), ctx: &Ctx) -> Option<String> {
    if st.engine != "anime4k" {
        return None;
    }
    if size.1 <= info.height && size.0 <= info.width {
        return None; // «только плавность»: апскейлить нечего
    }
    if !has_filter("libplacebo") {
        ctx.log(i18n::s(
            "  Anime4K требует libplacebo — используется обычный масштаб",
            "  Anime4K requires libplacebo — using plain scaling",
        ));
        return None;
    }
    match anime4k::filter(st, size, info) {
        Ok(f) => Some(f),
        Err(e) => {
            ctx.log(tf!(
                "  Anime4K недоступен ({e:#}) — обычный libplacebo",
                "  Anime4K unavailable ({e:#}) — using plain libplacebo"
            ));
            None
        }
    }
}

fn run_fast(
    info: &MediaInfo,
    st: &Settings,
    size: (u32, u32),
    out: &Path,
    ctx: &Ctx,
) -> Result<()> {
    let (_, th) = size;
    let enc = encoders::resolve_encoder(&st.encoder);
    let ten_bit = ten_bit_target(info, st, &enc, th);

    if st.engine == "turbo" {
        let problem = if !is_nvenc(&enc) {
            Some(tf!(
                "выбранный кодек {enc} — не NVENC",
                "selected codec {enc} is not NVENC"
            ))
        } else if !has_filter("scale_cuda") {
            Some(
                i18n::s(
                    "в ffmpeg нет фильтра scale_cuda",
                    "ffmpeg has no scale_cuda filter",
                )
                .to_string(),
            )
        } else if info.is_hdr {
            Some(
                i18n::s(
                    "HDR-источник требует тонального отображения",
                    "HDR source requires tone-mapping",
                )
                .to_string(),
            )
        } else {
            None
        };
        match problem {
            Some(p) => ctx.log(tf!(
                "  Турбо недоступен: {p} — используется libplacebo",
                "  Turbo unavailable: {p} — using libplacebo"
            )),
            None => {
                ctx.log(i18n::s(
                    "  Турбо: декодирование, масштаб и кодирование на GPU (scale_cuda lanczos + NVENC p1)",
                    "  Turbo: decode, scale and encode on the GPU (scale_cuda lanczos + NVENC p1)",
                ));
                match run_turbo(info, st, size, out, &enc, ten_bit, ctx) {
                    Ok(()) => return Ok(()),
                    Err(e) if ctx.cancelled() => return Err(e),
                    Err(e) => {
                        ctx.log(tf!(
                            "  Турбо не сработал ({e}) — обычный GPU-путь",
                            "  Turbo failed ({e}) — using the regular GPU path"
                        ));
                        let _ = fs::remove_file(out);
                    }
                }
            }
        }
    }

    let (tw, _) = size;
    let mult = interp_mult(st);
    let a4k = anime4k_filter(info, st, size, ctx);
    if a4k.is_some() {
        ctx.log(format!("  {}", anime4k::describe(st, size, info)));
    } else {
        ctx.log(i18n::s(
            "  Быстрый GPU-апскейл (libplacebo · ewa_lanczossharp + deband)",
            "  Fast GPU upscale (libplacebo · ewa_lanczossharp + deband)",
        ));
    }
    if mult > 1 {
        ctx.log(tf!(
            "  Интерполяция ffmpeg minterpolate ×{mult} — RIFE не установлен, качество ниже",
            "  ffmpeg minterpolate ×{mult} interpolation — RIFE is not installed, lower quality"
        ));
    }

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into()];
    for a in ["-loglevel", "error", "-stats_period", "0.5", "-progress", "pipe:1"] {
        args.push(a.into());
    }
    args.push("-i".into());
    args.push(info.path.to_string_lossy().into_owned());
    args.push("-vf".into());
    args.push(fast_filter(info, st, tw, th, a4k.as_deref()));
    args.extend(encoders::mux_args(st, info, 0));
    args.extend(encoders::video_args(&enc, &st.quality, ten_bit));
    args.extend(encoders::color_args(info, st));
    args.push("-max_muxing_queue_size".into());
    args.push("4096".into());
    args.push(out.to_string_lossy().into_owned());

    run_ffmpeg_progress(
        i18n::s("Апскейл", "Upscale"),
        &args,
        info.frames.saturating_mul(mult as u64),
        ctx,
    )
}

fn is_nvenc(enc: &str) -> bool {
    enc.ends_with("_nvenc")
}

fn ten_bit_target(info: &MediaInfo, st: &Settings, enc: &str, th: u32) -> bool {
    let hdr_keep = info.is_hdr && st.hdr_policy == "preserve";
    encoders::is_10bit_capable(enc) && (th >= 1440 || hdr_keep)
}

/// Турбо-фильтр: кадры остаются на GPU, в память возвращается уже готовый 4K.
fn turbo_filter(info: &MediaInfo, st: &Settings, tw: u32, th: u32, ten_bit: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    if info.interlaced {
        parts.push("bwdif_cuda=mode=send_frame:parity=auto:deint=all".into());
    }
    parts.push(format!("scale_cuda={tw}:{th}:interp_algo=lanczos"));
    parts.push("hwdownload,format=nv12".into());
    if ten_bit {
        parts.push("format=yuv420p10le".into());
    }
    if st.denoise {
        parts.push("hqdn3d=1.5:1.5:6:6".into());
    }
    parts.join(",")
}

fn run_turbo(
    info: &MediaInfo,
    st: &Settings,
    size: (u32, u32),
    out: &Path,
    enc: &str,
    ten_bit: bool,
    ctx: &Ctx,
) -> Result<()> {
    let (tw, th) = size;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into()];
    for a in ["-loglevel", "error", "-stats_period", "0.5", "-progress", "pipe:1"] {
        args.push(a.into());
    }
    for a in ["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"] {
        args.push(a.into());
    }
    args.push("-i".into());
    args.push(info.path.to_string_lossy().into_owned());
    args.push("-vf".into());
    args.push(turbo_filter(info, st, tw, th, ten_bit));
    args.extend(encoders::mux_args(st, info, 0));
    args.extend(encoders::video_args_turbo(enc, &st.quality, ten_bit));
    args.extend(encoders::color_args(info, st));
    args.push("-max_muxing_queue_size".into());
    args.push("4096".into());
    args.push(out.to_string_lossy().into_owned());

    run_ffmpeg_progress(i18n::s("Турбо-апскейл", "Turbo upscale"), &args, info.frames, ctx)
}

/// Запуск ffmpeg с разбором -progress.
fn run_ffmpeg_progress(stage: &str, args: &[String], total_frames: u64, ctx: &Ctx) -> Result<()> {
    let work = config::work_dir();
    let _ = fs::create_dir_all(&work);
    let log_path = work.join(format!("ffmpeg_{}.log", std::process::id()));
    let log = File::create(&log_path)?;

    let mut cmd = Command::new(ffmpeg());
    cmd.args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    hide_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!(tf!("Не удалось запустить ffmpeg: {e}", "Failed to start ffmpeg: {e}")))?;
    let stdout = child.stdout.take().expect("stdout");
    let counter = Arc::new(AtomicU64::new(0));
    let c2 = counter.clone();
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(v) = line.trim().strip_prefix("frame=") {
                        if let Ok(n) = v.trim().parse::<u64>() {
                            c2.store(n, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
    });

    let total = total_frames.max(1);
    let result = loop {
        if ctx.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Err(anyhow!(i18n::s("Отменено пользователем", "Cancelled by user")));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    break Ok(());
                }
                break Err(anyhow!("ffmpeg: {}", log_tail(&log_path)));
            }
            Ok(None) => {
                let n = counter.load(Ordering::Relaxed);
                ctx.progress(n as f32 / total as f32 * 0.99, stage);
                std::thread::sleep(Duration::from_millis(220));
            }
            Err(e) => break Err(anyhow!("ffmpeg: {e}")),
        }
    };
    let _ = reader.join();
    result
}

fn log_tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: Vec<&str> = lines.iter().rev().take(6).cloned().collect();
    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

struct TempJob {
    root: PathBuf,
    keep: bool,
}

impl Drop for TempJob {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

// ---------------------------------------------------------------------------
// AI-путь: кадры -> Real-ESRGAN -> энкодер (потоково, с ограниченным диском)
// ---------------------------------------------------------------------------

fn prefilter(info: &MediaInfo, st: &Settings) -> String {
    let mut parts: Vec<String> = Vec::new();
    if info.interlaced {
        parts.push("bwdif=mode=send_frame:parity=auto:deint=all".into());
    }
    if st.denoise {
        parts.push("hqdn3d=1.5:1.5:6:6".into());
    }
    if info.is_hdr && st.hdr_policy != "preserve" {
        if has_filter("zscale") {
            parts.push(
                "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=rgb24"
                    .into(),
            );
        } else if has_filter("tonemap") {
            parts.push("format=gbrpf32le,tonemap=hable:desat=0".into());
        }
    }
    parts.join(",")
}

fn choose_model(st: &Settings) -> String {
    let installed = ai::models();
    let wanted = match st.ai_model.as_str() {
        // Для видео по умолчанию берём лёгкую видео-модель: она в ~10 раз быстрее
        // и не даёт «дрожания» деталей между кадрами.
        "" | "auto" => "realesr-animevideov3",
        other => other,
    };
    if installed.is_empty() || installed.iter().any(|m| m == wanted) {
        return wanted.to_string();
    }
    installed
        .iter()
        .find(|m| m.as_str() == "realesrgan-x4plus")
        .or_else(|| installed.first())
        .cloned()
        .unwrap_or_else(|| "realesrgan-x4plus".into())
}

fn ai_scale(model: &str, src_h: u32, target_h: u32) -> u32 {
    let ratio = target_h as f64 / src_h.max(1) as f64;
    if model.contains("animevideo") {
        if ratio <= 2.2 {
            2
        } else if ratio <= 3.2 {
            3
        } else {
            4
        }
    } else {
        4
    }
}

fn choose_tile(st: &Settings, info: &MediaInfo, model: &str) -> u32 {
    if st.tile > 0 {
        return st.tile;
    }
    // Компактная видео-модель помещается в память без тайлов даже на 1080p.
    if model.contains("animevideo") {
        return 0;
    }
    if info.height <= 720 {
        0
    } else if info.height <= 1440 {
        256
    } else {
        320
    }
}

fn choose_chunk(st: &Settings, tw: u32, th: u32) -> usize {
    if st.chunk > 0 {
        return st.chunk as usize;
    }
    let px = (tw as u64) * (th as u64);
    let est_png = (px * 9 / 10).max(1);
    let budget: u64 = 1_600_000_000;
    (budget / est_png).clamp(16, 96) as usize
}

fn count_pngs(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("png"))
                        .unwrap_or(false)
                })
                .count() as u64
        })
        .unwrap_or(0)
}

/// Источник кадров PNG: поток декодера или выход интерполяции.
trait FrameSource {
    fn next(&mut self) -> Result<Option<Vec<u8>>>;
}

impl<R: Read> FrameSource for PngStream<R> {
    fn next(&mut self) -> Result<Option<Vec<u8>>> {
        self.next_frame()
            .map_err(|e| anyhow!(tf!("Ошибка чтения кадров: {e}", "Failed to read frames: {e}")))
    }
}

fn pngs_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("png"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Модель интерполяции: для аниме-моделей апскейла берём rife-anime.
fn choose_interp_model(st: &Settings) -> String {
    let installed = rife::models();
    let wanted = match st.interp_model.as_str() {
        "" | "auto" => {
            let anime =
                st.ai_model.contains("anime") || st.ai_model.is_empty() || st.ai_model == "auto";
            if anime {
                "rife-anime"
            } else {
                "rife-v4.6"
            }
        }
        other => other,
    };
    if installed.is_empty() || installed.iter().any(|m| m == wanted) {
        return wanted.to_string();
    }
    installed
        .first()
        .cloned()
        .unwrap_or_else(|| "rife-anime".into())
}

fn choose_interp_chunk(w: u32, h: u32) -> usize {
    // PNG ≈ 3 байта на пиксель; держим ~0.5 ГБ на блок.
    let est_png = (w.max(16) as u64) * (h.max(16) as u64) * 3;
    (500_000_000 / est_png.max(1)).clamp(8, 120) as usize
}

/// Удвоение кадров RIFE блоками: один кадр переносится в следующий блок,
/// чтобы на стыках не было повторов и разрывов.
struct RifeStage {
    inner: Box<dyn FrameSource>,
    exe: PathBuf,
    model: PathBuf,
    in_dir: PathBuf,
    out_dir: PathBuf,
    chunk: usize,
    carry: Option<Vec<u8>>,
    queue: VecDeque<Vec<u8>>,
    cancelled: Arc<AtomicBool>,
    tx: Sender<Event>,
    seen: u64,
    total: u64,
    span: f32,
    finished: bool,
}

impl RifeStage {
    fn run_chunk(&mut self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            bail!(i18n::s("Отменено пользователем", "Cancelled by user"));
        }
        let has_carry = self.carry.is_some();
        let mut frames: Vec<Vec<u8>> = Vec::with_capacity(self.chunk + 1);
        if let Some(c) = self.carry.take() {
            frames.push(c);
        }
        let mut fresh = 0usize;
        while fresh < self.chunk {
            match self.inner.next()? {
                Some(f) => {
                    frames.push(f);
                    fresh += 1;
                }
                None => break,
            }
        }
        if fresh == 0 {
            return Ok(());
        }
        self.carry = frames.last().cloned();

        let _ = fs::remove_dir_all(&self.in_dir);
        let _ = fs::remove_dir_all(&self.out_dir);
        fs::create_dir_all(&self.in_dir)?;
        fs::create_dir_all(&self.out_dir)?;
        for (i, f) in frames.iter().enumerate() {
            fs::write(self.in_dir.join(format!("{:08}.png", i + 1)), f)
                .map_err(|e| anyhow!(tf!("Запись кадра для RIFE: {e}", "Failed to write a frame for RIFE: {e}")))?;
        }

        let log_path = self
            .in_dir
            .parent()
            .unwrap_or(&self.in_dir)
            .join("rife.log");
        let mut cmd = Command::new(&self.exe);
        cmd.arg("-i")
            .arg(&self.in_dir)
            .arg("-o")
            .arg(&self.out_dir)
            .arg("-m")
            .arg(&self.model)
            .arg("-f")
            .arg("png")
            .arg("-j")
            .arg("2:2:2");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&log_path)?));
        hide_window(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!(tf!("Не удалось запустить RIFE: {e}", "Failed to start RIFE: {e}")))?;
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                bail!(i18n::s("Отменено пользователем", "Cancelled by user"));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        bail!(tf!(
            "RIFE завершился с ошибкой: {}",
            "RIFE failed: {}",
            log_tail(&log_path)
        ));
                    }
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(150)),
                Err(e) => bail!("RIFE: {e}"),
            }
        }

        let mut outs = pngs_sorted(&self.out_dir);
        // Первый кадр блока — дубль последнего кадра предыдущего блока.
        if has_carry && !outs.is_empty() {
            outs.remove(0);
        }
        // RIFE дописывает в конец дубль последнего кадра — он лишний,
        // иначе на каждом стыке блоков получается стоп-кадр.
        let want = if has_carry {
            fresh * 2
        } else {
            fresh * 2 - 1
        };
        if outs.len() > want {
            outs.truncate(want);
        }
        for path in outs {
            let data = fs::read(&path)
                .map_err(|e| {
            anyhow!(tf!(
                "Чтение кадра RIFE ({}): {e}",
                "Failed to read a RIFE frame ({}): {e}",
                path.display()
            ))
        })?;
            self.queue.push_back(data);
        }
        self.seen += fresh as u64;
        if self.span > 0.0 && self.total > 0 {
            let _ = self.tx.send(Event::Progress {
                fraction: (self.seen as f32 / self.total as f32 * self.span).clamp(0.0, 1.0),
                stage: i18n::s("Интерполяция", "Interpolation").into(),
            });
        }
        Ok(())
    }
}

impl FrameSource for RifeStage {
    fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(f) = self.queue.pop_front() {
                return Ok(Some(f));
            }
            if self.finished {
                return Ok(None);
            }
            self.run_chunk()?;
            if self.queue.is_empty() {
                self.finished = true;
                let _ = fs::remove_dir_all(&self.in_dir);
                let _ = fs::remove_dir_all(&self.out_dir);
                return Ok(None);
            }
        }
    }
}

/// Кадровый конвейер: декодер → (RIFE) → (Real-ESRGAN) → кодировщик.
fn run_frames(
    info: &MediaInfo,
    st: &Settings,
    size: (u32, u32),
    out: &Path,
    use_ai: bool,
    ctx: &Ctx,
) -> Result<()> {
    let (tw, th) = size;
    let mult = interp_mult(st);
    let fps_in = info.fps.max(1.0);
    let fps_out = fps_in * mult as f64;

    // Real-ESRGAN и RIFE работают в 8-битном SDR, а кадры идут через PNG rgb24:
    // HDR-источник тонально отображаем, иначе картинка будет «выцветшей».
    let mut own = st.clone();
    if info.is_hdr && (use_ai || mult > 1) {
        own.hdr_policy = "tonemap".into();
    }
    let st = &own;

    let src_total = if info.frames > 0 {
        info.frames
    } else {
        (info.duration * fps_in).round() as u64
    }
    .max(1);
    let out_total = (src_total * mult as u64).max(1);

    let mut pre = prefilter(info, st);
    let use_rife = mult > 1 && rife::exe_path().is_some();
    if mult > 1 && st.engine == "turbo" {
        ctx.log(i18n::s(
            "  Турбо и плавность вместе несовместимы: масштаб делает libplacebo",
            "  Turbo and smoothness cannot be combined: libplacebo does the scaling",
        ));
    }
    if mult > 1 && !use_rife {
        let mp = format!(
            "minterpolate=fps={fps_out:.6}:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1:scd=none"
        );
        pre = if pre.is_empty() { mp } else { format!("{pre},{mp}") };
        ctx.log(tf!(
            "  Интерполяция ffmpeg minterpolate ×{mult} — RIFE не установлен, качество ниже",
            "  ffmpeg minterpolate ×{mult} interpolation — RIFE is not installed, lower quality"
        ));
    }

    let ai_plan = if use_ai {
        let exe = ai::exe_path()
            .ok_or_else(|| {
                anyhow!(i18n::s(
                    "AI-движок не установлен — нажмите «Скачать AI» в приложении",
                    "AI engine is not installed — click “Download AI” in the app"
                ))
            })?;
        let model = choose_model(st);
        let scale = ai_scale(&model, info.height, th);
        let tile = choose_tile(st, info, &model);
        let tile_text = if tile == 0 {
            i18n::s("авто", "auto").to_string()
        } else {
            tile.to_string()
        };
        ctx.log(tf!(
            "  AI: {model} ×{scale}, тайл {}, кадров ≈ {out_total}",
            "  AI: {model} ×{scale}, tile {}, frames ≈ {out_total}",
            tile_text
        ));
        Some((exe, model, scale, tile))
    } else {
        None
    };

    let job_root = config::work_dir().join(format!("job_{}_{}", std::process::id(), timestamp()));
    let in_dir = job_root.join("in");
    let out_dir = job_root.join("out");
    fs::create_dir_all(&in_dir)?;
    fs::create_dir_all(&out_dir)?;
    let _guard = TempJob {
        root: job_root.clone(),
        keep: st.keep_work,
    };

    // --- декодер: видео -> поток PNG в stdout ---
    let mut d = Command::new(ffmpeg());
    d.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(&info.path)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn"]);
    if !pre.is_empty() {
        d.args(["-vf", &pre]);
    }
    d.args([
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "rgb24",
        "-f",
        "image2pipe",
        "-c:v",
        "png",
        "-",
    ]);
    d.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(File::create(job_root.join("decode.log"))?));
    hide_window(&mut d);
    let mut decoder = d
        .spawn()
        .map_err(|e| {
            anyhow!(tf!(
                "Не удалось запустить ffmpeg (декодер): {e}",
                "Failed to start ffmpeg (decoder): {e}"
            ))
        })?;
    let dec_out = decoder.stdout.take().expect("decoder stdout");

    // --- цепочка источников: декодер + при необходимости RIFE ---
    let mut source: Box<dyn FrameSource> = Box::new(PngStream::new(dec_out));
    let prog_base = if use_rife && use_ai { 0.4f32 } else { 0.0f32 };
    if use_rife {
        let exe = rife::exe_path().expect("RIFE installed");
        let model = choose_interp_model(st);
        let mdir = rife::model_dir(&model);
        let passes: u32 = if mult >= 4 { 2 } else { 1 };
        ctx.log(tf!(
            "  Интерполяция: RIFE {model} ×{mult} — {fps_in:.3} → {fps_out:.3} fps",
            "  Interpolation: RIFE {model} ×{mult} — {fps_in:.3} → {fps_out:.3} fps"
        ));
        let chunk = choose_interp_chunk(info.width, info.height);
        for pass in 0..passes {
            let dir = job_root.join(format!("rife{}", pass + 1));
            fs::create_dir_all(&dir)?;
            let span = if pass == 0 {
                if use_ai {
                    0.4
                } else {
                    0.9
                }
            } else {
                0.0
            };
            let total = src_total * if pass == 0 { 1 } else { 2 };
            source = Box::new(RifeStage {
                inner: source,
                exe: exe.clone(),
                model: mdir.clone(),
                in_dir: dir.join("in"),
                out_dir: dir.join("out"),
                chunk,
                carry: None,
                queue: VecDeque::new(),
                cancelled: ctx.cancel.clone(),
                tx: ctx.tx.clone(),
                seen: 0,
                total,
                span,
                finished: false,
            });
        }
    }

    // --- энкодер: поток PNG со stdin + аудио/субтитры из оригинала ---
    let a4k = anime4k_filter(info, st, size, ctx);
    if a4k.is_some() {
        ctx.log(format!("  {}", anime4k::describe(st, size, info)));
    }
    let enc_args = encoder_pipe_args(info, st, size, fps_out, out, !use_ai, a4k.as_deref());
    let mut e = Command::new(ffmpeg());
    e.args(&enc_args);
    e.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(job_root.join("encode.log"))?));
    hide_window(&mut e);
    let mut encoder = e
        .spawn()
        .map_err(|err| {
            anyhow!(tf!(
                "Не удалось запустить ffmpeg (энкодер): {err}",
                "Failed to start ffmpeg (encoder): {err}"
            ))
        })?;
    let mut sink = std::io::BufWriter::with_capacity(
        1 << 20,
        encoder.stdin.take().expect("encoder stdin"),
    );

    let mut idx: u64 = 0;
    let mut done: u64 = 0;
    let mut failure: Option<anyhow::Error> = None;

    if let Some((exe, model, scale, tile)) = &ai_plan {
        let chunk_size = choose_chunk(st, tw, th);
        let mut pending: Vec<String> = Vec::new();
        loop {
            if ctx.cancelled() {
                failure = Some(anyhow!(i18n::s("Отменено пользователем", "Cancelled by user")));
                break;
            }
            match source.next() {
                Ok(Some(bytes)) => {
                    idx += 1;
                    let name = format!("{idx:08}.png");
                    if let Err(e) = fs::write(in_dir.join(&name), &bytes) {
                        failure = Some(anyhow!(tf!("Запись кадра: {e}", "Failed to write a frame: {e}")));
                        break;
                    }
                    pending.push(name);
                    if pending.len() >= chunk_size {
                        if let Err(e) = process_chunk(
                            exe,
                            model,
                            *scale,
                            *tile,
                            &in_dir,
                            &out_dir,
                            &mut pending,
                            &mut sink,
                            &mut done,
                            out_total,
                            prog_base,
                            ctx,
                        ) {
                            failure = Some(e);
                            break;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if failure.is_none() && !pending.is_empty() {
            if let Err(e) = process_chunk(
                exe,
                model,
                *scale,
                *tile,
                &in_dir,
                &out_dir,
                &mut pending,
                &mut sink,
                &mut done,
                out_total,
                prog_base,
                ctx,
            ) {
                failure = Some(e);
            }
        }
    } else {
        loop {
            if ctx.cancelled() {
                failure = Some(anyhow!(i18n::s("Отменено пользователем", "Cancelled by user")));
                break;
            }
            match source.next() {
                Ok(Some(bytes)) => {
                    idx += 1;
                    if let Err(e) = sink.write_all(&bytes) {
                        failure = Some(anyhow!(tf!(
                "Подача кадра в кодировщик: {e}",
                "Failed to feed a frame to the encoder: {e}"
            )));
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        sink.flush().ok();
    }

    if failure.is_some() {
        let _ = encoder.kill();
        let _ = decoder.kill();
    }
    drop(source);
    drop(sink);
    let enc_status = encoder.wait();
    let dec_status = decoder.wait();

    if let Some(err) = failure {
        return Err(err);
    }
    if idx == 0 {
        bail!(tf!(
            "Не удалось извлечь кадры: {}",
            "Failed to extract frames: {}",
            log_tail(&job_root.join("decode.log"))
        ));
    }
    match enc_status {
        Ok(s) if s.success() => {}
        _ => bail!(tf!(
            "Ошибка кодирования: {}",
            "Encoding error: {}",
            log_tail(&job_root.join("encode.log"))
        )),
    }
    if let Ok(s) = dec_status {
        if !s.success() {
            ctx.log(tf!(
                "  Предупреждение декодера: {}",
                "  Decoder warning: {}",
                log_tail(&job_root.join("decode.log"))
            ));
        }
    }

    ctx.progress(1.0, i18n::s("Готово", "Done"));
    Ok(())
}

fn encoder_pipe_args(
    info: &MediaInfo,
    st: &Settings,
    size: (u32, u32),
    fps: f64,
    out: &Path,
    libplacebo: bool,
    a4k: Option<&str>,
) -> Vec<String> {
    let (tw, th) = size;
    let enc = encoders::resolve_encoder(&st.encoder);
    let hdr_keep = info.is_hdr && st.hdr_policy == "preserve";
    let ten_bit = encoders::is_10bit_capable(&enc) && (th >= 1440 || hdr_keep);

    let mut a: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into()];
    for v in ["-loglevel", "error"] {
        a.push(v.into());
    }
    a.push("-f".into());
    a.push("image2pipe".into());
    a.push("-framerate".into());
    a.push(format!("{fps:.6}"));
    a.push("-i".into());
    a.push("-".into());
    a.push("-i".into());
    a.push(info.path.to_string_lossy().into_owned());

    let mut vf: Vec<String> = Vec::new();
    if let Some(lp) = a4k {
        // Кадры после RIFE (1080p) поднимаем нейросетевыми шейдерами Anime4K.
        vf.push(lp.to_string());
    } else if libplacebo && has_filter("libplacebo") {
        // Быстрый путь после RIFE: тот же ewa_lanczossharp, что и без интерполяции.
        vf.push(format!(
            "libplacebo=w={tw}:h={th}:upscaler=ewa_lanczossharp:downscaler=mitchell:\
sigmoid=true:deband=true:deband_iterations=2:deband_threshold=6:deband_radius=16:dithering=blue"
        ));
    } else {
        vf.push(format!("scale={tw}:{th}:flags=lanczos"));
    }
    if st.sharpen > 0.01 {
        vf.push(format!("cas=strength={:.2}", st.sharpen.clamp(0.0, 1.0)));
    }
    a.push("-vf".into());
    a.push(vf.join(","));

    a.extend(encoders::mux_args(st, info, 1));
    a.extend(encoders::video_args(&enc, &st.quality, ten_bit));
    a.extend(encoders::color_args(info, st));
    a.push("-max_muxing_queue_size".into());
    a.push("4096".into());
    a.push(out.to_string_lossy().into_owned());
    a
}

#[allow(clippy::too_many_arguments)]
fn process_chunk(
    exe: &Path,
    model: &str,
    scale: u32,
    tile: u32,
    in_dir: &Path,
    out_dir: &Path,
    pending: &mut Vec<String>,
    sink: &mut impl Write,
    done: &mut u64,
    total: u64,
    prog_base: f32,
    ctx: &Ctx,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    ai_process(
        exe, in_dir, out_dir, model, scale, tile, *done, total, prog_base, ctx,
    )?;

    let mut n = 0u64;
    for name in pending.iter() {
        let path = out_dir.join(name);
        let data = fs::read(&path)
            .map_err(|e| {
            anyhow!(tf!(
                "Чтение результата AI ({}): {e}",
                "Failed to read AI output ({}): {e}",
                path.display()
            ))
        })?;
        sink.write_all(&data)
            .map_err(|e| {
            anyhow!(tf!(
                "Подача кадра в кодировщик: {e}",
                "Failed to feed a frame to the encoder: {e}"
            ))
        })?;
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(in_dir.join(name));
        n += 1;
    }
    sink.flush().ok();
    pending.clear();
    *done += n;
    ctx.progress(
        prog_base + (1.0 - prog_base) * (*done as f32 / total as f32),
        i18n::s("AI-апскейл", "AI upscale"),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ai_process(
    exe: &Path,
    in_dir: &Path,
    out_dir: &Path,
    model: &str,
    scale: u32,
    tile: u32,
    base: u64,
    total: u64,
    prog_base: f32,
    ctx: &Ctx,
) -> Result<()> {
    let log_path = in_dir.parent().unwrap_or(in_dir).join("ai.log");
    let mut cmd = Command::new(exe);
    cmd.arg("-i")
        .arg(in_dir)
        .arg("-o")
        .arg(out_dir)
        .arg("-n")
        .arg(model)
        .arg("-s")
        .arg(scale.to_string())
        .arg("-f")
        .arg("png")
        .arg("-j")
        .arg("2:2:2");
    if tile > 0 {
        cmd.arg("-t").arg(tile.to_string());
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&log_path)?));
    hide_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| {
            anyhow!(tf!(
                "Не удалось запустить AI-движок: {e}",
                "Failed to start the AI engine: {e}"
            ))
        })?;

    loop {
        if ctx.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            bail!(i18n::s("Отменено пользователем", "Cancelled by user"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                bail!(tf!(
                    "AI-движок завершился с ошибкой: {}",
                    "AI engine failed: {}",
                    log_tail(&log_path)
                ));
            }
            Ok(None) => {
                let cur = count_pngs(out_dir);
                ctx.progress(
                    prog_base + (1.0 - prog_base) * ((base + cur) as f32 / total as f32),
                    i18n::s("AI-апскейл", "AI upscale"),
                );
                std::thread::sleep(Duration::from_millis(180));
            }
            Err(e) => bail!(tf!("AI-движок: {e}", "AI engine: {e}")),
        }
    }
}

/// Читает поток склеенных PNG из pipe и режет его на отдельные кадры.
struct PngStream<R: Read> {
    inner: R,
    buf: Vec<u8>,
    scan: usize,
    tmp: Vec<u8>,
    eof: bool,
}

const PNG_IEND: [u8; 12] = [0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];

impl<R: Read> PngStream<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::with_capacity(1 << 22),
            scan: 0,
            tmp: vec![0u8; 1 << 20],
            eof: false,
        }
    }

    fn next_frame(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if self.buf.len() >= PNG_IEND.len() {
                let start = self.scan.min(self.buf.len());
                if let Some(pos) = find_sub(&self.buf[start..], &PNG_IEND) {
                    let end = start + pos + PNG_IEND.len();
                    let frame = self.buf[..end].to_vec();
                    self.buf.drain(..end);
                    self.scan = 0;
                    return Ok(Some(frame));
                }
                self.scan = self.buf.len().saturating_sub(PNG_IEND.len() - 1);
            }
            if self.eof {
                return Ok(None);
            }
            let n = self.inner.read(&mut self.tmp)?;
            if n == 0 {
                self.eof = true;
                return Ok(None);
            }
            self.buf.extend_from_slice(&self.tmp[..n]);
        }
    }
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let last_start = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}


