//! Snapshot history, baseline reconstruction, and interpolation sampling.
use super::*;
#[derive(Clone)]
pub(super) struct TimestampedSnapshot {
    pub(super) server_tick: u32,
    pub(super) receive_time: f32,
    pub(super) world: WorldDelta,
}

#[derive(Resource)]
pub(super) struct SnapshotBuffer {
    pub(super) snapshots: VecDeque<TimestampedSnapshot>,
    pub(super) interpolation_delay: f32,
    pub(super) render_time: f32,
}

impl Default for SnapshotBuffer {
    fn default() -> Self {
        Self {
            snapshots: VecDeque::with_capacity(64),
            interpolation_delay: FIXED_DT_SECONDS * 2.0,
            render_time: 0.0,
        }
    }
}

/// Positions interpolated from the snapshot buffer for remote entities.
pub(super) struct InterpolatedPositions {
    pub(super) heroes: HashMap<u64, [f32; 2]>,
    pub(super) enemies: HashMap<u64, ([f32; 2], [f32; 2])>, // (pos, vel)
}

impl SnapshotBuffer {
    pub(super) fn push(&mut self, snapshot: TimestampedSnapshot) -> bool {
        // Gate every authoritative consumer, including effects and input ACKs.
        if self
            .snapshots
            .back()
            .is_some_and(|last| !is_newer_input_seq(snapshot.server_tick, last.server_tick))
        {
            return false;
        }
        self.snapshots.push_back(snapshot);
        // Server baselines may be 60 ticks old. Keep the complete usable window.
        while self.snapshots.len() > 64 {
            self.snapshots.pop_front();
        }
        true
    }

    pub(super) fn update_interpolation_delay(&mut self, net_stats: &NetStats) {
        let target = (net_stats.rtt_ema * 0.5 + net_stats.jitter_ema * 3.0)
            .max(FIXED_DT_SECONDS * 2.0)
            .min(0.2);
        const DELAY_ALPHA: f32 = 0.05;
        self.interpolation_delay += DELAY_ALPHA * (target - self.interpolation_delay);
    }

    pub(super) fn advance_render_time(&mut self, dt: f32) {
        self.render_time += dt;
    }

    /// Sync render_time to latest snapshot receive_time minus interpolation_delay.
    /// Called when a new snapshot arrives to keep the clock on track.
    pub(super) fn sync_render_clock(&mut self) {
        if let Some(latest) = self.snapshots.back() {
            let target_render_time = latest.receive_time - self.interpolation_delay;
            // Softly chase the target to avoid jumps
            let diff = target_render_time - self.render_time;
            if diff.abs() > 0.5 {
                // Too far off — snap
                self.render_time = target_render_time;
            }
            // Otherwise render_time advances naturally via advance_render_time
        }
    }

    pub(super) fn sample(&self, render_time: f32, enemy_lead: f32) -> InterpolatedPositions {
        let mut heroes = HashMap::new();
        let mut enemies = HashMap::new();

        if self.snapshots.is_empty() {
            return InterpolatedPositions { heroes, enemies };
        }

        // Find bracketing snapshots
        let (older, newer, t) = self.find_bracketing(render_time);

        // Interpolate heroes
        for hero_new in &newer.world.heroes {
            if let Some(hero_old) = older
                .world
                .heroes
                .iter()
                .find(|h| h.client_id == hero_new.client_id)
            {
                let pos = lerp_pos(hero_old.pos, hero_new.pos, t);
                heroes.insert(hero_new.client_id, pos);
            } else {
                heroes.insert(hero_new.client_id, hero_new.pos);
            }
        }

        // Interpolate enemies
        for enemy_new in &newer.world.enemies {
            if let Some(enemy_old) = older.world.enemies.iter().find(|e| e.id == enemy_new.id) {
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
        render_time: f32,
    ) -> (&TimestampedSnapshot, &TimestampedSnapshot, f32) {
        let len = self.snapshots.len();
        if len < 2 {
            let snap = &self.snapshots[0];
            return (snap, snap, 1.0);
        }

        // Find the newest snapshot with receive_time <= render_time
        let mut older_idx = 0;
        for i in 0..len {
            if self.snapshots[i].receive_time <= render_time {
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
        let span = newer.receive_time - older.receive_time;
        let t = if span > f32::EPSILON {
            ((render_time - older.receive_time) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (older, newer, t)
    }

    pub(super) fn has_enough_data(&self) -> bool {
        self.snapshots.len() >= 2
    }

    pub(super) fn clear(&mut self) {
        self.snapshots.clear();
        self.render_time = 0.0;
        self.interpolation_delay = FIXED_DT_SECONDS * 2.0;
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
    let base = &baseline.world;

    // Start with baseline entity lists
    let mut heroes: Vec<HeroSnapshot> = Vec::new();
    let mut enemies: Vec<EnemySnapshot> = Vec::new();
    let mut towers: Vec<TowerSnapshot> = Vec::new();

    let removed: HashSet<u64> = patch.removed_ids.into_iter().collect();
    let removed_heroes: HashSet<u64> = patch.removed_hero_ids.into_iter().collect();

    // Heroes: start from baseline, apply patches
    for hero in &base.heroes {
        if removed_heroes.contains(&hero.client_id) {
            continue;
        }
        if let Some(patched) = patch
            .hero_patches
            .iter()
            .find(|h| h.client_id == hero.client_id)
        {
            heroes.push(patched.clone());
        } else {
            heroes.push(hero.clone());
        }
    }
    // Add new heroes (in patch but not in baseline)
    for patched in &patch.hero_patches {
        if !base.heroes.iter().any(|h| h.client_id == patched.client_id) {
            heroes.push(patched.clone());
        }
    }

    // Enemies: start from baseline, apply patches
    for enemy in &base.enemies {
        if removed.contains(&enemy.id) {
            continue;
        }
        if let Some(patched) = patch.enemy_patches.iter().find(|e| e.id == enemy.id) {
            enemies.push(*patched);
        } else {
            enemies.push(*enemy);
        }
    }
    for patched in &patch.enemy_patches {
        if !base.enemies.iter().any(|e| e.id == patched.id) {
            enemies.push(*patched);
        }
    }

    // Towers: start from baseline, apply patches
    for tower in &base.towers {
        if removed.contains(&tower.id) {
            continue;
        }
        if let Some(patched) = patch.tower_patches.iter().find(|t| t.id == tower.id) {
            towers.push(*patched);
        } else {
            towers.push(*tower);
        }
    }
    for patched in &patch.tower_patches {
        if !base.towers.iter().any(|t| t.id == patched.id) {
            towers.push(*patched);
        }
    }

    Some(WorldDelta {
        presentations: patch.presentations,
        tick: patch.tick,
        phase: patch.phase,
        match_restart_ticks_remaining: patch.match_restart_ticks_remaining,
        wave: patch.wave,
        team_life: patch.team_life,
        objectives: patch.objectives,
        heroes,
        enemies,
        towers,
        your_last_input_seq: patch.your_last_input_seq,
        sim_meta: patch.sim_meta,
    })
}
