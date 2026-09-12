pub(crate) mod capture;
mod charge;
use bevy::{input::InputSystems, prelude::*};
use dreamwake_sim::DreamInput;
#[derive(Resource, Default)]
pub(crate) struct CapturedInput(pub(crate) DreamInput);

/// Button edges retain the combat view observed at hardware capture.
#[derive(Resource, Default)]
pub(crate) struct CapturedActions(
    pub(crate) engine_net::codec::BoundedVec<dreamwake_protocol::DreamAction, 8>,
);

/// Latest held beam aim and its capture-time view, plus previous button state.
#[derive(Resource, Default)]
pub(crate) struct CapturedBeam(
    pub(crate) Option<dreamwake_protocol::live::BeamAimSample>,
    bool,
);

#[derive(Resource, Default)]
pub(crate) struct CapturedCharge(bool, charge::ChargeEdges);

/// Input stages all run in PreUpdate, before the fixed prediction loop.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DreamInputSystems {
    Receive,
    Keyboard,
    Buttons,
    Apply,
    Capture,
}

pub(crate) fn configure_input_schedule(app: &mut App) {
    // In Bevy 0.19 UI focus and input capture both run in PreUpdate. Explicitly
    // consume current Interaction before routing clicks or suppressing attacks.
    app.configure_sets(
        PreUpdate,
        (
            DreamInputSystems::Receive,
            DreamInputSystems::Keyboard.after(InputSystems),
            DreamInputSystems::Buttons.after(bevy::ui::UiSystems::Focus),
            DreamInputSystems::Apply,
            DreamInputSystems::Capture,
        )
            .chain(),
    );
}
pub struct DreamInputPlugin;
impl Plugin for DreamInputPlugin {
    fn build(&self, app: &mut App) {
        configure_input_schedule(app);
        app.init_resource::<CapturedInput>()
            .init_resource::<CapturedActions>()
            .init_resource::<CapturedBeam>()
            .init_resource::<CapturedCharge>()
            .add_systems(
                PreUpdate,
                (
                    capture::keyboard_actions.in_set(DreamInputSystems::Keyboard),
                    capture::apply_ui_actions.in_set(DreamInputSystems::Apply),
                    capture::capture_input.in_set(DreamInputSystems::Capture),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DreamConnection, DreamPreferences, DreamView, plugins::diagnostics::Playtest};

    fn capture_app() -> App {
        let mut view =
            crate::offline_presentation(dreamwake_sim::DreamSimulation::new(7, false).snapshot());
        view.phase = dreamwake_sim::RunPhase::Combat;
        let mut app = App::new();
        app.insert_resource(DreamView(view))
            .init_resource::<CapturedInput>()
            .init_resource::<CapturedActions>()
            .init_resource::<CapturedBeam>()
            .init_resource::<CapturedCharge>()
            .init_resource::<DreamConnection>()
            .init_resource::<DreamPreferences>()
            .init_resource::<Playtest>()
            .init_resource::<Time<Real>>()
            .add_message::<bevy::input::keyboard::KeyboardInput>()
            .add_message::<bevy::input::gamepad::GamepadEvent>()
            .add_message::<bevy::input::keyboard::KeyboardFocusLost>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(
                PreUpdate,
                capture::capture_input.in_set(DreamInputSystems::Capture),
            );
        app
    }

    #[test]
    fn pause_stops_a_held_beam_even_when_action_capture_is_full() {
        let mut app = capture_app();
        app.world_mut().resource_mut::<CapturedBeam>().1 = true;
        app.world_mut().resource_mut::<CapturedActions>().0 =
            engine_net::codec::BoundedVec::new(vec![
                dreamwake_protocol::DreamAction::Dash {
                    direction: [1.0, 0.0]
                };
                8
            ])
            .unwrap();
        app.world_mut().resource_mut::<DreamPreferences>().paused = true;
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world().resource::<CapturedActions>().0.as_slice(),
            &[dreamwake_protocol::DreamAction::BeamStop]
        );
        assert!(!app.world().resource::<CapturedBeam>().1);
        assert!(app.world().resource::<CapturedBeam>().0.is_none());
        // The network journal consumes this frame's stop once.
        app.world_mut().resource_mut::<CapturedActions>().0 = Default::default();
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<CapturedActions>().0.is_empty());
    }

    #[test]
    fn compensated_hardware_edge_without_coherent_view_never_invents_a_stamp() {
        let mut app = capture_app();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<CapturedActions>().0.is_empty());
        assert!(
            app.world()
                .resource::<DreamConnection>()
                .status
                .contains("coherent view")
        );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.world_mut()
            .resource_mut::<DreamConnection>()
            .status
            .clear();
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<CapturedActions>().0.is_empty());
        assert!(app.world().resource::<DreamConnection>().status.is_empty());
    }
    #[test]
    fn charge_controls_do_not_cast_or_dash_and_pause_cancels_once() {
        let mut app = capture_app();
        key(&mut app, bevy::input::ButtonState::Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world().resource::<CapturedActions>().0.as_slice(),
            &[dreamwake_protocol::DreamAction::ChargeBegin]
        );
        assert!(!app.world().resource::<CapturedInput>().0.dash);
        assert_eq!(app.world().resource::<CapturedInput>().0.casts, [false; 4]);
        app.world_mut().resource_mut::<CapturedActions>().0 = Default::default();
        app.world_mut().resource_mut::<DreamPreferences>().paused = true;
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            app.world().resource::<CapturedActions>().0.as_slice(),
            &[dreamwake_protocol::DreamAction::ChargeCancel { episode: 0 }]
        );
        app.world_mut().resource_mut::<CapturedActions>().0 = Default::default();
        app.world_mut().run_schedule(PreUpdate);
        assert!(app.world().resource::<CapturedActions>().0.is_empty());
    }
    fn key(app: &mut App, state: bevy::input::ButtonState, repeat: bool) {
        app.world_mut()
            .write_message(bevy::input::keyboard::KeyboardInput {
                key_code: KeyCode::KeyG,
                logical_key: bevy::input::keyboard::Key::Character("g".into()),
                state,
                text: None,
                repeat,
                window: Entity::PLACEHOLDER,
            });
    }
    fn take_charge(app: &mut App) -> Vec<dreamwake_protocol::DreamAction> {
        std::mem::take(&mut app.world_mut().resource_mut::<CapturedActions>().0)
            .as_slice()
            .to_vec()
    }
    fn raw_app() -> App {
        let mut app = capture_app();
        app.add_plugins(bevy::input::InputPlugin);
        app.configure_sets(PreUpdate, DreamInputSystems::Capture.after(InputSystems));
        app
    }
    #[test]
    fn production_charge_preserves_keyboard_tap_restart_and_ignores_repeat() {
        use bevy::input::ButtonState::{Pressed, Released};
        use dreamwake_protocol::DreamAction::{ChargeBegin, ChargeRelease};
        let mut app = raw_app();
        key(&mut app, Pressed, false);
        key(&mut app, Released, false);
        app.world_mut().run_schedule(PreUpdate);
        assert!(
            !app.world()
                .resource::<ButtonInput<KeyCode>>()
                .pressed(KeyCode::KeyG)
        );
        assert_eq!(
            take_charge(&mut app),
            vec![ChargeBegin, ChargeRelease { episode: 0 }]
        );
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
        key(&mut app, Released, false);
        key(&mut app, Pressed, false);
        key(&mut app, Pressed, true);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            take_charge(&mut app),
            vec![ChargeRelease { episode: 0 }, ChargeBegin]
        );
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
    }
    fn pad(app: &mut App, entity: Entity, value: f32) {
        app.world_mut()
            .write_message(bevy::input::gamepad::RawGamepadEvent::Button(
                bevy::input::gamepad::RawGamepadButtonChangedEvent {
                    gamepad: entity,
                    button: GamepadButton::LeftThumb,
                    value,
                },
            ));
    }
    #[test]
    fn production_charge_preserves_gamepad_order_and_aggregates_sources() {
        use bevy::input::ButtonState::{Pressed, Released};
        use dreamwake_protocol::DreamAction::{ChargeBegin, ChargeCancel, ChargeRelease};
        let mut app = raw_app();
        let first = app.world_mut().spawn(Gamepad::default()).id();
        let second = app.world_mut().spawn(Gamepad::default()).id();
        pad(&mut app, first, 1.0);
        pad(&mut app, first, 0.0);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            take_charge(&mut app),
            vec![ChargeBegin, ChargeRelease { episode: 0 }]
        );
        pad(&mut app, first, 1.0);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
        pad(&mut app, first, 0.0);
        pad(&mut app, first, 1.0);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            take_charge(&mut app),
            vec![ChargeRelease { episode: 0 }, ChargeBegin]
        );
        key(&mut app, Pressed, false);
        pad(&mut app, second, 1.0);
        pad(&mut app, first, 0.0);
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
        key(&mut app, Released, false);
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
        let disconnect = bevy::input::gamepad::GamepadConnectionEvent::new(
            second,
            bevy::input::gamepad::GamepadConnection::Disconnected,
        );
        app.world_mut().write_message(disconnect.clone());
        app.world_mut()
            .write_message(bevy::input::gamepad::RawGamepadEvent::Connection(
                disconnect,
            ));
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeCancel { episode: 0 }]);
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
    }
    #[test]
    fn production_charge_retains_backpressured_edges_and_recovers_bounded_overflow() {
        use bevy::input::ButtonState::{Pressed, Released};
        use dreamwake_protocol::DreamAction::{ChargeBegin, ChargeCancel, ChargeRelease, Dash};
        let mut app = raw_app();
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
        app.world_mut().resource_mut::<CapturedActions>().0 =
            engine_net::codec::BoundedVec::new(vec![
                Dash {
                    direction: [1.0, 0.0]
                };
                8
            ])
            .unwrap();
        key(&mut app, Released, false);
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app).len(), 8);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(
            take_charge(&mut app),
            vec![ChargeRelease { episode: 0 }, ChargeBegin]
        );
        for _ in 0..6 {
            key(&mut app, Released, false);
            key(&mut app, Pressed, false);
        }
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeCancel { episode: 0 }]);
        key(&mut app, Pressed, true);
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
        key(&mut app, Released, false);
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
    }
    #[test]
    fn production_charge_focus_and_autoplay_cancel_without_restarting_held_input() {
        use bevy::input::ButtonState::{Pressed, Released};
        use dreamwake_protocol::DreamAction::{ChargeBegin, ChargeCancel};
        let mut app = raw_app();
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
        app.world_mut()
            .write_message(bevy::input::keyboard::KeyboardFocusLost);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeCancel { episode: 0 }]);
        key(&mut app, Pressed, true);
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
        app.world_mut().resource_mut::<Playtest>().autoplay = true;
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeCancel { episode: 0 }]);
        app.world_mut().resource_mut::<Playtest>().autoplay = false;
        app.world_mut().run_schedule(PreUpdate);
        assert!(take_charge(&mut app).is_empty());
        key(&mut app, Released, false);
        key(&mut app, Pressed, false);
        app.world_mut().run_schedule(PreUpdate);
        assert_eq!(take_charge(&mut app), vec![ChargeBegin]);
    }
}
