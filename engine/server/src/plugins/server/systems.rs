use super::*;

pub(super) fn poll_server<A: Authority + 'static>(
    mut driver: NonSendMut<ServerDriver<A>>,
    elapsed: Res<ServerElapsed>,
    time: Res<Time<Real>>,
    failure: Res<ServerFailure>,
    mut exits: MessageWriter<AppExit>,
) {
    if failure.error().is_some() {
        return;
    }
    if let Err(error) = driver.poll(elapsed.0.unwrap_or_else(|| time.elapsed())) {
        failure.record(error);
        exits.write(AppExit::error());
    }
}

pub(super) fn shutdown_server<A: Authority + 'static>(
    mut driver: NonSendMut<ServerDriver<A>>,
    mut exits: MessageReader<AppExit>,
    failure: Res<ServerFailure>,
) {
    if exits.read().next().is_some() {
        if let Err(error) = driver.shutdown() {
            failure.record(error);
        }
    }
}
