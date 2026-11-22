//! Консольный интерфейс: upizy-cli [опции] <файлы...>
//!
//! Язык сообщений выбирается переменной окружения `UPIZY_LANG` (ru / en)
//! или настройкой `lang` из settings.json.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;

use upscaler::config::{self, Settings};
use upscaler::i18n;
use upscaler::pipeline::{self, Ctx, Event};
use upscaler::{ai, encoders, ffmpeg, rife, tf};

fn help() {
    if i18n::is_en() {
        println!(
            r#"UPizy {ver} — CLI

Usage:
  upizy-cli [options] <file> [file...]

Main options:
  --target 4K|2K|2x|3x|1.5x|keep|custom   target resolution (default 4K)
                                     keep = no upscale, frame interpolation only
  --width N --height N               for --target custom
  --engine ai|fast|turbo|anime4k   ai = Real-ESRGAN, fast = GPU libplacebo,
                                     turbo = everything on the GPU (5-7x faster, a bit softer),
                                     anime4k = neural shader: an episode in ~15-20 min
  --a4k S|M|L                        Anime4K network size (default M)
  --model NAME                       realesrgan-x4plus | realesrgan-x4plus-anime | realesr-animevideov3
  --interp 1|2|4                     frame interpolation: 1 = off (default), 2/4 = x2/x4
  --interp-model NAME                auto | rife-anime (anime) | rife-v4.6 (live action)
  --quality max|high|balanced        encoding quality profile
  --encoder auto|hevc_nvenc|h264_nvenc|libx265|libx264|libsvtav1
  --container mkv|mp4                output container (default mkv)
  --sharpen 0..1                     CAS sharpening after upscaling (default 0.25)
  --denoise                          light denoise before upscaling
  --hdr tonemap|preserve             policy for HDR sources
  --output DIR                       output folder
  --overwrite                        overwrite existing files
  --keep-work                        keep temporary frames

Service:
  --no-interp                        synonym for --interp 1 (disable frame doubling)
  --install-ai                       download the Real-ESRGAN AI engine (~45 MB)
  --install-rife                     download RIFE for frame interpolation (~410 MB)
  --list                             show available codecs/models/environment
  -h, --help                         this help

Examples:
  upizy-cli --target 4K "video.mp4"
  upizy-cli --target 2K --engine fast "film.mkv"
  upizy-cli --target keep --interp 2 "anime.mkv"      (30 -> 60 fps only)
  upizy-cli --target 4K --interp 2 --model realesr-animevideov3 "anime.mkv"
  upizy-cli --target 4K --quality max --sharpen 0.35 "clip.mp4"
  upizy-cli --install-ai --install-rife
"#,
            ver = config::VERSION
        );
        return;
    }
    println!(
        r#"UPizy {ver} — CLI

Использование:
  upizy-cli [опции] <файл> [файл...]

Основные опции:
  --target 4K|2K|2x|3x|1.5x|keep|custom   целевое разрешение (по умолчанию 4K)
                                     keep = без апскейла, только интерполяция кадров
  --width N --height N               для --target custom
  --engine ai|fast|turbo|anime4k   ai = Real-ESRGAN, fast = GPU libplacebo,
                                     turbo = всё на GPU (в 5-7 раз быстрее, чуть мягче),
                                     anime4k = нейросеть-шейдер: серия за ~15-20 мин
  --a4k S|M|L                        размер сети Anime4K (по умолчанию M)
  --model NAME                       realesrgan-x4plus | realesrgan-x4plus-anime | realesr-animevideov3
  --interp 1|2|4                     интерполяция кадров: 1 = выкл (по умолчанию), 2/4 = ×2/×4
  --interp-model NAME                auto | rife-anime (аниме) | rife-v4.6 (видео)
  --quality max|high|balanced        профиль качества кодирования
  --encoder auto|hevc_nvenc|h264_nvenc|libx265|libx264|libsvtav1
  --container mkv|mp4                контейнер результата (по умолчанию mkv)
  --sharpen 0..1                     резкость CAS после апскейла (по умолчанию 0.25)
  --denoise                          лёгкое подавление шума перед апскейлом
  --hdr tonemap|preserve             политика для HDR-источников
  --output DIR                       папка результатов
  --overwrite                        перезаписывать существующие файлы
  --keep-work                        не удалять временные кадры

Служебное:
  --no-interp                        синоним --interp 1 (выключить удвоение кадров)
  --install-ai                       скачать AI-движок Real-ESRGAN (~45 МБ)
  --install-rife                     скачать RIFE для интерполяции кадров (~410 МБ)
  --list                             показать доступные кодеки/модели/окружение
  -h, --help                         эта справка

Примеры:
  upizy-cli --target 4K "video.mp4"
  upizy-cli --target 2K --engine fast "film.mkv"
  upizy-cli --target keep --interp 2 "anime.mkv"      (только 30 -> 60 fps)
  upizy-cli --target 4K --interp 2 --model realesr-animevideov3 "anime.mkv"
  upizy-cli --target 4K --quality max --sharpen 0.35 "clip.mp4"
  upizy-cli --install-ai --install-rife
"#,
        ver = config::VERSION
    );
}

fn main() {
    // Язык: переменная окружения важнее сохранённой настройки.
    match std::env::var("UPIZY_LANG").ok().map(|v| i18n::parse(&v)) {
        Some(lang) => i18n::set(lang),
        None => i18n::set(i18n::parse(&Settings::load().lang)),
    }

    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || argv.iter().any(|a| a == "-h" || a == "--help") {
        help();
        return;
    }

    // CLI всегда стартует с явных значений по умолчанию, а не с настроек GUI:
    // иначе результат зависит от того, что последним выбрали в окне.
    let mut st = Settings::default();
    st.output_dir.clear();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    let mut install_ai = false;
    let mut install_rife = false;

    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "--target" => {
                i += 1;
                st.target = argv.get(i).cloned().unwrap_or_else(|| "4K".into());
            }
            "--width" => {
                i += 1;
                st.custom_w = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(2560);
            }
            "--height" => {
                i += 1;
                st.custom_h = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(1440);
            }
            "--engine" => {
                i += 1;
                st.engine = argv.get(i).cloned().unwrap_or_else(|| "ai".into());
            }
            "--model" => {
                i += 1;
                st.ai_model = argv.get(i).cloned().unwrap_or_else(|| "auto".into());
            }
            "--a4k" => {
                i += 1;
                st.a4k_size = argv.get(i).cloned().unwrap_or_else(|| "M".into());
            }
            "--quality" => {
                i += 1;
                st.quality = argv.get(i).cloned().unwrap_or_else(|| "high".into());
            }
            "--encoder" => {
                i += 1;
                st.encoder = argv.get(i).cloned().unwrap_or_else(|| "auto".into());
            }
            "--container" => {
                i += 1;
                st.container = argv.get(i).cloned().unwrap_or_else(|| "mkv".into());
            }
            "--sharpen" => {
                i += 1;
                st.sharpen = argv.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.25);
            }
            "--hdr" => {
                i += 1;
                st.hdr_policy = argv.get(i).cloned().unwrap_or_else(|| "tonemap".into());
            }
            "--interp" => {
                i += 1;
                st.interp = match argv.get(i).map(|v| v.as_str()) {
                    Some("2") => 2,
                    Some("4") => 4,
                    Some("1") | Some("0") | Some("off") => 1,
                    Some(other) => {
                        eprintln!(
                            "{}",
                            tf!(
                                "--interp: ожидалось 1, 2 или 4 (получено {other})",
                                "--interp: expected 1, 2 or 4 (got {other})"
                            )
                        );
                        std::process::exit(2);
                    }
                    None => 1,
                };
            }
            "--no-interp" => st.interp = 1,
            "--interp-model" => {
                i += 1;
                st.interp_model = argv.get(i).cloned().unwrap_or_else(|| "auto".into());
            }
            "--output" | "-o" => {
                i += 1;
                st.output_dir = argv.get(i).cloned().unwrap_or_default();
            }
            "--denoise" => st.denoise = true,
            "--overwrite" => st.overwrite = true,
            "--keep-work" => st.keep_work = true,
            "--install-ai" => install_ai = true,
            "--install-rife" => install_rife = true,
            "--list" => {
                print_env();
                return;
            }
            other if other.starts_with('-') => {
                eprintln!(
                    "{}",
                    tf!("Неизвестная опция: {other}", "Unknown option: {other}")
                );
                std::process::exit(2);
            }
            other => files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    if install_ai {
        match ai::download(|m| println!("{m}")) {
            Ok(()) => println!(
                "{}",
                tf!(
                    "AI-движок установлен: {}",
                    "AI engine installed: {}",
                    ai::exe_path().unwrap_or_default().display()
                )
            ),
            Err(e) => {
                eprintln!("{}", tf!("Ошибка: {e:#}", "Error: {e:#}"));
                std::process::exit(1);
            }
        }
        if files.is_empty() && !install_rife {
            return;
        }
    }

    if install_rife {
        match rife::download(|m| println!("{m}")) {
            Ok(()) => println!(
                "{}",
                tf!(
                    "RIFE установлен: {}",
                    "RIFE installed: {}",
                    rife::exe_path().unwrap_or_default().display()
                )
            ),
            Err(e) => {
                eprintln!("{}", tf!("Ошибка: {e:#}", "Error: {e:#}"));
                std::process::exit(1);
            }
        }
        if files.is_empty() {
            return;
        }
    }

    if files.is_empty() {
        eprintln!(
            "{}",
            i18n::s(
                "Не указаны файлы. Смотрите --help.",
                "No input files. See --help."
            )
        );
        std::process::exit(2);
    }

    if !ffmpeg::ffmpeg_available() {
        eprintln!(
            "{}",
            i18n::s("ffmpeg не найден в PATH.", "ffmpeg not found in PATH.")
        );
        std::process::exit(1);
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel::<Event>();
    let ctx = Ctx::new(tx, cancel.clone());

    let printer = std::thread::spawn(move || {
        use std::io::Write;
        while let Ok(ev) = rx.recv() {
            match ev {
                Event::Log(m) => {
                    println!("\r{m}");
                    let _ = std::io::stdout().flush();
                }
                Event::Progress { fraction, stage } => {
                    print!("\r{stage:<14} {:>6.1}%   ", fraction * 100.0);
                    let _ = std::io::stdout().flush();
                }
                Event::AllFinished { .. } => {
                    println!();
                }
                _ => {}
            }
        }
    });

    let (mut ok, mut failed) = (0usize, 0usize);
    for path in &files {
        match pipeline::upscale_file(path, &st, &ctx) {
            Ok(out) => {
                ok += 1;
                println!("{}", tf!("Готово: {}", "Done: {}", out.display()));
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "{}",
                    tf!("Ошибка ({}): {e:#}", "Error ({}): {e:#}", path.display())
                );
            }
        }
    }

    drop(ctx);
    let _ = printer.join();
    println!(
        "{}",
        tf!(
            "Итог: {ok} успешно, {failed} с ошибкой.",
            "Result: {ok} succeeded, {failed} failed."
        )
    );
    if failed > 0 {
        std::process::exit(1);
    }
}

fn print_env() {
    let yes = i18n::s("да", "yes");
    let no = i18n::s("нет", "no");
    println!("UPizy {}", config::VERSION);
    println!("ffmpeg: {}", ffmpeg::ffmpeg());
    println!("  {}", ffmpeg::ffmpeg_version());
    println!(
        "{}: {}",
        i18n::s("AI-движок", "AI engine"),
        if ai::installed() {
            i18n::s("установлен", "installed")
        } else {
            i18n::s("не установлен (--install-ai)", "not installed (--install-ai)")
        }
    );
    if ai::installed() {
        println!(
            "  {}: {}",
            i18n::s("модели", "models"),
            ai::models().join(", ")
        );
    }
    let encs: Vec<&str> = ["hevc_nvenc", "h264_nvenc", "libx265", "libx264", "libsvtav1"]
        .into_iter()
        .filter(|e| ffmpeg::available_encoders().contains(*e))
        .collect();
    println!("{}: {}", i18n::s("Кодеки", "Codecs"), encs.join(", "));
    println!(
        "libplacebo: {}",
        if ffmpeg::has_filter("libplacebo") {
            yes
        } else {
            no
        }
    );
    println!(
        "{}: {}",
        i18n::s("RIFE (интерполяция)", "RIFE (interpolation)"),
        if rife::installed() {
            tf!(
                "установлен ({})",
                "installed ({})",
                rife::models().join(", ")
            )
        } else {
            i18n::s(
                "не установлен (--install-rife)",
                "not installed (--install-rife)"
            )
            .to_string()
        }
    );
    println!(
        "{}: {}",
        i18n::s(
            "Выбранный по умолчанию кодек",
            "Default encoder resolved"
        ),
        encoders::resolve_encoder("auto")
    );
}
