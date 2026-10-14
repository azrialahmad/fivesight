//! Speech-to-text: segment filtering plus the whisper.cpp engine (feature `engine`).

pub mod filter;
pub mod hints;
pub mod lang;

#[cfg(feature = "engine")]
pub mod engine;
