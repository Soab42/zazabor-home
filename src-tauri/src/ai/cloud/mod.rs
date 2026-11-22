//! Cloud AI as a second way to do Meet's speech to text, translation and notes (the first is the
//! local Whisper and Qwen next to this folder). Nothing here runs unless the user turned Cloud on
//! in Settings → Meet and confirmed what is sent. Each provider is raw HTTPS with the HTTP stack
//! the Statements reader already uses (there's no official Rust SDK for any of them), behind one
//! small trait: `transcribe` (audio to timed segments) and `complete_json` (a prompt and a JSON
//! schema to a JSON value). Claude has no audio input, so it only does the second.
//!
//! The API key is read from the keyring by the caller, lives only inside a provider value and in
//! its request header, and is never logged, formatted into an error, or returned to the page:
//! `redact` strips it from anything an API echoes back. Transcript text and audio are never logged.

pub mod claude;
pub mod gemini;
pub mod openai;
pub mod openrouter;

use serde::Serialize;
use serde_json::Value;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What the audio helpers expect: 16 kHz mono, as Whisper takes it.
pub const RATE: usize = 16_000;
/// Attempts for a request that failed with 429, 5xx or a dropped connection.
const ATTEMPTS: u32 = 4;
/// The longest we wait before trying again, even if the API asks for more.
const MAX_WAIT: Duration = Duration::from_secs(60);

// ---- providers

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Claude,
    OpenAi,
    Gemini,
    OpenRouter,
}

pub const ALL: [Provider; 4] = [Provider::Claude, Provider::OpenAi, Provider::Gemini, Provider::OpenRouter];

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
            Provider::OpenRouter => "openrouter",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude (Anthropic)",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Gemini (Google)",
            Provider::OpenRouter => "OpenRouter",
        }
    }

    /// The short name used in sentences.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Gemini",
            Provider::OpenRouter => "OpenRouter",
        }
    }

    /// Where the user gets a key.
    pub fn key_page(self) -> &'static str {
        match self {
            Provider::Claude => "https://platform.claude.com/settings/keys",
            Provider::OpenAi => "https://platform.openai.com/api-keys",
            Provider::Gemini => "https://aistudio.google.com/apikey",
            Provider::OpenRouter => "https://openrouter.ai/settings/keys",
        }
    }

    /// Claude's Messages API takes no audio.
    pub fn can_transcribe(self) -> bool {
        self != Provider::Claude
    }

    /// The model for speech to text when the user hasn't chosen one.
    pub fn default_stt_model(self) -> Option<&'static str> {
        match self {
            Provider::Claude => None,
            Provider::OpenAi => Some(openai::STT_MODEL),
            Provider::Gemini => Some(gemini::STT_MODEL),
            Provider::OpenRouter => Some(openrouter::STT_MODEL),
        }
    }

    /// The model for translation and notes when the user hasn't chosen one.
    pub fn default_llm_model(self) -> &'static str {
        match self {
            Provider::Claude => claude::MODEL,
            Provider::OpenAi => openai::LLM_MODEL,
            Provider::Gemini => gemini::LLM_MODEL,
            Provider::OpenRouter => openrouter::LLM_MODEL,
        }
    }
}

/// A model id the user typed: letters, digits and `- . _ : / ~` only (OpenRouter ids have a slash).
pub fn clean_model(model: &str) -> Option<String> {
    let m = model.trim();
    (!m.is_empty() && m.len() <= 80 && m.chars().all(|c| c.is_ascii_alphanumeric() || "-._:/~".contains(c))).then(|| m.to_string())
}

// ---- what comes back

/// One stretch of speech, in seconds from the start of the audio given.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    /// "en" or "bn".
    pub lang: String,
    pub text: String,
}

/// Why a request failed, in words for the page (`text`).
#[derive(Clone, Debug, PartialEq)]
pub enum CloudError {
    /// Rate limit or overload that didn't clear after the retries.
    Busy(Provider),
    /// The key is missing, wrong, revoked or not allowed to do this.
    Key(Provider),
    NoCredit(Provider),
    /// No connection, a timeout, or a dropped connection.
    Network(Provider),
    /// The provider declined to handle the content.
    Refused(Provider, String),
    /// The answer didn't fit in one reply.
    TooLong(Provider),
    /// The request itself was refused (unknown model, too large, not allowed).
    Request(Provider, String),
    /// The answer arrived but wasn't usable.
    Bad(Provider, String),
    Cancelled,
}

impl CloudError {
    pub fn text(&self) -> String {
        match self {
            CloudError::Busy(p) => format!("{} is busy or limiting this key. Try again in a few minutes.", p.name()),
            CloudError::Key(p) => format!("{} didn't accept the API key. Check it in Settings → Meet.", p.name()),
            CloudError::NoCredit(p) => format!("{} says this account has no credit (or quota) left.", p.name()),
            CloudError::Network(p) => format!("Couldn't reach {}. Check the internet connection.", p.name()),
            CloudError::Refused(p, why) => format!("{} declined to handle this{}.", p.name(), if why.is_empty() { String::new() } else { format!(": {why}") }),
            CloudError::TooLong(p) => format!("{}'s reply was too long for one request.", p.name()),
            CloudError::Request(p, why) => format!("{} refused the request: {why}", p.name()),
            CloudError::Bad(p, why) => format!("{}'s answer wasn't usable: {why}", p.name()),
            CloudError::Cancelled => "Cancelled".into(),
        }
    }
}

impl From<CloudError> for String {
    fn from(e: CloudError) -> String {
        e.text()
    }
}

pub type Result<T> = std::result::Result<T, CloudError>;

// ---- the trait

/// What a provider can do. `Client` picks one by id (async fns in a trait can't be `dyn`).
#[allow(async_fn_in_trait)]
pub trait Cloud {
    /// Speech to text for 16 kHz mono audio. `langs` are the languages expected ("en", "bn"; one
    /// means it's known). Times are seconds from the start of `audio`; long audio is cut into
    /// requests that fit, and the times offset. `progress` gets 0…1.
    async fn transcribe(&self, audio: &[f32], langs: &[&str], progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> Result<Vec<Segment>>;

    /// One answer as JSON matching `schema` (the provider's structured-output mode where it has one).
    async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value>;

    /// A tiny request that shows the key and model work.
    async fn ping(&self, cancel: &AtomicBool) -> Result<()>;
}

/// A provider with its key and model. `base` is the API's address (a local stand-in in tests).
pub enum Client {
    Claude(claude::Claude),
    OpenAi(openai::OpenAi),
    Gemini(gemini::Gemini),
    OpenRouter(openrouter::OpenRouter),
}

impl Client {
    pub fn new(provider: Provider, key: String, model: String) -> Result<Client> {
        Client::with_base(provider, key, model, None)
    }

    pub fn with_base(provider: Provider, key: String, model: String, base: Option<String>) -> Result<Client> {
        Ok(match provider {
            Provider::Claude => Client::Claude(claude::Claude::new(key, model, base)?),
            Provider::OpenAi => Client::OpenAi(openai::OpenAi::new(key, model, base)?),
            Provider::Gemini => Client::Gemini(gemini::Gemini::new(key, model, base)?),
            Provider::OpenRouter => Client::OpenRouter(openrouter::OpenRouter::new(key, model, base)?),
        })
    }

    pub fn provider(&self) -> Provider {
        match self {
            Client::Claude(_) => Provider::Claude,
            Client::OpenAi(_) => Provider::OpenAi,
            Client::Gemini(_) => Provider::Gemini,
            Client::OpenRouter(_) => Provider::OpenRouter,
        }
    }

    pub async fn transcribe(&self, audio: &[f32], langs: &[&str], progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> Result<Vec<Segment>> {
        match self {
            Client::Claude(c) => c.transcribe(audio, langs, progress, cancel).await,
            Client::OpenAi(c) => c.transcribe(audio, langs, progress, cancel).await,
            Client::Gemini(c) => c.transcribe(audio, langs, progress, cancel).await,
            Client::OpenRouter(c) => c.transcribe(audio, langs, progress, cancel).await,
        }
    }

    pub async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value> {
        match self {
            Client::Claude(c) => c.complete_json(system, prompt, schema, cancel).await,
            Client::OpenAi(c) => c.complete_json(system, prompt, schema, cancel).await,
            Client::Gemini(c) => c.complete_json(system, prompt, schema, cancel).await,
            Client::OpenRouter(c) => c.complete_json(system, prompt, schema, cancel).await,
        }
    }

    pub async fn ping(&self, cancel: &AtomicBool) -> Result<()> {
        match self {
            Client::Claude(c) => c.ping(cancel).await,
            Client::OpenAi(c) => c.ping(cancel).await,
            Client::Gemini(c) => c.ping(cancel).await,
            Client::OpenRouter(c) => c.ping(cancel).await,
        }
    }
}

// ---- HTTP

pub(crate) fn http_client(read_timeout: Duration, provider: Provider) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(read_timeout)
        .build()
        .map_err(|e| CloudError::Request(provider, e.without_url().to_string()))
}

/// Takes the key (and anything that looks like it) out of text an API sent back.
pub(crate) fn redact(text: &str, key: &str) -> String {
    let mut out = text.to_string();
    for secret in [key, key.trim_start_matches("Bearer ")] {
        if secret.len() >= 6 {
            out = out.replace(secret, "…");
        }
    }
    out.chars().take(300).collect()
}

/// The error message in an API's error reply (all four put it at `error.message`).
fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    v["error"]["message"].as_str().or(v["message"].as_str()).map(str::to_string).unwrap_or_else(|| body.chars().take(200).collect())
}

/// How a non-2xx reply is told to the user, and whether trying again could help.
pub(crate) fn classify(provider: Provider, status: u16, body: &str, key: &str) -> (CloudError, bool) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or("");
    let kind = v["error"]["type"].as_str().unwrap_or("");
    let message = error_message(body);
    // Out of credit is reported as 402, as 429 with a quota code, or (Anthropic) as a 400 saying so.
    let no_credit = status == 402 || code == "insufficient_quota" || kind == "insufficient_quota" || (provider == Provider::Claude && status == 400 && message.to_lowercase().contains("credit balance"));
    if no_credit {
        return (CloudError::NoCredit(provider), false);
    }
    match status {
        401 | 403 => (CloudError::Key(provider), false),
        429 | 500 | 502 | 503 | 504 | 529 => (CloudError::Busy(provider), true),
        413 => (CloudError::Request(provider, "the audio or text was too large to send".into()), false),
        404 => (CloudError::Request(provider, format!("not found; is the model name right? ({})", redact(&message, key))), false),
        _ => (CloudError::Request(provider, format!("{status}: {}", redact(&message, key))), false),
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).map(Duration::from_secs)
}

/// Waits, noticing a cancel within a quarter of a second.
pub(crate) async fn nap(wait: Duration, cancel: &AtomicBool) -> Result<()> {
    let mut left = wait;
    while !left.is_zero() {
        if cancel.load(Ordering::Relaxed) {
            return Err(CloudError::Cancelled);
        }
        let step = left.min(Duration::from_millis(250));
        tokio::time::sleep(step).await;
        left -= step;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CloudError::Cancelled);
    }
    Ok(())
}

/// Sends the request `build` makes, retrying 429, 5xx and dropped connections with a growing wait
/// (the API's `retry-after` if it gave one). Returns the body of the first 2xx reply.
pub(crate) async fn send(provider: Provider, key: &str, cancel: &AtomicBool, build: impl Fn() -> reqwest::RequestBuilder) -> Result<String> {
    let mut last = CloudError::Busy(provider);
    for n in 0..ATTEMPTS {
        if cancel.load(Ordering::Relaxed) {
            return Err(CloudError::Cancelled);
        }
        let mut wait = Duration::from_millis(1500 * 2u64.pow(n));
        match build().send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if let Some(w) = retry_after(response.headers()) {
                    wait = w;
                }
                match response.text().await {
                    Ok(body) if (200..300).contains(&status) => return Ok(body),
                    Ok(body) => {
                        let (error, retry) = classify(provider, status, &body, key);
                        if !retry {
                            return Err(error);
                        }
                        last = error;
                    }
                    Err(_) => last = CloudError::Network(provider),
                }
            }
            Err(e) if e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() => last = CloudError::Network(provider),
            Err(e) => return Err(CloudError::Request(provider, redact(&e.without_url().to_string(), key))),
        }
        if n + 1 < ATTEMPTS {
            nap(wait.min(MAX_WAIT), cancel).await?;
        }
    }
    Err(last)
}

// ---- audio

/// A 16-bit mono WAV file of the samples.
pub fn wav16(samples: &[f32]) -> Vec<u8> {
    let data = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// A WAV of the samples as base64, for requests that carry the audio inline.
pub fn b64_wav(samples: &[f32]) -> String {
    crate::statements::ai::b64(&wav16(samples))
}

/// Cuts `audio` into consecutive pieces of at most `max_secs`, with no overlap and nothing left
/// out. Each cut sits at the quietest moment of the last `slack_secs` of the piece, so a word is
/// seldom split. The pieces' start times are the sums of what came before.
pub fn plan_chunks(audio: &[f32], max_secs: f64, slack_secs: f64) -> Vec<Range<usize>> {
    let max = (max_secs * RATE as f64) as usize;
    let slack = ((slack_secs * RATE as f64) as usize).min(max / 2);
    let frame = RATE / 20; // 50 ms
    let mut out = Vec::new();
    let mut from = 0;
    while from < audio.len() {
        let hard = from + max;
        if hard >= audio.len() {
            out.push(from..audio.len());
            break;
        }
        let mut cut = hard;
        if slack >= frame {
            let mut best = f32::MAX;
            let mut at = hard - slack;
            while at + frame <= hard {
                let energy: f32 = audio[at..at + frame].iter().map(|s| s * s).sum();
                // Later wins a tie: longer pieces, fewer requests.
                if energy <= best {
                    best = energy;
                    cut = at + frame / 2;
                }
                at += frame / 2;
            }
        }
        out.push(from..cut);
        from = cut;
    }
    out
}

/// Whether a piece of audio is silent (a muted microphone): nothing worth sending.
pub fn is_silent(audio: &[f32]) -> bool {
    audio.iter().fold(0f32, |m, v| m.max(v.abs())) < 0.003
}

/// "~N minutes of audio" for the text before something is sent.
pub fn minutes_text(seconds: f64) -> String {
    let m = (seconds / 60.0).round().max(1.0) as u64;
    format!("~{m} minute{} of audio", if m == 1 { "" } else { "s" })
}

// ---- timed segments from a model's JSON answer

/// The language of a piece of text by its script: Bangla letters make it "bn", else "en".
pub fn script_lang(text: &str) -> &'static str {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return "en";
    }
    let bn = letters.iter().filter(|c| ('\u{0980}'..='\u{09FF}').contains(*c)).count();
    if bn * 2 >= letters.len() {
        "bn"
    } else {
        "en"
    }
}

/// "1:05", "01:05.5" or "1:02:03" or plain seconds, to seconds.
pub fn parse_clock(text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches('s');
    let mut total = 0.0;
    let parts: Vec<&str> = t.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for p in &parts {
        let v: f64 = p.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        total = total * 60.0 + v;
    }
    Some(total)
}

/// What a model is asked to answer with when it transcribes (providers without a transcription
/// endpoint, and Gemini's general models): `{"segments":[{"start":"mm:ss","end":"mm:ss","text":""}]}`.
pub fn segments_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["segments"],
        "properties": {
            "segments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["start", "end", "text"],
                    "properties": {
                        "start": { "type": "string", "description": "When the speech starts, as mm:ss (or h:mm:ss)" },
                        "end": { "type": "string", "description": "When it ends, as mm:ss (or h:mm:ss)" },
                        "text": { "type": "string", "description": "Exactly what was said, in the language spoken (Bangla in Bangla script)" }
                    }
                }
            }
        }
    })
}

/// The instruction that goes with the audio for `segments_schema`.
pub fn transcribe_prompt(langs: &[&str]) -> String {
    let spoken = match langs {
        ["bn"] => "The speech is Bangla (Bengali).".to_string(),
        ["en"] => "The speech is English.".to_string(),
        _ => "The speech is English, Bangla (Bengali), or a mix of both within the same recording.".to_string(),
    };
    format!(
        "Transcribe this audio. {spoken} Write Bangla in Bangla script and English in English, exactly as said: no translation, no summary, no speaker labels. \
Split it into segments of one or two sentences (at most about 15 seconds each) and give each its start and end time as mm:ss from the beginning of the audio, with times that only move forward. \
Leave out silence, music and noise. If nothing is said, answer with an empty list."
    )
}

/// The segments of a model's answer to `transcribe_prompt`, checked: times parsed, kept inside
/// `length` seconds, in order, with an end after each start. Pieces with no text are dropped.
pub fn prompted_segments(answer: &Value, length: f64) -> Vec<Segment> {
    let Some(list) = answer["segments"].as_array() else { return Vec::new() };
    let mut out: Vec<Segment> = Vec::new();
    let mut floor = 0.0f64;
    for item in list {
        let text = item["text"].as_str().unwrap_or("").trim();
        let Some(start) = item["start"].as_str().and_then(parse_clock).or(item["start"].as_f64()) else { continue };
        if text.is_empty() || start > length + 1.0 {
            continue;
        }
        let start = start.max(floor).min(length);
        let end = item["end"].as_str().and_then(parse_clock).or(item["end"].as_f64()).unwrap_or(start + 2.0).max(start + 0.2).min(length.max(start + 0.2));
        floor = start;
        out.push(Segment { start, end, lang: script_lang(text).into(), text: text.to_string() });
    }
    out
}

/// A sentence's end in Latin or Bangla script.
fn sentence_end(word: &str) -> bool {
    word.ends_with(['.', '?', '!', '।'])
}

/// Words with their times (seconds), grouped into subtitle-sized segments: a new one starts after
/// a pause of 0.8 s, after a sentence end once there's 3 s of speech, or at 12 s.
pub fn words_to_segments(words: &[(f64, f64, String)]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut current: Option<(f64, f64, Vec<&str>)> = None;
    let close = |c: (f64, f64, Vec<&str>), out: &mut Vec<Segment>| {
        let text = c.2.join(" ");
        if !text.trim().is_empty() {
            out.push(Segment { start: c.0, end: c.1.max(c.0 + 0.2), lang: script_lang(&text).into(), text });
        }
    };
    for (start, end, text) in words {
        let word = text.trim();
        if word.is_empty() {
            continue;
        }
        if let Some(c) = current.take() {
            let pause = start - c.1 >= 0.8;
            let long = end - c.0 >= 12.0;
            let sentence = c.2.last().is_some_and(|w| sentence_end(w)) && c.1 - c.0 >= 3.0;
            if pause || long || sentence {
                close(c, &mut out);
            } else {
                current = Some(c);
            }
        }
        match &mut current {
            Some(c) => {
                c.1 = *end;
                c.2.push(word);
            }
            None => current = Some((*start, *end, vec![word])),
        }
    }
    if let Some(c) = current {
        close(c, &mut out);
    }
    out
}

/// Text with no times of its own (a model that returns only the transcript of a short piece of
/// audio): one segment per sentence, the piece's time shared out by how long each sentence is.
pub fn split_timed(text: &str, start: f64, end: f64) -> Vec<Segment> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in text.trim().chars() {
        current.push(c);
        if matches!(c, '.' | '?' | '!' | '।') {
            sentences.push(std::mem::take(&mut current));
        }
    }
    sentences.push(current);
    // A piece that is only punctuation (a trailing mark after a space) joins the sentence before it.
    let mut merged: Vec<String> = Vec::new();
    for s in sentences.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        match merged.last_mut() {
            Some(prev) if s.chars().filter(|c| c.is_alphanumeric()).count() == 0 => prev.push_str(&s),
            _ => merged.push(s),
        }
    }
    let total: usize = merged.iter().map(|s| s.chars().count()).sum();
    let span = (end - start).max(0.2);
    let mut at = start;
    let mut out = Vec::new();
    for s in merged {
        let share = span * s.chars().count() as f64 / total.max(1) as f64;
        out.push(Segment { start: at, end: at + share, lang: script_lang(&s).into(), text: s });
        at += share;
    }
    out
}

/// Moves a chunk's segments onto the track's clock.
pub(crate) fn offset(segments: Vec<Segment>, secs: f64) -> Vec<Segment> {
    segments.into_iter().map(|s| Segment { start: s.start + secs, end: s.end + secs, ..s }).collect()
}

/// Shows an audio chunk's place in the whole, for `progress`.
pub(crate) fn fraction(done: usize, total: usize) -> f64 {
    if total == 0 {
        1.0
    } else {
        done as f64 / total as f64
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A stand-in for an API on a local port, as the Statements tests use: answers each request
    //! with the next canned reply and keeps what it was sent.
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    pub fn stand_in(replies: Vec<String>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (stream, reply) in listener.incoming().zip(replies) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push((head, body));
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        (base, seen)
    }

    /// An HTTP reply with a JSON body.
    pub fn ok(body: &serde_json::Value) -> String {
        reply(200, "OK", &body.to_string(), "")
    }

    pub fn reply(status: u16, reason: &str, body: &str, extra: &str) -> String {
        format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}", body.len())
    }

    pub fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    /// Seconds of a quiet tone-free signal with a short burst of "speech" at each given second.
    pub fn audio(secs: f64, bursts: &[f64]) -> Vec<f32> {
        let n = (secs * super::RATE as f64) as usize;
        let mut a = vec![0.0f32; n];
        for b in bursts {
            let from = (b * super::RATE as f64) as usize;
            for (i, s) in a.iter_mut().enumerate().skip(from).take(super::RATE / 2) {
                *s = 0.3 * ((i as f32) * 0.05).sin();
            }
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn providers_have_ids_defaults_and_only_claude_cannot_listen() {
        for p in ALL {
            assert_eq!(Provider::from_id(p.id()), Some(p));
            assert!(!p.default_llm_model().is_empty());
            assert_eq!(p.can_transcribe(), p.default_stt_model().is_some());
        }
        assert!(!Provider::Claude.can_transcribe());
        assert_eq!(Provider::Claude.default_llm_model(), "claude-opus-5-5");
        assert_eq!(Provider::from_id("nope"), None);
    }

    #[test]
    fn model_names_are_checked() {
        assert_eq!(clean_model(" claude-opus-5-5 ").as_deref(), Some("claude-opus-5-5"));
        assert_eq!(clean_model("google/gemini-3.8-flash").as_deref(), Some("google/gemini-3.8-flash"));
        assert_eq!(clean_model("gpt 6"), None);
        assert_eq!(clean_model("a\"b"), None);
        assert_eq!(clean_model(""), None);
    }

    #[test]
    fn the_wav_header_matches_the_samples() {
        let w = wav16(&[0.0, 1.0, -1.0, 0.5]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(w[46..48].try_into().unwrap()), 32767);
        assert_eq!(i16::from_le_bytes(w[48..50].try_into().unwrap()), -32767);
        assert_eq!(w.len(), 44 + 8);
        // 10 minutes is under every provider's 25 MB limit.
        assert!(44 + 10 * 60 * RATE * 2 < 25 * 1024 * 1024);
    }

    #[test]
    fn chunks_cover_everything_once_and_never_exceed_the_limit() {
        let a = testing::audio(100.0, &[10.0, 45.0, 80.0]);
        let chunks = plan_chunks(&a, 30.0, 6.0);
        assert_eq!(chunks.first().unwrap().start, 0);
        assert_eq!(chunks.last().unwrap().end, a.len());
        for w in chunks.windows(2) {
            assert_eq!(w[0].end, w[1].start, "no gap and no overlap");
        }
        assert!(chunks.iter().all(|c| c.len() <= 30 * RATE && !c.is_empty()), "{chunks:?}");
        assert!(chunks.len() >= 4);
        // Short audio is one piece; empty audio has none.
        assert_eq!(plan_chunks(&a[..RATE * 5], 30.0, 6.0), vec![0..RATE * 5]);
        assert!(plan_chunks(&[], 30.0, 6.0).is_empty());
    }

    #[test]
    fn cuts_fall_in_pauses_not_in_speech() {
        // Speech from 20.0 to 20.5 s sits in the last 6 s of the first 24 s window.
        let a = testing::audio(60.0, &[20.0]);
        let chunks = plan_chunks(&a, 24.0, 6.0);
        let cut = chunks[0].end;
        let burst = 20 * RATE..20 * RATE + RATE / 2;
        assert!(!burst.contains(&cut), "cut at {} s is inside the burst", cut as f64 / RATE as f64);
        // Times come from the chunk starts: the second chunk starts exactly where the first ended.
        assert_eq!(chunks[1].start as f64 / RATE as f64, cut as f64 / RATE as f64);
    }

    #[test]
    fn offsets_move_a_chunks_segments_onto_the_tracks_clock() {
        let s = vec![Segment { start: 1.0, end: 2.5, lang: "en".into(), text: "x".into() }];
        let moved = offset(s, 600.0);
        assert_eq!((moved[0].start, moved[0].end), (601.0, 602.5));
    }

    #[test]
    fn silence_is_not_sent() {
        assert!(is_silent(&vec![0.0; 1000]));
        assert!(is_silent(&[0.001, -0.002]));
        assert!(!is_silent(&[0.0, 0.2]));
        assert_eq!(minutes_text(40.0), "~1 minute of audio");
        assert_eq!(minutes_text(3.0 * 3600.0), "~180 minutes of audio");
    }

    #[test]
    fn script_tells_bangla_from_english() {
        assert_eq!(script_lang("আমি কাল ডিজাইন শেষ করব"), "bn");
        assert_eq!(script_lang("I will finish the design"), "en");
        assert_eq!(script_lang("আমরা ship করব on Friday"), "en", "mostly Latin letters");
        assert_eq!(script_lang("123"), "en");
    }

    #[test]
    fn clocks_parse() {
        assert_eq!(parse_clock("1:05"), Some(65.0));
        assert_eq!(parse_clock("01:05.5"), Some(65.5));
        assert_eq!(parse_clock("1:02:03"), Some(3723.0));
        assert_eq!(parse_clock("12.5s"), Some(12.5));
        assert_eq!(parse_clock("0.100s"), Some(0.1));
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(parse_clock("1:2:3:4"), None);
        assert_eq!(parse_clock("-3"), None);
    }

    #[test]
    fn a_models_segments_are_checked_for_order_and_bounds() {
        let answer = json!({"segments": [
            {"start": "00:01", "end": "00:04", "text": "Hello everyone."},
            {"start": "00:03", "end": "00:02", "text": "  Backwards times  "},
            {"start": "99:00", "end": "99:05", "text": "Past the end"},
            {"start": "00:08", "end": "00:09", "text": "   "},
            {"start": "00:10", "end": "00:12", "text": "আমি ডিজাইন শেষ করব"},
            {"start": "oops", "end": "00:12", "text": "No start"}
        ]});
        let got = prompted_segments(&answer, 30.0);
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].start, got[0].end, got[0].lang.as_str()), (1.0, 4.0, "en"));
        assert!(got[1].start >= got[0].start && got[1].end > got[1].start, "moves forward: {:?}", got[1]);
        assert_eq!(got[2].lang, "bn");
        assert!(got.windows(2).all(|w| w[1].start >= w[0].start));
        assert!(got.iter().all(|s| s.end <= 30.0 && s.end > s.start));
        assert!(prompted_segments(&json!({"segments": "no"}), 30.0).is_empty());
    }

    #[test]
    fn words_group_into_subtitle_sized_segments() {
        let w = |s: f64, e: f64, t: &str| (s, e, t.to_string());
        let words = vec![w(0.1, 0.4, "Hello"), w(0.5, 0.8, "world."), w(3.0, 3.3, "Next"), w(3.4, 3.8, "line"), w(4.0, 7.0, "goes"), w(7.1, 7.5, "on."), w(7.6, 8.0, "Done")];
        let segs = words_to_segments(&words);
        let texts: Vec<&str> = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Hello world.", "Next line goes on.", "Done"], "a pause and a sentence end split them");
        assert_eq!(segs[1].start, 3.0);
        assert!(words_to_segments(&[]).is_empty());
        let long: Vec<_> = (0..60).map(|i| w(i as f64 * 0.5, i as f64 * 0.5 + 0.4, "word")).collect();
        assert!(words_to_segments(&long).iter().all(|s| s.end - s.start <= 13.0));
    }

    #[test]
    fn untimed_text_is_split_by_sentence_across_the_pieces_time() {
        let got = split_timed("We ship on Friday. Ayesha will test it.  আমি দেখব।", 10.0, 20.0);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].start, 10.0);
        assert!((got[2].end - 20.0).abs() < 1e-9, "{:?}", got[2]);
        assert!(got.windows(2).all(|w| (w[0].end - w[1].start).abs() < 1e-9 && w[1].start > w[0].start));
        assert_eq!(got[2].lang, "bn");
        assert_eq!(split_timed("no punctuation here", 0.0, 3.0).len(), 1);
        assert!(split_timed("   ", 0.0, 3.0).is_empty());
    }

    #[test]
    fn errors_are_told_plainly_and_never_carry_the_key() {
        let key = "sk-test-SECRET-123456";
        let body = json!({"error": {"message": format!("Incorrect API key provided: {key}. Also {key}"), "type": "x"}}).to_string();
        let (e, retry) = classify(Provider::OpenAi, 400, &body, key);
        assert!(!retry);
        assert!(!e.text().contains("SECRET"), "{}", e.text());
        let (e, _) = classify(Provider::OpenAi, 401, &body, key);
        assert_eq!(e, CloudError::Key(Provider::OpenAi));
        assert!(!e.text().contains("SECRET"));
        assert_eq!(classify(Provider::OpenRouter, 402, "{}", key).0, CloudError::NoCredit(Provider::OpenRouter));
        assert_eq!(classify(Provider::OpenAi, 429, r#"{"error":{"code":"insufficient_quota"}}"#, key), (CloudError::NoCredit(Provider::OpenAi), false));
        assert_eq!(classify(Provider::OpenAi, 429, "{}", key), (CloudError::Busy(Provider::OpenAi), true));
        assert_eq!(classify(Provider::Claude, 529, "{}", key), (CloudError::Busy(Provider::Claude), true));
        assert_eq!(classify(Provider::Claude, 400, r#"{"error":{"message":"Your credit balance is too low to access the API."}}"#, key).0, CloudError::NoCredit(Provider::Claude));
        assert!(classify(Provider::Gemini, 404, r#"{"error":{"message":"models/zzz is not found"}}"#, key).0.text().contains("model name"));
        assert_eq!(redact(&format!("a {key} b"), key), "a … b");
        assert_eq!(CloudError::Cancelled.text(), "Cancelled");
    }
}
