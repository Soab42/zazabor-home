//! Zazabor TV for Android: the desktop's TV section (`src-tauri/src/tv`) on its own.
//!
//! The catalog, crawler, poster cache and Live TV code are compiled in from the desktop tree by
//! path, so both apps share one implementation. Playing is not done here: the page hands the
//! video link to VLC (`MainActivity.kt`, the `ZazaborPlayer` bridge).

#[path = "../../../src-tauri/src/tv/mod.rs"]
mod tv;

/// `%XX` escapes of a query value.
fn unescape(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `cs://localhost/file?p=<path>` (on Android `http://cs.localhost/…`), the desktop's scheme for
/// cached pictures: serves a poster or logo, and only from inside the app's cache folder.
fn serve_picture(cache: Option<std::path::PathBuf>, uri: &tauri::http::Uri) -> tauri::http::Response<Vec<u8>> {
    let reply = |status: u16, type_: &str, body: Vec<u8>| tauri::http::Response::builder().status(status).header("Content-Type", type_).header("Access-Control-Allow-Origin", "*").header("Cache-Control", "max-age=31536000").body(body).unwrap_or_default();
    let wanted = uri.query().unwrap_or("").split('&').find_map(|kv| kv.strip_prefix("p=")).map(unescape);
    let (Some(cache), Some(wanted)) = (cache.and_then(|c| c.canonicalize().ok()), wanted) else { return reply(404, "text/plain", Vec::new()) };
    let Ok(path) = std::path::Path::new(&wanted).canonicalize() else { return reply(404, "text/plain", Vec::new()) };
    if !path.starts_with(&cache) {
        return reply(403, "text/plain", Vec::new());
    }
    let type_ = match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    };
    match std::fs::read(&path) {
        Ok(bytes) => reply(200, type_, bytes),
        Err(_) => reply(404, "text/plain", Vec::new()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .register_uri_scheme_protocol("cs", |ctx, request| serve_picture(ctx.app_handle().path().app_cache_dir().ok(), request.uri()))
        .setup(|app| {
            // posters and logos are cache (the system may clear them); the catalog is data
            tv::init(app.handle().clone(), app.path().app_data_dir()?, app.path().app_cache_dir()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // The desktop registers the same list in `src-tauri/src/lib.rs`.
            tv::tv_discover,
            tv::tv_state,
            tv::tv_start,
            tv::tv_home,
            tv::tv_items,
            tv::tv_resolve,
            tv::tv_episodes,
            tv::tv_played,
            tv::tv_cache_info,
            tv::tv_clear_cache,
            tv::tv_live_refresh,
            tv::tv_live_channels,
            tv::tv_live_fav,
            tv::tv_live_watched,
            tv::tv_live_logo,
            tv::tv_live_probe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Zazabor TV");
}
