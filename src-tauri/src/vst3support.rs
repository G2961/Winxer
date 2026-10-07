//! Хостинг VST3-плагинов через крейт vst3-host.
//!
//! Архитектура потоков (критично для Win32/JUCE-плагинов):
//! один постоянный HOST-поток, в котором создаются все объекты плагинов,
//! открываются/закрываются редакторы и который вечно качает Win32-сообщения.
//! JUCE-плагины (Cramit, Omnisphere) привязывают свой MessageManager к потоку
//! первого вызова и ждут от него насоса: если тот поток умер или не качает
//! сообщения, следующий COM-вызов виснет намертво. Отдельный поток под каждое
//! окно поэтому не подходит – после закрытия окна поток умирает, и повторное
//! открытие дедлокится. Аудио-`process()` остаётся в аудиопотоке (норма VST3).

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use vst3_host::{AudioBuffers, Plugin, PluginWindow, Vst3Host};

use std::io::Write;

/// Лог в файл + stderr.
pub fn log(msg: &str) {
    eprintln!("[winxer] {msg}");
    let path = std::path::Path::new("C:/Users/G2961/.winxer/editor.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            f,
            "[{}] {msg}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
    }
}

/// Стабильный id по пути плагина: движок и редактор обращаются к одной загрузке.
pub fn stable_id(path: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Загруженный VST3-плагин: инстанс под мьютексом (крейт требует &mut).
pub struct LoadedVst3 {
    pub plugin: Arc<Mutex<Plugin>>,
}

pub static LOADED_VST3: Lazy<Mutex<HashMap<u64, Arc<LoadedVst3>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Хост один на процесс.
///
/// Изоляция (helper-процесс) выключена сознательно: на Windows крейт vst3-host
/// не умеет открывать GUI плагина через границу процессов (см. README крейта,
/// раздел Caveats), плюс таймаут ответов helper-а блокировал аудиопоток.
pub static VST3_HOST: Lazy<Mutex<Vst3Host>> = Lazy::new(|| {
    match Vst3Host::builder()
        .sample_rate(48_000.0)
        .block_size(512)
        .with_process_isolation(false)
        .build()
    {
        Ok(h) => Mutex::new(h),
        Err(e) => {
            log(&format!("vst3 host не создан: {e}"));
            panic!("VST3 host init failed: {e}");
        }
    }
});

// ---------------------------------------------------------------------------
// HOST-поток: апартамент для всего control-plane (load/editor/params) + насос.
// ---------------------------------------------------------------------------

enum HostCmd {
    Load {
        id: u64,
        path: String,
        ack: std::sync::mpsc::Sender<Result<Arc<LoadedVst3>, String>>,
    },
    OpenEditor {
        id: u64,
        path: String,
    },
    CloseEditor {
        id: u64,
    },
    SetParam {
        id: u64,
        param: u32,
        value: f64,
    },
}

static HOST_TX: Lazy<Mutex<Option<std::sync::mpsc::Sender<HostCmd>>>> =
    Lazy::new(|| Mutex::new(None));

fn host_thread() -> Option<std::sync::mpsc::Sender<HostCmd>> {
    if let Some(tx) = HOST_TX.lock().ok().and_then(|t| t.clone()) {
        return Some(tx);
    }
    let (tx, rx) = std::sync::mpsc::channel::<HostCmd>();
    std::thread::Builder::new()
        .name("winxer-vst3-host".into())
        .spawn(move || host_loop(rx))
        .ok()?;
    *HOST_TX.lock().unwrap() = Some(tx.clone());
    Some(tx)
}

fn panic_msg(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "неизвестная паника".into())
}

/// Главный цикл host-потока: команды + насос Win32-сообщений + уход окон.
fn host_loop(rx: std::sync::mpsc::Receiver<HostCmd>) {
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    log("vst3 host-поток запущен");

    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                HostCmd::Load { id, path, ack } => {
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        load_in_host(id, &path)
                    }))
                    .unwrap_or_else(|e| Err(format!("паника загрузки: {}", panic_msg(e))));
                    let _ = ack.send(res);
                }
                HostCmd::OpenEditor { id, path } => open_editor_in_host(id, &path),
                HostCmd::CloseEditor { id } => close_editor_in_host(id),
                HostCmd::SetParam { id, param, value } => set_param_in_host(id, param, value),
            }
        }

        // Насос сообщений: COM-маршализация и окна редакторов живут здесь.
        #[cfg(windows)]
        pump_messages();

        // Пользователь мог закрыть окно крестиком – прибираем сессию.
        if let Ok(mut editors) = VST3_EDITORS.lock() {
            let closed: Vec<u64> = editors
                .iter()
                .filter(|(_, s)| s.window.lock().map(|w| w.closed_by_user()).unwrap_or(false))
                .map(|(id, _)| *id)
                .collect();
            for id in closed {
                if let Some(sess) = editors.remove(&id) {
                    if let Ok(mut w) = sess.window.lock() {
                        w.close();
                    }
                    log(&format!("vst3 editor id={id}: окно закрыто пользователем"));
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

/// Перекачка очереди Win32-сообщений текущего потока.
#[cfg(windows)]
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Загрузка плагина – только в host-потоке.
fn load_in_host(id: u64, path: &str) -> Result<Arc<LoadedVst3>, String> {
    if let Some(p) = LOADED_VST3.lock().ok().and_then(|m| m.get(&id).cloned()) {
        return Ok(p);
    }
    log(&format!("vst3 load: {path}"));

    let mut host = VST3_HOST.lock().map_err(|e| e.to_string())?;
    let mut plugin = host
        .load_plugin(Path::new(path))
        .map_err(|e| format!("vst3 загрузка: {e}"))?;
    drop(host);
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
    // Включаем обработку: без этого process_audio молча возвращает NotProcessing,
    // и звук идёт сквозь цепочку без изменений.
    plugin
        .start_processing()
        .map_err(|e| format!("vst3 start_processing: {e}"))?;

    let p = Arc::new(LoadedVst3 {
        plugin: Arc::new(Mutex::new(plugin)),
    });
    LOADED_VST3.lock().unwrap().insert(id, Arc::clone(&p));
    Ok(p)
}

/// Публичная загрузка: маршализуется в host-поток.
pub fn load(id: u64, path: &str) -> Result<Arc<LoadedVst3>, String> {
    let Some(tx) = host_thread() else {
        return Err("host-поток не запущен".into());
    };
    let (ack_tx, ack_rx) = std::sync::mpsc::channel();
    tx.send(HostCmd::Load {
        id,
        path: path.to_string(),
        ack: ack_tx,
    })
    .map_err(|_| "host-поток мёртв".to_string())?;
    match ack_rx.recv_timeout(std::time::Duration::from_secs(120)) {
        Ok(res) => res,
        Err(_) => Err("host-поток не ответил за 120 с".into()),
    }
}

/// Изменение параметра из UI (value 0..1) – через host-поток.
pub fn set_param(id: u64, param: u32, value: f64) {
    if let Some(tx) = host_thread() {
        let _ = tx.send(HostCmd::SetParam { id, param, value });
    }
}

fn set_param_in_host(id: u64, param: u32, value: f64) {
    if let Some(p) = LOADED_VST3.lock().ok().and_then(|m| m.get(&id).cloned()) {
        if let Ok(mut plugin) = p.plugin.lock() {
            if let Err(e) = plugin.set_parameter(param, value) {
                log(&format!("vst3 id={id}: set_param({param}) = {value}: {e}"));
            }
        }
    }
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

// ---------------------------------------------------------------------------
// Процессинг (аудиопоток).
// ---------------------------------------------------------------------------

/// Идентификаторы плагинов в ошибочном состоянии (для одноразового лога).
static ERRORED: Lazy<Mutex<std::collections::HashSet<u64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

fn plugin_error(id: u64, msg: &str) {
    if let Ok(mut set) = ERRORED.lock() {
        if set.insert(id) {
            log(&format!(
                "vst3 id={id}: {msg} (дальше – тихо до восстановления)"
            ));
        }
    }
}

fn plugin_ok(id: u64) {
    if let Ok(mut set) = ERRORED.lock() {
        set.remove(&id);
    }
}

/// Обработка блока: l/r — каналы (in-place). Выполняется в аудиопотоке.
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
    if let Err(e) = plugin.process_audio(&mut buffers) {
        plugin_error(id, &format!("process: {e}"));
        return false;
    }
    plugin_ok(id);
    l.copy_from_slice(&buffers.outputs[0]);
    r.copy_from_slice(&buffers.outputs[1]);
    true
}

// ---------------------------------------------------------------------------
// Окна редакторов (создаются и живут в host-потоке).
// ---------------------------------------------------------------------------

pub struct Vst3EditorSession {
    pub window: Arc<Mutex<PluginWindow>>,
}

unsafe impl Send for Vst3EditorSession {}
unsafe impl Sync for Vst3EditorSession {}

pub static VST3_EDITORS: Lazy<Mutex<HashMap<u64, Arc<Vst3EditorSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Открывает редактор (неблокирующе): команда уходит в host-поток.
pub fn open_editor(id: u64, path: &str) -> Result<(), String> {
    {
        let mut editors = VST3_EDITORS.lock().map_err(|e| e.to_string())?;
        let mut opening = OPENING.lock().unwrap();
        if editors.contains_key(&id) || opening.contains(&id) {
            log(&format!("vst3 editor id={id}: уже открыт"));
            return Ok(());
        }
        opening.insert(id);
    }
    let Some(tx) = host_thread() else {
        OPENING.lock().unwrap().remove(&id);
        return Err("host-поток не запущен".into());
    };
    if tx
        .send(HostCmd::OpenEditor {
            id,
            path: path.to_string(),
        })
        .is_err()
    {
        OPENING.lock().unwrap().remove(&id);
        return Err("host-поток мёртв".into());
    }
    Ok(())
}

/// Идентификаторы редакторов в процессе открытия (двойной клик не должен
/// открыть два окна, а карта сессий заполняется только после успеха).
static OPENING: Lazy<Mutex<std::collections::HashSet<u64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

/// Выполняется в host-потоке.
fn open_editor_in_host(id: u64, path: &str) {
    log(&format!("vst3 editor id={id}: открытие в host-потоке"));
    let p = match load_in_host(id, path) {
        Ok(p) => p,
        Err(e) => {
            log(&format!("vst3 editor id={id}: {e}"));
            OPENING.lock().unwrap().remove(&id);
            return;
        }
    };

    let mut win = PluginWindow::new(Arc::clone(&p.plugin));
    if let Err(e) = win.open() {
        log(&format!("vst3 editor id={id}: open: {e}"));
        OPENING.lock().unwrap().remove(&id);
        return;
    }
    let session = Arc::new(Vst3EditorSession {
        window: Arc::new(Mutex::new(win)),
    });
    VST3_EDITORS.lock().unwrap().insert(id, session);
    OPENING.lock().unwrap().remove(&id);
    log(&format!("vst3 editor id={id}: окно открыто"));
}

/// Закрывает редактор по команде UI – через host-поток.
pub fn close_editor(id: u64) -> bool {
    let present = VST3_EDITORS
        .lock()
        .map(|m| m.contains_key(&id))
        .unwrap_or(false);
    if !present {
        return false;
    }
    match host_thread() {
        Some(tx) => tx.send(HostCmd::CloseEditor { id }).is_ok(),
        None => false,
    }
}

fn close_editor_in_host(id: u64) {
    OPENING.lock().unwrap().remove(&id);
    if let Some(sess) = VST3_EDITORS.lock().ok().and_then(|mut m| m.remove(&id)) {
        if let Ok(mut w) = sess.window.lock() {
            w.close();
        }
        log(&format!("vst3 editor id={id}: закрыт командой"));
    }
}
