//! Ordered input ingestion, action barriers, and bounded per-tick movement.
use super::*;
impl Simulation {
    pub fn queue_action(&mut self, client_id: u64, action: game_shared::ClientAction) {
        if action.match_epoch != self.match_epoch
            || self.phase != MatchPhase::InProgress
            || matches!(action.command, ClientCommand::Move { .. })
        {
            return;
        }
        let Some(hero) = self.heroes.get_mut(&client_id) else {
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
            .last_action_seq
            .is_some_and(|ack| !is_newer_input_seq(seq, ack))
            || hero.pending_actions.iter().any(|a| a.command.seq() == seq)
            || hero.pending_actions.len() >= 64
        {
            return;
        }
        let index = hero
            .pending_actions
            .iter()
            .position(|a| is_newer_input_seq(a.command.seq(), seq))
            .unwrap_or(hero.pending_actions.len());
        hero.pending_actions.insert(index, action);
    }

    pub(super) fn release_ready_actions(&mut self) {
        for (&id, hero) in &mut self.heroes {
            while let Some(action) = hero.pending_actions.front() {
                let ready = action.after_move_seq.is_none_or(|boundary| {
                    hero.last_processed_seq
                        .is_some_and(|ack| !is_newer_input_seq(boundary, ack))
                });
                if !ready {
                    break;
                }
                let action = hero.pending_actions.pop_front().unwrap();
                self.pending_commands.push((id, action.command));
            }
        }
    }

    pub fn queue_command(&mut self, client_id: u64, command: ClientCommand) {
        if self.phase != MatchPhase::InProgress || !self.heroes.contains_key(&client_id) {
            return;
        }
        if !matches!(command, ClientCommand::Move { .. })
            && self.heroes[&client_id]
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
        if self.pending_commands.iter().any(|(id, pending)| {
            *id == client_id
                && pending.seq() == command.seq()
                && matches!(pending, ClientCommand::Move { .. })
                    == matches!(command, ClientCommand::Move { .. })
        }) {
            return;
        }
        if self
            .pending_commands
            .iter()
            .filter(|(id, _)| *id == client_id)
            .count()
            < 64
        {
            self.pending_commands.push((client_id, command));
        }
    }

    pub(super) fn apply_pending_commands(&mut self, reliable_events: &mut Vec<ReliableGameEvent>) {
        let commands: Vec<(u64, ClientCommand)> = self.pending_commands.drain(..).collect();
        for (client_id, command) in commands {
            if !self.heroes.contains_key(&client_id) {
                continue;
            }

            let seq = command.seq();
            let Some(hero) = self.heroes.get(&client_id) else {
                continue;
            };

            let watermark = if matches!(command, ClientCommand::Move { .. }) {
                hero.last_processed_seq
            } else {
                hero.last_action_seq
            };
            if watermark.is_some_and(|last| !is_newer_input_seq(seq, last)) {
                continue;
            }
            // Reliable actions are ordered only relative to other reliable actions.
            // Never fast-forward movement to catch an action up.
            if !matches!(command, ClientCommand::Move { .. }) {
                self.heroes.get_mut(&client_id).unwrap().last_action_seq = Some(seq);
            }

            match command {
                ClientCommand::Move { dir, .. } => {
                    let queue = self.move_queues.entry(client_id).or_default();
                    // Also reject moves already queued but not yet consumed.
                    // The client re-sends unacked moves in every bundle for
                    // packet-loss resilience; without this check, duplicates
                    // pile up and the queue grows unboundedly.
                    let dominated_by_queue = queue
                        .back()
                        .is_some_and(|(last_q, _)| !is_newer_input_seq(seq, *last_q));
                    if !dominated_by_queue {
                        queue.push_back((seq, normalize_or_zero(dir)));
                        // Cap queue to prevent unbounded growth under extreme jitter.
                        // Drop oldest moves — they're the most stale.
                        while queue.len() > 64 {
                            queue.pop_front();
                        }
                    }
                }
                ClientCommand::SetLockTarget { target_id, .. } => {
                    self.try_set_lock_target(client_id, target_id);
                }
                ClientCommand::SetCharging { active, .. } => {
                    self.try_set_charging(client_id, active);
                }
                ClientCommand::BasicAttack { .. } => {
                    let before = self.heroes[&client_id].regular_attack.phase;
                    self.try_start_regular_attack(client_id);
                    let hero = self.heroes.get_mut(&client_id).unwrap();
                    if before != AttackPhase::Windup
                        && hero.regular_attack.phase == AttackPhase::Windup
                    {
                        hero.last_attack_seq = Some(seq);
                    }
                }
                ClientCommand::CastAbility { ability, .. } => {
                    self.try_cast_ability(client_id, ability, seq, reliable_events);
                }
                ClientCommand::BuildTower {
                    node_id,
                    tower_type,
                    ..
                } => {
                    self.try_build_tower(client_id, node_id, tower_type, reliable_events);
                }
            }
        }
    }

    pub(super) fn consume_move_queues(&mut self) {
        let client_ids: Vec<u64> = self.move_queues.keys().copied().collect();
        for client_id in client_ids {
            // Pop exactly one move per tick (separate borrow from heroes)
            let consumed = {
                let Some(queue) = self.move_queues.get_mut(&client_id) else {
                    continue;
                };
                queue.pop_front()
            };

            let Some(hero) = self.heroes.get_mut(&client_id) else {
                continue;
            };

            if let Some((seq, dir)) = consumed {
                hero.move_dir = dir;
                hero.last_processed_seq = Some(seq);
            } else {
                // No input this tick — stop (don't replay stale direction)
                hero.move_dir = [0.0, 0.0];
            }
        }
    }
}

impl Simulation {
    /// Server-only context, consumed at the next step; never replicated or replayed.
    pub fn set_historical_targets(&mut self, client: u64, seq: u32, targets: Vec<(u64, [f32; 2])>) {
        self.historical_targets.insert((client, seq), targets);
    }

    pub fn ready_actions(&self) -> Vec<(u64, game_shared::ClientAction)> {
        let mut ready = Vec::new();
        for (&id, hero) in &self.heroes {
            for action in &hero.pending_actions {
                if !action.after_move_seq.is_none_or(|boundary| {
                    hero.last_processed_seq
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
