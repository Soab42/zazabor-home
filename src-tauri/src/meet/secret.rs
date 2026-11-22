//! Where the named tunnel's token is kept (the same approach as `mail/secrets.rs`): the desktop's
//! keyring (Secret Service), else a file only the user can read in the data folder. The page never
//! gets the token back, and it is never logged.
//!
//! `ZAZABOR_MAIL_NO_KEYRING=1` (or `ZAZABOR_MEET_NO_KEYRING=1`) keeps it in the file (tests, which
//! must never touch the real keyring).

use std::path::{Path, PathBuf};

fn file(dir: &Path) -> PathBuf {
    dir.join("secrets").join("tunnel-token")
}

fn use_keyring() -> bool {
    std::env::var_os("ZAZABOR_MAIL_NO_KEYRING").is_none() && std::env::var_os("ZAZABOR_MEET_NO_KEYRING").is_none()
}

#[cfg(target_os = "linux")]
mod keyring {
    use dbus_secret_service::{EncryptionType, SecretService};
    use std::collections::HashMap;

    fn attrs() -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", "zazabor"), ("purpose", "meet-tunnel-token")])
    }

    fn service() -> Option<SecretService> {
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Plain, 60).ok()
    }

    pub fn get() -> Option<String> {
        let ss = service()?;
        let found = ss.search_items(attrs()).ok()?;
        let item = found.unlocked.into_iter().next().or_else(|| found.locked.into_iter().next())?;
        let _ = item.ensure_unlocked();
        String::from_utf8(item.get_secret().ok()?).ok().filter(|s| !s.is_empty())
    }

    pub fn set(token: &str) -> bool {
        let Some(ss) = service() else { return false };
        let Ok(collection) = ss.get_default_collection().or_else(|_| ss.get_any_collection()) else { return false };
        if collection.ensure_unlocked().is_err() {
            return false;
        }
        collection.create_item("Zazabor Meet: tunnel token", attrs(), token.as_bytes(), true, "text/plain").is_ok()
    }

    pub fn clear() {
        if let Some(ss) = service() {
            if let Ok(found) = ss.search_items(attrs()) {
                for item in found.unlocked.iter().chain(found.locked.iter()) {
                    let _ = item.delete();
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod keyring {
    pub fn get() -> Option<String> {
        None
    }
    pub fn set(_token: &str) -> bool {
        false
    }
    pub fn clear() {}
}

pub fn get(dir: &Path) -> Option<String> {
    use_keyring()
        .then(keyring::get)
        .flatten()
        .or_else(|| std::fs::read_to_string(file(dir)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

pub fn set(dir: &Path, token: &str) -> Result<(), String> {
    let path = file(dir);
    if use_keyring() {
        keyring::clear();
        if keyring::set(token) {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
    }
    let folder = path.parent().ok_or("No place to keep the token.")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_file(&path);
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
        out.write_all(token.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, token).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear(dir: &Path) {
    if use_keyring() {
        keyring::clear();
    }
    let _ = std::fs::remove_file(file(dir));
}
