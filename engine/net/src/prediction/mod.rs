//! Bounded scoped prediction with complete checkpoints and transactional replay.
//!
//! Models must be pure shared-simulation adapters: no hardware polling, wall clock,
//! rendering, transport or irreversible effects during stepping. Only an opaque
//! fully decoded replication group can initialize or correct prediction. Complete
//! state and dependency history are independently owned, never replica pointers.
mod history;
mod manager;
#[cfg(test)]
mod tests;
pub use history::*;
pub use manager::*;
