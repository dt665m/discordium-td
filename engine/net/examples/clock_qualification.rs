//! Manual clock qualification; simulated durations never qualify real-hour gates.
use engine_net::{
    clock::TickClock,
    synchronization::{ClockConfig, ClockEstimator, LeadController, TimeExchange},
    types::{ServerTick, TickRate},
};
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
const HZ: u64 = 60;
const NS: u128 = 1_000_000_000;
const POLL_NS: u64 = 10_000_000;
const PROBE_NS: u64 = 250_000_000;
const TRANSIT_NS: u64 = 22_000_000;
const WARMUP_NS: u128 = 10 * NS;
const MAX_GAP: Duration = Duration::from_millis(100);
// Declared fixture configuration only; callbacks do not integrate gameplay dt.
const DECLARED_FIXED_DT: f64 = 1.0 / 60.0;
type Result<T> = std::result::Result<T, String>;

fn duration(ns: u128) -> Result<Duration> {
    Ok(Duration::new(
        u64::try_from(ns / NS).map_err(|_| "time overflow")?,
        (ns % NS) as u32,
    ))
}
fn local_time(server: Duration, ppm: i32) -> Result<Duration> {
    if ![-1000, 1000].contains(&ppm) {
        return Err("unsupported drift profile".into());
    }
    duration(5 * NS + server.as_nanos() * (1_000_000 + ppm) as u128 / 1_000_000)
}
fn exchange(send: Duration, ppm: i32) -> Result<TimeExchange> {
    Ok(TimeExchange {
        client_send: local_time(send, ppm)?,
        server_receive: send + Duration::from_millis(10),
        server_send: send + Duration::from_millis(12),
        client_receive: local_time(send + Duration::from_nanos(TRANSIT_NS), ppm)?,
    })
}
fn rational_deadline(tick: u64, deadline: Duration) -> Result<()> {
    let scaled = deadline.as_nanos() * HZ as u128;
    let exact = tick as u128 * NS;
    if scaled < exact || scaled - exact >= HZ as u128 {
        return Err(format!("tick {tick}: nonrational deadline"));
    }
    Ok(())
}
#[derive(Default)]
struct Stats {
    n: u64,
    mean_x: f64,
    mean_y: f64,
    sxx: f64,
    sxy: f64,
    squared: f64,
    maximum: u128,
    first: i128,
    last: i128,
    histogram: [u64; 8],
}
impl Stats {
    fn add(&mut self, now: Duration, phase: i128) {
        self.n += 1;
        if self.n == 1 {
            self.first = phase;
        }
        self.last = phase;
        self.maximum = self.maximum.max(phase.unsigned_abs());
        self.squared += (phase as f64).powi(2);
        let (x, y) = (now.as_secs_f64(), phase as f64);
        let (dx, dy) = (x - self.mean_x, y - self.mean_y);
        self.mean_x += dx / self.n as f64;
        self.mean_y += dy / self.n as f64;
        self.sxx += dx * (x - self.mean_x);
        self.sxy += dx * (y - self.mean_y);
        let bin = [
            100_000, 250_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 16_666_667,
        ]
        .iter()
        .position(|bound| phase.unsigned_abs() <= *bound)
        .unwrap_or(7);
        self.histogram[bin] += 1;
    }
    fn json(&self) -> String {
        format!(
            "{{\"samples\":{},\"max_abs_ns\":{},\"mean_ns\":{},\"rms_ns\":{},\"linear_ns_per_hour\":{},\"first_ns\":{},\"last_ns\":{},\"histogram\":{:?}}}",
            self.n,
            self.maximum,
            self.mean_y,
            if self.n == 0 {
                0.0
            } else {
                (self.squared / self.n as f64).sqrt()
            },
            if self.sxx == 0.0 {
                0.0
            } else {
                self.sxy / self.sxx * 3600.0
            },
            self.first,
            self.last,
            self.histogram
        )
    }
}
struct Drift {
    ppm: i32,
    clock: ClockEstimator,
    lead: LeadController,
    next_probe: Duration,
    pending: Option<(Duration, TimeExchange)>,
    last_target: Option<u64>,
    last_estimate: Option<Duration>,
    probes: u64,
    predicted: TickClock,
    predicted_ticks: u64,
    phase: Stats,
    warm_phase: Stats,
    max_target_phase: u64,
    max_samples: usize,
}
impl Drift {
    fn new(ppm: i32) -> Result<Self> {
        Ok(Self {
            ppm,
            clock: ClockEstimator::new(
                TickRate::new(60).unwrap(),
                Duration::ZERO,
                ClockConfig::default(),
            )
            .map_err(|e| e.to_string())?,
            lead: LeadController::new(2).map_err(|e| e.to_string())?,
            next_probe: Duration::ZERO,
            pending: None,
            last_target: None,
            last_estimate: None,
            probes: 0,
            predicted: TickClock::new(60, 3),
            predicted_ticks: 0,
            phase: Stats::default(),
            warm_phase: Stats::default(),
            max_target_phase: 0,
            max_samples: 0,
        })
    }
    fn poll(&mut self, now: Duration) -> Result<()> {
        if self.pending.as_ref().is_some_and(|(due, _)| *due <= now) {
            let (_, sample) = self.pending.take().unwrap();
            self.clock.observe(sample).map_err(|e| e.to_string())?;
            self.probes += 1;
        }
        if now >= self.next_probe {
            if self.pending.is_some() {
                return Err("probe queue overflow".into());
            }
            self.pending = Some((
                self.next_probe + Duration::from_nanos(TRANSIT_NS),
                exchange(self.next_probe, self.ppm)?,
            ));
            self.next_probe += Duration::from_nanos(PROBE_NS);
        }
        if self.probes == 0 {
            return Ok(());
        }
        let estimated = self
            .clock
            .estimate_server_elapsed(local_time(now, self.ppm)?)
            .map_err(|e| e.to_string())?;
        if self.last_estimate.is_some_and(|last| estimated < last) {
            return Err("nonmonotonic estimate".into());
        }
        self.last_estimate = Some(estimated);
        self.max_samples = self.max_samples.max(self.clock.sample_count());
        if self.max_samples > 32 {
            return Err("sample budget exceeded".into());
        }
        let phase = estimated.as_nanos() as i128 - now.as_nanos() as i128;
        self.phase.add(now, phase);
        if now.as_nanos() >= WARMUP_NS {
            self.warm_phase.add(now, phase);
            if phase.unsigned_abs() * HZ as u128 > NS {
                return Err(format!("{}ppm: phase exceeds one tick", self.ppm));
            }
        }
        let mut callback_error = None;
        let predicted_now = estimated
            + TickRate::new(60)
                .unwrap()
                .deadline(ServerTick(u64::from(self.lead.lead())))
                .ok_or("lead deadline overflow")?;
        let decision = self.predicted.advance_ticked(predicted_now, |context| {
            if context.tick.0 != self.predicted_ticks + 1 {
                callback_error = Some("noncontiguous predicted ticks".to_string());
            }
            if let Err(error) = rational_deadline(context.tick.0, context.deadline) {
                callback_error = Some(error);
            }
            self.predicted_ticks = context.tick.0;
        });
        if let Some(error) = callback_error {
            return Err(error);
        }
        if decision.health.debt_ticks != 0 {
            return Err("predicted dispatch debt".into());
        }
        let estimated_tick = TickRate::new(60)
            .unwrap()
            .elapsed_ticks(estimated)
            .ok_or("tick overflow")?;
        if let Some(target) = self
            .lead
            .assign_target(estimated_tick)
            .map_err(|e| e.to_string())?
        {
            if self.last_target.is_some_and(|last| target.0 <= last) {
                return Err("issued target rewound".into());
            }
            self.last_target = Some(target.0);
            let true_tick = (now.as_nanos() * HZ as u128 / NS) as u64;
            let target_phase = target.0.abs_diff(true_tick + u64::from(self.lead.lead()));
            self.max_target_phase = self.max_target_phase.max(target_phase);
            if now.as_nanos() >= WARMUP_NS && target_phase > 2 {
                return Err("target phase exceeds two ticks".into());
            }
        }
        Ok(())
    }
    fn json(&self) -> String {
        format!(
            "{{\"ppm\":{},\"probes\":{},\"max_samples\":{},\"predicted_fixed_steps\":{},\"max_target_phase_ticks\":{},\"phase\":{},\"after_warmup_phase\":{}}}",
            self.ppm,
            self.probes,
            self.max_samples,
            self.predicted_ticks,
            self.max_target_phase,
            self.phase.json(),
            self.warm_phase.json()
        )
    }
}
struct Run {
    server: TickClock,
    drifts: [Drift; 2],
    ticks: u64,
    last: Duration,
    max_gap_ns: u128,
    max_lateness_ns: u128,
    max_debt: u64,
}
impl Run {
    fn new() -> Result<Self> {
        Ok(Self {
            server: TickClock::new(60, 3),
            drifts: [Drift::new(-1000)?, Drift::new(1000)?],
            ticks: 0,
            last: Duration::ZERO,
            max_gap_ns: 0,
            max_lateness_ns: 0,
            max_debt: 0,
        })
    }
    fn poll(&mut self, now: Duration) -> Result<()> {
        let gap = now
            .checked_sub(self.last)
            .ok_or("reference clock moved backward")?;
        self.max_gap_ns = self.max_gap_ns.max(gap.as_nanos());
        if gap > MAX_GAP {
            return Err("reference polling gap exceeds 100ms; scheduling continuity lost".into());
        }
        self.last = now;
        let mut error = None;
        let decision = self.server.advance_ticked(now, |context| {
            if context.tick.0 != self.ticks + 1 {
                error = Some("noncontiguous server ticks".into());
            }
            if let Err(e) = rational_deadline(context.tick.0, context.deadline) {
                error = Some(e);
            }
            if context.deadline > now {
                error = Some("early server tick".into());
            }
            self.max_lateness_ns = self
                .max_lateness_ns
                .max(now.saturating_sub(context.deadline).as_nanos());
            self.ticks = context.tick.0;
        });
        if let Some(error) = error {
            return Err(error);
        }
        self.max_debt = self.max_debt.max(decision.health.debt_ticks);
        if decision.health.debt_ticks != 0 {
            return Err("server tick debt exceeds bounded dispatch; no debt forgiven".into());
        }
        for drift in &mut self.drifts {
            drift.poll(now)?;
        }
        Ok(())
    }
    fn json(
        &self,
        phase: &str,
        mode: &str,
        real: Duration,
        seconds: u64,
        error: Option<&str>,
    ) -> String {
        let complete = phase == "final"
            && error.is_none()
            && self.last == Duration::from_secs(seconds)
            && self.ticks == seconds * HZ;
        let real_complete = complete && mode == "realtime" && real >= Duration::from_secs(seconds);
        format!(
            "{{\"phase\":{phase:?},\"mode\":{mode:?},\"requested_seconds\":{seconds},\"reference_seconds\":{},\"actual_elapsed_seconds\":{},\"ticks\":{},\"declared_fixed_dt_bits\":{},\"max_poll_gap_ns\":{},\"max_deadline_lateness_ns\":{},\"max_debt_ticks\":{},\"completed\":{complete},\"T004_actual_hour_passed\":{},\"T007_actual_day_passed\":{},\"simulated_regression_only\":{},\"error\":{},\"drifts\":[{},{}]}}",
            self.last.as_secs_f64(),
            real.as_secs_f64(),
            self.ticks,
            DECLARED_FIXED_DT.to_bits(),
            self.max_gap_ns,
            self.max_lateness_ns,
            self.max_debt,
            real_complete && seconds >= 3600,
            real_complete && seconds >= 86400,
            mode == "simulated",
            error.map_or("null".into(), |e| format!("{e:?}")),
            self.drifts[0].json(),
            self.drifts[1].json()
        )
    }
}
struct Args {
    seconds: u64,
    report: u64,
    mode: String,
    output: PathBuf,
}
fn args(values: &[String]) -> Result<Args> {
    let mut parsed = Args {
        seconds: 0,
        report: 60,
        mode: String::new(),
        output: PathBuf::new(),
    };
    for pair in values.chunks(2) {
        if pair.len() != 2 {
            return Err("each option needs a value".into());
        }
        match pair[0].as_str() {
            "--seconds" => parsed.seconds = pair[1].parse().map_err(|_| "invalid duration")?,
            "--report-seconds" => {
                parsed.report = pair[1].parse().map_err(|_| "invalid report period")?
            }
            "--mode" => parsed.mode = pair[1].clone(),
            "--output" => parsed.output = pair[1].clone().into(),
            _ => return Err("unknown option".into()),
        }
    }
    if !(1..=86400).contains(&parsed.seconds)
        || !(1..=3600).contains(&parsed.report)
        || parsed.seconds.div_ceil(parsed.report) > 1440
        || !matches!(parsed.mode.as_str(), "simulated" | "realtime")
        || parsed.output.as_os_str().is_empty()
    {
        return Err("invalid mode, duration, output or report count (maximum1440)".into());
    }
    Ok(parsed)
}
fn main() -> Result<()> {
    let values: Vec<_> = std::env::args().skip(1).collect();
    if values == ["--help"] {
        println!(
            "clock_qualification --mode simulated|realtime --seconds 1..86400 --report-seconds 1..3600 --output METRICS_JSONL\nManual qualification only. Simulated time never passes actual-duration gates."
        );
        return Ok(());
    }
    let args = args(&values)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args.output)
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let end = Duration::from_secs(args.seconds);
    let mut run = Run::new()?;
    let mut next_report = Duration::from_secs(args.report);
    let mut poll_index = 0_u64;
    let mut failure = None;
    loop {
        let scheduled = Duration::from_nanos(poll_index * POLL_NS).min(end);
        let observed = if args.mode == "realtime" {
            if let Some(wait) = scheduled.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
            let actual = start.elapsed();
            // Check the final real polling gap BEFORE clamping to the exact cutoff.
            if actual.saturating_sub(run.last) > MAX_GAP {
                failure = Some("real polling gap exceeds 100ms".into());
                break;
            }
            actual.min(end)
        } else {
            scheduled
        };
        if let Err(error) = run.poll(observed) {
            failure = Some(error);
            break;
        }
        if observed >= next_report && observed < end {
            writeln!(
                file,
                "{}",
                run.json("interval", &args.mode, start.elapsed(), args.seconds, None)
            )
            .map_err(|e| e.to_string())?;
            file.flush().map_err(|e| e.to_string())?;
            next_report += Duration::from_secs(args.report);
        }
        if observed == end {
            break;
        }
        poll_index += 1;
    }
    writeln!(
        file,
        "{}",
        run.json(
            "final",
            &args.mode,
            start.elapsed(),
            args.seconds,
            failure.as_deref()
        )
    )
    .map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    failure.map_or(Ok(()), Err)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_rounding_rejects_accumulated_truncation() {
        for tick in [1, 2, 60, 5_184_000] {
            rational_deadline(
                tick,
                TickRate::new(60)
                    .unwrap()
                    .deadline(ServerTick(tick))
                    .unwrap(),
            )
            .unwrap();
        }
        assert!(rational_deadline(60, Duration::from_millis(960)).is_err());
        assert!(rational_deadline(1, Duration::from_nanos(16_666_666)).is_err());
    }
    #[test]
    fn few_injected_samples_use_both_signs_and_bounded_state() {
        for ppm in [-1000, 1000] {
            let mut drift = Drift::new(ppm).unwrap();
            for ms in [0, 30, 60, 90, 120, 150, 180, 210, 250, 280] {
                drift.poll(Duration::from_millis(ms)).unwrap();
            }
            assert_eq!(drift.probes, 2);
            assert_eq!(drift.max_samples, 2);
            assert!(drift.phase.maximum < 1_000_000);
            assert!(local_time(Duration::from_secs(1), ppm).unwrap() != Duration::from_secs(6));
        }
    }
    #[test]
    fn finite_unit_regression_crosses_warmup_for_both_signs() {
        // No CLI, files or wall-time run: 1,004 injected polls exercise the
        // previously uncovered warmup branch in the production estimators.
        let mut run = Run::new().unwrap();
        for poll in 0..=1003 {
            run.poll(Duration::from_millis(poll * 10)).unwrap();
        }
        for drift in &run.drifts {
            assert_eq!(drift.max_samples, 32);
            assert_eq!(drift.warm_phase.n, 4);
            assert!(drift.warm_phase.maximum * HZ as u128 <= NS);
            assert!(drift.max_target_phase <= 2);
            assert!(drift.predicted_ticks > run.ticks);
        }
    }

    #[test]
    fn errors_do_not_masquerade_as_elapsed_qualification() {
        let mut run = Run::new().unwrap();
        run.poll(Duration::from_millis(10)).unwrap();
        assert!(run.poll(Duration::ZERO).is_err());
        assert!(run.poll(Duration::from_secs(1)).is_err());
        assert!(
            run.json(
                "final",
                "simulated",
                Duration::from_secs(90000),
                86400,
                None
            )
            .contains("\"T007_actual_day_passed\":false")
        );
        assert!(args(&["--seconds".into(), "86401".into()]).is_err());
    }
}
