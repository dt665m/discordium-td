use super::*;
use crate::{codec::BoundedVec, types::SchemaId};

const PLAIN: FieldSpec = FieldSpec::exact(
    Units::Unitless,
    Range::None,
    Visibility::Public,
    Prediction::None,
);
const fn unsigned(maximum: u64) -> FieldSpec {
    FieldSpec::exact(
        Units::Unitless,
        Range::Unsigned {
            minimum: 0,
            maximum,
        },
        Visibility::Public,
        Prediction::None,
    )
}
const fn signed(minimum: i64, maximum: i64) -> FieldSpec {
    FieldSpec::exact(
        Units::Unitless,
        Range::Signed { minimum, maximum },
        Visibility::Public,
        Prediction::None,
    )
}
const FLOAT: FieldSpec = FieldSpec::exact(
    Units::Metres,
    Range::Float32 {
        minimum: -100.0,
        maximum: 100.0,
    },
    Visibility::Public,
    Prediction::None,
);
fn registration<S: Schema>() -> Registration<S, S> {
    Registration {
        capture: |value| Ok(value.clone()),
        restore: Some(|_, value| Ok(value.clone())),
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

crate::schema! { struct Golden(SchemaId(101),1,Representation::PublicPresentation) {
    2 => position: f32 = -0.0 => FLOAT,
    1 => counter: u64 = u64::MAX => unsigned(u64::MAX),
}}
crate::schema! { struct GoldenAlias(SchemaId(101),1,Representation::PublicPresentation) {
    1 => renamed: u64 = u64::MAX => unsigned(u64::MAX),
    2 => coordinate: f32 = 0.0 => FLOAT,
}}

#[test]
fn compact_field_order_and_aliases_preserve_the_canonical_codec() {
    let original = registered::<Golden>();
    let alias = registered::<GoldenAlias>();
    assert_eq!(original.codec().identity(), alias.codec().identity());
    let compact = original
        .codec()
        .encode_compact(&Golden::defaults())
        .unwrap();
    assert_eq!(
        compact,
        [255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0]
    );
    assert_eq!(
        compact,
        alias
            .codec()
            .encode_compact(&GoldenAlias::defaults())
            .unwrap()
    );
    let decoded = original.codec().decode_compact(&compact).unwrap();
    assert_eq!(decoded.counter, u64::MAX);
    assert_eq!(decoded.position.to_bits(), 0);
    assert_eq!(
        alias.codec().decode_compact(&compact).unwrap(),
        GoldenAlias::defaults()
    );

    // The original envelope and per-field framing remain valid independently
    // of the compact representation used after schema negotiation.
    let mut canonical = b"NSC1".to_vec();
    canonical.extend_from_slice(&original.codec().identity().fingerprint);
    canonical.extend_from_slice(&[
        101, 0, 0, 0, 1, 0, 0, 0, 2, 0, 1, 0, 8, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255,
        2, 0, 4, 0, 0, 0, 0, 0, 0, 0,
    ]);
    assert_eq!(
        original.codec().encode(&Golden::defaults()).unwrap(),
        canonical
    );
    assert_eq!(
        alias.codec().encode(&GoldenAlias::defaults()).unwrap(),
        canonical
    );
    assert_eq!(
        original.codec().decode(&canonical).unwrap(),
        Golden::defaults()
    );
    assert!(original.codec().decode(&compact).is_err());
    assert!(original.codec().decode_compact(&canonical).is_err());
}

crate::schema! { struct Scalars(SchemaId(102),1,Representation::PublicPresentation) {
    11 => double: f64 = -0.0 => FieldSpec::exact(Units::Unitless,Range::Float64{minimum:-1.0,maximum:1.0},Visibility::Public,Prediction::None),
    10 => single: f32 = -0.0 => FLOAT,
    9 => boolean: bool = true => PLAIN,
    8 => signed64: i64 = i64::MIN => signed(i64::MIN,i64::MAX),
    7 => signed32: i32 = i32::MIN => signed(i32::MIN as i64,i32::MAX as i64),
    6 => signed16: i16 = i16::MIN => signed(i16::MIN as i64,i16::MAX as i64),
    5 => signed8: i8 = i8::MIN => signed(i8::MIN as i64,i8::MAX as i64),
    4 => unsigned64: u64 = u64::MAX => unsigned(u64::MAX),
    3 => unsigned32: u32 = u32::MAX => unsigned(u32::MAX as u64),
    2 => unsigned16: u16 = u16::MAX => unsigned(u16::MAX as u64),
    1 => unsigned8: u8 = u8::MAX => unsigned(u8::MAX as u64),
}}

#[test]
fn compact_scalars_use_fixed_little_endian_widths_and_normalize_signed_zero() {
    let registered = registered::<Scalars>();
    let bytes = registered
        .codec()
        .encode_compact(&Scalars::defaults())
        .unwrap();
    let expected = [
        255, // u8
        255, 255, // u16
        255, 255, 255, 255, // u32
        255, 255, 255, 255, 255, 255, 255, 255, // u64
        128, // i8
        0, 128, // i16
        0, 0, 0, 128, // i32
        0, 0, 0, 0, 0, 0, 0, 128, // i64
        1,   // bool
        0, 0, 0, 0, // f32
        0, 0, 0, 0, 0, 0, 0, 0, // f64
    ];
    assert_eq!(bytes, expected);
    let decoded = registered.codec().decode_compact(&bytes).unwrap();
    assert_eq!(decoded, Scalars::defaults());
    assert_eq!(decoded.single.to_bits(), 0);
    assert_eq!(decoded.double.to_bits(), 0);

    let mut signed_zero = bytes;
    signed_zero[31..35].copy_from_slice(&(-0.0f32).to_le_bytes());
    signed_zero[35..43].copy_from_slice(&(-0.0f64).to_le_bytes());
    let decoded = registered.codec().decode_compact(&signed_zero).unwrap();
    assert_eq!(decoded.single.to_bits(), 0);
    assert_eq!(decoded.double.to_bits(), 0);
    assert_eq!(
        registered.codec().encode_compact(&decoded).unwrap(),
        expected
    );
}

crate::schema! { struct Child(SchemaId(103),1,Representation::PublicPresentation) {
    2 => text: BoundedString<8> = BoundedString::default() => PLAIN,
    1 => number: i16 = 0 => signed(-30,30),
}}
crate::schema_enum! { enum Choice { 9 => Last, 3 => First } }
crate::schema_enum! { enum Payload { 4 => Empty(()), 12 => Child(Child) } }
crate::schema! { struct Composite(SchemaId(104),1,Representation::PublicPresentation) {
    7 => tail: u8 = 0 => unsigned(u8::MAX as u64),
    6 => text: BoundedString<16> = BoundedString::default() => PLAIN,
    5 => values: BoundedVec<Option<i32>,4> = BoundedVec::default() => signed(i32::MIN as i64,i32::MAX as i64),
    4 => payload: Payload = Payload::Empty(()) => PLAIN,
    3 => choice: Choice = Choice::First => PLAIN,
    2 => optional: Option<Child> = None => PLAIN,
    1 => fixed: [[u16;2];2] = [[0;2];2] => unsigned(u16::MAX as u64),
}}

fn composite() -> Composite {
    Composite {
        fixed: [[1, 2], [3, 4]],
        optional: Some(Child {
            number: -7,
            text: BoundedString::new("夢").unwrap(),
        }),
        choice: Choice::Last,
        payload: Payload::Child(Child {
            number: 22,
            text: BoundedString::new("ok").unwrap(),
        }),
        values: BoundedVec::new(vec![Some(-11), None, Some(i32::MAX)]).unwrap(),
        text: BoundedString::new("hi✓").unwrap(),
        tail: 173,
    }
}

#[test]
fn compact_composites_have_only_required_counts_and_tags() {
    let registered = registered::<Composite>();
    let value = composite();
    let bytes = registered.codec().encode_compact(&value).unwrap();
    assert_eq!(
        bytes,
        [
            1, 0, 2, 0, 3, 0, 4, 0, // fixed arrays have no counts or lengths
            1, 249, 255, 3, 0, 0, 0, 229, 164, 162, // Some(Child(-7, "夢"))
            9, 0, // unit enum
            12, 0, 22, 0, 2, 0, 0, 0, 111, 107, // payload enum with Child(22, "ok")
            3, 0, 0, 0, 1, 245, 255, 255, 255, 0, 1, 255, 255, 255, 127, // vector
            5, 0, 0, 0, 104, 105, 226, 156, 147, // UTF-8 byte count and text
            173, // a following field must survive variable-length children
        ]
    );
    assert_eq!(registered.codec().decode_compact(&bytes).unwrap(), value);
    assert!(bytes.len() < registered.codec().encode(&value).unwrap().len());
    for optional in [None, value.optional.clone()] {
        for payload in [Payload::Empty(()), value.payload.clone()] {
            for values in [
                BoundedVec::default(),
                BoundedVec::new(vec![None, Some(i32::MIN), Some(0), Some(i32::MAX)]).unwrap(),
            ] {
                let state = Composite {
                    optional: optional.clone(),
                    payload: payload.clone(),
                    values,
                    ..value.clone()
                };
                let bytes = registered.codec().encode_compact(&state).unwrap();
                assert_eq!(registered.codec().decode_compact(&bytes).unwrap(), state);
            }
        }
    }
}

#[test]
fn compact_decode_rejects_every_truncation_and_trailing_bytes() {
    let registered = registered::<Composite>();
    let bytes = registered.codec().encode_compact(&composite()).unwrap();
    for length in 0..bytes.len() {
        assert_eq!(
            registered.codec().decode_compact(&bytes[..length]),
            Err(SchemaError::Truncated),
            "accepted or misclassified prefix length {length}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert_eq!(
        registered.codec().decode_compact(&trailing),
        Err(SchemaError::TrailingBytes)
    );
}

crate::schema! { struct Controls(SchemaId(105),1,Representation::PublicPresentation) {
    1 => boolean: bool = false => PLAIN,
    2 => optional: Option<u16> = None => unsigned(u16::MAX as u64),
    3 => choice: Choice = Choice::First => PLAIN,
    4 => payload: Payload = Payload::Empty(()) => PLAIN,
    5 => text: BoundedString<16> = BoundedString::default() => PLAIN,
    6 => values: BoundedVec<u16,4> = BoundedVec::default() => unsigned(u16::MAX as u64),
}}

#[test]
fn compact_tags_counts_and_utf8_reject_malformed_values() {
    let registered = registered::<Controls>();
    let bytes = registered
        .codec()
        .encode_compact(&Controls::defaults())
        .unwrap();
    assert_eq!(bytes, [0, 0, 3, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    for (offset, replacement) in [
        (0, vec![2]),
        (1, vec![2]),
        (2, u16::MAX.to_le_bytes().to_vec()),
        (4, u16::MAX.to_le_bytes().to_vec()),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + replacement.len()].copy_from_slice(&replacement);
        assert_eq!(
            registered.codec().decode_compact(&invalid),
            Err(SchemaError::InvalidEncoding)
        );
    }
    for count in [17, u32::MAX] {
        let mut invalid = bytes.clone();
        invalid[6..10].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            registered.codec().decode_compact(&invalid),
            Err(SchemaError::TextBudget)
        );
    }
    for count in [5, u32::MAX] {
        let mut invalid = bytes.clone();
        invalid[10..14].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            registered.codec().decode_compact(&invalid),
            Err(SchemaError::CountBudget)
        );
    }
    let value = Controls {
        text: BoundedString::new("ok").unwrap(),
        ..Controls::defaults()
    };
    let mut invalid = registered.codec().encode_compact(&value).unwrap();
    invalid[10] = 255;
    assert_eq!(
        registered.codec().decode_compact(&invalid),
        Err(SchemaError::InvalidEncoding)
    );
}

#[test]
fn compact_encode_and_decode_enforce_float_and_nested_integer_ranges() {
    let golden = registered::<Golden>();
    for position in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            golden.codec().encode_compact(&Golden {
                position,
                ..Golden::defaults()
            }),
            Err(SchemaError::NonFinite)
        );
        let mut bytes = golden.codec().encode_compact(&Golden::defaults()).unwrap();
        bytes[8..12].copy_from_slice(&position.to_le_bytes());
        assert_eq!(
            golden.codec().decode_compact(&bytes),
            Err(SchemaError::InvalidEncoding)
        );
    }
    for position in [-101.0f32, 101.0] {
        assert_eq!(
            golden.codec().encode_compact(&Golden {
                position,
                ..Golden::defaults()
            }),
            Err(SchemaError::OutOfRange)
        );
        let mut bytes = golden.codec().encode_compact(&Golden::defaults()).unwrap();
        bytes[8..12].copy_from_slice(&position.to_le_bytes());
        assert_eq!(
            golden.codec().decode_compact(&bytes),
            Err(SchemaError::OutOfRange)
        );
    }
    let registered = registered::<Composite>();
    let mut value = composite();
    value.optional.as_mut().unwrap().number = 31;
    assert_eq!(
        registered.codec().encode_compact(&value),
        Err(SchemaError::OutOfRange)
    );
    let mut bytes = registered.codec().encode_compact(&composite()).unwrap();
    bytes[9..11].copy_from_slice(&31i16.to_le_bytes());
    assert_eq!(
        registered.codec().decode_compact(&bytes),
        Err(SchemaError::OutOfRange)
    );
    // An enum payload gets the same post-decode field policy validation.
    let mut bytes = registered.codec().encode_compact(&composite()).unwrap();
    bytes[22..24].copy_from_slice(&(-31i16).to_le_bytes());
    assert_eq!(
        registered.codec().decode_compact(&bytes),
        Err(SchemaError::OutOfRange)
    );
}

crate::schema! { struct EmptyCollections(SchemaId(106),1,Representation::PublicPresentation) {
    1 => values: BoundedVec<(),4> = BoundedVec::default() => PLAIN,
    2 => fixed: [();3] = [();3] => PLAIN,
    3 => optional: Option<()> = None => PLAIN,
    4 => tail: bool = false => PLAIN,
}}

#[test]
fn compact_zero_width_children_preserve_counts_options_and_following_fields() {
    let registered = registered::<EmptyCollections>();
    let value = EmptyCollections {
        values: BoundedVec::new(vec![(); 4]).unwrap(),
        fixed: [(); 3],
        optional: Some(()),
        tail: true,
    };
    let bytes = registered.codec().encode_compact(&value).unwrap();
    assert_eq!(bytes, [4, 0, 0, 0, 1, 1]);
    assert_eq!(registered.codec().decode_compact(&bytes).unwrap(), value);
    assert_eq!(
        registered.codec().decode_compact(&[5, 0, 0, 0, 1, 1]),
        Err(SchemaError::CountBudget)
    );
}

fn read_compact<T: CompactValue>(bytes: &[u8], limits: SchemaLimits) -> Result<T, SchemaError> {
    let mut reader = Reader::new(bytes);
    let value = T::read_compact(&mut reader, &mut Work::new(limits), 0)?;
    reader.finish()?;
    Ok(value)
}
fn assert_work_limit<T: CompactValue + std::fmt::Debug>(
    value: T,
    bytes: &[u8],
    limits: SchemaLimits,
    error: SchemaError,
) {
    assert_eq!(read_compact::<T>(bytes, limits), Err(error.clone()));
    assert_eq!(
        value.write_compact(&mut Vec::new(), &mut Work::new(limits), 0),
        Err(error)
    );
}

#[test]
fn compact_runtime_work_accounts_for_nested_collections_text_nodes_and_depth() {
    // Exercise runtime accounting independently of registration's conservative
    // worst-case analysis, including work shared by consecutive children.
    assert_work_limit(
        [
            BoundedVec::<u8, 2>::new(vec![1, 2]).unwrap(),
            BoundedVec::new(vec![3, 4]).unwrap(),
        ],
        &[2, 0, 0, 0, 1, 2, 2, 0, 0, 0, 3, 4],
        SchemaLimits {
            collection_items: 5,
            ..SchemaLimits::default()
        },
        SchemaError::CountBudget,
    );
    assert_work_limit(
        [
            BoundedString::<3>::new("one").unwrap(),
            BoundedString::new("two").unwrap(),
        ],
        &[3, 0, 0, 0, 111, 110, 101, 3, 0, 0, 0, 116, 119, 111],
        SchemaLimits {
            text_bytes: 5,
            ..SchemaLimits::default()
        },
        SchemaError::TextBudget,
    );
    assert_work_limit(
        [1u8, 2],
        &[1, 2],
        SchemaLimits {
            value_nodes: 2,
            ..SchemaLimits::default()
        },
        SchemaError::ValueBudget,
    );
    assert_work_limit(
        Some(Some(7u8)),
        &[1, 1, 7],
        SchemaLimits {
            nesting: 1,
            ..SchemaLimits::default()
        },
        SchemaError::NestingBudget,
    );
    let limits = SchemaLimits {
        allocation_bytes: 31,
        ..SchemaLimits::default()
    };
    let mut bytes = 4u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0; 32]);
    assert_eq!(
        read_compact::<BoundedVec<u64, 4>>(&bytes, limits),
        Err(SchemaError::AllocationBudget)
    );
    assert_eq!(
        read_compact::<[u64; 4]>(&[0; 32], limits),
        Err(SchemaError::AllocationBudget)
    );
    assert_eq!(
        read_compact::<BoundedString<8>>(
            &[8, 0, 0, 0, 97, 98, 99, 100, 101, 102, 103, 104],
            SchemaLimits {
                allocation_bytes: 7,
                ..SchemaLimits::default()
            }
        ),
        Err(SchemaError::AllocationBudget)
    );
}

#[test]
fn compact_codec_retains_registration_and_input_wire_budgets() {
    let limits = SchemaLimits {
        wire_bytes: 70,
        ..SchemaLimits::default()
    };
    let registered = SchemaRegistry::new(limits)
        .unwrap()
        .register(registration::<Golden>())
        .unwrap();
    let bytes = registered
        .codec()
        .encode_compact(&Golden::defaults())
        .unwrap();
    assert_eq!(
        registered.codec().decode_compact(&bytes).unwrap(),
        Golden::defaults()
    );
    assert_eq!(
        registered.codec().decode_compact(&[0; 71]),
        Err(SchemaError::WireBudget)
    );
    assert!(matches!(
        SchemaRegistry::new(SchemaLimits {
            wire_bytes: 48,
            ..limits
        })
        .unwrap()
        .register(registration::<Golden>()),
        Err(SchemaError::WireBudget)
    ));
    assert_eq!(
        [0u8; 71].write_compact(&mut Vec::new(), &mut Work::new(limits), 0),
        Err(SchemaError::WireBudget)
    );
}
