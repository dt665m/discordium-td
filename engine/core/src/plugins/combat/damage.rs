use super::effects::nonnegative;
use super::{CombatState, Health};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageOutcome {
    /// Nonnegative damage after the supplied multiplier, before defenses.
    pub incoming: f32,
    pub absorbed: f32,
    pub hp_damage: f32,
    /// True for an invulnerable or already depleted target.
    pub blocked: bool,
}

/// Applies immunity, shield absorption, and HP damage in that order.
/// Games compute critical hits and damage modifiers before calling, and decide
/// subsequent hit effects, immunity grants, lifesteal, and knockback themselves.
/// Invalid or negative inputs deal zero damage; overflow saturates to finite damage.
pub fn resolve_damage(
    health: &mut Health,
    combat: &mut CombatState,
    amount: f32,
    multiplier: f32,
) -> DamageOutcome {
    let incoming = (nonnegative(amount) * nonnegative(multiplier)).min(f32::MAX);
    if health.hp <= 0.0 || combat.is_invulnerable() {
        return DamageOutcome {
            incoming,
            blocked: true,
            ..Default::default()
        };
    }
    let absorbed = combat.absorb_damage(incoming);
    let hp_damage = health.damage(incoming - absorbed);
    DamageOutcome {
        incoming,
        absorbed,
        hp_damage,
        blocked: false,
    }
}
