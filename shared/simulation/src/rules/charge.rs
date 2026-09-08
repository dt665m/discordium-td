use crate::*;

pub(crate) fn is_charge_locked(state: ChargeStateComponent) -> bool {
    state.phase != ChargePhase::Idle
}

pub(crate) fn set_charge_input(
    hero: &mut HeroState,
    attack: DirectionalAttackStateComponent,
    active: bool,
) {
    if active {
        if is_attack_locked(attack) || hero.charge_state.power_active {
            return;
        }

        match hero.charge_state.phase {
            ChargePhase::Idle => {
                hero.charge_state.input_held = true;
                let startup_ticks = hero.charge_profile.startup_ticks;
                if startup_ticks == 0 {
                    hero.charge_state.phase = ChargePhase::Charging;
                    hero.charge_state.phase_ticks_remaining = 0;
                } else {
                    hero.charge_state.phase = ChargePhase::Startup;
                    hero.charge_state.phase_ticks_remaining = startup_ticks;
                }
            }
            ChargePhase::Startup | ChargePhase::Charging => {
                hero.charge_state.input_held = true;
            }
            ChargePhase::Recovery => {}
        }
        return;
    }

    hero.charge_state.input_held = false;
    if matches!(
        hero.charge_state.phase,
        ChargePhase::Startup | ChargePhase::Charging
    ) {
        begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
    }
}

pub(crate) fn begin_charge_recovery(
    state: &mut ChargeStateComponent,
    profile: ChargeProfileComponent,
) {
    let release_lag_ticks = profile.release_lag_ticks;
    if release_lag_ticks == 0 {
        state.phase = ChargePhase::Idle;
        state.phase_ticks_remaining = 0;
    } else {
        state.phase = ChargePhase::Recovery;
        state.phase_ticks_remaining = release_lag_ticks;
    }
}

pub(crate) fn advance_charge_state(hero: &mut HeroState, health: &mut Health) {
    match hero.charge_state.phase {
        ChargePhase::Idle => {}
        ChargePhase::Startup => {
            if hero.charge_state.phase_ticks_remaining > 0 {
                hero.charge_state.phase_ticks_remaining -= 1;
            }

            if hero.charge_state.phase_ticks_remaining == 0 {
                if hero.charge_state.input_held {
                    hero.charge_state.phase = ChargePhase::Charging;
                } else {
                    begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                }
            }
        }
        ChargePhase::Charging => {
            if !hero.charge_state.input_held {
                begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                return;
            }

            if hero.mana + f32::EPSILON < HERO_MAX_MANA {
                hero.mana = (hero.mana + hero.charge_profile.mana_per_tick).min(HERO_MAX_MANA);
                return;
            }

            if hero.charge_state.power_meter + f32::EPSILON < hero.charge_profile.power_meter_max {
                let previous_power_meter = hero.charge_state.power_meter;
                hero.charge_state.power_meter = (hero.charge_state.power_meter
                    + hero.charge_profile.power_gain_per_tick)
                    .min(hero.charge_profile.power_meter_max);
                if previous_power_meter <= f32::EPSILON
                    && hero.charge_state.power_meter > f32::EPSILON
                {
                    hero.charge_state.power_decay_ticks_remaining =
                        hero.power_decay_profile.interval_ticks.max(1);
                }
                if hero.charge_state.power_meter + f32::EPSILON
                    < hero.charge_profile.power_meter_max
                {
                    return;
                }
            }

            if hero.charge_state.power_meter + f32::EPSILON >= hero.charge_profile.power_meter_max {
                hero.charge_state.power_meter = hero.charge_profile.power_meter_max;
                hero.charge_state.power_active = true;
                if hero.charge_state.power_decay_ticks_remaining == 0 {
                    hero.charge_state.power_decay_ticks_remaining =
                        hero.power_decay_profile.interval_ticks.max(1);
                }
            }

            if health.hp + f32::EPSILON < HERO_MAX_HP {
                health.hp = (health.hp + hero.charge_profile.health_per_tick).min(HERO_MAX_HP);
                if health.hp + f32::EPSILON < HERO_MAX_HP {
                    return;
                }
            }

            if hero.charge_state.power_active {
                hero.charge_state.input_held = false;
                begin_charge_recovery(&mut hero.charge_state, hero.charge_profile);
                return;
            }
        }
        ChargePhase::Recovery => {
            if hero.charge_state.phase_ticks_remaining > 0 {
                hero.charge_state.phase_ticks_remaining -= 1;
            }

            if hero.charge_state.phase_ticks_remaining == 0 {
                hero.charge_state.phase = ChargePhase::Idle;
            }
        }
    }

    let is_actively_charging = hero.charge_state.phase == ChargePhase::Charging;
    if is_actively_charging || hero.charge_state.power_meter <= f32::EPSILON {
        if hero.charge_state.power_meter <= f32::EPSILON {
            hero.charge_state.power_meter = 0.0;
            hero.charge_state.power_active = false;
            hero.charge_state.power_decay_ticks_remaining = 0;
        }
        return;
    }

    if hero.charge_state.power_decay_ticks_remaining > 0 {
        hero.charge_state.power_decay_ticks_remaining -= 1;
    }

    if hero.charge_state.power_decay_ticks_remaining == 0 {
        hero.charge_state.power_meter =
            (hero.charge_state.power_meter - hero.power_decay_profile.amount_per_interval).max(0.0);
        if hero.charge_state.power_meter <= f32::EPSILON {
            hero.charge_state.power_meter = 0.0;
            hero.charge_state.power_active = false;
            hero.charge_state.power_decay_ticks_remaining = 0;
        } else {
            hero.charge_state.power_decay_ticks_remaining =
                hero.power_decay_profile.interval_ticks.max(1);
        }
    }
}

pub(crate) fn hero_attack_damage_multiplier(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        hero.powered_modifiers.attack_damage_multiplier.max(1.0)
    } else {
        1.0
    }
}

pub(crate) fn hero_ability_radius(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        HERO_ABILITY_RADIUS * hero.powered_modifiers.ability_radius_multiplier.max(0.0)
    } else {
        HERO_ABILITY_RADIUS
    }
}

pub(crate) fn hero_regular_attack_profile(
    hero: &HeroState,
    mut profile: DirectionalAttackComponent,
) -> DirectionalAttackComponent {
    if hero.charge_state.power_active {
        profile.range *= hero
            .powered_modifiers
            .regular_attack_range_multiplier
            .max(0.0);
    }
    profile
}

pub(crate) fn hero_ability_mana_cost(hero: &HeroState) -> f32 {
    if hero.charge_state.power_active {
        HERO_ABILITY_MANA_COST * hero.powered_modifiers.ability_mana_cost_multiplier.max(0.0)
    } else {
        HERO_ABILITY_MANA_COST
    }
}

pub(crate) fn hero_ability_cooldown_ticks(hero: &HeroState) -> u32 {
    let multiplier = if hero.charge_state.power_active {
        hero.powered_modifiers.ability_cooldown_multiplier.max(0.0)
    } else {
        1.0
    };
    let scaled = (HERO_ABILITY_COOLDOWN_TICKS as f32 * multiplier).round();
    if scaled <= 0.0 { 0 } else { scaled as u32 }
}
