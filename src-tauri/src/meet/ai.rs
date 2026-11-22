//! Transcripts, subtitles and notes for recorded meetings (docs/plans/meet-notes.md). The host's
//! page records one audio track per participant besides the mixed video; once a recording ends
//! (or on demand) a background job transcribes every track with Whisper, merges them on a
//! timeline, writes the transcript and the subtitles (original, English, Bangla), and then a small
//! local LLM translates what Whisper couldn't and writes the notes. All on this PC, at low
//! priority, cancellable. Transcript text is never logged: only counts and times.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::store::{self, Recording};
use super::transcript::{self, Doc, Heard, Notes, RawSeg, Seg, Track};
use super::{lock, now_ms, Hub, Result};
use crate::ai::llm::{self, Llm};
use crate::ai::models;
use crate::ai::whisper::{self, mel, Options};

/// A recording that hasn't had a chunk for this long has ended.
const IDLE_MS: i64 = 45_000;
/// Bangla transcription, translation and notes. Off in 0.3.4: stock Whisper and Qwen gave
/// unusable Bangla in tests, so Bangla speech gets a placeholder line until Bangla-tuned models
/// are in (docs/plans/meet-notes.md). The page has the same switch (`BANGLA_READY`).
pub const BANGLA_READY: bool = false;
/// What Bangla speech shows while `BANGLA_READY` is off.
const BANGLA_PLACEHOLDER: &str = "[Bangla: transcription coming in a later update]";
/// Whisper may only tell English from Bangla here.
const LANGS: [&str; 2] = ["en", "bn"];

// ---- settings

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Transcribe and take notes when a meeting ends.
    pub auto: bool,
    /// "auto" (small, medium for Bangla), "small" or "medium".
    pub model: String,
    /// "en", "bn" or "both".
    pub notes_lang: String,
    /// Translate English speech into Bangla subtitles with the LLM (slow).
    pub bn_subs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto: true, model: "auto".into(), notes_lang: "en".into(), bn_subs: BANGLA_READY }
    }
}

// ---- files beside a recording

pub struct Paths {
    pub tracks: PathBuf,
    pub transcript: PathBuf,
    pub vtt: PathBuf,
    pub en_vtt: PathBuf,
    pub bn_vtt: PathBuf,
    pub notes_json: PathBuf,
    pub notes_md: PathBuf,
    pub state: PathBuf,
}

pub fn paths(rec: &Path) -> Paths {
    Paths {
        tracks: rec.with_extension("tracks"),
        transcript: rec.with_extension("transcript.json"),
        vtt: rec.with_extension("vtt"),
        en_vtt: rec.with_extension("en.vtt"),
        bn_vtt: rec.with_extension("bn.vtt"),
        notes_json: rec.with_extension("notes.json"),
        notes_md: rec.with_extension("notes.md"),
        state: rec.with_extension("ai.json"),
    }
}

/// Every file of a recording's transcript and notes (deleted with it).
pub fn extra_files(rec: &Path) -> Vec<PathBuf> {
    let p = paths(rec);
    vec![p.transcript, p.vtt, p.en_vtt, p.bn_vtt, p.notes_json, p.notes_md, p.state]
}

pub struct TrackInfo {
    pub stem: String,
    pub name: String,
    /// Seconds into the meeting where the track starts.
    pub offset: f64,
    pub path: PathBuf,
}

/// The tracks saved for a recording, in the order they began.
pub fn list_tracks(rec: &Path) -> Vec<TrackInfo> {
    let Ok(dir) = std::fs::read_dir(paths(rec).tracks) else { return Vec::new() };
    let mut out: Vec<(u32, TrackInfo)> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "webm"))
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            let (n, rest) = stem.split_once('-')?;
            let n: u32 = n.parse().ok()?;
            let meta: Value = std::fs::read(path.with_extension("json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
            let name = meta.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(rest).to_string();
            let offset = meta.get("offsetMs").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
            Some((n, TrackInfo { stem, name, offset, path }))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out.into_iter().map(|(_, t)| t).collect()
}

// ---- track uploads

/// A track being written.
pub struct TrackFile {
    code: String,
    path: PathBuf,
    next: u64,
}

/// What a state file says about the last job.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Saved {
    /// "done", "error", "cancelled", "off" or "needs-models".
    status: String,
    message: String,
    /// Model ids a "needs-models" job is waiting for.
    #[serde(default)]
    missing: Vec<String>,
    #[serde(default)]
    at: i64,
}

fn read_saved(path: &Path) -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_saved(path: &Path, s: &Saved) {
    let _ = std::fs::write(path, serde_json::to_vec(s).unwrap_or_default());
}

#[derive(Default)]
pub struct State {
    pub(super) tracks: Mutex<HashMap<String, Arc<Mutex<TrackFile>>>>,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    /// One job at a time.
    run: Mutex<()>,
}

pub struct Job {
    cancel: AtomicBool,
    now: Mutex<(String, f64, String)>,
    queued: AtomicBool,
    /// (tokens, seconds) the LLM has used.
    llm: Mutex<(usize, f64)>,
}

impl Job {
    fn new() -> Arc<Job> {
        Arc::new(Job { cancel: AtomicBool::new(false), now: Mutex::new(("queued".into(), 0.0, "Waiting for its turn…".into())), queued: AtomicBool::new(true), llm: Mutex::new((0, 0.0)) })
    }
}

/// What the page shows on a recording card.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub id: String,
    pub tracks: usize,
    /// "none", "queued", "running", "done", "error", "cancelled", "off" or "needs-models".
    pub state: String,
    pub stage: String,
    pub progress: f64,
    pub message: String,
    pub has_transcript: bool,
    pub has_notes: bool,
    pub missing: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    /// "all" (default), or "notes" to keep the transcript and write the notes again.
    pub redo: Option<String>,
    /// "auto", "small" or "medium" for this run.
    pub model: Option<String>,
}

impl Hub {
    pub fn ai_settings(&self) -> Settings {
        self.with_db(|c| {
            let d = Settings::default();
            Ok(Settings {
                auto: store::meta(c, "ai_auto").map_or(d.auto, |v| v != "0"),
                model: store::meta(c, "ai_model").filter(|v| ["auto", "small", "medium"].contains(&v.as_str())).unwrap_or(d.model),
                notes_lang: if BANGLA_READY { store::meta(c, "ai_notes_lang").filter(|v| ["en", "bn", "both"].contains(&v.as_str())).unwrap_or(d.notes_lang) } else { "en".into() },
                bn_subs: BANGLA_READY && store::meta(c, "ai_bn_subs").map_or(d.bn_subs, |v| v != "0"),
            })
        })
        .unwrap_or_default()
    }

    pub fn set_ai_settings(&self, s: &Settings) -> Result<Settings> {
        if !["auto", "small", "medium"].contains(&s.model.as_str()) || !["en", "bn", "both"].contains(&s.notes_lang.as_str()) {
            return Err("That isn't one of the choices.".into());
        }
        self.with_db(|c| {
            store::set_meta(c, "ai_auto", if s.auto { "1" } else { "0" })?;
            store::set_meta(c, "ai_model", &s.model)?;
            store::set_meta(c, "ai_notes_lang", &s.notes_lang)?;
            store::set_meta(c, "ai_bn_subs", if s.bn_subs { "1" } else { "0" })
        })?;
        Ok(self.ai_settings())
    }

    /// Appends a chunk of one participant's audio track. Returns the next sequence number expected.
    #[allow(clippy::too_many_arguments)]
    pub fn track_append(&self, code: &str, rec: &str, track: &str, seq: u64, name: &str, offset_ms: i64, body: &[u8]) -> std::result::Result<u64, (&'static str, String)> {
        if track.len() < 4 || track.len() > 40 || !track.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(("400 Bad Request", "bad track id".into()));
        }
        // The recording must have started (the mixed file's first chunk is there).
        let rec_path = {
            let recs = lock(&self.recs);
            let f = recs.get(rec).ok_or(("404 Not Found", "unknown recording".to_string()))?;
            let f = lock(f);
            if f.code != code {
                return Err(("403 Forbidden", "wrong meeting".into()));
            }
            f.path.clone()
        };
        let key = format!("{rec}:{track}");
        let file = {
            let mut tracks = lock(&self.ai.tracks);
            if let Some(f) = tracks.get(&key) {
                f.clone()
            } else if seq == 0 {
                let prefix = format!("{rec}:");
                let n = tracks.keys().filter(|k| k.starts_with(&prefix)).count();
                if n >= 32 {
                    return Err(("429 Too Many Requests", "too many tracks".into()));
                }
                let dir = paths(&rec_path).tracks;
                std::fs::create_dir_all(&dir).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
                let name = super::file_safe(name).chars().take(40).collect::<String>();
                let stem = format!("{}-{name}", n + 1);
                let path = dir.join(format!("{stem}.webm"));
                std::fs::File::create(&path).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
                let meta = json!({"id": track, "name": name, "offsetMs": offset_ms.clamp(0, 86_400_000)});
                std::fs::write(dir.join(format!("{stem}.json")), meta.to_string()).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
                let f = Arc::new(Mutex::new(TrackFile { code: code.to_string(), path, next: 0 }));
                tracks.insert(key, f.clone());
                f
            } else {
                return Err(("404 Not Found", "unknown track".into()));
            }
        };
        let mut f = lock(&file);
        if seq < f.next {
            return Ok(f.next);
        }
        if seq > f.next {
            return Err(("409 Conflict", format!("expected {}", f.next)));
        }
        use std::io::Write;
        let mut out = std::fs::OpenOptions::new().append(true).open(&f.path).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
        out.write_all(body).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
        f.next += 1;
        Ok(f.next)
    }

    /// The host's page says the recording is complete: stop accepting chunks and start the job.
    pub fn rec_finish(self: &Arc<Self>, code: &str, rec: &str) -> std::result::Result<(), (&'static str, String)> {
        {
            let recs = lock(&self.recs);
            let f = recs.get(rec).ok_or(("404 Not Found", "unknown recording".to_string()))?;
            if lock(f).code != code {
                return Err(("403 Forbidden", "wrong meeting".into()));
            }
        }
        self.finished(rec);
        Ok(())
    }

    /// A recording is over: forget its open files, and start the job if that's wanted.
    fn finished(self: &Arc<Self>, rec: &str) {
        lock(&self.recs).remove(rec);
        let prefix = format!("{rec}:");
        lock(&self.ai.tracks).retain(|k, _| !k.starts_with(&prefix));
        self.changed();
        if self.ai_settings().auto {
            if let Ok(r) = self.with_db(|c| store::recording(c, rec)) {
                if !list_tracks(Path::new(&r.path)).is_empty() && !paths(Path::new(&r.path)).transcript.exists() && read_saved(&paths(Path::new(&r.path)).state).is_none() {
                    let _ = self.ai_start(rec, RunOptions::default());
                }
            }
        }
    }

    /// Every 30 s: recordings that stopped receiving chunks have ended.
    pub fn ai_tick(self: &Arc<Self>) {
        let now = now_ms();
        let open: Vec<String> = lock(&self.recs).keys().cloned().collect();
        if open.is_empty() {
            return;
        }
        let Ok(all) = self.with_db(|c| store::recordings(c)) else { return };
        for r in all.iter().filter(|r| open.contains(&r.id)) {
            if r.ended_at.is_some_and(|e| now - e > IDLE_MS) {
                self.finished(&r.id);
            }
        }
    }

    // ---- state for the page

    pub fn ai_infos(&self) -> Result<Vec<Info>> {
        let recs = self.recordings()?;
        let jobs = lock(&self.ai.jobs).clone();
        Ok(recs.iter().map(|r| self.info_of(r, jobs.get(&r.id))).collect())
    }

    fn info_of(&self, r: &Recording, job: Option<&Arc<Job>>) -> Info {
        let rec = Path::new(&r.path);
        let p = paths(rec);
        let mut info = Info {
            id: r.id.clone(),
            tracks: list_tracks(rec).len(),
            state: "none".into(),
            stage: String::new(),
            progress: 0.0,
            message: String::new(),
            has_transcript: p.transcript.exists(),
            has_notes: p.notes_json.exists(),
            missing: Vec::new(),
        };
        if let Some(job) = job {
            let (stage, progress, message) = lock(&job.now).clone();
            info.state = if job.queued.load(Ordering::Relaxed) { "queued" } else { "running" }.into();
            info.stage = stage;
            info.progress = progress;
            info.message = message;
        } else if let Some(s) = read_saved(&p.state) {
            info.state = s.status;
            info.message = s.message;
            info.missing = s.missing;
            info.progress = if info.state == "done" { 1.0 } else { 0.0 };
        } else if info.has_transcript {
            info.state = "done".into();
        }
        info
    }

    /// Turns the automatic job off (or back on) for one meeting.
    pub fn ai_off(&self, id: &str, off: bool) -> Result<()> {
        let r = self.with_db(|c| store::recording(c, id))?;
        let p = paths(Path::new(&r.path));
        if off {
            write_saved(&p.state, &Saved { status: "off".into(), message: "Transcripts are off for this meeting.".into(), at: now_ms(), ..Default::default() });
        } else if read_saved(&p.state).is_some_and(|s| s.status == "off") {
            let _ = std::fs::remove_file(&p.state);
        }
        self.changed();
        Ok(())
    }

    pub fn ai_cancel(&self, id: &str) {
        if let Some(j) = lock(&self.ai.jobs).get(id) {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn transcript(&self, id: &str) -> Result<Doc> {
        let r = self.with_db(|c| store::recording(c, id))?;
        let bytes = std::fs::read(paths(Path::new(&r.path)).transcript).map_err(|_| "This meeting has no transcript yet.".to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| format!("The transcript file is damaged: {e}"))
    }

    pub fn notes(&self, id: &str) -> Result<Option<Value>> {
        let r = self.with_db(|c| store::recording(c, id))?;
        match std::fs::read(paths(Path::new(&r.path)).notes_json) {
            Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("The notes file is damaged: {e}")),
            Err(_) => Ok(None),
        }
    }

    /// Saves notes the user edited (the same file the job writes) and refreshes the Markdown.
    pub fn notes_save(&self, id: &str, notes: Value) -> Result<()> {
        let r = self.with_db(|c| store::recording(c, id))?;
        let p = paths(Path::new(&r.path));
        let mut doc: Value = serde_json::from_slice(&std::fs::read(&p.notes_json).map_err(|_| "There are no notes to save.".to_string())?).map_err(|e| e.to_string())?;
        for lang in ["en", "bn"] {
            if let Some(v) = notes.get(lang) {
                let n: Notes = serde_json::from_value(v.clone()).map_err(|e| format!("Those notes aren't in the right shape: {e}"))?;
                doc[lang] = serde_json::to_value(n).map_err(|e| e.to_string())?;
            }
        }
        doc["edited"] = json!(true);
        std::fs::write(&p.notes_json, serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::write(&p.notes_md, notes_md_text(&r.title, &doc)).map_err(|e| e.to_string())?;
        self.changed();
        Ok(())
    }

    pub fn ai_models(&self) -> Vec<models::Status> {
        models::statuses(&self.data_dir)
    }

    pub fn ai_model_download(self: &Arc<Self>, id: &str) -> Result<()> {
        let hub = self.clone();
        models::start(self.data_dir.clone(), id, move || hub.emit("meet-ai", json!({"models": true})))
    }

    /// Starts the job for a recording in the background (it waits if another is running).
    pub fn ai_start(self: &Arc<Self>, id: &str, opts: RunOptions) -> Result<()> {
        let r = self.with_db(|c| store::recording(c, id))?;
        if lock(&self.recs).contains_key(id) {
            return Err("This meeting is still being recorded.".into());
        }
        let job = {
            let mut jobs = lock(&self.ai.jobs);
            if jobs.contains_key(id) {
                return Err("This meeting is already being worked on.".into());
            }
            let job = Job::new();
            jobs.insert(id.to_string(), job.clone());
            job
        };
        let _ = std::fs::remove_file(paths(Path::new(&r.path)).state);
        let hub = self.clone();
        let spawned = std::thread::Builder::new().name("meet-ai".into()).spawn(move || {
            hub.emit_ai(&r.id, &job);
            let _turn = lock_run(&hub);
            job.queued.store(false, Ordering::Relaxed);
            let result = hub.run_job(&r, &job, &opts);
            let p = paths(Path::new(&r.path));
            let saved = match result {
                Ok(message) => Saved { status: "done".into(), message, at: now_ms(), ..Default::default() },
                Err(e) if e == "Cancelled" => Saved { status: "cancelled".into(), message: "Stopped.".into(), at: now_ms(), ..Default::default() },
                Err(e) => match e.strip_prefix("models:") {
                    Some(list) => Saved { status: "needs-models".into(), message: "Download the speech and notes models first.".into(), missing: list.split(',').map(str::to_string).collect(), at: now_ms() },
                    None => Saved { status: "error".into(), message: e, at: now_ms(), ..Default::default() },
                },
            };
            write_saved(&p.state, &saved);
            lock(&hub.ai.jobs).remove(&r.id);
            hub.changed();
            hub.emit("meet-ai", json!({"id": r.id}));
        });
        if let Err(e) = spawned {
            lock(&self.ai.jobs).remove(id);
            return Err(e.to_string());
        }
        self.changed();
        Ok(())
    }

    fn emit_ai(&self, id: &str, job: &Job) {
        let (stage, progress, message) = lock(&job.now).clone();
        self.emit("meet-ai", json!({"id": id, "state": if job.queued.load(Ordering::Relaxed) { "queued" } else { "running" }, "stage": stage, "progress": progress, "message": message}));
    }

    fn step(&self, id: &str, job: &Job, stage: &str, progress: f64, message: &str) {
        let changed = {
            let mut now = lock(&job.now);
            let changed = now.0 != stage || now.2 != message || (progress - now.1).abs() >= 0.01;
            *now = (stage.to_string(), progress.clamp(0.0, 1.0), message.to_string());
            changed
        };
        if changed {
            self.emit_ai(id, job);
        }
    }
}

fn lock_run(hub: &Hub) -> std::sync::MutexGuard<'_, ()> {
    lock(&hub.ai.run)
}

// ---- the job

fn minutes(secs: f64) -> u64 {
    (secs / 60.0).round() as u64
}

fn bengali_ratio(text: &str) -> f64 {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return 0.0;
    }
    letters.iter().filter(|c| ('\u{0980}'..='\u{09FF}').contains(*c)).count() as f64 / letters.len() as f64
}

/// The model ids a job needs, or why it can't run.
fn plan_models(data: &Path, s: &Settings, redo_notes: bool) -> std::result::Result<(Option<&'static models::Spec>, Option<&'static models::Spec>, &'static models::Spec), String> {
    let get = |id: &str| models::spec(id).expect("a known model");
    let has = |id: &str| models::installed(data, get(id)).is_some();
    let mut missing: Vec<&str> = Vec::new();
    let (small, medium) = (has("small"), has("medium"));
    if !redo_notes {
        match s.model.as_str() {
            "small" if !small => missing.push("small"),
            "medium" if !medium => missing.push("medium"),
            "auto" if !small && !medium => missing.push("small"),
            _ => {}
        }
    }
    if !has("qwen") {
        missing.push("qwen");
    }
    if !missing.is_empty() {
        return Err(format!("models:{}", missing.join(",")));
    }
    Ok((small.then(|| get("small")), medium.then(|| get("medium")), get("qwen")))
}

impl Hub {
    /// The whole job. Returns the message for the card.
    fn run_job(&self, r: &Recording, job: &Arc<Job>, opts: &RunOptions) -> Result<String> {
        let settings = {
            let mut s = self.ai_settings();
            if let Some(m) = opts.model.as_deref().filter(|m| ["auto", "small", "medium"].contains(m)) {
                s.model = m.into();
            }
            s
        };
        let redo_notes = opts.redo.as_deref() == Some("notes");
        let rec_path = PathBuf::from(&r.path);
        let p = paths(&rec_path);
        let (small, medium, qwen) = plan_models(&self.data_dir, &settings, redo_notes && p.transcript.exists())?;
        let qwen_path = models::path_in(&self.data_dir, qwen);
        let id = r.id.as_str();

        let mut doc = if redo_notes && p.transcript.exists() {
            self.transcript(id)?
        } else {
            self.step(id, job, "transcribe", 0.0, "Getting the sound ready…");
            let doc = crate::ai::sys::pool().install(|| self.transcribe_all(r, job, &settings, small, medium))?;
            write_subtitles(&p, &doc)?;
            doc
        };
        self.changed();

        // The LLM: what Whisper couldn't translate, Bangla subtitles, then the notes.
        let result = llm::with(&qwen_path, |llm| {
            self.step(id, job, "translate", 0.6, "Loading the notes model…");
            let mut noted = String::new();
            if !redo_notes {
                self.fill_translations(llm, job, id, &mut doc, &settings)?;
                write_subtitles(&p, &doc)?;
            }
            if !doc.segments.is_empty() {
                let (notes_en, notes_bn) = self.write_notes(llm, job, id, &doc, &settings)?;
                let model = qwen.file.trim_end_matches(".gguf");
                let out = json!({
                    "version": 1, "language": settings.notes_lang, "model": model, "generatedAt": now_ms(), "edited": false,
                    "en": notes_en, "bn": notes_bn,
                });
                std::fs::write(&p.notes_json, serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                std::fs::write(&p.notes_md, notes_md_text(&r.title, &out)).map_err(|e| e.to_string())?;
                if out["bn"].is_null() && settings.notes_lang != "en" {
                    noted = " The Bangla notes couldn't be written.".into();
                }
            }
            Ok(noted)
        });
        llm::unload();
        let noted = result?;
        let (tokens, secs) = *lock(&job.llm);
        if secs > 0.0 {
            eprintln!("[meet-ai] notes model: {tokens} tokens in {secs:.0} s ({:.1} tokens/s)", tokens as f64 / secs);
        }
        let n = doc.segments.len();
        Ok(if n == 0 { "No speech was found.".to_string() } else { format!("{n} lines from {} min of talk.{noted}", minutes(doc.duration)) })
    }

    fn transcribe_all(&self, r: &Recording, job: &Arc<Job>, s: &Settings, small: Option<&'static models::Spec>, medium: Option<&'static models::Spec>) -> Result<Doc> {
        let rec_path = PathBuf::from(&r.path);
        let mut tracks = list_tracks(&rec_path);
        if tracks.is_empty() {
            // An older recording, or one without tracks: the mixed sound is one speaker.
            tracks.push(TrackInfo { stem: "mixed".into(), name: "Meeting".into(), offset: 0.0, path: rec_path.clone() });
        }
        let total_secs = (r.ended_at.unwrap_or(r.started_at) - r.started_at).max(1000) as f64 / 1000.0;
        let est: Vec<f64> = tracks.iter().map(|t| (total_secs - t.offset).max(1.0)).collect();
        let total_est: f64 = est.iter().sum();
        let first = match s.model.as_str() {
            "medium" => medium,
            "small" => small,
            _ => small.or(medium),
        };
        let cache = std::env::temp_dir().join(format!("zazabor-meet-ai-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cache);
        let mut heard = Vec::new();
        let mut used = String::new();
        let mut done_secs = 0.0;
        let mut duration = 0f64;
        let id = r.id.as_str();
        let busy = || {
            let _ = crate::ai::sys::wait_while_busy(&job.cancel, &|| {});
        };
        let started = std::time::Instant::now();
        let mut heard_secs = 0f64;
        for (i, t) in tracks.iter().enumerate() {
            if job.cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            self.step(id, job, "transcribe", done_secs / total_est * 0.6, &format!("Transcribing {} of {} min…", minutes(done_secs), minutes(total_est)));
            let bytes = crate::videoedit::audio::pcm(&t.path.to_string_lossy(), &cache, true)?;
            let pcm = mel::from_editor_pcm(&bytes);
            drop(bytes);
            let seconds = pcm.len() as f64 / mel::SAMPLE_RATE as f64;
            duration = duration.max(t.offset + seconds);
            let peak = pcm.iter().fold(0f32, |m, v| m.max(v.abs()));
            if peak < 0.003 {
                // A muted microphone: nothing to hear.
                done_secs += est[i];
                continue;
            }
            let progress = |f: f64| {
                busy();
                let at = done_secs + f * est[i];
                self.step(id, job, "transcribe", at / total_est * 0.6, &format!("Transcribing {} of {} min…", minutes(at), minutes(total_est)));
            };
            let clock = std::time::Instant::now();
            let mut model = first.ok_or("models:small")?;
            let mut result = whisper::transcribe_with(&models::path_in(&self.data_dir, model), &pcm, &Options { language: None, translate: false, per_window: true, candidates: &LANGS }, &progress, &job.cancel)?;
            // Auto: Bangla needs the bigger model.
            let bn_secs: f64 = result.segments.iter().filter(|s| s.lang == "bn").map(|s| s.end - s.start).sum();
            let all_secs: f64 = result.segments.iter().map(|s| s.end - s.start).sum();
            if BANGLA_READY && s.model == "auto" && model.id == "small" && all_secs > 0.0 && bn_secs / all_secs >= 0.4 {
                if let Some(m) = medium {
                    model = m;
                    self.step(id, job, "transcribe", done_secs / total_est * 0.6, "Bangla heard: listening again with the medium model…");
                    result = whisper::transcribe_with(&models::path_in(&self.data_dir, model), &pcm, &Options { language: None, translate: false, per_window: true, candidates: &LANGS }, &progress, &job.cancel)?;
                }
            }
            if !used.split('+').any(|u| u == model.id) {
                if !used.is_empty() {
                    used.push('+');
                }
                used.push_str(model.id);
            }
            let mut segs: Vec<RawSeg> = result
                .segments
                .iter()
                .map(|s| {
                    let text = if !BANGLA_READY && s.lang == "bn" { BANGLA_PLACEHOLDER.to_string() } else { s.text.clone() };
                    RawSeg { start: s.start, end: s.end, lang: s.lang.clone(), text, en: None }
                })
                .collect();
            // Bangla speech: Whisper's own translation into English, matched by time.
            if BANGLA_READY && segs.iter().any(|s| s.lang == "bn") {
                self.step(id, job, "transcribe", (done_secs + 0.5 * est[i]) / total_est * 0.6, "Translating the Bangla into English…");
                let tr = whisper::transcribe_with(&models::path_in(&self.data_dir, model), &pcm, &Options { language: None, translate: true, per_window: true, candidates: &LANGS }, &progress, &job.cancel)?;
                attach_translation(&mut segs, &tr.segments);
            }
            heard_secs += seconds;
            eprintln!("[meet-ai] track {}: {:.0} s of sound, {} lines, {:.1} s", i + 1, seconds, segs.len(), clock.elapsed().as_secs_f64());
            done_secs += est[i];
            heard.push(Heard { speaker: t.name.clone(), track: t.stem.clone(), offset: t.offset, segments: segs });
        }
        if heard_secs > 0.0 {
            eprintln!("[meet-ai] transcription took {:.1} s for {:.0} s of sound ({:.1} s per minute)", started.elapsed().as_secs_f64(), heard_secs, started.elapsed().as_secs_f64() / heard_secs * 60.0);
        }
        let _ = std::fs::remove_dir_all(&cache);
        whisper::unload();
        let segments = transcript::merge(heard);
        let language = transcript::dominant_language(&segments);
        Ok(Doc { version: 1, model: used, language, duration, segments })
    }

    /// English for Bangla lines Whisper's translation missed, and Bangla for the English lines.
    fn fill_translations(&self, llm: &mut Llm, job: &Arc<Job>, id: &str, doc: &mut Doc, s: &Settings) -> Result<()> {
        let need_en: Vec<usize> = (0..doc.segments.len()).filter(|&i| doc.segments[i].lang == "bn" && doc.segments[i].english().is_none()).collect();
        let need_bn: Vec<usize> = if s.bn_subs { (0..doc.segments.len()).filter(|&i| doc.segments[i].lang != "bn" && doc.segments[i].bangla().is_none()).collect() } else { Vec::new() };
        let total = (need_en.len() + need_bn.len()).max(1);
        let mut done = 0usize;
        for (idx, to) in [(need_en, "en"), (need_bn, "bn")] {
            for batch in batches(&doc.segments, &idx) {
                self.step(id, job, "translate", 0.6 + 0.15 * done as f64 / total as f64, &format!("Translating subtitles… {done} of {total}"));
                let lines: Vec<String> = batch.iter().map(|&i| if to == "bn" { doc.segments[i].english().unwrap_or(&doc.segments[i].text).to_string() } else { doc.segments[i].text.clone() }).collect();
                let got = translate_lines(llm, job, &lines, to)?;
                for (k, &i) in batch.iter().enumerate() {
                    if let Some(t) = got.get(k).cloned().flatten() {
                        if to == "bn" {
                            doc.segments[i].bn = Some(t);
                        } else {
                            doc.segments[i].en = Some(t);
                        }
                    }
                }
                done += batch.len();
            }
        }
        Ok(())
    }

    /// Chapters, the overall notes, and the Bangla version if wanted.
    fn write_notes(&self, llm: &mut Llm, job: &Arc<Job>, id: &str, doc: &Doc, s: &Settings) -> Result<(Notes, Option<Notes>)> {
        let line_of = |g: &Seg| format!("[{}] {}: {}\n", transcript::clock(g.start), g.speaker, g.english().unwrap_or(&g.text));
        let ranges = transcript::chapters(&doc.segments, transcript::CUT, |g| line_of(g).len());
        let mut parts: Vec<(String, Notes)> = Vec::new();
        let mut chapters = Vec::new();
        for (k, r) in ranges.iter().enumerate() {
            self.step(id, job, "notes", 0.75 + 0.2 * k as f64 / ranges.len().max(1) as f64, &format!("Writing notes… part {} of {}", k + 1, ranges.len()));
            let text: String = doc.segments[r.clone()].iter().map(&line_of).collect();
            let (title, notes) = ask_json(llm, job, MAP_SYSTEM, &format!("Transcript:\n{text}"), 700, transcript::parse_chapter)?;
            chapters.push(transcript::Chapter { start: doc.segments[r.start].start, end: doc.segments[r.end - 1].end, title: title.clone(), summary: notes.summary.clone() });
            parts.push((title, notes));
        }
        self.step(id, job, "notes", 0.95, "Writing notes… the summary");
        let mut overall = reduce(llm, job, parts)?;
        overall.chapters = chapters;
        let bn = if s.notes_lang == "en" {
            None
        } else {
            self.step(id, job, "notes", 0.96, "Writing the Bangla notes…");
            translate_notes(llm, job, &overall).ok()
        };
        Ok((overall, bn))
    }
}

/// English for each Bangla segment: the translation segments that sit inside its time.
fn attach_translation(segs: &mut [RawSeg], translated: &[whisper::Segment]) {
    for s in segs.iter_mut().filter(|s| s.lang == "bn") {
        let text: Vec<&str> = translated.iter().filter(|t| (t.start + t.end) / 2.0 >= s.start - 0.3 && (t.start + t.end) / 2.0 <= s.end + 0.3).map(|t| t.text.trim()).filter(|t| !t.is_empty()).collect();
        if !text.is_empty() {
            s.en = Some(text.join(" "));
        }
    }
}

// ---- subtitles on disk

fn write_subtitles(p: &Paths, doc: &Doc) -> Result<()> {
    let put = |path: &Path, text: String| std::fs::write(path, text).map_err(|e| e.to_string());
    std::fs::write(&p.transcript, serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    put(&p.vtt, transcript::vtt(&doc.segments, Track::Original))?;
    put(&p.en_vtt, transcript::vtt(&doc.segments, Track::English))?;
    put(&p.bn_vtt, transcript::vtt(&doc.segments, Track::Bangla))
}

fn notes_md_text(title: &str, doc: &Value) -> String {
    let lang = doc.get("language").and_then(Value::as_str).unwrap_or("en");
    let load = |k: &str| doc.get(k).and_then(|v| serde_json::from_value::<Notes>(v.clone()).ok());
    let mut out = format!("# {title}: meeting notes\n\n");
    let (en, bn) = (load("en"), load("bn"));
    match (lang, en, bn) {
        ("bn", _, Some(bn)) => out.push_str(&transcript::notes_markdown(&bn, "bn")),
        ("both", Some(en), Some(bn)) => {
            out.push_str(&format!("# English\n\n{}\n# বাংলা\n\n{}", transcript::notes_markdown(&en, "en"), transcript::notes_markdown(&bn, "bn")));
        }
        (_, Some(en), _) => out.push_str(&transcript::notes_markdown(&en, "en")),
        _ => {}
    }
    out
}

// ---- the LLM's work

const MAP_SYSTEM: &str = "You take notes on one part of a meeting transcript. Reply with one JSON object and nothing else, in exactly this shape: \
{\"title\": \"3 to 6 words\", \"summary\": \"2 or 3 sentences\", \"decisions\": [\"...\"], \"actions\": [{\"owner\": \"name\", \"task\": \"...\", \"due\": \"when it was said, or null\"}], \"questions\": [\"...\"]}. \
Use only what is said. An action's owner is the person who will do it, named as in the transcript. Use empty lists when there are none. Write in English.";

const REDUCE_SYSTEM: &str = "You combine the notes of consecutive parts of one meeting into the notes of the whole meeting. Reply with one JSON object and nothing else, in exactly this shape: \
{\"summary\": \"one short paragraph\", \"decisions\": [\"...\"], \"actions\": [{\"owner\": \"name\", \"task\": \"...\", \"due\": \"when, or null\"}], \"questions\": [\"...\"]}. \
Merge duplicates, keep owners and due dates, and leave out a question a later part answered. Write in English.";

/// Asks for JSON and checks it; one retry tells the model what was wrong.
fn ask_json<T>(llm: &mut Llm, job: &Job, system: &str, user: &str, max_new: usize, parse: impl Fn(&str) -> std::result::Result<T, String>) -> Result<T> {
    let mut ask = |user: &str| -> Result<String> {
        let g = llm.chat(system, user, max_new, &job.cancel, &|| {})?;
        let mut stats = lock(&job.llm);
        stats.0 += g.new_tokens;
        stats.1 += g.seconds;
        Ok(g.text)
    };
    let first = ask(user)?;
    match parse(&first) {
        Ok(v) => Ok(v),
        Err(why) => {
            eprintln!("[meet-ai] an answer of {} characters was refused; asking again", first.chars().count());
            let tail: String = first.chars().take(1500).collect();
            let again = ask(&format!("{user}\n\nYour previous answer was:\n{tail}\n\nIt was rejected because {why}. Reply again with only the corrected JSON object."))?;
            parse(&again).map_err(|e| format!("The notes model's answer wasn't usable: {e}"))
        }
    }
}

/// Combines parts into the notes of the whole meeting (in groups, if there are many).
fn reduce(llm: &mut Llm, job: &Job, parts: Vec<(String, Notes)>) -> Result<Notes> {
    let mut level: Vec<Notes> = parts.into_iter().map(|(_, n)| n).collect();
    if level.len() == 1 {
        return Ok(level.remove(0));
    }
    loop {
        let mut next = Vec::new();
        for group in level.chunks(8) {
            if group.len() == 1 {
                next.push(group[0].clone());
                continue;
            }
            let body: Vec<Value> = group.iter().map(|n| json!({"summary": n.summary, "decisions": n.decisions, "actions": n.actions, "questions": n.questions})).collect();
            let user = format!("Notes of the parts, in order:\n{}", serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?);
            next.push(ask_json(llm, job, REDUCE_SYSTEM, &user, 900, transcript::parse_overall)?);
        }
        if next.len() == 1 {
            return Ok(next.remove(0));
        }
        level = next;
    }
}

fn translate_notes(llm: &mut Llm, job: &Job, en: &Notes) -> Result<Notes> {
    let system = "You translate meeting notes from English into Bangla (Bengali script). The user sends a JSON object. Reply with the same JSON, same keys and same structure, with every text value translated. Keep the names of people, numbers and dates as they are. Reply with the JSON only.";
    let (nd, na, nq) = (en.decisions.len(), en.actions.len(), en.questions.len());
    let body = json!({"summary": en.summary, "decisions": en.decisions, "actions": en.actions, "questions": en.questions});
    let check = move |text: &str| -> std::result::Result<Notes, String> {
        let n = transcript::parse_overall(text)?;
        if (n.decisions.len(), n.actions.len(), n.questions.len()) != (nd, na, nq) {
            return Err("the lists must have the same number of items as the original".into());
        }
        if bengali_ratio(&n.summary) < 0.5 {
            return Err("the summary is not in Bangla script".into());
        }
        Ok(n)
    };
    let mut bn = ask_json(llm, job, system, &body.to_string(), 1800, check)?;
    // The owners are names: keep them as they were, and the due dates as said.
    for (b, e) in bn.actions.iter_mut().zip(&en.actions) {
        b.owner = e.owner.clone();
    }
    let mut chapters = Vec::new();
    for group in en.chapters.chunks(6) {
        let body = json!({"chapters": group.iter().map(|c| json!({"title": c.title, "summary": c.summary})).collect::<Vec<_>>()});
        let n = group.len();
        let got = ask_json(llm, job, system, &body.to_string(), 1800, move |t| transcript::parse_chapter_texts(t, n))?;
        for (c, (title, summary)) in group.iter().zip(got) {
            chapters.push(transcript::Chapter { start: c.start, end: c.end, title, summary });
        }
    }
    bn.chapters = chapters;
    Ok(bn)
}

/// Groups of segment indices to translate together: up to 8 lines and about 900 characters.
fn batches(segs: &[Seg], idx: &[usize]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut chars = 0;
    for &i in idx {
        let len = segs[i].text.len();
        match out.last_mut() {
            Some(b) if b.len() < 8 && chars + len <= 900 => {
                b.push(i);
                chars += len;
            }
            _ => {
                out.push(vec![i]);
                chars = len;
            }
        }
    }
    out
}

/// Translates numbered lines; a line that didn't come back right is None.
fn translate_lines(llm: &mut Llm, job: &Job, lines: &[String], to: &str) -> Result<Vec<Option<String>>> {
    let (system, ok): (&str, fn(&str) -> bool) = if to == "bn" {
        ("You translate meeting subtitles from English into Bangla (Bengali script). The user sends numbered lines. Reply with the same numbers, one translated line each, and nothing else. Keep the names of people, product names and numbers as they are. Never merge or skip a line.", |t| bengali_ratio(t) >= 0.5)
    } else {
        ("You translate meeting subtitles from Bangla into English. The user sends numbered lines. Reply with the same numbers, one translated line each, and nothing else. Keep the names of people, product names and numbers as they are. Never merge or skip a line.", |t| bengali_ratio(t) < 0.2)
    };
    let user = lines.iter().enumerate().map(|(i, l)| format!("{}. {}", i + 1, l.replace('\n', " "))).collect::<Vec<_>>().join("\n");
    let g = llm.chat(system, &user, 90 * lines.len() + 120, &job.cancel, &|| {})?;
    {
        let mut stats = lock(&job.llm);
        stats.0 += g.new_tokens;
        stats.1 += g.seconds;
    }
    let mut out: Vec<Option<String>> = vec![None; lines.len()];
    let mut current: Option<usize> = None;
    for line in g.text.lines() {
        let t = line.trim();
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        let rest = t[digits.len()..].strip_prefix(['.', ')']);
        match (digits.parse::<usize>().ok().filter(|n| (1..=lines.len()).contains(n)), rest) {
            (Some(n), Some(rest)) => {
                current = Some(n - 1);
                out[n - 1] = Some(rest.trim().to_string());
            }
            _ if !t.is_empty() => {
                if let Some(Some(prev)) = current.map(|c| &mut out[c]) {
                    prev.push(' ');
                    prev.push_str(t);
                }
            }
            _ => {}
        }
    }
    Ok(out.into_iter().map(|t| t.filter(|t| !t.is_empty() && ok(t))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> Seg {
        Seg { speaker: "A".into(), track: "t".into(), start: 0.0, end: 1.0, lang: "en".into(), text: text.into(), en: None, bn: None }
    }

    #[test]
    fn file_names_sit_beside_the_recording() {
        let p = paths(Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.webm"));
        assert_eq!(p.tracks, Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.tracks"));
        assert_eq!(p.transcript, Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.transcript.json"));
        assert_eq!(p.en_vtt, Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.en.vtt"));
        assert_eq!(p.bn_vtt, Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.bn.vtt"));
        assert_eq!(p.notes_md, Path::new("/v/Zazabor Meetings/Sync 2026-11-21 14.30.notes.md"));
        assert_eq!(extra_files(Path::new("/v/a.webm")).len(), 7);
    }

    #[test]
    fn whisper_translation_is_matched_to_the_bangla_lines_by_time() {
        let mut segs = vec![
            RawSeg { start: 0.0, end: 4.0, lang: "bn".into(), text: "আমি".into(), en: None },
            RawSeg { start: 5.0, end: 7.0, lang: "en".into(), text: "Yes.".into(), en: None },
            RawSeg { start: 8.0, end: 10.0, lang: "bn".into(), text: "ঠিক".into(), en: None },
        ];
        let t = |s: f64, e: f64, x: &str| whisper::Segment { lang: "bn".into(), start: s, end: e, text: x.into(), words: vec![] };
        attach_translation(&mut segs, &[t(0.2, 1.8, " I will"), t(2.0, 3.8, "do it."), t(5.0, 7.0, "Yes.")]);
        assert_eq!(segs[0].en.as_deref(), Some("I will do it."));
        assert_eq!(segs[1].en, None, "English lines are left alone");
        assert_eq!(segs[2].en, None, "nothing matched, so the LLM will do it");
    }

    #[test]
    fn translation_batches_are_small() {
        let segs: Vec<Seg> = (0..20).map(|_| seg(&"x".repeat(200))).collect();
        let idx: Vec<usize> = (0..20).collect();
        let b = batches(&segs, &idx);
        assert!(b.iter().all(|g| g.len() <= 4 && !g.is_empty()), "{b:?}");
        assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 20);
        let short: Vec<Seg> = (0..20).map(|_| seg("hi")).collect();
        assert_eq!(batches(&short, &idx).iter().map(Vec::len).collect::<Vec<_>>(), [8, 8, 4]);
    }

    #[test]
    fn bangla_is_told_from_english_by_script() {
        assert!(bengali_ratio("আমি কাল ডিজাইন শেষ করব") > 0.9);
        assert!(bengali_ratio("I will finish the design") < 0.1);
        assert_eq!(bengali_ratio("123 !!"), 0.0);
    }

    #[test]
    fn models_are_planned_from_what_is_installed() {
        let dir = std::env::temp_dir().join(format!("zazabor-meet-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Settings::default();
        assert_eq!(plan_models(&dir, &s, false).unwrap_err(), "models:small,qwen");
        // The notes model alone is missing when asked for notes only and the transcript exists.
        assert_eq!(plan_models(&dir, &s, true).unwrap_err(), "models:qwen");
        let mut medium = Settings { model: "medium".into(), ..Default::default() };
        assert_eq!(plan_models(&dir, &medium, false).unwrap_err(), "models:medium,qwen");
        medium.model = "small".into();
        assert!(plan_models(&dir, &medium, false).unwrap_err().starts_with("models:small"));
    }

    #[test]
    fn settings_round_trip_and_refuse_nonsense() {
        let dir = std::env::temp_dir().join(format!("zazabor-meet-set-{}", std::process::id()));
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl super::super::Host for Quiet {
            fn emit(&self, _: &str, _: Value) {}
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        assert_eq!(hub.ai_settings(), Settings::default());
        let s = Settings { auto: false, model: "medium".into(), notes_lang: "both".into(), bn_subs: false };
        // While Bangla is off, Bangla notes aren't offered: what's read back is English only.
        let expect = if BANGLA_READY { s.clone() } else { Settings { notes_lang: "en".into(), ..s.clone() } };
        assert_eq!(hub.set_ai_settings(&s).unwrap(), expect);
        assert!(hub.set_ai_settings(&Settings { model: "huge".into(), ..s.clone() }).is_err());
        assert_eq!(hub.ai_settings(), expect);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole job on two short speaker tracks, with the real models. Needs `ZAZABOR_TEST_MODELS`
    /// (a folder with ggml-small.bin, ggml-medium.bin and the Qwen GGUF) and `ZAZABOR_TEST_CLIPS`
    /// (en1.wav and bn1.wav, at most 10 s each); prints the results to look at.
    /// `cargo test --offline --lib meet::ai::tests::the_whole_job -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_whole_job_on_two_short_tracks() {
        let (Some(models_dir), Some(clips)) = (std::env::var_os("ZAZABOR_TEST_MODELS"), std::env::var_os("ZAZABOR_TEST_CLIPS")) else { panic!("set ZAZABOR_TEST_MODELS and ZAZABOR_TEST_CLIPS") };
        std::env::set_var("ZAZABOR_AI_LOAD_LIMIT", "1000");
        let (models_dir, clips) = (PathBuf::from(models_dir), PathBuf::from(clips));
        let dir = std::env::temp_dir().join(format!("zazabor-meet-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (sub, files) in [("videoedit/models", vec!["ggml-small.bin", "ggml-medium.bin"]), ("ai/models", vec!["qwen2.5-3b-instruct-q4_k_m.gguf"])] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            for f in files {
                std::os::unix::fs::symlink(models_dir.join(f), dir.join(sub).join(f)).unwrap();
            }
        }
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl super::super::Host for Quiet {
            fn emit(&self, e: &str, v: Value) {
                if e == "meet-ai" {
                    eprintln!("[event] {v}");
                }
            }
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        let rec_dir = dir.join("rec");
        let path = rec_dir.join("E2E 2026-11-22 10.00.webm");
        std::fs::create_dir_all(paths(&path).tracks).unwrap();
        std::fs::copy(clips.join("en1.wav"), &path).unwrap();
        let tracks = paths(&path).tracks;
        std::fs::copy(clips.join("en1.wav"), tracks.join("1-Ayesha.webm")).unwrap();
        std::fs::write(tracks.join("1-Ayesha.json"), r#"{"name":"Ayesha","offsetMs":0}"#).unwrap();
        std::fs::copy(clips.join("bn1.wav"), tracks.join("2-Tanvir.webm")).unwrap();
        std::fs::write(tracks.join("2-Tanvir.json"), r#"{"name":"Tanvir","offsetMs":12000}"#).unwrap();
        let rec = Recording { id: "e2e".into(), meeting_id: "m".into(), title: "E2E".into(), path: path.to_string_lossy().into_owned(), started_at: 0, ended_at: Some(25_000), bytes: 1 };
        hub.with_db(|c| store::add_recording(c, &rec)).unwrap();
        let mut settings = Settings { notes_lang: "both".into(), ..Default::default() };
        if let Ok(m) = std::env::var("ZAZABOR_TEST_MODEL") {
            settings.model = m;
        }
        hub.set_ai_settings(&settings).unwrap();
        let job = Job::new();
        let started = std::time::Instant::now();
        let msg = hub.run_job(&rec, &job, &RunOptions::default()).unwrap();
        println!("job: {msg} in {:.0} s", started.elapsed().as_secs_f64());
        let p = paths(&path);
        for f in [&p.transcript, &p.vtt, &p.en_vtt, &p.bn_vtt, &p.notes_json, &p.notes_md] {
            println!("==== {}\n{}", f.file_name().unwrap().to_string_lossy(), std::fs::read_to_string(f).unwrap());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Speed and Bangla quality of the models on the clips (see the test above for the variables).
    #[test]
    #[ignore]
    fn bench_models() {
        let models_dir = PathBuf::from(std::env::var_os("ZAZABOR_TEST_MODELS").expect("ZAZABOR_TEST_MODELS"));
        let clips = PathBuf::from(std::env::var_os("ZAZABOR_TEST_CLIPS").expect("ZAZABOR_TEST_CLIPS"));
        let cache = std::env::temp_dir();
        let only = std::env::var("ZAZABOR_TEST_CLIP").ok();
        for clip in ["bn1.wav", "en1.wav"].into_iter().filter(|c| only.as_deref().is_none_or(|o| o == *c)) {
            let bytes = crate::videoedit::audio::pcm(&clips.join(clip).to_string_lossy(), &cache, true).unwrap();
            let pcm = mel::from_editor_pcm(&bytes);
            for model in ["ggml-small.bin", "ggml-medium.bin"].into_iter().filter(|m| std::env::var("ZAZABOR_TEST_WHISPER").map_or(true, |w| m.contains(&w))) {
                for (lang, translate) in [(Some("bn"), false), (None, false), (Some("bn"), true)] {
                    if clip == "en1.wav" && (lang.is_some() || translate) {
                        continue;
                    }
                    let clock = std::time::Instant::now();
                    let r = crate::ai::sys::pool().install(|| whisper::transcribe_with(&models_dir.join(model), &pcm, &Options { language: lang, translate, per_window: lang.is_none(), candidates: &LANGS }, &|_| {}, &AtomicBool::new(false))).unwrap();
                    let text: Vec<String> = r.segments.iter().map(|s| format!("[{}] {}", s.lang, s.text)).collect();
                    println!("{clip} {model} lang={lang:?} translate={translate}: {:.1} s for {:.0} s of sound -> {}", clock.elapsed().as_secs_f64(), pcm.len() as f64 / 16000.0, text.join(" | "));
                }
            }
        }
        whisper::unload();
        if let Some(q) = std::env::var_os("ZAZABOR_TEST_LLM") {
            let _ = q;
            let path = models_dir.join("qwen2.5-3b-instruct-q4_k_m.gguf");
            llm::with(&path, |m| {
                let cancel = AtomicBool::new(false);
                let g = m.chat("You translate English into Bangla.", "Translate: We will ship the new version on Friday.", 60, &cancel, &|| {})?;
                println!("llm: {} prompt tokens, {} new tokens in {:.1} s ({:.2} tokens/s): {}", g.prompt_tokens, g.new_tokens, g.seconds, g.new_tokens as f64 / g.seconds, g.text);
                Ok(())
            })
            .unwrap();
        }
    }
}
