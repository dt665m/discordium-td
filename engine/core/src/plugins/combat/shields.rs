use super::effects::nonnegative;
use super::status::duration;
use super::{CombatState, DurationPolicy};

#[derive(Debug, Clone, Copy)]
pub enum ShieldPolicy {
    Replace,
    Add,
    AddCapped(f32),
}

impl CombatState {
    pub fn grant_shield(
        &mut self,
        amount: f32,
        seconds: f32,
        amount_policy: ShieldPolicy,
        duration_policy: DurationPolicy,
    ) {
        let amount = nonnegative(amount);
        self.shield = match amount_policy {
            ShieldPolicy::Replace => amount,
            ShieldPolicy::Add => (nonnegative(self.shield) + amount).min(f32::MAX),
            ShieldPolicy::AddCapped(cap) => {
                (nonnegative(self.shield) + amount).min(nonnegative(cap))
            }
        };
        self.shield_remaining = duration(self.shield_remaining, seconds, duration_policy);
        if self.shield_remaining == 0.0 {
            self.shield = 0.0;
        }
    }

    /// Returns the absorbed portion of the supplied damage.
    pub fn absorb_damage(&mut self, amount: f32) -> f32 {
        if self.shield_remaining <= 0.0 {
            self.shield = 0.0;
        }
        let absorbed = nonnegative(self.shield).min(amount.max(0.0));
        self.shield -= absorbed;
        absorbed
    }
}
