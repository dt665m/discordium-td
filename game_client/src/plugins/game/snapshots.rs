//! Snapshot history, baseline reconstruction, and interpolation sampling.
use super::*;
use game_shared::SERVER_TICK_HZ;
#[derive(Clone)]
pub(super) struct TimestampedSnapshot {
    pub(super) server_tick: u32,
    pub(super) receive_time: f32,
    pub(super) simulation_time: f64,
    pub(super) world: WorldDelta,
}

#[derive(Resource)]
pub(super) struct SnapshotBuffer {
    pub(super) snapshots: VecDeque<TimestampedSnapshot>,
    pub(super) interpolation_delay: f32,
    pub(super) render_time: f64,
    arrival_jitter: f32,
    clock_initialized: bool,
}

impl Default for SnapshotBuffer {
    fn default() -> Self {
        Self {
            snapshots: VecDeque::with_capacity(64),
            interpolation_delay: FIXED_DT_SECONDS * 2.0,
            render_time: 0.0,
            arrival_jitter: 0.0,
            clock_initialized: false,
        }
    }
}

/// Positions interpolated from the snapshot buffer for remote entities.
pub(super) struct InterpolatedPositions {
    pub(super) heroes: HashMap<u64, [f32; 2]>,
    pub(super) enemies: HashMap<u64, ([f32; 2], [f32; 2])>, // (pos, vel)
}

impl SnapshotBuffer {
    pub(super) fn push(&mut self, mut snapshot: TimestampedSnapshot) -> bool {
        // Gate every authoritative consumer, including effects and input ACKs.
        if self
            .snapshots
            .back()
            .is_some_and(|last| !is_newer_input_seq(snapshot.server_tick, last.server_tick))
        {
            return false;
        }
        // Packet arrival time is not simulation time: several ticks can arrive in
        // one browser frame. Preserve their spacing, including across tick wrap.
        snapshot.simulation_time = if let Some(last) = self.snapshots.back() {
            let tick_span =
                snapshot.server_tick.wrapping_sub(last.server_tick) as f64 / SERVER_TICK_HZ as f64;
            let arrival_span = (snapshot.receive_time - last.receive_time).max(0.0);
            let jitter = (arrival_span - tick_span as f32).abs();
            self.arrival_jitter += 0.1 * (jitter - self.arrival_jitter);
            if snapshot.world.sim_meta.map(|meta| meta.match_epoch)
                != last.world.sim_meta.map(|meta| meta.match_epoch)
            {
                self.clock_initialized = false;
            }
            last.simulation_time + tick_span
        } else {
            0.0
        };
        self.snapshots.push_back(snapshot);
        // Server baselines may be 60 ticks old. Keep the complete usable window.
        while self.snapshots.len() > 64 {
            self.snapshots.pop_front();
        }
        true
    }

    pub(super) fn update_interpolation_delay(&mut self) {
        // Buffer delivery jitter, not input-ACK latency (which also includes
        // simulation/command processing). Two ticks cover an isolated lost update.
        let target = (FIXED_DT_SECONDS * 2.0 + self.arrival_jitter * 3.0).min(0.25);
        let alpha = if target > self.interpolation_delay {
            0.2
        } else {
            0.02
        };
        self.interpolation_delay += alpha * (target - self.interpolation_delay);
    }

    pub(super) fn advance_render_time(&mut self, now: f32, dt: f32) {
        let Some(latest) = self.snapshots.back() else {
            return;
        };
        let age = (now - latest.receive_time).max(0.0) as f64;
        let target = latest.simulation_time + age - self.interpolation_delay as f64;
        let resumed_delivery = age < 0.1 && (target - self.render_time).abs() > 0.5;
        if !self.clock_initialized || dt > 0.5 || resumed_delivery {
            // Initialization and a suspended browser are discontinuities. During
            // normal play adjust playback speed instead of jumping the clock.
            self.render_time = target.min(latest.simulation_time);
            self.clock_initialized = true;
        } else {
            let error = target - (self.render_time + dt as f64);
            let rate = (1.0 + error * 2.0).clamp(0.9, 1.1);
            self.render_time = (self.render_time + dt as f64 * rate).min(latest.simulation_time);
        }
    }

    pub(super) fn sample(&self, render_time: f64, enemy_lead: f32) -> InterpolatedPositions {
        let mut heroes = HashMap::new();
        let mut enemies = HashMap::new();

        if self.snapshots.is_empty() {
            return InterpolatedPositions { heroes, enemies };
        }

        // Find bracketing snapshots
        let (older, newer, t) = self.find_bracketing(render_time);
        let latest = self.snapshots.back().unwrap();
        let (older, newer, t) = if newer.world.sim_meta.map(|meta| meta.match_epoch)
            != latest.world.sim_meta.map(|meta| meta.match_epoch)
        {
            (latest, latest, 1.0)
        } else {
            (older, newer, t)
        };

        // Newcomers and respawns hold their first position in the current life
        // until the delayed timeline reaches it. Never walk on latest authority
        // temporarily and then jump backward when interpolation takes ownership.
        for current in &latest.world.heroes {
            let same_life = |hero: &&HeroSnapshot| {
                hero.client_id == current.client_id
                    && hero.respawn_generation == current.respawn_generation
            };
            let position = if let Some(new) = newer.world.heroes.iter().find(same_life) {
                older
                    .world
                    .heroes
                    .iter()
                    .find(same_life)
                    .map_or(new.pos, |old| lerp_pos(old.pos, new.pos, t))
            } else {
                self.snapshots
                    .iter()
                    .filter(|snapshot| {
                        snapshot.world.sim_meta.map(|meta| meta.match_epoch)
                            == latest.world.sim_meta.map(|meta| meta.match_epoch)
                    })
                    .find_map(|snapshot| {
                        snapshot
                            .world
                            .heroes
                            .iter()
                            .find(same_life)
                            .map(|hero| hero.pos)
                    })
                    .unwrap_or(current.pos)
            };
            heroes.insert(current.client_id, position);
        }

        // Interpolate enemies
        for enemy_new in &newer.world.enemies {
            if let Some(enemy_old) = older
                .world
                .enemies
                .iter()
                .find(|e| e.id == enemy_new.id && e.spawn == enemy_new.spawn)
            {
                let pos = lerp_pos(enemy_old.pos, enemy_new.pos, t);
                // Apply extrapolation lead on top of interpolated position
                let led_pos = clamp_to_world([
                    pos[0] + enemy_new.vel[0] * enemy_lead,
                    pos[1] + enemy_new.vel[1] * enemy_lead,
                ]);
                enemies.insert(enemy_new.id, (led_pos, enemy_new.vel));
            } else {
                let led_pos = clamp_to_world([
                    enemy_new.pos[0] + enemy_new.vel[0] * enemy_lead,
                    enemy_new.pos[1] + enemy_new.vel[1] * enemy_lead,
                ]);
                enemies.insert(enemy_new.id, (led_pos, enemy_new.vel));
            }
        }

        InterpolatedPositions { heroes, enemies }
    }

    pub(super) fn find_bracketing(
        &self,
        render_time: f64,
    ) -> (&TimestampedSnapshot, &TimestampedSnapshot, f32) {
        let len = self.snapshots.len();
        if len < 2 {
            let snap = &self.snapshots[0];
            return (snap, snap, 1.0);
        }

        // Find the newest snapshot on the server simulation timeline.
        let mut older_idx = 0;
        for i in 0..len {
            if self.snapshots[i].simulation_time <= render_time {
                older_idx = i;
            } else {
                break;
            }
        }
        let newer_idx = (older_idx + 1).min(len - 1);
        if older_idx == newer_idx {
            // render_time is past all snapshots — use last two and extrapolate (clamp t to 1.0)
            if len >= 2 {
                let older = &self.snapshots[len - 2];
                let newer = &self.snapshots[len - 1];
                return (older, newer, 1.0);
            }
            let snap = &self.snapshots[0];
            return (snap, snap, 1.0);
        }

        let older = &self.snapshots[older_idx];
        let newer = &self.snapshots[newer_idx];
        let span = newer.simulation_time - older.simulation_time;
        let t = if span > f64::EPSILON {
            ((render_time - older.simulation_time) / span).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        // A new match must not blend a respawn with positions from the old match.
        if older.world.sim_meta.map(|meta| meta.match_epoch)
            != newer.world.sim_meta.map(|meta| meta.match_epoch)
        {
            return (newer, newer, 1.0);
        }
        (older, newer, t)
    }

    pub(super) fn has_enough_data(&self) -> bool {
        self.snapshots.len() >= 2
    }

    pub(super) fn clear(&mut self) {
        self.snapshots.clear();
        self.render_time = 0.0;
        self.interpolation_delay = FIXED_DT_SECONDS * 2.0;
        self.arrival_jitter = 0.0;
        self.clock_initialized = false;
    }

    /// Find a snapshot by tick for delta reconstruction.
    pub(super) fn find_by_tick(&self, tick: u32) -> Option<&TimestampedSnapshot> {
        self.snapshots.iter().find(|s| s.server_tick == tick)
    }
}

fn lerp_pos(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Reconstruct a full WorldDelta from a patch and its baseline in the snapshot buffer.
pub(super) fn reconstruct_from_patch(
    buffer: &SnapshotBuffer,
    patch: WorldPatch,
) -> Option<WorldDelta> {
    let baseline = buffer.find_by_tick(patch.baseline_tick)?;
    game_shared::apply_world_patch(&baseline.world, &patch).ok()
}

#[cfg(test)]
mod playback_tests {
    use super::*;

    fn moving_snapshot(tick: u32, arrival: f32) -> TimestampedSnapshot {
        let mut sim = Simulation::new();
        sim.add_player(7);
        let mut world = sim.world_delta();
        world.tick = tick;
        world.heroes[0].pos = [tick as f32 / SERVER_TICK_HZ as f32 * 4.0, 0.0];
        TimestampedSnapshot {
            server_tick: tick,
            receive_time: arrival,
            simulation_time: 0.0,
            world,
        }
    }

    #[test]
    fn newcomer_holds_first_position_until_playback_catches_up() {
        let mut buffer = SnapshotBuffer::default();
        for tick in 0..6 {
            let mut snapshot = moving_snapshot(tick, tick as f32 / 30.0);
            if tick >= 3 {
                let mut hero = snapshot.world.heroes[0].clone();
                hero.client_id = 8;
                hero.pos = [(tick - 3) as f32, 0.0];
                snapshot.world.heroes.push(hero);
            }
            buffer.push(snapshot);
        }
        assert_eq!(buffer.sample(0.0, 0.0).heroes[&8], [0.0, 0.0]);
        assert_eq!(buffer.sample(0.05, 0.0).heroes[&8], [0.0, 0.0]);
        assert!(buffer.sample(0.12, 0.0).heroes[&8][0] > 0.0);
    }
    #[test]
    fn respawn_moves_to_the_current_life_before_delayed_playback_arrives() {
        let mut buffer = SnapshotBuffer::default();
        for tick in 0..6 {
            let mut snapshot = moving_snapshot(tick, tick as f32 / 30.0);
            snapshot.world.heroes[0].pos = [20.0, 0.0];
            if tick >= 3 {
                snapshot.world.heroes[0].respawn_generation = 1;
                snapshot.world.heroes[0].pos = [-10.0 + (tick - 3) as f32, 0.0];
            }
            buffer.push(snapshot);
        }
        assert_eq!(buffer.sample(0.0, 0.0).heroes[&7], [-10.0, 0.0]);
        assert_eq!(buffer.sample(0.05, 0.0).heroes[&7], [-10.0, 0.0]);
        assert!(buffer.sample(0.12, 0.0).heroes[&7][0] > -10.0);
    }

    #[test]
    fn remote_respawn_does_not_interpolate_across_the_map() {
        let mut buffer = SnapshotBuffer::default();
        let mut older = moving_snapshot(1, 0.0);
        older.world.heroes[0].pos = [20.0, 0.0];
        let mut newer = moving_snapshot(2, 0.033);
        newer.world.heroes[0].pos = [-10.0, 0.0];
        newer.world.heroes[0].respawn_generation += 1;
        let id = newer.world.heroes[0].client_id;
        buffer.push(older);
        buffer.push(newer);
        assert_eq!(buffer.sample(0.016, 0.0).heroes[&id], [-10.0, 0.0]);
    }

    #[test]
    fn batched_packets_keep_server_tick_spacing() {
        let mut buffer = SnapshotBuffer::default();
        for tick in 0..4 {
            buffer.push(moving_snapshot(tick, 10.0));
        }
        let positions = buffer.sample(0.05, 0.0);
        assert!((positions.heroes[&7][0] - 0.2).abs() < 0.0001);
        let (_, _, t) = buffer.find_bracketing(0.05);
        assert!((t - 0.5).abs() < 0.0001);
    }

    fn play_batched_stream(fps: u32) -> f32 {
        let mut buffer = SnapshotBuffer::default();
        let mut next_tick = 0;
        let mut last_pos = 0.0;
        for frame in 0..fps * 8 {
            let now = frame as f32 / fps as f32;
            // Deliver three simulation ticks together every 100 ms, with a
            // repeating extra 20 ms delay and one missing update per ten ticks.
            while ((next_tick / 3 + 1) as f32 * 0.1
                + if next_tick / 3 % 3 == 0 { 0.02 } else { 0.0 })
                <= now
            {
                if next_tick % 10 != 5 {
                    buffer.push(moving_snapshot(next_tick, now));
                    buffer.update_interpolation_delay();
                }
                next_tick += 1;
            }
            buffer.advance_render_time(now, 1.0 / fps as f32);
            let pos = buffer
                .sample(buffer.render_time, 0.0)
                .heroes
                .get(&7)
                .map_or(0.0, |p| p[0]);
            if now > 2.0 {
                // A remote walking player must move each frame, without packet-
                // sized jumps or reverse corrections, after the buffer settles.
                let step = pos - last_pos;
                assert!(step > 0.0, "fps={fps} time={now} stopped");
                assert!(
                    step <= 4.0 * 1.101 / fps as f32,
                    "fps={fps} time={now} step={step}"
                );
            }
            last_pos = pos;
        }
        last_pos
    }

    #[test]
    fn remote_walk_stays_smooth_at_30_60_and_120_fps_with_loss_and_jitter() {
        let a = play_batched_stream(30);
        let b = play_batched_stream(60);
        let c = play_batched_stream(120);
        assert!((a - b).abs() < 0.2);
        assert!((b - c).abs() < 0.2);
    }

    #[test]
    fn delay_changes_adjust_clock_without_backward_jumps() {
        let mut buffer = SnapshotBuffer::default();
        for frame in 0..600 {
            let now = frame as f32 / 60.0;
            if frame % 2 == 0 {
                buffer.push(moving_snapshot(frame / 2, now));
            }
            if frame == 120 {
                buffer.interpolation_delay = 0.2;
            }
            let previous = buffer.render_time;
            buffer.advance_render_time(now, 1.0 / 60.0);
            if frame > 0 {
                assert!(buffer.render_time >= previous);
                assert!(buffer.render_time - previous <= 1.101 / 60.0);
            }
        }
        let latest = buffer.snapshots.back().unwrap().simulation_time;
        assert!((latest - buffer.render_time - 0.2).abs() < 0.025);
    }

    #[test]
    fn receive_outage_resynchronizes_without_replaying_seconds_of_stale_movement() {
        let mut buffer = SnapshotBuffer::default();
        for frame in 0..180 {
            let now = frame as f32 / 60.0;
            if frame % 2 == 0 && !(60..150).contains(&frame) {
                buffer.push(moving_snapshot(frame / 2, now));
                buffer.update_interpolation_delay();
            }
            buffer.advance_render_time(now, 1.0 / 60.0);
            if frame == 150 {
                let latest = buffer.snapshots.back().unwrap().simulation_time;
                assert!(
                    (latest - buffer.render_time - buffer.interpolation_delay as f64).abs() < 0.001
                );
            }
        }
    }

    #[test]
    fn pause_reset_and_tick_wrap_do_not_blend_old_match_positions() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(moving_snapshot(u32::MAX - 1, 0.0));
        buffer.push(moving_snapshot(0, 0.1));
        assert!((buffer.snapshots.back().unwrap().simulation_time - 2.0 / 30.0).abs() < 1e-9);
        buffer.clear();
        buffer.push(moving_snapshot(0, 0.0));
        buffer.advance_render_time(0.0, 0.016);
        let mut new_match = moving_snapshot(300, 10.0);
        new_match.world.sim_meta.as_mut().unwrap().match_epoch += 1;
        new_match.world.heroes[0].pos = [-10.0, 5.0];
        buffer.push(new_match);
        buffer.advance_render_time(10.0, 10.0);
        assert_eq!(
            buffer.sample(buffer.render_time, 0.0).heroes[&7],
            [-10.0, 5.0]
        );
        buffer.clear();
        buffer.push(moving_snapshot(0, 20.0));
        buffer.advance_render_time(20.0, 0.016);
        assert_eq!(
            buffer.sample(buffer.render_time, 0.0).heroes[&7],
            [0.0, 0.0]
        );
    }
}
