//! Where Meet's secrets are kept (the same approach as `mail/secrets.rs`): the desktop's keyring
//! (Secret Service), else a file only the user can read in the data folder. One entry per secret:
//! the named tunnel's token (`TUNNEL`) and one API key per cloud AI provider (`cloud_key`). The
//! page never gets a secret back, and it is never logged.
//!
//! `ZAZABOR_MAIL_NO_KEYRING=1` (or `ZAZABOR_MEET_NO_KEYRING=1`) keeps them in files (tests, which
//! must never touch the real keyring).

use std::path::{Path, PathBuf};

/// The entry of the named tunnel's token.
pub const TUNNEL: &str = "tunnel-token";

/// The entry of a cloud AI provider's API key (`provider` is `ai::cloud::Provider::id`).
pub fn cloud_key(provider: &str) -> String {
    format!("ai-key-{provider}")
}

/// A name that is safe as a file name and a keyring attribute.
fn valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join("secrets").join(name)
}

fn use_keyring() -> bool {
    std::env::var_os("ZAZABOR_MAIL_NO_KEYRING").is_none() && std::env::var_os("ZAZABOR_MEET_NO_KEYRING").is_none()
}

#[cfg(target_os = "linux")]
mod keyring {
    use dbus_secret_service::{EncryptionType, SecretService};
    use std::collections::HashMap;

    /// The tunnel token keeps the attributes it was first filed under.
    fn purpose(name: &str) -> String {
        if name == super::TUNNEL {
            "meet-tunnel-token".into()
        } else {
            format!("meet-{name}")
        }
    }

    fn attrs(purpose: &str) -> HashMap<&str, &str> {
        HashMap::from([("application", "zazabor"), ("purpose", purpose)])
    }

    fn service() -> Option<SecretService> {
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Plain, 60).ok()
    }

    pub fn get(name: &str) -> Option<String> {
        let ss = service()?;
        let purpose = purpose(name);
        let found = ss.search_items(attrs(&purpose)).ok()?;
        let item = found.unlocked.into_iter().next().or_else(|| found.locked.into_iter().next())?;
        let _ = item.ensure_unlocked();
        String::from_utf8(item.get_secret().ok()?).ok().filter(|s| !s.is_empty())
    }

    pub fn set(name: &str, secret: &str) -> bool {
        let Some(ss) = service() else { return false };
        let purpose = purpose(name);
        let Ok(collection) = ss.get_default_collection().or_else(|_| ss.get_any_collection()) else { return false };
        if collection.ensure_unlocked().is_err() {
            return false;
        }
        let label = if name == super::TUNNEL { "Zazabor Meet: tunnel token".to_string() } else { format!("Zazabor Meet: {name}") };
        collection.create_item(&label, attrs(&purpose), secret.as_bytes(), true, "text/plain").is_ok()
    }

    pub fn clear(name: &str) {
        if let Some(ss) = service() {
            let purpose = purpose(name);
            if let Ok(found) = ss.search_items(attrs(&purpose)) {
                for item in found.unlocked.iter().chain(found.locked.iter()) {
                    let _ = item.delete();
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod keyring {
    pub fn get(_name: &str) -> Option<String> {
        None
    }
    pub fn set(_name: &str, _secret: &str) -> bool {
        false
    }
    pub fn clear(_name: &str) {}
}

/// The secret filed under `name`, if there is one.
pub fn get_named(dir: &Path, name: &str) -> Option<String> {
    if !valid(name) {
        return None;
    }
    use_keyring()
        .then(|| keyring::get(name))
        .flatten()
        .or_else(|| std::fs::read_to_string(file(dir, name)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

pub fn set_named(dir: &Path, name: &str, secret: &str) -> Result<(), String> {
    if !valid(name) {
        return Err("That isn't a place a secret can be kept.".into());
    }
    let secret = secret.trim();
    if secret.is_empty() {
        return Err("There is nothing to save.".into());
    }
    let path = file(dir, name);
    if use_keyring() {
        keyring::clear(name);
        if keyring::set(name, secret) {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
    }
    let folder = path.parent().ok_or("No place to keep the secret.")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_file(&path);
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
        out.write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, secret).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear_named(dir: &Path, name: &str) {
    if !valid(name) {
        return;
    }
    if use_keyring() {
        keyring::clear(name);
    }
    let _ = std::fs::remove_file(file(dir, name));
}

pub fn get(dir: &Path) -> Option<String> {
    get_named(dir, TUNNEL)
}

pub fn set(dir: &Path, token: &str) -> Result<(), String> {
    set_named(dir, TUNNEL, token)
}

pub fn clear(dir: &Path) {
    clear_named(dir, TUNNEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files only: the real keyring is never touched.
    fn quiet() -> PathBuf {
        std::env::set_var("ZAZABOR_MEET_NO_KEYRING", "1");
        let dir = std::env::temp_dir().join(format!("zazabor-meet-secret-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn each_provider_has_its_own_entry_and_the_tunnel_token_is_unchanged() {
        let dir = quiet();
        assert_eq!(file(&dir, TUNNEL), dir.join("secrets").join("tunnel-token"));
        set(&dir, "cf-token").unwrap();
        set_named(&dir, &cloud_key("claude"), "sk-ant-secret-1").unwrap();
        set_named(&dir, &cloud_key("openai"), " sk-secret-2 \n").unwrap();
        assert_eq!(get(&dir).as_deref(), Some("cf-token"));
        assert_eq!(get_named(&dir, &cloud_key("claude")).as_deref(), Some("sk-ant-secret-1"));
        assert_eq!(get_named(&dir, &cloud_key("openai")).as_deref(), Some("sk-secret-2"), "trimmed");
        assert_eq!(get_named(&dir, &cloud_key("gemini")), None);
        clear_named(&dir, &cloud_key("claude"));
        assert_eq!(get_named(&dir, &cloud_key("claude")), None);
        assert_eq!(get_named(&dir, &cloud_key("openai")).as_deref(), Some("sk-secret-2"), "the others stay");
        assert_eq!(get(&dir).as_deref(), Some("cf-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_names_cannot_escape_the_folder() {
        use std::os::unix::fs::PermissionsExt;
        let dir = quiet();
        set_named(&dir, &cloud_key("gemini"), "AIza-secret").unwrap();
        let mode = std::fs::metadata(file(&dir, &cloud_key("gemini"))).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(set_named(&dir, "../escape", "x").is_err());
        assert!(set_named(&dir, "", "x").is_err());
        assert!(set_named(&dir, &cloud_key("claude"), "  ").is_err(), "an empty key isn't saved");
        assert_eq!(get_named(&dir, "../escape"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
