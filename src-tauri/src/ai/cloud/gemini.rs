//! Gemini (Google): speech to text, translation and notes through the Interactions API.
//!
//! Docs used (read 2026-11-22):
//! - https://ai.google.dev/gemini-api/docs/interactions (the Interactions API is the recommended
//!   interface; `generateContent` is legacy) and https://ai.google.dev/api/interactions-api
//!   (POST /v1beta/interactions; `status`, `steps[].content[].text`; `system_instruction`,
//!   `store`)
//! - https://ai.google.dev/gemini-api/docs/audio (audio input: `{"type":"audio","data":<base64>,
//!   "mime_type":"audio/wav"}`, headers `x-goog-api-key` and `Api-Revision: 2026-05-20`)
//! - https://ai.google.dev/gemini-api/docs/file-input-methods (inline data up to 100 MB per request,
//!   so the Files API isn't needed for 10-minute pieces)
//! - https://ai.google.dev/gemini-api/docs/structured-output (`response_format` = `{"type":"text",
//!   "mime_type":"application/json","schema":…}`)
//! - https://ai.google.dev/gemini-api/docs/speech-to-text and /models/gemini-3.5-transcribe (the
//!   transcription model: `generation_config.transcription_config.mode` with
//!   `timestamp_granularities: ["word"]`; word times come back as `word_info` annotations with
//!   `start_offset`/`end_offset` like "0.100s"; Bengali is `bn-BD`)
//!
//! `store: false` keeps Google from keeping the interaction. `gemini-3.5-transcribe` gets the
//! dedicated transcription mode (real word times, grouped into subtitle segments); any other model
//! is asked for strict JSON segments with mm:ss times, which are then validated for order.

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::{b64_wav, fraction, http_client, is_silent, offset, parse_clock, plan_chunks, prompted_segments, segments_schema, send, split_timed, transcribe_prompt, words_to_segments, Cloud, CloudError, Provider, Result, Segment, RATE};

pub const API: &str = "https://generativelanguage.googleapis.com";
pub const STT_MODEL: &str = "gemini-3.5-transcribe";
pub const LLM_MODEL: &str = "gemini-4-hydrogen";
const REVISION: &str = "2026-05-20";
const ME: Provider = Provider::Gemini;

pub struct Gemini {
    client: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

impl Gemini {
    pub fn new(key: String, model: String, base: Option<String>) -> Result<Gemini> {
        Ok(Gemini { client: http_client(Duration::from_secs(600), ME)?, base: base.unwrap_or_else(|| API.into()), key, model })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1beta/interactions", self.base))
            .header("x-goog-api-key", &self.key)
            .header("Api-Revision", REVISION)
            .header("content-type", "application/json")
            .body(body.to_string())
    }

    fn is_transcriber(&self) -> bool {
        self.model.trim_start_matches("models/").starts_with("gemini-3.5-transcribe")
    }
}

/// The request for one JSON answer.
pub fn json_body(model: &str, system: &str, prompt: &str, schema: &Value) -> Value {
    json!({
        "model": model,
        "store": false,
        "system_instruction": system,
        "input": prompt,
        "response_format": { "type": "text", "mime_type": "application/json", "schema": schema }
    })
}

/// A request that only shows the key works.
pub fn ping_body(model: &str) -> Value {
    json!({ "model": model, "store": false, "input": "Reply with the single word OK." })
}

/// The BCP-47 code Gemini 3.5 Transcribe takes for one expected language (none when it's both).
fn language_codes(langs: &[&str]) -> Vec<&'static str> {
    match langs {
        ["bn"] => vec!["bn-BD"],
        ["en"] => vec!["en-US"],
        _ => Vec::new(),
    }
}

/// The request for the dedicated transcription model: verbatim text with word times.
pub fn transcriber_body(model: &str, wav_b64: &str, langs: &[&str]) -> Value {
    let mut config = json!({ "mode": { "type": "verbatim", "timestamp_granularities": ["word"] } });
    let codes = language_codes(langs);
    if !codes.is_empty() {
        config["language_codes"] = json!(codes);
    }
    json!({
        "model": model,
        "store": false,
        "input": [{ "type": "audio", "data": wav_b64, "mime_type": "audio/wav" }],
        "generation_config": { "transcription_config": config }
    })
}

/// The request for a general model asked to write the segments as JSON.
pub fn prompted_body(model: &str, wav_b64: &str, langs: &[&str]) -> Value {
    json!({
        "model": model,
        "store": false,
        "input": [
            { "type": "text", "text": transcribe_prompt(langs) },
            { "type": "audio", "data": wav_b64, "mime_type": "audio/wav" }
        ],
        "response_format": { "type": "text", "mime_type": "application/json", "schema": segments_schema() }
    })
}

/// The model's output blocks: `steps[type=model_output].content[]` (or `outputs[]`).
fn content_blocks(reply: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    if let Some(steps) = reply["steps"].as_array() {
        for step in steps.iter().filter(|s| s["type"] == "model_output") {
            out.extend(step["content"].as_array().into_iter().flatten());
        }
    }
    if out.is_empty() {
        out.extend(reply["outputs"].as_array().into_iter().flatten());
    }
    out
}

/// The status of an interaction checked, then its text.
pub fn read_text(reply: &Value) -> Result<String> {
    match reply["status"].as_str() {
        Some("completed") | None => {}
        Some("incomplete") => return Err(CloudError::TooLong(ME)),
        Some(other) => return Err(CloudError::Bad(ME, format!("it ended as \"{other}\""))),
    }
    let text: String = content_blocks(reply).into_iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    if text.trim().is_empty() {
        return Err(CloudError::Bad(ME, "the reply has no text".into()));
    }
    Ok(text)
}

/// The words with times in a transcription reply: `word_info` annotations on the text content.
pub fn read_words(reply: &Value) -> Vec<(f64, f64, String)> {
    content_blocks(reply)
        .into_iter()
        .flat_map(|b| b["annotations"].as_array().into_iter().flatten())
        .filter(|a| a["type"] == "word_info")
        .filter_map(|a| {
            let (start, end) = (parse_clock(a["start_offset"].as_str()?)?, parse_clock(a["end_offset"].as_str()?)?);
            Some((start, end.max(start), a["text"].as_str()?.to_string()))
        })
        .collect()
}

impl Cloud for Gemini {
    async fn transcribe(&self, audio: &[f32], langs: &[&str], progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> Result<Vec<Segment>> {
        // The transcription model keeps its own times; a general model's drift, so its pieces are shorter.
        let chunks = if self.is_transcriber() { plan_chunks(audio, 600.0, 20.0) } else { plan_chunks(audio, 300.0, 15.0) };
        let mut out = Vec::new();
        for (i, range) in chunks.iter().enumerate() {
            progress(fraction(i, chunks.len()));
            let piece = &audio[range.clone()];
            if is_silent(piece) {
                continue;
            }
            let secs = piece.len() as f64 / RATE as f64;
            let wav = b64_wav(piece);
            let request = if self.is_transcriber() { transcriber_body(&self.model, &wav, langs) } else { prompted_body(&self.model, &wav, langs) };
            let text = send(ME, &self.key, cancel, || self.post(&request)).await?;
            let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
            let segments = if self.is_transcriber() {
                let words = read_words(&reply);
                if words.is_empty() {
                    split_timed(&read_text(&reply)?, 0.0, secs)
                } else {
                    words_to_segments(&words)
                }
            } else {
                let answer: Value = serde_json::from_str(read_text(&reply)?.trim()).map_err(|e| CloudError::Bad(ME, format!("the segments aren't the JSON asked for ({e})")))?;
                prompted_segments(&answer, secs)
            };
            out.extend(offset(segments, range.start as f64 / RATE as f64));
        }
        progress(1.0);
        Ok(out)
    }

    async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value> {
        let request = json_body(&self.model, system, prompt, schema);
        let text = send(ME, &self.key, cancel, || self.post(&request)).await?;
        let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
        serde_json::from_str(read_text(&reply)?.trim()).map_err(|e| CloudError::Bad(ME, format!("the answer isn't the JSON asked for ({e})")))
    }

    async fn ping(&self, cancel: &AtomicBool) -> Result<()> {
        let request = ping_body(&self.model);
        send(ME, &self.key, cancel, || self.post(&request)).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    /// The docs' "parsing output" example, with the model output wrapped in `steps`.
    fn transcription_reply() -> Value {
        json!({"id": "interactions/abc123xyz", "status": "completed", "steps": [{"id": "step_001", "type": "model_output", "content": [{
            "type": "text", "text": "Hello world. আমি দেখব",
            "annotations": [
                {"type": "word_info", "text": "Hello", "speaker": "spk_1", "start_offset": "0.100s", "end_offset": "0.400s"},
                {"type": "word_info", "text": "world.", "speaker": "spk_1", "start_offset": "0.500s", "end_offset": "0.800s"},
                {"type": "word_info", "text": "আমি", "start_offset": "4.000s", "end_offset": "4.300s"},
                {"type": "word_info", "text": "দেখব", "start_offset": "4.400s", "end_offset": "4.900s"}]}]}]})
    }

    fn text_reply(text: &str) -> Value {
        json!({"id": "v1_x", "model": "gemini-4-hydrogen", "object": "interaction", "status": "completed", "steps": [{"type": "model_output", "content": [{"type": "text", "text": text}]}]})
    }

    #[test]
    fn requests_follow_the_docs() {
        let schema = json!({"type": "object"});
        let b = json_body("gemini-4-hydrogen", "SYS", "Hi", &schema);
        assert_eq!(b["system_instruction"], "SYS");
        assert_eq!(b["input"], "Hi");
        assert_eq!(b["store"], false);
        assert_eq!(b["response_format"], json!({"type": "text", "mime_type": "application/json", "schema": {"type": "object"}}));
        let t = transcriber_body("gemini-3.5-transcribe", "QUJD", &["bn"]);
        assert_eq!(t["input"][0], json!({"type": "audio", "data": "QUJD", "mime_type": "audio/wav"}));
        assert_eq!(t["generation_config"]["transcription_config"]["mode"], json!({"type": "verbatim", "timestamp_granularities": ["word"]}));
        assert_eq!(t["generation_config"]["transcription_config"]["language_codes"], json!(["bn-BD"]));
        assert!(transcriber_body("m", "x", &["en", "bn"])["generation_config"]["transcription_config"].get("language_codes").is_none(), "a mixed meeting is left to detect");
        let p = prompted_body("gemini-4-hydrogen", "QUJD", &["en", "bn"]);
        assert_eq!(p["input"][1]["type"], "audio");
        assert!(p["input"][0]["text"].as_str().unwrap().contains("mm:ss"));
        assert_eq!(p["response_format"]["mime_type"], "application/json");
        assert_eq!(p["response_format"]["schema"]["required"][0], "segments");
    }

    #[test]
    fn replies_are_read_by_status_then_text() {
        assert_eq!(read_text(&text_reply("{\"ok\":true}")).unwrap(), "{\"ok\":true}");
        let mut incomplete = text_reply("{");
        incomplete["status"] = json!("incomplete");
        assert_eq!(read_text(&incomplete), Err(CloudError::TooLong(ME)));
        incomplete["status"] = json!("failed");
        assert!(matches!(read_text(&incomplete), Err(CloudError::Bad(..))));
        assert!(matches!(read_text(&json!({"status": "completed", "steps": []})), Err(CloudError::Bad(..))));
        // The older shape with `outputs` is read too.
        assert_eq!(read_text(&json!({"status": "completed", "outputs": [{"type": "text", "text": "hi"}]})).unwrap(), "hi");
        // Thinking and tool steps are not the answer.
        let r = json!({"status": "completed", "steps": [{"type": "thought", "content": [{"type": "text", "text": "hmm"}]}, {"type": "model_output", "content": [{"type": "text", "text": "answer"}]}]});
        assert_eq!(read_text(&r).unwrap(), "answer");
    }

    #[test]
    fn word_times_become_subtitle_segments() {
        let words = read_words(&transcription_reply());
        assert_eq!(words.len(), 4);
        assert_eq!(words[0], (0.1, 0.4, "Hello".to_string()));
        let segs = words_to_segments(&words);
        assert_eq!(segs.iter().map(|s| (s.text.as_str(), s.lang.as_str())).collect::<Vec<_>>(), [("Hello world.", "en"), ("আমি দেখব", "bn")]);
        assert_eq!(segs[1].start, 4.0);
    }

    #[test]
    fn the_transcription_model_is_used_with_word_times_and_offsets() {
        let audio = audio(30.0, &[3.0]);
        let (base, seen) = stand_in(vec![ok(&transcription_reply())]);
        let c = Gemini::new("AIza-test-KEY-12345".into(), STT_MODEL.into(), Some(base)).unwrap();
        let segs = run(c.transcribe(&audio, &["en", "bn"], &|_| {}, &AtomicBool::new(false))).unwrap();
        assert_eq!(segs.len(), 2);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "30 s is one piece");
        let head = seen[0].0.to_lowercase();
        assert!(head.starts_with("post /v1beta/interactions"));
        assert!(head.contains("x-goog-api-key: aiza-test-key-12345"));
        assert!(head.contains("api-revision: 2026-05-20"));
        let body = String::from_utf8_lossy(&seen[0].1);
        assert!(!body.contains("AIza-test-KEY"), "the key is only in the header, never the body or URL");
        assert!(!head.lines().next().unwrap().contains("key"), "not in the URL");
    }

    #[test]
    fn a_general_model_is_asked_for_json_segments_that_are_then_checked() {
        let audio = audio(30.0, &[3.0]);
        let answer = json!({"segments": [{"start": "00:02", "end": "00:05", "text": "We ship on Friday."}, {"start": "00:04", "end": "00:03", "text": "ঠিক আছে"}, {"start": "09:59", "end": "10:05", "text": "Way past the end"}]});
        let (base, _) = stand_in(vec![ok(&text_reply(&answer.to_string()))]);
        let c = Gemini::new("AIza-test-KEY-12345".into(), LLM_MODEL.into(), Some(base)).unwrap();
        let segs = run(c.transcribe(&audio, &["en", "bn"], &|_| {}, &AtomicBool::new(false))).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].start, segs[0].end), (2.0, 5.0));
        assert!(segs[1].start >= segs[0].start && segs[1].end > segs[1].start);
        assert!(segs.iter().all(|s| s.end <= 30.0));
    }

    #[test]
    fn json_answers_and_failures() {
        let (base, seen) = stand_in(vec![ok(&text_reply("{\"ok\":true}"))]);
        let c = Gemini::new("AIza-test-KEY-12345".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &json!({"type": "object"}), &AtomicBool::new(false))).unwrap(), json!({"ok": true}));
        assert!(seen.lock().unwrap()[0].0.starts_with("POST /v1beta/interactions"));
        let (base, _) = stand_in(vec![ok(&text_reply("not json"))]);
        let c = Gemini::new("AIza-test-KEY-12345".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert!(matches!(run(c.complete_json("s", "p", &json!({}), &AtomicBool::new(false))), Err(CloudError::Bad(..))));
        let (base, _) = stand_in(vec![reply(403, "Forbidden", r#"{"error":{"code":403,"message":"API key not valid AIza-test-KEY-12345","status":"PERMISSION_DENIED"}}"#, "")]);
        let c = Gemini::new("AIza-test-KEY-12345".into(), LLM_MODEL.into(), Some(base)).unwrap();
        let e = run(c.ping(&AtomicBool::new(false))).unwrap_err();
        assert_eq!(e, CloudError::Key(ME));
        assert!(!e.text().contains("test-KEY"));
    }
}
