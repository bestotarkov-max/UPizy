//! AI-движок Real-ESRGAN (ncnn-vulkan): установка и запуск.

use anyhow::{anyhow, bail, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::bin_dir;
use crate::ffmpeg::hide_window;
use crate::i18n;
use crate::tf;

pub const ZIP_URL: &str = "https://github.com/xinntao/Real-ESRGAN/releases/download/\
v0.2.5.0/realesrgan-ncnn-vulkan-20220424-windows.zip";
pub const EXE_NAME: &str = "realesrgan-ncnn-vulkan.exe";

pub fn exe_path() -> Option<PathBuf> {
    let p = bin_dir().join(EXE_NAME);
    p.exists().then_some(p)
}

/// Установлен ли движок и есть ли файлы моделей.
pub fn installed() -> bool {
    match exe_path() {
        Some(p) => p
            .parent()
            .map(|d| d.join("models"))
            .map(|m| m.exists() && fs::read_dir(&m).map(|it| it.count() > 0).unwrap_or(false))
            .unwrap_or(false),
        None => false,
    }
}

/// Список моделей (по именам .param без суффикса масштаба).
pub fn models() -> Vec<String> {
    let Some(exe) = exe_path() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else { return Vec::new() };
    let models_dir = dir.join("models");
    let Ok(rd) = fs::read_dir(models_dir) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("param") {
            let mut stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            for suffix in ["-x2", "-x3", "-x4"] {
                if stem.ends_with(suffix) {
                    stem.truncate(stem.len() - suffix.len());
                }
            }
            if !stem.is_empty() && !names.contains(&stem) {
                names.push(stem);
            }
        }
    }
    names.sort();
    names
}

fn run_checked(mut cmd: Command, what: &str) -> Result<()> {
    hide_window(&mut cmd);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| {
            anyhow!(tf!(
                "{what}: не удалось запустить процесс: {e}",
                "{what}: failed to start process: {e}"
            ))
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ");
        bail!("{what}: {tail}");
    }
    Ok(())
}

/// Скачивает и распаковывает движок в bin/.
pub fn download(log: impl Fn(String)) -> Result<()> {
    if installed() {
        return Ok(());
    }
    let bin = bin_dir();
    fs::create_dir_all(&bin)?;
    let zip = std::env::temp_dir().join("realesrgan-ncnn-vulkan.zip");

    log(i18n::s(
        "Скачивание AI-движка Real-ESRGAN (~45 МБ)…",
        "Downloading the Real-ESRGAN AI engine (~45 MB)…",
    )
    .to_string());
    let mut curl = Command::new("curl.exe");
    curl.args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&zip)
        .arg(ZIP_URL);
    if let Err(e) = run_checked(curl, "curl") {
        // запасной вариант через PowerShell
        log(i18n::s(
            "curl недоступен, пробуем PowerShell…",
            "curl is unavailable, trying PowerShell…",
        )
        .to_string());
        let ps = format!(
            "$ProgressPreference='SilentlyContinue'; Invoke-WebRequest -Uri '{}' -OutFile '{}' -UseBasicParsing",
            ZIP_URL,
            zip.display()
        );
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps]);
        run_checked(cmd, "PowerShell")?;
        let _ = e;
    }

    log(i18n::s("Распаковка…", "Unpacking…").to_string());
    let staging = bin.join("_staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let ps = format!(
        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
        zip.display(),
        staging.display()
    );
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps]);
    run_checked(cmd, "unzip")?;

    // Внутри архива один каталог — разворачиваем его содержимое в bin/.
    let mut source = staging.clone();
    if let Ok(rd) = fs::read_dir(&staging) {
        let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        if entries.len() == 1 && entries[0].is_dir() {
            source = entries[0].clone();
        }
    }
    copy_dir_contents(&source, &bin)?;
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&zip);
    // Демонстрационные файлы из архива в bin/ не нужны.
    for junk in ["input.jpg", "input2.jpg", "onepiece_demo.mp4", "README_windows.md"] {
        let _ = fs::remove_file(bin.join(junk));
    }

    if !installed() {
        bail!(i18n::s(
            "Не удалось установить AI-движок: файлы не найдены после распаковки",
            "Could not install the AI engine: no files found after unpacking",
        ));
    }
    log(i18n::s("AI-движок установлен.", "AI engine installed.").to_string());
    Ok(())
}

fn copy_dir_contents(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_dir_contents(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}
