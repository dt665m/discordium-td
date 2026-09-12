//! `cargo run -p dreamwake_sim --example replay -- record PATH [combat|wall-dash]`
//! `cargo run -p dreamwake_sim --example replay -- verify PATH`
#[path = "replay/support.rs"]
mod support;
use serde::{Deserialize, Serialize};
use std::{error::Error, fs, io::Read, path::Path};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    scenario: String,
    steps: usize,
    segments: Vec<String>,
}
fn read(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(support::MAX_FIXTURE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > support::MAX_FIXTURE_BYTES {
        return Err("fixture exceeds byte budget".into());
    }
    Ok(bytes)
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        return Err("usage: replay record PATH [combat|wall-dash] | verify PATH".into());
    }
    let path = Path::new(&args[2]);
    let parent = path.parent().unwrap_or(Path::new("."));
    match args[1].as_str() {
        "record" => {
            let scenario = args.get(3).map_or("combat", String::as_str);
            let stem = path
                .file_stem()
                .ok_or("missing fixture filename")?
                .to_str()
                .ok_or("invalid filename")?;
            let mut segments = Vec::new();
            support::record(scenario, |index, bytes| {
                let name = format!("{stem}-segment-{index:03}.json");
                fs::write(parent.join(&name), bytes).map_err(|e| e.to_string())?;
                segments.push(name);
                Ok(())
            })?;
            fs::write(
                path,
                serde_json::to_vec(&Manifest {
                    version: 3,
                    scenario: scenario.into(),
                    steps: if scenario == "combat" { 120 } else { 256 },
                    segments,
                })?,
            )?;
            println!("Recorded {scenario}");
        }
        "verify" if args.len() == 3 => {
            let manifest: Manifest = serde_json::from_slice(&read(path)?)?;
            if manifest.version != 3
                || manifest.segments.is_empty()
                || manifest.segments.len() > 256
                || manifest.steps
                    != match manifest.scenario.as_str() {
                        "combat" => 120,
                        "wall-dash" => 256,
                        _ => 0,
                    }
            {
                return Err("invalid segmented fixture manifest".into());
            }
            let mut reports = Vec::new();
            let mut steps = 0;
            let mut previous = None;
            for name in manifest.segments {
                if Path::new(&name).file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
                    return Err("invalid segment path".into());
                }
                let report = support::verify(&read(&parent.join(name))?)?;
                if report.scenario != manifest.scenario
                    || report.start_step != steps
                    || previous
                        .as_ref()
                        .is_some_and(|digest| digest != &report.initial_digest)
                {
                    return Err(format!("segment continuity failure at replay step {steps}").into());
                }
                steps += report.steps;
                previous = Some(report.final_digest.clone());
                reports.push(serde_json::to_value(report)?);
            }
            if steps != manifest.steps {
                return Err("incomplete trajectory".into());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"scenario": manifest.scenario, "steps": steps, "restored_boundaries": steps, "final_digest": previous, "segments": reports})
                )?
            );
        }
        _ => return Err("expected record or verify".into()),
    }
    Ok(())
}
