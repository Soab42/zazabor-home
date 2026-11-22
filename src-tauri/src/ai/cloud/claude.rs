//! Claude (Anthropic) for translation and notes: the Messages API over raw HTTPS, the answer in
//! JSON by structured outputs. Claude takes no audio, so it can't do speech to text.
//!
//! Docs used (read 2026-11-22):
//! - https://platform.claude.com/docs/en/api/messages (POST /v1/messages; headers `x-api-key`,
//!   `anthropic-version: 2026-10-21`; `stop_reason`, `stop_details`; content blocks)
//! - https://platform.claude.com/docs/en/build-with-claude/structured-outputs
//!   (`output_config.format` = `{"type":"json_schema","schema":…}`, the supported schema subset)
//!
//! `thinking` is left out (adaptive is the default on this model), `output_config.effort` is
//! "medium", and a refused request falls back server-side (`fallbacks: "default"` with the beta
//! header, as Statements does). `stop_reason` is always checked before the content is read.

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::{http_client, send, Cloud, CloudError, Provider, Result, Segment};

pub const API: &str = "https://api.anthropic.com";
pub const MODEL: &str = "claude-opus-5-5";
const VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// A non-streaming answer; if it isn't enough the request is made once more with `MORE_TOKENS`.
const MAX_TOKENS: u32 = 16_000;
const MORE_TOKENS: u32 = 32_000;
const ME: Provider = Provider::Claude;

pub struct Claude {
    client: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

/// How a reply ended, read from `stop_reason` before anything else.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// The answer's text (after the last fallback switch).
    Text(String),
    MaxTokens,
    Refusal(String),
}

impl Claude {
    pub fn new(key: String, model: String, base: Option<String>) -> Result<Claude> {
        Ok(Claude { client: http_client(Duration::from_secs(600), ME)?, base: base.unwrap_or_else(|| API.into()), key, model })
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/v1/messages", self.base))
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .header("content-type", "application/json")
            .body(body.to_string())
    }
}

/// The request for one JSON answer.
pub fn body(model: &str, system: &str, prompt: &str, schema: &Value, max_tokens: u32) -> Value {
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "fallbacks": "default",
        "system": system,
        "output_config": { "effort": "medium", "format": { "type": "json_schema", "schema": schema } },
        "messages": [{ "role": "user", "content": prompt }]
    })
}

/// A request that only shows the key works.
pub fn ping_body(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 1024,
        "fallbacks": "default",
        "output_config": { "effort": "low" },
        "messages": [{ "role": "user", "content": "Reply with the single word OK." }]
    })
}

/// Reads a Messages reply: `stop_reason` first, then the text blocks (only those after a
/// `fallback` block, if the first model declined and another took over).
pub fn read(reply: &Value) -> Result<Reply> {
    match reply["stop_reason"].as_str() {
        Some("refusal") => {
            let d = &reply["stop_details"];
            let why = d["explanation"].as_str().or(d["category"].as_str()).unwrap_or("");
            return Ok(Reply::Refusal(why.to_string()));
        }
        Some("max_tokens") | Some("model_context_window_exceeded") => return Ok(Reply::MaxTokens),
        Some("end_turn") | Some("stop_sequence") => {}
        Some(other) => return Err(CloudError::Bad(ME, format!("it stopped early ({other})"))),
        None => return Err(CloudError::Bad(ME, "the reply has no stop reason".into())),
    }
    let Some(blocks) = reply["content"].as_array() else { return Err(CloudError::Bad(ME, "the reply has no content".into())) };
    let mut text = String::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("fallback") => text.clear(),
            Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Err(CloudError::Bad(ME, "the reply has no text".into()));
    }
    Ok(Reply::Text(text))
}

impl Cloud for Claude {
    async fn transcribe(&self, _audio: &[f32], _langs: &[&str], _progress: &(dyn Fn(f64) + Sync), _cancel: &AtomicBool) -> Result<Vec<Segment>> {
        Err(CloudError::Request(ME, "Claude can't listen to audio; choose OpenAI, Gemini or OpenRouter for speech to text".into()))
    }

    async fn complete_json(&self, system: &str, prompt: &str, schema: &Value, cancel: &AtomicBool) -> Result<Value> {
        for max in [MAX_TOKENS, MORE_TOKENS] {
            let request = body(&self.model, system, prompt, schema, max);
            let text = send(ME, &self.key, cancel, || self.post(&request)).await?;
            let reply: Value = serde_json::from_str(&text).map_err(|e| CloudError::Bad(ME, format!("the reply isn't JSON ({e})")))?;
            match read(&reply)? {
                Reply::Text(t) => return serde_json::from_str(t.trim()).map_err(|e| CloudError::Bad(ME, format!("the answer isn't the JSON asked for ({e})"))),
                Reply::Refusal(why) => return Err(CloudError::Refused(ME, why)),
                Reply::MaxTokens => continue,
            }
        }
        Err(CloudError::TooLong(ME))
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

    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": false, "required": ["ok"], "properties": {"ok": {"type": "boolean"}}})
    }

    /// The docs' example reply shape (content, stop_details, stop_reason, usage).
    fn sample(text: &str, stop: &str) -> Value {
        json!({
            "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": text, "citations": null}],
            "stop_details": null, "stop_reason": stop, "stop_sequence": null,
            "usage": {"input_tokens": 2095, "output_tokens": 503}
        })
    }

    #[test]
    fn the_request_follows_the_docs() {
        let b = body("claude-opus-5-5", "SYS", "Hi", &schema(), 16_000);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["max_tokens"], 16_000);
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["system"], "SYS");
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert_eq!(b["output_config"]["format"]["schema"]["required"][0], "ok");
        assert_eq!(b["messages"][0], json!({"role": "user", "content": "Hi"}));
        for banned in ["thinking", "temperature", "top_p", "top_k", "stream"] {
            assert!(b.get(banned).is_none(), "{banned} must not be sent");
        }
    }

    #[test]
    fn the_stop_reason_decides_what_the_reply_means() {
        assert_eq!(read(&sample("{\"ok\":true}", "end_turn")).unwrap(), Reply::Text("{\"ok\":true}".into()));
        assert_eq!(read(&sample("{", "max_tokens")).unwrap(), Reply::MaxTokens);
        assert_eq!(read(&sample("", "model_context_window_exceeded")).unwrap(), Reply::MaxTokens);
        let mut refused = sample("", "refusal");
        refused["stop_details"] = json!({"type": "refusal", "category": "cyber", "explanation": "Not allowed."});
        assert_eq!(read(&refused).unwrap(), Reply::Refusal("Not allowed.".into()));
        refused["stop_details"] = json!({"type": "refusal", "category": "bio", "explanation": null});
        assert_eq!(read(&refused).unwrap(), Reply::Refusal("bio".into()));
        assert!(matches!(read(&sample("x", "pause_turn")), Err(CloudError::Bad(..))));
        assert!(matches!(read(&json!({"content": []})), Err(CloudError::Bad(..))), "no stop reason");
        assert!(matches!(read(&json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": "hm"}]})), Err(CloudError::Bad(..))), "no text block");
    }

    #[test]
    fn the_answer_after_a_fallback_switch_is_the_one_used() {
        let r = json!({"stop_reason": "end_turn", "content": [
            {"type": "text", "text": "{\"partial"}, {"type": "fallback"}, {"type": "thinking", "thinking": "…"}, {"type": "text", "text": "{\"ok\":"}, {"type": "text", "text": "true}"}]});
        assert_eq!(read(&r).unwrap(), Reply::Text("{\"ok\":true}".into()));
    }

    #[test]
    fn claude_cannot_transcribe() {
        let c = Claude::new("k".into(), MODEL.into(), None).unwrap();
        let never = AtomicBool::new(false);
        let r = run(c.transcribe(&[0.0; 10], &["en"], &|_| {}, &never));
        assert!(matches!(r, Err(CloudError::Request(Provider::Claude, _))));
    }

    #[test]
    fn a_json_answer_is_sent_with_the_right_headers_and_parsed() {
        let (base, seen) = stand_in(vec![ok(&sample("{\"ok\":true}", "end_turn"))]);
        let c = Claude::new("sk-ant-test-key-1".into(), MODEL.into(), Some(base)).unwrap();
        let v = run(c.complete_json("sys", "prompt", &schema(), &AtomicBool::new(false))).unwrap();
        assert_eq!(v, json!({"ok": true}));
        let seen = seen.lock().unwrap();
        let head = seen[0].0.to_lowercase();
        assert!(head.starts_with("post /v1/messages"));
        assert!(head.contains("x-api-key: sk-ant-test-key-1"));
        assert!(head.contains("anthropic-version: 2026-10-21"));
        assert!(head.contains("anthropic-beta: server-side-fallback-2026-07-01"));
        let sent: Value = serde_json::from_slice(&seen[0].1).unwrap();
        assert!(!sent.to_string().contains("sk-ant-test-key-1"), "the key goes in a header only");
    }

    #[test]
    fn a_cut_off_answer_is_asked_for_again_with_more_room_and_a_refusal_is_reported() {
        let (base, seen) = stand_in(vec![ok(&sample("{\"ok", "max_tokens")), ok(&sample("{\"ok\":false}", "end_turn"))]);
        let c = Claude::new("k-123456".into(), MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &schema(), &AtomicBool::new(false))).unwrap(), json!({"ok": false}));
        let sent: Vec<Value> = seen.lock().unwrap().iter().map(|(_, b)| serde_json::from_slice(b).unwrap()).collect();
        assert_eq!((sent[0]["max_tokens"].as_u64(), sent[1]["max_tokens"].as_u64()), (Some(16_000), Some(32_000)));

        let mut refused = sample("", "refusal");
        refused["stop_details"] = json!({"type": "refusal", "category": null, "explanation": "No."});
        let (base, _) = stand_in(vec![ok(&refused)]);
        let c = Claude::new("k-123456".into(), MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &schema(), &AtomicBool::new(false))), Err(CloudError::Refused(Provider::Claude, "No.".into())));

        let (base, _) = stand_in(vec![ok(&sample("{", "max_tokens")), ok(&sample("{", "max_tokens"))]);
        let c = Claude::new("k-123456".into(), MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.complete_json("s", "p", &schema(), &AtomicBool::new(false))), Err(CloudError::TooLong(Provider::Claude)));
    }

    #[test]
    fn a_bad_key_and_no_credit_are_told_apart_and_not_retried() {
        let (base, seen) = stand_in(vec![reply(401, "Unauthorized", r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key sk-ant-wrong-0000"}}"#, "")]);
        let c = Claude::new("sk-ant-wrong-0000".into(), MODEL.into(), Some(base)).unwrap();
        let e = run(c.ping(&AtomicBool::new(false))).unwrap_err();
        assert_eq!(e, CloudError::Key(Provider::Claude));
        assert!(!e.text().contains("wrong-0000"));
        assert_eq!(seen.lock().unwrap().len(), 1);
        let (base, _) = stand_in(vec![reply(400, "Bad Request", r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#, "")]);
        let c = Claude::new("k-123456".into(), MODEL.into(), Some(base)).unwrap();
        assert_eq!(run(c.ping(&AtomicBool::new(false))), Err(CloudError::NoCredit(Provider::Claude)));
    }

    #[test]
    fn a_rate_limit_is_retried_after_the_wait_it_asked_for() {
        let (base, seen) = stand_in(vec![reply(429, "Too Many Requests", r#"{"error":{"message":"slow down"}}"#, "retry-after: 0\r\n"), ok(&sample("OK", "end_turn"))]);
        let c = Claude::new("k-123456".into(), MODEL.into(), Some(base)).unwrap();
        run(c.ping(&AtomicBool::new(false))).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 2);
    }
}
