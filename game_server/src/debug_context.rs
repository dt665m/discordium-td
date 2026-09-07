use bevy::prelude::Resource;
use game_shared::DebugServerIdentity;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Resource, Clone)]
pub(crate) struct DebugContext {
    pub identity: DebugServerIdentity,
    started: Instant,
}

impl DebugContext {
    pub fn new() -> Self {
        let process_session = format!("{}-{}", unix_ms(), std::process::id());
        Self {
            identity: DebugServerIdentity {
                realm: std::env::var("TD_REALM_ID").unwrap_or_else(|_| "local".into()),
                instance: std::env::var("TD_INSTANCE_ID")
                    .unwrap_or_else(|_| process_session.clone()),
                process_session,
                build: std::env::var("TD_BUILD_REVISION").unwrap_or_else(|_| {
                    option_env!("TD_BUILD_REVISION").unwrap_or("unknown").into()
                }),
            },
            started: Instant::now(),
        }
    }
    pub fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
