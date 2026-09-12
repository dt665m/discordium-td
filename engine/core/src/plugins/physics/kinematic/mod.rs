//! Shared upright capsule motion on a versioned static collision scene.
//!
//! `advance_kinematic` is the single state writer for this capability. Its fixed
//! duration, acceleration, braking, gravity, stance, dash and jump rules are game
//! data. Parry 0.30.2 performs contacts and continuous shape casts; this adapter
//! owns bounded work, stable collider ordering and transactional error handling.
//! Geometry is immutable world space (+Y up, -Z forward), never render transforms.
//! `CollisionScene::sphere_cast` exposes the prepared geometry for pure swept
//! queries, with a caller-owned aggregate `SceneQueryBudget` and nearest stable
//! collider hits. Initial and terminal contact are included; no motor is stepped.
//!
//! The first adapter accepts at most 1024 validated convex map primitives and
//! scans that bounded set per query. It does not claim a large-world broadphase,
//! player collision prediction, dynamic solver restore or cross-platform
//! floating-point equivalence. Games must checkpoint
//! config/state and retain the exact collision revision for replay. A scene
//! revision mismatch requires an explicit authoritative transition or restore.
//! `advance_kinematic_with_bases` additionally consumes exact consecutive-tick
//! support poses. It sweeps base carry before intent, retains local attachment
//! anchors and converts velocity once on dismount/parent change. Each supplied
//! end pose must match the frozen collision scene; history is game-owned and
//! must remain available through replay. Other moving objects are frozen at the
//! supplied end tick, so this is not a general dynamic collision solver.
//!
//! `KinematicPlugin(schedule)` is opt-in and exposes `KinematicStep` for ordering
//! after input/dependency installation and before combat. The legacy arena
//! `MotorState`/`PhysicsPlugin` remain independent. Query failures preserve actor
//! state and surface in `KinematicStatus`; games decide recovery policy.
mod bases;
#[cfg(test)]
mod bases_tests;
mod controller;
mod model;
mod scene;
#[cfg(test)]
mod tests;
pub use bases::*;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
pub use controller::{
    advance_kinematic, advance_kinematic_with_authored_motion, advance_kinematic_with_bases,
    validate_kinematic_state,
};
pub use model::*;
pub(crate) use scene::prepare_collider;
pub use scene::{
    CollisionScene, CollisionShape, MAX_COLLIDERS, MAX_SCENE_QUERY_RESULTS, MAX_SCENE_QUERY_TESTS,
    SceneCastHit, SceneQueryBudget, StaticCollider,
};

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct KinematicStep;
pub struct KinematicPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for KinematicPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), step_kinematic.in_set(KinematicStep));
    }
}
pub fn step_kinematic(
    scene: Res<CollisionScene>,
    mut actors: Query<(
        &mut KinematicState,
        &KinematicConfig,
        &KinematicInput,
        &mut KinematicStatus,
    )>,
) {
    for (mut state, config, input, mut status) in &mut actors {
        let mut next = *state;
        let result = advance_kinematic(&mut next, config, *input, &scene);
        let next_status = match result {
            Ok(report) => {
                state.set_if_neq(next);
                KinematicStatus {
                    report,
                    last_error: None,
                }
            }
            Err(error) => KinematicStatus {
                report: Default::default(),
                last_error: Some(error),
            },
        };
        status.set_if_neq(next_status);
    }
}
