use dreamwake_sim::{replication::*, *};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
pub fn fixture() -> Result<String, Box<dyn std::error::Error>> {
    let sim = DreamSimulation::new(17, false);
    let stamp = ReplicationStamp {
        match_epoch: 1,
        server_tick: 9,
        gameplay_tick: 0,
        scene_revision: 1,
        revision: 10,
    };
    let capture = sim.capture_replication(stamp)?;
    let owner = capture.owner_checkpoint(1, 1)?.with_input_continuity(
        engine_net::types::ServerTick(stamp.server_tick),
        engine_net::commands::InputContinuity {
            last_input: Some(DreamInput {
                movement: [0.6, 0.8],
                aim: [-1.0, 0.0],
                attack: true,
                ..Default::default()
            }),
            missing_streak: 2,
        },
    )?;
    let mut result = vec![("registry".to_owned(), hex(&schema_identity()))];
    let global = capture.global();
    let bytes = global.encode()?;
    assert_eq!(DreamGlobalView::decode(&bytes)?, *global);
    result.push(("global".to_owned(), hex(blake3::hash(&bytes).as_bytes())));
    let bytes = owner.encode()?;
    assert_eq!(OwnerCheckpoint::decode(&bytes, owner.expectation())?, owner);
    result.push(("owner".to_owned(), hex(blake3::hash(&bytes).as_bytes())));
    let collision = collision::CollisionManifest::current(1);
    let bytes = collision.encode()?;
    assert_eq!(collision::CollisionManifest::decode(&bytes)?, collision);
    result.push(("collision".to_owned(), hex(blake3::hash(&bytes).as_bytes())));
    let replicas = [
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
                kind: EnemyKind::Elite,
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
                essence: Some(EssenceKind::Frost),
            }),
        ),
        (
            "wisp",
            PublicReplica::Wisp(PublicWispView {
                id: 10,
                owner: None,
                position: [2.0, 1.0],
                remaining: 9.0,
                essence: Some(EssenceKind::Echo),
            }),
        ),
        (
            "effect",
            PublicReplica::Effect(PublicEffectView {
                id: engine_core::GraphicsId {
                    scope: None,
                    match_epoch: 1,
                    owner: 1,
                    action_seq: 7,
                    slot: 2,
                },
                kind: engine_core::GraphicsKind::Beam {
                    direction: [0.6, 0.0, 0.8],
                    elevation: 1.25,
                    length: 20.0,
                },
                position: [3.0, 4.0],
                radius: 2.0,
                age_ticks: 4,
                duration_ticks: 10,
            }),
        ),
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
    for (name, replica) in replicas {
        let bytes = replica.encode()?;
        assert_eq!(PublicReplica::decode(&bytes)?, replica);
        result.push((name.to_owned(), hex(blake3::hash(&bytes).as_bytes())));
        let bytes = replica.encode_compact()?;
        assert_eq!(PublicReplica::decode_compact(&bytes)?, replica);
        result.push((
            format!("compact_{name}"),
            hex(blake3::hash(&bytes).as_bytes()),
        ));
    }
    Ok(serde_json::to_string(&result)?)
}
