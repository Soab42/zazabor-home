//! The models Meet's transcripts and notes need, downloaded on first use into the app's data
//! folder after the user agrees: Whisper small and medium (whisper.cpp's official ggml files, the
//! same ones and the same folder as Studio's captions) and Qwen2.5-3B-Instruct (Q4_K_M GGUF, from
//! Qwen's own repository). Resumable, cancellable, with progress, and each file is checked
//! against the size and SHA-256 pinned here before it's used.

use crate::download::fetch;
use crate::videoedit::models::finish;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// whisper.cpp's repository at the commit Studio's checksums come from.
const WHISPER_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1";
/// Qwen's official GGUF repository at a fixed commit.
const QWEN_BASE: &str = "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/7dabda4d13d513e3e842b20f0d435c732f172cbe";

#[derive(Debug)]
pub struct Spec {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub base: &'static str,
    /// Where it lives under the app data folder.
    pub folder: &'static [&'static str],
    pub bytes: u64,
    pub sha256: &'static str,
    pub note: &'static str,
}

pub const MODELS: [Spec; 3] = [
    Spec {
        id: "small",
        label: "Whisper small",
        file: "ggml-small.bin",
        base: WHISPER_BASE,
        folder: &["videoedit", "models"],
        bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        note: "Speech to text; good for English.",
    },
    Spec {
        id: "medium",
        label: "Whisper medium",
        file: "ggml-medium.bin",
        base: WHISPER_BASE,
        folder: &["videoedit", "models"],
        bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        note: "Speech to text; clearly better for Bangla.",
    },
    Spec {
        id: "qwen",
        label: "Qwen2.5 3B Instruct",
        file: "qwen2.5-3b-instruct-q4_k_m.gguf",
        base: QWEN_BASE,
        folder: &["ai", "models"],
        bytes: 2_104_932_768,
        sha256: "626b4a6678b86442240e33df819e00132d3ba7dddfe1cdc4fbb18e0a9615c62d",
        note: "Writes the notes and translates between English and Bangla.",
    },
];

pub fn spec(id: &str) -> Result<&'static Spec, String> {
    MODELS.iter().find(|m| m.id == id).ok_or_else(|| format!("There's no model “{id}”."))
}

fn dir_of(data: &Path, spec: &Spec) -> PathBuf {
    spec.folder.iter().fold(data.to_path_buf(), |p, s| p.join(s))
}

pub fn path_in(data: &Path, spec: &Spec) -> PathBuf {
    dir_of(data, spec).join(spec.file)
}

fn part_in(data: &Path, spec: &Spec) -> PathBuf {
    dir_of(data, spec).join(format!("{}.part", spec.file))
}

fn url_for(spec: &Spec) -> String {
    let base = std::env::var("ZAZABOR_AI_BASE_URL").unwrap_or_else(|_| spec.base.into());
    format!("{}/{}", base.trim_end_matches('/'), spec.file)
}

/// The installed model file, if it's there and whole.
pub fn installed(data: &Path, spec: &Spec) -> Option<PathBuf> {
    let path = path_in(data, spec);
    (std::fs::metadata(&path).ok()?.len() == spec.bytes).then_some(path)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: String,
    pub label: String,
    pub note: String,
    pub bytes: u64,
    pub installed: bool,
    /// Bytes downloaded so far (a paused or running download).
    pub partial: u64,
    pub downloading: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct Download {
    done: AtomicU64,
    cancel: AtomicBool,
    running: AtomicBool,
    error: Mutex<Option<String>>,
}

fn downloads() -> &'static Mutex<HashMap<String, Arc<Download>>> {
    static D: OnceLock<Mutex<HashMap<String, Arc<Download>>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn statuses(data: &Path) -> Vec<Status> {
    let running = lock(downloads()).clone();
    MODELS
        .iter()
        .map(|m| {
            let d = running.get(m.id);
            let partial = d.filter(|d| d.running.load(Ordering::Relaxed)).map(|d| d.done.load(Ordering::Relaxed)).unwrap_or_else(|| std::fs::metadata(part_in(data, m)).map_or(0, |md| md.len()));
            Status {
                id: m.id.into(),
                label: m.label.into(),
                note: m.note.into(),
                bytes: m.bytes,
                installed: installed(data, m).is_some(),
                partial,
                downloading: d.is_some_and(|d| d.running.load(Ordering::Relaxed)),
                error: d.and_then(|d| lock(&d.error).clone()),
            }
        })
        .collect()
}

/// Starts (or continues) downloading a model in the background; `on_done` runs when it ends
/// (whether it worked, was cancelled or failed).
pub fn start(data: PathBuf, id: &str, on_done: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let spec = spec(id)?;
    if installed(&data, spec).is_some() {
        on_done();
        return Ok(());
    }
    std::fs::create_dir_all(dir_of(&data, spec)).map_err(|e| e.to_string())?;
    let job = {
        let mut map = lock(downloads());
        if map.get(id).is_some_and(|d| d.running.load(Ordering::Relaxed)) {
            return Ok(());
        }
        let job = Arc::new(Download::default());
        job.running.store(true, Ordering::Relaxed);
        map.insert(id.to_string(), job.clone());
        job
    };
    std::thread::Builder::new()
        .name(format!("ai-model-{id}"))
        .spawn(move || {
            super::sys::lower_priority();
            let (part, dest) = (part_in(&data, spec), path_in(&data, spec));
            let mut result = fetch(&url_for(spec), &part, spec.bytes, &job.done, &job.cancel);
            if result.is_ok() {
                result = finish(&part, &dest, spec.bytes, spec.sha256);
            }
            if let Err(why) = result {
                if why != "Cancelled" {
                    *lock(&job.error) = Some(why);
                }
            }
            job.running.store(false, Ordering::Relaxed);
            on_done();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Pauses a download (what's there is kept for later).
pub fn cancel(id: &str) {
    if let Some(d) = lock(downloads()).get(id) {
        d.cancel.store(true, Ordering::Relaxed);
    }
}

/// Deletes a model (and any half-downloaded copy).
pub fn remove(data: &Path, id: &str) -> Result<(), String> {
    let spec = spec(id)?;
    cancel(id);
    for p in [path_in(data, spec), part_in(data, spec)] {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => {}
        }
    }
    lock(downloads()).remove(id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_models_are_the_official_files() {
        for m in &MODELS {
            assert_eq!(m.sha256.len(), 64);
            assert!(m.base.starts_with("https://huggingface.co/"));
        }
        assert!(MODELS.iter().filter(|m| m.id != "qwen").all(|m| m.file.starts_with("ggml-") && !m.file.contains(".en.")), "multilingual whisper only");
        // Studio's small model is the same file, so it isn't downloaded twice.
        let small = spec("small").unwrap();
        let studio = crate::videoedit::models::spec("small").unwrap();
        assert_eq!((small.file, small.bytes, small.sha256), (studio.file, studio.bytes, studio.sha256));
        assert_eq!(path_in(Path::new("/d"), small), PathBuf::from("/d/videoedit/models/ggml-small.bin"));
    }

    #[test]
    fn a_missing_or_short_file_isnt_installed() {
        let dir = std::env::temp_dir().join(format!("zazabor-ai-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let spec = spec("small").unwrap();
        assert!(installed(&dir, spec).is_none());
        std::fs::create_dir_all(dir_of(&dir, spec)).unwrap();
        std::fs::write(path_in(&dir, spec), b"short").unwrap();
        assert!(installed(&dir, spec).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
