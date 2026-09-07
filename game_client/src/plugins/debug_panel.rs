//! Presentation-only diagnostic rows; telemetry remains in the game resources.
use bevy::prelude::*;
use std::collections::VecDeque;

#[derive(Component)]
pub(super) struct DebugPanel;
#[derive(Component, Clone, Copy)]
pub(super) enum DebugMetric {
    Connection,
    Conditioning,
    Rtt,
    Loss,
    Sent,
    Received,
    SnapshotAge,
    BaselineMisses,
    DecodeErrors,
    TransportErrors,
    BrowserDrops,
    Messages,
    InputAck,
    AckJitter,
    Interpolation,
    InputBuffer,
    Correction,
}

pub(super) fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            DebugPanel,
            Interaction::None,
            Visibility::Hidden,
            GlobalZIndex(900),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(28.0),
                left: Val::Px(12.0),
                width: Val::Px(520.0),
                max_width: Val::Percent(95.0),
                max_height: Val::Vh(90.0),
                overflow: Overflow::scroll_y(),
                padding: UiRect::all(Val::Px(8.0)),
                row_gap: Val::Px(4.0),
                flex_direction: FlexDirection::Column,
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.035, 0.055, 0.62)),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("Network / F3     F4 Mark  /  F6 Lag"),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.89, 0.94, 1.0)),
            ));
            p.spawn(Node {
                width: Val::Percent(100.0),
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(6.0),
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|p| {
                for (series, label, metric) in [
                    (0, "RTT", DebugMetric::Rtt),
                    (1, "ACK jitter", DebugMetric::AckJitter),
                    (2, "Packet loss", DebugMetric::Loss),
                    (3, "Receive", DebugMetric::Received),
                    (4, "Send", DebugMetric::Sent),
                ] {
                    p.spawn((
                        Node {
                            flex_basis: Val::Px(90.0),
                            flex_grow: 1.0,
                            min_width: Val::Px(90.0),
                            padding: UiRect::all(Val::Px(3.0)),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(3.0),
                            border_radius: BorderRadius::all(Val::Px(5.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.035, 0.06, 0.09, 0.25)),
                    ))
                    .with_children(|p| {
                        p.spawn((
                            Text::new(label),
                            TextFont {
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(graph_color(series)),
                        ));
                        p.spawn((
                            metric,
                            Text::new("--"),
                            TextFont {
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                        p.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(20.0),
                                border: UiRect::bottom(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor::all(Color::srgba(0.7, 0.8, 0.9, 0.35)),
                        ))
                        .with_children(|p| {
                            for slot in 0..HISTORY_SAMPLES {
                                p.spawn((
                                    GraphBar { series, slot },
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: Val::Percent(
                                            slot as f32 * 100.0 / HISTORY_SAMPLES as f32,
                                        ),
                                        bottom: Val::Px(0.0),
                                        width: Val::Percent(100.0 / HISTORY_SAMPLES as f32),
                                        height: Val::Px(0.0),
                                        ..default()
                                    },
                                    BackgroundColor(graph_color(series)),
                                ));
                            }
                        });
                        p.spawn((
                            GraphScale(series),
                            Text::new(""),
                            TextFont {
                                font_size: 9.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.72, 0.8, 0.9)),
                        ));
                    });
                }
            });
            p.spawn(Node {
                width: Val::Percent(100.0),
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Percent(2.0),
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|p| {
                for (label, metric) in [
                    ("Connection", DebugMetric::Connection),
                    ("Lag", DebugMetric::Conditioning),
                    ("Snapshot age", DebugMetric::SnapshotAge),
                    ("Input ACK", DebugMetric::InputAck),
                    ("Interpolation", DebugMetric::Interpolation),
                    ("Pending inputs", DebugMetric::InputBuffer),
                    ("Correction", DebugMetric::Correction),
                    ("Snapshots", DebugMetric::Messages),
                    ("Baseline misses", DebugMetric::BaselineMisses),
                    ("Decode errors", DebugMetric::DecodeErrors),
                    ("Transport errors", DebugMetric::TransportErrors),
                    ("Drops S/R/size", DebugMetric::BrowserDrops),
                ] {
                    p.spawn(Node {
                        width: Val::Percent(49.0),
                        justify_content: JustifyContent::SpaceBetween,
                        column_gap: Val::Px(4.0),
                        row_gap: Val::Px(1.0),
                        ..default()
                    })
                    .with_children(|p| {
                        p.spawn((
                            Text::new(label),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.65, 0.73, 0.83)),
                        ));
                        p.spawn((
                            metric,
                            Text::new("--"),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.89, 0.94, 1.0)),
                        ));
                    });
                }
            });
        });
}

#[derive(Component, Clone, Copy)]
pub(super) enum HudMetric {
    Simulation,
    Status,
    Wave,
    TeamLife,
}

#[derive(Component)]
pub(super) struct HudRoot;

pub(super) fn spawn_hud(commands: &mut Commands) {
    commands
        .spawn((
            HudRoot,
            Visibility::Inherited,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(28.0),
                left: Val::Px(12.0),
                width: Val::Px(248.0),
                max_width: Val::Percent(80.0),
                padding: UiRect::all(Val::Px(10.0)),
                row_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Column,
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.025, 0.035, 0.055, 0.9)),
        ))
        .with_children(|p| {
            p.spawn(Node {
                column_gap: Val::Px(20.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|p| {
                for (label, field) in [
                    ("WAVE", HudMetric::Wave),
                    ("TEAM LIFE", HudMetric::TeamLife),
                ] {
                    p.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0),
                        ..default()
                    })
                    .with_children(|p| {
                        p.spawn((
                            Text::new(label),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.56, 0.65, 0.76)),
                        ));
                        p.spawn((
                            field,
                            Text::new("--"),
                            TextFont {
                                font_size: 22.0,
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            Node::default(),
                        ));
                    });
                }
                p.spawn((
                    Text::new(
                        "F3
Debug",
                    ),
                    TextFont {
                        font_size: 10.0,
                        ..default()
                    },
                    TextColor(Color::srgb(0.56, 0.65, 0.76)),
                ));
            });
            for field in [HudMetric::Status, HudMetric::Simulation] {
                p.spawn((
                    field,
                    Text::new(""),
                    TextFont {
                        font_size: 11.0,
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.71, 0.35)),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }
        });
}

pub(super) fn scroll(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut panels: Query<
        (
            &Visibility,
            &Interaction,
            &ComputedNode,
            &mut ScrollPosition,
        ),
        With<DebugPanel>,
    >,
) {
    use bevy::input::mouse::MouseScrollUnit;
    let delta: f32 = wheel
        .read()
        .map(|event| {
            event.y
                * if event.unit == MouseScrollUnit::Line {
                    24.0
                } else {
                    1.0
                }
        })
        .sum();
    for (visible, interaction, node, mut scroll) in &mut panels {
        if *visible == Visibility::Hidden || *interaction == Interaction::None {
            continue;
        }
        let max = ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
        scroll.0.y = (scroll.0.y - delta).clamp(0.0, max);
    }
}

// Fixed real-time buckets keep the time axis honest through pauses and disconnects.
const HISTORY_SAMPLES: usize = 60;
const SAMPLE_SECONDS: f64 = 0.25;
#[derive(Resource, Default)]
pub(super) struct DiagnosticHistory {
    samples: VecDeque<[Option<f32>; 5]>,
    last_bucket: Option<u64>,
    revision: u64,
}

impl DiagnosticHistory {
    pub(super) fn record(&mut self, now: f64, values: [Option<f32>; 5]) {
        let bucket = (now / SAMPLE_SECONDS) as u64;
        if self.last_bucket == Some(bucket) {
            return;
        }
        if let Some(last) = self.last_bucket {
            for _ in 0..bucket
                .saturating_sub(last)
                .saturating_sub(1)
                .min(HISTORY_SAMPLES as u64)
            {
                self.samples.push_back([None; 5]);
            }
        }
        self.samples
            .push_back(values.map(|v| v.filter(|v| v.is_finite() && *v >= 0.0)));
        while self.samples.len() > HISTORY_SAMPLES {
            self.samples.pop_front();
        }
        self.last_bucket = Some(bucket);
        self.revision = self.revision.wrapping_add(1);
    }

    fn scale(&self, series: usize) -> f32 {
        let minimum = [100.0, 20.0, 5.0, 32.0, 8.0][series];
        let peak = self
            .samples
            .iter()
            .filter_map(|s| s[series])
            .fold(0.0_f32, f32::max);
        (peak / minimum).ceil().max(1.0) * minimum
    }

    fn value(&self, series: usize, slot: usize) -> Option<f32> {
        // Right-align short histories; do not invent pre-connection samples.
        slot.checked_sub(HISTORY_SAMPLES - self.samples.len())
            .and_then(|index| self.samples.get(index))
            .and_then(|sample| sample[series])
    }
}

#[derive(Component)]
pub(super) struct GraphBar {
    series: usize,
    slot: usize,
}
#[derive(Component)]
pub(super) struct GraphScale(usize);

fn graph_color(series: usize) -> Color {
    match series {
        0 => Color::srgb(0.35, 0.8, 1.0),
        1 => Color::srgb(0.9, 0.65, 1.0),
        2 => Color::srgb(1.0, 0.55, 0.35),
        3 => Color::srgb(0.45, 0.9, 0.65),
        _ => Color::srgb(0.95, 0.85, 0.4),
    }
}

pub(super) fn update_graphs(
    history: Res<DiagnosticHistory>,
    panel: Single<&Visibility, With<DebugPanel>>,
    mut last: Local<(u64, bool)>,
    mut bars: Query<(&GraphBar, &mut Node)>,
    mut scales: Query<(&GraphScale, &mut Text)>,
) {
    let visible = **panel != Visibility::Hidden;
    if !visible {
        last.1 = false;
        return;
    }
    if last.1 && last.0 == history.revision {
        return;
    }
    *last = (history.revision, true);
    let ranges: [f32; 5] = std::array::from_fn(|i| history.scale(i));
    for (bar, mut node) in &mut bars {
        node.height = Val::Px(
            history
                .value(bar.series, bar.slot)
                .map_or(0.0, |v| (v / ranges[bar.series] * 19.0).clamp(1.0, 19.0)),
        );
    }
    for (GraphScale(series), mut text) in &mut scales {
        let unit = match series {
            0 | 1 => "ms",
            2 => "%",
            _ => "KiB/s",
        };
        text.0 = format!("Max: {:.0} {unit}", ranges[*series]);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn buckets_are_bounded_and_stalls_leave_gaps() {
        let mut history = DiagnosticHistory::default();
        history.record(0.0, [Some(1.0); 5]);
        history.record(0.1, [Some(99.0); 5]);
        assert_eq!(history.samples.len(), 1);
        history.record(1.0, [Some(2.0); 5]);
        assert_eq!(history.samples.len(), 5);
        assert_eq!(history.value(0, 58), None);
        assert_eq!(history.value(0, 59), Some(2.0));
        history.record(1000.0, [Some(f32::NAN); 5]);
        assert_eq!(history.samples.len(), HISTORY_SAMPLES);
        assert!(
            history
                .samples
                .iter()
                .all(|s| s.iter().all(Option::is_none))
        );
    }

    #[test]
    fn scale_keeps_recent_spikes_and_zero_is_a_valid_sample() {
        let mut history = DiagnosticHistory::default();
        history.record(0.0, [Some(300.0), None, Some(0.0), None, None]);
        history.record(0.25, [Some(8.0); 5]);
        assert_eq!(history.scale(0), 300.0);
        assert_eq!(history.value(2, 58), Some(0.0));
        assert_eq!(history.value(0, 0), None);
    }
}
