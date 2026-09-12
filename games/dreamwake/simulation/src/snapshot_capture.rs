//! Current committed components shared by replication capture and full checkpoints.
//! The offline combat archive is captured separately and is never a live projection input.
use crate::state::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CapturedState {
    pub collision: crate::collision::CollisionManifest,
    pub run: Run,
    pub heroes: Vec<SavedHero>,
    pub enemies: Vec<SavedEnemy>,
    pub covers: Vec<SavedCover>,
    pub platforms: Vec<SavedPlatform>,
    pub projectiles: Vec<SavedProjectile>,
    pub wisps: Vec<SavedWisp>,
    pub effects: Vec<Effect>,
    pub numbers: Vec<Number>,
    pub delayed: Vec<DelayedCast>,
}
impl CapturedState {
    pub(crate) fn into_saved(
        self,
        combat_history: crate::combat_history::CombatArchive,
    ) -> SavedState {
        SavedState {
            combat_history,
            collision: self.collision,
            run: self.run,
            heroes: self.heroes,
            enemies: self.enemies,
            covers: self.covers,
            platforms: self.platforms,
            projectiles: self.projectiles,
            wisps: self.wisps,
            effects: self.effects,
            numbers: self.numbers,
            delayed: self.delayed,
        }
    }
}

/// Borrow current components without cloning an archive or constructing an incomplete checkpoint.
pub(crate) struct StateView<'a> {
    pub collision: &'a crate::collision::CollisionManifest,
    pub run: &'a Run,
    pub heroes: &'a Vec<SavedHero>,
    pub enemies: &'a Vec<SavedEnemy>,
    pub covers: &'a Vec<SavedCover>,
    pub platforms: &'a Vec<SavedPlatform>,
    pub projectiles: &'a Vec<SavedProjectile>,
    pub wisps: &'a Vec<SavedWisp>,
    pub effects: &'a Vec<Effect>,
    pub numbers: &'a Vec<Number>,
    pub delayed: &'a Vec<DelayedCast>,
}
impl<'a> From<&'a CapturedState> for StateView<'a> {
    fn from(value: &'a CapturedState) -> Self {
        Self {
            collision: &value.collision,
            run: &value.run,
            heroes: &value.heroes,
            enemies: &value.enemies,
            covers: &value.covers,
            platforms: &value.platforms,
            projectiles: &value.projectiles,
            wisps: &value.wisps,
            effects: &value.effects,
            numbers: &value.numbers,
            delayed: &value.delayed,
        }
    }
}
impl<'a> From<&'a SavedState> for StateView<'a> {
    fn from(value: &'a SavedState) -> Self {
        Self {
            collision: &value.collision,
            run: &value.run,
            heroes: &value.heroes,
            enemies: &value.enemies,
            covers: &value.covers,
            platforms: &value.platforms,
            projectiles: &value.projectiles,
            wisps: &value.wisps,
            effects: &value.effects,
            numbers: &value.numbers,
            delayed: &value.delayed,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DreamInput, DreamSimulation, replication::ReplicationStamp};

    fn populated() -> DreamSimulation {
        let mut sim = DreamSimulation::new(8192, false);
        assert!(sim.add_player(2));
        sim.continue_run();
        assert!(sim.continue_run_for(2));
        for _ in 0..40 {
            sim.step_multiplayer(&[(1, DreamInput::default()), (2, DreamInput::default())]);
        }
        sim
    }

    #[test]
    fn current_and_full_captures_share_exact_components_without_archiving_for_replication() {
        let mut sim = populated();
        let snapshot = sim.snapshot();
        assert_eq!(snapshot.state.combat_history.frames.len(), 32);
        assert_eq!(
            sim.capture_current()
                .into_saved(snapshot.state.combat_history.clone()),
            snapshot.state,
        );
        let stamp = ReplicationStamp {
            match_epoch: 1,
            server_tick: 1000,
            gameplay_tick: sim.gameplay_tick(),
            scene_revision: 1,
            revision: 1,
        };
        let before = sim.capture_replication(stamp).unwrap();
        let owner_before = before.owner_checkpoint(1, 1).unwrap();
        // Archive retention is independent of positive live projections. Current
        // components, owner latent state and their validation stay identical.
        *sim.world
            .resource_mut::<crate::combat_history::CombatHistory>() = Default::default();
        let after = sim.capture_replication(stamp).unwrap();
        assert_eq!(before.global(), after.global());
        assert_eq!(owner_before, after.owner_checkpoint(1, 1).unwrap());
        for key in before.keys() {
            assert_eq!(before.position(key), after.position(key));
        }
    }

    #[test]
    fn current_capture_preserves_finite_identity_population_and_latent_validation() {
        let sim = populated();
        let current = sim.capture_current();
        crate::checkpoint::validate_capture(&current).unwrap();
        let mut corrupt = current.clone();
        corrupt.heroes[0].health.hp = f32::NAN;
        assert!(crate::checkpoint::validate_capture(&corrupt).is_err());
        let mut corrupt = current.clone();
        corrupt.enemies[0].view.id = current.heroes[0].view.id;
        assert!(crate::checkpoint::validate_capture(&corrupt).is_err());
        let mut corrupt = current.clone();
        corrupt.enemies[0].ai.home = [f32::INFINITY, 0.0];
        assert!(crate::checkpoint::validate_capture(&corrupt).is_err());
        let mut corrupt = current;
        corrupt.run.rng = 0;
        assert!(crate::checkpoint::validate_capture(&corrupt).is_err());
    }
}
