//! Winxer Bridge — 32-битный процесс-помощник, хостящий x86 VST2-плагины.
//!
//! Протокол (TCP localhost, порт = 45555 + n):
//!   --- "HELLO\n"                     --> проверка живости
//!   <-- "OK\n"
//!   --- "LOAD <base64 path>\n"        --> загрузка плагина
//!   <-- "OK <id>\n" | "ERR <text>\n"
//!   --- "PROC <id> <n> <pcm f32 le>"  --> обработка n сэмплов стерео
//!   <-- "OK <pcm f32 le>"
//!   --- "EDIT <id>\n"                 --> открыть редактор плагина
//!   <-- "OK\n"
//!   --- "DROP <id>\n"                 --> выгрузить плагин
//!   <-- "OK\n"

#[cfg(target_arch = "x86")]
mod imp {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use vst::buffer::AudioBuffer;
    use vst::editor::Editor;
    use vst::host::{Host, HostBuffer, PluginInstance, PluginLoader};
    use vst::plugin::Plugin;

    struct BridgeHost;
    impl Host for BridgeHost {
        fn automate(&self, _i: i32, _v: f32) {}
        fn get_info(&self) -> (isize, String, String) {
            (2400, "Winxer".into(), "Winxer Bridge".into())
        }
        fn update_display(&self) {}
    }

    struct Loaded {
        inst: Arc<Mutex<PluginInstance>>,
        io: (usize, usize),
        path: String,
    }

    /// id плагина -> hwnd открытого окна редактора (LMMS-модель: прячем, не закрываем).
    static EDITORS: Mutex<Option<HashMap<usize, isize>>> = Mutex::new(None);

    fn editors_get(id: usize) -> Option<isize> {
        EDITORS
            .lock()
            .ok()
            .and_then(|g| g.as_ref().and_then(|m| m.get(&id).copied()))
    }
    fn editors_put(id: usize, hwnd: isize) {
        if let Ok(mut g) = EDITORS.lock() {
            g.get_or_insert_with(HashMap::new).insert(id, hwnd);
        }
    }

    fn main_inner(port: u16) -> std::io::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        eprintln!("[winxer-bridge] слушаю 127.0.0.1:{port}");
        let mut loaded: Vec<Option<Loaded>> = Vec::new();

        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            eprintln!("[winxer-bridge] клиент подключился");
            loop {
                let mut line = Vec::new();
                let mut b = [0u8; 1];
                loop {
                    match s.read(&mut b) {
                        Ok(0) => break,
                        Ok(_) if b[0] == b'\n' => break,
                        Ok(_) => line.push(b[0]),
                        Err(_) => break,
                    }
                }
                if line.is_empty() {
                    break;
                }
                let cmd = String::from_utf8_lossy(&line).into_owned();
                let parts: Vec<&str> = cmd.splitn(3, ' ').collect();
                match parts[0] {
                    "HELLO" => {
                        let _ = s.write_all(b"OK\n");
                    }
                    "LOAD" => {
                        let raw = base64_decode(parts.get(1).unwrap_or(&""));
                        let path = String::from_utf8_lossy(&raw).into_owned();
                        // Уже загружен этим путём? Тот же id: повторная загрузка
                        // рвала связь редактор↔инстанс (крутилки крутили старую
                        // копию, звук шёл через новую с дефолтами).
                        if let Some(id) = loaded.iter().position(|l| {
                            l.as_ref()
                                .map_or(false, |l| l.path.eq_ignore_ascii_case(&path))
                        }) {
                            let _ = s.write_all(format!("OK {id}\n").as_bytes());
                            eprintln!("[winxer-bridge] reuse #{id}: {path}");
                            continue;
                        }
                        match load_plugin(&path) {
                            Ok((inst, io)) => {
                                let id = loaded.len();
                                loaded.push(Some(Loaded {
                                    inst: Arc::new(Mutex::new(inst)),
                                    io,
                                    path: path.clone(),
                                }));
                                let _ = s.write_all(format!("OK {id}\n").as_bytes());
                                eprintln!("[winxer-bridge] loaded #{id}: {path}");
                            }
                            Err(e) => {
                                let _ = s.write_all(format!("ERR {e}\n").as_bytes());
                            }
                        }
                    }
                    "PROC" => {
                        let id: usize = parts.get(1).unwrap_or(&"0").parse().unwrap_or(0);
                        let n: usize = parts.get(2).unwrap_or(&"0").parse().unwrap_or(0);
                        let want = n * 2 * 4;
                        let mut audio = vec![0u8; want];
                        if n > 0 && s.read_exact(&mut audio).is_err() {
                            break;
                        }
                        let inst = loaded
                            .get(id)
                            .and_then(|l| l.as_ref().map(|l| Arc::clone(&l.inst)));
                        let io = loaded.get(id).and_then(|l| l.as_ref().map(|l| l.io));
                        let out = match (inst, io) {
                            (Some(inst), Some(io)) => process_one(inst, io, n, &audio),
                            _ => audio.to_vec(),
                        };
                        let _ = s.write_all(b"OK ");
                        let _ = s.write_all(&out);
                    }
                    "EDIT" => {
                        let id: usize = parts.get(1).unwrap_or(&"0").parse().unwrap_or(0);
                        match open_editor(&loaded, id) {
                            Ok(()) => {
                                let _ = s.write_all(b"OK\n");
                            }
                            Err(e) => {
                                let _ = s.write_all(format!("ERR {e}\n").as_bytes());
                            }
                        }
                    }
                    "DROP" => {
                        let id: usize = parts.get(1).unwrap_or(&"0").parse().unwrap_or(0);
                        if let Some(slot) = loaded.get_mut(id) {
                            *slot = None;
                        }
                        let _ = s.write_all(b"OK\n");
                    }
                    _ => {
                        let _ = s.write_all(b"ERR unknown\n");
                    }
                }
            }
        }
        Ok(())
    }

    fn load_plugin(path: &str) -> Result<(PluginInstance, (usize, usize)), String> {
        let host = Arc::new(Mutex::new(BridgeHost));
        let mut loader =
            PluginLoader::load(std::path::Path::new(path), host).map_err(|e| format!("{e:?}"))?;
        let mut inst = loader.instance().map_err(|e| format!("{e:?}"))?;
        inst.init();
        inst.set_sample_rate(48000.0);
        inst.set_block_size(512);
        inst.resume();
        let info = inst.get_info();
        Ok((
            inst,
            (info.inputs.max(1) as usize, info.outputs.max(1) as usize),
        ))
    }

    fn process_one(
        inst: Arc<Mutex<PluginInstance>>,
        io: (usize, usize),
        n: usize,
        audio: &[u8],
    ) -> Vec<u8> {
        // try_lock: пока редактор открывается (держит лок), звук идёт транзитом.
        let mut inst = match inst.try_lock() {
            Ok(i) => i,
            Err(std::sync::TryLockError::Poisoned(pe)) => pe.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return audio.to_vec(),
        };
        let floats: Vec<f32> = audio
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let (need_in, need_out) = io;
        let mut chans_in: Vec<Vec<f32>> = (0..need_in).map(|_| vec![0f32; n]).collect();
        let mut chans_out: Vec<Vec<f32>> = (0..need_out).map(|_| vec![0f32; n]).collect();
        for ch in 0..need_in {
            for i in 0..n {
                let src = if ch % 2 == 0 {
                    floats[i * 2]
                } else {
                    floats[i * 2 + 1]
                };
                chans_in[ch][i] = src;
            }
        }
        let mut host_buf = HostBuffer::<f32>::new(need_in, need_out);
        {
            let inputs: Vec<&[f32]> = (0..need_in).map(|ch| &chans_in[ch][..]).collect();
            let mut outputs: Vec<&mut [f32]> = Vec::with_capacity(need_out);
            for ch in 0..need_out {
                outputs
                    .push(unsafe { std::slice::from_raw_parts_mut(chans_out[ch].as_mut_ptr(), n) });
            }
            let mut buf: AudioBuffer<f32> = host_buf.bind(&inputs, &mut outputs);
            inst.process(&mut buf);
        }
        let mut out = Vec::with_capacity(n * 2 * 4);
        for i in 0..n {
            out.extend_from_slice(&chans_out[0][i].to_le_bytes());
            let r = need_out.saturating_sub(1).min(1);
            out.extend_from_slice(&chans_out[r][i].to_le_bytes());
        }
        out
    }

    // -----------------------------------------------------------------------
    // Редакторы: окно Win32 в отдельном потоке, LMMS-модель (прячем, не закрываем).
    // -----------------------------------------------------------------------

    fn open_editor(loaded: &[Option<Loaded>], id: usize) -> Result<(), String> {
        // Уже открыто — показываем спрятанное окно.
        if let Some(hwnd) = editors_get(id) {
            unsafe {
                let _ = win::ShowWindow(win::HWND(hwnd as *mut _), win::SW_SHOW);
                let _ = win::SetForegroundWindow(win::HWND(hwnd as *mut _));
            }
            return Ok(());
        }
        let Some(Some(l)) = loaded.get(id) else {
            return Err("плагин не загружен".into());
        };
        let inst = Arc::clone(&l.inst);
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name(format!("bridge-editor-{id}"))
            .spawn(move || editor_thread(id, inst, tx))
            .map_err(|e| e.to_string())?;
        // Ждём создания окна/ошибки, но не дольше 30 сек (тяжёлые JUCE-открытия).
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|e| e.to_string())?
    }

    fn editor_thread(
        id: usize,
        inst: Arc<Mutex<PluginInstance>>,
        reply: Sender<Result<(), String>>,
    ) {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            editor_thread_inner(id, inst, reply.clone())
        }));
        if res.is_err() {
            let _ = reply.send(Err("паника в потоке редактора".into()));
        }
    }

    fn editor_thread_inner(
        id: usize,
        inst: Arc<Mutex<PluginInstance>>,
        reply: Sender<Result<(), String>>,
    ) {
        unsafe {
            let _ = windows::Win32::System::Ole::OleInitialize(None);
            win::register_class();

            // try_lock-цикл: аудио берёт лок коротко и часто, ждём щедро.
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            let mut editor = loop {
                match inst.try_lock() {
                    Ok(mut g) => match g.get_editor() {
                        Some(e) => break e,
                        None => {
                            let _ = reply.send(Err("у плагина нет редактора".into()));
                            return;
                        }
                    },
                    Err(std::sync::TryLockError::Poisoned(pe)) => {
                        let mut g = pe.into_inner();
                        match g.get_editor() {
                            Some(e) => break e,
                            None => {
                                let _ = reply.send(Err("у плагина нет редактора".into()));
                                return;
                            }
                        }
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        if Instant::now() > deadline {
                            let _ = reply.send(Err("не дождались лока".into()));
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
            };

            let (w0, h0) = editor.size();
            let (w, h) = (if w0 > 0 { w0 } else { 400 }, if h0 > 0 { h0 } else { 300 });
            let title = format!("Winxer Bridge #{id}\0");
            let hwnd = match win::create_window(w, h, &title) {
                Ok(h) => h,
                Err(e) => {
                    let _ = reply.send(Err(format!("окно: {e}")));
                    return;
                }
            };
            let t0 = Instant::now();
            let opened = editor.open(hwnd as *mut std::os::raw::c_void);
            eprintln!(
                "[winxer-bridge] editor #{id}: open() = {opened} за {:?}",
                t0.elapsed()
            );
            if !opened {
                win::destroy_window(hwnd);
                let _ = reply.send(Err("плагин не смог открыть редактор".into()));
                return;
            }
            let (w1, h1) = editor.size();
            if w1 > 0 && h1 > 0 && (w1 != w || h1 != h) {
                win::resize_frame(hwnd, w1, h1);
            }
            editors_put(id, hwnd);
            let _ = reply.send(Ok(()));

            // Привязываем инстанс к idle-таймеру окна (effEditIdle каждые 20 мс).
            win::set_idle_instance(Arc::clone(&inst));
            let _ = win::SetTimer(Some(win::HWND(hwnd as *mut _)), id, 20, None);

            // Держим editor Box живым до конца потока (в wndproc idle идёт
            // через инстанс).
            std::mem::forget(editor);

            let mut msg = win::MSG::default();
            while win::GetMessageA(&mut msg, None, 0, 0).as_bool() {
                let _ = win::TranslateMessage(&msg);
                win::DispatchMessageA(&msg);
            }
            eprintln!("[winxer-bridge] editor #{id}: поток завершён");
        }
    }

    // --- Win32 минимальный слой ------------------------------------------------

    mod win {
        use std::sync::{Arc, Mutex};

        use vst::host::PluginInstance;
        use windows::core::PCSTR;
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            AdjustWindowRect, CreateWindowExA, DefWindowProcA, DestroyWindow, RegisterClassA,
            SetWindowPos, CS_HREDRAW, CS_VREDRAW, SWP_NOZORDER, WINDOW_EX_STYLE, WM_CLOSE,
            WM_DESTROY, WM_TIMER, WNDCLASSA, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        pub use windows::Win32::Foundation::HWND;
        pub use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageA, GetMessageA, SetForegroundWindow, SetTimer, ShowWindow,
            TranslateMessage, MSG, SW_HIDE, SW_SHOW,
        };

        pub const WM_USER_IDLE: u32 = 0x0402;

        static INST_FOR_IDLE: Mutex<Option<Arc<Mutex<PluginInstance>>>> = Mutex::new(None);

        pub fn set_idle_instance(inst: Arc<Mutex<PluginInstance>>) {
            if let Ok(mut g) = INST_FOR_IDLE.lock() {
                *g = Some(inst);
            }
        }

        const CLASS: &[u8] = b"WinxerBridgeHost\0";

        pub unsafe fn register_class() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                let wc = WNDCLASSA {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    lpszClassName: PCSTR::from_raw(CLASS.as_ptr()),
                    ..Default::default()
                };
                RegisterClassA(&wc);
            });
        }

        unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            if msg == WM_TIMER {
                if let Ok(g) = INST_FOR_IDLE.lock() {
                    if let Some(inst) = g.as_ref() {
                        if let Ok(mut i) = inst.try_lock() {
                            i.edit_idle(); // vendored-патч: настоящий effEditIdle
                        }
                    }
                }
                return LRESULT(0);
            }
            match msg {
                WM_CLOSE => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    return LRESULT(0);
                }
                WM_DESTROY => {
                    windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                }
                _ => return DefWindowProcA(hwnd, msg, w, l),
            }
            LRESULT::default()
        }

        pub unsafe fn create_window(w: i32, h: i32, title: &str) -> Result<isize, String> {
            register_class();
            let mut rect = windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let _ = AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false);
            let hwnd = CreateWindowExA(
                WINDOW_EX_STYLE::default(),
                PCSTR::from_raw(CLASS.as_ptr()),
                PCSTR::from_raw(title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN,
                120,
                120,
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

        pub unsafe fn resize_frame(hwnd: isize, w: i32, h: i32) {
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

        pub unsafe fn destroy_window(hwnd: isize) {
            let _ = DestroyWindow(HWND(hwnd as *mut _));
        }
    }

    fn base64_decode(s: &str) -> Vec<u8> {
        const TBL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        for c in s.bytes() {
            if let Some(pos) = TBL.iter().position(|&t| t == c) {
                acc = (acc << 6) | pos as u32;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((acc >> bits) as u8);
                }
            }
        }
        out
    }

    pub fn run() {
        let port: u16 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(45556);
        if let Err(e) = main_inner(port) {
            eprintln!("[winxer-bridge] ошибка: {e}");
        }
    }
}

#[cfg(not(target_arch = "x86"))]
mod imp {
    pub fn run() {
        eprintln!("winxer-bridge собран не для x86 — 32-битные плагины недоступны");
    }
}

fn main() {
    imp::run();
}
