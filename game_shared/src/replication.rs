//! Stable entity records with lossless field-byte changes. Adding fields to an
//! existing snapshot type automatically includes them in its replication schema.
use crate::{
    EnemySnapshot, HeroSnapshot, RecordPatch, StateKey, TowerSnapshot, WorldDelta, WorldPatch, wire,
};
use std::collections::BTreeMap;

pub type ReplicationState = BTreeMap<StateKey, Vec<u8>>;
#[derive(Clone, Default)]
pub struct RecordChanges {
    /// Creation/removal in the interval requires a full current record.
    pub reset: bool,
    pub words: Vec<u64>,
}
pub type ChangeSet = BTreeMap<StateKey, RecordChanges>;

/// The world record owns all globals and transient presentation state; entity
/// lists live in their own stable namespaces. This is a replication index only.
pub fn capture_world(world: &WorldDelta) -> ReplicationState {
    let mut records = BTreeMap::new();
    for hero in &world.heroes {
        records.insert(StateKey::Hero(hero.client_id), wire::serialize(hero));
    }
    for enemy in &world.enemies {
        records.insert(StateKey::Enemy(enemy.id), wire::serialize(enemy));
    }
    for tower in &world.towers {
        records.insert(StateKey::Tower(tower.id), wire::serialize(tower));
    }
    let globals = WorldDelta {
        heroes: Vec::new(),
        enemies: Vec::new(),
        towers: Vec::new(),
        presentations: world.presentations.clone(),
        objectives: world.objectives.clone(),
        tick: world.tick,
        phase: world.phase,
        match_restart_ticks_remaining: world.match_restart_ticks_remaining,
        wave: world.wave,
        team_life: world.team_life,
        sim_meta: world.sim_meta,
    };
    records.insert(StateKey::World, wire::serialize(&globals));
    records
}

fn changed_words(current: &[u8], previous: &[u8]) -> Vec<u64> {
    let mut words = vec![0; current.len().div_ceil(64)];
    for (offset, value) in current.iter().enumerate() {
        if previous.get(offset) != Some(value) {
            words[offset / 64] |= 1 << (offset % 64);
        }
    }
    words
}

pub fn changed_records(current: &ReplicationState, previous: &ReplicationState) -> ChangeSet {
    let mut changes = BTreeMap::new();
    for (&key, state) in current {
        match previous.get(&key) {
            Some(old) if old == state => {}
            Some(old) => {
                changes.insert(
                    key,
                    RecordChanges {
                        reset: false,
                        words: changed_words(state, old),
                    },
                );
            }
            None => {
                changes.insert(
                    key,
                    RecordChanges {
                        reset: true,
                        words: Vec::new(),
                    },
                );
            }
        }
    }
    for &key in previous.keys().filter(|key| !current.contains_key(key)) {
        changes.insert(
            key,
            RecordChanges {
                reset: true,
                words: Vec::new(),
            },
        );
    }
    changes
}

pub fn union_changes(union: &mut ChangeSet, changes: &ChangeSet) {
    for (&key, change) in changes {
        let entry = union.entry(key).or_default();
        entry.reset |= change.reset;
        if entry.reset {
            entry.words.clear();
        } else {
            entry
                .words
                .resize(entry.words.len().max(change.words.len()), 0);
            for (target, source) in entry.words.iter_mut().zip(&change.words) {
                *target |= source;
            }
        }
    }
}

pub fn patch_from_changes(
    tick: u32,
    baseline_tick: u32,
    current: &ReplicationState,
    changes: &ChangeSet,
) -> WorldPatch {
    let records = changes
        .iter()
        .map(|(&key, change)| {
            let Some(state) = current.get(&key) else {
                return RecordPatch {
                    key,
                    state_len: None,
                    mask: Vec::new(),
                    values: Vec::new(),
                };
            };
            let mut mask = vec![0u8; state.len().div_ceil(8)];
            let capacity = if change.reset {
                state.len()
            } else {
                change
                    .words
                    .iter()
                    .map(|word| word.count_ones() as usize)
                    .sum::<usize>()
                    .min(state.len())
            };
            let mut values = Vec::with_capacity(capacity);
            if change.reset {
                mask.fill(255);
                if !state.len().is_multiple_of(8) {
                    *mask.last_mut().unwrap() = (1 << (state.len() % 8)) - 1;
                }
                values.extend_from_slice(state);
            } else {
                for (word_index, mut word) in change.words.iter().copied().enumerate() {
                    while word != 0 {
                        let offset = word_index * 64 + word.trailing_zeros() as usize;
                        word &= word - 1;
                        if let Some(&value) = state.get(offset) {
                            mask[offset / 8] |= 1 << (offset % 8);
                            values.push(value);
                        }
                    }
                }
            }
            RecordPatch {
                key,
                state_len: Some(state.len() as u32),
                mask,
                values,
            }
        })
        .collect();
    WorldPatch {
        tick,
        baseline_tick,
        records,
    }
}

pub fn build_world_patch(current: &WorldDelta, baseline: &WorldDelta) -> WorldPatch {
    let state = capture_world(current);
    let changes = changed_records(&state, &capture_world(baseline));
    patch_from_changes(current.tick, baseline.tick, &state, &changes)
}

pub fn apply_world_patch(
    baseline: &WorldDelta,
    patch: &WorldPatch,
) -> Result<WorldDelta, bincode::Error> {
    if baseline.tick != patch.baseline_tick {
        return Err(wire::invalid("invalid delta baseline"));
    }
    let mut records = capture_world(baseline);
    let mut previous_key = None;
    let mut total = records.values().map(Vec::len).sum::<usize>();
    for patch in &patch.records {
        if previous_key.is_some_and(|key| key >= patch.key) {
            return Err(wire::invalid("unordered or duplicate record"));
        }
        previous_key = Some(patch.key);
        let Some(len) = patch.state_len else {
            if !patch.mask.is_empty() || !patch.values.is_empty() {
                return Err(wire::invalid("removed record has values"));
            }
            if let Some(old) = records.remove(&patch.key) {
                total -= old.len();
            }
            continue;
        };
        let len = len as usize;
        let state = records.entry(patch.key).or_default();
        total = total.saturating_sub(state.len()).saturating_add(len);
        if total > wire::MAX_STATE_BYTES {
            return Err(wire::invalid("oversized reconstructed world"));
        }
        state.resize(len, 0);
        validate_record_patch(patch)?;
        let mut values = patch.values.iter();
        for (byte_index, &byte) in patch.mask.iter().enumerate() {
            let mut bits = byte;
            while bits != 0 {
                let offset = byte_index * 8 + bits.trailing_zeros() as usize;
                state[offset] = *values.next().unwrap();
                bits &= bits - 1;
            }
        }
    }
    let globals = records
        .remove(&StateKey::World)
        .ok_or_else(|| wire::invalid("missing world record"))?;
    let mut world: WorldDelta = wire::deserialize(&globals)?;
    if world.tick != patch.tick
        || !world.heroes.is_empty()
        || !world.enemies.is_empty()
        || !world.towers.is_empty()
    {
        return Err(wire::invalid("invalid world record"));
    }
    for (key, state) in records {
        match key {
            StateKey::Hero(id) => {
                let hero: HeroSnapshot = wire::deserialize(&state)?;
                if hero.client_id != id {
                    return Err(wire::invalid("hero identity mismatch"));
                }
                world.heroes.push(hero);
            }
            StateKey::Enemy(id) => {
                let enemy: EnemySnapshot = wire::deserialize(&state)?;
                if enemy.id != id {
                    return Err(wire::invalid("enemy identity mismatch"));
                }
                world.enemies.push(enemy);
            }
            StateKey::Tower(id) => {
                let tower: TowerSnapshot = wire::deserialize(&state)?;
                if tower.id != id {
                    return Err(wire::invalid("tower identity mismatch"));
                }
                world.towers.push(tower);
            }
            StateKey::World => unreachable!(),
        }
    }
    Ok(world)
}

/// Check masks before either serialization or applying them to a baseline.
pub(crate) fn validate_record_patch(record: &RecordPatch) -> Result<(), bincode::Error> {
    let Some(len) = record.state_len else {
        return if record.mask.is_empty() && record.values.is_empty() {
            Ok(())
        } else {
            Err(wire::invalid("removed record has values"))
        };
    };
    let len = len as usize;
    if len > wire::MAX_STATE_BYTES
        || record.mask.len() != len.div_ceil(8)
        || (!len.is_multiple_of(8)
            && record
                .mask
                .last()
                .is_some_and(|byte| byte >> (len % 8) != 0))
        || record
            .mask
            .iter()
            .map(|byte| byte.count_ones() as usize)
            .sum::<usize>()
            != record.values.len()
    {
        return Err(wire::invalid("invalid record byte mask"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_recovery_preserves_exact_bytes_through_resize_removal_and_recreation() {
        for age in [1, 2, 9, 60] {
            let key = StateKey::Enemy(u64::MAX);
            let baseline = ReplicationState::from([(key, vec![11; 129])]);
            let mut previous = baseline.clone();
            let mut current = baseline.clone();
            let mut union = ChangeSet::new();
            for tick in 1..=age {
                if tick == 5 {
                    current.remove(&key);
                } else {
                    let bytes = current.entry(key).or_default();
                    bytes.resize((tick * 47) % 257, 0);
                    for (index, byte) in bytes.iter_mut().enumerate() {
                        if (index + tick) % 5 == 0 {
                            *byte = (index ^ tick) as u8;
                        }
                    }
                }
                union_changes(&mut union, &changed_records(&current, &previous));
                previous = current.clone();
            }
            let patch = patch_from_changes(age as u32, 0, &current, &union);
            let encoded = crate::encode(&crate::ServerWorldMessage::Patch(patch));
            let crate::ServerWorldMessage::Patch(patch) = crate::decode(&encoded).unwrap() else {
                panic!()
            };
            let mut restored = baseline;
            for record in patch.records {
                validate_record_patch(&record).unwrap();
                if let Some(len) = record.state_len {
                    let target = restored.entry(record.key).or_default();
                    target.resize(len as usize, 0);
                    let mut values = record.values.into_iter();
                    for (offset, byte) in target.iter_mut().enumerate() {
                        if record.mask[offset / 8] & (1 << (offset % 8)) != 0 {
                            *byte = values.next().unwrap();
                        }
                    }
                    assert!(values.next().is_none());
                } else {
                    restored.remove(&record.key);
                }
            }
            assert_eq!(restored, current, "recovery age {age}");
        }
    }

    #[test]
    fn change_history_is_bounded_by_record_size_not_number_of_missed_frames() {
        let key = StateKey::Enemy(1);
        let mut previous = ReplicationState::from([(key, vec![0; 129])]);
        let mut union = ChangeSet::new();
        for tick in 0..1000 {
            let mut current = previous.clone();
            current.get_mut(&key).unwrap()[tick % 129] ^= 1;
            union_changes(&mut union, &changed_records(&current, &previous));
            previous = current;
        }
        assert_eq!(union[&key].words.len(), 3);
    }
}
