use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use game_shared::{ServerDebugBridgeExport, ServerDebugFrame};

#[derive(Clone, Default)]
pub(crate) struct ServerDebugBridgeHandle {
    inner: Arc<Mutex<ServerDebugBridgeState>>,
}

impl ServerDebugBridgeHandle {
    pub(crate) fn new(history_limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ServerDebugBridgeState {
                history_limit,
                ..Default::default()
            })),
        }
    }

    pub(crate) fn push_frame(&self, frame: ServerDebugFrame) {
        let Ok(mut state) = self.inner.lock() else {
            log::warn!("server debug bridge mutex poisoned while pushing frame");
            return;
        };

        state.latest = Some(frame.clone());
        state.frames.push_back(frame);
        while state.frames.len() > state.history_limit {
            state.frames.pop_front();
        }
    }

    pub(crate) fn export(&self) -> ServerDebugBridgeExport {
        let Ok(state) = self.inner.lock() else {
            log::warn!("server debug bridge mutex poisoned while exporting");
            return ServerDebugBridgeExport {
                enabled: true,
                latest: None,
                frames: Vec::new(),
            };
        };

        ServerDebugBridgeExport {
            enabled: true,
            latest: state.latest.clone(),
            frames: state.frames.iter().cloned().collect(),
        }
    }
}

#[derive(Default)]
struct ServerDebugBridgeState {
    history_limit: usize,
    latest: Option<ServerDebugFrame>,
    frames: VecDeque<ServerDebugFrame>,
}
