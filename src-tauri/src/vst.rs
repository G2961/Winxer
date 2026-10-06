//! Сканирование установленных VST2/VST3 плагинов и открытие их редакторов.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct PluginInfo {
    /// Имя, показываемое в UI (пока — имя файла без расширения).
    pub name: String,
    pub vendor: String,
    /// "VST2" | "VST3"
    pub format: String,
    pub path: String,
}

fn base_dirs(format: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    match format {
        "VST3" => {
            v.push(PathBuf::from(r"C:\Program Files\Common Files\VST3"));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                v.push(PathBuf::from(l).join("Programs\\Common\\VST3"));
            }
        }
        "VST2" => {
            // Стандартные расположения, плюс значение из HKLM\SOFTWARE\VST / VSTLayout (упрощённо).
            v.push(PathBuf::from(r"C:\Program Files\VSTPlugins"));
            v.push(PathBuf::from(r"C:\Program Files\Steinberg\VstPlugins"));
            v.push(PathBuf::from(r"C:\Program Files\Common Files\VST2"));
            v.push(PathBuf::from(r"C:\Program Files (x86)\VSTPlugins"));
            v.push(PathBuf::from(r"C:\Program Files (x86)\Steinberg\VstPlugins"));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                v.push(PathBuf::from(l).join("Programs\\VSTPlugins"));
            }
        }
        _ => {}
    }
    v
}

/// Плагины *.vst3 — это папки-бандлы; *.dll — одиночные файлы.
fn collect(dir: &Path, format: &str, out: &mut Vec<PluginInfo>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let is_vst3 = format == "VST3"
            && p.extension().map_or(false, |e| e.eq_ignore_ascii_case("vst3"))
            && p.is_dir();
        let is_dll = p.extension().map_or(false, |e| e.eq_ignore_ascii_case("dll"));
        if is_vst3 || (is_dll && p.is_file()) {
            // В FL Studio / verify plugins имя показывается как имя файла без расширения.
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(PluginInfo {
                name,
                vendor: String::new(),
                format: format.to_string(),
                path: p.to_string_lossy().into_owned(),
            });
        }
    }
}

pub fn scan_all() -> Vec<PluginInfo> {
    let mut out = Vec::new();
    for fmt in ["VST3", "VST2"] {
        for dir in base_dirs(fmt) {
            collect(&dir, fmt, &mut out);
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Пока заглушка: настоящий показ редактора появится вместе с хостингом плагинов.
pub fn open_editor(path: PathBuf) -> Result<(), String> {
    if path.exists() {
        Ok(())
    } else {
        Err(format!("плагин не найден: {}", path.display()))
    }
}
