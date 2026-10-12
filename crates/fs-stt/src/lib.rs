//! Speech-to-text: segment filtering plus the whisper.cpp engine (feature `engine`).

pub mod filter;

#[cfg(feature = "engine")]
pub mod engine;
