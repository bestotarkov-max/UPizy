//! Поиск ffmpeg/ffprobe, кэш списков кодеков и фильтров, запуск процессов.

use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::config;
use crate::i18n;

fn resolve(name: &str) -> String {
    let local = config::bin_dir().join(format!("{name}.exe"));
    if local.exists() {
        return local.to_string_lossy().into_owned();
    }
    let local_nix = config::bin_dir().join(name);
    if local_nix.exists() {
        return local_nix.to_string_lossy().into_owned();
    }
    if config::tool_available(name) {
        return name.to_string();
    }
    name.to_string()
}

static FFMPEG: OnceLock<String> = OnceLock::new();
static FFPROBE: OnceLock<String> = OnceLock::new();

pub fn ffmpeg() -> &'static str {
    FFMPEG.get_or_init(|| resolve("ffmpeg"))
}

pub fn ffprobe() -> &'static str {
    FFPROBE.get_or_init(|| resolve("ffprobe"))
}

pub fn ffmpeg_available() -> bool {
    config::tool_available(ffmpeg())
}

pub fn ffmpeg_version() -> String {
    let mut cmd = Command::new(ffmpeg());
    cmd.arg("-version");
    hide_window(&mut cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    match cmd.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .next()
                .map(|l| l.trim().to_string())
                .unwrap_or_else(|| "?".into())
        }
        Err(_) => i18n::s("не найден", "not found").into(),
    }
}

/// Прячем консольные окна дочерних процессов в GUI-режиме.
pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

fn graph_names(extra_args: &[&str], marker: char) -> HashSet<String> {
    let mut cmd = Command::new(ffmpeg());
    cmd.args(["-hide_banner"]).args(extra_args);
    hide_window(&mut cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(out) = cmd.output() else {
        return HashSet::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut names = HashSet::new();
    for line in text.lines() {
        // Формат строк: " V->V       cas   VV->V  ..." / " V..... libx265  H.265 ..."
        let mut parts = line.split_whitespace();
        let Some(flags) = parts.next() else { continue };
        if !flags.starts_with(marker) || flags.len() < 2 {
            continue;
        }
        if let Some(name) = parts.next() {
            if !name.contains("->") && !name.starts_with('-') {
                names.insert(name.to_string());
            }
        }
        // Для фильтров имя идёт третьим токеном
        if marker == '.' {
            let mut it = line.split_whitespace();
            let _ = it.next();
            let _ = it.next();
            if let Some(name) = it.next() {
                if name.contains("->") {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

pub fn has_filter(name: &str) -> bool {
    static FILTERS: OnceLock<HashSet<String>> = OnceLock::new();
    FILTERS
        .get_or_init(|| graph_names(&["-filters"], '.'))
        .contains(name)
}

pub fn available_encoders() -> &'static HashSet<String> {
    static ENCODERS: OnceLock<HashSet<String>> = OnceLock::new();
    ENCODERS.get_or_init(|| graph_names(&["-encoders"], 'V'))
}
