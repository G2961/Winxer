//! Winxer — системный аудиопроцессор с хостингом VST2/VST3.

mod audio;
mod vst;
mod vsthost;

use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use vst::PluginInfo;

pub static ENGINE: Lazy<Mutex<audio::Engine>> = Lazy::new(|| Mutex::new(audio::Engine::new()));

#[tauri::command]
fn scan_plugins() -> Vec<PluginInfo> {
    vst::scan_all()
}

/// Реальный список устройств вывода (render) через WASAPI — имена всегда есть.
#[tauri::command]
fn list_devices() -> Vec<String> {
    audio::list_render_devices()
}

#[tauri::command]
fn engine_start(device: String, out_device: String, chain: Vec<String>) -> Result<(), String> {
    ENGINE
        .lock()
        .map_err(|e| e.to_string())?
        .start(device, out_device, chain)
}

/// Пересборка графа: стоп старого потока, выгрузка исчезнувших плагинов, старт нового.
#[tauri::command]
fn engine_rebuild(device: String, out_device: String, chain: Vec<String>) -> Result<(), String> {
    ENGINE
        .lock()
        .map_err(|e| e.to_string())?
        .rebuild(device, out_device, chain)
}

#[tauri::command]
fn engine_stop() {
    if let Ok(mut eng) = ENGINE.lock() {
        eng.stop();
    }
}

#[tauri::command]
fn plugin_meta(id: u64, path: String) -> Result<serde_json::Value, String> {
    let m = vsthost::meta(id)?;
    Ok(serde_json::json!({
        "name": m.name, "vendor": m.vendor,
        "params": m.params, "presets": m.presets
    }))
}

/// Открывает редактор плагина. Повторный вызов — фокус/ничего не делает.
/// id считается здесь из пути — фронт его не передаёт (JS number ненадёжен для u64).
#[tauri::command]
fn open_editor(app: AppHandle, path: String, title: String) -> Result<(), String> {
    let id = vsthost::stable_id(&path);
    let label = format!("editor-{id}");

    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }

    // Реальный нативный редактор (VST2) в отдельном Win32-окне.
    let native = vsthost::open_editor_window(id, &path, &title);

    match native {
        Ok(()) => Ok(()),
        Err(native_err) => {
            // Fallback: веб-окно с метаданными плагина.
            WebviewWindowBuilder::new(
                &app,
                &label,
                WebviewUrl::App(PathBuf::from(format!("editor.html#id={id}"))),
            )
            .title(format!("{title} — Winxer"))
            .inner_size(440.0, 620.0)
            .min_inner_size(320.0, 400.0)
            .build()
            .map_err(|e| e.to_string())?;
            let _ = native_err;
            Ok(())
        }
    }
}

#[tauri::command]
fn close_editor(id: u64) -> bool {
    vsthost::close_editor(id)
}

#[tauri::command]
fn close_window(app: AppHandle, label: String) {
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            scan_plugins,
            list_devices,
            engine_start,
            engine_rebuild,
            engine_stop,
            open_editor,
            close_editor,
            close_window,
            plugin_meta
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
