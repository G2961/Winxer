//! Клиент моста: связь с 32-битным winxer-bridge.exe для x86-плагинов.
//!
//! Мост запускается как дочерний процесс (лежит рядом с winxer.exe),
//! общение — TCP localhost. Аудиоблоки гоняются синхронно в цикле движка:
//! на 512 сэмплов это ~20 байт заголовка + 4КБ данных — микросекунды на loopback.

use once_cell::sync::Lazy;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;

use crate::vsthost::log;

pub struct Bridge {
    stream: TcpStream,
    /// id плагина в мосте (по порядку загрузки).
    pub plugin_id: usize,
}

pub static BRIDGE: Lazy<Mutex<Option<Bridge>>> = Lazy::new(|| Mutex::new(None));

/// путь плагина → id в мосте (заполняется при загрузке цепочки/редактора).
static PATH_TO_ID: Lazy<Mutex<std::collections::HashMap<String, usize>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

pub fn remember_id(path: &str, id: usize) {
    if let Ok(mut m) = PATH_TO_ID.lock() {
        m.insert(path.to_string(), id);
    }
}

pub fn bridge_id_for(path: &str) -> Option<usize> {
    PATH_TO_ID.lock().ok().and_then(|m| m.get(path).copied())
}

fn base64_encode(data: &[u8]) -> String {
    const TBL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in data {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 6 {
            bits -= 6;
            out.push(TBL[((acc >> bits) & 0x3f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(TBL[((acc << (6 - bits)) & 0x3f) as usize] as char);
    }
    out
}

/// Находит exe моста: рядом с текущим бинарём.
fn bridge_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("winxer-bridge.exe");
    if candidate.exists() {
        Some(candidate)
    } else {
        // dev-режим: target-папка общая, мост лежит рядом в debug/.
        None
    }
}

/// Запускает мост (если ещё не запущен) и подключается.
fn ensure_bridge() -> Result<(), String> {
    if BRIDGE.lock().map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    let exe = bridge_exe().ok_or("winxer-bridge.exe не найден рядом с программой")?;
    let port: u16 = 45556;
    let child = std::process::Command::new(exe)
        .arg(port.to_string())
        .spawn()
        .map_err(|e| format!("запуск моста: {e}"))?;
    std::mem::forget(child); // живёт сам; закроем с приложением
                             // Подключаемся с ретраями: мост поднимается миллисекунды.
    let mut last = String::new();
    for _ in 0..50 {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut s) => {
                let _ = s.write_all(b"HELLO\n");
                let mut ans = [0u8; 16];
                let n = s.read(&mut ans).unwrap_or(0);
                if ans[..n].starts_with(b"OK") {
                    *BRIDGE.lock().map_err(|e| e.to_string())? = Some(Bridge {
                        stream: s,
                        plugin_id: 0,
                    });
                    log("bridge: подключён");
                    return Ok(());
                }
                last = "нет OK".into();
            }
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!("bridge не поднялся: {last}"))
}

/// Загружает x86-плагин в мост. Возвращает bridge-id.
pub fn load(path: &str) -> Result<usize, String> {
    ensure_bridge()?;
    let mut guard = BRIDGE.lock().map_err(|e| e.to_string())?;
    let Some(b) = guard.as_mut() else {
        return Err("bridge мёртв".into());
    };
    let msg = format!("LOAD {}\n", base64_encode(path.as_bytes()));
    b.stream
        .write_all(msg.as_bytes())
        .map_err(|e| e.to_string())?;
    let line = read_line(&mut b.stream)?;
    if let Some(rest) = line.strip_prefix("OK ") {
        Ok(rest.trim().parse().map_err(|e| format!("bad id: {e}"))?)
    } else {
        Err(line)
    }
}

fn read_line(s: &mut TcpStream) -> Result<String, String> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match s.read(&mut b) {
            Ok(0) => return Err("bridge закрыл соединение".into()),
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => line.push(b[0]),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Обрабатывает блок: interleaved stereo f32 in/out.
pub fn process(id: usize, l: &mut [f32], r: &mut [f32]) -> bool {
    let Ok(mut guard) = BRIDGE.lock() else {
        return false;
    };
    let Some(b) = guard.as_mut() else {
        return false;
    };
    let n = l.len();
    // interleaved
    let mut pcm = Vec::with_capacity(n * 2 * 4);
    for i in 0..n {
        pcm.extend_from_slice(&l[i].to_le_bytes());
        pcm.extend_from_slice(&r[i].to_le_bytes());
    }
    let msg = format!("PROC {id} {n}\n");
    if b.stream.write_all(msg.as_bytes()).is_err() {
        return false;
    }
    if b.stream.write_all(&pcm).is_err() {
        return false;
    }
    // Ответ: "OK " + n*2*4 байт.
    let mut hdr = [0u8; 3];
    if b.stream.read_exact(&mut hdr).is_err() {
        return false;
    }
    if &hdr != b"OK " {
        return false;
    }
    let mut buf = vec![0u8; n * 2 * 4];
    if b.stream.read_exact(&mut buf).is_err() {
        return false;
    }
    for i in 0..n {
        l[i] = f32::from_le_bytes([buf[i * 8], buf[i * 8 + 1], buf[i * 8 + 2], buf[i * 8 + 3]]);
        r[i] = f32::from_le_bytes([
            buf[i * 8 + 4],
            buf[i * 8 + 5],
            buf[i * 8 + 6],
            buf[i * 8 + 7],
        ]);
    }
    true
}

/// Просит мост открыть редактор плагина.
pub fn open_editor(id: usize) -> Result<(), String> {
    let mut guard = BRIDGE.lock().map_err(|e| e.to_string())?;
    let Some(b) = guard.as_mut() else {
        return Err("bridge мёртв".into());
    };
    b.stream
        .write_all(format!("EDIT {id}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let line = read_line(&mut b.stream)?;
    if line.starts_with("OK") {
        Ok(())
    } else {
        Err(line)
    }
}
