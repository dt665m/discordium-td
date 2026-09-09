//! Game-neutral server configuration, transport and Bevy plugins.
pub mod conditioner;
mod config;
pub mod http_api;
mod net;
pub mod plugins;

pub use config::{ServerConfig, start};
pub use net::SharedNet;
pub use plugins::server::{
    Authority, ServerDriver, ServerElapsed, ServerFailure, ServerPlugin, ServerSystems, run_app,
};
