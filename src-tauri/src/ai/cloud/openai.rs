//! OpenAI: speech to text through the audio transcriptions endpoint, translation and notes through
//! Chat Completions with Structured Outputs. (OpenRouter's chat endpoint has the same shape, so
//! `chat_body` and `read_chat` are shared with it.)
//!
//! Docs used (read 2026-11-22):
//! - https://developers.openai.com/api/docs/guides/speech-to-text (25 MB per file; `gpt-transcribe`
//!   is the recommended model; `whisper-1`, the only one with timestamps, is deprecated and shuts
//!   down 2027-02-26)
//! - https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create
//!   (POST /v1/audio/transcriptions, multipart: `file`, `model`, `languages[]` for gpt-transcribe,
//!   `language`, `response_format`, `timestamp_granularities[]`; replies `{text, languages}` or
//!   verbose_json `{language, duration, text, segments[{start,end,text}]}`)
//! - https://developers.openai.com/api/docs/guides/structured-outputs and
//!   https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
//!   (POST /v1/chat/completions; `response_format: {type:"json_schema", json_schema:{name, strict,
//!   schema}}`; `max_completion_tokens`; `choices[0].finish_reason`, `message.content`, `message.refusal`)
//! - https://developers.openai.com/api/docs/models/gpt-transcribe, …/gpt-6.2-sol (model ids)
//!
//! `gpt-transcribe` returns no times, so its audio goes in short pieces cut at pauses and a piece's
//! sentences share its time; `whisper-1` (if the user picks it) returns real segment times.

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::{fraction, http_client, is_silent, offset, plan_chunks, script_lang, send, split_timed, wav16, Cloud, CloudError, Provider, Result, Segment, RATE};

pub const API: &str = "https://api.openai.com";
pub const STT_MODEL: &str = "gpt-transcribe";
pub const LLM_MODEL: &str = "gpt-6.2-sol";
const ME: Provider = Provider::OpenAi;
const MAX_TOKENS: u32 = 16_000;

pub struct OpenAi {
    client: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

impl OpenAi {
    pub fn new(key: String, model: String, base: Option<String>) -> Result<OpenAi> {
        Ok(OpenAi { client: http_client(Duration::from_secs(300), ME)?, base: base.unwrap_or_else(|| API.into()), key, model })
    }
}

// ---- multipart (reqwest's multipart feature isn't on; the body is simple enough to build)

/// A multipart/form-data body: returns its content type and bytes.
pub fn multipart(fields: &[(&str, &str)], file: (&str, &str, &str, &[u8])) -> (String, Vec<u8>) {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let boundary = format!("zazabor-form-{nanos:x}-{:x}", file.3.len());
    let mut out = Vec::with_capacity(file.3.len() + 1024);
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    let (name, filename, mime, bytes) = file;
    out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
    out.extend_from_slice(bytes);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

/// The form fields for one piece of audio. `whisper-1` asks for segment times; `gpt-transcribe`
/// takes the languages that may be spoken; the older gpt-4o models take one `language`.
pub fn transcription_fields<'a>(model: &'a str, langs: &[&'a str]) -> Vec<(&'static str, &'a str)> {
    let mut f = vec![("model", model)];
    match model {
        "whisper-1" => {
            f.push(("response_format", "verbose_json"));
            f.push(("timestamp_granularities[]", "segment"));
            if let [one] = langs {
                f.push(("language", one));
            }
        }
        "gpt-transcribe" => f.extend(langs.iter().map(|l| ("languages[]", *l))),
        _ => {
            f.push(("response_format", "json"));
            if let [one] = langs {
                f.push(("language", one));
            }
        }
    }
    f
}

/// The segments of a transcription reply (`length` is the piece's seconds). A reply with times
/// (verbose_json) keeps them; a plain `{text}` is split by sentence over the piece.
pub fn read_transcription(reply: &Value, length: f64) -> Vec<Segment> {
    if let Some(list) = reply["segments"].as_array() {
        let segs: Vec<Segment> = list
            .iter()
            .filter_map(|s| {
                let text = s["text"].as_str()?.trim();
                let (start, end) = (s["start"].as_f64()?, s["end"].as_f64()?);
                (!text.is_empty()).then(|| Segment { start, end: end.max(start + 0.2), lang: script_lang(text).into(), text: text.to_string() })
            })
            .collect();
        if !segs.is_empty() {
            return segs;
        }
    }
    split_timed(reply["text"].as_str().unwrap_or(""), 0.0, length)
}

impl Cloud for OpenAi {
    async fn transcribe(&self, audio: &[f32], langs: &[&str], progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> Result<Vec<Segment>> {
        // Whisper's times are its own, so a piece can be long (but under 25 MB: 10 minutes is 19 MB).
        let (max, slack) = if self.model == "whisper-1" { (600.0, 20.0) } else { (20.0, 6.0) };
        let chunks = plan_chunks(audio, max, slack);
        let mut out = Vec::new();
        for (i, range) in chunks.iter().enumerate() {
            progress(fraction(i, chunks.len()));
            let piece = &audio[range.clone()];
            if is_silent(piece) {
                continue;
            }
            let wav = wav16(piece);
            let (content_type, body) = multipart(&transcription_fields(&self.model, langs), ("file", "audio.wav", "audio/wav", &wav));
            let text = send(ME, &self.key, cancel, || {
                self.client.post(format!("{}/v1/audio/transcriptions", self.base)).bearer_auth(&self.key).header("content-type", &content_type).body(body.clone())
            })
            .await?;
            let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
            let secs = piece.len() as f64 / RATE as f64;
            out.extend(offset(read_transcription(&reply, secs), range.start as f64 / RATE as f64));
        }
        progress(1.0);
        Ok(out)
    }

    async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value> {
        let request = chat_body(&self.model, system, prompt, schema, "max_completion_tokens");
        let text = send(ME, &self.key, cancel, || self.client.post(format!("{}/v1/chat/completions", self.base)).bearer_auth(&self.key).header("content-type", "application/json").body(request.to_string())).await?;
        let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
        let answer = read_chat(ME, &reply)?;
        serde_json::from_str(answer.trim()).map_err(|e| CloudError::Bad(ME, format!("the answer isn't the JSON asked for ({e})")))
    }

    async fn ping(&self, cancel: &AtomicBool) -> Result<()> {
        let request = json!({ "model": self.model, "max_completion_tokens": 256, "messages": [{ "role": "user", "content": "Reply with the single word OK." }] });
        send(ME, &self.key, cancel, || self.client.post(format!("{}/v1/chat/completions", self.base)).bearer_auth(&self.key).header("content-type", "application/json").body(request.to_string())).await.map(|_| ())
    }
}

// ---- Chat Completions (shared with OpenRouter)

/// The request for one JSON answer. `max_key` is the name of the token limit (`max_completion_tokens`
/// for OpenAI, `max_tokens` for OpenRouter).
pub fn chat_body(model: &str, system: &str, prompt: &str, schema: &Value, max_key: &str) -> Value {
    let mut b = json!({
        "model": model,
        "messages": [{ "role": "system", "content": system }, { "role": "user", "content": prompt }],
        "response_format": { "type": "json_schema", "json_schema": { "name": "answer", "strict": true, "schema": schema } }
    });
    b[max_key] = json!(MAX_TOKENS);
    b
}

/// The text of a chat reply, after `finish_reason` and any refusal are checked.
pub fn read_chat(provider: Provider, reply: &Value) -> Result<String> {
    let Some(choice) = reply["choices"].get(0) else {
        let why = reply["error"]["message"].as_str().unwrap_or("the reply has no choices");
        return Err(CloudError::Bad(provider, why.chars().take(200).collect()));
    };
    if let Some(r) = choice["message"]["refusal"].as_str().filter(|r| !r.is_empty()) {
        return Err(CloudError::Refused(provider, r.chars().take(200).collect()));
    }
    match choice["finish_reason"].as_str() {
        Some("length") => return Err(CloudError::TooLong(provider)),
        Some("content_filter") => return Err(CloudError::Refused(provider, "its content filter stopped the answer".into())),
        _ => {}
    }
    match choice["message"]["content"].as_str().filter(|t| !t.trim().is_empty()) {
        Some(t) => Ok(t.to_string()),
        None => Err(CloudError::Bad(provider, "the reply has no text".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn text_of(body: &[u8]) -> String {
        String::from_utf8_lossy(body).into_owned()
    }

    #[test]
    fn multipart_has_each_field_and_the_file() {
        let (ct, body) = multipart(&[("model", "gpt-transcribe"), ("languages[]", "en"), ("languages[]", "bn")], ("file", "audio.wav", "audio/wav", b"RIFFdata"));
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let t = text_of(&body);
        assert!(t.starts_with(&format!("--{boundary}\r\n")));
        assert!(t.ends_with(&format!("\r\n--{boundary}--\r\n")));
        assert!(t.contains("name=\"model\"\r\n\r\ngpt-transcribe\r\n"));
        assert_eq!(t.matches("name=\"languages[]\"").count(), 2);
        assert!(t.contains("name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFdata\r\n"));
    }

    #[test]
    fn each_model_gets_the_fields_the_docs_give_it() {
        assert_eq!(transcription_fields("gpt-transcribe", &["en", "bn"]), [("model", "gpt-transcribe"), ("languages[]", "en"), ("languages[]", "bn")]);
        assert_eq!(transcription_fields("whisper-1", &["bn"]), [("model", "whisper-1"), ("response_format", "verbose_json"), ("timestamp_granularities[]", "segment"), ("language", "bn")]);
        assert_eq!(transcription_fields("whisper-1", &["en", "bn"]).iter().filter(|(k, _)| k.starts_with("language")).count(), 0, "two candidates: let it detect");
        assert_eq!(transcription_fields("gpt-4o-transcribe", &["en"]), [("model", "gpt-4o-transcribe"), ("response_format", "json"), ("language", "en")]);
    }

    #[test]
    fn the_docs_sample_replies_are_read() {
        // gpt-transcribe: text and detected languages.
        let plain = json!({"text": "Bonjour, pouvez-vous m'entendre ?", "languages": [{"code": "fr"}]});
        let s = read_transcription(&plain, 4.0);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].start, s[0].end), (0.0, 4.0));
        // whisper-1 verbose_json: segments with times.
        let verbose = json!({"language": "english", "duration": 8.47, "text": "The beach was a popular spot.", "segments": [
            {"id": 0, "seek": 0, "start": 0.0, "end": 3.2, "text": " The beach was a popular spot", "temperature": 0.0, "avg_logprob": -0.28, "compression_ratio": 1.2, "no_speech_prob": 0.01},
            {"id": 1, "seek": 0, "start": 3.2, "end": 8.0, "text": " on a hot summer day.", "temperature": 0.0, "avg_logprob": -0.28, "compression_ratio": 1.2, "no_speech_prob": 0.01}]});
        let s = read_transcription(&verbose, 8.47);
        assert_eq!(s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["The beach was a popular spot", "on a hot summer day."]);
        assert_eq!((s[1].start, s[1].end), (3.2, 8.0));
        assert!(read_transcription(&json!({"text": ""}), 3.0).is_empty());
    }

    #[test]
    fn chat_requests_and_replies_follow_the_docs() {
        let schema = json!({"type": "object", "additionalProperties": false, "required": ["ok"], "properties": {"ok": {"type": "boolean"}}});
        let b = chat_body("gpt-6.2-sol", "SYS", "Hi", &schema, "max_completion_tokens");
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
        assert_eq!(b["response_format"]["json_schema"]["schema"]["required"][0], "ok");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["max_completion_tokens"], 16_000);
        assert!(b.get("max_tokens").is_none() && b.get("temperature").is_none());
        let ok = json!({"id": "chatcmpl-1", "object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "content": "{\"ok\":true}", "refusal": null}, "finish_reason": "stop"}]});
        assert_eq!(read_chat(ME, &ok).unwrap(), "{\"ok\":true}");
        let cut = json!({"choices": [{"message": {"content": "{\"o"}, "finish_reason": "length"}]});
        assert_eq!(read_chat(ME, &cut), Err(CloudError::TooLong(ME)));
        let refused = json!({"choices": [{"message": {"content": null, "refusal": "I can't help with that."}, "finish_reason": "stop"}]});
        assert_eq!(read_chat(ME, &refused), Err(CloudError::Refused(ME, "I can't help with that.".into())));
        let filtered = json!({"choices": [{"message": {"content": ""}, "finish_reason": "content_filter"}]});
        assert!(matches!(read_chat(ME, &filtered), Err(CloudError::Refused(..))));
        assert!(matches!(read_chat(ME, &json!({"choices": []})), Err(CloudError::Bad(..))));
        assert!(matches!(read_chat(ME, &json!({"choices": [{"message": {"content": null}, "finish_reason": "stop"}]})), Err(CloudError::Bad(..))));
    }

    #[test]
    fn audio_is_sent_in_pieces_with_the_bearer_key_and_times_offset() {
        // 50 s of audio with speech at 5 s and 35 s: pieces of at most 20 s.
        let audio = audio(50.0, &[5.0, 35.0]);
        let reply = |t: &str| ok(&json!({"text": t, "languages": [{"code": "en"}]}));
        let (base, seen) = stand_in(vec![reply("First."), reply("Second."), reply("Third.")]);
        let c = OpenAi::new("sk-test-KEY-123456".into(), STT_MODEL.into(), Some(base)).unwrap();
        let progress = std::sync::Mutex::new(Vec::new());
        let segs = run(c.transcribe(&audio, &["en", "bn"], &|f| progress.lock().unwrap().push(f), &AtomicBool::new(false))).unwrap();
        // Pieces with no sound aren't sent: only the ones holding the two bursts.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), segs.len());
        assert!(seen.len() == 2, "{} requests", seen.len());
        let head = seen[0].0.to_lowercase();
        assert!(head.starts_with("post /v1/audio/transcriptions"));
        assert!(head.contains("authorization: bearer sk-test-key-123456"));
        assert!(head.contains("multipart/form-data; boundary="));
        assert!(text_of(&seen[0].1).contains("name=\"languages[]\"\r\n\r\nen"));
        assert!(!text_of(&seen[0].1).contains("sk-test-KEY"), "the key is never in the body");
        assert!(segs[1].start >= 19.0, "the second piece starts later: {segs:?}");
        assert_eq!(segs[0].text, "First.");
        assert_eq!(*progress.lock().unwrap().last().unwrap(), 1.0);
    }

    #[test]
    fn a_json_answer_comes_back_parsed_and_errors_are_mapped() {
        let schema = json!({"type": "object", "properties": {}});
        let good = json!({"choices": [{"message": {"content": "{\"ok\":true}"}, "finish_reason": "stop"}]});
        let (base, seen) = stand_in(vec![ok(&good)]);
        let c = OpenAi::new("sk-test-KEY-123456".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &schema, &AtomicBool::new(false))).unwrap(), json!({"ok": true}));
        assert!(seen.lock().unwrap()[0].0.starts_with("POST /v1/chat/completions"));
        let (base, _) = stand_in(vec![reply(401, "Unauthorized", r#"{"error":{"message":"Incorrect API key provided: sk-test-KEY-123456.","type":"invalid_request_error","code":"invalid_api_key"}}"#, "")]);
        let c = OpenAi::new("sk-test-KEY-123456".into(), LLM_MODEL.into(), Some(base)).unwrap();
        let e = run(c.ping(&AtomicBool::new(false))).unwrap_err();
        assert_eq!(e, CloudError::Key(ME));
        assert!(!e.text().contains("KEY-123456"));
        let (base, _) = stand_in(vec![reply(429, "Too Many Requests", r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#, "")]);
        let c = OpenAi::new("k-123456".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.ping(&AtomicBool::new(false))), Err(CloudError::NoCredit(ME)));
    }
}
