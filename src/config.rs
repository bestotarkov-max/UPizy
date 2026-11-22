//! Настройки, пути и константы.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_NAME: &str = "UPizy";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Расширения, которые показываем в диалогах выбора файлов.
/// Внутри работает ffmpeg, поэтому реально поддерживается почти всё.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "m2ts",
    "mts", "vob", "3gp", "ogv", "rm", "rmvb", "asf", "divx", "f4v", "mxf", "m2v", "m1v",
    "dav", "mpe", "mpv", "av1", "ivf", "y4m", "gif",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 2K | 4K | 1.5x | 2x | 3x | 4x | custom
    pub target: String,
    pub custom_w: u32,
    pub custom_h: u32,
    /// ai | fast | turbo | anime4k
    pub engine: String,
    /// auto | realesrgan-x4plus | realesrgan-x4plus-anime | realesr-animevideov3
    pub ai_model: String,
    /// Anime4K: S | M | L (быстрее ↔ детальнее)
    pub a4k_size: String,
    /// max | high | balanced
    pub quality: String,
    /// auto | hevc_nvenc | h264_nvenc | libx265 | libx264 | libsvtav1
    pub encoder: String,
    /// mkv | mp4
    pub container: String,
    /// Интерполяция кадров: 1 = выкл, 2 = ×2, 4 = ×4
    pub interp: u32,
    /// auto | rife-anime | rife-v4.6
    pub interp_model: String,
    /// сила CAS-шарпа после апскейла, 0..1
    pub sharpen: f32,
    pub denoise: bool,
    /// tonemap | preserve
    pub hdr_policy: String,
    /// 0 = авто
    pub tile: u32,
    /// 0 = авто
    pub chunk: u32,
    pub output_dir: String,
    pub overwrite: bool,
    pub keep_work: bool,
    /// Язык интерфейса: ru | en
    pub lang: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target: "4K".into(),
            custom_w: 2560,
            custom_h: 1440,
            engine: "ai".into(),
            ai_model: "auto".into(),
            a4k_size: "M".into(),
            quality: "high".into(),
            encoder: "auto".into(),
            container: "mkv".into(),
            interp: 1,
            interp_model: "auto".into(),
            sharpen: 0.25,
            denoise: false,
            hdr_policy: "tonemap".into(),
            tile: 0,
            chunk: 0,
            output_dir: String::new(),
            overwrite: false,
            keep_work: false,
            lang: system_lang().into(),
        }
    }
}

/// Язык интерфейса по умолчанию — язык системы (русский или английский).
fn system_lang() -> &'static str {
    #[cfg(windows)]
    {
        extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        // Младшие 10 бит — идентификатор основного языка; 0x19 = русский.
        let id = unsafe { GetUserDefaultUILanguage() } as u32;
        if id & 0x3ff == 0x19 {
            return "ru";
        }
        "en"
    }
    #[cfg(not(windows))]
    {
        "en"
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        std::fs::read_to_string(path)
            .ok()
            // Файл мог быть сохранён редактором с BOM — иначе настройки «слетают».
            .map(|s| s.trim_start_matches('\u{feff}').to_string())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(settings_path(), text);
        }
    }
}

/// Каталог приложения: рядом с `.exe`.
///
/// В release-сборке путь всегда определяется по расположению исполняемого файла —
/// программа остаётся переносимой и не зависит от машины разработчика.
/// В debug-сборке допускается откат на каталог проекта, чтобы `cargo run`
/// находил `bin/` и `assets/`.
pub fn root_dir() -> PathBuf {
    use std::sync::OnceLock;
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if let Ok(p) = std::env::var("UPIZY_ROOT") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(dir) = exe_dir {
            #[cfg(debug_assertions)]
            {
                if dir.join("bin").exists() || dir.join("models").exists() {
                    return dir;
                }
            }
            #[cfg(not(debug_assertions))]
            {
                return dir;
            }
        }
        dev_root()
    })
    .clone()
}

#[cfg(debug_assertions)]
fn dev_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// В release-сборке путь проекта не попадает в бинарник: берём текущий каталог.
#[cfg(not(debug_assertions))]
fn dev_root() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

pub fn bin_dir() -> PathBuf {
    root_dir().join("bin")
}

pub fn work_dir() -> PathBuf {
    root_dir().join("_work")
}

pub fn settings_path() -> PathBuf {
    root_dir().join("settings.json")
}

pub fn tool_available(exe: &str) -> bool {
    let p = std::path::Path::new(exe);
    if p.is_absolute() {
        return p.exists();
    }
    std::process::Command::new(exe)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
