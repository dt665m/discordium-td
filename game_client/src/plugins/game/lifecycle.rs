//! Final-frame transport cleanup follows Bevy's window exit systems.
use super::*;

pub(super) struct ClientLifecyclePlugin;

impl Plugin for ClientLifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Last,
            graceful_disconnect_on_app_exit.after(bevy::window::ExitSystems),
        );
    }
}

pub(super) fn graceful_disconnect_runtime(runtime: &mut NetworkRuntime, reason: &str) {
    if runtime.renet.disconnect_reason().is_some() {
        return;
    }

    runtime.renet.disconnect();
    if let Err(err) = runtime.transport_update(Duration::ZERO) {
        log::warn!("failed to send disconnect packet during {reason}: {err}");
    } else {
        log::info!("sent graceful disconnect packet during {reason}");
    }
}

fn graceful_disconnect_on_app_exit(
    mut app_exit_reader: MessageReader<AppExit>,
    runtime: Option<NonSendMut<NetworkRuntime>>,
) {
    if app_exit_reader.read().next().is_none() {
        return;
    }

    let Some(mut runtime) = runtime else {
        return;
    };
    graceful_disconnect_runtime(&mut runtime, "app exit");
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn window_close_disconnects_transport_in_the_final_frame() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let transport = ClientTransport::new(
            Duration::ZERO,
            renet_cross::ClientAuthentication::Unsecure {
                protocol_id: PROTOCOL_ID,
                client_id: 1,
                server_addr: receiver.local_addr().unwrap(),
                user_data: None,
            },
            socket,
        )
        .unwrap();
        let mut renet = RenetClient::new(renet::ConnectionConfig::default());
        renet.set_connected();

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            WindowPlugin::default(),
            ClientLifecyclePlugin,
        ))
        .insert_non_send(NetworkRuntime::new(1, renet, transport));
        app.update();
        assert!(
            app.world()
                .non_send::<NetworkRuntime>()
                .renet
                .is_connected()
        );

        let world = app.world_mut();
        let window = world
            .query_filtered::<Entity, With<Window>>()
            .single(world)
            .unwrap();
        world.write_message(bevy::window::WindowCloseRequested { window });
        app.update();
        // WindowPlugin marks ClosingWindow first, then despawns it on the next frame.
        assert!(app.should_exit().is_none());
        app.update();

        assert!(app.should_exit().is_some());
        assert!(
            app.world()
                .non_send::<NetworkRuntime>()
                .renet
                .disconnect_reason()
                .is_some()
        );
    }
}
