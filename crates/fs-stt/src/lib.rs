//! Speech-to-text: segment filtering plus the engines (feature `engine`): whisper.cpp, and
//! Qwen3-ASR through llama.cpp's server.

pub mod filter;
pub mod hints;
pub mod lang;

#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "engine")]
pub mod qwen;
