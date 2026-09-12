use super::*;
use crate::{codec::BoundedVec, types::SchemaId};
const U: FieldSpec = FieldSpec::exact(
    Units::Unitless,
    Range::Unsigned {
        minimum: 0,
        maximum: u64::MAX,
    },
    Visibility::Public,
    Prediction::None,
);
const F: FieldSpec = FieldSpec::exact(
    Units::Metres,
    Range::Float32 {
        minimum: -100.0,
        maximum: 100.0,
    },
    Visibility::Public,
    Prediction::None,
);
crate::schema! { struct Sample(SchemaId(7),1,Representation::PublicPresentation) {
    2 => position: f32 = -0.0 => F,
    1 => counter: u64 = u64::MAX => U,
}}
crate::schema! { struct Alias(SchemaId(7),1,Representation::PublicPresentation) {
    1 => renamed: u64 = u64::MAX => U,
    2 => coordinate: f32 = 0.0 => F,
}}
fn registration<S: Schema>() -> Registration<S, S> {
    Registration {
        capture: |s| Ok(s.clone()),
        restore: Some(|_, s| Ok(s.clone())),
        dependencies: DependencyDeclaration::NoneRequired,
        lifecycle: LifecycleDeclaration::StaticOnly,
    }
}
fn registered<S: Schema>() -> RegisteredSchema<S, S> {
    SchemaRegistry::new(SchemaLimits::default())
        .unwrap()
        .register(registration())
        .unwrap()
}
#[test]
fn canonical_order_names_and_signed_zero_have_identical_fingerprints() {
    let a = registered::<Sample>();
    let b = registered::<Alias>();
    assert_eq!(a.codec().identity(), b.codec().identity());
    let bytes = a.codec().encode(&Sample::defaults()).unwrap();
    assert_eq!(bytes, b.codec().encode(&Alias::defaults()).unwrap());
    assert_eq!(
        &bytes[36..],
        &[
            7, 0, 0, 0, 1, 0, 0, 0, 2, 0, 1, 0, 8, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255,
            2, 0, 4, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    let decoded = a.codec().decode(&bytes).unwrap();
    assert_eq!(decoded.counter, u64::MAX);
    assert_eq!(decoded.position.to_bits(), 0);
}
#[test]
fn strict_framing_rejects_unknown_duplicate_truncated_and_incompatible() {
    let r = registered::<Sample>();
    let bytes = r.codec().encode(&Sample::defaults()).unwrap();
    for length in 0..bytes.len() {
        assert!(r.codec().decode(&bytes[..length]).is_err());
    }
    let mut changed = bytes.clone();
    changed[4] ^= 1;
    assert_eq!(
        r.codec().decode(&changed),
        Err(SchemaError::IncompatibleSchema)
    );
    let mut changed = bytes.clone();
    changed[46] = 3;
    assert_eq!(
        r.codec().decode(&changed),
        Err(SchemaError::UnknownField(FieldId(3)))
    );
    let mut changed = bytes.clone();
    changed[60] = 1;
    assert_eq!(
        r.codec().decode(&changed),
        Err(SchemaError::NonCanonicalOrder)
    );
    let mut changed = bytes.clone();
    changed[48..52].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(r.codec().decode(&changed), Err(SchemaError::Truncated));
    let mut changed = bytes;
    changed.push(0);
    assert_eq!(r.codec().decode(&changed), Err(SchemaError::TrailingBytes));
}
#[test]
fn floats_and_typed_ranges_are_checked_on_both_paths() {
    let r = registered::<Sample>();
    for position in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            r.codec().encode(&Sample {
                counter: 0,
                position
            }),
            Err(SchemaError::NonFinite)
        );
    }
    assert_eq!(
        r.codec().encode(&Sample {
            counter: 0,
            position: 101.0
        }),
        Err(SchemaError::OutOfRange)
    );
    let mut bytes = r.codec().encode(&Sample::defaults()).unwrap();
    bytes[66..70].copy_from_slice(&101f32.to_le_bytes());
    assert_eq!(r.codec().decode(&bytes), Err(SchemaError::OutOfRange));
    bytes[66..70].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(r.codec().decode(&bytes).is_err());
}
crate::schema! { struct Sequence(SchemaId(8),1,Representation::PublicPresentation) {
    1 => values: BoundedVec<u64,4> = BoundedVec::new(vec![]).unwrap() => U,
}}
#[test]
fn sequence_count_and_allocation_are_bounded_before_decode() {
    let r = registered::<Sequence>();
    let value = Sequence {
        values: BoundedVec::new(vec![u64::MAX, 42]).unwrap(),
    };
    let bytes = r.codec().encode(&value).unwrap();
    assert_eq!(r.codec().decode(&bytes).unwrap(), value);
    let mut hostile = bytes;
    hostile[52..56].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(r.codec().decode(&hostile), Err(SchemaError::CountBudget));
    let mut limits = SchemaLimits::default();
    limits.value_nodes = 2;
    assert!(matches!(
        SchemaRegistry::new(limits)
            .unwrap()
            .register(registration::<Sequence>()),
        Err(SchemaError::ValueBudget)
    ));
}
crate::schema! { struct Replay(SchemaId(9),1,Representation::OwnerCheckpoint) {
    1 => count: u64 = 0 => FieldSpec::exact(Units::Ticks,Range::Unsigned{minimum:0,maximum:u64::MAX},Visibility::Owner,Prediction::Replayed),
}}
crate::schema! { struct Leak(SchemaId(10),1,Representation::PublicPresentation) {
    1 => child: Replay = Replay::defaults() => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
crate::schema! { struct Downgrade(SchemaId(11),1,Representation::OwnerCheckpoint) {
    1 => child: Replay = Replay::defaults() => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Owner,Prediction::None),
}}
#[test]
fn nested_policy_cannot_smuggle_private_or_replayed_state() {
    assert!(matches!(
        SchemaRegistry::new(SchemaLimits::default())
            .unwrap()
            .register(registration::<Leak>()),
        Err(SchemaError::VisibilityDowngrade)
    ));
    assert!(matches!(
        SchemaRegistry::new(SchemaLimits::default())
            .unwrap()
            .register(registration::<Downgrade>()),
        Err(SchemaError::PredictionDowngrade)
    ));
    let mut hooks = registration::<Replay>();
    hooks.restore = None;
    assert!(matches!(
        SchemaRegistry::new(SchemaLimits::default())
            .unwrap()
            .register(hooks),
        Err(SchemaError::MissingRestore)
    ));
}
#[test]
fn restore_is_staged_and_recaptured_and_diagnostics_require_disclosure() {
    let r = registered::<Replay>();
    let before = Replay { count: 2 };
    let after = Replay { count: 3 };
    assert_eq!(r.restore_replacement(&before, &after).unwrap(), after);
    assert_eq!(before.count, 2);
    let changes = r
        .codec()
        .changes(&before, &after, DiagnosticAccess::FieldIdsOnly)
        .unwrap();
    assert_eq!(changes[0].before, None);
    assert_eq!(changes[0].after, None);
    let changes = r
        .codec()
        .changes(
            &before,
            &after,
            DiagnosticAccess::Authorized(&|f| f.spec.visibility == Visibility::Owner),
        )
        .unwrap();
    assert_eq!(changes[0].after, Some(3u64.to_le_bytes().to_vec()));
    let mut hooks = registration::<Replay>();
    hooks.restore = Some(|old, _| Ok(old.clone()));
    let r = SchemaRegistry::new(SchemaLimits::default())
        .unwrap()
        .register(hooks)
        .unwrap();
    assert_eq!(
        r.restore_replacement(&before, &after),
        Err(SchemaError::RestoreMismatch)
    );
}
#[test]
fn registry_checks_duplicates_and_exact_negotiation() {
    let mut registry = SchemaRegistry::new(SchemaLimits::default()).unwrap();
    registry.register(registration::<Sample>()).unwrap();
    assert!(matches!(
        registry.register(registration::<Alias>()),
        Err(SchemaError::DuplicateSchema)
    ));
    assert_eq!(registry.require_exact_match(registry.identities()), Ok(()));
    assert_eq!(
        registry.require_exact_match(&[]),
        Err(SchemaError::IncompatibleSchema)
    );
    let mut hooks = registration::<Sequence>();
    hooks.dependencies = DependencyDeclaration::Unspecified;
    assert!(matches!(
        registry.register(hooks),
        Err(SchemaError::MissingDependencyDeclaration)
    ));
}
crate::schema! { struct Parent(SchemaId(12),1,Representation::OwnerCheckpoint) {
    1 => child: Replay = Replay::defaults() => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Owner,Prediction::Replayed),
}}
crate::schema! { struct ChangedReplay(SchemaId(9),1,Representation::OwnerCheckpoint) {
    1 => count: u64 = 1 => FieldSpec::exact(Units::Ticks,Range::Unsigned{minimum:0,maximum:u64::MAX},Visibility::Owner,Prediction::Replayed),
}}
crate::schema! { struct Duplicated(SchemaId(13),1,Representation::PublicPresentation) {
    1 => first: u64 = 0 => U,
    1 => second: u64 = 0 => U,
}}
crate::schema! { struct InvalidDefault(SchemaId(14),1,Representation::PublicPresentation) {
    1 => value: f32 = f32::NAN => F,
}}
crate::schema! { struct Large(SchemaId(15),1,Representation::PublicPresentation) {
    1 => values: BoundedVec<[[u64;4];4],4> = BoundedVec::new(vec![]).unwrap() => U,
}}
#[test]
fn nested_id_conflicts_and_default_errors_are_transactional() {
    let mut registry = SchemaRegistry::new(SchemaLimits::default()).unwrap();
    let parent = registry.register(registration::<Parent>()).unwrap();
    let bytes = parent.codec().encode(&Parent::defaults()).unwrap();
    assert_eq!(parent.codec().decode(&bytes).unwrap(), Parent::defaults());
    assert_eq!(registry.identities().len(), 2);
    let prior = registry.identities().to_vec();
    assert!(matches!(
        registry.register(registration::<ChangedReplay>()),
        Err(SchemaError::DuplicateSchema)
    ));
    assert_eq!(registry.identities(), prior);
    assert!(matches!(
        registry.register(registration::<Duplicated>()),
        Err(SchemaError::DuplicateField(FieldId(1)))
    ));
    assert!(matches!(
        registry.register(registration::<InvalidDefault>()),
        Err(SchemaError::InvalidDefault)
    ));
    assert_eq!(registry.identities(), prior);
}
#[test]
fn registration_bounds_worst_case_wire_nesting_and_typed_allocations() {
    for (limits, error) in [
        (
            SchemaLimits {
                wire_bytes: 48,
                ..SchemaLimits::default()
            },
            SchemaError::WireBudget,
        ),
        (
            SchemaLimits {
                nesting: 1,
                ..SchemaLimits::default()
            },
            SchemaError::NestingBudget,
        ),
        (
            SchemaLimits {
                collection: 2,
                ..SchemaLimits::default()
            },
            SchemaError::CountBudget,
        ),
        (
            SchemaLimits {
                wire_bytes: 1024,
                allocation_bytes: 1024,
                ..SchemaLimits::default()
            },
            SchemaError::AllocationBudget,
        ),
    ] {
        // Nested fixed arrays allocate staging storage on each level, so the
        // allocator bound accounts for both the outer typed array and children.
        let result = SchemaRegistry::new(limits)
            .unwrap()
            .register(registration::<Large>());
        assert!(
            matches!(result,Err(ref e) if *e==error),
            "expected {error:?}"
        );
    }
}
crate::schema_enum! { enum Choice { 2=>Second, 1=>First } }
crate::schema_enum! { enum ChoiceAlias { 1=>RenamedFirst, 2=>RenamedSecond } }
crate::schema_enum! { enum Detail { 9=>Counter(Sample), 3=>Text(TextOnly) } }
crate::schema! { struct TextOnly(SchemaId(16),1,Representation::PublicPresentation) {
    1 => text: BoundedString<16> = BoundedString::default() => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
crate::schema! { struct Composite(SchemaId(17),1,Representation::PublicPresentation) {
    1 => choice: Choice = Choice::First => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
    2 => optional: Option<u64> = None => U,
    3 => detail: Detail = Detail::Text(TextOnly::defaults()) => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
crate::schema! { struct ChoiceRoot(SchemaId(18),1,Representation::PublicPresentation) {
    1 => choice: Choice = Choice::First => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
crate::schema! { struct ChoiceRenamed(SchemaId(18),1,Representation::PublicPresentation) {
    1 => renamed: ChoiceAlias = ChoiceAlias::RenamedFirst => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
#[test]
fn explicit_enum_tags_options_and_utf8_are_canonical_and_bounded() {
    let codec = registered::<Composite>();
    for optional in [None, Some(u64::MAX)] {
        for detail in [
            Detail::Text(TextOnly {
                text: BoundedString::new("夢✓").unwrap(),
            }),
            Detail::Counter(Sample::defaults()),
        ] {
            let value = Composite {
                choice: Choice::Second,
                optional,
                detail,
            };
            let bytes = codec.codec().encode(&value).unwrap();
            assert_eq!(codec.codec().decode(&bytes).unwrap(), value);
        }
    }
    assert_eq!(
        registered::<ChoiceRoot>().codec().identity(),
        registered::<ChoiceRenamed>().codec().identity()
    );
    let mut bytes = codec.codec().encode(&Composite::defaults()).unwrap();
    bytes[52..54].copy_from_slice(&999u16.to_le_bytes());
    assert!(codec.codec().decode(&bytes).is_err());
    assert!(BoundedString::<2>::new("夢").is_err());
    let text = registered::<TextOnly>();
    let mut bytes = text
        .codec()
        .encode(&TextOnly {
            text: BoundedString::new("ok").unwrap(),
        })
        .unwrap();
    bytes[56] = 255;
    assert_eq!(
        text.codec().decode(&bytes),
        Err(SchemaError::InvalidEncoding)
    );
}
crate::schema! { struct TextCollection(SchemaId(19),1,Representation::PublicPresentation) {
    1 => values: BoundedVec<BoundedString<8>,4> = BoundedVec::default() => FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
#[test]
fn aggregate_text_and_collection_work_survive_nested_maximum_analysis() {
    let text = SchemaLimits {
        text_bytes: 31,
        ..SchemaLimits::default()
    };
    assert!(matches!(
        SchemaRegistry::new(text)
            .unwrap()
            .register(registration::<TextCollection>()),
        Err(SchemaError::TextBudget)
    ));
    let items = SchemaLimits {
        collection_items: 3,
        ..SchemaLimits::default()
    };
    assert!(matches!(
        SchemaRegistry::new(items)
            .unwrap()
            .register(registration::<TextCollection>()),
        Err(SchemaError::CountBudget)
    ));
    let limit = SchemaLimits {
        text_bytes: 32,
        collection_items: 4,
        ..SchemaLimits::default()
    };
    let registered = SchemaRegistry::new(limit)
        .unwrap()
        .register(registration::<TextCollection>())
        .unwrap();
    let state = TextCollection {
        values: BoundedVec::new(vec![BoundedString::new("12345678").unwrap(); 4]).unwrap(),
    };
    let bytes = registered.codec().encode(&state).unwrap();
    assert_eq!(registered.codec().decode(&bytes).unwrap(), state);
}
#[test]
fn external_dependency_policy_cannot_masquerade_as_an_empty_closed_set() {
    let mut hooks = registration::<Sample>();
    hooks.dependencies = DependencyDeclaration::External { policy: 7 };
    let registered = SchemaRegistry::new(SchemaLimits::default())
        .unwrap()
        .register(hooks)
        .unwrap();
    assert_eq!(registered.external_dependency_policy(), Some(7));
    assert_eq!(
        registered.declared_dependencies(&Sample::defaults()),
        Err(SchemaError::DependencyContextRequired)
    );
    let mut hooks = registration::<Sample>();
    hooks.dependencies = DependencyDeclaration::External { policy: 0 };
    assert!(matches!(
        SchemaRegistry::new(SchemaLimits::default())
            .unwrap()
            .register(hooks),
        Err(SchemaError::MissingDependencyDeclaration)
    ));
}
crate::schema_enum! {enum MixedPayload {1=>Empty(()),2=>Detail(Sample)}}
crate::schema! {struct MixedRoot(SchemaId(99),1,Representation::PublicPresentation){
    1=>value:MixedPayload=MixedPayload::Empty(())=>FieldSpec::exact(Units::Unitless,Range::None,Visibility::Public,Prediction::None),
}}
#[test]
fn empty_and_typed_enum_payloads_are_bounded_and_unknown_tags_reject() {
    let codec = registered::<MixedRoot>();
    for value in [
        MixedPayload::Empty(()),
        MixedPayload::Detail(Sample::defaults()),
    ] {
        let state = MixedRoot { value };
        let bytes = codec.codec().encode(&state).unwrap();
        assert_eq!(codec.codec().decode(&bytes).unwrap(), state);
    }
    assert!(<() as FieldValue>::read(&[0], &mut Work::new(SchemaLimits::default()), 0).is_err());
    let mut bytes = codec.codec().encode(&MixedRoot::defaults()).unwrap();
    bytes[52..54].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(codec.codec().decode(&bytes).is_err());
}
