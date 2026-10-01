//! Client prediction: a rollback predictor that replays retained frames from a
//! corrected tick, or rebases when the correction predates its history.
//!
//! Models must be pure shared-simulation adapters: no hardware polling, wall clock,
//! rendering, transport or irreversible effects during stepping.
pub mod rewind;
