//! Графический интерфейс (egui/eframe).

use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use upscaler::config::{Settings, VIDEO_EXTS, VERSION};
use upscaler::i18n::{self, Lang};
use upscaler::pipeline::{self, Ctx, Event};
use upscaler::{ai, ffmpeg, rife, tf};

const BG: egui::Color32 = egui::Color32::from_rgb(16, 18, 22);
const CARD: egui::Color32 = egui::Color32::from_rgb(24, 27, 33);
const STROKE: egui::Color32 = egui::Color32::from_rgb(45, 50, 60);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 160, 255);
const OK_C: egui::Color32 = egui::Color32::from_rgb(94, 200, 128);
const WARN_C: egui::Color32 = egui::Color32::from_rgb(240, 180, 90);
const ERR_C: egui::Color32 = egui::Color32::from_rgb(235, 100, 100);
const MUTED: egui::Color32 = egui::Color32::from_rgb(140, 150, 165);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Queued,
    Running,
    Done,
    Failed,
}

struct Entry {
    path: PathBuf,
    summary: String,
    note: String,
    status: Status,
}

// Списки вариантов для меню живут в модуле i18n: у каждого языка свои подписи,
// а ключи (`4K`, `anime4k`, ...) одинаковые.

pub fn run() -> eframe::Result {
    let mut initial: Vec<PathBuf> = Vec::new();
    let mut autostart = false;
    for arg in std::env::args().skip(1) {
        if arg == "--start" {
            autostart = true;
        } else {
            initial.push(PathBuf::from(arg));
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([980.0, 600.0])
            .with_title("UPizy — 2K / 4K")
            .with_icon(std::sync::Arc::new(app_icon())),
        ..Default::default()
    };
    eframe::run_native(
        "UPizy",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc);
            app.autostart = autostart;
            for p in initial {
                app.add_path(p);
            }
            Ok(Box::new(app))
        }),
    )
}

struct App {
    st: Settings,
    files: Vec<Entry>,
    log: Vec<String>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    running: bool,
    downloading: bool,
    progress: f32,
    stage: String,
    status_line: String,
    ffmpeg_line: String,
    ai_ready: bool,
    rife_ready: bool,
    downloading_rife: bool,
    last_save: Instant,
    job_start: Option<Instant>,
    eta: String,
    autostart: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BG;
        visuals.window_fill = BG;
        visuals.extreme_bg_color = egui::Color32::from_rgb(12, 14, 17);
        visuals.faint_bg_color = CARD;
        visuals.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, STROKE);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(32, 36, 44);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(42, 48, 58);
        visuals.widgets.active.bg_fill = ACCENT.gamma_multiply(0.7);
        cc.egui_ctx.set_visuals(visuals);

        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        cc.egui_ctx.set_style(style);

        let (tx, rx) = channel();
        let ai_ready = ai::installed();
        let rife_ready = rife::installed();
        let st = Settings::load();
        i18n::set(i18n::parse(&st.lang));
        let ffmpeg_line = if ffmpeg::ffmpeg_available() {
            ffmpeg::ffmpeg_version()
        } else {
            i18n::s("ffmpeg не найден", "ffmpeg not found").to_string()
        };

        let mut app = Self {
            st,
            files: Vec::new(),
            log: Vec::new(),
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            running: false,
            downloading: false,
            progress: 0.0,
            stage: i18n::s("Готов", "Ready").into(),
            status_line: String::new(),
            ffmpeg_line,
            ai_ready,
            rife_ready,
            downloading_rife: false,
            last_save: Instant::now(),
            job_start: None,
            eta: String::new(),
            autostart: false,
        };
        app.push_log(format!("UPizy v{VERSION}"));
        app.push_log(format!("ffmpeg: {}", app.ffmpeg_line));
        if app.ai_ready {
            app.push_log(tf!(
                "AI-модели: {}",
                "AI models: {}",
                ai::models().join(", ")
            ));
        } else {
            app.push_log(
                i18n::s(
                    "AI-движок не установлен — доступен быстрый GPU-режим.",
                    "AI engine is not installed — fast GPU mode is available.",
                )
                .to_string(),
            );
        }
        if app.rife_ready {
            app.push_log(tf!(
                "RIFE (интерполяция): {}",
                "RIFE (interpolation): {}",
                rife::models().join(", ")
            ));
        } else {
            app.push_log(
                i18n::s(
                    "RIFE не установлен — интерполяция кадров через minterpolate.",
                    "RIFE is not installed — frames will be interpolated with minterpolate.",
                )
                .to_string(),
            );
        }
        app
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 1500 {
            self.log.drain(0..500);
        }
    }

    fn add_path(&mut self, path: PathBuf) {
        let mut found: Vec<PathBuf> = Vec::new();
        if path.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&path) {
                for e in rd.flatten() {
                    let p = e.path();
                    if is_video(&p) {
                        found.push(p);
                    }
                }
            }
        } else if is_video(&path) {
            found.push(path);
        }
        for p in found {
            if self.files.iter().any(|f| f.path == p) {
                continue;
            }
            self.files.push(Entry {
                path: p.clone(),
                summary: i18n::s("чтение…", "reading…").into(),
                note: String::new(),
                status: Status::Queued,
            });
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let summary = match upscaler::probe::probe(&p) {
                    Ok(info) => info.summary(),
                    Err(e) => tf!("ошибка: {e}", "error: {e}"),
                };
                let _ = tx.send(Event::Probed { path: p, summary });
            });
        }
    }

    fn start(&mut self) {
        if self.running {
            return;
        }
        if !ffmpeg::ffmpeg_available() {
            self.push_log(i18n::s(
                "✖ ffmpeg не найден. Установите ffmpeg и добавьте его в PATH.",
                "✖ ffmpeg not found. Install ffmpeg and add it to PATH.",
            ));
            return;
        }
        let pending: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|f| f.status != Status::Done)
            .map(|f| f.path.clone())
            .collect();
        if pending.is_empty() {
            self.push_log(i18n::s(
                "Нет файлов в очереди — добавьте видео.",
                "No files in the queue — add a video.",
            ));
            return;
        }
        if self.st.engine == "ai" && !self.ai_ready {
            self.push_log(i18n::s(
                "AI-движок не установлен — используется быстрый GPU-режим.",
                "AI engine is not installed — using fast GPU mode.",
            ));
        }
        if self.st.interp > 1 && !self.rife_ready {
            self.push_log(i18n::s(
                "RIFE не установлен — интерполяция будет выполнена ffmpeg minterpolate.",
                "RIFE is not installed — interpolation will use ffmpeg minterpolate.",
            ));
        }

        self.st.save();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.running = true;
        self.progress = 0.0;
        self.stage = i18n::s("Подготовка", "Preparing").into();
        self.status_line = tf!("В очереди: {}", "Queued: {}", pending.len());

        let cancel = self.cancel.clone();
        let tx = self.tx.clone();
        let st = self.st.clone();
        std::thread::spawn(move || {
            let ctx = Ctx::new(tx.clone(), cancel.clone());
            let (mut ok, mut failed) = (0usize, 0usize);
            for path in pending {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                match pipeline::upscale_file(&path, &st, &ctx) {
                    Ok(_) => {
                        ok += 1;
                        let _ = tx.send(Event::FileFinished {
                            path,
                            ok: true,
                            message: String::new(),
                        });
                    }
                    Err(e) => {
                        failed += 1;
                        let msg = format!("{e:#}");
                        let _ = tx.send(Event::Log(format!("✖ {msg}")));
                        let _ = tx.send(Event::FileFinished {
                            path,
                            ok: false,
                            message: msg,
                        });
                    }
                }
            }
            let _ = tx.send(Event::AllFinished { ok, failed });
        });
    }

    fn download_ai(&mut self) {
        if self.downloading {
            return;
        }
        self.downloading = true;
        self.push_log(
            i18n::s("Загрузка AI-движка…", "Downloading the AI engine…").to_string(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let logger = {
                let tx = tx.clone();
                move |m: String| {
                    let _ = tx.send(Event::Log(m));
                }
            };
            let ok = match ai::download(logger) {
                Ok(()) => true,
                Err(e) => {
                    let _ = tx.send(Event::Log(format!("✖ {e:#}")));
                    false
                }
            };
            let _ = tx.send(Event::AiReady(ok));
        });
    }

    fn download_rife(&mut self) {
        if self.downloading_rife {
            return;
        }
        self.downloading_rife = true;
        self.push_log(
            i18n::s(
                "Загрузка RIFE для интерполяции кадров…",
                "Downloading RIFE for frame interpolation…",
            )
            .to_string(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let logger = {
                let tx = tx.clone();
                move |m: String| {
                    let _ = tx.send(Event::Log(m));
                }
            };
            let ok = match rife::download(logger) {
                Ok(()) => true,
                Err(e) => {
                    let _ = tx.send(Event::Log(format!("✖ {e:#}")));
                    false
                }
            };
            let _ = tx.send(Event::RifeReady(ok));
        });
    }

    fn poll_events(&mut self) -> bool {
        let mut got = false;
        while let Ok(ev) = self.rx.try_recv() {
            got = true;
            match ev {
                Event::Log(m) => self.push_log(m),
                Event::Progress { fraction, stage } => {
                    self.progress = fraction;
                    if !stage.is_empty() {
                        self.stage = stage;
                    }
                    if let Some(t0) = self.job_start {
                        if fraction > 0.02 && self.running {
                            let elapsed = t0.elapsed().as_secs_f64();
                            let remaining = (elapsed / fraction as f64 - elapsed).max(0.0);
                            self.eta = tf!("осталось ≈ {}", "left ≈ {}", hms(remaining));
                        }
                    }
                }
                Event::FileStarted { path, info } => {
                    self.job_start = Some(Instant::now());
                    self.eta.clear();
                    if let Some(f) = self.files.iter_mut().find(|f| f.path == path) {
                        f.status = Status::Running;
                        f.summary = info;
                        f.note = i18n::s("в работе", "working").into();
                    }
                }
                Event::FileFinished { path, ok, message } => {
                    if let Some(f) = self.files.iter_mut().find(|f| f.path == path) {
                        f.status = if ok { Status::Done } else { Status::Failed };
                        f.note = if ok {
                            i18n::s("готово", "done").into()
                        } else {
                            message
                                .lines()
                                .next()
                                .unwrap_or(i18n::s("ошибка", "error"))
                                .to_string()
                        };
                    }
                }
                Event::Probed { path, summary } => {
                    if let Some(f) = self.files.iter_mut().find(|f| f.path == path) {
                        f.summary = summary;
                    }
                }
                Event::AllFinished { ok, failed } => {
                    self.running = false;
                    self.stage = i18n::s("Готов", "Ready").into();
                    self.eta.clear();
                    self.job_start = None;
                    self.status_line = tf!(
                        "Завершено: успешно {ok}, с ошибкой {failed}",
                        "Finished: {ok} succeeded, {failed} failed"
                    );
                    self.push_log(tf!(
                        "━━ Готово: {ok} успешно, {failed} с ошибкой",
                        "━━ Done: {ok} succeeded, {failed} failed"
                    ));
                }
                Event::AiReady(ok) => {
                    self.downloading = false;
                    self.ai_ready = ai::installed();
                    if ok && self.ai_ready {
                        self.push_log(i18n::s("✔ AI-движок готов.", "✔ AI engine is ready."));
                    }
                }
                Event::RifeReady(ok) => {
                    self.downloading_rife = false;
                    self.rife_ready = rife::installed();
                    if ok && self.rife_ready {
                        self.push_log(i18n::s(
                            "✔ RIFE готов: интерполяция кадров доступна.",
                            "✔ RIFE is ready: frame interpolation is available.",
                        ));
                    }
                }
            }
        }
        got
    }
}

fn combo_num(ui: &mut egui::Ui, label: &str, value: &mut u32, options: &[(u32, &str)]) {
    let current = options
        .iter()
        .find(|(k, _)| *k == *value)
        .map(|(_, v)| *v)
        .unwrap_or("—");
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED).size(12.0));
    });
    egui::ComboBox::from_id_salt(label)
        .selected_text(egui::RichText::new(current).size(13.0))
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            for (key, text) in options {
                ui.selectable_value(value, *key, *text);
            }
        });
}

fn hms(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

fn is_video(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn install_fonts(ctx: &egui::Context) {
    // Segoe UI даёт корректную кириллицу, а Symbol/Emoji — недостающие глифы.
    let mut fonts = egui::FontDefinitions::default();
    let mut ok = false;
    for (_name, path) in [
        ("ui", "C:/Windows/Fonts/segoeui.ttf"),
        ("ui", "C:/Windows/Fonts/tahoma.ttf"),
        ("ui", "C:/Windows/Fonts/arial.ttf"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui".to_owned(), egui::FontData::from_owned(bytes).into());
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, "ui".to_owned());
            }
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                list.push("ui".to_owned());
            }
            ok = true;
            break;
        }
    }
    if ok {
        for (name, path) in [
            ("sym", "C:/Windows/Fonts/seguisym.ttf"),
            ("emoji", "C:/Windows/Fonts/seguiemj.ttf"),
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    if let Some(list) = fonts.families.get_mut(&family) {
                        list.push(name.to_owned());
                    }
                }
            }
        }
        ctx.set_fonts(fonts);
    }
}

/// Простая иконка приложения: синий круг со стрелкой вверх.
fn app_icon() -> egui::IconData {
    let s: u32 = 64;
    let mut rgba = Vec::with_capacity((s * s * 4) as usize);
    let c = 31.5f32;
    for y in 0..s {
        for x in 0..s {
            let px = x as f32;
            let py = y as f32;
            let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            let alpha = if d <= 29.5 {
                255
            } else if d <= 31.5 {
                ((31.5 - d) * 127.0) as u8
            } else {
                0
            };
            let arrow = (py >= 13.0 && py <= 29.0 && (px - c).abs() <= (py - 13.0) * 1.05)
                || (py > 29.0 && py <= 49.0 && (px - c).abs() <= 4.5);
            let (r, g, b, a) = if arrow {
                (255, 255, 255, 255)
            } else {
                let t = py / s as f32;
                (
                    (40.0 + 40.0 * (1.0 - t)) as u8,
                    (110.0 + 30.0 * (1.0 - t)) as u8,
                    (225.0 + 20.0 * (1.0 - t)) as u8,
                    alpha,
                )
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    egui::IconData {
        rgba,
        width: s,
        height: s,
    }
}

fn combo(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[(&str, &str)]) {    let current = options
        .iter()
        .find(|(k, _)| k == value)
        .map(|(_, v)| *v)
        .unwrap_or(value.as_str());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(MUTED).size(12.0));
    });
    egui::ComboBox::from_id_salt(label)
        .selected_text(egui::RichText::new(current).size(13.0))
        .width(ui.available_width() - 8.0)
        .show_ui(ui, |ui| {
            for (key, text) in options {
                ui.selectable_value(value, (*key).to_string(), *text);
            }
        });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let got = self.poll_events();

        // drag & drop файлов
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for p in dropped {
            self.add_path(p);
        }

        if self.last_save.elapsed() > Duration::from_secs(2) {
            self.st.save();
            self.last_save = Instant::now();
        }

        if self.autostart && !self.files.is_empty() {
            self.autostart = false;
            self.start();
        }

        // ---------------- заголовок ----------------
        let lang = i18n::parse(&self.st.lang);
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::NONE.fill(CARD).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("UPizy")
                            .size(21.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("720p / 1080p → 2K / 4K")
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("v{VERSION}"))
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.add_space(10.0);
                        if ui
                            .selectable_label(lang == Lang::En, "EN")
                            .on_hover_text("English")
                            .clicked()
                        {
                            self.st.lang = Lang::En.code().into();
                            i18n::set(Lang::En);
                        }
                        if ui
                            .selectable_label(lang == Lang::Ru, "RU")
                            .on_hover_text("Русский")
                            .clicked()
                        {
                            self.st.lang = Lang::Ru.code().into();
                            i18n::set(Lang::Ru);
                        }
                        ui.add_space(10.0);
                        if self.ai_ready {
                            ui.label(
                                egui::RichText::new(tf!("● AI готов", "● AI ready"))
                                    .size(12.0)
                                    .color(OK_C),
                            );
                        } else if self.downloading {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(
                                egui::RichText::new(tf!("загрузка AI…", "downloading AI…"))
                                    .size(12.0)
                                    .color(WARN_C),
                            );
                        } else if ui
                            .add(egui::Button::new(
                                egui::RichText::new(tf!("Скачать AI", "Download AI")).size(12.0),
                            ))
                            .on_hover_text(tf!(
                                "Real-ESRGAN ncnn-vulkan, ~45 МБ",
                                "Real-ESRGAN ncnn-vulkan, ~45 MB"
                            ))
                            .clicked()
                        {
                            self.download_ai();
                        }
                    });
                });
            });

        // ---------------- нижняя панель ----------------
        egui::TopBottomPanel::bottom("bottom")
            .resizable(true)
            .default_height(205.0)
            .frame(egui::Frame::NONE.fill(CARD).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let start_text = if self.running {
                        tf!("Работает…", "Working…")
                    } else {
                        tf!("▶  Запустить", "▶  Start")
                    };
                    let start = egui::Button::new(
                        egui::RichText::new(start_text).size(14.0).strong(),
                    )
                    .fill(if self.running { STROKE } else { ACCENT })
                    .min_size(egui::vec2(150.0, 30.0));
                    if ui.add_enabled(!self.running, start).clicked() {
                        self.start();
                    }
                    let stop = egui::Button::new(
                        egui::RichText::new(tf!("■  Стоп", "■  Stop")).size(13.0),
                    )
                    .min_size(egui::vec2(90.0, 30.0));
                    if ui.add_enabled(self.running, stop).clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                        self.push_log(i18n::s("Остановка…", "Stopping…"));
                    }
                    if ui
                        .add(
                            egui::Button::new(tf!("Открыть папку", "Open folder"))
                                .min_size(egui::vec2(120.0, 30.0)),
                        )
                        .clicked()
                    {
                        open_output_dir(&self.st);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(&self.status_line)
                                .size(12.0)
                                .color(MUTED),
                        );
                    });
                });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&self.stage)
                            .size(12.5)
                            .color(egui::Color32::from_rgb(205, 214, 226)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.eta.is_empty() {
                            ui.label(egui::RichText::new(&self.eta).size(12.0).color(MUTED));
                            ui.add_space(10.0);
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.1}%", self.progress * 100.0))
                                .size(12.5)
                                .color(ACCENT),
                        );
                    });
                });
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .animate(self.running)
                        .desired_height(16.0),
                );
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in self.log.iter().rev().take(300).rev() {
                            let color = if line.starts_with('✖') {
                                ERR_C
                            } else if line.starts_with('✔') || line.starts_with("━━") {
                                OK_C
                            } else if line.starts_with('▶') {
                                ACCENT
                            } else {
                                egui::Color32::from_rgb(190, 198, 210)
                            };
                            ui.label(
                                egui::RichText::new(line)
                                    .monospace()
                                    .size(11.5)
                                    .color(color),
                            );
                        }
                    });
            });

        // ---------------- правая панель: настройки ----------------
        egui::SidePanel::right("settings")
            .resizable(true)
            .default_width(340.0)
            .width_range(300.0..=460.0)
            .frame(egui::Frame::NONE.fill(CARD).inner_margin(egui::Margin::symmetric(14, 12)))
            .show(ctx, |ui| {
                let mut st = self.st.clone();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    ui.label(
                        egui::RichText::new(tf!("НАСТРОЙКИ", "SETTINGS"))
                            .size(11.0)
                            .color(MUTED)
                            .strong(),
                    );
                    ui.add_space(4.0);

                    // --- готовые пресеты: одна кнопка вместо четырёх настроек ---
                    ui.label(
                        egui::RichText::new(tf!("Пресеты", "Presets"))
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.horizontal_wrapped(|ui| {
                        for (name, engine, a4k, interp, sharpen, target) in i18n::presets(lang) {
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(*name).size(10.5))
                                        .min_size(egui::vec2(0.0, 22.0)),
                                )
                                .clicked()
                            {
                                st.engine = (*engine).into();
                                st.a4k_size = (*a4k).into();
                                st.interp = *interp;
                                st.sharpen = *sharpen;
                                st.target = (*target).into();
                            }
                        }
                    });
                    ui.add_space(4.0);

                    combo(
                        ui,
                        tf!("Целевое разрешение", "Target resolution").as_str(),
                        &mut st.target,
                        i18n::targets(lang),
                    );
                    if st.target == "custom" {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut st.custom_w)
                                    .range(16..=7680)
                                    .prefix("W "),
                            );
                            ui.add(
                                egui::DragValue::new(&mut st.custom_h)
                                    .range(16..=4320)
                                    .prefix("H "),
                            );
                        });
                    }

                    ui.add_space(6.0);
                    combo(
                        ui,
                        tf!("Движок", "Engine").as_str(),
                        &mut st.engine,
                        i18n::engines(lang),
                    );
                    if st.engine == "turbo" {
                        ui.label(
                            egui::RichText::new(tf!(
                                "Вся цепочка на GPU: декодирование → scale_cuda lanczos → NVENC p1. \
Примерно в 5–7 раз быстрее libplacebo, но без CAS-резкости и deband.",
                                "The whole chain runs on the GPU: decode → scale_cuda lanczos → NVENC p1. \
About 5–7× faster than libplacebo, but without CAS sharpening and deband."
                            ))
                            .size(10.5)
                            .color(MUTED),
                        );
                    }
                    if st.engine == "anime4k" {
                        ui.label(
                            egui::RichText::new(tf!(
                                "Нейросеть Anime4K работает как шейдер внутри ffmpeg: \
без отдельных процессов и без обмена через диск. ~30 кадров/с при 1080p → 4K \
(серия за ~15–20 минут вместо 3 часов). С плавностью ×2 — примерно 2.5 часа.",
                                "The Anime4K neural network runs as an ffmpeg shader: \
no extra processes, no disk round-trips. ~30 fps at 1080p → 4K \
(a 23-min episode in ~15–20 min instead of 3 hours). With ×2 smoothing — about 2.5 hours."
                            ))
                            .size(10.5)
                            .color(OK_C),
                        );
                        combo(ui, "Anime4K", &mut st.a4k_size, i18n::a4k(lang));
                        ui.label(
                            egui::RichText::new(tf!(
                                "S/L — та же сеть, но мельче/крупнее: L даёт чуть больше \
мелких деталей и стоит на ~20 % дольше.",
                                "S/L — the same network, smaller/larger: L adds a little \
fine detail and takes ~20% longer."
                            ))
                            .size(10.0)
                            .color(MUTED),
                        );
                    }
                    if st.engine == "ai" {
                        if !self.ai_ready {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "AI не установлен — будет использован GPU-режим",
                                    "AI is not installed — GPU mode will be used"
                                ))
                                .size(11.0)
                                .color(WARN_C),
                            );
                        }
                        combo(ui, tf!("Модель", "Model").as_str(), &mut st.ai_model, i18n::models(lang));
                        let hint = if st.ai_model.contains("x4plus") {
                            tf!(
                                "≈ в 10 раз медленнее — для коротких клипов и максимума деталей",
                                "≈10× slower — for short clips and maximum detail"
                            )
                        } else {
                            tf!(
                                "≈ единицы кадров/с — подходит для полнометражных видео",
                                "a few frames/sec — suits feature-length videos"
                            )
                        };
                        ui.label(egui::RichText::new(hint).size(10.5).color(MUTED));
                    }

                    ui.add_space(6.0);
                    combo_num(
                        ui,
                        tf!("Плавность · интерполяция кадров", "Smoothness · frame interpolation")
                            .as_str(),
                        &mut st.interp,
                        i18n::interp(lang),
                    );
                    if st.interp > 1 {
                        if self.rife_ready {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "RIFE дорисовывает промежуточные кадры — без «мыла» и смазывания",
                                    "RIFE generates in-between frames — no blur or smearing"
                                ))
                                .size(10.5)
                                .color(OK_C),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "RIFE не установлен — будет minterpolate (заметно хуже)",
                                    "RIFE is not installed — minterpolate will be used (visibly worse)"
                                ))
                                .size(10.5)
                                .color(WARN_C),
                            );
                            if self.downloading_rife {
                                ui.horizontal(|ui| {
                                    ui.add(egui::Spinner::new().size(13.0));
                                    ui.label(
                                        egui::RichText::new(tf!("загрузка RIFE…", "downloading RIFE…"))
                                            .size(11.0)
                                            .color(WARN_C),
                                    );
                                });
                            } else if ui
                                .add(egui::Button::new(
                                    egui::RichText::new(tf!(
                                        "Скачать RIFE (~410 МБ)",
                                        "Download RIFE (~410 MB)"
                                    ))
                                    .size(11.5),
                                ))
                                .clicked()
                            {
                                self.download_rife();
                            }
                        }
                        combo(
                            ui,
                            tf!("Модель интерполяции", "Interpolation model").as_str(),
                            &mut st.interp_model,
                            i18n::interp_models(lang),
                        );
                        ui.label(
                            egui::RichText::new(tf!(
                                "×2: 24 → 48 fps, 30 → 60 fps. Отключить можно в любой момент — \
пункт «Выключена».",
                                "×2: 24 → 48 fps, 30 → 60 fps. You can turn it off at any time — \
choose “Off”."
                            ))
                            .size(10.0)
                            .color(MUTED),
                        );
                    }
                    if st.target == "keep" {
                        ui.label(
                            egui::RichText::new(tf!(
                                "Режим «только плавность»: разрешение и детали не меняются.",
                                "“Smoothness only” mode: resolution and detail stay unchanged."
                            ))
                            .size(10.5)
                            .color(MUTED),
                        );
                    }

                    ui.add_space(6.0);
                    combo(
                        ui,
                        tf!("Качество", "Quality").as_str(),
                        &mut st.quality,
                        i18n::quality(lang),
                    );

                    let encs: Vec<(String, String)> = ffmpeg::available_encoders()
                        .iter()
                        .filter(|e| {
                            matches!(
                                e.as_str(),
                                "hevc_nvenc"
                                    | "h264_nvenc"
                                    | "libx265"
                                    | "libx264"
                                    | "libsvtav1"
                            )
                        })
                        .map(|e| {
                            let nice = match e.as_str() {
                                "hevc_nvenc" => "H.265 NVENC (GPU)",
                                "h264_nvenc" => "H.264 NVENC (GPU)",
                                "libx265" => "H.265 x265 (CPU)",
                                "libx264" => "H.264 x264 (CPU)",
                                "libsvtav1" => "AV1 SVT (CPU)",
                                other => other,
                            };
                            (e.clone(), nice.to_string())
                        })
                        .collect();
                    let mut enc_opts: Vec<(&str, &str)> = vec![(
                        "auto",
                        i18n::s("Авто (лучший доступный)", "Auto (best available)"),
                    )];
                    for (k, v) in &encs {
                        enc_opts.push((k.as_str(), v.as_str()));
                    }
                    combo(
                        ui,
                        tf!("Кодек", "Codec").as_str(),
                        &mut st.encoder,
                        &enc_opts,
                    );
                    combo(
                        ui,
                        tf!("Контейнер", "Container").as_str(),
                        &mut st.container,
                        i18n::containers(lang),
                    );

                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{}: {:.2}",
                            tf!("Резкость (CAS)", "Sharpness (CAS)"),
                            st.sharpen
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                    ui.add_enabled(
                        st.engine != "turbo",
                        egui::Slider::new(&mut st.sharpen, 0.0..=1.0).show_value(false),
                    );
                    if st.engine == "turbo" {
                        ui.label(
                            egui::RichText::new(tf!(
                                "В турбо-режиме не применяется (CAS снимает кадры с GPU)",
                                "Not used in Turbo mode (CAS would copy frames off the GPU)"
                            ))
                            .size(10.0)
                            .color(MUTED),
                        );
                    }

                    ui.checkbox(
                        &mut st.denoise,
                        tf!("Лёгкое подавление шума", "Light denoise"),
                    );
                    ui.checkbox(
                        &mut st.overwrite,
                        tf!("Перезаписывать результаты", "Overwrite results"),
                    );

                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("HDR").size(12.0).color(MUTED));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut st.hdr_policy, "tonemap".into(), "→ SDR");
                        ui.selectable_value(
                            &mut st.hdr_policy,
                            "preserve".into(),
                            tf!("Сохранить HDR", "Keep HDR"),
                        );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(tf!("ПАПКА РЕЗУЛЬТАТОВ", "OUTPUT FOLDER"))
                            .size(11.0)
                            .color(MUTED)
                            .strong(),
                    );
                    let out_text = if st.output_dir.trim().is_empty() {
                        tf!("рядом с исходным файлом", "next to the source file")
                    } else {
                        st.output_dir.clone()
                    };
                    ui.label(egui::RichText::new(out_text).size(11.5).color(
                        egui::Color32::from_rgb(195, 205, 220),
                    ));
                    ui.horizontal(|ui| {
                        if ui.button(tf!("Выбрать…", "Browse…")).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                st.output_dir = dir.to_string_lossy().into_owned();
                            }
                        }
                        if ui.button(tf!("Сбросить", "Reset")).clicked() {
                            st.output_dir.clear();
                        }
                    });

                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(tf!(
                            "AI даёт максимум деталей, но медленный — для серий берите \
Anime4K (аниме) или Турбо, для фильмов — GPU-режим (libplacebo).",
                            "AI gives the most detail but is slow — use Anime4K (anime) or \
Turbo for series, and GPU mode (libplacebo) for films."
                        ))
                        .size(10.5)
                        .color(MUTED),
                    );
                });

                if st.target != self.st.target
                    || st.engine != self.st.engine
                    || st.ai_model != self.st.ai_model
                    || st.a4k_size != self.st.a4k_size
                    || st.interp != self.st.interp
                    || st.interp_model != self.st.interp_model
                    || st.quality != self.st.quality
                    || st.encoder != self.st.encoder
                    || st.container != self.st.container
                    || (st.sharpen - self.st.sharpen).abs() > f32::EPSILON
                    || st.denoise != self.st.denoise
                    || st.overwrite != self.st.overwrite
                    || st.hdr_policy != self.st.hdr_policy
                    || st.output_dir != self.st.output_dir
                    || st.custom_w != self.st.custom_w
                    || st.custom_h != self.st.custom_h
                    || st.lang != self.st.lang
                {
                    self.st = st;
                }
            });

        // ---------------- список файлов ----------------
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(tf!("+  Добавить файлы", "+  Add files")).size(13.0),
                        ))
                        .clicked()
                    {
                        let exts: Vec<&str> = VIDEO_EXTS.to_vec();
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter(tf!("Видео", "Video"), &exts)
                            .add_filter(tf!("Все файлы", "All files"), &["*"])
                            .pick_files()
                        {
                            for p in paths {
                                self.add_path(p);
                            }
                        }
                    }
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(tf!("+  Папка", "+  Folder")).size(13.0),
                        ))
                        .clicked()
                    {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.add_path(dir);
                        }
                    }
                    if ui
                        .add_enabled(
                            !self.files.is_empty() && !self.running,
                            egui::Button::new(tf!("Очистить", "Clear")),
                        )
                        .clicked()
                    {
                        self.files.clear();
                        self.progress = 0.0;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(tf!(
                                "файлов: {}",
                                "files: {}",
                                self.files.len()
                            ))
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
                });
                ui.add_space(6.0);

                if self.files.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label(
                            egui::RichText::new(tf!("Перетащите видео сюда", "Drop videos here"))
                                .size(20.0)
                                .color(MUTED),
                        );
                        ui.label(
                            egui::RichText::new(tf!(
                                "или нажмите «Добавить файлы»",
                                "or click “Add files”"
                            ))
                            .size(13.0)
                            .color(MUTED),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(tf!(
                                "Поддерживаются любые форматы, которые читает ffmpeg:\n\
mp4, mkv, avi, mov, wmv, webm, ts, m2ts, mpg, vob, flv и другие",
                                "Any format ffmpeg can read is supported:\n\
mp4, mkv, avi, mov, wmv, webm, ts, m2ts, mpg, vob, flv and more"
                            ))
                            .size(11.0)
                            .color(egui::Color32::from_rgb(105, 115, 130)),
                        );
                    });
                    return;
                }

                let mut remove: Option<usize> = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, f) in self.files.iter().enumerate() {
                            let (icon, color) = match f.status {
                                Status::Queued => ("•", MUTED),
                                Status::Running => ("▶", ACCENT),
                                Status::Done => ("✓", OK_C),
                                Status::Failed => ("✕", ERR_C),
                            };
                            egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(21, 24, 29))
                                .stroke(egui::Stroke::new(1.0_f32, STROKE))
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(icon).color(color).size(14.0));
                                        ui.vertical(|ui| {
                                            let name = f
                                                .path
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("?");
                                            ui.label(
                                                egui::RichText::new(name)
                                                    .size(13.0)
                                                    .color(egui::Color32::from_rgb(225, 230, 238)),
                                            );
                                            let sub = if f.note.is_empty() {
                                                f.summary.clone()
                                            } else {
                                                format!("{} · {}", f.summary, f.note)
                                            };
                                            ui.label(egui::RichText::new(sub).size(10.5).color(MUTED));
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add_enabled(
                                                        f.status != Status::Running,
                                                        egui::Button::new("×").small(),
                                                    )
                                                    .clicked()
                                                {
                                                    remove = Some(i);
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(4.0);
                        }
                    });
                if let Some(i) = remove {
                    self.files.remove(i);
                }
            });

        if self.running || got {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }
}

fn open_output_dir(st: &Settings) {
    let dir = if st.output_dir.trim().is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(st.output_dir.trim())
    };
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
