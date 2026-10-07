//! Winxer — системный аудиопроцессор с хостингом VST3.

mod audio;
mod vst;
mod vst3support;

use once_cell::sync::Lazy;
use std::sync::Mutex;
use vst::PluginInfo;

pub static ENGINE: Lazy<Mutex<audio::Engine>> = Lazy::new(|| Mutex::new(audio::Engine::new()));

#[tauri::command]
fn scan_plugins() -> Vec<PluginInfo> {
    vst::scan_all()
}

#[tauri::command]
fn list_custom_dirs() -> Vec<String> {
    vst::list_custom_dirs()
}

#[tauri::command]
fn add_custom_dir(dir: String) -> Result<Vec<String>, String> {
    vst::add_custom_dir(&dir)
}

#[tauri::command]
fn remove_custom_dir(dir: String) -> Result<Vec<String>, String> {
    vst::remove_custom_dir(&dir)
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

/// Пересборка графа: стоп старого потока, старт нового.
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

/// Открывает редактор VST3-плагина.
#[tauri::command]
fn open_editor(path: String) -> Result<(), String> {
    let id = crate::vst3support::stable_id(&path);
    vst3support::open_editor(id, &path)
}

#[tauri::command]
fn close_editor(id: u64) -> bool {
    vst3support::close_editor(id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            scan_plugins,
            list_custom_dirs,
            add_custom_dir,
            remove_custom_dir,
            list_devices,
            engine_start,
            engine_rebuild,
            engine_stop,
            open_editor,
            close_editor
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
