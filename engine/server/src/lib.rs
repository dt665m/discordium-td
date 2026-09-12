//! Game-neutral server configuration, transport and Bevy plugins.
pub mod conditioner;
mod config;
pub mod http_api;
mod net;
pub mod plugins;

pub use config::{ServerConfig, ServerEgressArgs, start_development, start_with_admission};
pub use net::{
    BootstrapLifecycle, PeerEgressStats, PublicationBudget, ServerEgressStats, SharedNet,
};
pub use plugins::server::{
    Authority, ServerDriver, ServerElapsed, ServerFailure, ServerPlugin, ServerSystems, run_app,
};
mod health;
pub use health::{HealthReport, InstanceHealth, InstanceStatus};
