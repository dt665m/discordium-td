//! Reusable server scheduling, polling and graceful shutdown.
mod driver;
mod systems;
#[cfg(test)]
mod tests;

use bevy::prelude::*;
pub use driver::{Authority, ServerDriver};
use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
    time::Duration,
};
use systems::{poll_server, shutdown_server};

/// Optional elapsed-time override for embedded hosts and deterministic tests.
/// Without an override polling uses real elapsed time, unaffected by game pause.
#[derive(Resource, Default)]
pub struct ServerElapsed(pub Option<Duration>);

/// A shared error handle survives `App::run`, which consumes the app's world.
#[derive(Resource, Clone, Default)]
pub struct ServerFailure(Arc<Mutex<Option<String>>>);
impl ServerFailure {
    pub fn error(&self) -> Option<String> {
        self.0.lock().expect("server failure lock poisoned").clone()
    }
    fn record(&self, error: String) {
        log::error!("server runtime: {error}");
        self.0
            .lock()
            .expect("server failure lock poisoned")
            .get_or_insert(error);
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerSystems {
    Poll,
    Shutdown,
}

/// Game-neutral Bevy runtime. The game installs one non-send `ServerDriver<A>`;
/// this plugin owns real-time polling, fixed-tick dispatch, publication, and exit.
/// Non-send storage allows authorities to own a simulation world without cloning it.
pub struct ServerPlugin<A>(PhantomData<fn() -> A>);
impl<A> Default for ServerPlugin<A> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<A: Authority + 'static> Plugin for ServerPlugin<A> {
    fn build(&self, app: &mut App) {
        app.init_resource::<ServerElapsed>()
            .init_resource::<ServerFailure>()
            .add_systems(Update, poll_server::<A>.in_set(ServerSystems::Poll))
            .add_systems(
                Last,
                shutdown_server::<A>
                    .in_set(ServerSystems::Shutdown)
                    .after(bevy::window::ExitSystems),
            );
    }
}

/// Run a composed server app and retain detailed errors after its world is dropped.
pub fn run_app(mut app: App) -> Result<(), String> {
    let failure = app.world().resource::<ServerFailure>().clone();
    let exit = app.run();
    if let Some(error) = failure.error() {
        Err(error)
    } else if exit.is_error() {
        Err(format!("server exited: {exit:?}"))
    } else {
        Ok(())
    }
}
