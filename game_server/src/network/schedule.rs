//! Bounded PreUpdate input staging, fixed gameplay/lag compensation and export.
use super::ingress::{INGRESS_TIME_BUDGET, MAX_INGRESS_PER_PASS};
use super::{Ingress, NetworkMetrics, NetworkWorker, Outbound, OutputBatch, PendingOutput};
use crate::app::DebugRecorderResource;
use bevy::prelude::*;
use game_shared::{ClientCommand, ReliableServerMessage};
use game_sim::world as sim;
use std::collections::VecDeque;
use std::{sync::atomic::Ordering, time::Instant};

#[derive(Resource, Default)]
pub(crate) struct PendingJoinSnapshots(pub(crate) VecDeque<u64>);

/// Capture the published range once. A producer refilling the ring cannot
/// extend this pass. Count also bounds input bytes (at most 2 KiB per command)
/// and bundle fan-out (16 moves / 64 actions), with a soft deadline between items.
pub(crate) fn receive_client_commands(world: &mut World) {
    let published = {
        let mut worker = world.resource_mut::<NetworkWorker>();
        worker.check_health();
        worker.input.get().pending()
    };
    if published == 0 {
        return;
    }
    world.resource_scope(|world, mut worker: Mut<NetworkWorker>| {
        let input = worker.input.get();
        let count = published.min(MAX_INGRESS_PER_PASS);
        let started = Instant::now();
        let mut handled = 0;
        for index in 0..count {
            if index > 0 && started.elapsed() >= INGRESS_TIME_BUDGET {
                break;
            }
            let Some(event) = input.try_recv() else {
                break;
            };
            handled += 1;
            match event {
                Ingress::Connected(id) => {
                    sim::add_player(world, id);
                    world.resource_mut::<PendingJoinSnapshots>().0.push_back(id);
                }
                Ingress::Disconnected(id) => {
                    sim::remove_player(world, id);
                    world
                        .resource_mut::<PendingJoinSnapshots>()
                        .0
                        .retain(|pending| *pending != id);
                }
                Ingress::Action(id, action) => sim::queue_action(world, id, action),
                Ingress::Moves(id, bundle) => {
                    if bundle.match_epoch != sim::sim_meta(world).match_epoch {
                        continue;
                    }
                    for action in bundle.actions {
                        sim::queue_action(world, id, action);
                    }
                    for (seq, dir) in bundle.moves {
                        sim::queue_command(world, id, ClientCommand::Move { seq, dir });
                    }
                }
                Ingress::Metrics(peers) => world.resource_mut::<NetworkMetrics>().peers = peers,
            }
        }
        let mut metrics = world.resource_mut::<NetworkMetrics>();
        metrics.ingress_messages += handled as u64;
        metrics.ingress_deferred_passes += u64::from(handled < published);
        metrics.ingress_peak_pending = metrics.ingress_peak_pending.max(published);
    });
}

pub(crate) fn fixed_server_tick(world: &mut World) {
    world
        .resource::<NetworkWorker>()
        .ingress_epoch
        .fetch_add(1, Ordering::Release);
    let pending = std::mem::take(&mut world.resource_mut::<PendingJoinSnapshots>().0);
    let joins = pending
        .into_iter()
        .map(|id| {
            Outbound::ToClient(
                id,
                ReliableServerMessage::JoinSnapshot(sim::join_snapshot(world, id)),
            )
        })
        .collect();
    let mut batch = OutputBatch {
        messages: joins,
        ..default()
    };
    if sim::has_players(world) {
        let started = std::time::Instant::now();
        let current = sim::world_delta(world);
        world
            .resource_mut::<crate::state_history::StateHistory>()
            .record(&current);
        for (client, action) in sim::ready_actions(world) {
            if !matches!(
                action.command,
                ClientCommand::CastAbility {
                    ability: game_shared::AbilityId::ArcBurst,
                    ..
                }
            ) {
                continue;
            }
            let rtt = world
                .resource::<NetworkMetrics>()
                .peers
                .iter()
                .find(|(id, _, _)| *id == client)
                .and_then(|(_, _, info)| info.as_ref())
                .map_or(0.0, |n| n.rtt);
            let decision =
                crate::state_history::validate_view_tick(sim::tick(world), action.view_tick, rtt)
                    .and_then(|tick| {
                        let origin = current
                            .heroes
                            .iter()
                            .find(|hero| hero.client_id == client)
                            .ok_or(crate::state_history::RewindFallback::CasterDiscontinuity)?
                            .pos;
                        world
                            .resource::<crate::state_history::StateHistory>()
                            .validated_targets(
                                action.match_epoch,
                                tick,
                                sim::tick(world),
                                client,
                                origin,
                            )
                    });
            let detail = match decision {
                Ok(targets) => {
                    let count = targets.len();
                    sim::set_historical_targets(world, client, action.command.seq(), targets);
                    format!(
                        "seq={} view_tick={:?} historical_candidates={count}",
                        action.command.seq(),
                        action.view_tick
                    )
                }
                Err(reason) => format!(
                    "seq={} view_tick={:?} current_world_fallback={reason:?}",
                    action.command.seq(),
                    action.view_tick
                ),
            };
            if let Some(recorder) = &world.resource::<DebugRecorderResource>().0 {
                recorder.event(
                    "lag_compensation_query",
                    Some(client),
                    Some(sim::tick(world)),
                    detail,
                );
            }
        }
        let output = sim::step(world);
        let snapshot = sim::world_delta(world);
        world
            .resource_mut::<crate::state_history::StateHistory>()
            .record(&snapshot);
        world.resource_mut::<NetworkMetrics>().tick = snapshot.tick;
        batch.messages.extend(
            output
                .reliable_events
                .into_iter()
                .map(|event| Outbound::Broadcast(ReliableServerMessage::Event(event))),
        );
        batch.world = Some(snapshot);
        batch.step_duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    }
    world.resource_mut::<PendingOutput>().push(batch);
}
