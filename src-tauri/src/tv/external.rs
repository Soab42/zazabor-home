//! Opening a TV title in VLC, when the PC has it: "Open in VLC" on a title's page shows only then.

use std::path::PathBuf;

use super::Result;

fn vlc_path() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["vlc.exe"] } else { &["vlc"] };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).flat_map(|dir| names.iter().map(move |n| dir.join(n))).find(|p| p.is_file())
}

#[tauri::command]
pub fn tv_vlc_available() -> bool {
    vlc_path().is_some()
}

/// Starts VLC on a video link (or a downloaded file's path).
#[tauri::command]
pub fn tv_open_vlc(url: String) -> Result<()> {
    if !(url.starts_with("http://") || url.starts_with("https://") || std::path::Path::new(&url).is_absolute()) {
        return Err("That isn't a video link".into());
    }
    let vlc = vlc_path().ok_or("VLC isn't installed")?;
    std::process::Command::new(vlc).arg("--").arg(url).spawn().map(|_| ()).map_err(|e| format!("Couldn't start VLC: {e}"))
}
