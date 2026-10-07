# Winxer

**[English](README.md)** | **[Русский](README.ru.md)**

A Metro-style (Windows 8) system-wide audio processor for Windows: equalize and shape any system sound through a chain of VST3 plugins. Run your music, games or browser through your own plugins – like a mastering rack for the entire system.

No virtual cables needed: install the app, press start – done.

## Features

- **System audio capture** – Windows process loopback: Winxer grabs the system mix directly, excluding its own output, so there is no feedback loop and no VB-Cable setup
- **VST3 hosting** – native editors in separate windows, settings persist while the app is open
- **Custom scan folders** – plugins do not have to sit in Program Files; the search is recursive, up to 4 levels deep
- **Chain presets** – save and load whole plugin racks
- **Metro UI** – tiles, pivot tabs, live feel: tiles bend under the cursor, shrink smoothly at the edges, resize on right-click

## How it works

```
system mix (default device) → Winxer (VST3 chain) → your headphones/speakers
```

Winxer holds an audio graph: it captures the system sound of the default output device via process loopback (its own rendered audio is excluded from the capture, so the chain output can play on the same device), processes 512-sample blocks through the plugin chain and writes to the output device. While the app is running and "start" is on – the sound goes through the chain.

## Development setup

Requirements: Rust (stable, MSVC), Node.js.

```bash
npm install

# run
npm run tauri dev
```

## Usage

1. Pick the device to filter on the main tile (capture always follows the Windows default output device)
2. Build a chain on the "plugins" tab – clicking a tile adds it to the chain
3. Press "start" – the sound now goes through the plugins
4. "Window" on a selected plugin opens its editor; right-click a tile to resize it

## Architecture

- `src/` – frontend (Tauri WebView): Metro tiles, chain, presets
- `src-tauri/src/audio.rs` – audio engine: process loopback → chain → render
- `src-tauri/src/vst3support.rs` – VST3 hosting via the vst3-host crate: loading, processing, editor windows; all plugin control-plane runs on a persistent STA host thread
- `src-tauri/src/vst.rs` – plugin discovery: standard and custom folders
- `src-tauri/src/bin/winxer-selftest.rs` – headless plugin lifecycle test (load, DSP, parameter turn, editor open/close/reopen)

## Known limitations

- Capture always follows the Windows **default** output device – to filter a specific device, make it the default in Windows
- Synth/instrument plugins (SWAM, Serum and similar) generate their own sound instead of passing the input through – they silence the chain and are not useful for system audio processing
- The VST3 host query set is minimal – rare plugins may fail to open
- Latency of ~20–40 ms is normal for the loopback scheme

## License

Personal use. Dependencies: vst3-host (MIT), wasapi (Apache-2.0).
