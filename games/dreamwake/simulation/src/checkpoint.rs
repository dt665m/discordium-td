//! Versioned canonical checkpoints. JSON is a bounded interchange format, not
//! raw memory hashing; canonical objects are sorted and negative zero normalized.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::OnceLock};
pub(crate) mod bounded_json;

pub const CHECKPOINT_SCHEMA: u32 = 11;
// Full-authority/offline envelope budget; this is NOT the planned 16 KiB
// per-owner scoped checkpoint transport budget.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_COLLECTION: usize = 16_384;
const MAX_STRING: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointError(pub String);
impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CheckpointError {}
fn fault(message: impl Into<String>) -> CheckpointError {
    CheckpointError(message.into())
}
fn json_error(error: serde_json::Error) -> CheckpointError {
    fault(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointIdentity {
    pub schema: u32,
    pub ruleset: String,
    pub scene_revision: u64,
}
impl CheckpointIdentity {
    /// Binds configuration and ordered game rules to this build, rather than a
    /// caller-supplied label. Complete game/engine sources, manifests, lockfile
    /// and compiler version bind the implementation; layout changes also require
    /// a schema bump. Seed/lucid and mutable game state are
    /// in the checkpoint; caller-owned scene assets are identified by scene_revision.
    /// Transport/arena-profile TOML files are not gameplay inputs or hashed here.
    pub fn current(scene_revision: u64) -> Self {
        static RULESET: OnceLock<String> = OnceLock::new();
        let ruleset = RULESET.get_or_init(|| {
            let mut hash = blake3::Hasher::new();
            hash.update(b"dreamwake-ruleset-source-v2");
            hash.update(&engine_core::SOURCE_IDENTITY);
            hash.update(&engine_net::SOURCE_IDENTITY);
            hash.update(&crate::source_identity::SOURCE_IDENTITY);
            hash.finalize().to_hex().to_string()
        });
        Self {
            schema: CHECKPOINT_SCHEMA,
            ruleset: ruleset.clone(),
            scene_revision,
        }
    }
    fn validate(&self) -> Result<(), CheckpointError> {
        if *self != Self::current(self.scene_revision) {
            return Err(fault("checkpoint schema/ruleset mismatch"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DreamCheckpoint {
    pub identity: CheckpointIdentity,
    pub snapshot: DreamSnapshot,
}
impl DreamCheckpoint {
    pub fn new(snapshot: DreamSnapshot, scene_revision: u64) -> Result<Self, CheckpointError> {
        validate(&snapshot)?;
        if snapshot.state.collision.scene_revision() != scene_revision {
            return Err(fault("snapshot collision scene revision mismatch"));
        }
        Ok(Self {
            identity: CheckpointIdentity::current(scene_revision),
            snapshot,
        })
    }
    pub fn encode(&self) -> Result<Vec<u8>, CheckpointError> {
        self.identity.validate()?;
        if self.identity.scene_revision != self.snapshot.state.collision.scene_revision() {
            return Err(fault("checkpoint collision identity mismatch"));
        }
        validate(&self.snapshot)?;
        canonical(self)
    }
    /// Callers supply the required scene revision; it is never trusted from wire.
    pub fn decode(bytes: &[u8], scene_revision: u64) -> Result<Self, CheckpointError> {
        bounded_json::preflight(bytes)?;
        let value: Value = serde_json::from_slice(bytes).map_err(json_error)?;
        check_value(&value, 0)?;
        let result: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        result.identity.validate()?;
        if result.identity.scene_revision != scene_revision
            || result.snapshot.state.collision.scene_revision() != scene_revision
        {
            return Err(fault("scene revision mismatch"));
        }
        validate(&result.snapshot)?;
        // Reject unknown fields anywhere, including latent engine components.
        if !same_shape(&serde_json::to_value(&result).map_err(json_error)?, &value) {
            return Err(fault("unknown or noncanonical checkpoint fields"));
        }
        Ok(result)
    }
    /// BLAKE3 digest includes only authoritative latent state and identity, not
    /// per-viewer projections or presentation DTO duplicates.
    pub fn canonical_digest(&self) -> Result<[u8; 32], CheckpointError> {
        Ok(*blake3::hash(&self.snapshot.canonical_bytes(&self.identity)?).as_bytes())
    }
}
impl DreamSimulation {
    /// Checked envelope entry point for versioned persistence and replay callers.
    pub fn try_from_checkpoint(
        checkpoint: &DreamCheckpoint,
        scene_revision: u64,
    ) -> Result<Self, CheckpointError> {
        checkpoint.identity.validate()?;
        if checkpoint.identity.scene_revision != scene_revision
            || checkpoint.snapshot.state.collision.scene_revision() != scene_revision
        {
            return Err(fault("scene revision mismatch"));
        }
        Self::try_from_snapshot(&checkpoint.snapshot)
    }
    pub fn try_restore_checkpoint(
        &mut self,
        checkpoint: &DreamCheckpoint,
        scene_revision: u64,
    ) -> Result<(), CheckpointError> {
        checkpoint.identity.validate()?;
        if checkpoint.identity.scene_revision != scene_revision
            || checkpoint.snapshot.state.collision.scene_revision() != scene_revision
        {
            return Err(fault("scene revision mismatch"));
        }
        self.try_restore(&checkpoint.snapshot)
    }
}
impl DreamSnapshot {
    pub fn canonical_bytes(
        &self,
        identity: &CheckpointIdentity,
    ) -> Result<Vec<u8>, CheckpointError> {
        identity.validate()?;
        if identity.scene_revision != self.state.collision.scene_revision() {
            return Err(fault("canonical collision identity mismatch"));
        }
        validate(self)?;
        let mut state = self.state.clone();
        state.heroes.sort_by_key(|v| v.view.id);
        state.enemies.sort_by_key(|v| v.view.id);
        state.covers.sort_by_key(|v| v.id);
        state.platforms.sort_by_key(|v| v.id);
        state.projectiles.sort_by_key(|v| v.state.id);
        state.wisps.sort_by_key(|v| v.state.id);
        state
            .effects
            .sort_by_key(|v| (v.id.match_epoch, v.id.owner, v.id.action_seq, v.id.slot));
        state.numbers.sort_by_key(|v| v.0.id);
        state.delayed.sort_by_key(|v| v.payload.id);
        canonical(&(identity, state))
    }
    pub fn canonical_digest(
        &self,
        identity: &CheckpointIdentity,
    ) -> Result<[u8; 32], CheckpointError> {
        Ok(*blake3::hash(&self.canonical_bytes(identity)?).as_bytes())
    }
}
pub(crate) fn same_shape(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| same_shape(v, other)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_shape(x, y))
        }
        _ => true,
    }
}
pub(crate) fn canonical(value: &impl Serialize) -> Result<Vec<u8>, CheckpointError> {
    let mut value = serde_json::to_value(value).map_err(json_error)?;
    check_value(&value, 0)?;
    normalize(&mut value);
    let bytes = serde_json::to_vec(&value).map_err(json_error)?;
    bounded_json::preflight(&bytes)?;
    Ok(bytes)
}
fn normalize(value: &mut Value) {
    match value {
        Value::Number(n) if n.is_f64() && n.as_f64() == Some(0.0) => {
            *value = serde_json::json!(0.0)
        }
        Value::Array(values) => values.iter_mut().for_each(normalize),
        Value::Object(values) => {
            values.values_mut().for_each(normalize);
            values.sort_keys();
        }
        _ => {}
    }
}
fn check_value(value: &Value, depth: usize) -> Result<(), CheckpointError> {
    if depth > 64 {
        return Err(fault("checkpoint nesting budget exceeded"));
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_COLLECTION {
                return Err(fault("checkpoint collection budget exceeded"));
            }
            for v in values {
                check_value(v, depth + 1)?;
            }
        }
        Value::Object(values) => {
            if values.len() > 256 {
                return Err(fault("checkpoint object budget exceeded"));
            }
            for (key, v) in values {
                if key.len() > MAX_STRING {
                    return Err(fault("checkpoint key budget exceeded"));
                }
                check_value(v, depth + 1)?;
            }
        }
        Value::String(s) if s.len() > MAX_STRING => {
            return Err(fault("checkpoint string budget exceeded"));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate(snapshot: &DreamSnapshot) -> Result<(), CheckpointError> {
    let s = &snapshot.state;
    crate::combat_history::CombatHistory::restore(s.combat_history.clone()).map_err(fault)?;
    if s.combat_history
        .frames
        .last()
        .is_some_and(|frame| frame.tick > s.run.tick)
    {
        return Err(fault("combat history exceeds authoritative tick"));
    }
    validate_current(crate::snapshot_capture::StateView::from(s))?;
    let bytes = serde_json::to_vec(snapshot).map_err(json_error)?;
    bounded_json::preflight(&bytes)?;
    // serde_json emits null for NaN/infinity. Typed reconstruction rejects it at
    // every required float; equality also rejects optional NaN becoming None.
    let checked: DreamSnapshot = serde_json::from_slice(&bytes).map_err(json_error)?;
    if checked != *snapshot {
        return Err(fault(
            "snapshot contains nonfinite or nonserializable state",
        ));
    }
    if snapshot.tick != s.run.tick || snapshot.seed != s.run.seed || snapshot.lucid != s.run.lucid {
        return Err(fault("snapshot header disagrees with authoritative state"));
    }
    Ok(())
}

/// Live publication validates current components, without rebuilding or encoding
/// the offline archive. All current semantic and nested finite-state checks remain.
pub(crate) fn validate_capture(
    state: &crate::snapshot_capture::CapturedState,
) -> Result<(), CheckpointError> {
    validate_current(crate::snapshot_capture::StateView::from(state))?;
    let bytes = serde_json::to_vec(state).map_err(json_error)?;
    bounded_json::preflight(&bytes)?;
    let restored: crate::snapshot_capture::CapturedState =
        serde_json::from_slice(&bytes).map_err(json_error)?;
    if restored != *state {
        return Err(fault("capture contains nonfinite or nonserializable state"));
    }
    Ok(())
}

fn validate_current(s: crate::snapshot_capture::StateView<'_>) -> Result<(), CheckpointError> {
    for (identity, defense) in s
        .heroes
        .iter()
        .map(|h| (&h.combat_identity, &h.defense_episodes))
        .chain(
            s.enemies
                .iter()
                .map(|e| (&e.combat_identity, &e.defense_episodes)),
        )
    {
        if !identity.valid() || identity.tick > s.run.tick || !defense.valid(identity.tick) {
            return Err(fault("invalid combat lifecycle or defense ledger"));
        }
    }
    if s.heroes.len() > MAX_HEROES
        || s.run.encounter_spawns > MAX_ENCOUNTER_SPAWNS
        || s.enemies.len() > MAX_ENCOUNTER_SPAWNS + AMBIENT_ENEMY_POSITIONS.len()
        || s.enemies.iter().any(|enemy| !enemy.ai.valid())
        || s.enemies
            .iter()
            .filter(|enemy| enemy.ai.role == EnemyRole::Encounter)
            .count()
            > s.run.encounter_spawns
        || s.enemies
            .iter()
            .filter(|enemy| enemy.ai.role == EnemyRole::Ambient)
            .count()
            > AMBIENT_ENEMY_POSITIONS.len()
        || s.enemies
            .iter()
            .filter(|enemy| enemy.ai.role == EnemyRole::Ambient)
            .map(|enemy| enemy.ai.home.map(f32::to_bits))
            .collect::<BTreeSet<_>>()
            .len()
            != s.enemies
                .iter()
                .filter(|enemy| enemy.ai.role == EnemyRole::Ambient)
                .count()
    {
        return Err(fault("invalid enemy population or activation state"));
    }
    if s.covers.len() > DEMO_COVER_POSITIONS.len()
        || s.covers.iter().any(|cover| !cover.valid(s.run.tick))
        || s.covers
            .iter()
            .map(|cover| cover.ground_position.map(f32::to_bits))
            .collect::<BTreeSet<_>>()
            .len()
            != s.covers.len()
        || s.covers
            .iter()
            .map(|cover| cover.id)
            .collect::<BTreeSet<_>>()
            .len()
            != s.covers.len()
    {
        return Err(fault("invalid saved cover"));
    }
    if s.platforms.len() != crate::platform::MAX_PLATFORMS
        || s.platforms
            .iter()
            .any(|p| !p.valid(s.run.tick, s.collision.scene_revision()))
    {
        return Err(fault("invalid saved platform"));
    }
    let collision = s.collision.build().map_err(|e| fault(e.to_string()))?;
    let views: Vec<_> = s
        .platforms
        .iter()
        .map(|p| p.presentation(s.collision.scene_revision()))
        .collect();
    let environment = crate::platform::MotionEnvironment::committed(&collision, &views, s.run.tick)
        .map_err(|e| fault(e.to_string()))?;
    for hero in s.heroes {
        environment
            .validate_motion(&hero.motion, hero.view.movement_speed)
            .map_err(|e| fault(e.to_string()))?;
    }
    for count in [
        s.heroes.len(),
        s.enemies.len(),
        s.projectiles.len(),
        s.wisps.len(),
        s.effects.len(),
        s.numbers.len(),
        s.delayed.len(),
    ] {
        if count > MAX_COLLECTION {
            return Err(fault("checkpoint entity budget exceeded"));
        }
    }
    if s.run.rng == 0
        || s.run.next_id < 1 << 63
        || s.run.party_size != s.heroes.iter().filter(|h| h.active).count()
    {
        return Err(fault("invalid run RNG, allocator or roster"));
    }
    let mut ids = BTreeSet::new();
    for id in s
        .heroes
        .iter()
        .map(|v| v.view.id)
        .chain(s.enemies.iter().map(|v| v.view.id))
        .chain(s.covers.iter().flat_map(|v| [v.id, v.marker_id]))
        .chain(s.platforms.iter().map(|v| v.id))
        .chain(s.projectiles.iter().map(|v| v.state.id))
        .chain(s.wisps.iter().map(|v| v.state.id))
        .chain(s.numbers.iter().map(|v| v.0.id))
        .chain(s.delayed.iter().map(|v| v.payload.id))
    {
        if id == 0 || !ids.insert(id) {
            return Err(fault("duplicate or zero stable entity ID"));
        }
        if id >= 1 << 63 && id >= s.run.next_id {
            return Err(fault("entity ID exceeds allocator frontier"));
        }
    }
    if s.heroes.iter().any(|h| {
        !h.charge.valid(h.view.id, h.combat_identity.generation)
            || !h
                .ray
                .valid_at(h.view.id, h.combat_identity.generation, s.run.tick)
    }) {
        return Err(fault("invalid saved ability state"));
    }
    if s.heroes
        .iter()
        .any(|v| v.view.id >= 1 << 63 || v.loadout.0.len() != 4 || v.health.max_hp <= 0.0)
    {
        return Err(fault("invalid hero identity, loadout or health capacity"));
    }
    if s.heroes.iter().any(|hero| {
        !hero.critical_rng.belongs_to(
            s.run.seed,
            hero.view.id,
            hero.combat_identity.generation,
            CRITICAL_RANDOM_DOMAIN,
        )
    }) {
        return Err(fault("hero random stream identity/domain mismatch"));
    }
    if s.enemies.iter().any(|v| v.health.max_hp <= 0.0) {
        return Err(fault("invalid enemy health capacity"));
    }
    let mut effects = BTreeSet::new();
    for e in s.effects {
        if !effects.insert((e.id.match_epoch, e.id.owner, e.id.action_seq, e.id.slot))
            || e.radius < 0.0
            || !crate::valid_graphics_kind(e.kind)
            || !crate::valid_graphics_id(e.id)
        {
            return Err(fault("invalid graphics identity or radius"));
        }
    }
    for p in s.projectiles {
        if p.state.owner == 0
            || p.state.radius < 0.0
            || p.state
                .hit_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != p.state.hit_ids.len()
        {
            return Err(fault("invalid projectile owner, radius or hit history"));
        }
    }
    let mut spawns = Vec::new();
    for p in s.projectiles {
        if let Some((key, origin)) = p.payload.spawn {
            if !key.action.valid()
                || key.action.actor != p.state.owner
                || key.ordinal > 3
                || origin > s.run.tick
                || u32::try_from(key.action.command_sequence).is_err()
            {
                return Err(fault("invalid projectile spawn identity"));
            }
            spawns.push(key);
        }
    }
    for d in s.delayed {
        if let Some((key, origin)) = d.payload.spawn {
            if !key.action.valid()
                || key.action.actor != d.payload.owner
                || key.ordinal != 3
                || origin > s.run.tick
                || d.payload.kind != crate::MemoryKind::Starfall
                || d.remaining > 0.55
            {
                return Err(fault("invalid reserved Starfall repeat"));
            }
            spawns.push(key);
        }
    }
    for (index, key) in spawns.iter().enumerate() {
        if spawns[..index].contains(key)
            || spawns
                .iter()
                .filter(|k| k.action.actor == key.action.actor)
                .count()
                > crate::starfall::STARFALL_CAPACITY
        {
            return Err(fault("duplicate or overflowing owner projectile domain"));
        }
    }
    // Sources may legitimately have despawned (independent projectiles and
    // completed actions survive their source), so absence is not an invalid link.
    if s.wisps.iter().any(|v| v.state.owner == 0) || s.delayed.iter().any(|v| v.payload.owner == 0)
    {
        return Err(fault("invalid latent owner identity"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_restore_preserves_complete_world() {
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run();
        let before = sim.snapshot();
        let unrelated = sim.world_mut().spawn(Health::new(17.0)).id();
        let mut invalid = before.clone();
        invalid.state.heroes[0].motion.velocity[0] = f32::NAN;
        assert!(sim.try_restore(&invalid).is_err());
        assert_eq!(sim.snapshot(), before);
        assert_eq!(sim.world().get::<Health>(unrelated).unwrap().hp, 17.0);
        let mut invalid = before.clone();
        invalid.state.heroes.push(invalid.state.heroes[0].clone());
        assert!(sim.try_restore(&invalid).is_err());
        assert_eq!(sim.snapshot(), before);
        let mut checkpoint = DreamCheckpoint::new(before.clone(), 1).unwrap();
        checkpoint.identity.schema += 1;
        assert!(sim.try_restore_checkpoint(&checkpoint, 1).is_err());
        assert_eq!(sim.snapshot(), before);
    }
    #[test]
    fn restore_at_every_tick_replays_canonical_state() {
        let mut sim = DreamSimulation::new(42, false);
        sim.continue_run();
        let commands: Vec<_> = (0..24)
            .map(|i| DreamInput {
                movement: [0.3, 0.2],
                aim: [0.0, -1.0],
                attack: true,
                casts: [i == 3, i == 8, false, false],
                ..Default::default()
            })
            .collect();
        let mut checkpoints = vec![sim.snapshot()];
        for input in &commands {
            sim.step(*input);
            checkpoints.push(sim.snapshot());
        }
        let identity = CheckpointIdentity::current(1);
        let expected = sim.snapshot().canonical_digest(&identity).unwrap();
        for (k, checkpoint) in checkpoints.iter().enumerate() {
            let wire = DreamCheckpoint::new(checkpoint.clone(), 1)
                .unwrap()
                .encode()
                .unwrap();
            let decoded = DreamCheckpoint::decode(&wire, 1).unwrap();
            let mut replay = DreamSimulation::try_from_snapshot(&decoded.snapshot).unwrap();
            for input in &commands[k..] {
                replay.step(*input);
            }
            assert_eq!(
                replay.snapshot().canonical_digest(&identity).unwrap(),
                expected,
                "restore boundary {k}"
            );
        }
    }
    #[test]
    fn codec_rejects_wrong_identity_unknown_fields_and_bounds() {
        let checkpoint =
            DreamCheckpoint::new(DreamSimulation::new(7, false).snapshot(), 1).unwrap();
        let bytes = checkpoint.encode().unwrap();
        assert!(DreamCheckpoint::decode(&bytes, 10).is_err());
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value["snapshot"]["state"]["run"]["surprise"] = Value::Bool(true);
        assert!(DreamCheckpoint::decode(&serde_json::to_vec(&value).unwrap(), 1).is_err());
        assert!(DreamCheckpoint::decode(&vec![b' '; MAX_BYTES + 1], 1).is_err());
        let mut changed = checkpoint.clone();
        changed.identity.schema += 1;
        assert!(changed.encode().is_err());
    }
    #[test]
    fn digest_is_view_independent_and_normalizes_negative_zero() {
        let mut sim = DreamSimulation::new(7, false);
        sim.add_player(2);
        let identity = CheckpointIdentity::current(1);
        assert_eq!(
            sim.snapshot_for(1).canonical_digest(&identity).unwrap(),
            sim.snapshot_for(2).canonical_digest(&identity).unwrap()
        );
        let mut snapshot = sim.snapshot();
        snapshot.state.heroes[0].motion.velocity[0] = -0.0;
        assert_eq!(
            snapshot.canonical_digest(&identity).unwrap(),
            sim.snapshot().canonical_digest(&identity).unwrap()
        );
    }
}
