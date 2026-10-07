//! Сканирование установленных VST2/VST3 плагинов и открытие их редакторов.

use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct PluginInfo {
    /// Имя, показываемое в UI (пока — имя файла без расширения).
    pub name: String,
    pub vendor: String,
    /// "VST2" | "VST3"
    pub format: String,
    pub path: String,
    /// "x64" | "x86" | "?" — 32-битные DLL не грузятся в 64-битный Winxer.
    pub arch: String,
}

/// Разрядность PE-файла по заголовку: 0x14c = x86, 0x8664 = x64.
fn pe_machine(path: &Path) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return "?".into();
    };
    let mut dos = [0u8; 64];
    if f.read_exact(&mut dos).is_err() {
        return "?".into();
    }
    let off = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;
    use std::io::Seek;
    if f.seek(std::io::SeekFrom::Start(off + 4)).is_err() {
        return "?".into();
    }
    let mut m = [0u8; 2];
    if f.read_exact(&mut m).is_err() {
        return "?".into();
    }
    match u16::from_le_bytes(m) {
        0x14c => "x86".into(),
        0x8664 => "x64".into(),
        _ => "?".into(),
    }
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
            v.push(PathBuf::from(
                r"C:\Program Files (x86)\Steinberg\VstPlugins",
            ));
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                v.push(PathBuf::from(l).join("Programs\\VSTPlugins"));
            }
        }
        _ => {}
    }
    v
}

/// Плагины *.vst3 — это папки-бандлы; *.dll — одиночные файлы.
/// Поиск рекурсивный: у многих плагины лежат по подпапкам (F:\Plugins\EQ\...").
fn collect(dir: &Path, format: &str, out: &mut Vec<PluginInfo>, depth: usize) {
    if depth > 4 {
        return; // защита от бесконечной рекурсии и длинных обходов
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let is_vst3 = format == "VST3"
            && p.extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("vst3"))
            && p.is_dir();
        let is_dll = p
            .extension()
            .map_or(false, |e| e.eq_ignore_ascii_case("dll"));
        if is_vst3 || (is_dll && p.is_file()) {
            // В FL Studio / verify plugins имя показывается как имя файла без расширения.
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Для .vst3-бандла разрядность — по папке Contents\<arch>-win.
            let arch = if is_vst3 {
                let mut found = "?".to_string();
                if let Ok(rd) = fs::read_dir(p.join("Contents")) {
                    for sub in rd.flatten() {
                        let d = sub.file_name().to_string_lossy().into_owned();
                        if d.contains("x86_64") || (d.contains('x') && d.contains("64")) {
                            found = "x64".into();
                        } else if d.contains("x86") || d.contains("i386") || d.contains("win32") {
                            found = "x86".into();
                        }
                    }
                }
                found
            } else {
                pe_machine(&p)
            };
            out.push(PluginInfo {
                name,
                vendor: String::new(),
                format: format.to_string(),
                path: p.to_string_lossy().into_owned(),
                arch,
            });
        } else if p.is_dir() {
            // Внутри .vst3-бандла не ищем (Contents\... — не плагины).
            if !is_vst3 {
                collect(&p, format, out, depth + 1);
            }
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
    for fmt in ["VST3", "VST2"] {
        for dir in base_dirs(fmt) {
            collect(&dir, fmt, &mut out, 0);
        }
    }
    // Пользовательские папки: формат определяем по содержимому (и .vst3, и .dll).
    for dir in custom_dirs() {
        collect(&dir, "VST3", &mut out, 0);
        collect(&dir, "VST2", &mut out, 0);
    }
    // Дедуп по пути.
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.path.to_lowercase()));
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
