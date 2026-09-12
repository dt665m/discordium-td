use super::*;
use engine_server::Authority;
#[test]
fn overload_blocks_new_runs_and_recovers_without_resetting_existing_match() {
    let mut authority = DreamAuthority::new(19, false, 1);
    assert!(authority.admit_player(41, PlayerId::new(1).unwrap()));
    authority.update_health(engine_net::clock::TickHealth {
        overloaded: true,
        debt_ticks: 60,
        committed: ServerTick(0),
    });
    for action in [
        DreamAction::Start { lucid: false },
        DreamAction::Continue,
        DreamAction::Restart,
    ] {
        authority.menu(41, action);
        assert_eq!(authority.sim.phase(), RunPhase::Intro);
        assert_eq!(authority.epoch, 1);
    }
    assert_eq!(authority.notices.len(), 3);
    authority.step(Duration::from_millis(17));
    authority.sim.set_player_active(1, true);
    authority.update_health(Default::default());
    authority.menu(41, DreamAction::Start { lucid: false });
    assert_ne!(authority.sim.phase(), RunPhase::Intro);
    authority.update_health(engine_net::clock::TickHealth {
        overloaded: true,
        ..Default::default()
    });
    authority.menu(41, DreamAction::Restart);
    assert_eq!(authority.epoch, 1);
    assert_ne!(authority.sim.phase(), RunPhase::Intro);
}
