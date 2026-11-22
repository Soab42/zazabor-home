//! OpenRouter: one key for many models. Chat Completions (the same shape as OpenAI's, so the
//! request and reply code is shared with `openai.rs`) for translation and notes, and, for models
//! that take audio, the same endpoint with an `input_audio` part for speech to text.
//!
//! Docs used (read 2026-11-22):
//! - https://openrouter.ai/docs/guides/overview/multimodal/audio (audio input: base64 `input_audio`
//!   `{data, format}`; Chat Completions takes any format string the model supports, so `wav`;
//!   models with audio input are listed by https://openrouter.ai/api/v1/models?input_modalities=audio)
//! - https://openrouter.ai/docs/guides/features/structured-outputs (`response_format` =
//!   `{"type":"json_schema","json_schema":{name, strict, schema}}` on /api/v1/chat/completions)
//! - https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request (POST
//!   /api/v1/chat/completions, `Authorization: Bearer`, `max_tokens`, `choices[].finish_reason`)
//! - https://openrouter.ai/docs/api/reference/errors-and-debugging (402 out of credit, 429 and 503
//!   with `Retry-After`)
//!
//! There's no transcription endpoint here, so a model that can hear is asked for strict JSON
//! segments with mm:ss times, which are validated for order. Pick an audio model (the default is
//! `google/gemini-3.8-flash`); a text-only model returns an error from OpenRouter.

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::openai::{chat_body, read_chat};
use super::{b64_wav, fraction, http_client, is_silent, offset, plan_chunks, prompted_segments, segments_schema, send, transcribe_prompt, Cloud, CloudError, Provider, Result, Segment, RATE};

pub const API: &str = "https://openrouter.ai/api";
pub const STT_MODEL: &str = "google/gemini-3.8-flash";
pub const LLM_MODEL: &str = "google/gemini-3.8-flash";
const ME: Provider = Provider::OpenRouter;

pub struct OpenRouter {
    client: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

impl OpenRouter {
    pub fn new(key: String, model: String, base: Option<String>) -> Result<OpenRouter> {
        Ok(OpenRouter { client: http_client(Duration::from_secs(600), ME)?, base: base.unwrap_or_else(|| API.into()), key, model })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client.post(format!("{}/v1/chat/completions", self.base)).bearer_auth(&self.key).header("content-type", "application/json").body(body.to_string())
    }
}

/// The request for a model that hears: the instruction and the audio, the answer as JSON segments.
pub fn audio_body(model: &str, wav_b64: &str, langs: &[&str]) -> Value {
    json!({
        "model": model,
        "max_tokens": 16_000,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": transcribe_prompt(langs) },
                { "type": "input_audio", "input_audio": { "data": wav_b64, "format": "wav" } }
            ]
        }],
        "response_format": { "type": "json_schema", "json_schema": { "name": "segments", "strict": true, "schema": segments_schema() } }
    })
}

impl Cloud for OpenRouter {
    async fn transcribe(&self, audio: &[f32], langs: &[&str], progress: &(dyn Fn(f64) + Sync), cancel: &AtomicBool) -> Result<Vec<Segment>> {
        // Times a model reads off the audio drift over long pieces: five minutes each.
        let chunks = plan_chunks(audio, 300.0, 15.0);
        let mut out = Vec::new();
        for (i, range) in chunks.iter().enumerate() {
            progress(fraction(i, chunks.len()));
            let piece = &audio[range.clone()];
            if is_silent(piece) {
                continue;
            }
            let request = audio_body(&self.model, &b64_wav(piece), langs);
            let text = send(ME, &self.key, cancel, || self.post(&request)).await?;
            let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
            let answer: Value = serde_json::from_str(read_chat(ME, &reply)?.trim()).map_err(|e| CloudError::Bad(ME, format!("the segments aren't the JSON asked for ({e})")))?;
            out.extend(offset(prompted_segments(&answer, piece.len() as f64 / RATE as f64), range.start as f64 / RATE as f64));
        }
        progress(1.0);
        Ok(out)
    }

    async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value> {
        let request = chat_body(&self.model, system, prompt, schema, "max_tokens");
        let text = send(ME, &self.key, cancel, || self.post(&request)).await?;
        let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
        serde_json::from_str(read_chat(ME, &reply)?.trim()).map_err(|e| CloudError::Bad(ME, format!("the answer isn't the JSON asked for ({e})")))
    }

    async fn ping(&self, cancel: &AtomicBool) -> Result<()> {
        let request = json!({ "model": self.model, "max_tokens": 256, "messages": [{ "role": "user", "content": "Reply with the single word OK." }] });
        send(ME, &self.key, cancel, || self.post(&request)).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn chat(text: &str) -> Value {
        json!({"id": "gen-1794950371-66aSkH2jJjcJz83iT5Xk", "model": "google/gemini-3.8-flash", "object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": text}}]})
    }

    #[test]
    fn the_audio_request_follows_the_docs() {
        let b = audio_body("google/gemini-3.8-flash", "QUJD", &["en", "bn"]);
        assert_eq!(b["messages"][0]["content"][1], json!({"type": "input_audio", "input_audio": {"data": "QUJD", "format": "wav"}}));
        assert_eq!(b["messages"][0]["content"][0]["type"], "text");
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
        assert_eq!(b["max_tokens"], 16_000);
    }

    #[test]
    fn audio_goes_to_the_chat_endpoint_with_a_bearer_key_and_the_segments_are_checked() {
        let audio = audio(40.0, &[4.0]);
        let answer = json!({"segments": [{"start": "00:03", "end": "00:06", "text": "Hello team."}, {"start": "00:02", "end": "00:08", "text": "ঠিক আছে।"}]});
        let (base, seen) = stand_in(vec![ok(&chat(&answer.to_string()))]);
        let c = OpenRouter::new("sk-or-v1-test-KEY-123".into(), STT_MODEL.into(), Some(base)).unwrap();
        let segs = run(c.transcribe(&audio, &["en", "bn"], &|_| {}, &AtomicBool::new(false))).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].lang, "bn");
        assert!(segs[1].start >= segs[0].start, "time only moves forward: {segs:?}");
        let seen = seen.lock().unwrap();
        let head = seen[0].0.to_lowercase();
        assert!(head.starts_with("post /v1/chat/completions"));
        assert!(head.contains("authorization: bearer sk-or-v1-test-key-123"));
        assert!(!String::from_utf8_lossy(&seen[0].1).contains("test-KEY"));
    }

    #[test]
    fn json_answers_and_errors() {
        let (base, _) = stand_in(vec![ok(&chat("{\"ok\":true}"))]);
        let c = OpenRouter::new("sk-or-v1-test-KEY-123".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &json!({}), &AtomicBool::new(false))).unwrap(), json!({"ok": true}));
        let (base, _) = stand_in(vec![reply(402, "Payment Required", r#"{"error":{"code":402,"message":"Insufficient credits. Add more using https://openrouter.ai/settings/credits"}}"#, "")]);
        let c = OpenRouter::new("sk-or-v1-test-KEY-123".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.ping(&AtomicBool::new(false))), Err(CloudError::NoCredit(ME)));
        let (base, _) = stand_in(vec![reply(401, "Unauthorized", r#"{"error":{"code":401,"message":"No auth credentials found"}}"#, "")]);
        let c = OpenRouter::new("sk-or-v1-test-KEY-123".into(), LLM_MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.ping(&AtomicBool::new(false))), Err(CloudError::Key(ME)));
        // A 200 that carries an error instead of choices.
        let (base, _) = stand_in(vec![ok(&json!({"error": {"code": 400, "message": "This model does not support audio input"}}))]);
        let c = OpenRouter::new("sk-or-v1-test-KEY-123".into(), "openai/gpt-6.2-sol".into(), Some(base)).unwrap();
        let e = run(c.complete_json("s", "p", &json!({}), &AtomicBool::new(false))).unwrap_err();
        assert!(matches!(e, CloudError::Bad(ME, ref m) if m.contains("audio")), "{e:?}");
    }
}
