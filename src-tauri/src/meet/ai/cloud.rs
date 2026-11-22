//! The cloud way to do a meeting's job: speech to text, Bangla/English subtitles and notes through
//! the user's own API keys (`crate::ai::cloud`), used only when Settings → Meet says Cloud and the
//! user confirmed what is sent. The shape is the local job's (tracks, merge, subtitles, map-reduce
//! notes); what differs is who does the listening and writing, and that Bangla is on, since cloud
//! models handle it. Transcript text and audio are never logged: only counts and times.

use serde::Serialize;

use super::*;
use crate::ai::cloud::{self as api, Client, CloudError};
use crate::meet::secret;

/// Lines per translation request, and the characters in them (a cloud model takes far more than
/// the local one).
const LINES: usize = 30;
const LINE_CHARS: usize = 4000;

/// What the page knows of a provider: never the key, only whether there is one.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    pub id: &'static str,
    pub label: &'static str,
    pub name: &'static str,
    pub can_transcribe: bool,
    pub key_set: bool,
    pub key_page: &'static str,
    pub stt_default: Option<&'static str>,
    pub llm_default: &'static str,
}

/// Where the providers are reached: their own addresses, except in tests, which point at a local
/// stand-in (never the real APIs).
fn base_override() -> Option<String> {
    #[cfg(test)]
    {
        TEST_BASE.with(|b| b.borrow().clone())
    }
    #[cfg(not(test))]
    {
        None
    }
}

#[cfg(test)]
thread_local! {
    static TEST_BASE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

impl Hub {
    pub fn cloud_state(&self) -> Vec<ProviderState> {
        api::ALL
            .into_iter()
            .map(|p| ProviderState {
                id: p.id(),
                label: p.label(),
                name: p.name(),
                can_transcribe: p.can_transcribe(),
                key_set: secret::get_named(&self.data_dir, &secret::cloud_key(p.id())).is_some(),
                key_page: p.key_page(),
                stt_default: p.default_stt_model(),
                llm_default: p.default_llm_model(),
            })
            .collect()
    }

    pub fn cloud_key_set(&self, provider: &str, key: &str) -> Result<Vec<ProviderState>> {
        let p = Provider::from_id(provider).ok_or("That isn't one of the providers.")?;
        let key = key.trim();
        if key.is_empty() || key.len() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("That doesn't look like an API key: it has no spaces and is one line.".into());
        }
        secret::set_named(&self.data_dir, &secret::cloud_key(p.id()), key)?;
        Ok(self.cloud_state())
    }

    pub fn cloud_key_clear(&self, provider: &str) -> Result<Vec<ProviderState>> {
        let p = Provider::from_id(provider).ok_or("That isn't one of the providers.")?;
        secret::clear_named(&self.data_dir, &secret::cloud_key(p.id()));
        Ok(self.cloud_state())
    }

    fn cloud_key(&self, p: Provider) -> Result<String> {
        secret::get_named(&self.data_dir, &secret::cloud_key(p.id())).ok_or_else(|| format!("Add your {} API key in Settings → Meet first.", p.name()))
    }

    /// A client for the "Test key" button: the provider's notes model, with the saved key.
    pub fn cloud_test_client(&self, provider: &str) -> Result<(Client, String)> {
        let p = Provider::from_id(provider).ok_or("That isn't one of the providers.")?;
        let model = self.ai_settings().model_for(p, Task::Llm);
        let client = Client::with_base(p, self.cloud_key(p)?, model.clone(), base_override())?;
        Ok((client, model))
    }

    /// The cloud job. Returns the message for the card.
    pub(super) fn run_cloud_job(&self, r: &Recording, job: &Arc<Job>, s: &Settings, redo_notes: bool) -> Result<String> {
        let rec_path = PathBuf::from(&r.path);
        let p = paths(&rec_path);
        let id = r.id.as_str();
        let llm_p = s.llm().ok_or("Choose a provider for translation and notes in Settings → Meet.")?;
        let llm_model = s.model_for(llm_p, Task::Llm);
        let llm = Client::with_base(llm_p, self.cloud_key(llm_p)?, llm_model.clone(), base_override())?;
        let again = redo_notes && p.transcript.exists();

        let mut doc = if again {
            self.transcript(id)?
        } else {
            let stt_p = s.stt().ok_or("Choose a provider for speech to text in Settings → Meet.")?;
            let stt_model = s.model_for(stt_p, Task::Stt);
            let stt = Client::with_base(stt_p, self.cloud_key(stt_p)?, stt_model.clone(), base_override())?;
            self.step(id, job, "transcribe", 0.0, &format!("Sending the sound to {}…", stt_p.name()));
            let doc = tauri::async_runtime::block_on(self.transcribe_cloud(r, job, &stt, &stt_model))?;
            write_subtitles(&p, &doc)?;
            doc
        };
        self.changed();

        let mut noted = String::new();
        tauri::async_runtime::block_on(async {
            if !again {
                self.translate_cloud(&llm, job, id, &mut doc, s).await?;
                write_subtitles(&p, &doc)?;
            }
            if !doc.segments.is_empty() {
                let (notes_en, notes_bn) = self.notes_cloud(&llm, job, id, &doc, s).await?;
                let out = json!({
                    "version": 1, "language": s.notes_lang, "model": llm_model, "provider": llm_p.id(), "generatedAt": now_ms(), "edited": false,
                    "en": notes_en, "bn": notes_bn,
                });
                std::fs::write(&p.notes_json, serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                std::fs::write(&p.notes_md, notes_md_text(&r.title, &out)).map_err(|e| e.to_string())?;
                if out["bn"].is_null() && s.notes_lang != "en" {
                    noted = " The Bangla notes couldn't be written.".into();
                }
            }
            Ok::<(), String>(())
        })?;
        let n = doc.segments.len();
        Ok(if n == 0 { "No speech was found.".to_string() } else { format!("{n} lines from {} min of talk.{noted}", minutes(doc.duration)) })
    }

    async fn transcribe_cloud(&self, r: &Recording, job: &Arc<Job>, stt: &Client, model: &str) -> Result<Doc> {
        let rec_path = PathBuf::from(&r.path);
        let mut tracks = list_tracks(&rec_path);
        if tracks.is_empty() {
            // An older recording, or one without tracks: the mixed sound is one speaker.
            tracks.push(TrackInfo { stem: "mixed".into(), name: "Meeting".into(), offset: 0.0, path: rec_path.clone() });
        }
        let total_secs = (r.ended_at.unwrap_or(r.started_at) - r.started_at).max(1000) as f64 / 1000.0;
        let est: Vec<f64> = tracks.iter().map(|t| (total_secs - t.offset).max(1.0)).collect();
        let total_est: f64 = est.iter().sum();
        let cache = std::env::temp_dir().join(format!("zazabor-meet-ai-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cache);
        let id = r.id.as_str();
        let who = stt.provider().name();
        let mut heard = Vec::new();
        let (mut done_secs, mut duration) = (0.0, 0f64);
        for (i, t) in tracks.iter().enumerate() {
            if job.cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            self.step(id, job, "transcribe", done_secs / total_est * 0.6, &format!("Sending {} to {who}…", api::minutes_text(est[i])));
            let bytes = crate::videoedit::audio::pcm(&t.path.to_string_lossy(), &cache, true)?;
            let pcm = mel::from_editor_pcm(&bytes);
            drop(bytes);
            duration = duration.max(t.offset + pcm.len() as f64 / mel::SAMPLE_RATE as f64);
            if api::is_silent(&pcm) {
                // A muted microphone: nothing to send.
                done_secs += est[i];
                continue;
            }
            let progress = |f: f64| {
                let at = done_secs + f * est[i];
                self.step(id, job, "transcribe", at / total_est * 0.6, &format!("Transcribing {} of {} min ({who})…", minutes(at), minutes(total_est)));
            };
            let clock = std::time::Instant::now();
            let segs = stt.transcribe(&pcm, &["en", "bn"], &progress, &job.cancel).await?;
            eprintln!("[meet-ai] track {}: {:.0} s of sound, {} lines, {:.1} s ({who})", i + 1, pcm.len() as f64 / mel::SAMPLE_RATE as f64, segs.len(), clock.elapsed().as_secs_f64());
            done_secs += est[i];
            let segments = segs.into_iter().map(|s| RawSeg { start: s.start, end: s.end, lang: s.lang, text: s.text, en: None }).collect();
            heard.push(Heard { speaker: t.name.clone(), track: t.stem.clone(), offset: t.offset, segments });
        }
        let _ = std::fs::remove_dir_all(&cache);
        let segments = transcript::merge(heard);
        let language = transcript::dominant_language(&segments);
        Ok(Doc { version: 1, model: format!("{}:{model}", stt.provider().id()), language, duration, segments })
    }

    /// English for Bangla lines, and Bangla for English lines when asked, by the notes model.
    async fn translate_cloud(&self, llm: &Client, job: &Arc<Job>, id: &str, doc: &mut Doc, s: &Settings) -> Result<()> {
        let need_en: Vec<usize> = (0..doc.segments.len()).filter(|&i| doc.segments[i].lang == "bn" && doc.segments[i].english().is_none()).collect();
        let need_bn: Vec<usize> = if s.bn_subs { (0..doc.segments.len()).filter(|&i| doc.segments[i].lang != "bn" && doc.segments[i].bangla().is_none()).collect() } else { Vec::new() };
        let total = (need_en.len() + need_bn.len()).max(1);
        let mut done = 0usize;
        for (idx, to) in [(need_en, "en"), (need_bn, "bn")] {
            for batch in batches_of(&doc.segments, &idx, LINES, LINE_CHARS) {
                self.step(id, job, "translate", 0.6 + 0.15 * done as f64 / total as f64, &format!("Translating subtitles… {done} of {total}"));
                let lines: Vec<String> = batch.iter().map(|&i| if to == "bn" { doc.segments[i].english().unwrap_or(&doc.segments[i].text).to_string() } else { doc.segments[i].text.clone() }).collect();
                let got = translate_lines(llm, &lines, to, &job.cancel).await?;
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
    async fn notes_cloud(&self, llm: &Client, job: &Arc<Job>, id: &str, doc: &Doc, s: &Settings) -> Result<(Notes, Option<Notes>)> {
        let line_of = |g: &Seg| format!("[{}] {}: {}\n", transcript::clock(g.start), g.speaker, g.english().unwrap_or(&g.text));
        let ranges = transcript::chapters(&doc.segments, transcript::CUT, |g| line_of(g).len());
        let mut parts: Vec<(String, Notes)> = Vec::new();
        let mut chapters = Vec::new();
        for (k, r) in ranges.iter().enumerate() {
            self.step(id, job, "notes", 0.75 + 0.2 * k as f64 / ranges.len().max(1) as f64, &format!("Writing notes… part {} of {}", k + 1, ranges.len()));
            let text: String = doc.segments[r.clone()].iter().map(&line_of).collect();
            let (title, notes) = ask_notes(llm, MAP_SYSTEM, &format!("Transcript:\n{text}"), &map_schema(), &job.cancel, transcript::parse_chapter).await?;
            chapters.push(transcript::Chapter { start: doc.segments[r.start].start, end: doc.segments[r.end - 1].end, title: title.clone(), summary: notes.summary.clone() });
            parts.push((title, notes));
        }
        self.step(id, job, "notes", 0.95, "Writing notes… the summary");
        let mut overall = reduce_cloud(llm, &job.cancel, parts).await?;
        overall.chapters = chapters;
        let bn = if s.notes_lang == "en" {
            None
        } else {
            self.step(id, job, "notes", 0.96, "Writing the Bangla notes…");
            translate_notes_cloud(llm, &job.cancel, &overall).await.ok()
        };
        Ok((overall, bn))
    }
}

// ---- schemas (strict: every property required, no extras, so OpenAI's and Claude's modes take them)

fn text_list(what: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": what })
}

fn actions_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object", "additionalProperties": false, "required": ["owner", "task", "due"],
            "properties": {
                "owner": { "type": "string", "description": "The person who will do it, named as in the transcript" },
                "task": { "type": "string" },
                "due": { "anyOf": [{ "type": "string" }, { "type": "null" }], "description": "When it was said, or null" }
            }
        }
    })
}

/// One chapter's notes.
pub fn map_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["title", "summary", "decisions", "actions", "questions"],
        "properties": {
            "title": { "type": "string", "description": "3 to 6 words" },
            "summary": { "type": "string", "description": "2 or 3 sentences" },
            "decisions": text_list("What was decided"),
            "actions": actions_schema(),
            "questions": text_list("Open questions")
        }
    })
}

/// The notes of the whole meeting (and the Bangla copy of them).
pub fn overall_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["summary", "decisions", "actions", "questions"],
        "properties": {
            "summary": { "type": "string", "description": "One short paragraph" },
            "decisions": text_list("What was decided"),
            "actions": actions_schema(),
            "questions": text_list("Open questions")
        }
    })
}

fn translated_schema() -> Value {
    let mut s = overall_schema();
    s["required"] = json!(["summary", "decisions", "actions", "questions", "chapters"]);
    s["properties"]["chapters"] = json!({
        "type": "array",
        "items": { "type": "object", "additionalProperties": false, "required": ["title", "summary"], "properties": { "title": { "type": "string" }, "summary": { "type": "string" } } }
    });
    s
}

fn lines_schema() -> Value {
    json!({ "type": "object", "additionalProperties": false, "required": ["lines"], "properties": { "lines": { "type": "array", "items": { "type": "string" } } } })
}

// ---- asking

/// One answer, parsed and checked; if the check fails the model is asked again once, told why.
async fn ask_notes<T>(llm: &Client, system: &str, user: &str, schema: &Value, cancel: &AtomicBool, parse: impl Fn(&str) -> std::result::Result<T, String>) -> Result<T> {
    let first = llm.complete_json(system, user, schema, cancel).await?;
    match parse(&first.to_string()) {
        Ok(v) => Ok(v),
        Err(why) => {
            eprintln!("[meet-ai] an answer of {} characters was refused; asking again", first.to_string().chars().count());
            let again = llm.complete_json(system, &format!("{user}\n\nYour previous answer was rejected because {why}. Answer again."), schema, cancel).await?;
            parse(&again.to_string()).map_err(|e| format!("The notes model's answer wasn't usable: {e}"))
        }
    }
}

async fn reduce_cloud(llm: &Client, cancel: &AtomicBool, parts: Vec<(String, Notes)>) -> Result<Notes> {
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
            next.push(ask_notes(llm, REDUCE_SYSTEM, &user, &overall_schema(), cancel, transcript::parse_overall).await?);
        }
        if next.len() == 1 {
            return Ok(next.remove(0));
        }
        level = next;
    }
}

async fn translate_notes_cloud(llm: &Client, cancel: &AtomicBool, en: &Notes) -> Result<Notes> {
    let system = "You translate meeting notes from English into Bangla (Bengali script). The user sends a JSON object. Reply with the same JSON, same keys and same structure, with every text value translated. Keep the names of people, numbers and dates as they are.";
    let (nd, na, nq, nc) = (en.decisions.len(), en.actions.len(), en.questions.len(), en.chapters.len());
    let body = json!({
        "summary": en.summary, "decisions": en.decisions, "actions": en.actions, "questions": en.questions,
        "chapters": en.chapters.iter().map(|c| json!({"title": c.title, "summary": c.summary})).collect::<Vec<_>>(),
    });
    let check = move |text: &str| -> std::result::Result<(Notes, Vec<(String, String)>), String> {
        let n = transcript::parse_overall(text)?;
        if (n.decisions.len(), n.actions.len(), n.questions.len()) != (nd, na, nq) {
            return Err("the lists must have the same number of items as the original".into());
        }
        if api::script_lang(&n.summary) != "bn" {
            return Err("the summary is not in Bangla script".into());
        }
        let chapters = transcript::parse_chapter_texts(text, nc)?;
        Ok((n, chapters))
    };
    let (mut bn, chapters) = ask_notes(llm, system, &body.to_string(), &translated_schema(), cancel, check).await?;
    // The owners are names: keep them as they were.
    for (b, e) in bn.actions.iter_mut().zip(&en.actions) {
        b.owner = e.owner.clone();
    }
    bn.chapters = en.chapters.iter().zip(chapters).map(|(c, (title, summary))| transcript::Chapter { start: c.start, end: c.end, title, summary }).collect();
    Ok(bn)
}

/// Whether a translated line is in the language asked for (a line with no letters, like a number,
/// is fine either way).
fn in_language(line: &str, to: &str) -> bool {
    !line.chars().any(char::is_alphabetic) || api::script_lang(line) == to
}

/// One request for the lines of a batch; None if the answer wasn't one line back for each.
async fn translate_batch(llm: &Client, lines: &[String], to: &str, cancel: &AtomicBool) -> std::result::Result<Option<Vec<String>>, CloudError> {
    let (from, into) = if to == "bn" { ("English", "Bangla (Bengali script)") } else { ("Bangla", "English") };
    let system = format!("You translate meeting subtitles from {from} into {into}. The user sends a JSON object {{\"lines\": [...]}}. Reply with a JSON object {{\"lines\": [...]}} holding the translation of each line, in the same order and the same number. Keep the names of people, product names and numbers as they are. Never merge, split or skip a line.");
    let answer = match llm.complete_json(&system, &json!({ "lines": lines }).to_string(), &lines_schema(), cancel).await {
        Ok(v) => v,
        // A single unusable or declined answer costs those lines their translation, not the job.
        Err(CloudError::Bad(..) | CloudError::Refused(..) | CloudError::TooLong(..)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(got) = answer["lines"].as_array() else { return Ok(None) };
    let got: Vec<String> = got.iter().filter_map(|l| l.as_str().map(|t| t.trim().to_string())).collect();
    Ok((got.len() == lines.len() && got.iter().zip(lines).all(|(g, l)| !g.is_empty() && (l.trim().is_empty() || in_language(g, to)))).then_some(got))
}

/// Translates lines; a line that didn't come back right is None. A batch that came back wrong is
/// asked again one line at a time.
async fn translate_lines(llm: &Client, lines: &[String], to: &str, cancel: &AtomicBool) -> Result<Vec<Option<String>>> {
    if let Some(all) = translate_batch(llm, lines, to, cancel).await? {
        return Ok(all.into_iter().map(Some).collect());
    }
    if lines.len() == 1 {
        return Ok(vec![None]);
    }
    let mut out = Vec::new();
    for line in lines {
        out.push(translate_batch(llm, std::slice::from_ref(line), to, cancel).await?.and_then(|mut v| v.pop()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::cloud::testing::{ok, run, stand_in};

    fn seg(text: &str, lang: &str) -> Seg {
        Seg { speaker: "A".into(), track: "t".into(), start: 0.0, end: 1.0, lang: lang.into(), text: text.into(), en: None, bn: None }
    }

    /// A stand-in for OpenAI's chat endpoint that answers with `content`.
    fn chat(content: &Value) -> String {
        ok(&json!({"choices": [{"message": {"content": content.to_string()}, "finish_reason": "stop"}]}))
    }

    fn llm(base: String) -> Client {
        Client::with_base(Provider::OpenAi, "sk-test-key-123456".into(), "gpt-6.2-sol".into(), Some(base)).unwrap()
    }

    #[test]
    fn bangla_is_available_only_where_something_can_do_it() {
        // This PC alone: hidden while the local models are poor at Bangla.
        assert!(!bangla_gate(false, false));
        // Cloud speech to text turns it on; a ready local engine always has it.
        assert!(bangla_gate(false, true));
        assert!(bangla_gate(true, false));
        let available = |s: &Settings| bangla_gate(BANGLA_READY, s.cloud_on());
        assert_eq!(available(&Settings::default()), BANGLA_READY);
        let consented = Settings { processing: "cloud".into(), cloud_consent: true, ..Default::default() };
        assert!(available(&consented));
        // Cloud chosen but not confirmed: nothing is sent, so nothing changes.
        let unconfirmed = Settings { processing: "cloud".into(), cloud_consent: false, ..Default::default() };
        assert!(!unconfirmed.cloud_on());
        assert_eq!(available(&unconfirmed), BANGLA_READY);
    }

    #[test]
    fn models_default_per_provider_and_the_users_choice_wins() {
        let mut s = Settings::default();
        assert_eq!(s.model_for(Provider::Claude, Task::Llm), "claude-opus-5-5");
        assert_eq!(s.model_for(Provider::OpenAi, Task::Stt), "gpt-transcribe");
        assert_eq!(s.model_for(Provider::Gemini, Task::Stt), "gemini-3.5-transcribe");
        s.cloud_models.insert("openai.stt".into(), "whisper-1".into());
        s.cloud_models.insert("claude.llm".into(), "bad name!".into());
        assert_eq!(s.model_for(Provider::OpenAi, Task::Stt), "whisper-1");
        assert_eq!(s.model_for(Provider::OpenAi, Task::Llm), "gpt-6.2-sol", "the other task keeps its default");
        assert_eq!(s.model_for(Provider::Claude, Task::Llm), "claude-opus-5-5", "an invalid name falls back");
        assert_eq!(s.stt(), Some(Provider::OpenAi));
        s.stt_provider = "claude".into();
        assert_eq!(s.stt(), None, "Claude can't be the speech-to-text provider");
    }

    #[test]
    fn schemas_are_strict_enough_for_every_provider() {
        fn strict(v: &Value) {
            if v["type"] == "object" {
                assert_eq!(v["additionalProperties"], false, "{v}");
                let props: Vec<&str> = v["properties"].as_object().unwrap().keys().map(String::as_str).collect();
                let mut req: Vec<&str> = v["required"].as_array().unwrap().iter().map(|r| r.as_str().unwrap()).collect();
                let mut all = props.clone();
                all.sort();
                req.sort();
                assert_eq!(all, req, "every property is required");
                v["properties"].as_object().unwrap().values().for_each(strict);
            }
            if v["type"] == "array" {
                strict(&v["items"]);
            }
        }
        for s in [map_schema(), overall_schema(), translated_schema(), lines_schema(), api::segments_schema()] {
            strict(&s);
        }
    }

    #[test]
    fn a_chapter_answer_passes_the_same_checks_as_the_local_models() {
        let answer = json!({"title": "Launch plan", "summary": "They plan the launch.", "decisions": ["Ship on Friday"], "actions": [{"owner": "Ayesha", "task": "Test it", "due": null}], "questions": []});
        let (base, _) = stand_in(vec![chat(&answer)]);
        let c = llm(base);
        let got = run(ask_notes(&c, MAP_SYSTEM, "Transcript:\n", &map_schema(), &AtomicBool::new(false), transcript::parse_chapter)).unwrap();
        assert_eq!(got.0, "Launch plan");
        assert_eq!(got.1.actions[0].owner, "Ayesha");
        // An empty summary is refused, the model is told why, and the second answer is used.
        let bad = json!({"title": "T", "summary": "", "decisions": [], "actions": [], "questions": []});
        let (base, seen) = stand_in(vec![chat(&bad), chat(&answer)]);
        let c = llm(base);
        assert!(run(ask_notes(&c, MAP_SYSTEM, "Transcript:\n", &map_schema(), &AtomicBool::new(false), transcript::parse_chapter)).is_ok());
        let second = String::from_utf8_lossy(&seen.lock().unwrap()[1].1).into_owned();
        assert!(second.contains("rejected because"), "{second}");
    }

    #[test]
    fn translations_come_back_line_for_line_or_one_by_one() {
        let lines = vec!["We ship on Friday.".to_string(), "Ayesha will test it.".to_string()];
        let (base, _) = stand_in(vec![chat(&json!({"lines": ["আমরা শুক্রবার পাঠাব।", "আয়েশা এটা পরীক্ষা করবে।"]}))]);
        let got = run(translate_lines(&llm(base), &lines, "bn", &AtomicBool::new(false))).unwrap();
        assert_eq!(got, vec![Some("আমরা শুক্রবার পাঠাব।".to_string()), Some("আয়েশা এটা পরীক্ষা করবে।".to_string())]);
        // The wrong count: the lines are asked for one at a time, and one that is still English is dropped.
        let (base, seen) = stand_in(vec![chat(&json!({"lines": ["only one"]})), chat(&json!({"lines": ["আমরা শুক্রবার পাঠাব।"]})), chat(&json!({"lines": ["Ayesha will test it."]}))]);
        let got = run(translate_lines(&llm(base), &lines, "bn", &AtomicBool::new(false))).unwrap();
        assert_eq!(got, vec![Some("আমরা শুক্রবার পাঠাব।".to_string()), None]);
        assert_eq!(seen.lock().unwrap().len(), 3);
        // Into English, a line that stayed Bangla is dropped.
        let (base, _) = stand_in(vec![chat(&json!({"lines": ["I will do it.", "ঠিক আছে"]})), chat(&json!({"lines": ["I will do it."]})), chat(&json!({"lines": ["ঠিক আছে"]}))]);
        let got = run(translate_lines(&llm(base), &["আমি করব".to_string(), "ঠিক আছে".to_string()], "en", &AtomicBool::new(false))).unwrap();
        assert_eq!(got, vec![Some("I will do it.".to_string()), None]);
        assert!(in_language("123", "bn") && in_language("123", "en"));
    }

    #[test]
    fn the_bangla_notes_keep_the_owners_and_are_checked_for_script() {
        let en = Notes {
            summary: "They plan the launch.".into(),
            decisions: vec!["Ship on Friday".into()],
            actions: vec![transcript::Action { owner: "Ayesha".into(), task: "Test it".into(), due: Some("Thursday".into()) }],
            questions: vec![],
            chapters: vec![transcript::Chapter { start: 0.0, end: 60.0, title: "Launch".into(), summary: "The plan.".into() }],
        };
        let bn = json!({"summary": "তারা লঞ্চের পরিকল্পনা করছে।", "decisions": ["শুক্রবার পাঠানো হবে"], "actions": [{"owner": "আয়েশা", "task": "এটা পরীক্ষা করা", "due": "বৃহস্পতিবার"}], "questions": [],
            "chapters": [{"title": "লঞ্চ", "summary": "পরিকল্পনা।"}]});
        let (base, _) = stand_in(vec![chat(&bn)]);
        let got = run(translate_notes_cloud(&llm(base), &AtomicBool::new(false), &en)).unwrap();
        assert_eq!(got.actions[0].owner, "Ayesha", "names are kept as they were");
        assert_eq!(got.chapters.len(), 1);
        assert_eq!((got.chapters[0].start, got.chapters[0].end), (0.0, 60.0));
        assert!(api::script_lang(&got.summary) == "bn");
        // English back: refused twice, so no Bangla notes.
        let english = json!({"summary": "They plan.", "decisions": ["Ship"], "actions": [{"owner": "A", "task": "T", "due": null}], "questions": [], "chapters": [{"title": "L", "summary": "P"}]});
        let (base, _) = stand_in(vec![chat(&english), chat(&english)]);
        assert!(run(translate_notes_cloud(&llm(base), &AtomicBool::new(false), &en)).is_err());
    }

    #[test]
    fn keys_are_saved_per_provider_and_the_page_only_learns_whether_one_is_set() {
        std::env::set_var("ZAZABOR_MEET_NO_KEYRING", "1");
        let dir = std::env::temp_dir().join(format!("zazabor-meet-cloudkeys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl crate::meet::Host for Quiet {
            fn emit(&self, _: &str, _: Value) {}
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        assert!(hub.cloud_state().iter().all(|p| !p.key_set));
        let after = hub.cloud_key_set("claude", " sk-ant-test-1234567890 ").unwrap();
        assert!(after.iter().find(|p| p.id == "claude").unwrap().key_set);
        assert!(!after.iter().find(|p| p.id == "openai").unwrap().key_set);
        assert!(hub.cloud_key_set("claude", "two words").is_err());
        assert!(hub.cloud_key_set("claude", "").is_err());
        assert!(hub.cloud_key_set("nope", "abc").is_err());
        // Nothing the page can read holds the key: not the state, the settings, nor the database.
        let shown = format!("{}{}", serde_json::to_string(&hub.cloud_state()).unwrap(), serde_json::to_string(&hub.ai_settings()).unwrap());
        assert!(!shown.contains("sk-ant-test"));
        let db = std::fs::read(dir.join("meet.db")).unwrap();
        assert!(!db.windows(11).any(|w| w == b"sk-ant-test"), "the key isn't in the database");
        assert!(hub.cloud_test_client("openai").is_err(), "no key, no client");
        assert!(hub.cloud_test_client("claude").is_ok());
        assert!(!hub.cloud_key_clear("claude").unwrap().iter().any(|p| p.key_set));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_needs_a_confirmation_and_valid_choices() {
        std::env::set_var("ZAZABOR_MEET_NO_KEYRING", "1");
        let dir = std::env::temp_dir().join(format!("zazabor-meet-cloudset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl crate::meet::Host for Quiet {
            fn emit(&self, _: &str, _: Value) {}
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        assert_eq!(hub.ai_settings().processing, "local", "this PC is the default");
        let cloud = Settings { processing: "cloud".into(), notes_lang: "both".into(), bn_subs: true, ..Default::default() };
        assert!(hub.set_ai_settings(&cloud).is_err(), "unconfirmed");
        let confirmed = Settings { cloud_consent: true, fallback_local: true, stt_provider: "gemini".into(), llm_provider: "openrouter".into(), ..cloud.clone() };
        let saved = hub.set_ai_settings(&confirmed).unwrap();
        assert!(saved.cloud_on() && saved.bangla_on && saved.fallback_local);
        assert_eq!((saved.notes_lang.as_str(), saved.bn_subs), ("both", true), "Bangla notes and subtitles are kept with Cloud");
        assert_eq!((saved.stt_provider.as_str(), saved.llm_provider.as_str()), ("gemini", "openrouter"));
        assert!(hub.set_ai_settings(&Settings { stt_provider: "claude".into(), ..confirmed.clone() }).is_err(), "Claude can't listen");
        assert!(hub.set_ai_settings(&Settings { llm_provider: "bard".into(), ..confirmed.clone() }).is_err());
        assert!(hub.set_ai_settings(&Settings { cloud_models: [("openai.llm".to_string(), "x\"y".to_string())].into(), ..confirmed.clone() }).is_err());
        let with_model = hub.set_ai_settings(&Settings { cloud_models: [("openai.stt".to_string(), " whisper-1 ".to_string()), ("junk".to_string(), "z".to_string())].into(), ..confirmed.clone() }).unwrap();
        assert_eq!(with_model.cloud_models.get("openai.stt").map(String::as_str), Some("whisper-1"));
        assert!(!with_model.cloud_models.contains_key("junk"));
        // Back to this PC: Bangla goes away again (unless the local engine has it).
        let back = hub.set_ai_settings(&Settings { processing: "local".into(), ..confirmed }).unwrap();
        assert!(!back.cloud_on());
        assert_eq!(back.bangla_on, BANGLA_READY);
        if !BANGLA_READY {
            assert_eq!((back.notes_lang.as_str(), back.bn_subs), ("en", false));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cloud_run_says_how_much_audio_it_will_send_and_a_local_one_says_nothing() {
        std::env::set_var("ZAZABOR_MEET_NO_KEYRING", "1");
        let dir = std::env::temp_dir().join(format!("zazabor-meet-cloudinfo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl crate::meet::Host for Quiet {
            fn emit(&self, _: &str, _: Value) {}
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        let rec = Recording { id: "r".into(), meeting_id: "m".into(), title: "T".into(), path: dir.join("rec").join("T.webm").to_string_lossy().into_owned(), started_at: 0, ended_at: Some(10 * 60_000), bytes: 1 };
        let local = hub.info_of(&rec, None, &hub.ai_settings());
        assert_eq!((local.send_minutes, local.cloud.as_str()), (0, ""));
        let s = hub.set_ai_settings(&Settings { processing: "cloud".into(), cloud_consent: true, ..Default::default() }).unwrap();
        let cloud = hub.info_of(&rec, None, &s);
        assert_eq!((cloud.send_minutes, cloud.cloud.as_str()), (10, "OpenAI"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole cloud job on one short track, with a local stand-in for OpenAI's two endpoints:
    /// the sound goes out, the English and Bangla subtitles and both notes come back and are
    /// written beside the recording, and the key is in none of them.
    #[test]
    fn the_whole_cloud_job_writes_the_transcript_subtitles_and_notes() {
        std::env::set_var("ZAZABOR_MEET_NO_KEYRING", "1");
        let dir = std::env::temp_dir().join(format!("zazabor-meet-cloudjob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = store::open(&dir.join("meet.db")).unwrap();
        struct Quiet;
        impl crate::meet::Host for Quiet {
            fn emit(&self, _: &str, _: Value) {}
        }
        let hub = Hub::new(Box::new(Quiet), conn, dir.clone(), dir.join("rec")).unwrap();
        let path = dir.join("rec").join("Cloud 2026-11-22 10.00.webm");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // 8 s of sound with a burst of "speech" in it (a WAV under a .webm name, as the local test does).
        let sound = crate::ai::cloud::testing::audio(8.0, &[2.0]);
        std::fs::write(&path, api::wav16(&sound)).unwrap();
        let rec = Recording { id: "cj".into(), meeting_id: "m".into(), title: "Cloud".into(), path: path.to_string_lossy().into_owned(), started_at: 0, ended_at: Some(8_000), bytes: 1 };
        hub.with_db(|c| store::add_recording(c, &rec)).unwrap();
        let key = "sk-test-SECRET-9876543210";
        hub.cloud_key_set("openai", key).unwrap();
        let settings = hub
            .set_ai_settings(&Settings { processing: "cloud".into(), cloud_consent: true, notes_lang: "both".into(), bn_subs: true, stt_provider: "openai".into(), llm_provider: "openai".into(), ..Default::default() })
            .unwrap();
        assert!(settings.cloud_on() && settings.bangla_on);

        let map = json!({"title": "Launch", "summary": "They plan the launch.", "decisions": ["Ship on Friday"], "actions": [{"owner": "Meeting", "task": "Look at it", "due": null}], "questions": []});
        let bn_notes = json!({"summary": "তারা লঞ্চের পরিকল্পনা করছে।", "decisions": ["শুক্রবার পাঠানো হবে"], "actions": [{"owner": "Meeting", "task": "দেখা", "due": null}], "questions": [], "chapters": [{"title": "লঞ্চ", "summary": "পরিকল্পনা।"}]});
        let (base, seen) = stand_in(vec![
            ok(&json!({"text": "Hello team. আমি দেখব।", "languages": [{"code": "en"}, {"code": "bn"}]})),
            chat(&json!({"lines": ["I will look."]})),
            chat(&json!({"lines": ["হ্যালো টিম।"]})),
            chat(&map),
            chat(&bn_notes),
        ]);
        TEST_BASE.with(|b| *b.borrow_mut() = Some(base));
        let job = Job::new();
        let message = hub.run_job(&rec, &job, &RunOptions::default()).unwrap();
        TEST_BASE.with(|b| *b.borrow_mut() = None);
        assert!(message.starts_with("2 lines"), "{message}");

        let p = paths(&path);
        let doc = hub.transcript("cj").unwrap();
        assert_eq!(doc.segments.len(), 2);
        assert_eq!((doc.segments[0].lang.as_str(), doc.segments[1].lang.as_str()), ("en", "bn"));
        assert_eq!(doc.segments[1].en.as_deref(), Some("I will look."), "the Bangla line got its English");
        assert_eq!(doc.segments[0].bn.as_deref(), Some("হ্যালো টিম।"), "the English line got its Bangla");
        assert!(doc.model.starts_with("openai:gpt-transcribe"));
        assert!(std::fs::read_to_string(&p.bn_vtt).unwrap().contains("হ্যালো টিম।"));
        assert!(std::fs::read_to_string(&p.en_vtt).unwrap().contains("I will look."));
        let notes = hub.notes("cj").unwrap().unwrap();
        assert_eq!(notes["provider"], "openai");
        assert_eq!(notes["en"]["summary"], "They plan the launch.");
        assert_eq!(notes["bn"]["chapters"][0]["title"], "লঞ্চ");
        assert_eq!(notes["en"]["chapters"][0]["title"], "Launch");

        // What was sent: the audio as WAV in a multipart upload, then JSON text; the key only as a header.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        assert!(seen[0].0.starts_with("POST /v1/audio/transcriptions"));
        assert!(seen[0].1.windows(4).any(|w| w == b"RIFF"));
        assert!(seen[1..].iter().all(|(head, _)| head.starts_with("POST /v1/chat/completions")));
        for (head, body) in seen.iter() {
            assert!(head.contains(key), "sent as the bearer token");
            assert!(!body.windows(key.len()).any(|w| w == key.as_bytes()), "never in a body");
        }
        // Nothing the job wrote, nor the database, holds the key.
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap().flatten() {
            if entry.path().is_file() && entry.path() != path {
                let bytes = std::fs::read(entry.path()).unwrap();
                assert!(!bytes.windows(key.len()).any(|w| w == key.as_bytes()), "{:?}", entry.path());
            }
        }
        let db = std::fs::read(dir.join("meet.db")).unwrap();
        assert!(!db.windows(key.len()).any(|w| w == key.as_bytes()));

        // A failure says why, and with "fall back" off nothing else is tried.
        let (base, _) = stand_in(vec![crate::ai::cloud::testing::reply(401, "Unauthorized", r#"{"error":{"message":"bad key"}}"#, "")]);
        TEST_BASE.with(|b| *b.borrow_mut() = Some(base));
        let failed = hub.run_job(&rec, &Job::new(), &RunOptions::default()).unwrap_err();
        TEST_BASE.with(|b| *b.borrow_mut() = None);
        assert!(failed.contains("didn't accept the API key") && !failed.contains("SECRET"), "{failed}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untranslated_lines_are_found_and_batched_larger_for_the_cloud() {
        let segs: Vec<Seg> = (0..70).map(|_| seg("hello there", "en")).collect();
        let idx: Vec<usize> = (0..70).collect();
        let b = batches_of(&segs, &idx, LINES, LINE_CHARS);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [30, 30, 10]);
    }
}
