use super::*;
#[test]
fn bevy_circle_clamps_translated_bounds_and_zero_radius() {
    let bounds = CircleBounds {
        center: [4.0, -2.0],
        radius: 5.0,
    };
    assert_eq!(bounds.clamp([5.0, -1.0]), [5.0, -1.0]);
    assert_eq!(bounds.clamp([7.0, 2.0]), [7.0, 2.0]);
    let clamped = bounds.clamp([10.0, 6.0]);
    assert!(spatial::distance(clamped, [7.0, 2.0]) < 0.00001);
    assert_eq!(
        CircleBounds {
            radius: 0.0,
            ..bounds
        }
        .clamp([10.0, 6.0]),
        bounds.center
    );
}
#[test]
fn displacement_response_supports_immunity_resistance_and_bounds() {
    let mut motor = MotorState {
        position: [1.0, 0.0],
        velocity: [0.0, 3.0],
        ..Default::default()
    };
    motor.displace([4.0, 0.0], 0.0, None);
    assert_eq!(motor.position, [1.0, 0.0]);
    motor.displace([4.0, 0.0], 0.25, None);
    assert_eq!(motor.position, [2.0, 0.0]);
    motor.displace([4.0, 0.0], 1.0, None);
    assert_eq!(motor.position, [6.0, 0.0]);
    motor.displace(
        [4.0, 0.0],
        1.0,
        Some(CircleBounds {
            center: [1.0, 0.0],
            radius: 6.0,
        }),
    );
    assert_eq!(motor.position, [7.0, 0.0]);
    assert_eq!(motor.velocity, [0.0, 3.0]);
}
#[test]
fn movement_normalizes_and_dash_overrides_lock_with_bounds() {
    let mut motor = MotorState::default();
    motor.advance([1.0; 2], [0.0, -1.0], 2.0, None, 1.0);
    assert!((spatial::length(motor.position) - 2.0).abs() < 0.0001);
    motor.position = [0.0; 2];
    motor.movement_lock = 1.0;
    motor.start_dash([1.0, 0.0], 10.0, 0.5, 2.0);
    motor.advance(
        [0.0; 2],
        [0.0; 2],
        2.0,
        Some(CircleBounds {
            center: [0.0; 2],
            radius: 3.0,
        }),
        0.5,
    );
    assert_eq!(motor.position, [3.0, 0.0]);
    motor.tick(0.5);
    motor.advance([1.0; 2], [0.0; 2], 2.0, None, 0.1);
    assert_eq!(motor.velocity, [0.0; 2]);
    assert_eq!(motor.dash_cooldown, 1.5);
}

#[test]
fn spatial_normalization_preserves_the_cross_platform_replay_contract() {
    // Real combat replay vector: native hypot rounded its norm one ULP above
    // WASM, then persisted a different enemy facing on the second tick.
    let direction = spatial::normalize([f32::from_bits(0x4123_d675), f32::from_bits(0xbfd6_8968)]);
    assert_eq!(direction[0].to_bits(), 0x3f7c_a35b);
    assert_eq!(spatial::normalize([0.0, 0.0]), [0.0, 0.0]);
}
