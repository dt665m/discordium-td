//! Preserve edges before ButtonInput's frame-level pressed set loses their order.
use super::{CapturedActions, CapturedCharge};
use bevy::{
    ecs::{message::MessageCursor, system::SystemParam},
    input::{
        ButtonState,
        gamepad::GamepadEvent,
        keyboard::{KeyboardFocusLost, KeyboardInput},
    },
    prelude::*,
};
use dreamwake_protocol::DreamAction;
use std::collections::{BTreeSet, VecDeque};

const MAX_SOURCES: usize = 8;
const MAX_EDGES: usize = 8;

#[derive(Default)]
pub(super) struct ChargeEdges {
    keyboard: bool,
    pads: BTreeSet<Entity>,
    pending: VecDeque<bool>,
    recovering: bool,
    blocked: bool,
    source_overflow: bool,
}
impl ChargeEdges {
    fn held(&self) -> bool {
        self.keyboard || !self.pads.is_empty()
    }
    fn transition(&mut self, was_held: bool, enabled: bool) {
        let held = self.held();
        if enabled && !self.blocked && held != was_held {
            if self.pending.len() == MAX_EDGES {
                self.interrupt();
            } else {
                self.pending.push_back(held);
            }
        }
        if !held && !self.recovering && !self.source_overflow {
            self.blocked = false;
        }
    }
    fn interrupt(&mut self) {
        self.pending.clear();
        self.recovering = true;
        self.blocked = true;
    }
}

/// Bevy publishes separate keyboard and gamepad streams, with no cross-device
/// timestamp. Preserve each stream's order; use keyboard then gamepad order
/// when both arrive in one frame. GamepadEvent also preserves disconnect order
/// and already applies Bevy's button thresholds and filtering.
#[derive(SystemParam)]
pub(crate) struct ChargeInput<'w, 's> {
    keyboard: Option<Res<'w, Messages<KeyboardInput>>>,
    gamepad: Option<Res<'w, Messages<GamepadEvent>>>,
    focus: Option<Res<'w, Messages<KeyboardFocusLost>>>,
    keyboard_cursor: Local<'s, MessageCursor<KeyboardInput>>,
    gamepad_cursor: Local<'s, MessageCursor<GamepadEvent>>,
    focus_cursor: Local<'s, MessageCursor<KeyboardFocusLost>>,
}
impl ChargeInput<'_, '_> {
    pub(super) fn collect(
        &mut self,
        charge: &mut CapturedCharge,
        enabled: bool,
        gamepads: &Query<&Gamepad>,
    ) {
        let edges = &mut charge.1;
        if let Some(messages) = &self.keyboard {
            for event in self.keyboard_cursor.read(messages) {
                if event.key_code != KeyCode::KeyG || event.repeat {
                    continue;
                }
                let before = edges.held();
                edges.keyboard = event.state == ButtonState::Pressed;
                edges.transition(before, enabled);
            }
        }
        if let Some(messages) = &self.gamepad {
            for event in self.gamepad_cursor.read(messages) {
                let before = edges.held();
                match event {
                    GamepadEvent::Button(event) if event.button == GamepadButton::LeftThumb => {
                        if event.state == ButtonState::Pressed {
                            if edges.pads.len() == MAX_SOURCES
                                && !edges.pads.contains(&event.entity)
                            {
                                edges.source_overflow = true;
                                edges.interrupt();
                                continue;
                            }
                            edges.pads.insert(event.entity);
                        } else {
                            edges.pads.remove(&event.entity);
                        }
                    }
                    GamepadEvent::Connection(event) if event.disconnected() => {
                        if edges.pads.remove(&event.gamepad) && !edges.held() {
                            // Disconnect is cancellation, never a charged release.
                            edges.interrupt();
                        }
                    }
                    _ => continue,
                }
                edges.transition(before, enabled);
            }
        }
        // A source beyond the tracking cap cannot count as neutral merely
        // because it was not inserted. Final button state is used only for this
        // recovery barrier, never to reconstruct an edge's order.
        if edges.source_overflow
            && !edges.keyboard
            && gamepads
                .iter()
                .all(|pad| !pad.pressed(GamepadButton::LeftThumb))
        {
            edges.source_overflow = false;
            if !edges.recovering {
                edges.blocked = false;
            }
        }
        // An entity can also disappear without a backend disconnect message.
        let before = edges.held();
        edges.pads.retain(|entity| gamepads.contains(*entity));
        if before && !edges.held() {
            edges.interrupt();
        }
        if self
            .focus
            .as_ref()
            .is_some_and(|messages| self.focus_cursor.read(messages).count() > 0)
        {
            edges.keyboard = false;
            edges.interrupt();
        }
        if !enabled {
            edges.interrupt();
        }
    }
}

impl CapturedCharge {
    pub(super) fn flush(&mut self, actions: &mut CapturedActions) -> bool {
        if self.1.recovering {
            if self.0
                && actions
                    .0
                    .push(DreamAction::ChargeCancel { episode: 0 })
                    .is_err()
            {
                return false;
            }
            self.0 = false;
            self.1.recovering = false;
            if !self.1.held() && !self.1.source_overflow {
                self.1.blocked = false;
            }
        }
        while let Some(held) = self.1.pending.front().copied() {
            let action = if held {
                DreamAction::ChargeBegin
            } else {
                DreamAction::ChargeRelease { episode: 0 }
            };
            if actions.0.push(action).is_err() {
                return false;
            }
            self.1.pending.pop_front();
            self.0 = held;
        }
        true
    }
}
