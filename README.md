# Winxer

**[English](README.md)** | **[Русский](README.ru.md)**

A Metro-style (Windows 8) system-wide audio processor for Windows: equalize and shape any system sound through a chain of VST3 plugins. Run your music, games or browser through your own plugins – like a mastering rack for the entire system.

## Features

- **System audio capture** – WASAPI loopback: the system plays into a virtual cable (VB-Cable), Winxer picks the sound up, runs it through the plugin chain and outputs to your real speakers or headphones
- **VST3 hosting** – native editors in separate windows, settings persist while the app is open
- **Custom scan folders** – plugins do not have to sit in Program Files; the search is recursive, up to 4 levels deep
- **Chain presets** – save and load whole plugin racks
- **Metro UI** – tiles, pivot tabs, live feel: tiles bend under the cursor, shrink smoothly at the edges, resize on right-click

## How it works

```
system → CABLE Input → Winxer (VST3 chain) → your headphones/speakers
```

Winxer holds an audio graph: it captures the loopback of the selected device, processes 512-sample blocks through the plugin chain and writes to the output device. While the app is running and "start" is on – the sound goes through the chain.

## Development setup

Requirements: Rust (stable, MSVC), Node.js.

```bash
npm install

# run
npm run tauri dev
```

## Usage

1. Install VB-Audio Virtual Cable and set **CABLE Input** as the Windows output device – system sound will flow into the cable
2. In Winxer pick the source (CABLE Input) and the output (your speakers) – the two must be different devices
3. Build a chain on the "plugins" tab → clicking a tile adds it to the chain
4. Press "start" – the sound now goes through the plugins
5. "Window" on a selected plugin opens its editor; right-click a tile to resize it

## Architecture

- `src/` – frontend (Tauri WebView): Metro tiles, chain, presets
- `src-tauri/src/audio.rs` – audio engine: WASAPI loopback → chain → render
- `src-tauri/src/vst3support.rs` – VST3 hosting via the vst3-host crate: loading, processing, editor windows
- `src-tauri/src/vst.rs` – plugin discovery: standard and custom folders

## Known limitations

- The VST3 host query set is minimal – rare plugins may fail to open
- Latency of ~20–40 ms is normal for the loopback-through-virtual-cable scheme

## License

Personal use. Dependencies: vst3-host (MIT), wasapi (Apache-2.0).
