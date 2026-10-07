fn main() {
    tauri_build::build();

    // Копируем 32-битный мост рядом с основным бинарём, если он собран.
    // Мост собирается отдельно: cargo build --target i686-pc-windows-msvc --bin winxer-bridge
    let bridge_src = std::path::Path::new(
        "C:/Users/G2961/.winxer-target/i686-pc-windows-msvc/debug/winxer-bridge.exe",
    );
    let dst_dir = std::env::var("OUT_DIR").unwrap_or_default();
    // OUT_DIR = <target>/<profile>/build/<crate>-<hash>/out
    // Ищем <target>/<profile> поднятием до папки с "build".
    let mut dir = std::path::PathBuf::from(&dst_dir);
    for _ in 0..5 {
        if dir.file_name().map_or(false, |n| n == "build") {
            dir.pop(); // <target>/<profile>
            let dst = dir.join("winxer-bridge.exe");
            if bridge_src.exists() {
                let _ = std::fs::copy(bridge_src, &dst);
            }
            break;
        }
        dir.pop();
    }
}
