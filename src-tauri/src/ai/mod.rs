//! The on-device AI the app shares: Whisper speech to text (Studio's captions, Meet's transcripts)
//! and a small quantized LLM (Meet's notes and translation), both on the CPU with candle. Models
//! are downloaded on first use (`models`), checked against pinned checksums, and nothing leaves
//! the computer. `cloud` is the optional second way for Meet: the user's own Claude, OpenAI,
//! Gemini or OpenRouter key, used only when they turned it on.

pub mod cloud;
pub mod llm;
pub mod models;
pub mod sys;
pub mod whisper;
