// Run the documented prototype directly so examples and tests cannot drift.
#[allow(dead_code)]
#[path = "../examples/sandbox.rs"]
mod sandbox;

#[test]
fn unrelated_actor_reuses_headless_plugins_and_simulation_owned_lifetime() {
    // Includes strict ambiguity checking, motion, Timer recharge, health marker
    // synchronization, explicit revival, and stable presentation expiry.
    sandbox::demonstrate();
}
