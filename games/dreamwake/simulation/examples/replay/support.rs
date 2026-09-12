//! Identical bounded checkpoint verification for native and browser WASM.
use dreamwake_sim::{DreamCheckpoint, DreamInput, DreamSimulation};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use std::{fmt, marker::PhantomData};

pub const MAX_FIXTURE_BYTES: usize = 32 * 1024 * 1024;
const MAX_STEPS: usize = 256;
const MAX_SEGMENT_STEPS: usize = 8;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    version: u32,
    scenario: String,
    recorded_arch: String,
    recorded_os: String,
    start_step: usize,
    initial: DreamCheckpoint,
    #[serde(deserialize_with = "steps")]
    commands: Vec<DreamInput>,
    #[serde(deserialize_with = "steps")]
    expected: Vec<DreamCheckpoint>,
}
fn steps<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    struct Bounded<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "at most {MAX_STEPS} replay steps")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            if seq.size_hint().is_some_and(|size| size > MAX_STEPS) {
                return Err(de::Error::custom("replay step budget exceeded"));
            }
            let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_STEPS));
            while values.len() < MAX_STEPS {
                let Some(value) = seq.next_element()? else {
                    return Ok(values);
                };
                values.push(value);
            }
            if seq.next_element::<de::IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("replay step budget exceeded"));
            }
            Ok(values)
        }
    }
    d.deserialize_seq(Bounded(PhantomData))
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub scenario: String,
    recorded_arch: String,
    recorded_os: String,
    verified_arch: &'static str,
    verified_os: &'static str,
    pub start_step: usize,
    pub initial_digest: String,
    pub steps: usize,
    restored_boundaries: usize,
    final_tick: u64,
    pub final_digest: String,
}
#[cfg(not(target_arch = "wasm32"))]
pub fn record(
    scenario: &str,
    mut emit: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<usize, String> {
    let count = match scenario {
        "combat" => 120,
        "wall-dash" => MAX_STEPS,
        _ => return Err("unknown scenario; use combat or wall-dash".into()),
    };
    record_steps(scenario, count, &mut emit)
}
#[cfg(not(target_arch = "wasm32"))]
fn record_steps(
    scenario: &str,
    count: usize,
    mut emit: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<usize, String> {
    let mut sim = DreamSimulation::new(42, false);
    sim.continue_run();
    let initial = DreamCheckpoint::new(sim.snapshot(), 1).map_err(|e| e.to_string())?;
    let commands: Vec<_> = (0..count)
        .map(|i| DreamInput {
            movement: if scenario == "wall-dash" {
                [1.0, if i > 180 { 0.5 } else { -0.0 }]
            } else {
                [0.3, 0.2]
            },
            aim: [0.0, -1.0],
            attack: scenario == "combat",
            dash: i == 3 || i == 91 || i == 210,
            casts: [i == 3, i == 8, false, false],
            ..Default::default()
        })
        .collect();
    // Each segment starts with a complete checkpoint from the SAME continuous
    // simulation. Byte-based packing bounds files without dropping any boundary.
    let mut fixture = Fixture {
        version: 3,
        scenario: scenario.into(),
        recorded_arch: std::env::consts::ARCH.into(),
        recorded_os: std::env::consts::OS.into(),
        start_step: 0,
        initial,
        commands: Vec::new(),
        expected: Vec::new(),
    };
    let mut segment = 0;
    let mut accepted = Vec::new();
    for command in commands {
        sim.step(command);
        let checkpoint = DreamCheckpoint::new(sim.snapshot(), 1).map_err(|e| e.to_string())?;
        fixture.commands.push(command);
        fixture.expected.push(checkpoint);
        let candidate = serde_json::to_vec(&fixture).map_err(|e| e.to_string())?;
        if candidate.len() <= MAX_FIXTURE_BYTES && fixture.commands.len() <= MAX_SEGMENT_STEPS {
            accepted = candidate;
            continue;
        }
        let checkpoint = fixture.expected.pop().ok_or("missing checkpoint")?;
        fixture.commands.pop();
        if fixture.commands.is_empty() {
            return Err("one complete replay step exceeds segment byte budget".into());
        }
        emit(segment, &accepted)?;
        segment += 1;
        fixture.start_step += fixture.commands.len();
        fixture.initial = fixture.expected.pop().ok_or("missing segment boundary")?;
        fixture.commands = vec![command];
        fixture.expected = vec![checkpoint];
        accepted = serde_json::to_vec(&fixture).map_err(|e| e.to_string())?;
        if accepted.len() > MAX_FIXTURE_BYTES {
            return Err("one complete replay step exceeds segment byte budget".into());
        }
    }
    emit(segment, &accepted)?;
    Ok(segment + 1)
}

fn difference(a: &Value, b: &Value, path: String) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let Some(other) = b.get(key) else {
                    return Some(format!("{path}.{key}"));
                };
                if let Some(p) = difference(value, other, format!("{path}.{key}")) {
                    return Some(p);
                }
            }
            (a.len() != b.len()).then_some(path)
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .find_map(|(i, (a, b))| difference(a, b, format!("{path}[{i}]"))),
        _ => (a != b).then(|| {
            // This projected direction is public pose data; private checkpoint
            // fields remain path-only in diagnostic output.
            if path.ends_with(".motor.facing[0]") || path.ends_with(".motor.facing[1]") {
                format!("{path} (expected {a}, actual {b})")
            } else {
                path
            }
        }),
    }
}
fn compare(sim: &DreamSimulation, expected: &DreamCheckpoint, step: usize) -> Result<(), String> {
    let actual = sim
        .snapshot()
        .canonical_bytes(&expected.identity)
        .map_err(|e| e.to_string())?;
    let expected = expected
        .snapshot
        .canonical_bytes(&expected.identity)
        .map_err(|e| e.to_string())?;
    if actual != expected {
        let path = difference(
            &serde_json::from_slice(&expected).map_err(|e| e.to_string())?,
            &serde_json::from_slice(&actual).map_err(|e| e.to_string())?,
            "checkpoint".into(),
        )
        .unwrap_or_else(|| "encoding".into());
        return Err(format!(
            "first divergence at replay step {step}, field {path}"
        ));
    }
    Ok(())
}
pub fn verify(bytes: &[u8]) -> Result<Report, String> {
    if bytes.len() > MAX_FIXTURE_BYTES {
        return Err("fixture exceeds byte budget".into());
    }
    let f: Fixture = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if f.version != 3
        || f.start_step
            .checked_add(f.commands.len())
            .is_none_or(|end| end > MAX_STEPS)
        || f.commands.len() > MAX_SEGMENT_STEPS
        || f.commands.is_empty()
        || f.commands.len() != f.expected.len()
        || f.scenario.len() > 64
        || f.recorded_arch.len() > 32
        || f.recorded_os.len() > 32
    {
        return Err("invalid fixture version, labels, or step count".into());
    }
    let revision = f.initial.identity.scene_revision;
    f.initial.encode().map_err(|e| e.to_string())?;
    for (command, checkpoint) in f.commands.iter().zip(&f.expected) {
        if command
            .movement
            .iter()
            .chain(&command.aim)
            .any(|v| !v.is_finite() || v.abs() > 1.0)
        {
            return Err("invalid command axis".into());
        }
        if checkpoint.identity != f.initial.identity {
            return Err("mixed fixture identity".into());
        }
        checkpoint.encode().map_err(|e| e.to_string())?;
    }
    let mut sim =
        DreamSimulation::try_from_checkpoint(&f.initial, revision).map_err(|e| e.to_string())?;
    let mut restored =
        DreamSimulation::try_from_checkpoint(&f.initial, revision).map_err(|e| e.to_string())?;
    for (i, (command, expected)) in f.commands.iter().zip(&f.expected).enumerate() {
        sim.step(*command);
        compare(&sim, expected, f.start_step + i + 1)?;
        let previous = if i == 0 {
            &f.initial
        } else {
            &f.expected[i - 1]
        };
        restored
            .try_restore_checkpoint(previous, revision)
            .map_err(|e| e.to_string())?;
        restored.step(*command);
        compare(&restored, expected, f.start_step + i + 1)
            .map_err(|e| format!("after restore: {e}"))?;
    }
    let final_checkpoint = f.expected.last().ok_or("empty fixture")?;
    Ok(Report {
        scenario: f.scenario,
        recorded_arch: f.recorded_arch,
        recorded_os: f.recorded_os,
        verified_arch: std::env::consts::ARCH,
        verified_os: std::env::consts::OS,
        start_step: f.start_step,
        initial_digest: blake3::Hash::from(
            f.initial.canonical_digest().map_err(|e| e.to_string())?,
        )
        .to_hex()
        .to_string(),
        steps: f.commands.len(),
        restored_boundaries: f.commands.len(),
        final_tick: u64::from(final_checkpoint.snapshot.tick),
        final_digest: blake3::Hash::from(
            final_checkpoint
                .canonical_digest()
                .map_err(|e| e.to_string())?,
        )
        .to_hex()
        .to_string(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segments_keep_complete_boundary_and_every_command() {
        let mut segments = Vec::new();
        assert_eq!(
            record_steps("combat", MAX_SEGMENT_STEPS + 1, |index, bytes| {
                assert_eq!(index, segments.len());
                assert!(bytes.len() <= MAX_FIXTURE_BYTES);
                segments.push(bytes.to_vec());
                Ok(())
            })
            .unwrap(),
            2
        );
        let first = verify(&segments[0]).unwrap();
        let second = verify(&segments[1]).unwrap();
        assert_eq!(first.start_step, 0);
        assert_eq!(first.steps, MAX_SEGMENT_STEPS);
        assert_eq!(first.restored_boundaries, MAX_SEGMENT_STEPS);
        assert_eq!(second.start_step, MAX_SEGMENT_STEPS);
        assert_eq!(second.steps, 1);
        assert_eq!(second.restored_boundaries, 1);
        assert_eq!(first.final_digest, second.initial_digest);
        assert_eq!(second.final_tick, first.final_tick + 1);
    }
    #[test]
    fn bounded_steps_and_nested_divergence() {
        #[derive(Deserialize)]
        struct Values(#[serde(deserialize_with = "steps")] Vec<u8>);
        assert!(
            serde_json::from_str::<Values>(&format!("[{}]", vec!["0"; MAX_STEPS + 1].join(",")))
                .is_err()
        );
        assert_eq!(
            serde_json::from_str::<Values>(&format!("[{}]", vec!["0"; MAX_STEPS].join(",")))
                .unwrap()
                .0
                .len(),
            MAX_STEPS
        );
        assert_eq!(
            difference(
                &serde_json::json!({"a": [{"latent": 1}]}),
                &serde_json::json!({"a": [{"latent": 2}]}),
                "checkpoint".into()
            ),
            Some("checkpoint.a[0].latent".into())
        );
    }
}
