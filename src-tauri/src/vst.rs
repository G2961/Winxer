//! Сканирование установленных VST3-плагинов.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct PluginInfo {
    /// Имя, показываемое в UI (имя файла без расширения, как в verify plugins).
    pub name: String,
    pub vendor: String,
    /// "VST3"
    pub format: String,
    pub path: String,
    /// "x64" | "x86" | "?"
    pub arch: String,
}

fn base_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    v.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(l).join("Programs\\Common\\VST3"));
    }
    v
}

/// .vst3-бандлы; разрядность — по папке Contents\<arch>-win.
fn collect(dir: &Path, out: &mut Vec<PluginInfo>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let is_vst3 = p
            .extension()
            .map_or(false, |e| e.eq_ignore_ascii_case("vst3"))
            && p.is_dir();
        if is_vst3 {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut arch = "?".to_string();
            if let Ok(rd) = fs::read_dir(p.join("Contents")) {
                for sub in rd.flatten() {
                    let d = sub.file_name().to_string_lossy().into_owned();
                    if d.contains("x86_64") || (d.contains('x') && d.contains("64")) {
                        arch = "x64".into();
                    } else if d.contains("x86") || d.contains("i386") || d.contains("win32") {
                        arch = "x86".into();
                    }
                }
            }
            out.push(PluginInfo {
                name,
                vendor: String::new(),
                format: "VST3".to_string(),
                path: p.to_string_lossy().into_owned(),
                arch,
            });
        } else if p.is_dir() {
            collect(&p, out, depth + 1);
        }
    }
}

/// Пользовательские папки сканирования: файл в %APPDATA%\winxer\paths.txt.
fn custom_dirs() -> Vec<PathBuf> {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return Vec::new();
    };
    let file = PathBuf::from(appdata).join("winxer\\paths.txt");
    let Ok(content) = fs::read_to_string(file) else {
        return Vec::new();
    };
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn add_custom_dir(dir: &str) -> Result<Vec<String>, String> {
    let appdata = std::env::var("APPDATA").map_err(|e| e.to_string())?;
    let base = PathBuf::from(appdata).join("winxer");
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let file = base.join("paths.txt");
    let mut lines: Vec<String> = fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !lines.iter().any(|l| l.eq_ignore_ascii_case(dir)) {
        lines.push(dir.to_string());
        fs::write(&file, lines.join("\n")).map_err(|e| e.to_string())?;
    }
    Ok(lines)
}

pub fn remove_custom_dir(dir: &str) -> Result<Vec<String>, String> {
    let appdata = std::env::var("APPDATA").map_err(|e| e.to_string())?;
    let file = PathBuf::from(appdata).join("winxer\\paths.txt");
    let mut lines: Vec<String> = fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    lines.retain(|l| !l.eq_ignore_ascii_case(dir));
    fs::write(&file, lines.join("\n")).map_err(|e| e.to_string())?;
    Ok(lines)
}

pub fn list_custom_dirs() -> Vec<String> {
    custom_dirs()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

pub fn scan_all() -> Vec<PluginInfo> {
    let mut out = Vec::new();
    for dir in base_dirs() {
        collect(&dir, &mut out, 0);
    }
    for dir in custom_dirs() {
        collect(&dir, &mut out, 0);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.path.to_lowercase()));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}
