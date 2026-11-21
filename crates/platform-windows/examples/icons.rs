//! Extracts a real icon into an ICO cache file and validates it.
//!
//! Run with:
//!
//! ```text
//! cargo run -p ansa-platform-windows --example icons --offline
//! ```
//!
//! The example uses `%SystemRoot%\System32\notepad.exe` and skips gracefully
//! when that file is absent. It prints the actual cache path, dimensions and
//! byte size so the result can be checked on a real machine.

use std::path::PathBuf;

fn notepad() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let path = root.join("System32").join("notepad.exe");
    path.is_file().then_some(path)
}

fn main() {
    let Some(notepad) = notepad() else {
        println!("icon_example=skipped reason=\"%SystemRoot%\\System32\\notepad.exe is absent\"");
        return;
    };

    let cache_dir = std::env::temp_dir().join(format!(
        "ansa-platform-icons-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let source = format!("{},0", notepad.display());

    match ansa_platform_windows::extract_icon_to_ico(&source, &cache_dir) {
        Ok(entry) => {
            let bytes = std::fs::read(&entry.cache_path).expect("read ICO cache file");
            match ansa_platform_windows::validate_ico(&bytes) {
                Ok(()) => {
                    println!(
                        "icon_source={source:?} cache={:?} size={} bytes dimensions={}x{} valid_ico=true",
                        entry.cache_path, entry.bytes, entry.width, entry.height
                    );
                }
                Err(error) => {
                    println!(
                        "icon_source={source:?} cache={:?} valid_ico=false error={error:?}",
                        entry.cache_path
                    );
                    std::process::exit(1);
                }
            }
            let _ = std::fs::remove_dir_all(&cache_dir);
        }
        Err(error) => {
            println!("icon_source={source:?} extraction_failed={error:?}");
            let _ = std::fs::remove_dir_all(&cache_dir);
            std::process::exit(1);
        }
    }
}
