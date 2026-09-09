use crate::DreamView;
use bevy::prelude::*;
#[cfg(target_arch = "wasm32")]
use dreamwake_sim::RunPhase;
#[derive(Resource, Default)]
pub(crate) struct Playtest {
    pub(crate) autoplay: bool,
    pub(crate) manual_rewards: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) capture_dir: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) quit_after: f32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) next_capture: f32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) captures: u32,
    pub(crate) next_decision: f64,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn playtest_capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut test: ResMut<Playtest>,
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<DreamView>,
    mut exit: MessageWriter<AppExit>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if keys.just_pressed(KeyCode::F9)
        || (test.capture_dir.is_some() && time.elapsed_secs() > test.next_capture)
    {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let dir = test.capture_dir.as_deref().unwrap_or("/tmp/dreamwake");
        let _ = std::fs::create_dir_all(dir);
        let path = format!("{dir}/dreamwake-{:03}.png", test.captures);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        info!(
            "Dreamwake capture: room={} phase={:?} hp={:.0} kills={}",
            view.0.room + 1,
            view.0.phase,
            view.0.hero.hp,
            view.0.kills
        );
        test.captures += 1;
        test.next_capture = time.elapsed_secs() + 8.0;
    }
    if test.quit_after > 0.0 && time.elapsed_secs() >= test.quit_after {
        exit.write(AppExit::Success);
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn playtest_capture(
    test: Res<Playtest>,
    view: Res<DreamView>,
    time: Res<Time<Real>>,
    mut previous: Local<Option<(usize, RunPhase, u8)>>,
    mut sample: Local<(f32, u32)>,
) {
    if !test.autoplay {
        return;
    }
    sample.0 += time.delta_secs();
    sample.1 += 1;
    let boss_phase = view.0.enemies.iter().map(|e| e.phase).max().unwrap_or(0);
    let state = (view.0.room, view.0.phase, boss_phase);
    if previous.as_ref() != Some(&state) {
        info!(
            "Dreamwake browser playtest: room={} phase={:?} enemy_phase={} party={} hp={:.0} kills={} elapsed={:.1}s mean_fps={:.1}",
            view.0.room + 1,
            view.0.phase,
            boss_phase,
            view.0.heroes.len(),
            view.0.hero.hp,
            view.0.kills,
            view.0.elapsed,
            sample.1 as f32 / sample.0.max(0.001)
        );
        *previous = Some(state);
        *sample = (0.0, 0);
    }
}
