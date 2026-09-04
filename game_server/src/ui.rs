#![cfg(feature = "ui")]

use bevy::prelude::*;
use game_sim::Simulation;

use crate::net::NetRuntime;

#[derive(Component)]
pub(crate) struct NetworkPanelText;

pub(crate) fn setup_ui_scene(mut commands: Commands) {
    commands.spawn(Camera2d);

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                left: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                ..Default::default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Discordium TD Server\nstarting..."),
                TextFont {
                    font_size: 14.0,
                    ..Default::default()
                },
                TextColor(Color::srgba(0.8, 1.0, 0.8, 1.0)),
                NetworkPanelText,
            ));
        });
}

pub(crate) fn update_network_panel(
    net: Res<NetRuntime>,
    sim: Res<Simulation>,
    mut text: Single<&mut Text, With<NetworkPanelText>>,
) {
    let tick = sim.tick();

    let shared = &net.shared;

    let info = shared.with_server_and_transport(|server, transport| {
        let client_ids = server.clients_id();
        let num_clients = client_ids.len();

        let mut total_rtt = 0.0_f64;
        let mut total_loss = 0.0_f64;
        let mut total_up = 0.0_f64;
        let mut total_down = 0.0_f64;
        let mut per_client = Vec::new();

        for &client_id in &client_ids {
            if let Ok(net_info) = server.network_info(client_id) {
                let rtt_ms = net_info.rtt * 1000.0;
                let loss_pct = net_info.packet_loss * 100.0;
                let up_kbs = net_info.bytes_sent_per_second as f64 / 1024.0;
                let down_kbs = net_info.bytes_received_per_second as f64 / 1024.0;

                total_rtt += rtt_ms;
                total_loss += loss_pct;
                total_up += up_kbs;
                total_down += down_kbs;

                let transport_type = if transport.udp().client_addr(client_id).is_some() {
                    "udp"
                } else if transport.webrtc().client_addr(client_id).is_some() {
                    "webrtc"
                } else {
                    "?"
                };

                per_client.push(format!(
                    " #{client_id}  {transport_type:<6} {rtt_ms:>4.0}ms  {loss_pct:.1}%  \u{2191}{up_kbs:.1}  \u{2193}{down_kbs:.1} KB/s"
                ));
            }
        }

        let avg_rtt = if num_clients > 0 {
            total_rtt / num_clients as f64
        } else {
            0.0
        };
        let avg_loss = if num_clients > 0 {
            total_loss / num_clients as f64
        } else {
            0.0
        };

        let mut out = format!(
            "Discordium TD Server\ntick: {}   clients: {}\navg rtt: {:.1}ms   loss: {:.2}%\nup: {:.1} KB/s   down: {:.1} KB/s",
            tick, num_clients, avg_rtt, avg_loss, total_up, total_down,
        );

        for line in &per_client {
            out.push('\n');
            out.push_str(line);
        }

        out
    });

    if let Some(info) = info {
        text.0 = info;
    }
}
