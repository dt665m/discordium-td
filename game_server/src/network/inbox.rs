//! Bounded ownership handoff. No transport or ECS world is shared here.
use super::*;

/// Preserve reliable deliveries and disconnects in simulation order, including
/// when several fixed ticks are exported together after a slow app frame.
pub(crate) enum Outbound {
    ToClient(u64, ReliableServerMessage),
    Broadcast(ReliableServerMessage),
}
#[derive(Default)]
pub(crate) struct OutputBatch {
    pub messages: Vec<Outbound>,
    pub world: Option<WorldDelta>,
    pub step_duration_ms: f64,
}

/// A frame can contain several catch-up ticks. Keep their ordered messages but
/// export only the most recent state, once, after FixedUpdate has finished.
#[derive(Resource, Default)]
pub(crate) struct PendingOutput(Option<OutputBatch>);
impl PendingOutput {
    pub(crate) fn push(&mut self, batch: OutputBatch) {
        if batch.world.is_none() && batch.messages.is_empty() {
            return;
        }
        if let Some(pending) = &mut self.0 {
            pending.messages.extend(batch.messages);
            if batch.world.is_some() {
                pending.world = batch.world;
                pending.step_duration_ms = batch.step_duration_ms;
            }
        } else {
            self.0 = Some(batch);
        }
    }
}
pub(crate) fn publish_output(mut pending: ResMut<PendingOutput>, worker: Res<NetworkWorker>) {
    if let Some(batch) = pending.0.take() {
        worker.send(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catch_up_keeps_latest_state_and_preserves_ordered_deliveries() {
        let sim = game_sim::Simulation::new();
        let mut pending = PendingOutput::default();
        for tick in 1..=3 {
            let mut world = sim.world_delta();
            world.tick = tick;
            pending.push(OutputBatch {
                messages: vec![Outbound::ToClient(
                    tick as u64,
                    ReliableServerMessage::JoinSnapshot(sim.join_snapshot(tick as u64)),
                )],
                world: Some(world),
                step_duration_ms: tick as f64,
            });
        }
        pending.push(OutputBatch {
            messages: vec![Outbound::ToClient(
                4,
                ReliableServerMessage::JoinSnapshot(sim.join_snapshot(4)),
            )],
            ..default()
        });
        pending.push(OutputBatch::default());
        let batch = pending.0.take().unwrap();
        assert_eq!(batch.world.unwrap().tick, 3);
        assert_eq!(batch.step_duration_ms, 3.0);
        assert!(matches!(
            &batch.messages[..],
            [
                Outbound::ToClient(1, _),
                Outbound::ToClient(2, _),
                Outbound::ToClient(3, _),
                Outbound::ToClient(4, _)
            ]
        ));
        assert!(pending.0.is_none());
    }
}
