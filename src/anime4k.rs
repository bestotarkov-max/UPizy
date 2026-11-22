//! Anime4K v4 — нейросети для аниме, скомпилированные в GLSL-шейдеры (формат mpv `.hook`).
//!
//! Их выполняет libplacebo прямо внутри ffmpeg: без отдельных процессов, без PNG-файлов
//! и без обмена через диск. Те же свёрточные слои, что и у Real-ESRGAN, но посчитанные
//! фрагментными шейдерами на видеокарте — поэтому 1080p → 4K идёт со скоростью
//! ~30 кадров/с (RTX 2080 Ti) вместо 3 кадров/с у ncnn-сборки.
//!
//! Шейдеры распространяются по лицензии MIT (c) 2019-2021 bloc97, см. заголовки файлов.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{self, Settings};
use crate::probe::MediaInfo;
use crate::tf;

const CLAMP: &str = include_str!("../assets/shaders/Anime4K_Clamp_Highlights.glsl");

const RESTORE_S: &str = include_str!("../assets/shaders/Anime4K_Restore_CNN_S.glsl");
const RESTORE_M: &str = include_str!("../assets/shaders/Anime4K_Restore_CNN_M.glsl");
const RESTORE_L: &str = include_str!("../assets/shaders/Anime4K_Restore_CNN_L.glsl");

const UPSCALE_S: &str = include_str!("../assets/shaders/Anime4K_Upscale_CNN_x2_S.glsl");
const UPSCALE_M: &str = include_str!("../assets/shaders/Anime4K_Upscale_CNN_x2_M.glsl");
const UPSCALE_L: &str = include_str!("../assets/shaders/Anime4K_Upscale_CNN_x2_L.glsl");

/// Приводим настройку к одному из известных вариантов.
pub fn variant(st: &Settings) -> &'static str {
    match st.a4k_size.trim().to_ascii_uppercase().as_str() {
        "S" => "S",
        "L" => "L",
        _ => "M",
    }
}

fn restore(v: &str) -> &'static str {
    match v {
        "S" => RESTORE_S,
        "L" => RESTORE_L,
        _ => RESTORE_M,
    }
}

fn upscale(v: &str) -> &'static str {
    match v {
        "S" => UPSCALE_S,
        "L" => UPSCALE_L,
        _ => UPSCALE_M,
    }
}

/// Сколько раз прогнать x2-апскейлер. Для 1080p → 4K хватает одного прохода,
/// для 720p → 4K (×4) — двух: второй даёт суперсэмплинг, как x4plus у Real-ESRGAN.
fn passes(info: &MediaInfo, target_h: u32) -> usize {
    let ratio = target_h as f64 / info.height.max(1) as f64;
    if ratio > 2.2 {
        2
    } else {
        1
    }
}

/// Пишет собранный шейдер в `_work/shaders` и возвращает путь.
///
/// libplacebo читает один файл, поэтому Clamp + Restore + Upscale склеиваются
/// в один `.glsl`: у каждого блока свой заголовок `//!HOOK`, порядок = порядок проходов.
fn shader_file(st: &Settings, passes: usize) -> Result<PathBuf> {
    let v = variant(st);
    let dir = config::work_dir().join("shaders");
    fs::create_dir_all(&dir).with_context(|| {
        tf!(
            "Не удалось создать каталог шейдеров {}",
            "Could not create the shader directory {}",
            dir.display()
        )
    })?;

    let path = dir.join(format!(
        "anime4k_{}{}.glsl",
        v.to_ascii_lowercase(),
        if passes > 1 { "x2" } else { "" }
    ));

    let mut text = String::with_capacity(160 * 1024);
    text.push_str(CLAMP.trim_start_matches('\u{feff}'));
    text.push('\n');
    text.push_str(restore(v));
    text.push('\n');
    for _ in 0..passes {
        text.push_str(upscale(v));
        text.push('\n');
    }
    fs::write(&path, text).with_context(|| {
        tf!(
            "Не удалось записать шейдер {}",
            "Could not write the shader {}",
            path.display()
        )
    })?;
    Ok(path)
}

/// Путь для фильтра ffmpeg: двоеточие разделяет опции, поэтому `C:` экранируется.
fn ff_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace(':', r"\\:")
}

/// Аргументы фильтра `libplacebo` с подключёнными шейдерами Anime4K.
pub fn filter(st: &Settings, size: (u32, u32), info: &MediaInfo) -> Result<String> {
    let (tw, th) = size;
    let path = shader_file(st, passes(info, th))?;
    // ewa_lanczossharp остаётся страховкой: если шейдер дал не ровно ×2
    // (например, 1080p → 1440p), остаток досчитает он.
    Ok(format!(
        "libplacebo=w={tw}:h={th}:upscaler=ewa_lanczossharp:downscaler=mitchell:\
dithering=blue:deband=true:deband_iterations=1:deband_threshold=4:\
custom_shader_path={}",
        ff_path(&path)
    ))
}

/// Человеческое описание выбранного режима для лога.
pub fn describe(st: &Settings, size: (u32, u32), info: &MediaInfo) -> String {
    let n = passes(info, size.1);
    tf!(
        "Anime4K · вариант {}, ×{} (Restore_CNN + Upscale_CNN_x2 по {} раз)",
        "Anime4K · variant {}, ×{} (Restore_CNN + Upscale_CNN_x2 ×{} passes)",
        variant(st),
        n * 2,
        n
    )
}
