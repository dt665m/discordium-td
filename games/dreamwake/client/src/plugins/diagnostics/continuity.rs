//! Player-facing session health, measured from what is actually displayed.
//!
//! These counters judge the network layer by its visible outcome: does the
//! owner's character move continuously, and does the session stay connected?
//! They read only the display contract, [`DreamView`] and [`DreamConnection`],
//! so they stay meaningful however the layer underneath is built, and nothing
//! resets them on reconnect. Internal invariants can all hold while the player
//! watches their character snap across the room; this is what catches that.
use crate::{DreamConnection, DreamView};
use bevy::prelude::*;
use dreamwake_sim::RunPhase;
use serde::Serialize;

/// Smallest displacement beyond physical motion that reads as a stutter: at
/// 60 Hz it is most of a frame's worth of walking, delivered in one frame.
pub(crate) const CORRECTION_METRES: f32 = 0.10;
/// A displacement no player would mistake for movement.
pub(crate) const TELEPORT_METRES: f32 = 1.0;
/// Headroom over the fastest scripted motion, for a platform carrying a
/// dashing rider. Anything beyond this per unit time cannot be movement.
const SPEED_MARGIN: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    at: f64,
    key: ContinuityKey,
    position: [f32; 2],
}

/// Display changes that legitimately move the owner without motion: a new
/// identity, an authored relocation (encounter placement, respawn), room,
/// match or scene, or a phase that relocates the party.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ContinuityKey {
    hero: u64,
    motion_segment: u64,
    room: usize,
    realm: usize,
    match_epoch: u32,
    scene_revision: u64,
    phase: RunPhase,
}

#[derive(Resource, Debug, Default, Clone, Serialize)]
pub(crate) struct SessionLedger {
    /// Connected frames compared against the previous displayed position.
    pub frames: u64,
    /// Frames whose displacement exceeded physical motion by a visible margin.
    pub corrections: u64,
    /// Frames whose displacement exceeded physical motion by a metre or more.
    pub teleports: u64,
    pub max_excess_metres: f32,
    pub excess_metres: f64,
    /// Legitimate discontinuities skipped rather than measured.
    pub continuity_resets: u64,
    pub connects: u64,
    pub disconnects: u64,
    /// Session-level recoveries the client requested from the server.
    pub recoveries: u64,
    pub connected_seconds: f64,
    #[serde(skip)]
    last: Option<Sample>,
    #[serde(skip)]
    was_connected: bool,
    #[serde(skip)]
    connected_since: Option<f64>,
}

impl SessionLedger {
    pub(crate) fn record_recovery(&mut self) {
        self.recoveries += 1;
    }

    fn observe(
        &mut self,
        now: f64,
        connected: bool,
        key: ContinuityKey,
        position: [f32; 2],
        alive: bool,
        max_speed: f32,
        tick_seconds: f32,
    ) {
        if connected != self.was_connected {
            if connected {
                self.connects += 1;
                self.connected_since = Some(now);
            } else {
                self.disconnects += 1;
                if let Some(since) = self.connected_since.take() {
                    self.connected_seconds += now - since;
                }
            }
            self.was_connected = connected;
            // The first connected frame replaces an offline preview, and the
            // first disconnected one reverts to it: neither is motion.
            self.last = None;
        }
        if !connected || !alive || position.iter().any(|axis| !axis.is_finite()) {
            self.last = None;
            return;
        }
        let current = Sample {
            at: now,
            key,
            position,
        };
        let Some(previous) = self.last.replace(current) else {
            return;
        };
        if previous.key != key {
            self.continuity_resets += 1;
            return;
        }
        // A fixed-step display moves a whole tick at a time, so a frame can
        // show up to one tick more motion than its real duration. More than
        // that means the display held still and then caught up, or snapped.
        let elapsed = (now - previous.at).max(0.0) as f32 + tick_seconds;
        let moved = (position[0] - previous.position[0]).hypot(position[1] - previous.position[1]);
        let excess = (moved - max_speed * elapsed).max(0.0);
        self.frames += 1;
        self.excess_metres += f64::from(excess);
        self.max_excess_metres = self.max_excess_metres.max(excess);
        if excess > CORRECTION_METRES {
            self.corrections += 1;
        }
        if excess > TELEPORT_METRES {
            self.teleports += 1;
        }
    }

    /// Include time in the current connection, which has not yet closed.
    pub(crate) fn connected_seconds_at(&self, now: f64) -> f64 {
        self.connected_seconds + self.connected_since.map_or(0.0, |since| now - since)
    }
}

pub(super) fn measure_continuity(
    time: Res<Time<Real>>,
    view: Res<DreamView>,
    connection: Res<DreamConnection>,
    mut ledger: ResMut<SessionLedger>,
) {
    let view = &view.0;
    let key = ContinuityKey {
        hero: view.hero.id,
        motion_segment: view.hero.motion_segment,
        room: view.room,
        realm: view.realm,
        match_epoch: view.stamp.match_epoch,
        scene_revision: view.stamp.scene_revision,
        phase: view.phase,
    };
    let profile = dreamwake_sim::collision::movement_profile();
    let max_speed = profile.dash_speed * SPEED_MARGIN;
    let before = (
        ledger.corrections,
        ledger.last.map(|sample| sample.position),
    );
    // What the player waited between frames, including a hitch the frame
    // clock has not caught up with yet.
    let now = crate::plugins::network::network_now(&time).as_secs_f64();
    ledger.observe(
        now,
        connection.connected,
        key,
        view.hero.position,
        view.hero.hp > 0.0,
        max_speed,
        profile.fixed_dt,
    );
    if ledger.corrections > before.0 {
        let from = before.1.unwrap_or_default();
        let to = view.hero.position;
        log::info!(
            "display discontinuity at {now:.3}s: {:.2} m, worst excess {:.2} m, \
             dashing {} invulnerable {} memories {:?}",
            (to[0] - from[0]).hypot(to[1] - from[1]),
            ledger.max_excess_metres,
            view.hero.dashing,
            view.hero.invulnerable,
            view.hero
                .memories
                .iter()
                .map(|m| m.kind)
                .collect::<Vec<_>>(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEED: f32 = 30.0;
    const TICK: f32 = 1.0 / 60.0;

    fn key() -> ContinuityKey {
        ContinuityKey {
            hero: 1,
            motion_segment: 1,
            room: 0,
            realm: 0,
            match_epoch: 1,
            scene_revision: 1,
            phase: RunPhase::Combat,
        }
    }

    #[test]
    fn physical_motion_is_continuous_and_a_snap_is_counted_by_its_excess() {
        let mut ledger = SessionLedger::default();
        // Walking at 8 m/s for a second of 60 Hz frames.
        for frame in 0..=60 {
            let at = f64::from(frame) / 60.0;
            ledger.observe(at, true, key(), [8.0 * at as f32, 0.0], true, SPEED, TICK);
        }
        assert_eq!(ledger.frames, 60);
        assert_eq!(ledger.corrections, 0);
        assert_eq!(ledger.teleports, 0);
        // A three-metre snap in one frame is a teleport, not movement.
        ledger.observe(61.0 / 60.0, true, key(), [11.0, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.corrections, 1);
        assert_eq!(ledger.teleports, 1);
        assert!(ledger.max_excess_metres > 1.0);
    }

    #[test]
    fn a_stall_is_not_a_teleport_but_catching_up_beyond_physics_is() {
        let mut ledger = SessionLedger::default();
        ledger.observe(0.0, true, key(), [0.0, 0.0], true, SPEED, TICK);
        // Half a second passes with no frames; walking covers four metres.
        ledger.observe(0.5, true, key(), [4.0, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.teleports, 0);
        // Then the display leaps twenty metres in a single frame.
        ledger.observe(
            0.5 + 1.0 / 60.0,
            true,
            key(),
            [24.0, 0.0],
            true,
            SPEED,
            TICK,
        );
        assert_eq!(ledger.teleports, 1);
    }

    #[test]
    fn legitimate_relocation_resets_instead_of_counting() {
        let mut ledger = SessionLedger::default();
        ledger.observe(0.0, true, key(), [0.0, 0.0], true, SPEED, TICK);
        let next_room = ContinuityKey { room: 1, ..key() };
        ledger.observe(1.0 / 60.0, true, next_room, [90.0, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.teleports, 0);
        assert_eq!(ledger.continuity_resets, 1);
        // Death and respawn relocate the hero without motion.
        ledger.observe(2.0 / 60.0, true, next_room, [90.0, 0.0], false, SPEED, TICK);
        ledger.observe(3.0 / 60.0, true, next_room, [0.0, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.teleports, 0);
    }

    #[test]
    fn reconnects_are_counted_and_never_reset_the_ledger() {
        let mut ledger = SessionLedger::default();
        ledger.observe(0.0, true, key(), [0.0, 0.0], true, SPEED, TICK);
        ledger.observe(1.0, false, key(), [0.0, 0.0], true, SPEED, TICK);
        ledger.observe(2.0, true, key(), [50.0, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.connects, 2);
        assert_eq!(ledger.disconnects, 1);
        // Reconnecting replaces the display; that jump is not measured.
        assert_eq!(ledger.teleports, 0);
        assert!((ledger.connected_seconds_at(3.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_frame_may_show_one_tick_ahead_but_not_a_held_back_burst() {
        let mut ledger = SessionLedger::default();
        ledger.observe(0.0, true, key(), [0.0, 0.0], true, SPEED, TICK);
        // An 8 ms frame displays a whole dash tick at 24 m/s.
        ledger.observe(0.008, true, key(), [0.4, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.corrections, 0);
        // Three ticks in one 8 ms frame: the display held still, then jumped.
        ledger.observe(0.016, true, key(), [1.6, 0.0], true, SPEED, TICK);
        assert_eq!(ledger.corrections, 1);
    }
}
