//! The on-device AI the app shares: Whisper speech to text (Studio's captions, Meet's transcripts)
//! and a small quantized LLM (Meet's notes and translation), both on the CPU with candle. Models
//! are downloaded on first use (`models`), checked against pinned checksums, and nothing leaves
//! the computer.

pub mod llm;
pub mod models;
pub mod sys;
pub mod whisper;
