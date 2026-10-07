//! Живой хостинг VST2-плагинов: загрузка DLL, инстансы, окна редакторов.
//!
//! Архитектура: КАЖДЫЙ редактор живёт в СВОЁМ потоке со своим message loop
//! (создание окна и effEditOpen обязаны идти в одном потоке). Если один
//! плагин зависает в open(), остальные окна продолжают жить — и наоборот.

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
    // Drop инстанса зовёт effClose сам.
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
// Окна редакторов: один плагин = один поток = один message loop.
// ---------------------------------------------------------------------------

use vst::editor::Editor;

/// Живая сессия редактора: окно + поток, который его обслуживает.
pub struct EditorSession {
    pub plugin_id: u64,
    pub hwnd: isize,
    /// Держим плагин (инстанс для idle) и Box редактора живыми до unload.
    pub plugin: Arc<LoadedPlugin>,
}

pub static EDITORS: Lazy<Mutex<HashMap<u64, Arc<EditorSession>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Открытые редакторы: Box живёт в потоке своего окна; при остановке потока
/// (окна убиты) сюда кладём None. Ключ — id плагина.
static EDITOR_BOXES: Lazy<Mutex<HashMap<u64, Option<WrappedEditor>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub struct WrappedEditor(pub Box<dyn Editor>);
unsafe impl Send for WrappedEditor {}

impl WrappedEditor {}

const WM_USER_SHOW: u32 = 0x0401;

/// Открывает редактор плагина. Неблокирующе: поток создаётся и всё делает сам.
pub fn open_editor_window(id: u64, path: &str, title: &str) -> Result<(), String> {
    if let Some(sess) = EDITORS.lock().map_err(|e| e.to_string())?.get(&id).cloned() {
        // Уже открыто: посылаем сообщение потоку окна — он сам покажет себя.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(HWND(sess.hwnd as *mut _)),
                WM_USER_SHOW,
                WPARAM(0),
                LPARAM(0),
            );
        }
        log(&format!(
            "editor id={id}: запрошен показ существующего окна"
        ));
        return Ok(());
    }

    let path = path.to_string();
    let title = title.to_string();
    std::thread::Builder::new()
        .name(format!("winxer-editor-{id}"))
        .spawn(move || editor_thread(id, &path, &title))
        .map_err(|e| format!("поток редактора: {e}"))?;
    Ok(())
}

/// Поток одного окна редактора: load → окно → effEditOpen → GetMessage loop.
fn editor_thread(id: u64, path: &str, title: &str) {
    log(&format!("editor id={id}: поток запущен"));
    unsafe {
        // JUCE-плагины требуют OLE на UI-потоке.
        let _ = windows::Win32::System::Ole::OleInitialize(None);
        register_class();

        // Загрузка плагина и редактора — в ЭТОМ потоке.
        let p = match load(id, path) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("editor id={id}: {e}"));
                return;
            }
        };
        let mut editor = {
            let Ok(mut inst) = p.instance.lock() else {
                return;
            };
            match inst.get_editor() {
                Some(e) => e,
                None => {
                    // Крейт помнит редактор «активным» после прошлой сессии —
                    // сбрасываем и пробуем снова (окно той сессии давно убито).
                    inst.reset_editor_flag();
                    match inst.get_editor() {
                        Some(e) => e,
                        None => {
                            log(&format!("editor id={id}: у плагина нет редактора"));
                            return;
                        }
                    }
                }
            }
        };

        // rect до open: почти все плагины знают размер заранее.
        let (w0, h0) = editor.size();
        let (w, h) = (if w0 > 0 { w0 } else { 400 }, if h0 > 0 { h0 } else { 300 });
        log(&format!("editor id={id}: rect до open = {w}x{h}"));

        let hwnd = match create_plugin_window(w, h, title) {
            Ok(h) => h,
            Err(e) => {
                log(&format!("editor id={id}: окно: {e}"));
                return;
            }
        };

        let t0 = std::time::Instant::now();
        let opened = editor.open(hwnd as *mut std::os::raw::c_void);
        log(&format!(
            "editor id={id}: open() = {opened} за {:?}",
            t0.elapsed()
        ));
        if !opened {
            destroy_window(hwnd);
            return;
        }

        // Финальный rect после open.
        let (w1, h1) = editor.size();
        if w1 > 0 && h1 > 0 && (w1 != w || h1 != h) {
            resize_frame(hwnd, w1, h1);
        }

        // idle-таймер этого редактора: effEditIdle каждые 20 мс.
        let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(
            Some(HWND(hwnd as *mut _)),
            (id as usize) as usize,
            20,
            None,
        );

        // Регистрируем сессию и Box (Box принадлежит этому потоку).
        EDITORS.lock().unwrap().insert(
            id,
            Arc::new(EditorSession {
                plugin_id: id,
                hwnd,
                plugin: Arc::clone(&p),
            }),
        );
        EDITOR_BOXES
            .lock()
            .unwrap()
            .insert(id, Some(WrappedEditor(editor)));
        log(&format!(
            "editor id={id}: сессия сохранена, качаем сообщения"
        ));

        let mut msg = MSG::default();
        while GetMessageA(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_USER_SHOW {
                let _ = ShowWindow(HWND(hwnd as *mut _), SW_SHOW);
                let _ = SetForegroundWindow(HWND(hwnd as *mut _));
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageA(&msg);
        }

        // Поток завершён (окно уничтожено): закрываем редактор и убираем сессии.
        if let Some(Some(w)) = EDITOR_BOXES.lock().unwrap().get_mut(&id) {
            w.0.close();
        }
        EDITOR_BOXES.lock().unwrap().remove(&id);
        EDITORS.lock().unwrap().remove(&id);
        log(&format!("editor id={id}: поток завершён, редактор закрыт"));
    }
}

// Хелпер вынесен ниже; макросы не нужны.

// --- Публичный API ---------------------------------------------------------

pub fn close_editor(id: u64) -> bool {
    // Прячем окно: редактор живёт, повторный open мгновенный.
    if let Some(sess) = EDITORS.lock().ok().and_then(|m| m.get(&id).cloned()) {
        unsafe { ShowWindow(HWND(sess.hwnd as *mut _), SW_HIDE) };
        log(&format!("editor id={id}: окно спрятано (редактор жив)"));
        true
    } else {
        false
    }
}

// --- Win32 -----------------------------------------------------------------

use windows::core::PCSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExA, DefWindowProcA, DestroyWindow, DispatchMessageA,
    GetMessageA, RegisterClassA, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE,
    WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSA, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

const WINXER_CLASS: &[u8] = b"WinxerEditorHost\0";

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
    });
}

unsafe extern "system" fn editor_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            // effEditIdle для редактора ЭТОГО окна: ищем сессию по hwnd.
            let plugin = {
                let Ok(eds) = EDITORS.lock() else {
                    return LRESULT(0);
                };
                let Some(sess) = eds.values().find(|s| s.hwnd == hwnd.0 as isize) else {
                    return LRESULT(0);
                };
                Arc::clone(&sess.plugin)
            };
            if let Ok(mut inst) = plugin.instance.try_lock() {
                inst.edit_idle();
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            // Крестик: прячем окно, НЕ уничтожаем (LMMS-модель).
            let _ = ShowWindow(hwnd, SW_HIDE);
            return LRESULT(0);
        }
        WM_DESTROY => {
            // Сюда попадаем только при явном уничтожении (выгрузка плагина).
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
        }
        _ => return DefWindowProcA(hwnd, msg, w, l),
    }
    LRESULT::default()
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
