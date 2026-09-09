use crate::SimulationStep;
use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use serde::{Deserialize, Serialize};

/// A committed action preserves its target until game logic consumes it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionState {
    pub target: [f32; 2],
    pub windup: f32,
    pub recovery: f32,
    pub sequence: u32,
    pub due: bool,
}
impl ActionState {
    pub fn idle(&self) -> bool {
        self.windup <= 0.0 && self.recovery <= 0.0 && !self.due
    }
    pub fn commit(&mut self, target: [f32; 2], windup: f32) -> bool {
        if !self.idle() || !windup.is_finite() || windup < 0.0 {
            return false;
        }
        self.target = target;
        self.windup = windup;
        self.sequence = self.sequence.wrapping_add(1);
        self.due = windup == 0.0;
        true
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        crate::advance_cooldown(&mut self.recovery, dt);
        if self.windup > 0.0 {
            crate::advance_cooldown(&mut self.windup, dt);
            self.due |= self.windup == 0.0;
        }
    }
    pub fn take_due(&mut self) -> bool {
        std::mem::take(&mut self.due)
    }
    pub fn begin_recovery(&mut self, seconds: f32) {
        self.windup = 0.0;
        self.due = false;
        self.recovery = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
    }
    pub fn cancel(&mut self) {
        self.windup = 0.0;
        self.recovery = 0.0;
        self.due = false;
    }
}
pub fn tick_actions(step: Res<SimulationStep>, mut actions: Query<&mut ActionState>) {
    for mut action in &mut actions {
        let mut next = *action;
        next.tick(step.0);
        action.set_if_neq(next);
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActionStep;
pub struct ActionPlugin<S: ScheduleLabel + Clone>(pub S);
impl<S: ScheduleLabel + Clone> Plugin for ActionPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_systems(self.0.clone(), tick_actions.in_set(ActionStep));
    }
}

/// Serializable payloads carry game-selected identity and execution parameters.
/// Readiness is sticky until game logic executes and removes the entity.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelayedAction<T: Send + Sync + 'static> {
    pub remaining: f32,
    pub payload: T,
    pub ready: bool,
}
impl<T: Send + Sync + 'static> DelayedAction<T> {
    pub fn new(delay: f32, payload: T) -> Self {
        let remaining = if delay.is_finite() {
            delay.max(0.0)
        } else {
            0.0
        };
        Self {
            remaining,
            payload,
            ready: remaining == 0.0,
        }
    }
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            return;
        }
        crate::advance_cooldown(&mut self.remaining, dt);
        self.ready |= self.remaining == 0.0;
    }
}
pub fn tick_delayed_actions<T: Send + Sync + 'static>(
    step: Res<SimulationStep>,
    mut actions: Query<&mut DelayedAction<T>>,
) {
    if !step.0.is_finite() || step.0 < 0.0 {
        return;
    }
    for mut action in &mut actions {
        let remaining = (action.remaining - step.0).max(0.0);
        let ready = action.ready || remaining == 0.0;
        if action.remaining != remaining || action.ready != ready {
            action.tick(step.0);
        }
    }
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DelayedActionStep;
pub struct DelayedActionPlugin<T: Send + Sync + 'static, S: ScheduleLabel + Clone> {
    schedule: S,
    marker: std::marker::PhantomData<T>,
}
impl<T: Send + Sync + 'static, S: ScheduleLabel + Clone> DelayedActionPlugin<T, S> {
    pub fn new(schedule: S) -> Self {
        Self {
            schedule,
            marker: std::marker::PhantomData,
        }
    }
}
impl<T: Send + Sync + 'static, S: ScheduleLabel + Clone> Plugin for DelayedActionPlugin<T, S> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            self.schedule.clone(),
            tick_delayed_actions::<T>.in_set(DelayedActionStep),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct TestTick;
    #[test]
    fn action_plugins_advance_component_state_on_selected_schedule() {
        let mut app = App::new();
        app.init_schedule(TestTick)
            .insert_resource(SimulationStep(0.25))
            .add_plugins((
                ActionPlugin(TestTick),
                DelayedActionPlugin::<u64, _>::new(TestTick),
            ));
        let mut action = ActionState::default();
        action.commit([2.0, 3.0], 0.25);
        let actor = app.world_mut().spawn(action).id();
        let delayed = app.world_mut().spawn(DelayedAction::new(0.5, 42_u64)).id();
        app.world_mut().run_schedule(TestTick);
        assert!(app.world().get::<ActionState>(actor).unwrap().due);
        assert!(
            !app.world()
                .get::<DelayedAction<u64>>(delayed)
                .unwrap()
                .ready
        );
        app.world_mut().run_schedule(TestTick);
        assert!(
            app.world()
                .get::<DelayedAction<u64>>(delayed)
                .unwrap()
                .ready
        );
    }
    #[test]
    fn committed_action_is_consumed_once_and_recovers() {
        let mut action = ActionState::default();
        assert!(action.commit([1.0, 2.0], 0.5));
        assert!(!action.commit([9.0; 2], 0.0));
        action.tick(0.5);
        assert_eq!(action.target, [1.0, 2.0]);
        assert!(action.take_due());
        assert!(!action.take_due());
        action.begin_recovery(0.25);
        assert!(!action.idle());
        action.tick(0.25);
        assert!(action.commit([0.0; 2], 0.0));
        assert!(action.take_due());
        action.cancel();
        assert!(action.idle());
        assert_eq!(action.sequence, 2);
    }
    #[test]
    fn delayed_continuation_preserves_payload_and_readiness() {
        let mut original = DelayedAction::new(0.5, (17_u64, [1.0, 2.0]));
        original.tick(0.25);
        let saved = serde_json::to_string(&original).unwrap();
        let mut restored: DelayedAction<(u64, [f32; 2])> = serde_json::from_str(&saved).unwrap();
        original.tick(0.25);
        restored.tick(0.25);
        assert_eq!(original, restored);
        assert!(restored.ready);
        restored.tick(0.25);
        assert!(restored.ready);
        assert_eq!(restored.payload.0, 17);
    }
}
