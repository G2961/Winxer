//! Хостинг VST3-плагинов через крейт vst3-host.
//!
//! Симметрично vsthost.rs (VST2): единая карта загруженных плагинов,
//! процессинг из аудиодвижка, окна редакторов в отдельных потоках.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use vst3_host::{AudioBuffers, Plugin, PluginWindow, Vst3Host};

use crate::vsthost::log;

/// Загруженный VST3-плагин: инстанс под мьютексом (крейт требует &mut).
pub struct LoadedVst3 {
    pub plugin: Arc<Mutex<Plugin>>,
}

pub static LOADED_VST3: Lazy<Mutex<HashMap<u64, Arc<LoadedVst3>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Хост один на процесс: держит загруженные модули VST3.
pub static VST3_HOST: Lazy<Mutex<Vst3Host>> = Lazy::new(|| match Vst3Host::builder().build() {
    Ok(h) => Mutex::new(h),
    Err(e) => {
        log(&format!("vst3 host не создан: {e}"));
        panic!("VST3 host init failed: {e}");
    }
});

/// Загружает VST3-плагин один раз (id = stable_id от полного пути к .vst3).
pub fn load(id: u64, path: &str) -> Result<Arc<LoadedVst3>, String> {
    if let Ok(m) = LOADED_VST3.lock() {
        if let Some(p) = m.get(&id) {
            return Ok(Arc::clone(p));
        }
    }
    let mut map = LOADED_VST3.lock().map_err(|e| e.to_string())?;
    if let Some(p) = map.get(&id) {
        return Ok(Arc::clone(p));
    }
    log(&format!("vst3 load: {path}"));

    let mut host = VST3_HOST.lock().map_err(|e| e.to_string())?;
    let mut plugin = host
        .load_plugin(Path::new(path))
        .map_err(|e| format!("vst3 загрузка: {e}"))?;
    plugin
        .reconfigure(48_000.0, 512)
        .map_err(|e| format!("vst3 reconfigure: {e}"))?;
    let _ = plugin.set_process_mode(vst3_host::ProcessMode::Realtime);
    let _ = plugin.set_bus_arrangements(
        &[vst3_host::SpeakerArrangement::STEREO],
        &[vst3_host::SpeakerArrangement::STEREO],
    );
    let _ = plugin.set_bus_active(
        vst3_host::MediaType::Audio,
        vst3_host::BusDirection::Input,
        0,
        true,
    );
    let _ = plugin.set_bus_active(
        vst3_host::MediaType::Audio,
        vst3_host::BusDirection::Output,
        0,
        true,
    );
    let _ = plugin.set_playing(true);

    let p = Arc::new(LoadedVst3 {
        plugin: Arc::new(Mutex::new(plugin)),
    });
    map.insert(id, Arc::clone(&p));
    Ok(p)
}

/// Метаданные для UI.
pub struct Vst3Meta {
    pub name: String,
    pub vendor: String,
    pub params: usize,
}

pub fn meta(id: u64) -> Result<Vst3Meta, String> {
    let p = get(id)?;
    let plugin = p.plugin.lock().map_err(|e| e.to_string())?;
    let info = plugin.info().clone();
    let params = plugin.get_parameters().map(|v| v.len()).unwrap_or(0);
    Ok(Vst3Meta {
        name: info.name,
        vendor: info.vendor,
        params,
    })
}

pub fn get(id: u64) -> Result<Arc<LoadedVst3>, String> {
    LOADED_VST3
        .lock()
        .map_err(|e| e.to_string())?
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| "vst3 плагин не загружен".into())
}

pub fn loaded_ids() -> Vec<u64> {
    LOADED_VST3
        .lock()
        .map(|m| m.keys().copied().collect())
        .unwrap_or_default()
}

/// Обработка блока: l/r — каналы (in-place).
pub fn process(id: u64, l: &mut [f32], r: &mut [f32]) -> bool {
    let Ok(p) = get(id) else { return false };
    let Ok(mut plugin) = p.plugin.try_lock() else {
        return false;
    };
    let inputs = vec![l.to_vec(), r.to_vec()];
    let outputs = vec![vec![0f32; l.len()], vec![0f32; r.len()]];
    let mut buffers = AudioBuffers {
        inputs,
        outputs,
        sample_rate: 48_000.0,
        block_size: l.len(),
    };
    if plugin.process_audio(&mut buffers).is_err() {
        return false;
    }
    l.copy_from_slice(&buffers.outputs[0]);
    r.copy_from_slice(&buffers.outputs[1]);
    true
}

// ---------------------------------------------------------------------------
// Окно редактора: PluginWindow крейта умеет Win32-хостинг и message loop.
// ---------------------------------------------------------------------------

pub struct Vst3EditorSession {
    // PluginWindow не Send (HWND внутри); окно живёт в своём потоке,
    // а арк доступен оттуда — обёртка с ручным Send.
    pub window: Arc<Mutex<PluginWindow>>,
}
unsafe impl Send for Vst3EditorSession {}
unsafe impl Sync for Vst3EditorSession {}

pub static VST3_EDITORS: Lazy<Mutex<HashMap<u64, Arc<Vst3EditorSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Открывает редактор VST3-плагина (неблокирующе, в своём потоке).
pub fn open_editor(id: u64, path: &str) -> Result<(), String> {
    if VST3_EDITORS
        .lock()
        .map_err(|e| e.to_string())?
        .contains_key(&id)
    {
        log(&format!("vst3 editor id={id}: уже открыт"));
        return Ok(());
    }
    let path = path.to_string();
    std::thread::Builder::new()
        .name(format!("winxer-vst3-editor-{id}"))
        .spawn(move || {
            let res = std::panic::catch_unwind(|| run_editor(id, &path));
            if let Err(e) = res {
                let what = e
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| e.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "неизвестная паника".into());
                log(&format!("vst3 editor id={id}: ПАНИКА: {what}"));
            }
        })
        .map_err(|e| format!("поток vst3-редактора: {e}"))?;
    Ok(())
}

fn run_editor(id: u64, path: &str) {
    log(&format!("vst3 editor id={id}: поток запущен"));
    let p = match load(id, path) {
        Ok(p) => p,
        Err(e) => {
            log(&format!("vst3 editor id={id}: {e}"));
            return;
        }
    };

    let mut win = PluginWindow::new(Arc::clone(&p.plugin));
    if let Err(e) = win.open() {
        log(&format!("vst3 editor id={id}: open: {e}"));
        return;
    }
    let session = Arc::new(Vst3EditorSession {
        window: Arc::new(Mutex::new(win)),
    });
    VST3_EDITORS.lock().unwrap().insert(id, session);
    log(&format!("vst3 editor id={id}: окно открыто"));

    // Качаем платформенные события (Win32 message pump крейта) и следим за
    // закрытием окна пользователем (крейт требует поллинга закрытия).
    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let guard = VST3_EDITORS.lock().unwrap();
        let Some(sess) = guard.get(&id).cloned() else {
            return;
        };
        drop(guard);
        let Ok(mut w) = sess.window.try_lock() else {
            continue;
        };
        if w.closed_by_user() {
            w.close();
            drop(w);
            VST3_EDITORS.lock().unwrap().remove(&id);
            log(&format!("vst3 editor id={id}: окно закрыто пользователем"));
            return;
        }
        let _ = w.service_platform_events();
    }
}
