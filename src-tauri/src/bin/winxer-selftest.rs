//! Самотест хостинга VST3 без UI: воспроизводит жизненный цикл плагина,
//! который гоняет пользователь в Winxer (загрузка, обработка, окно редактора,
//! закрытие, переоткрытие), плюс программное кручение параметра и замер RMS.
//!
//! Запуск: cargo run --bin winxer-selftest -- [путь.vst3]

use winxer_lib::vst3support;

fn log(msg: &str) {
    println!("[selftest] {msg}");
}

fn rms(v: &[f32]) -> f64 {
    let s: f64 = v.iter().map(|x| (*x as f64) * (*x as f64)).sum();
    (s / v.len().max(1) as f64).sqrt()
}

fn main() {
    // STA COM для UI-потока редактора, как в winxer.
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "C:/Program Files/Common Files/VST3/Cramit.vst3".into());
    log(&format!("плагин: {path}"));

    // --- 1. загрузка и обработка до окна ---
    let id = vst3support::stable_id(&path);
    let p = match vst3support::load(id, &path) {
        Ok(p) => p,
        Err(e) => {
            log(&format!("FAIL загрузка: {e}"));
            std::process::exit(1);
        }
    };
    log("ok: загрузка");

    let test_process = |tag: &str| -> bool {
        let n = 512;
        let mut l: Vec<f32> = (0..n).map(|i| (i as f32 / 16.0).sin() * 0.5).collect();
        let mut r = l.clone();
        let before = rms(&l);
        let ok = vst3support::process(id, &mut l, &mut r);
        let after = rms(&l);
        log(&format!(
            "{tag}: process={ok} rms {before:.4} -> {after:.4}"
        ));
        ok
    };

    // ЭКСПЕРИМЕНТ: не валимся на process=false – идём дальше к окну.
    test_process("до окна");

    // --- 2. кручение параметра gain (программно, как крутилкой) ---
    {
        let plugin = p.plugin.lock().unwrap();
        let params = plugin.get_parameters().unwrap_or_default();
        log(&format!("ok: параметров: {}", params.len()));
        let mut shown = 0;
        for par in &params {
            if shown >= 12 {
                log("  ...");
                break;
            }
            log(&format!(
                "  param #{:x} {:?} = {:.3}",
                par.id, par.name, par.value
            ));
            shown += 1;
        }
        let gain = params
            .iter()
            .find(|par| par.name.to_lowercase().contains("gain"))
            .or_else(|| {
                params
                    .iter()
                    .find(|par| par.name.to_lowercase().contains("output"))
            });
        drop(plugin);
        if let Some(g) = gain {
            vst3support::set_param(id, g.id, 0.9);
            // даём команде дойти до host-потока
            std::thread::sleep(std::time::Duration::from_millis(300));
            log(&format!("ok: gain #{:x} покручен до 0.9", g.id));
            test_process("после кручения gain");
        } else {
            log("нет параметра gain/output – пропускаю кручение");
        }
    }

    // --- 3. окно редактора: открыть, подержать, закрыть, переоткрыть ---
    let open_and_wait = |tag: &str, ms: u64| -> bool {
        if let Err(e) = vst3support::open_editor(id, &path) {
            log(&format!("FAIL open_editor ({tag}): {e}"));
            return false;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            let opened = vst3support::VST3_EDITORS
                .lock()
                .map(|m| m.contains_key(&id))
                .unwrap_or(false);
            if opened {
                log(&format!("ok: окно открыто ({tag})"));
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        log(&format!("FAIL: окно не открылось за {ms}мс ({tag})"));
        false
    };

    if !open_and_wait("1-й раз", 10_000) {
        std::process::exit(1);
    }
    std::thread::sleep(std::time::Duration::from_millis(1_500));

    // process() во время открытого окна
    if !test_process("окно открыто") {
        log("FAIL: process=false при открытом окне (mutex удерживается?)");
        std::process::exit(1);
    }

    // Закрытие командой (как close_editor из UI).
    if !vst3support::close_editor(id) {
        log("FAIL: close_editor вернул false");
        std::process::exit(1);
    }
    log("ok: окно закрыто командой");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // process() сразу после закрытия окна: если редактор удерживает mutex –
    // здесь будет тишина в обработке (баг, который ловим).
    if !test_process("после закрытия") {
        log("FAIL: process=false после закрытия окна – mutex отравлен/удержан");
        std::process::exit(1);
    }

    // Переоткрытие: то, что не получалось у пользователя.
    if !open_and_wait("2-й раз (переоткрытие)", 10_000) {
        log("FAIL: переоткрытие не сработало");
        std::process::exit(1);
    }
    std::thread::sleep(std::time::Duration::from_millis(1_000));
    if !test_process("после переоткрытия") {
        log("FAIL: process=false после переоткрытия");
        std::process::exit(1);
    }
    let _ = vst3support::close_editor(id);
    log("ok: финальное закрытие");

    // --- 4. держим процесс живым чуть-чуть, чтобы UI-потоки дорисовали ---
    std::thread::sleep(std::time::Duration::from_millis(300));
    log("ВСЁ ПРОШЛО");
}
