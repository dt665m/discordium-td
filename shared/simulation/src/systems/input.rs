//! Ingestion is ordered; movement consumption is independent for each hero.
use super::*;
pub(crate) fn ingest(world: &mut World) {
    world.resource_scope(|world, mut globals: Mut<Globals>| {
        globals.release_ready_actions(world);
        let mut events = Vec::new();
        globals.apply_pending_commands(world, &mut events);
        world.resource_mut::<TickEvents>().0.extend(events);
    });
}
pub(crate) fn consume_moves(mut heroes: Query<&mut HeroState>) {
    let update = |mut hero: Mut<'_, HeroState>| {
        while hero.pending_moves.len() > game_shared::MAX_MOVEMENT_BACKLOG {
            hero.pending_moves.pop_front();
        }
        if let Some((seq, dir)) = hero.pending_moves.pop_front() {
            hero.move_dir = dir;
            hero.last_processed_seq = Some(seq);
        } else {
            hero.move_dir = [0.0, 0.0];
        }
    };
    if heroes.iter().len() >= 128 {
        heroes
            .par_iter_mut()
            .batching_strategy(bevy::ecs::batching::BatchingStrategy::new().min_batch_size(128))
            .for_each(update);
    } else {
        heroes.iter_mut().for_each(update);
    }
}

// Ordered input ingestion, action barriers, and bounded per-tick movement.

impl Globals {
    pub(crate) fn queue_action(
        &mut self,
        world: &mut World,
        client_id: u64,
        action: game_shared::ClientAction,
    ) {
        if action.match_epoch != world.resource::<Epoch>().0
            || self.phase != MatchPhase::InProgress
            || matches!(action.command, ClientCommand::Move { .. })
        {
            return;
        }
        let Some(hero) = hero_mut(world, &client_id) else {
            return;
        };
        let seq = action.command.seq();
        if action
            .after_move_seq
            .is_some_and(|boundary| !is_newer_input_seq(seq, boundary))
        {
            return;
        }
        if hero
            .role
            .last_action_seq
            .is_some_and(|ack| !is_newer_input_seq(seq, ack))
            || hero
                .role
                .pending_actions
                .iter()
                .any(|a| a.command.seq() == seq)
            || hero.role.pending_actions.len() >= 64
        {
            return;
        }
        let index = hero
            .role
            .pending_actions
            .iter()
            .position(|a| is_newer_input_seq(a.command.seq(), seq))
            .unwrap_or(hero.role.pending_actions.len());
        hero.role.pending_actions.insert(index, action);
    }

    pub(crate) fn release_ready_actions(&mut self, world: &mut World) {
        let mut released = Vec::new();
        for id in actor_ids::<HeroState>(world) {
            let hero = hero_mut(world, &&id).unwrap();
            while let Some(action) = hero.role.pending_actions.front() {
                let ready = action.after_move_seq.is_none_or(|boundary| {
                    hero.role
                        .last_processed_seq
                        .is_some_and(|ack| !is_newer_input_seq(boundary, ack))
                });
                if !ready {
                    break;
                }
                let action = hero.role.pending_actions.pop_front().unwrap();
                released.push((id, action.command));
            }
        }
        world.resource_mut::<CommandInbox>().0.extend(released);
    }

    pub(crate) fn queue_command(
        &mut self,
        world: &mut World,
        client_id: u64,
        command: ClientCommand,
    ) {
        if self.phase != MatchPhase::InProgress || !storage::hero(world, &client_id).is_some() {
            return;
        }
        if !matches!(command, ClientCommand::Move { .. })
            && storage::hero(world, &client_id)
                .unwrap()
                .role
                .pending_actions
                .iter()
                .any(|a| a.command.seq() == command.seq())
        {
            return;
        }
        if let ClientCommand::Move { dir, .. } = command {
            if !dir.iter().all(|v| v.is_finite()) {
                return;
            }
        }
        if world
            .resource_mut::<CommandInbox>()
            .0
            .iter()
            .any(|(id, pending)| {
                *id == client_id
                    && pending.seq() == command.seq()
                    && matches!(pending, ClientCommand::Move { .. })
                        == matches!(command, ClientCommand::Move { .. })
            })
        {
            return;
        }
        if world
            .resource_mut::<CommandInbox>()
            .0
            .iter()
            .filter(|(id, _)| *id == client_id)
            .count()
            < 64
        {
            world
                .resource_mut::<CommandInbox>()
                .0
                .push((client_id, command));
        }
    }

    pub(crate) fn apply_pending_commands(
        &mut self,
        world: &mut World,
        reliable_events: &mut Vec<ReliableGameEvent>,
    ) {
        let commands: Vec<(u64, ClientCommand)> =
            world.resource_mut::<CommandInbox>().0.drain(..).collect();
        for (client_id, command) in commands {
            if !storage::hero(world, &client_id).is_some() {
                continue;
            }

            let seq = command.seq();
            let Some(hero) = storage::hero(world, &client_id) else {
                continue;
            };

            let watermark = if matches!(command, ClientCommand::Move { .. }) {
                hero.role.last_processed_seq
            } else {
                hero.role.last_action_seq
            };
            if watermark.is_some_and(|last| !is_newer_input_seq(seq, last)) {
                continue;
            }
            // Reliable actions are ordered only relative to other reliable actions.
            // Never fast-forward movement to catch an action up.
            if !matches!(command, ClientCommand::Move { .. }) {
                hero_mut(world, &client_id).unwrap().role.last_action_seq = Some(seq);
            }

            match command {
                ClientCommand::Move { dir, .. } => {
                    let queued_hero = hero_mut(world, &client_id).unwrap();
                    let queue = &mut queued_hero.role.pending_moves;
                    // Also reject moves already queued but not yet consumed.
                    // The client re-sends unacked moves in every bundle for
                    // packet-loss resilience; without this check, duplicates
                    // pile up and the queue grows unboundedly.
                    let dominated_by_queue = queue
                        .back()
                        .is_some_and(|(last_q, _)| !is_newer_input_seq(seq, *last_q));
                    if !dominated_by_queue {
                        queue.push_back((seq, normalize_or_zero(dir)));
                    }
                }
                ClientCommand::SetLockTarget { target_id, .. } => {
                    self.try_set_lock_target(world, client_id, target_id);
                }
                ClientCommand::SetCharging { active, .. } => {
                    self.try_set_charging(world, client_id, active);
                }
                ClientCommand::BasicAttack { .. } => {
                    let before = storage::hero(world, &client_id).unwrap().attack.state.phase;
                    self.try_start_regular_attack(world, client_id);
                    let hero = hero_mut(world, &client_id).unwrap();
                    if before != AttackPhase::Windup
                        && hero.attack.state.phase == AttackPhase::Windup
                    {
                        hero.role.last_attack_seq = Some(seq);
                    }
                }
                ClientCommand::CastAbility { ability, .. } => {
                    self.try_cast_ability(world, client_id, ability, seq, reliable_events);
                }
                ClientCommand::BuildTower {
                    node_id,
                    tower_type,
                    ..
                } => {
                    self.try_build_tower(world, client_id, node_id, tower_type, reliable_events);
                }
            }
        }
    }
}

impl Globals {
    /// Server-only context, consumed at the next step; never replicated or replayed.
    pub(crate) fn set_historical_targets(
        &mut self,
        world: &mut World,
        client: u64,
        seq: u32,
        targets: Vec<(u64, [f32; 2])>,
    ) {
        world
            .resource_mut::<HistoricalTargets>()
            .0
            .insert((client, seq), targets);
    }

    pub(crate) fn ready_actions(&self, world: &World) -> Vec<(u64, game_shared::ClientAction)> {
        let mut ready = Vec::new();
        for (id, hero) in hero_entries(world) {
            for action in &hero.role.pending_actions {
                if !action.after_move_seq.is_none_or(|boundary| {
                    hero.role
                        .last_processed_seq
                        .is_some_and(|ack| !is_newer_input_seq(boundary, ack))
                }) {
                    break;
                }
                ready.push((id, *action));
            }
        }
        ready
    }
}
