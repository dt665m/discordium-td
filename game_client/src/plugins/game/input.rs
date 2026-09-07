//! User input mapping and action capture.
use super::*;

pub(super) fn capture_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut input_state: ResMut<InputState>,
) {
    let mut input = [0.0, 0.0];
    if keyboard.pressed(KeyCode::KeyW) {
        input[1] += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        input[1] -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        input[0] -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        input[0] += 1.0;
    }

    // Top-down camera view is currently mirrored on screen X relative to world X.
    // Flip horizontal input so `A` is screen-left and `D` is screen-right.
    input_state.dir = normalize_or_zero([-input[0], input[1]]);
}

pub(super) fn send_action_commands(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
    world: Res<WorldView>,
    local: Res<LocalSimulation>,
    towers: Query<&TowerBuildNode, With<TowerActor>>,
    mut hud_state: ResMut<HudState>,
    mut pending_actions: ResMut<PendingActions>,
) {
    let Some(mut runtime) = runtime else {
        return;
    };

    if !runtime.renet.is_connected()
        || world.phase != MatchPhase::InProgress
        || !world.heroes.contains_key(&runtime.client_id)
    {
        return;
    }

    // Helper: send action to server AND queue for local sim
    let mut send_and_queue = |runtime: &mut NetworkRuntime, cmd: ClientCommand| {
        if runtime.unacked_actions.len() >= 64 {
            runtime.renet.disconnect();
            return;
        }
        let action = game_shared::ClientAction {
            view_tick: local.initialized.then(|| local.sim.tick()),
            match_epoch: runtime.match_epoch,
            command: cmd,
            after_move_seq: runtime.last_generated_move,
        };
        runtime.unacked_actions.push_back(action);
        runtime
            .renet
            .send_message(DefaultChannel::ReliableOrdered, encode(&action));
        pending_actions.0.push(cmd);
    };

    if keyboard.just_pressed(KeyCode::KeyQ) {
        let Some(hero) = world.heroes.get(&runtime.client_id) else {
            return;
        };

        let next_target = select_next_lock_target(
            hero.lock_target_id,
            hero.pos,
            world.enemies.values().map(|enemy| (enemy.id, enemy.pos)),
        );

        if let Some(target_id) = next_target {
            let seq = runtime.next_command_seq();
            let cmd = ClientCommand::SetLockTarget {
                seq,
                target_id: Some(target_id),
            };
            send_and_queue(&mut runtime, cmd);
            hud_state.last_event = format!("Locked enemy {target_id}");
        } else if hero.lock_mode_active {
            hud_state.last_event = "Lock mode active (no targets)".to_owned();
        } else {
            hud_state.last_event = "No valid lock target".to_owned();
        }
    }

    if keyboard.just_pressed(KeyCode::KeyE) {
        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::SetLockTarget {
            seq,
            target_id: None,
        };
        send_and_queue(&mut runtime, cmd);
        hud_state.last_event = "Lock cleared".to_owned();
    }

    if mouse.just_pressed(MouseButton::Left) || keyboard.just_pressed(KeyCode::KeyJ) {
        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::BasicAttack { seq };
        send_and_queue(&mut runtime, cmd);
    }

    if keyboard.just_pressed(KeyCode::KeyK) {
        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::CastAbility {
            seq,
            ability: AbilityId::ArcBurst,
        };
        send_and_queue(&mut runtime, cmd);
    }

    if keyboard.just_pressed(KeyCode::Space) {
        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::SetCharging { seq, active: true };
        send_and_queue(&mut runtime, cmd);
        hud_state.last_event = "Charge started".to_owned();
    }

    if keyboard.just_released(KeyCode::Space) {
        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::SetCharging { seq, active: false };
        send_and_queue(&mut runtime, cmd);
        hud_state.last_event = "Charge released".to_owned();
    }

    if keyboard.just_pressed(KeyCode::KeyB) {
        let Some(node_id) = pick_nearest_build_node(&world, runtime.client_id, &towers) else {
            hud_state.last_event =
                "No available build node in range. Move closer to a green node.".to_owned();
            return;
        };

        let seq = runtime.next_command_seq();
        let cmd = ClientCommand::BuildTower {
            seq,
            node_id,
            tower_type: TowerType::Arrow,
        };
        send_and_queue(&mut runtime, cmd);
    }
}
