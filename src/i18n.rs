//! Локализация: русский и английский языки интерфейса.
//!
//! Язык хранится в настройках (`Settings::lang`) и дублируется в глобальной
//! переменной, чтобы фоновые потоки (конвейер, загрузка движков) писали
//! сообщения на выбранном языке.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Ru,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Ru => "Русский",
            Lang::En => "English",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn parse(code: &str) -> Lang {
    if code.trim().eq_ignore_ascii_case("en") {
        Lang::En
    } else {
        Lang::Ru
    }
}

pub fn set(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Ru => 0,
            Lang::En => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::Ru
    }
}

pub fn is_en() -> bool {
    current() == Lang::En
}

/// Статическая строка на текущем языке (для логов и служебных сообщений).
pub fn s(ru: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        ru
    }
}

/// Форматированная строка на текущем языке: `tf!("было {}", "was {}", n)`.
#[macro_export]
macro_rules! tf {
    ($ru:literal, $en:literal) => {{
        if $crate::i18n::is_en() {
            ::std::format!($en)
        } else {
            ::std::format!($ru)
        }
    }};
    ($ru:literal, $en:literal, $($arg:tt)*) => {{
        if $crate::i18n::is_en() {
            ::std::format!($en, $($arg)*)
        } else {
            ::std::format!($ru, $($arg)*)
        }
    }};
}

// ---------------------------------------------------------------------------
// Списки вариантов для выпадающих меню
// ---------------------------------------------------------------------------

const TARGETS_RU: &[(&str, &str)] = &[
    ("4K", "4K — 2160p (3840×2160)"),
    ("2K", "2K — 1440p (2560×1440)"),
    ("2x", "×2 (удвоение)"),
    ("3x", "×3"),
    ("1.5x", "×1.5"),
    ("keep", "Без апскейла — только плавность"),
    ("custom", "Своё разрешение"),
];

const TARGETS_EN: &[(&str, &str)] = &[
    ("4K", "4K — 2160p (3840×2160)"),
    ("2K", "2K — 1440p (2560×1440)"),
    ("2x", "×2 (double)"),
    ("3x", "×3"),
    ("1.5x", "×1.5"),
    ("keep", "No upscale — smoothness only"),
    ("custom", "Custom resolution"),
];

const INTERP_RU: &[(u32, &str)] = &[
    (1, "Выключена (как в оригинале)"),
    (2, "×2 — удвоение кадров"),
    (4, "×4 — максимум плавности"),
];

const INTERP_EN: &[(u32, &str)] = &[
    (1, "Off (as in the source)"),
    (2, "×2 — double frame rate"),
    (4, "×4 — maximum smoothness"),
];

const INTERP_MODELS_RU: &[(&str, &str)] = &[
    ("auto", "Авто (под модель апскейла)"),
    ("rife-anime", "rife-anime — аниме/мультфильмы"),
    ("rife-v4.6", "rife-v4.6 — видео/фильмы"),
];

const INTERP_MODELS_EN: &[(&str, &str)] = &[
    ("auto", "Auto (match the upscale model)"),
    ("rife-anime", "rife-anime — anime/cartoons"),
    ("rife-v4.6", "rife-v4.6 — live action/films"),
];

const ENGINES_RU: &[(&str, &str)] = &[
    ("ai", "AI · Real-ESRGAN (максимум деталей)"),
    ("anime4k", "Аниме · Anime4K (нейросеть в шейдере, минуты)"),
    ("fast", "GPU · libplacebo (быстро, весь фильм)"),
    ("turbo", "Турбо · CUDA (минуты вместо часов)"),
];

const ENGINES_EN: &[(&str, &str)] = &[
    ("ai", "AI · Real-ESRGAN (maximum detail)"),
    ("anime4k", "Anime · Anime4K (neural shader, minutes)"),
    ("fast", "GPU · libplacebo (fast, whole movie)"),
    ("turbo", "Turbo · CUDA (minutes, not hours)"),
];

const A4K_RU: &[(&str, &str)] = &[
    ("S", "S · быстрее всех"),
    ("M", "M · баланс"),
    ("L", "L · максимум деталей"),
];

const A4K_EN: &[(&str, &str)] = &[
    ("S", "S · fastest"),
    ("M", "M · balanced"),
    ("L", "L · maximum detail"),
];

const MODELS_RU: &[(&str, &str)] = &[
    ("auto", "Авто · видео-модель (быстро)"),
    ("realesr-animevideov3", "animevideo v3 — видео, быстро"),
    ("realesrgan-x4plus", "x4plus — макс. детали (медленно)"),
    ("realesrgan-x4plus-anime", "x4plus-anime — аниме (медленно)"),
];

const MODELS_EN: &[(&str, &str)] = &[
    ("auto", "Auto · video model (fast)"),
    ("realesr-animevideov3", "animevideo v3 — video, fast"),
    ("realesrgan-x4plus", "x4plus — max detail (slow)"),
    ("realesrgan-x4plus-anime", "x4plus-anime — anime (slow)"),
];

const QUALITY_RU: &[(&str, &str)] = &[
    ("max", "Максимум (большой файл)"),
    ("high", "Высокое"),
    ("balanced", "Сбалансированное"),
];

const QUALITY_EN: &[(&str, &str)] = &[
    ("max", "Maximum (large file)"),
    ("high", "High"),
    ("balanced", "Balanced"),
];

const CONTAINERS_RU: &[(&str, &str)] = &[
    ("mkv", "MKV (всё сохраняется)"),
    ("mp4", "MP4 (совместимость)"),
];

const CONTAINERS_EN: &[(&str, &str)] = &[
    ("mkv", "MKV (keeps everything)"),
    ("mp4", "MP4 (compatibility)"),
];

/// Готовые пресеты: имя, движок, вариант Anime4K, интерполяция, резкость, цель.
pub type Preset = (&'static str, &'static str, &'static str, u32, f32, &'static str);

const PRESETS_RU: &[Preset] = &[
    ("Аниме · быстро", "anime4k", "M", 1, 0.25, "4K"),
    ("Аниме · плавно", "anime4k", "M", 2, 0.15, "4K"),
    ("Турбо · 5 минут", "turbo", "M", 1, 0.0, "4K"),
    ("Максимум деталей", "ai", "M", 1, 0.25, "4K"),
    ("Только плавность", "fast", "M", 2, 0.25, "keep"),
];

const PRESETS_EN: &[Preset] = &[
    ("Anime · fast", "anime4k", "M", 1, 0.25, "4K"),
    ("Anime · smooth", "anime4k", "M", 2, 0.15, "4K"),
    ("Turbo · 5 min", "turbo", "M", 1, 0.0, "4K"),
    ("Max detail", "ai", "M", 1, 0.25, "4K"),
    ("Smoothness only", "fast", "M", 2, 0.25, "keep"),
];

fn pick<'a, T: ?Sized>(lang: Lang, ru: &'a T, en: &'a T) -> &'a T {
    match lang {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

pub fn targets(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, TARGETS_RU, TARGETS_EN)
}

pub fn interp(lang: Lang) -> &'static [(u32, &'static str)] {
    pick(lang, INTERP_RU, INTERP_EN)
}

pub fn interp_models(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, INTERP_MODELS_RU, INTERP_MODELS_EN)
}

pub fn engines(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, ENGINES_RU, ENGINES_EN)
}

pub fn a4k(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, A4K_RU, A4K_EN)
}

pub fn models(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, MODELS_RU, MODELS_EN)
}

pub fn quality(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, QUALITY_RU, QUALITY_EN)
}

pub fn containers(lang: Lang) -> &'static [(&'static str, &'static str)] {
    pick(lang, CONTAINERS_RU, CONTAINERS_EN)
}

pub fn presets(lang: Lang) -> &'static [Preset] {
    pick(lang, PRESETS_RU, PRESETS_EN)
}
