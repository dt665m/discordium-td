//! Targeted field corruptions of the canonical (offline) schema encoding, each
//! decoded by that root's real canonical decoder.
use super::*;
use dreamwake_sim::{collision::CollisionManifest, replication::*};
use std::ops::Range;
pub const ROOT_NAMES: [&str; 12] = [
    "global",
    "owner",
    "collision",
    "platform",
    "cover",
    "cover_marker",
    "hero",
    "enemy",
    "projectile",
    "wisp",
    "effect",
    "damage",
];
pub struct SchemaCase {
    pub root: &'static str,
    pub mutation: &'static str,
    pub bytes: Vec<u8>,
    pub rejected: bool,
}
/// One public actor of every kind, from the capture where one exists.
pub fn actors(capture: &CommittedReplication) -> Vec<(&'static str, PublicReplica)> {
    let sources = ApprovedSources::new([1]).unwrap();
    let effect = capture
        .keys()
        .into_iter()
        .find_map(|key| {
            if matches!(key, ReplicationKey::Effect(_)) {
                capture.project(key, &sources)
            } else {
                None
            }
        })
        .expect("accepted beam owns its public graphic");
    vec![
        (
            "platform",
            capture
                .keys()
                .into_iter()
                .find_map(|key| match key {
                    ReplicationKey::Platform(id) => {
                        capture.project_platform(id).map(PublicReplica::Platform)
                    }
                    _ => None,
                })
                .expect("saved platform"),
        ),
        (
            "cover",
            PublicReplica::Cover(PublicCoverView {
                revision: 1,
                present: true,
                id: 12,
                position: [0.0, 3.4, -6.0],
                half_extents: [1.5, 1.0, 0.15],
                open: true,
            }),
        ),
        (
            "cover_marker",
            PublicReplica::CoverMarker(PublicCoverMarkerView {
                id: 13,
                parent: entity(12),
                revision: 2,
                local_offset: [0.0, 1.35, 0.0],
                open: true,
            }),
        ),
        (
            "hero",
            PublicReplica::Hero(capture.project_hero(1).unwrap()),
        ),
        (
            "enemy",
            PublicReplica::Enemy(PublicEnemyView {
                id: 8,
                position: [1.25, -2.0],
                facing: [0.0, -1.0],
                velocity: [0.25, -0.5],
                hp: 14.0,
                max_hp: 20.0,
                kind: dreamwake_sim::EnemyKind::Elite,
                windup: 0.25,
                target: [2.0, 3.0],
                warn_radius: 1.0,
                phase: 1,
                slowed: true,
                hit_flash: 0.125,
            }),
        ),
        (
            "projectile",
            PublicReplica::Projectile(PublicProjectileView {
                id: 9,
                owner: Some(1),
                position: [-1.0, 2.0],
                direction: [0.0, 1.0],
                speed: 17.0,
                remaining: 1.5,
                max_distance: Some(64.0),
                radius: 0.25,
                friendly: true,
                essence: Some(dreamwake_sim::EssenceKind::Frost),
            }),
        ),
        (
            "wisp",
            PublicReplica::Wisp(PublicWispView {
                id: 10,
                owner: None,
                position: [2.0, 1.0],
                velocity: [-0.5, 0.75],
                remaining: 9.0,
                essence: Some(dreamwake_sim::EssenceKind::Echo),
            }),
        ),
        ("effect", effect),
        (
            "damage",
            PublicReplica::Damage(PublicDamageView {
                id: 11,
                position: [1.0, 2.0],
                amount: 13.0,
                critical: true,
                friendly: false,
                age: 0.125,
            }),
        ),
    ]
}
pub fn seeds(
    capture: &CommittedReplication,
    checkpoint: OwnerCheckpoint,
    actors: &[(&'static str, PublicReplica)],
) -> Vec<(&'static str, Vec<u8>)> {
    let mut values = vec![
        ("global", capture.global().encode().unwrap()),
        ("owner", checkpoint.encode().unwrap()),
        ("collision", CollisionManifest::current(1).encode().unwrap()),
    ];
    values.extend(
        actors
            .iter()
            .map(|(name, actor)| (*name, actor.encode().unwrap())),
    );
    values
}
fn is_actor(root: &str) -> bool {
    !matches!(root, "global" | "owner" | "collision")
}
/// The root's real canonical decoder.
pub fn decode(root: &str, bytes: &[u8], expected: OwnerExpectation) -> Result<(), ReplicationError> {
    match root {
        "global" => DreamGlobalView::decode(bytes).map(|_| ()),
        "owner" => OwnerCheckpoint::decode(bytes, expected).map(|_| ()),
        "collision" => CollisionManifest::decode(bytes).map(|_| ()),
        _ => PublicReplica::decode(bytes).map(|_| ()),
    }
}
fn field(bytes: &[u8], record: usize, path: &[u16]) -> Range<usize> {
    if path[0] == 0 {
        assert_eq!(bytes[record], 1);
        return field(bytes, record + 1, &path[1..]);
    }
    if path[0] == u16::MAX {
        return field(bytes, record + 2, &path[1..]);
    }
    let count = u16::from_le_bytes(bytes[record + 8..record + 10].try_into().unwrap());
    let mut at = record + 10;
    for _ in 0..count {
        let id = u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[at + 2..at + 6].try_into().unwrap()) as usize;
        let body = at + 6..at + 6 + len;
        if id == path[0] {
            return if path.len() == 1 {
                body
            } else {
                field(bytes, body.start, &path[1..])
            };
        }
        at = body.end;
    }
    panic!("synthetic schema field missing: {path:?}")
}
fn patch(
    cases: &mut Vec<SchemaCase>,
    root: &'static str,
    original: &[u8],
    path: &[u16],
    value: &[u8],
    mutation: &'static str,
) {
    let mut bytes = original.to_vec();
    let start = usize::from(is_actor(root)) + 36;
    let range = field(&bytes, start, path);
    assert!(value.len() <= range.len());
    bytes[range.start..range.start + value.len()].copy_from_slice(value);
    cases.push(SchemaCase {
        root,
        mutation,
        bytes,
        rejected: true,
    });
}
pub fn cases(seeds: &[(&'static str, Vec<u8>)]) -> Vec<SchemaCase> {
    let mut cases = Vec::new();
    for &(root, ref body) in seeds {
        cases.push(SchemaCase {
            root,
            mutation: "valid",
            bytes: body.clone(),
            rejected: false,
        });
        let record = usize::from(is_actor(root)) + 36;
        for mutation in [
            "fingerprint",
            "unknown_field",
            "missing_fields",
            "field_length",
            "duplicate_field",
        ] {
            let mut bytes = body.clone();
            match mutation {
                "fingerprint" => bytes[record - 32] ^= 1,
                "unknown_field" => {
                    bytes[record + 10..record + 12].copy_from_slice(&u16::MAX.to_le_bytes())
                }
                "missing_fields" => bytes[record + 8..record + 10].fill(0),
                "field_length" => {
                    bytes[record + 12..record + 16].copy_from_slice(&u32::MAX.to_le_bytes())
                }
                _ => {
                    let first = field(&bytes, record, &[1]);
                    let second = first.end;
                    let id = bytes[record + 10..record + 12].to_vec();
                    bytes[second..second + 2].copy_from_slice(&id);
                }
            }
            cases.push(SchemaCase {
                root,
                mutation,
                bytes,
                rejected: true,
            });
        }
        let mut add =
            |path: &[u16], value: &[u8], name| patch(&mut cases, root, body, path, value, name);
        match root {
            "global" => {
                add(&[1, 1], &0u32.to_le_bytes(), "identity");
                add(&[2], &u16::MAX.to_le_bytes(), "enum_tag");
                add(&[7], &u32::MAX.to_le_bytes(), "text_count");
                add(&[9], &f32::NAN.to_le_bytes(), "nonfinite");
            }
            "owner" => {
                add(&[11], &u32::MAX.to_le_bytes(), "required_base_count");
                add(&[8, 10, 1, 1], &2.5f32.to_le_bytes(), "fractional_ammo");
                add(&[8, 10, 2], &f32::INFINITY.to_le_bytes(), "nonfinite");
                // Owner.hero(8) -> SavedHero.combat(3) -> Combat v2.
                // Field 4 was retired; the three parallel status vectors are
                // declared as IDs 5/6/7 in schema/components.rs.
                add(&[8, 3, 5], &u32::MAX.to_le_bytes(), "status_count");
                add(
                    &[8, 3, 6],
                    &u32::MAX.to_le_bytes(),
                    "status_magnitude_count",
                );
                add(
                    &[8, 3, 7],
                    &u32::MAX.to_le_bytes(),
                    "status_remaining_count",
                );
                add(&[8, 8], &u32::MAX.to_le_bytes(), "loadout_count");
                add(&[2], &u32::MAX.to_le_bytes(), "text_count");
                add(&[8, 1, 3, 1], &u32::MAX.to_le_bytes(), "rng_key_count");
                add(&[8, 10, 3], &[2], "option_tag");
                add(&[8, 10, 4], &[2], "beam_option_tag");
                add(&[8, 10, 4, 0, 2], &0u32.to_le_bytes(), "beam_start_tick");
                add(
                    &[8, 10, 4, 0, 4, 6],
                    &u32::MAX.to_le_bytes(),
                    "beam_aim_count",
                );
                add(
                    &[8, 10, 4, 0, 4, 1, 7],
                    &0u64.to_le_bytes(),
                    "beam_sample_key",
                );
            }
            "collision" => {
                add(&[4], &u32::MAX.to_le_bytes(), "collider_count");
                add(&[3, 5], &999f32.to_le_bytes(), "collision_profile");
            }
            "platform" => {
                add(&[2, 2], &0u32.to_le_bytes(), "base_generation");
                add(&[7], &1024u16.to_le_bytes(), "base_phase");
                add(&[4], &0u32.to_le_bytes(), "motion_revision");
                add(&[8, 1], &u32::MAX.to_le_bytes(), "base_pose_count");
            }
            "cover" => {
                add(&[2], &u32::MAX.to_le_bytes(), "cover_pose_count");
                add(&[4], &[2], "cover_bool_encoding");
            }
            "cover_marker" => {
                // Parent entity and local offset bindings from the production
                // CoverMarker schema (0x110c, version 2).
                add(&[3], &0u64.to_le_bytes(), "parent_index");
                add(&[4], &0u32.to_le_bytes(), "parent_generation");
                add(&[8], &u32::MAX.to_le_bytes(), "offset_count");
            }
            "hero" => add(&[2], &u32::MAX.to_le_bytes(), "pose_count"),
            "enemy" => add(&[6], &u16::MAX.to_le_bytes(), "enum_tag"),
            "projectile" => add(&[2], &[2], "option_tag"),
            "wisp" => add(&[2], &[2], "option_tag"),
            "effect" => {
                add(&[6], &0u32.to_le_bytes(), "effect_lifetime");
                add(&[2], &u16::MAX.to_le_bytes(), "graphics_kind_tag");
                add(&[2, u16::MAX, 3], &(-1.0f32).to_le_bytes(), "beam_length");
                add(
                    &[2, u16::MAX, 1],
                    &u32::MAX.to_le_bytes(),
                    "beam_direction_count",
                );
            }
            "damage" => add(&[3], &f32::NAN.to_le_bytes(), "nonfinite"),
            _ => unreachable!(),
        }
    }
    cases
}
pub fn reason(error: &dreamwake_sim::replication::ReplicationError) -> &'static str {
    use dreamwake_sim::replication::ReplicationError as E;
    match error {
        E::WrongOwner => "wrong_owner",
        E::WrongEpoch => "wrong_epoch",
        E::WrongScene => "wrong_scene",
        E::StaleRevision => "stale_revision",
        E::InvalidIdentity => "invalid_identity",
        E::InvalidState => "invalid_state",
        E::SchemaMismatch => "schema_mismatch",
        E::BudgetExceeded => "budget",
        E::Codec(value) => match value.as_str() {
            "IncompatibleSchema" => "schema_mismatch",
            "CountBudget" => "count_budget",
            "TextBudget" => "text_budget",
            "ValueBudget" => "value_budget",
            "WireBudget" => "wire_budget",
            "AllocationBudget" => "allocation_budget",
            "NestingBudget" => "nesting_budget",
            "NonFinite" => "nonfinite",
            "NonCanonicalOrder" => "field_order",
            "Truncated" => "truncated",
            "TrailingBytes" => "trailing_bytes",
            "InvalidEncoding" => "invalid_encoding",
            "OutOfRange" => "out_of_range",
            v if v.starts_with("MissingField") => "missing_field",
            v if v.starts_with("InvalidField") => "invalid_field",
            v if v.starts_with("UnknownField") => "unknown_field",
            _ => "field_validation",
        },
        _ => "decode_rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_corruptions_cover_every_current_root_and_reject_in_real_decoder() {
        let corpus = Corpus::new();
        let names: std::collections::BTreeSet<_> =
            corpus.schema_seeds.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ROOT_NAMES.into_iter().collect());
        for case in &corpus.schema_cases {
            let result = decode(case.root, &case.bytes, corpus.expected);
            assert_eq!(
                result.is_err(),
                case.rejected,
                "{} / {}: {result:?}",
                case.root,
                case.mutation
            );
        }
    }
}
