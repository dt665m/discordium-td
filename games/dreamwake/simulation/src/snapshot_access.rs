//! Small authority reads and recipient selection reuse an existing world capture.
use super::{DreamSimulation, DreamSnapshot, RunPhase, state::Run};

impl DreamSimulation {
    /// Menu validation does not need to clone and sort the simulation world.
    pub fn phase(&self) -> RunPhase {
        self.world.resource::<Run>().phase
    }

    pub fn is_paused(&self) -> bool {
        self.world.resource::<Run>().paused
    }
}

impl DreamSnapshot {
    /// Select an admitted recipient without capturing the world again. The saved
    /// heroes are sorted by identity when captured, and remain unchanged here.
    /// A missing player leaves this snapshot untouched and must not be sent as if
    /// it belonged to that player.
    pub fn select_player(&mut self, player_id: u64) -> bool {
        let Ok(index) = self
            .state
            .heroes
            .binary_search_by_key(&player_id, |hero| hero.view.id)
        else {
            return false;
        };
        let local = &self.state.heroes[index];
        self.hero = local.snapshot();
        self.ready = local.ready;
        self.awaiting_party = local.ready
            && matches!(
                self.phase,
                RunPhase::Intro | RunPhase::Reward | RunPhase::Rest | RunPhase::Transition
            );
        self.rewards.clone_from(&local.rewards);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryKind, Rarity, Reward, RewardKind, state::HeroActor};

    #[test]
    fn one_capture_keeps_eight_recipient_views_and_saved_state_exact() {
        let mut simulation = DreamSimulation::new(42, false);
        let ids = [1, 22, 65, 97, 111, 203, 904, 1024];
        for id in ids.into_iter().skip(1) {
            assert!(simulation.add_player(id));
        }
        // Per-player fields must follow the selected identity even though every
        // recipient shares one capture of enemies and rollback state.
        for (index, mut hero) in simulation
            .world
            .query::<HeroActor>()
            .iter_mut(&mut simulation.world)
            .enumerate()
        {
            hero.view.shards = index as u32 * 17;
            hero.health.hp -= index as f32;
            hero.ready = index % 2 == 0;
            hero.rewards = vec![Reward {
                kind: RewardKind::Memory(MemoryKind::Wisp),
                rarity: Rarity::Rare,
                title: format!("Recipient {index}"),
                description: "Individual reward".into(),
            }];
        }
        let mut shared = simulation.snapshot();
        let state = shared.state.clone();
        for id in ids.into_iter().rev() {
            assert!(shared.select_player(id));
            assert_eq!(shared, simulation.snapshot_for(id));
            assert_eq!(shared.state, state);
        }
        let restored = DreamSimulation::from_snapshot(&shared);
        assert_eq!(restored.snapshot_for(203), simulation.snapshot_for(203));
    }

    #[test]
    fn missing_recipient_never_reuses_another_players_view() {
        let mut simulation = DreamSimulation::new(42, false);
        assert!(simulation.add_player(22));
        assert!(simulation.remove_player(22));
        let mut snapshot = simulation.snapshot();
        let before = snapshot.clone();
        assert!(!snapshot.select_player(22));
        assert_eq!(snapshot, before);
        simulation.restart_party(19, true);
        assert!(!simulation.snapshot().select_player(22));
    }

    #[test]
    fn authority_status_reads_follow_pause_restore_and_restart() {
        let mut simulation = DreamSimulation::new(42, false);
        assert_eq!(simulation.phase(), RunPhase::Intro);
        assert!(!simulation.is_paused());
        simulation.continue_run();
        assert_eq!(simulation.phase(), RunPhase::Combat);
        simulation.set_paused(true);
        assert!(simulation.is_paused());
        let paused = simulation.snapshot();
        simulation.restart_party(19, true);
        assert_eq!(simulation.phase(), RunPhase::Intro);
        assert!(!simulation.is_paused());
        simulation.restore(&paused);
        assert_eq!(simulation.phase(), paused.phase);
        assert_eq!(simulation.is_paused(), paused.paused);
    }
}
