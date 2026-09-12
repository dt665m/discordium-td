//! Deterministic presentation events and exact predicted-spawn binding.
//!
//! Games emit a complete corrected journal for the replayed origin-tick range.
//! This layer returns declarative changes; it performs no rendering, audio,
//! inventory transaction or other external side effect. Irreversible gameplay
//! transactions must not be predicted through this presentation API.
mod journal;
mod model;
#[cfg(test)]
mod tests;
pub use journal::EventJournal;
pub use model::*;
