#![cfg(feature = "ui")]

use crate::net::NetRuntime;
use bevy::prelude::*;
use game_sim::Simulation;

const MUTED: Color = Color::srgb(0.56, 0.65, 0.76);
const INK: Color = Color::srgb(0.89, 0.94, 1.0);
const GOOD: Color = Color::srgb(0.38, 0.86, 0.68);
const WARN: Color = Color::srgb(1.0, 0.71, 0.35);

#[derive(Component, Clone, Copy)]
pub(crate) enum Metric {
    Realm,
    Instance,
    Process,
    Build,
    Tick,
    Clients,
    Rtt,
    Loss,
    Sent,
    Received,
    Recorder,
    Written,
    Dropped,
    Errors,
    Mode,
    Delay,
    AddedRtt,
    Jitter,
    SimLoss,
    Peers,
    PeerDrops,
    Expired,
    Incoming(usize),
    Outgoing(usize),
}
#[derive(Component)]
pub(crate) struct ClientCell {
    row: usize,
    column: usize,
}
#[derive(Component)]
pub(crate) struct ClientRow(usize);
#[derive(Component)]
pub(crate) struct EmptyClients;

fn label(parent: &mut ChildSpawnerCommands, text: impl Into<String>, size: f32, color: Color) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(size),
            ..default()
        },
        TextColor(color),
    ));
}
fn section(
    parent: &mut ChildSpawnerCommands,
    title: &str,
    content: impl FnOnce(&mut ChildSpawnerCommands),
) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(12.0)),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.055, 0.075, 0.105)),
        ))
        .with_children(|p| {
            label(p, title, 16.0, INK);
            content(p);
        });
}
fn metric(parent: &mut ChildSpawnerCommands, name: &str, field: Metric) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            justify_content: JustifyContent::SpaceBetween,
            column_gap: Val::Px(12.0),
            ..default()
        })
        .with_children(|p| {
            label(p, name, 13.0, MUTED);
            p.spawn((
                field,
                Text::new("--"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(INK),
            ));
        });
}
fn column(left: bool) -> Node {
    Node {
        position_type: PositionType::Absolute,
        top: Val::Px(16.0),
        left: if left { Val::Percent(1.5) } else { Val::Auto },
        right: if left { Val::Auto } else { Val::Percent(1.5) },
        width: Val::Percent(if left { 57.0 } else { 38.5 }),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(12.0),
        ..default()
    }
}

pub(crate) fn setup_ui_scene(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.insert_resource(ClearColor(Color::srgb(0.025, 0.035, 0.055)));
    commands.spawn(column(true)).with_children(|p| {
        label(p, "Server overview", 26.0, INK);
        label(p, "Live transport metrics and capture health", 13.0, MUTED);
        section(p, "Instance", |p| {
            for (name, field) in [
                ("Realm", Metric::Realm),
                ("Instance", Metric::Instance),
                ("Process session", Metric::Process),
                ("Build", Metric::Build),
            ] {
                metric(p, name, field);
            }
        });
        section(p, "Network", |p| {
            for (name, field) in [
                ("Simulation tick", Metric::Tick),
                ("Connected clients", Metric::Clients),
                ("Mean RTT", Metric::Rtt),
                ("Mean packet loss", Metric::Loss),
                ("Sent / server", Metric::Sent),
                ("Received / server", Metric::Received),
            ] {
                metric(p, name, field);
            }
        });
        section(p, "Connected clients", |p| {
            p.spawn(Node {
                width: Val::Percent(100.0),
                ..default()
            })
            .with_children(|p| {
                for name in [
                    "Client",
                    "Transport",
                    "RTT",
                    "Loss",
                    "Sent KiB/s",
                    "Recv KiB/s",
                ] {
                    p.spawn(Node {
                        width: Val::Percent(100.0 / 6.0),
                        ..default()
                    })
                    .with_children(|p| label(p, name, 12.0, MUTED));
                }
            });
            p.spawn((
                EmptyClients,
                Text::new("Waiting for players to connect"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(MUTED),
            ));
            // Server capacity is eight; entities remain stable across joins/leaves.
            for row in 0..8 {
                p.spawn((
                    ClientRow(row),
                    Node {
                        display: Display::None,
                        width: Val::Percent(100.0),
                        padding: UiRect::vertical(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.07, 0.095, 0.13)),
                ))
                .with_children(|p| {
                    for column in 0..6 {
                        p.spawn((
                            ClientCell { row, column },
                            Text::new("--"),
                            TextFont {
                                font_size: FontSize::Px(13.0),
                                ..default()
                            },
                            TextColor(INK),
                            Node {
                                width: Val::Percent(100.0 / 6.0),
                                ..default()
                            },
                        ));
                    }
                });
            }
        });
        section(p, "Debug recorder", |p| {
            for (name, field) in [
                ("Status", Metric::Recorder),
                ("Written records", Metric::Written),
                ("Dropped records", Metric::Dropped),
                ("Write failures", Metric::Errors),
            ] {
                metric(p, name, field);
            }
        });
    });
}

#[derive(Component, Clone, Copy)]
pub(crate) enum ConditionerAction {
    Off,
    AddedRtt(u64),
    Delay(i64),
    Jitter(i64),
    Loss(f32),
    Outage,
}

pub(crate) fn setup_conditioner_ui(mut commands: Commands) {
    commands.spawn(column(false)).with_children(|p| {
        label(p,"Network simulation",26.0,INK);
        label(p,"Applies to all UDP and WebRTC clients",13.0,MUTED);
        section(p,"Traffic conditions",|p| {
            for (name,field) in [("Status",Metric::Mode),("Delay / direction",Metric::Delay),("Added round trip",Metric::AddedRtt),("Jitter / direction",Metric::Jitter),("Loss / direction",Metric::SimLoss)] { metric(p,name,field); }
            for row in [
                vec![("Off",ConditionerAction::Off),("+150 ms RTT",ConditionerAction::AddedRtt(150)),("+300 ms RTT",ConditionerAction::AddedRtt(300))],
                vec![("Delay -10",ConditionerAction::Delay(-10)),("Delay +10",ConditionerAction::Delay(10))],
                vec![("Jitter -5",ConditionerAction::Jitter(-5)),("Jitter +5",ConditionerAction::Jitter(5))],
                vec![("Loss -1%",ConditionerAction::Loss(-0.01)),("Loss +1%",ConditionerAction::Loss(0.01)),("Outage 1 s",ConditionerAction::Outage)],
            ] {
                p.spawn(Node {column_gap:Val::Px(6.0),row_gap:Val::Px(4.0),flex_wrap:FlexWrap::Wrap,..default()}).with_children(|p| {
                    for (text,action) in row {p.spawn((Button,action,Node {padding:UiRect::axes(Val::Px(10.0),Val::Px(8.0)),border_radius:BorderRadius::all(Val::Px(5.0)),..default()},BackgroundColor(Color::srgb(0.13,0.18,0.25))))
                        .with_children(|p|label(p,text,13.0,INK));}
                });
            }
            label(p,"Delay is added to real latency. Keep client simulation off when testing server distance.",12.0,MUTED);
        });
        section(p,"Packet queues and drops",|p| {
            p.spawn(Node {justify_content:JustifyContent::SpaceBetween,width:Val::Percent(100.0),..default()}).with_children(|p| {
                for (name,width) in [("Metric",52.0),("Incoming",24.0),("Outgoing",24.0)] {p.spawn(Node {width:Val::Percent(width),..default()}).with_children(|p|label(p,name,12.0,MUTED));}
            });
            for (index,name) in ["Queued packets","Queued bytes","Simulated loss","Outage drops","Queue overflow","Transition drops"].iter().enumerate() {
                p.spawn(Node {width:Val::Percent(100.0),..default()}).with_children(|p| {
                    p.spawn(Node {width:Val::Percent(52.0),..default()}).with_children(|p|label(p,*name,13.0,MUTED));
                    for field in [Metric::Incoming(index),Metric::Outgoing(index)] {p.spawn((field,Text::new("0"),TextFont {font_size:FontSize::Px(13.0),..default()},TextColor(INK),Node {width:Val::Percent(24.0),..default()}));}
                });
            }
            metric(p,"Tracked peers",Metric::Peers);metric(p,"Peer-limit drops",Metric::PeerDrops);metric(p,"Expired peers",Metric::Expired);
        });
    });
}

pub(crate) fn update_network_panel(
    context: Res<crate::debug_context::DebugContext>,
    recorder: Res<crate::app::DebugRecorderResource>,
    conditioner: Res<crate::conditioner::ServerConditioner>,
    net: Res<NetRuntime>,
    sim: Res<Simulation>,
    mut fields: Query<(&Metric, &mut Text, &mut TextColor)>,
    mut cells: Query<(&ClientCell, &mut Text, &mut TextColor), Without<Metric>>,
    mut rows: Query<(&ClientRow, &mut Node), Without<EmptyClients>>,
    mut empty: Single<&mut Node, With<EmptyClients>>,
) {
    let peers = net.shared.with_server_and_transport(|server, transport| {
        let mut ids = server.clients_id();
        ids.sort_unstable();
        ids.into_iter()
            .map(|id| {
                (
                    id,
                    if transport.udp().client_addr(id).is_some() {
                        "UDP"
                    } else {
                        "WebRTC"
                    },
                    server.network_info(id).ok(),
                )
            })
            .collect::<Vec<_>>()
    });
    let health = recorder.0.as_ref().map(|r| r.info());
    let state = crate::conditioner::snapshot(&conditioner.0);
    let p = &state.packets;
    let peer_slice = peers.as_deref().unwrap_or_default();
    let samples: Vec<_> = peer_slice
        .iter()
        .filter_map(|(_, _, n)| n.as_ref())
        .collect();
    let mean = |f: fn(&renet::NetworkInfo) -> f64| {
        if samples.is_empty() {
            "--".into()
        } else {
            format!(
                "{:.1}",
                samples.iter().map(|n| f(n)).sum::<f64>() / samples.len() as f64
            )
        }
    };
    for (field, mut text, mut color) in &mut fields {
        let mut warning = false;
        let value = match *field {
            Metric::Realm => context.identity.realm.clone(),
            Metric::Instance => context.identity.instance.clone(),
            Metric::Process => context.identity.process_session.clone(),
            Metric::Build => context.identity.build.clone(),
            Metric::Tick => sim.tick().to_string(),
            Metric::Clients => {
                if peers.is_some() {
                    peer_slice.len().to_string()
                } else {
                    "Unavailable".into()
                }
            }
            Metric::Rtt => format!("{} ms", mean(|n| n.rtt * 1000.0)),
            Metric::Loss => format!("{} %", mean(|n| n.packet_loss * 100.0)),
            Metric::Sent => format!(
                "{:.1} KiB/s",
                samples
                    .iter()
                    .map(|n| n.bytes_sent_per_second as f64 / 1024.0)
                    .sum::<f64>()
                    .max(0.0)
            ),
            Metric::Received => format!(
                "{:.1} KiB/s",
                samples
                    .iter()
                    .map(|n| n.bytes_received_per_second as f64 / 1024.0)
                    .sum::<f64>()
                    .max(0.0)
            ),
            Metric::Recorder => match &health {
                None => "Off".into(),
                Some(h) if h.limit_reached => {
                    warning = true;
                    "Capture limit reached".into()
                }
                Some(h) if h.write_failures > 0 => {
                    warning = true;
                    "Write failures".into()
                }
                Some(_) => "Recording".into(),
            },
            Metric::Written => health
                .as_ref()
                .map_or("--".into(), |h| h.written_records.to_string()),
            Metric::Dropped => health.as_ref().map_or("--".into(), |h| {
                warning = h.dropped_records > 0;
                h.dropped_records.to_string()
            }),
            Metric::Errors => health.as_ref().map_or("--".into(), |h| {
                warning = h.write_failures > 0;
                h.write_failures.to_string()
            }),
            Metric::Mode => {
                warning = p.enabled || p.outage_active;
                if p.outage_active {
                    "OUTAGE"
                } else if p.enabled {
                    "Enabled"
                } else {
                    "Off"
                }
                .into()
            }
            Metric::Delay => format!("{:.0} ms", p.delay_each_way_ms),
            Metric::AddedRtt => format!(
                "~{:.0} ms",
                if p.enabled {
                    p.delay_each_way_ms * 2.0
                } else {
                    0.0
                }
            ),
            Metric::Jitter => format!("{:.0} ms", p.jitter_ms),
            Metric::SimLoss => format!("{:.1} %", p.packet_loss * 100.0),
            Metric::Peers => state.peers.to_string(),
            Metric::PeerDrops => {
                warning = state.peer_limit_drops > 0;
                state.peer_limit_drops.to_string()
            }
            Metric::Expired => state.expired_peers.to_string(),
            Metric::Incoming(i) | Metric::Outgoing(i) => {
                let d = if matches!(field, Metric::Incoming(_)) {
                    &p.incoming
                } else {
                    &p.outgoing
                };
                let v = [
                    d.queued_packets as u64,
                    d.queued_bytes as u64,
                    d.simulated_loss_drops,
                    d.outage_drops,
                    d.overflow_drops,
                    d.transition_drops,
                ][i];
                warning = i == 4 && v > 0;
                v.to_string()
            }
        };
        if text.0 != value {
            text.0 = value;
        }
        color.0 = if warning {
            WARN
        } else if matches!(field, Metric::Recorder | Metric::Mode) {
            GOOD
        } else {
            INK
        };
    }
    for (row, mut node) in &mut rows {
        node.display = if row.0 < peer_slice.len() {
            Display::Flex
        } else {
            Display::None
        };
    }
    empty.display = if peer_slice.is_empty() {
        Display::Flex
    } else {
        Display::None
    };
    for (cell, mut text, mut color) in &mut cells {
        let Some((id, transport, info)) = peer_slice.get(cell.row) else {
            continue;
        };
        let value = match cell.column {
            0 => format!("#{id}"),
            1 => (*transport).into(),
            2 => info
                .as_ref()
                .map_or("--".into(), |n| format!("{:.0} ms", n.rtt * 1000.0)),
            3 => info
                .as_ref()
                .map_or("--".into(), |n| format!("{:.1}%", n.packet_loss * 100.0)),
            4 => info.as_ref().map_or("--".into(), |n| {
                format!("{:.1}", n.bytes_sent_per_second as f64 / 1024.0)
            }),
            _ => info.as_ref().map_or("--".into(), |n| {
                format!("{:.1}", n.bytes_received_per_second as f64 / 1024.0)
            }),
        };
        if text.0 != value {
            text.0 = value;
        }
        color.0 = if info.as_ref().is_some_and(|n| {
            (cell.column == 2 && n.rtt > 0.2) || (cell.column == 3 && n.packet_loss > 0.01)
        }) {
            WARN
        } else {
            INK
        };
    }
}
fn apply_conditioner_action(
    action: ConditionerAction,
    config: &mut renet_cross::conditioner::ConditionerConfig,
) -> bool {
    use std::time::Duration;
    let adjust = |value: Duration, delta: i64| {
        Duration::from_millis((value.as_millis() as i64 + delta).clamp(0, 5000) as u64)
    };
    match action {
        ConditionerAction::Off => config.enabled = false,
        ConditionerAction::AddedRtt(ms) => {
            config.enabled = true;
            config.latency = Duration::from_millis(ms / 2);
        }
        ConditionerAction::Delay(ms) => {
            config.enabled = true;
            config.latency = adjust(config.latency, ms);
        }
        ConditionerAction::Jitter(ms) => {
            config.enabled = true;
            config.jitter = adjust(config.jitter, ms);
        }
        ConditionerAction::Loss(amount) => {
            config.enabled = true;
            config.packet_loss = (config.packet_loss + amount).clamp(0.0, 1.0);
        }
        ConditionerAction::Outage => return true,
    }
    false
}

#[cfg(test)]
mod conditioner_ui_tests {
    use super::*;
    use renet_cross::conditioner::ConditionerConfig;
    use std::time::Duration;

    #[test]
    fn added_rtt_presets_split_delay_and_off_preserves_settings() {
        let mut config = ConditionerConfig::default();
        apply_conditioner_action(ConditionerAction::AddedRtt(300), &mut config);
        assert_eq!(config.latency, Duration::from_millis(150));
        apply_conditioner_action(ConditionerAction::Loss(0.01), &mut config);
        apply_conditioner_action(ConditionerAction::Off, &mut config);
        assert!(!config.enabled);
        assert_eq!(config.packet_loss, 0.01);
        apply_conditioner_action(ConditionerAction::Delay(-5000), &mut config);
        assert_eq!(config.latency, Duration::ZERO);
        assert!(config.enabled);
        assert!(apply_conditioner_action(
            ConditionerAction::Outage,
            &mut config
        ));
    }
}

pub(crate) fn conditioner_controls(
    conditioner: Res<crate::conditioner::ServerConditioner>,
    recorder: Res<crate::app::DebugRecorderResource>,
    mut buttons: Query<
        (&Interaction, &ConditionerAction, &mut BackgroundColor),
        (Changed<Interaction>, With<Button>),
    >,
) {
    use std::time::Duration;
    for (interaction, action, mut color) in &mut buttons {
        *color = BackgroundColor(match interaction {
            Interaction::Pressed => Color::srgb(0.25, 0.42, 0.58),
            Interaction::Hovered => Color::srgb(0.19, 0.28, 0.38),
            Interaction::None => Color::srgb(0.13, 0.18, 0.25),
        });
        if *interaction != Interaction::Pressed {
            continue;
        }
        let mut config = conditioner.0.config();
        if apply_conditioner_action(*action, &mut config.packets) {
            conditioner.0.outage(Duration::from_secs(1));
        } else {
            if matches!(action, ConditionerAction::Off) {
                conditioner.0.outage(Duration::ZERO);
            }
            if let Err(error) = conditioner.0.configure(config) {
                log::error!("server conditioner configuration failed: {error}");
            }
        }
        let detail = format!("{:?}", crate::conditioner::snapshot(&conditioner.0));
        log::info!("server conditioner changed: {detail}");
        if let Some(recorder) = &recorder.0 {
            recorder.event("server_conditioner_changed", None, None, detail);
        }
    }
}
