//! The phone apps' download page (Settings → Phone apps): QR codes for the APKs attached to the
//! latest GitHub release, and their versions from the release's `phone-apps.json` (written by
//! scripts/release-phone-apps.sh). The links never change: `releases/latest/download/<name>`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

const MANIFEST_URL: &str = "https://github.com/Soab42/zazabor-home/releases/latest/download/phone-apps.json";
/// A QR code holds a URL here; anything longer is refused.
const MAX_QR_TEXT: usize = 1024;
/// How long a fetched manifest is reused.
const MANIFEST_TTL: Duration = Duration::from_secs(10 * 60);

/// A QR code for `text` (a download link), as SVG, drawn on this PC.
#[tauri::command]
pub fn qr_svg(text: String) -> Result<String, String> {
    if text.is_empty() || text.len() > MAX_QR_TEXT {
        return Err("That's too long for a QR code.".into());
    }
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M).map_err(|e| e.to_string())?;
    Ok(code.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build())
}

fn cache() -> &'static Mutex<Option<(Instant, Value)>> {
    static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
    &CACHE
}

/// The `apps` object of the latest release's `phone-apps.json` (each app's version and size),
/// or null when it can't be read (offline): the page then just leaves the versions out.
#[tauri::command]
pub async fn phone_apps_info() -> Value {
    if let Some((at, v)) = cache().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if at.elapsed() < MANIFEST_TTL {
            return v.clone();
        }
    }
    let fetched = tauri::async_runtime::spawn_blocking(fetch_manifest).await.ok().flatten();
    match fetched {
        Some(apps) => {
            *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), apps.clone()));
            apps
        }
        None => Value::Null,
    }
}

fn fetch_manifest() -> Option<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent.get(MANIFEST_URL).call().ok()?;
    if response.status() != 200 {
        return None;
    }
    let body = response.into_body().with_config().limit(256 << 10).read_to_string().ok()?;
    apps_of(&body)
}

/// The `apps` object of a manifest, if it is one.
fn apps_of(body: &str) -> Option<Value> {
    let manifest: Value = serde_json::from_str(body).ok()?;
    (manifest.get("kind")?.as_str()? == "zazabor-phone-apps").then(|| manifest.get("apps").cloned()).flatten().filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_codes_are_svg_and_refuse_huge_text() {
        let svg = qr_svg("https://github.com/Soab42/zazabor-home/releases/latest/download/zazabor-tv.apk".into()).unwrap();
        assert!(svg.starts_with("<?xml") && svg.contains("<svg"), "{svg:.80}");
        assert!(qr_svg(String::new()).is_err());
        assert!(qr_svg("x".repeat(MAX_QR_TEXT + 1)).is_err());
    }

    #[test]
    fn the_manifest_gives_its_apps_only_when_it_is_ours() {
        let ok = r#"{"kind":"zazabor-phone-apps","apps":{"tv":{"version":"0.2.0","bytes":12458190}}}"#;
        assert_eq!(apps_of(ok).unwrap()["tv"]["version"], "0.2.0");
        assert!(apps_of(r#"{"kind":"other","apps":{}}"#).is_none());
        assert!(apps_of("not json").is_none());
    }
}
