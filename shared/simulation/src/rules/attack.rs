use crate::*;

pub(crate) fn is_attack_locked(state: DirectionalAttackStateComponent) -> bool {
    state.phase != AttackPhase::Ready
}

pub(crate) fn start_attack(
    state: &mut DirectionalAttackStateComponent,
    profile: DirectionalAttackComponent,
) -> bool {
    if is_attack_locked(*state) {
        return false;
    }

    state.phase = AttackPhase::Windup;
    state.ticks_remaining = profile.windup_ticks.max(1);
    true
}

pub(crate) fn advance_attack_state(
    state: &mut DirectionalAttackStateComponent,
    profile: DirectionalAttackComponent,
) -> bool {
    match state.phase {
        AttackPhase::Ready => false,
        AttackPhase::Windup => {
            if state.ticks_remaining > 0 {
                state.ticks_remaining -= 1;
            }

            if state.ticks_remaining == 0 {
                if profile.recovery_ticks == 0 {
                    state.phase = AttackPhase::Ready;
                } else {
                    state.phase = AttackPhase::Recovery;
                    state.ticks_remaining = profile.recovery_ticks;
                }
                return true;
            }

            false
        }
        AttackPhase::Recovery => {
            if state.ticks_remaining > 0 {
                state.ticks_remaining -= 1;
            }
            if state.ticks_remaining == 0 {
                state.phase = AttackPhase::Ready;
            }
            false
        }
    }
}
