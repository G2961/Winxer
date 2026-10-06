//! Живой хостинг VST2-плагинов: загрузка DLL, инстансы, окна редакторов.
//!
//! Модель = рабочая версия на крейте vst + vendored-патч idle_real()
//! (настоящий effEditIdle для JUCE-плагинов) + WM_TIMER-качание.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use vst::host::{Host, PluginInstance, PluginLoader};
use vst::plugin::Plugin;

/// Лог в файл + stderr.
pub fn log(msg: &str) {
    eprintln!("[winxer] {msg}");
    let path = std::path::Path::new("C:/Users/G2961/.winxer/editor.log");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
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

pub struct WinxerHost;
impl Host for WinxerHost {
    fn automate(&self, _index: i32, _value: f32) {}
    fn get_info(&self) -> (isize, String, String) {
        (2400, "Winxer".into(), "Winxer Audio".into())
    }
    fn update_display(&self) {}
}

pub struct LoadedPlugin {
    pub instance: Arc<Mutex<PluginInstance>>,
}

pub static LOADED: Lazy<Mutex<HashMap<u64, Arc<LoadedPlugin>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Сериализует LoadLibrary: защита от loader lock при гонке движок vs редактор.
static LOADING: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Загружает плагин один раз и держит в памяти (настройки не сбрасываются).
pub fn load(id: u64, path: &str) -> Result<Arc<LoadedPlugin>, String> {
    if let Ok(m) = LOADED.lock() {
        if let Some(p) = m.get(&id) {
            return Ok(Arc::clone(p));
        }
    }
    let _serial = LOADING.lock().map_err(|e| e.to_string())?;
    let mut map = LOADED.lock().map_err(|e| e.to_string())?;
    if let Some(p) = map.get(&id) {
        return Ok(Arc::clone(p));
    }
    log(&format!("load: SerialLoading {path}"));
    let host = Arc::new(Mutex::new(WinxerHost));
    let mut loader = PluginLoader::load(std::path::Path::new(path), host)
        .map_err(|e| format!("загрузка: {e:?}"))?;
    let mut inst = loader.instance().map_err(|e| format!("инстанс: {e:?}"))?;
    inst.init();
    inst.set_sample_rate(48000.0);
    inst.set_block_size(512);
    inst.resume();
    let p = Arc::new(LoadedPlugin {
        instance: Arc::new(Mutex::new(inst)),
    });
    map.insert(id, Arc::clone(&p));
    Ok(p)
}

pub fn unload(id: u64) {
    // Сначала закрываем редактор, потом drop инстанса зовёт effClose.
    if let Some(mut ed) = LEAKED_EDITORS.lock().ok().and_then(|mut m| m.remove(&id)) {
        ed.0.close();
    }
    EDITORS.lock().ok().map(|mut m| m.remove(&id));
    if let Ok(mut m) = LOADED.lock() {
        m.remove(&id);
    }
}

pub fn loaded_ids() -> Vec<u64> {
    LOADED
        .lock()
        .map(|m| m.keys().copied().collect())
        .unwrap_or_default()
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

/// Информация о плагине для UI.
pub struct PluginMeta {
    pub name: String,
    pub vendor: String,
    pub params: i32,
    pub presets: i32,
}

pub fn meta(id: u64) -> Result<PluginMeta, String> {
    let p = get(id)?;
    let inst = p.instance.lock().map_err(|e| e.to_string())?;
    let info = inst.get_info();
    Ok(PluginMeta {
        name: info.name,
        vendor: info.vendor,
        params: info.parameters,
        presets: info.presets,
    })
}

pub fn get(id: u64) -> Result<Arc<LoadedPlugin>, String> {
    LOADED
        .lock()
        .map_err(|e| e.to_string())?
        .get(&id)
        .map(Arc::clone)
        .ok_or_else(|| "плагин не загружен".into())
}

// ---------------------------------------------------------------------------
// Окна редакторов: выделенный поток (GetMessage + WM_TIMER → idle_real).
// ---------------------------------------------------------------------------

use vst::editor::Editor;

pub struct EditorSession {
    pub plugin_id: u64,
    hwnd: isize,
    // Загруженный плагин: оттуда инстанс для edit_idle().
    plugin: Arc<LoadedPlugin>,
}

pub static EDITORS: Lazy<Mutex<HashMap<u64, Arc<EditorSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Открытые редакторы плагинов: живут до выгрузки плагина (effEditOpen зовётся
/// один раз — LMMS-модель). Ключ — id плагина. Используются только потоком окон.
static LEAKED_EDITORS: Lazy<Mutex<HashMap<u64, WrappedEditor>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub struct WrappedEditor(pub Box<dyn Editor>);
unsafe impl Send for WrappedEditor {}

enum HostMsg {
    Open {
        id: u64,
        path: String,
        title: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    Close {
        id: u64,
    },
}

static TX: Lazy<Sender<HostMsg>> = Lazy::new(spawn_editor_thread);

fn spawn_editor_thread() -> Sender<HostMsg> {
    let (tx, rx) = mpsc::channel::<HostMsg>();
    std::thread::spawn(move || editor_thread(rx));
    tx
}

const WM_USER_DISPATCH: u32 = 0x0400;

static DISPATCH_RX: Mutex<Option<std::sync::Mutex<mpsc::Receiver<HostMsg>>>> = Mutex::new(None);

fn editor_thread(rx: mpsc::Receiver<HostMsg>) {
    let (wnd_tx, wnd_rx) = mpsc::channel::<isize>();
    set_wnd_channel(wnd_tx);

    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
        register_class();
        let dispatch_hwnd = create_dispatch_window();
        let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(
            Some(dispatch_hwnd),
            1000,
            20, // effEditIdle каждые 20 мс — OTT/JUCE перерисовываются живо
            None,
        );

        // Почтальон: команды канала -> PostMessage в message loop.
        let (cmd_tx, cmd_rx) = mpsc::channel::<HostMsg>();
        let dispatch_raw = dispatch_hwnd.0 as isize;
        std::thread::spawn(move || {
            let dispatch_hwnd = HWND(dispatch_raw as *mut _);
            while let Ok(m) = rx.recv() {
                if cmd_tx.send(m).is_err() {
                    break;
                }
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(dispatch_hwnd),
                    WM_USER_DISPATCH,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        *DISPATCH_RX.lock().unwrap() = Some(std::sync::Mutex::new(cmd_rx));

        let mut msg = MSG::default();
        while GetMessageA(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_USER_DISPATCH {
                let guard = DISPATCH_RX.lock().unwrap().take();
                if let Some(rxm) = guard.as_ref() {
                    let inner = rxm.lock().unwrap();
                    while let Ok(m) = inner.try_recv() {
                        match m {
                            HostMsg::Open {
                                id,
                                path,
                                title,
                                reply,
                            } => {
                                let _ = reply.send(open_in_this_thread(id, &path, &title));
                            }
                            HostMsg::Close { id } => close_in_this_thread(id),
                        }
                    }
                }
                if let Some(rxm) = guard {
                    *DISPATCH_RX.lock().unwrap() = Some(rxm);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageA(&msg);
            // Окна, закрытые крестиком: прячем (редактор живёт).
            while let Ok(hwnd) = wnd_rx.try_recv() {
                hide_by_hwnd(hwnd);
            }
        }
    }
}

fn hide_by_hwnd(hwnd: isize) {
    if let Ok(m) = EDITORS.lock() {
        if let Some((id, _)) = m.iter().find(|(_, s)| s.hwnd == hwnd) {
            let id = *id;
            drop(m);
            unsafe { ShowWindow(HWND(hwnd as *mut _), SW_HIDE) };
            log(&format!(
                "editor id={id}: окно скрыто крестиком (редактор жив)"
            ));
        }
    }
}

fn open_in_this_thread(id: u64, path: &str, title: &str) -> Result<(), String> {
    log(&format!("editor id={id}: команда получена, ищем сессию"));
    if let Some(sess) = EDITORS.lock().map_err(|e| e.to_string())?.get(&id).cloned() {
        // Уже открыто: показываем спрятанное окно, ничего не переоткрываем.
        unsafe {
            ShowWindow(HWND(sess.hwnd as *mut _), SW_SHOW);
            let _ = SetForegroundWindow(HWND(sess.hwnd as *mut _));
        }
        log(&format!(
            "editor id={id}: окно показано повторно (без переоткрытия)"
        ));
        return Ok(());
    }

    log(&format!("editor id={id}: load()"));
    let t_load = std::time::Instant::now();
    let p = load(id, path)?;
    log(&format!("editor id={id}: load() за {:?}", t_load.elapsed()));

    let t_ge = std::time::Instant::now();
    let mut editor = {
        // Блокирующий lock здесь ок: аудиопоток на try_lock и быстро отпускает.
        let mut inst = p.instance.lock().map_err(|e| e.to_string())?;
        inst.get_editor().ok_or("у плагина нет редактора")?
    };
    log(&format!(
        "editor id={id}: get_editor() за {:?}",
        t_ge.elapsed()
    ));

    unsafe {
        // rect до open: почти все плагины знают размер заранее.
        let (w0, h0) = editor.size();
        let (w, h) = (if w0 > 0 { w0 } else { 400 }, if h0 > 0 { h0 } else { 300 });
        log(&format!("editor id={id}: rect до open = {w}x{h}"));

        let hwnd = create_plugin_window(w, h, title).map_err(|e| format!("окно: {e}"))?;
        log(&format!(
            "editor id={id}: окно создано hwnd={hwnd:#x}, звали open()"
        ));
        let t0 = std::time::Instant::now();
        let opened = editor.open(hwnd as *mut std::os::raw::c_void);
        log(&format!(
            "editor id={id}: open() = {opened} за {:?}",
            t0.elapsed()
        ));
        if !opened {
            destroy_window(hwnd);
            return Err("плагин не смог открыть редактор".into());
        }

        // Финальный rect после open.
        let (w1, h1) = editor.size();
        if w1 > 0 && h1 > 0 && (w1 != w || h1 != h) {
            resize_frame(hwnd, w1, h1);
        }

        // Editor сознательно НЕ хранится в сессии: инстанс крейта помечает
        // редактор активным, а effEditIdle идёт через edit_idle() инстанса.
        // Box живёт в LEAKED_EDITORS до выгрузки плагина (LMMS-модель: окно
        // прячем, редактор не закрываем).
        LEAKED_EDITORS
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id, WrappedEditor(editor));

        EDITORS.lock().map_err(|e| e.to_string())?.insert(
            id,
            Arc::new(EditorSession {
                plugin_id: id,
                hwnd,
                plugin: Arc::clone(&p),
            }),
        );
        log(&format!("editor id={id}: сессия сохранена"));
    }
    Ok(())
}

fn close_in_this_thread(id: u64) {
    // LMMS-модель: не закрываем редактор, только прячем окно.
    if let Some(sess) = EDITORS.lock().ok().and_then(|m| m.get(&id).cloned()) {
        unsafe { ShowWindow(HWND(sess.hwnd as *mut _), SW_HIDE) };
        log(&format!("editor id={id}: окно спрятано (редактор жив)"));
    }
}

/// Настоящий effEditIdle всем открытым редакторам. Вызывается из WM_TIMER.
/// idle() у EditorInstance пропатчен шлёт effEditIdle (см. vendor/vst).
pub fn pump_idle() {
    // Собираем плагины под коротким локом EDITORS, затем шлём effEditIdle
    // через edit_idle() инстанса (try_lock: аудио-поток важнее).
    let targets: Vec<Arc<LoadedPlugin>> = {
        let Ok(eds) = EDITORS.lock() else { return };
        eds.values().map(|s| Arc::clone(&s.plugin)).collect()
    };
    for p in targets {
        if let Ok(mut inst) = p.instance.try_lock() {
            inst.edit_idle();
        }
    }
}

// --- Публичный API ---------------------------------------------------------

/// Ответчик, который пишет ошибки открытия в лог (не блокирует никого).
static LOG_REPLY: Lazy<mpsc::Sender<Result<(), String>>> = Lazy::new(|| {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(r) = rx.recv() {
            if let Err(e) = r {
                log(&format!("нативный редактор не открылся: {e}"));
            }
        }
    });
    tx
});

pub fn open_editor_window(id: u64, path: &str, title: &str) -> Result<(), String> {
    // Асинхронно: UI не ждёт открытия (тяжёлые плагины открываются секундами,
    // блокировать окно на минуту недопустимо). Ошибки идут в лог.
    TX.send(HostMsg::Open {
        id,
        path: path.into(),
        title: title.into(),
        reply: LOG_REPLY.clone(),
    })
    .map_err(|_| "поток редакторов остановлен".to_string())
}

pub fn close_editor(id: u64) -> bool {
    TX.send(HostMsg::Close { id }).is_ok()
}

// --- Win32 -----------------------------------------------------------------

use windows::core::PCSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExA, DefWindowProcA, DestroyWindow, DispatchMessageA,
    GetMessageA, RegisterClassA, SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage,
    CS_HREDRAW, CS_VREDRAW, MSG, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY,
    WM_TIMER, WNDCLASSA, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const WINXER_CLASS: &[u8] = b"WinxerEditorHost\0";
const DISPATCH_CLASS: &[u8] = b"WinxerDispatch\0";

static WND_TX: Lazy<Mutex<mpsc::Sender<isize>>> = Lazy::new(|| Mutex::new(mpsc::channel().0));

fn set_wnd_channel(tx: mpsc::Sender<isize>) {
    if let Ok(mut guard) = WND_TX.lock() {
        *guard = tx;
    }
}

unsafe fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let class_name = PCSTR::from_raw(WINXER_CLASS.as_ptr());
        let wc = WNDCLASSA {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(editor_wndproc),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassA(&wc);
        let dclass = PCSTR::from_raw(DISPATCH_CLASS.as_ptr());
        let dwc = WNDCLASSA {
            lpfnWndProc: Some(editor_wndproc),
            lpszClassName: dclass,
            ..Default::default()
        };
        RegisterClassA(&dwc);
    });
}

unsafe extern "system" fn editor_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_TIMER {
        pump_idle();
        return LRESULT(0);
    }
    match msg {
        WM_DESTROY => {
            if let Ok(tx) = WND_TX.lock() {
                let _ = tx.send(hwnd.0 as isize);
            }
        }
        _ => return DefWindowProcA(hwnd, msg, w, l),
    }
    LRESULT::default()
}

unsafe fn create_dispatch_window() -> HWND {
    CreateWindowExA(
        WINDOW_EX_STYLE::default(),
        PCSTR::from_raw(DISPATCH_CLASS.as_ptr()),
        PCSTR::from_raw(b"winxer-dispatch\0".as_ptr()),
        windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        None,
        None,
        None,
        None,
    )
    .expect("диспетчерское окно не создано")
}

unsafe fn create_plugin_window(w: i32, h: i32, title: &str) -> Result<isize, String> {
    register_class();
    let mut titlez = title.as_bytes().to_vec();
    titlez.push(0);
    let mut rect = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let _ = AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false);
    let hwnd = CreateWindowExA(
        WINDOW_EX_STYLE::default(),
        PCSTR::from_raw(WINXER_CLASS.as_ptr()),
        PCSTR::from_raw(titlez.as_ptr()),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
        100,
        100,
        rect.right - rect.left,
        rect.bottom - rect.top,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| e.to_string())?;
    Ok(hwnd.0 as isize)
}

unsafe fn resize_frame(hwnd: isize, w: i32, h: i32) {
    let mut rect = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let _ = AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false);
    let _ = SetWindowPos(
        HWND(hwnd as *mut _),
        None,
        0,
        0,
        rect.right - rect.left,
        rect.bottom - rect.top,
        SWP_NOZORDER,
    );
}

unsafe fn destroy_window(hwnd: isize) {
    let _ = DestroyWindow(HWND(hwnd as *mut _));
}
