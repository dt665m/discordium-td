//! Targeted generated-field cases retain valid outer protocol/compression framing.
use super::*;
use dreamwake_sim::{collision::CollisionManifest, replication::*};
use std::ops::Range;
const DTO_BYTES: usize = 16_374;
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
pub fn seeds(
    capture: &CommittedReplication,
    checkpoint: OwnerCheckpoint,
) -> Vec<(&'static str, Vec<u8>)> {
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
    let actors = [
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
                parent: engine_net::types::ScopeIdentity {
                    connection: engine_net::types::ConnectionEpoch(1),
                    entity: engine_net::types::EntityId {
                        index: 12,
                        generation: 1,
                    },
                    scope: engine_net::types::ScopeEpoch(2),
                    representation: engine_net::types::RepresentationRevision(1),
                },
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
    ];
    let mut values = vec![
        (
            "global",
            encode_replica(&ReplicaPayload::Global(capture.global().clone())).unwrap(),
        ),
        (
            "owner",
            encode_replica(&ReplicaPayload::Owner(checkpoint)).unwrap(),
        ),
        (
            "collision",
            encode_replica(&ReplicaPayload::Collision(CollisionManifest::current(1))).unwrap(),
        ),
    ];
    values.extend(
        actors
            .into_iter()
            .map(|(name, v)| (name, encode_replica(&ReplicaPayload::Actor(v)).unwrap())),
    );
    values
}
fn unwrap(bytes: &[u8]) -> (u8, Vec<u8>) {
    let (&tag, body) = bytes.split_first().unwrap();
    let value: BoundedVec<u8, DTO_BYTES> =
        engine_net::decode_with_limit(body, DTO_BYTES + 8).unwrap();
    (tag, value.into_vec())
}
fn wrap(tag: u8, body: Vec<u8>) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(engine_net::encode(
        &BoundedVec::<u8, DTO_BYTES>::new(body).unwrap(),
    ));
    bytes
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
    tag: u8,
    original: &[u8],
    path: &[u16],
    value: &[u8],
    mutation: &'static str,
) {
    let mut bytes = original.to_vec();
    let start = usize::from(tag == 2) + 36;
    let range = field(&bytes, start, path);
    assert!(value.len() <= range.len());
    bytes[range.start..range.start + value.len()].copy_from_slice(value);
    cases.push(SchemaCase {
        root,
        mutation,
        bytes: wrap(tag, bytes),
        rejected: true,
    });
}
pub fn cases(seeds: &[(&'static str, Vec<u8>)]) -> Vec<SchemaCase> {
    let mut cases = Vec::new();
    for &(root, ref payload) in seeds {
        cases.push(SchemaCase {
            root,
            mutation: "valid",
            bytes: payload.clone(),
            rejected: false,
        });
        let (tag, body) = unwrap(payload);
        let record = usize::from(tag == 2) + 36;
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
                bytes: wrap(tag, bytes),
                rejected: true,
            });
        }
        let mut add = |path: &[u16], value: &[u8], name| {
            patch(&mut cases, root, tag, &body, path, value, name)
        };
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
                add(&[4], &[0], "cover_pose_state");
            }
            "cover_marker" => {
                // Exact parent scope and local offset bindings from the
                // production CoverMarker schema (0x110c, version 1).
                add(&[2], &0u64.to_le_bytes(), "parent_connection");
                add(&[3], &0u64.to_le_bytes(), "parent_index");
                add(&[4], &0u32.to_le_bytes(), "parent_generation");
                add(&[5], &0u64.to_le_bytes(), "parent_scope");
                add(&[6], &0u32.to_le_bytes(), "parent_representation");
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
pub fn dependency_cases(global: &[u8], owner: &[u8], collision: &[u8]) -> Vec<SchemaCase> {
    let full = vec![
        state(1, global.to_vec(), 2),
        state(2, owner.to_vec(), 2),
        state(3, collision.to_vec(), 2),
    ];
    let mut cases = Vec::new();
    for mutation in [
        "valid",
        "missing_collision",
        "wrong_collision_scene",
        "collision_wrong_representation",
    ] {
        let mut members = full.clone();
        match mutation {
            "missing_collision" => {
                members.pop();
            }
            "wrong_collision_scene" => {
                members[2].payload =
                    encode_replica(&ReplicaPayload::Collision(CollisionManifest::current(2)))
                        .unwrap()
            }
            "collision_wrong_representation" => members[2].payload = global.to_vec(),
            _ => {}
        }
        let chunk = GroupChunk {
            publication: GroupPublication {
                connection: EPOCH,
                group: live::OWNER_GROUP,
                revision: GroupRevision(1),
                snapshot: SnapshotId(2),
            },
            end_tick: ServerTick(10),
            manifest: members.iter().map(|v| v.scope).collect(),
            index: 0,
            count: 1,
            members,
        };
        let bytes = encode_group(&chunk, group_limits(), |p| Ok(p.clone())).unwrap();
        cases.push(SchemaCase {
            root: "owner_group",
            mutation,
            bytes,
            rejected: mutation != "valid",
        });
    }
    cases
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
        assert_eq!(corpus.schema_seeds.len(), ROOT_NAMES.len());
        for case in &corpus.schema_cases {
            let result = decode_replica(&case.bytes, Some(corpus.expected));
            assert_eq!(
                result.is_err(),
                case.rejected,
                "{} / {}: {result:?}",
                case.root,
                case.mutation
            );
        }
        // The short randomized smoke visits only a subset of generated cases.
        // This regression always checks every root's framing and semantic cases.
        for root in ROOT_NAMES {
            for mutation in [
                "valid",
                "fingerprint",
                "unknown_field",
                "missing_fields",
                "field_length",
                "duplicate_field",
            ] {
                assert!(
                    corpus
                        .schema_cases
                        .iter()
                        .any(|case| case.root == root && case.mutation == mutation),
                    "missing {root} / {mutation}"
                );
            }
        }
    }
}
