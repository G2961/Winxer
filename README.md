# Winxer

**[English](README.md)** | **[Русский](README.ru.md)**

A Metro-style (Windows 8) system-wide audio processor for Windows: equalize and shape any system sound through a chain of VST plugins. Run your music, games or browser through your own plugins – like a mastering rack for the entire system.

## Features

- **System audio capture** – WASAPI loopback: the system plays into a virtual cable (VB-Cable), Winxer picks the sound up, runs it through the plugin chain and outputs to your real speakers or headphones
- **VST2 x64** – native hosting, plugin editors in separate windows
- **VST3** – supported via the vst3-host crate
- **32-bit plugins** – a separate bridge process `winxer-bridge.exe` (x86): plugins that physically cannot load into a 64-bit app work through it
- **Settings persist** – plugin instances live as long as the app is open; reopening an editor is instant
- **Custom scan folders** – plugins do not have to sit in Program Files; the search is recursive, up to 4 levels deep
- **Chain presets** – save and load whole plugin racks
- **Metro UI** – tiles, pivot tabs, live feel: tiles bend under the cursor, shrink smoothly at the edges, resize on right-click

## How it works

```
system → CABLE Input → Winxer (VST chain) → your headphones/speakers
```

Winxer holds an audio graph: it captures the loopback of the selected device, processes 512-sample blocks through the plugin chain (VST2 / VST3 / x86 bridge) and writes to the output device. While the app is running and "start" is on – the sound goes through the chain.

## Development setup

Requirements: Rust (stable, MSVC), Node.js, the `i686-pc-windows-msvc` target for the bridge.

```bash
npm install
rustup target add i686-pc-windows-msvc

# 32-bit bridge (for x86 plugins)
cd src-tauri
cargo build --target i686-pc-windows-msvc --bin winxer-bridge
# copy winxer-bridge.exe from target/i686-pc-windows-msvc/debug/ next to winxer.exe

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
- `src-tauri/src/vsthost.rs` – VST2 x64 hosting: editor windows, idle, loading
- `src-tauri/src/vst3support.rs` – VST3 via the vst3-host crate
- `src-tauri/src/bridge.rs` + `src/bin/winxer-bridge.rs` – the 32-bit bridge (TCP localhost)
- `src-tauri/vendor/vst/` – the vst crate with a patch: a real effEditIdle (the original idle is empty – without the patch JUCE plugins never repaint their UI)

## Known limitations

- The VST3 host query set is minimal – rare plugins may fail to open
- Editors of 32-bit plugins do not sync presets back to the main process (sound and knobs work)
- Latency of ~20–40 ms is normal for the loopback-through-virtual-cable scheme

## License

Personal use. Dependencies: vst (MIT), vst3-host (MIT), wasapi (Apache-2.0).
