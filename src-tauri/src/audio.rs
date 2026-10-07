//! Аудиодвижок: захват системного звука выбранного устройства (WASAPI loopback),
//! прогон через цепочку VST3-плагинов, вывод на другое устройство.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

const BLOCK: usize = 512;
const CHANNELS: usize = 2;
const SR: f32 = 48_000.0;

/// Имена всех активных устройств вывода (для выбора источника и выхода).
pub fn list_render_devices() -> Vec<String> {
    let _ = wasapi::initialize_mta();
    let Ok(enumerator) = wasapi::DeviceEnumerator::new() else {
        return vec![];
    };
    let Ok(collection) = enumerator.get_device_collection(&wasapi::Direction::Render) else {
        return vec![];
    };
    let Ok(n) = collection.get_nbr_devices() else {
        return vec![];
    };
    let mut out = Vec::new();
    for i in 0..n {
        if let Ok(dev) = collection.get_device_at_index(i) {
            if let Ok(name) = dev.get_friendlyname() {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

pub struct Engine {
    pub running: bool,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            running: false,
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn start(
        &mut self,
        device: String,
        out_device: String,
        chain: Vec<String>,
    ) -> Result<(), String> {
        // Пустая цепочка допустима: чистый проход (bypass).
        if out_device == device {
            return Err("устройство вывода совпадает с источником – будет петля фидбэка. Выбери другое устройство вывода".into());
        }

        self.stop();

        let flag = Arc::new(AtomicBool::new(false));
        let dev = device.clone();
        let odev = out_device.clone();
        let f = Arc::clone(&flag);
        let ch = chain.clone();
        let handle = std::thread::Builder::new()
            .name("winxer-audio".into())
            .spawn(move || {
                // Паника аудиопотока не должна уходить в пустоту: из-за неё
                // поток умирает молча, звук пропадает без единой строки лога.
                let res = std::panic::catch_unwind(|| run_graph(&f, &dev, &odev, &ch));
                match res {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        crate::vst3support::log(&format!("аудиопоток остановлен: {e}"));
                    }
                    Err(pan) => {
                        let what = pan
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| pan.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "неизвестная паника".into());
                        crate::vst3support::log(&format!("аудиопоток ПАНИКА: {what}"));
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        self.stop_flag = flag;
        self.worker = Some(handle);
        self.running = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.worker.take() {
            // Обязательно ждём завершения: брошенный поток продолжал
            // дёргать инстансы плагинов и порождал «занятость» локов.
            let _ = h.join();
        }
        self.running = false;
    }

    /// Пересборка графа: стоп старого потока и старт нового.
    /// ПЛАГИНЫ НЕ ВЫГРУЖАЮТСЯ: инстансы живут до закрытия приложения,
    /// иначе рвётся связь редактор-инстанс.
    pub fn rebuild(
        &mut self,
        device: String,
        out_device: String,
        chain: Vec<String>,
    ) -> Result<(), String> {
        self.stop();
        self.start(device, out_device, chain)
    }
}

/// Захват loopback -> цепочка VST3 -> вывод. Отдельный поток.
fn run_graph(
    stop: &AtomicBool,
    device_name: &str,
    out_name: &str,
    chain_paths: &[String],
) -> Result<(), String> {
    wasapi::initialize_mta()
        .ok()
        .map_err(|e| format!("COM: {e}"))?;

    let enumerator = wasapi::DeviceEnumerator::new().map_err(|e| format!("enumerator: {e}"))?;
    let collection = enumerator
        .get_device_collection(&wasapi::Direction::Render)
        .map_err(|e| format!("collection: {e}"))?;
    let device = collection
        .get_device_with_name(device_name)
        .or_else(|_| enumerator.get_default_device(&wasapi::Direction::Render))
        .map_err(|e| format!("устройство «{device_name}»: {e}"))?;

    let fmt = wasapi::WaveFormat::new(32, 32, &wasapi::SampleType::Float, SR as usize, 2, None);
    let blockalign = fmt.get_blockalign() as usize;

    // 1. Loopback-клиент: ПОЛЛИНГ (WASAPI loopback не поддерживает event-driven).
    let mut cap_client = device
        .get_iaudioclient()
        .map_err(|e| format!("client: {e}"))?;
    let (def_period, _min) = cap_client
        .get_device_period()
        .map_err(|e| format!("period: {e}"))?;
    let cap_mode = wasapi::StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: def_period,
    };
    cap_client
        .initialize_client(&fmt, &wasapi::Direction::Capture, &cap_mode)
        .map_err(|e| format!("init loopback: {e}"))?;
    let capture = cap_client
        .get_audiocaptureclient()
        .map_err(|e| format!("capture: {e}"))?;

    // 2. Render-клиент на ДРУГОМ устройстве.
    let out_device = collection
        .get_device_with_name(out_name)
        .or_else(|_| enumerator.get_default_device(&wasapi::Direction::Render))
        .map_err(|e| format!("устройство вывода «{out_name}»: {e}"))?;
    let mut out_client = out_device
        .get_iaudioclient()
        .map_err(|e| format!("client2: {e}"))?;
    let out_mode = wasapi::StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: def_period,
    };
    out_client
        .initialize_client(&fmt, &wasapi::Direction::Render, &out_mode)
        .map_err(|e| format!("init render: {e}"))?;
    let out_event = out_client
        .set_get_eventhandle()
        .map_err(|e| format!("event2: {e}"))?;
    let render = out_client
        .get_audiorenderclient()
        .map_err(|e| format!("render: {e}"))?;

    // 3. Цепочка VST3.
    let mut chain_order: Vec<u64> = Vec::new();
    for path in chain_paths {
        let id = crate::vst3support::stable_id(path);
        crate::vst3support::load(id, path)?;
        chain_order.push(id);
    }

    cap_client
        .start_stream()
        .map_err(|e| format!("start capture: {e}"))?;
    out_client
        .start_stream()
        .map_err(|e| format!("start render: {e}"))?;

    let mut fifo: std::collections::VecDeque<f32> =
        std::collections::VecDeque::with_capacity(BLOCK * CHANNELS * 16);
    let mut out_l = vec![0f32; BLOCK];
    let mut out_r = vec![0f32; BLOCK];
    let mut bytebuf = vec![0u8; blockalign * BLOCK];
    let mut interleaved = vec![0f32; BLOCK * CHANNELS];

    crate::vst3support::log(&format!(
        "аудиограф запущен: «{device_name}» → «{out_name}», {} плагинов",
        chain_order.len()
    ));

    'outer: loop {
        if stop.load(Ordering::SeqCst) {
            break 'outer;
        }

        // --- захват: вычитываем всё доступное в FIFO ---
        // В polling-режиме «данных пока нет» приходит как ОШИБКА
        // (AUDCLNT_E_BUFFER_EMPTY) — это норма, не смерть графа.
        loop {
            let (frames, _info) = match capture.read_from_device(&mut bytebuf) {
                Ok(v) => v,
                Err(e) => {
                    let s = e.to_string();
                    if s.contains("88890001") || s.contains("BUFFER_EMPTY") {
                        break;
                    }
                    return Err(format!("read: {e}"));
                }
            };
            if frames == 0 {
                break;
            }
            let floats: &[f32] = unsafe {
                std::slice::from_raw_parts(
                    bytebuf.as_ptr() as *const f32,
                    frames as usize * CHANNELS,
                )
            };
            fifo.extend(floats.iter().copied());
            if frames < (BLOCK / 2) as u32 {
                break;
            }
        }

        // --- анти-дрейф: при переполнении дропаем ОДИН блок за проход ---
        let max_fifo = BLOCK * CHANNELS * 8;
        if fifo.len() > max_fifo {
            fifo.drain(0..BLOCK * CHANNELS);
        }

        // --- обработка и вывод ---
        let avail = match out_client.get_available_space_in_frames() {
            Ok(v) => v as usize,
            Err(e) => return Err(format!("space: {e}")),
        };
        let mut to_write = (fifo.len() / (BLOCK * CHANNELS)) * BLOCK;
        if to_write > avail {
            to_write = avail;
        }
        let blocks = to_write / BLOCK;
        for _ in 0..blocks {
            for i in 0..BLOCK {
                out_l[i] = fifo.pop_front().unwrap_or(0.0);
                out_r[i] = fifo.pop_front().unwrap_or(0.0);
            }
            for id in &chain_order {
                crate::vst3support::process(*id, &mut out_l, &mut out_r);
            }
            for i in 0..BLOCK {
                interleaved[i * 2] = out_l[i];
                interleaved[i * 2 + 1] = out_r[i];
            }
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(interleaved.as_ptr() as *const u8, interleaved.len() * 4)
            };
            let _ = render.write_to_device(BLOCK, bytes, None);
        }

        if out_event.wait_for_event(50).is_err() {
            break 'outer;
        }
    }

    let _ = cap_client.stop_stream();
    let _ = out_client.stop_stream();
    crate::vst3support::log("аудиограф остановлен");
    Ok(())
}
