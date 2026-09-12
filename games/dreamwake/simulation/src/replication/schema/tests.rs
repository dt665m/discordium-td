use super::*;
fn roundtrip<S: Root>(r: &RegisteredSchema<S, S::Model>)
where
    S::Model: PartialEq + std::fmt::Debug,
{
    let state = S::defaults();
    let model = state
        .restore()
        .unwrap_or_else(|e| panic!("{}: {e:?}", std::any::type_name::<S>()));
    let encoded = encode(r, &model).unwrap();
    assert!(encoded.len() <= 16_374);
    assert_eq!(decode(r, &encoded).unwrap(), model);
    assert_eq!(
        r.codec().encode(&r.capture(&model).unwrap()).unwrap(),
        encoded
    );
    for cut in [0, 3, 35, encoded.len() - 1] {
        assert!(decode(r, &encoded[..cut]).is_err());
    }
    let mut malformed = encoded.clone();
    malformed[4] ^= 1;
    assert!(decode(r, &malformed).is_err());
    let mut malformed = encoded.clone();
    malformed[46..48].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(decode(r, &malformed).is_err());
    let mut malformed = encoded;
    malformed.push(0);
    assert!(decode(r, &malformed).is_err());
}
#[test]
fn production_registry_is_bounded_and_all_roots_roundtrip() {
    let value = Registry::new().unwrap();
    assert!(value.identities.len() > 20);
    assert!(
        value
            .identities
            .windows(2)
            .all(|v| v[0].schema < v[1].schema)
    );
    roundtrip(&value.global);
    roundtrip(&value.owner);
    roundtrip(&value.collision);
    roundtrip(&value.hero);
    roundtrip(&value.enemy);
    roundtrip(&value.projectile);
    roundtrip(&value.wisp);
    roundtrip(&value.effect);
    roundtrip(&value.damage);
    roundtrip(&value.cover);
    roundtrip(&value.cover_marker);
    roundtrip(&value.platform);
    assert_eq!(value.owner.external_dependency_policy(), Some(1));
    assert_eq!(value.cover_marker.external_dependency_policy(), Some(2));
    assert_eq!(
        value.owner.declared_dependencies(&Owner::defaults()),
        Err(SchemaError::DependencyContextRequired)
    );
}
#[test]
fn owner_cross_field_validation_and_restore_are_transactional() {
    let registry = registry();
    let valid = Owner::defaults();
    let model = valid.restore().unwrap();
    let mut bad = valid.clone();
    bad.hero.health.hp = bad.hero.health.max_hp + 1.0;
    let bytes = registry.owner.codec().encode(&bad).unwrap();
    assert!(decode_owner(&bytes).is_err());
    assert!(registry.owner.restore_replacement(&model, &bad).is_err());
    assert_eq!(
        encode_owner(&model).unwrap(),
        registry.owner.codec().encode(&valid).unwrap()
    );
    bad = valid.clone();
    bad.hero.combat.status_ids = engine_net::codec::BoundedVec::new(vec![1, 1]).unwrap();
    bad.hero.combat.status_magnitudes = engine_net::codec::BoundedVec::new(vec![0.0; 2]).unwrap();
    bad.hero.combat.status_remaining = engine_net::codec::BoundedVec::new(vec![0.0; 2]).unwrap();
    assert!(decode_owner(&registry.owner.codec().encode(&bad).unwrap()).is_err());
    bad = valid;
    bad.hero.motion.position[0] = f32::NAN;
    assert!(registry.owner.codec().encode(&bad).is_err());
}
#[test]
fn malformed_collision_configuration_is_rejected_after_typed_decode() {
    let registry = registry();
    let mut bad = Collision::defaults();
    bad.config.speed = 999.0;
    assert!(decode_collision(&registry.collision.codec().encode(&bad).unwrap()).is_err());
    bad = Collision::defaults();
    bad.schema_version = 0;
    assert!(decode_collision(&registry.collision.codec().encode(&bad).unwrap()).is_err());
}
#[test]
fn owner_maximum_collections_and_text_fit_the_wire_budget() {
    let mut owner = Owner::defaults();
    owner.context.encounter_name = BoundedString::new("x".repeat(256)).unwrap();
    owner.context.message = BoundedString::new("y".repeat(512)).unwrap();
    let reward = owner::Reward {
        title: BoundedString::new("t".repeat(128)).unwrap(),
        description: BoundedString::new("d".repeat(512)).unwrap(),
        ..owner::Reward::defaults()
    };
    owner.hero.actor.rewards = engine_net::codec::BoundedVec::new(vec![reward; 8]).unwrap();
    owner.hero.combat.status_ids = engine_net::codec::BoundedVec::new((1..=64).collect()).unwrap();
    owner.hero.combat.status_magnitudes =
        engine_net::codec::BoundedVec::new(vec![0.0; 64]).unwrap();
    owner.hero.combat.status_remaining = engine_net::codec::BoundedVec::new(vec![0.0; 64]).unwrap();
    owner.hero.defense_episodes.next_episode = 65;
    owner.hero.defense_episodes.ids =
        engine_net::codec::BoundedVec::new((1..=64).collect()).unwrap();
    owner.hero.defense_episodes.activated_ticks =
        engine_net::codec::BoundedVec::new(vec![0; 64]).unwrap();
    owner.hero.defense_episodes.expires_ticks =
        engine_net::codec::BoundedVec::new(vec![1; 64]).unwrap();
    owner.hero.defense_episodes.granted =
        engine_net::codec::BoundedVec::new(vec![1.0; 64]).unwrap();
    owner.hero.defense_episodes.spent = engine_net::codec::BoundedVec::new(vec![0.0; 64]).unwrap();
    let mut domain = crate::starfall::StarfallDomain::default();
    for sequence in 1..=8 {
        let mut flight = crate::starfall::flight(
            crate::starfall::SpawnKey {
                action: crate::combat::RayActionKey {
                    match_epoch: 1,
                    connection_epoch: 0,
                    command_stream: 0,
                    ownership_epoch: 0,
                    actor: 1,
                    actor_generation: 1,
                    command_sequence: sequence,
                    action_slot: 0,
                },
                ordinal: 0,
            },
            0,
            [0.0; 2],
            [1.0, 0.0],
            Some(crate::EssenceKind::Vast),
        );
        flight.authority_id = Some((1u64 << 63) + sequence);
        domain.flights.push(flight);
    }
    owner.starfall = (&domain).try_into().unwrap();
    let model = owner.restore().unwrap();
    let bytes = encode_owner(&model).unwrap();
    eprintln!(
        "Maximum populated owner collections plus8flights: {} bytes",
        bytes.len()
    );
    assert!(bytes.len() <= 16_374);
    assert_eq!(decode_owner(&bytes).unwrap(), model);
}
#[test]
fn private_ray_meter_and_full_width_action_identity_restore_exactly() {
    let mut owner = Owner::defaults();
    owner.hero.ray.ammo.current = 3.0;
    owner.hero.ray.cooldown = 0.375;
    owner.hero.ray.last_key = Some(owner::RayKey {
        connection_epoch: u64::MAX,
        command_stream: 9,
        ownership_epoch: 7,
        command_sequence: u64::MAX - 1,
        ..owner::RayKey::defaults()
    });
    let model = owner.restore().unwrap();
    let decoded = decode_owner(&encode_owner(&model).unwrap()).unwrap();
    assert_eq!(decoded, model);
    assert_eq!(
        decoded.hero.ray.last_key.unwrap().command_sequence,
        u64::MAX - 1
    );
    let mut bad = owner;
    bad.hero.ray.ammo.current = 2.5;
    assert!(decode_owner(&registry().owner.codec().encode(&bad).unwrap()).is_err());
}
#[test]
fn actual_owner_diagnostics_do_not_disclose_private_nested_values() {
    let before = Owner::defaults();
    let mut after = before.clone();
    after.hero.actor.critical_rng.key[0] ^= 1;
    after.hero.ray.ammo.current = 2.0;
    let changes = registry()
        .owner
        .codec()
        .changes(
            &before,
            &after,
            DiagnosticAccess::Authorized(&|field| field.spec.visibility == Visibility::Public),
        )
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, FieldId(8));
    assert!(
        changes
            .iter()
            .all(|v| v.before.is_none() && v.after.is_none())
    );
    let granted = registry()
        .owner
        .codec()
        .changes(
            &before,
            &after,
            DiagnosticAccess::Authorized(&|field| field.spec.visibility == Visibility::Owner),
        )
        .unwrap();
    assert!(granted[0].before.is_some() && granted[0].after.is_some());
}
#[test]
fn beam_episode_and_latest_sample_restore_with_distinct_full_keys() {
    let mut state = Owner::defaults();
    state.context.stamp.server_tick = 5;
    state.context.stamp.gameplay_tick = 5;
    state.hero.combat_identity.tick = 5;
    let begin = owner::RayKey {
        command_sequence: u64::from(u32::MAX),
        ..owner::RayKey::defaults()
    };
    let sample = owner::RayKey {
        command_sequence: u64::MAX - 1,
        ..begin.clone()
    };
    state.hero.ray.last_key = Some(sample.clone());
    state.hero.ray.beam = Some(owner::BeamEpisode {
        key: begin,
        started_tick: 1,
        last_damage_tick: 1,
        sample: owner::BeamSample {
            key: sample,
            execution_server_tick: 5,
            query_server_tick: 4,
            query_gameplay_tick: 4,
            query_fraction: 0,
            accepted_gameplay_tick: 5,
            aim: [0.6, 0.8],
        },
    });
    let model = state.restore().unwrap();
    let restored = decode_owner(&encode_owner(&model).unwrap()).unwrap();
    assert_eq!(restored, model);
    assert_eq!(
        restored.hero.ray.beam.unwrap().sample.key.command_sequence,
        u64::MAX - 1
    );
    let mut malformed = state.clone();
    malformed.hero.ray.beam.as_mut().unwrap().sample.aim = [0.0, 0.0];
    assert!(decode_owner(&registry().owner.codec().encode(&malformed).unwrap()).is_err());
    malformed = state;
    malformed
        .hero
        .ray
        .beam
        .as_mut()
        .unwrap()
        .sample
        .query_server_tick = 6;
    assert!(decode_owner(&registry().owner.codec().encode(&malformed).unwrap()).is_err());
}
#[test]
fn public_beam_graphics_fields_are_exact_and_validate_geometry_bounds() {
    let mut state = Effect::defaults();
    state.kind = GraphicKind::Beam(BeamGraphic {
        direction: [0.6, 0.0, 0.8],
        elevation: 1.25,
        length: 20.0,
    });
    let model = state.restore().unwrap();
    assert_eq!(
        decode(
            &registry().effect,
            &encode(&registry().effect, &model).unwrap()
        )
        .unwrap(),
        model
    );
    if let GraphicKind::Beam(beam) = &mut state.kind {
        beam.length = -1.0;
    }
    assert!(registry().effect.codec().encode(&state).is_err());
}

#[test]
fn cover_content_and_moving_pose_are_validated_after_staged_decode() {
    let mut state = Cover::defaults();
    state.open = true;
    for height in [1.0, 2.2, 3.4] {
        state.position[1] = height;
        let model = state.restore().unwrap();
        assert_eq!(
            decode(
                &registry().cover,
                &encode(&registry().cover, &model).unwrap()
            )
            .unwrap(),
            model
        );
    }
    for height in [0.99, 3.41] {
        state.position[1] = height;
        assert!(
            decode(
                &registry().cover,
                &registry().cover.codec().encode(&state).unwrap()
            )
            .is_err()
        );
    }
}
#[test]
fn starfall_maximum_schema_budget() {
    let mut limits = SchemaLimits::default();
    limits.wire_bytes = 1024 * 1024;
    let bytes = record_maximum::<Owner>(&mut Work::new(limits), 0).unwrap() + 36;
    let entry = <starfall::Entry as FieldValue>::maximum_size(&mut Work::new(limits), 0).unwrap();
    eprintln!(
        "Owner schema maximum bytes including Starfall: {bytes}, each entry {entry}, empty domain {}",
        bytes - 8 * (entry + 4)
    );
    assert!(bytes <= 16_374, "owner maximum {bytes}");
}

#[test]
fn owner_group_maxima_are_explicit() {
    let maxima = owner_group_schema_maxima().unwrap();
    eprintln!("Owner-group schema maxima [global,owner,collision,platform]: {maxima:?}");
    assert!(maxima.iter().all(|n| *n <= 16_374));
}
#[test]
fn shield_columns_reject_mismatched_lengths_and_invalid_intervals() {
    let mut columns = owner::DefenseEpisodes::defaults();
    columns.next_episode = 2;
    columns.ids = engine_net::codec::BoundedVec::new(vec![1]).unwrap();
    assert!(crate::state::DefenseEpisodes::try_from(columns.clone()).is_err());
    columns.activated_ticks = engine_net::codec::BoundedVec::new(vec![0]).unwrap();
    columns.expires_ticks = engine_net::codec::BoundedVec::new(vec![1]).unwrap();
    columns.granted = engine_net::codec::BoundedVec::new(vec![1.0]).unwrap();
    columns.spent = engine_net::codec::BoundedVec::new(vec![0.0]).unwrap();
    let mut value = Owner::defaults();
    value.hero.defense_episodes = columns.clone();
    assert!(value.restore().is_ok());
    columns.expires_ticks = engine_net::codec::BoundedVec::new(vec![0]).unwrap();
    value.hero.defense_episodes = columns;
    assert!(value.restore().is_err());
}

#[test]
fn status_columns_preserve_all_sixty_four_ids_order_and_float_bits() {
    let statuses = (0..64)
        .map(|n| engine_core::TimedStatus {
            id: engine_core::StatusId(u16::MAX - n),
            magnitude: f32::from_bits(0x3f800000 + u32::from(n)),
            remaining: f32::from_bits(0x40000000 + u32::from(n)),
        })
        .collect();
    let model = engine_core::CombatState {
        statuses,
        ..Default::default()
    };
    let wire = Combat::try_from(&model).unwrap();
    assert_eq!(
        engine_core::CombatState::try_from(wire.clone()).unwrap(),
        model
    );
    let mut bad = wire;
    bad.status_remaining = engine_net::codec::BoundedVec::default();
    assert!(engine_core::CombatState::try_from(bad).is_err());
}

#[test]
fn owner_charge_curve_phase_and_resource_roundtrip_and_version_fence() {
    let mut model = Owner::defaults().restore().unwrap();
    model.hero.charge.state.episode = 7;
    model
        .hero
        .charge
        .stamina
        .try_spend(crate::CHARGE_STAMINA_COST);
    for cursor in 0..12 {
        model.hero.charge.state.phase = engine_core::ChargePhase::Executing {
            curve: crate::CHARGE_CURVE,
            cursor,
            direction: [1.0, 0.0],
            distance: 7.5,
        };
        let bytes = encode_owner(&model).unwrap();
        assert_eq!(decode_owner(&bytes).unwrap(), model);
    }
    let mut wire = owner::Owner::capture(&model).unwrap();
    wire.hero.charge.curve_version += 1;
    assert!(decode_owner(&registry().owner.codec().encode(&wire).unwrap()).is_err());
    model.hero.charge.state.phase = engine_core::ChargePhase::Charging { ticks: 36 };
    assert_eq!(decode_owner(&encode_owner(&model).unwrap()).unwrap(), model);
}

#[test]
fn compact_public_roots_keep_canonical_state_and_game_validation() {
    use crate::replication::PublicReplica as P;
    let mut beam = Effect::defaults();
    beam.kind = GraphicKind::Beam(BeamGraphic {
        direction: [0.6, 0.0, 0.8],
        elevation: 1.25,
        length: 20.0,
    });
    let values = [
        P::Hero(Hero::defaults().restore().unwrap()),
        P::Enemy(Enemy::defaults().restore().unwrap()),
        P::Projectile(Projectile::defaults().restore().unwrap()),
        P::Wisp(Wisp::defaults().restore().unwrap()),
        P::Effect(Effect::defaults().restore().unwrap()),
        P::Effect(beam.restore().unwrap()),
        P::Damage(Damage::defaults().restore().unwrap()),
        P::Cover(Cover::defaults().restore().unwrap()),
        P::CoverMarker(CoverMarker::defaults().restore().unwrap()),
        P::Platform(Platform::defaults().restore().unwrap()),
    ];
    for value in values {
        let canonical = value.encode().unwrap();
        let compact = value.encode_compact().unwrap();
        assert!(compact.len() < canonical.len());
        assert_eq!(P::decode(&canonical).unwrap(), value);
        assert_eq!(P::decode_compact(&compact).unwrap(), value);
        assert_eq!(value.encode().unwrap(), canonical);
        for cut in 0..compact.len() {
            assert!(P::decode_compact(&compact[..cut]).is_err());
        }
        let mut trailing = compact.clone();
        trailing.push(0);
        assert!(P::decode_compact(&trailing).is_err());
        let mut unknown = compact;
        unknown[0] = 255;
        assert!(P::decode_compact(&unknown).is_err());
    }
    // The registered field range permits every u64; game identity validation
    // still rejects zero after typed staging in the compact live path.
    let mut invalid = Enemy::defaults();
    invalid.id = 0;
    let mut bytes = vec![2];
    bytes.extend(registry().enemy.codec().encode_compact(&invalid).unwrap());
    assert!(P::decode_compact(&bytes).is_err());
    let mut invalid = Cover::defaults();
    invalid.position[1] = 0.99;
    let mut bytes = vec![7];
    bytes.extend(registry().cover.codec().encode_compact(&invalid).unwrap());
    assert!(P::decode_compact(&bytes).is_err());
}
