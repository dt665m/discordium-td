//! Trusted same-Traveler controller replacement. Authentication remains in the
//! existing session grant path; no client message can authorize replacement.
use super::*;
use engine_net::ownership::OwnershipHandoff;

const MAX_FUTURE_TICKS: u64 = dreamwake_sim::TICK_HZ as u64 * 5;

pub(super) struct PendingHandoff {
    previous: OwnerStream,
    effective: ServerTick,
    replacement: Option<(Peer, Duration)>,
}
impl DreamAuthority {
    pub(crate) fn authorize_handoff(
        &mut self,
        current: u64,
        effective: ServerTick,
    ) -> Result<(), String> {
        if effective <= self.tick || effective.0 - self.tick.0 > MAX_FUTURE_TICKS {
            return Err("Handoff must be within the next five seconds of simulation".into());
        }
        let peer = self
            .peers
            .get(&current)
            .ok_or("Unknown current controller")?;
        if !peer.active || self.handoffs.contains_key(&peer.welcome.player.get()) {
            return Err("Controller not active or handoff already pending".into());
        }
        if self.handoffs.len() >= self.max_clients {
            return Err("Handoff capacity".into());
        }
        self.handoffs.insert(
            peer.welcome.player.get(),
            PendingHandoff {
                previous: peer.welcome.stream,
                effective,
                replacement: None,
            },
        );
        Ok(())
    }
    pub(super) fn stage_handoff(&mut self, id: u64, player: PlayerId, deadline: Duration) -> bool {
        let Some(intent) = self.handoffs.get(&player.get()) else {
            return false;
        };
        if self.tick >= intent.effective
            || deadline <= self.now
            || intent.replacement.is_some()
            || self.peers.contains_key(&id)
            || id == 0
            || self.handoffs.values().any(|pending| {
                pending
                    .replacement
                    .as_ref()
                    .is_some_and(|(peer, _)| peer.welcome.client.0 == id)
            })
        {
            return false;
        }
        let previous = intent.previous;
        let effective = intent.effective;
        if self
            .peers
            .get(&previous.connection.0)
            .is_none_or(|peer| peer.welcome.stream != previous)
        {
            return false;
        }
        let Ok(peer) = self.make_peer(id, player) else {
            return false;
        };
        if OwnershipHandoff::new(previous, peer.welcome.stream, effective, self.tick).is_err() {
            return false;
        }
        self.handoffs.get_mut(&player.get()).unwrap().replacement = Some((peer, deadline));
        true
    }
    pub(super) fn cancel_handoffs_for(&mut self, id: u64) {
        let players: Vec<_> = self
            .handoffs
            .iter()
            .filter_map(|(&player, pending)| {
                (pending.previous.connection.0 == id
                    || pending
                        .replacement
                        .as_ref()
                        .is_some_and(|(peer, _)| peer.welcome.client.0 == id))
                .then_some(player)
            })
            .collect();
        for player in players {
            if let Some((peer, _)) = self
                .handoffs
                .remove(&player)
                .and_then(|pending| pending.replacement)
            {
                if peer.welcome.client.0 != id {
                    self.failed.insert(peer.welcome.client.0);
                }
            }
        }
    }
    pub(super) fn cancel_all_handoffs(&mut self) {
        for pending in std::mem::take(&mut self.handoffs).into_values() {
            if let Some((peer, _)) = pending.replacement {
                self.failed.insert(peer.welcome.client.0);
            }
        }
    }
    pub(super) fn apply_handoffs(&mut self, tick: ServerTick) {
        let due: Vec<_> = self
            .handoffs
            .iter()
            .filter_map(|(&player, pending)| (pending.effective <= tick).then_some(player))
            .collect();
        for player in due {
            let pending = self.handoffs.remove(&player).unwrap();
            let Some((staged, deadline)) = pending.replacement else {
                continue;
            };
            let id = staged.welcome.client.0;
            if pending.effective != tick
                || deadline <= self.now
                || self
                    .peers
                    .get(&pending.previous.connection.0)
                    .is_none_or(|peer| peer.welcome.stream != pending.previous)
            {
                self.failed.insert(id);
                continue;
            }
            // Rebuild the normal bootstrap at the committed pre-cutover tick.
            // The first publication is generated after this tick commits with
            // the new epoch and neutral control until Ready + full decode.
            let mut welcome = staged.welcome;
            welcome.tick = self.tick;
            welcome.server_time_nanos = nanos(self.now);
            let Ok(next) = Peer::new(welcome, self.now) else {
                self.failed.insert(id);
                continue;
            };
            self.outcomes.revoke(player, pending.previous.epoch, tick);
            self.disconnect(pending.previous.connection.0);
            self.failed.insert(pending.previous.connection.0);
            self.peers.insert(id, next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn active() -> DreamAuthority {
        let mut authority = DreamAuthority::new(19, false, 4);
        assert!(authority.admit(41));
        authority.step(Duration::from_millis(17));
        authority.peers.get_mut(&41).unwrap().active = true;
        authority.sim.set_player_active(41, true);
        authority
    }
    #[test]
    fn trusted_handoff_preserves_actor_and_starts_neutral_fresh_bootstrap() {
        let mut authority = active();
        let first = authority.peers[&41].welcome.clone();
        let hp = authority.sim.snapshot_for(41).hero.hp;
        authority.authorize_handoff(41, ServerTick(3)).unwrap();
        assert!(!authority.stage_handoff(42, PlayerId::new(99).unwrap(), Duration::from_secs(1)));
        assert!(authority.stage_handoff(42, first.player, Duration::from_secs(1)));
        assert!(!authority.peers.contains_key(&42));
        let queued = engine_net::commands::Command {
            owner: first.stream,
            sequence: CommandSeq(1),
            target: TargetTick(3),
            input: DreamInput {
                movement: [1.0, 0.0],
                ..Default::default()
            }
            .into(),
            actions: BoundedVec::new(vec![engine_net::commands::ActionEdge {
                slot: 0,
                action: DreamAction::Dash {
                    direction: [1.0, 0.0],
                },
            }])
            .unwrap(),
        };
        let key = queued.action_key(0);
        authority
            .receive_bundle(
                41,
                live::CommandBundle {
                    records: BoundedVec::new(vec![queued]).unwrap(),
                },
            )
            .unwrap();
        assert!(authority.outcomes.lookup(41, key).is_none());
        authority.step(Duration::from_millis(34));
        assert!(authority.peers.contains_key(&41));
        authority.step(Duration::from_millis(51));
        assert!(!authority.peers.contains_key(&41));
        let next = &authority.peers[&42];
        assert_eq!(next.welcome.player, first.player);
        assert_eq!(next.welcome.owner_entity, first.owner_entity);
        assert!(next.welcome.stream.epoch > first.stream.epoch);
        assert!(next.welcome.stream.ownership > first.stream.ownership);
        assert_eq!(next.welcome.tick, ServerTick(2));
        assert_eq!(next.inbox.finalized_through(), ServerTick(3));
        assert!(!next.active && !next.ready && !next.activation_committed);
        assert!(next.welcome_pending && next.frozen.is_none());
        assert!(!authority.sim.player_is_active(41));
        assert_eq!(authority.sim.snapshot_for(41).hero.hp, hp);
        assert!(authority.handoffs.is_empty());
        let verdict = authority.outcomes.lookup(41, key).unwrap();
        assert_eq!(verdict.data.status, live::TerminalStatus::Rejected);
        assert_eq!(verdict.data.reason, live::OutcomeReason::OwnershipDenied);
        assert_eq!(verdict.execution_tick, ServerTick(3));
        authority
            .outcomes
            .revoke(41, first.stream.epoch, ServerTick(4));
        assert_eq!(authority.outcomes.lookup(41, key).unwrap(), verdict);
    }
    #[test]
    fn unfilled_disconnected_expired_and_canceled_intents_keep_current_controller() {
        let mut authority = active();
        assert!(authority.authorize_handoff(41, ServerTick(1)).is_err());
        assert!(authority.authorize_handoff(41, ServerTick(1000)).is_err());
        authority.authorize_handoff(41, ServerTick(2)).unwrap();
        authority.step(Duration::from_millis(34));
        assert!(authority.peers.contains_key(&41));
        assert!(authority.handoffs.is_empty());
        authority.authorize_handoff(41, ServerTick(3)).unwrap();
        assert!(authority.stage_handoff(42, PlayerId::new(41).unwrap(), Duration::from_secs(1)));
        authority.disconnect(42);
        assert!(authority.handoffs.is_empty());
        authority.step(Duration::from_millis(51));
        assert!(authority.peers.contains_key(&41));
        authority.authorize_handoff(41, ServerTick(4)).unwrap();
        assert!(authority.stage_handoff(43, PlayerId::new(41).unwrap(), Duration::from_millis(60)));
        authority.step(Duration::from_millis(68));
        assert!(authority.peers.contains_key(&41));
        assert!(!authority.peers.contains_key(&43));
        assert!(authority.failed.contains(&43));
        assert!(authority.handoffs.is_empty());
        authority.authorize_handoff(41, ServerTick(5)).unwrap();
        assert!(authority.stage_handoff(44, PlayerId::new(41).unwrap(), Duration::from_secs(1)));
        authority.cancel_all_handoffs();
        assert!(authority.failed.contains(&44));
        authority.step(Duration::from_millis(85));
        assert!(authority.peers.contains_key(&41));
        assert!(authority.sim.player_is_active(41));
    }
    #[test]
    fn replacement_requires_existing_authenticated_grant_and_unexpired_deadline() {
        let mut authority = active();
        authority.authorize_handoff(41, ServerTick(3)).unwrap();
        let mut server = RenetServer::new(connection_config());
        assert!(!engine_server::Authority::admit_authenticated(
            &mut authority,
            42,
            None,
            &mut server
        ));
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut application = b"DWID1".to_vec();
        application.extend_from_slice(&41_u64.to_be_bytes());
        let mut grant = renet_cross::SessionGrant {
            protocol_id: wire::PROTOCOL_ID,
            service: wire::SESSION_SERVICE.into(),
            match_id: wire::SESSION_MATCH.into(),
            expires_at: unix.saturating_sub(1),
            replay_key: [1; 32],
            application,
        };
        assert!(!engine_server::Authority::admit_authenticated(
            &mut authority,
            42,
            Some(&grant),
            &mut server
        ));
        grant.expires_at = unix + 60;
        assert!(engine_server::Authority::admit_authenticated(
            &mut authority,
            42,
            Some(&grant),
            &mut server
        ));
        assert!(authority.peers.contains_key(&41));
        assert!(!authority.peers.contains_key(&42));
        assert!(authority.handoffs[&41].replacement.is_some());
    }
    #[test]
    fn stream_recovery_interrupts_charge_without_changing_participation_or_resources() {
        let mut authority = active();
        authority.begin_run();
        let first = authority.peers[&41].welcome.clone();
        let command = engine_net::commands::Command {
            owner: first.stream,
            sequence: CommandSeq(1),
            target: TargetTick(2),
            input: DreamInput {
                aim: [1.0, 0.0],
                ..Default::default()
            }
            .into(),
            actions: BoundedVec::new(vec![engine_net::commands::ActionEdge {
                slot: 0,
                action: DreamAction::ChargeBegin,
            }])
            .unwrap(),
        };
        let key = command.action_key(0);
        authority
            .receive_bundle(
                41,
                live::CommandBundle {
                    records: BoundedVec::new(vec![command]).unwrap(),
                },
            )
            .unwrap();
        authority.step(Duration::from_millis(34));
        authority.step(Duration::from_millis(51));
        let before = authority.sim.snapshot_for(41).hero;
        assert!(before.charge_ticks > 0);
        let terminal = authority.outcomes.lookup(41, key).unwrap();
        assert_eq!(terminal.data.status, live::TerminalStatus::Accepted);
        authority.resync(41).unwrap();
        // Stream recovery queues gameplay interruption for the next committed
        // step; the receive handler never rewrites already committed state.
        assert_eq!(
            authority.sim.snapshot_for(41).hero.charge_ticks,
            before.charge_ticks
        );
        authority.step(Duration::from_millis(68));
        let after = authority.sim.snapshot_for(41).hero;
        assert_eq!(after.charge_ticks, 0);
        assert!(!after.charge_executing);
        assert_eq!(after.stamina, before.stamina);
        assert_eq!(after.charge_cooldown_ticks, before.charge_cooldown_ticks);
        assert_eq!(after.hp, before.hp);
        assert!(authority.sim.player_is_active(41));
        assert!(authority.peers[&41].welcome.stream.epoch > first.stream.epoch);
        assert_eq!(authority.outcomes.lookup(41, key).unwrap(), terminal);
    }
}
