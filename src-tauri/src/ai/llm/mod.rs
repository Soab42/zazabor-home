//! A small local LLM: Qwen2.5-3B-Instruct (GGUF Q4_K_M) on the CPU with candle. The tokenizer is
//! built from the vocabulary inside the GGUF file, so nothing else is downloaded. Greedy decoding
//! (so a run is repeatable), a stop on the end token or a runaway repeat, cancellable, and it
//! waits while the PC is busy (`sys::wait_while_busy`).

mod qwen2;

use candle_core::quantized::gguf_file;
use candle_core::quantized::tokenizer::TokenizerFromGguf;
use candle_core::{Device, Tensor};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tokenizers::Tokenizer;

pub struct Llm {
    path: String,
    model: qwen2::ModelWeights,
    tok: Tokenizer,
    stops: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Generated {
    pub text: String,
    pub prompt_tokens: usize,
    pub new_tokens: usize,
    pub seconds: f64,
}

fn loaded() -> &'static Mutex<Option<Llm>> {
    static L: OnceLock<Mutex<Option<Llm>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

fn err(e: impl std::fmt::Display) -> String {
    format!("The notes model failed: {e}")
}

fn load(path: &Path) -> Result<Llm, String> {
    let mut file = std::fs::File::open(path).map_err(err)?;
    let content = gguf_file::Content::read(&mut file).map_err(err)?;
    let tok = Tokenizer::from_gguf(&content).map_err(err)?;
    let stops = ["<|im_end|>", "<|endoftext|>"].iter().filter_map(|t| tok.token_to_id(t)).collect();
    let model = qwen2::ModelWeights::from_gguf(content, &mut file, &Device::Cpu).map_err(err)?;
    Ok(Llm { path: path.to_string_lossy().into_owned(), model, tok, stops })
}

/// Runs `f` with the model at `path` (loaded once and kept for the next call), inside the AI
/// thread pool. `f` is how a job asks questions: `llm.chat(...)`.
pub fn with<T: Send>(path: &Path, f: impl FnOnce(&mut Llm) -> Result<T, String> + Send) -> Result<T, String> {
    super::sys::pool().install(|| {
        let mut guard = loaded().lock().unwrap_or_else(|e| e.into_inner());
        let key = path.to_string_lossy();
        if guard.as_ref().is_none_or(|l| l.path != key) {
            *guard = None;
            *guard = Some(load(path)?);
        }
        f(guard.as_mut().expect("just loaded"))
    })
}

/// Lets the loaded model go (about 2.5 GB).
pub fn unload() {
    *loaded().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// A Qwen chat prompt: system, user, and the start of the assistant's turn.
pub fn chat_prompt(system: &str, user: &str) -> String {
    format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n")
}

/// Whether the last tokens are the same few repeated (a loop).
fn looping(tokens: &[u32]) -> bool {
    for period in 1..=8usize {
        let reps = if period == 1 { 24 } else { 8 };
        if tokens.len() >= period * reps {
            let tail = &tokens[tokens.len() - period * reps..];
            if tail.chunks(period).all(|c| c == &tail[..period]) {
                return true;
            }
        }
    }
    false
}

impl Llm {
    /// Answers `user` under `system`, up to `max_new` tokens. `on_wait` is called once when the
    /// work pauses because the PC is busy.
    pub fn chat(&mut self, system: &str, user: &str, max_new: usize, cancel: &AtomicBool, on_wait: &dyn Fn()) -> Result<Generated, String> {
        let clock = std::time::Instant::now();
        let encoding = self.tok.encode(chat_prompt(system, user), false).map_err(err)?;
        let prompt: Vec<u32> = encoding.get_ids().to_vec();
        if prompt.is_empty() || prompt.len() > 12_000 {
            return Err(err("the request is too long"));
        }
        let device = Device::Cpu;
        let mut out: Vec<u32> = Vec::new();
        // The prompt goes in chunks so a long one can be cancelled (and the PC given a rest).
        let mut logits = None;
        let mut pos = 0usize;
        for chunk in prompt.chunks(256) {
            super::sys::wait_while_busy(cancel, on_wait)?;
            let input = Tensor::new(chunk, &device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
            logits = Some(self.model.forward(&input, pos).map_err(err)?);
            pos += chunk.len();
        }
        let mut logits = logits.expect("a prompt has a chunk");
        for step in 0..max_new {
            if step % 16 == 0 {
                super::sys::wait_while_busy(cancel, on_wait)?;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            let row = logits.squeeze(0).and_then(|l| l.to_vec1::<f32>()).map_err(err)?;
            let next = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i as u32);
            if self.stops.contains(&next) {
                break;
            }
            out.push(next);
            if looping(&out) {
                break;
            }
            let input = Tensor::new(&[next], &device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
            logits = self.model.forward(&input, pos).map_err(err)?;
            pos += 1;
        }
        let text = self.tok.decode(&out, true).map_err(err)?;
        Ok(Generated { text: text.trim().to_string(), prompt_tokens: prompt.len(), new_tokens: out.len(), seconds: clock.elapsed().as_secs_f64() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_chatml() {
        let p = chat_prompt("Be brief.", "Hi");
        assert!(p.starts_with("<|im_start|>system\nBe brief.<|im_end|>\n"));
        assert!(p.ends_with("<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n"));
    }

    #[test]
    fn a_runaway_repeat_is_a_loop_but_ordinary_text_isnt() {
        assert!(looping(&[7; 24]));
        let pair: Vec<u32> = (0..8).flat_map(|_| [3, 9]).collect();
        assert!(looping(&pair));
        assert!(!looping(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24]));
    }
}
