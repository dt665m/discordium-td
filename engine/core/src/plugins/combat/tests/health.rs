use super::super::*;

#[test]
fn zero_capacity_and_saturating_damage_remain_finite_without_implicit_revival() {
    let mut zero = Health::new(0.0);
    assert_eq!(zero.damage(f32::INFINITY), 0.0);
    zero.heal(10.0);
    assert_eq!(zero.hp, 0.0);
    let mut health = Health::new(20.0);
    assert_eq!(health.damage(f32::NAN), 0.0);
    assert_eq!(health.damage(-10.0), 0.0);
    assert_eq!(health.damage(5.0), 5.0);
    health.heal(f32::INFINITY);
    assert_eq!(health.hp, 20.0);
    assert_eq!(health.damage(f32::INFINITY), 20.0);
    health.heal(20.0);
    assert_eq!(health.hp, 0.0);
}

#[test]
fn invalid_capacity_is_rejected_before_it_can_produce_nan_health() {
    for capacity in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(std::panic::catch_unwind(|| Health::new(capacity)).is_err());
    }
}
