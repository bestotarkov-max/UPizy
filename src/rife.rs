//! Интерполяция кадров RIFE (ncnn-vulkan): установка и запуск.

use anyhow::{anyhow, bail, Result};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::bin_dir;
use crate::ffmpeg::hide_window;
use crate::i18n;
use crate::tf;

pub const ZIP_URL: &str =
    "https://github.com/nihui/rife-ncnn-vulkan/releases/download/20221029/rife-ncnn-vulkan-20221029-windows.zip";
pub const EXE_NAME: &str = "rife-ncnn-vulkan.exe";

/// Оставляем только нужные модели: архив целиком весит 411 МБ.
pub const KEEP_MODELS: &[&str] = &["rife-anime", "rife-v4.6"];

pub fn exe_path() -> Option<PathBuf> {
    let p = bin_dir().join(EXE_NAME);
    p.exists().then_some(p)
}

pub fn model_dir(name: &str) -> PathBuf {
    bin_dir().join("models").join(name)
}

/// Установлен ли движок и хотя бы одна модель.
pub fn installed() -> bool {
    exe_path().is_some() && !models().is_empty()
}

/// Доступные модели (каталог с .param и .bin).
pub fn models() -> Vec<String> {
    KEEP_MODELS
        .iter()
        .filter(|name| {
            let dir = model_dir(name);
            dir.join("flownet.param").exists() || dir.join("contextnet.param").exists()
        })
        .map(|s| s.to_string())
        .collect()
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

/// Скрипт точечной распаковки: только exe и две модели, без остальных 400 МБ.
const UNZIP_PS: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = '__ZIP__'
$bin = '__BIN__'
$z = [System.IO.Compression.ZipFile]::OpenRead($zip)
$n = 0
foreach ($e in $z.Entries) {
  $nm = $e.FullName
  if ($nm -match '^[^/]+/rife-ncnn-vulkan\.exe$') {
    $dest = Join-Path $bin 'rife-ncnn-vulkan.exe'
  } elseif ($nm -match '^[^/]+/(?:models/)?(?<m>rife-anime|rife-v4\.6)/(?<f>[^/]+)$') {
    $dir = Join-Path (Join-Path $bin 'models') $Matches['m']
    $dest = Join-Path $dir $Matches['f']
  } else {
    continue
  }
  New-Item -ItemType Directory -Force -Path (Split-Path $dest) | Out-Null
  [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $dest, $true)
  $n++
}
$z.Dispose()
if ($n -lt 8) { throw "extracted only $n files" }
"#;

/// Скачивает движок и две модели интерполяции в bin/.
pub fn download(log: impl Fn(String)) -> Result<()> {
    if installed() {
        return Ok(());
    }
    let bin = bin_dir();
    fs::create_dir_all(&bin)?;
    let zip = std::env::temp_dir().join("rife-ncnn-vulkan-20221029.zip");

    log(i18n::s(
        "Скачивание RIFE для интерполяции кадров (~410 МБ, один раз)…",
        "Downloading RIFE for frame interpolation (~410 MB, one time)…",
    )
    .to_string());
    let mut curl = Command::new("curl.exe");
    curl.args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(&zip)
        .arg(ZIP_URL);
    if run_checked(curl, "curl").is_err() {
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
        run_checked(cmd, "download")?;
    }

    log(i18n::s(
        "Распаковка (движок + модели rife-anime и rife-v4.6)…",
        "Unpacking (engine + rife-anime and rife-v4.6 models)…",
    )
    .to_string());
    let script = UNZIP_PS
        .replace("__ZIP__", &zip.to_string_lossy())
        .replace("__BIN__", &bin.to_string_lossy());
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    run_checked(cmd, "unzip")?;
    let _ = fs::remove_file(&zip);

    if !installed() {
        bail!(i18n::s(
            "Не удалось установить RIFE: файлы не найдены после распаковки",
            "Could not install RIFE: no files found after unpacking",
        ));
    }
    log(i18n::s("RIFE установлен.", "RIFE installed.").to_string());
    Ok(())
}
