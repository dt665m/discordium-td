# Moving supports

Dreamwake's arena contains one low rotating platform centered at `(0, 0.125, -10)`
with half extents `(3, 0.125, 1.5)`. Its top is within the capsule controller's
walk-on step height. The game owns its stable collider identity, motion episode,
integer gameplay tick and phase. One rotation takes 1024 gameplay ticks. Intro,
reward and pause do not advance that clock. The checked-in binary32 quaternion
table defines every phase; neither authority nor replay derives motion from a
render transform or runtime trigonometry.

`MotionEnvironment` composes the immutable static collision manifest with the
platform's current cuboid and the exact consecutive `BaseFrame` endpoints.
Authority and restricted owner prediction call the same capsule controller. The
controller sweeps support carry before intent, preserves the local attachment
anchor and advances its pose tick. Walk-off, dash and blink detach the rider and
advance the attachment revision. A dead or inactive attached traveler still
receives zero-intent carry while another traveler keeps combat running.

Complete snapshots contain the platform state and each traveler's engine base
state. Restore validates the platform phase, collider identity, committed support
pose, attachment tick and local anchor before publishing the replacement world.
The static collision manifest identity remains separate from moving support
history.

The scoped public platform representation declares a deterministic trajectory.
`pose_at` accepts only ticks within 256 of its retained sample and no earlier
than its motion episode. An owner checkpoint names the platform as a required
base whenever attached or reachable within 32 command ticks during combat.
The envelope includes the capsule and rotating support extents, current speed and
velocity, and possible dash time. Twelve metres is its minimum; speed upgrades
expand it. A per-step reach guard stops prediction before an omitted platform can
participate in a sweep, even if the checkpoint is retained beyond 32 commands.
Blink remains authority-only and uses the full composed scene. Current Starfall
spheres clear this low platform vertically, including the largest Essence radius;
that content invariant has a test and must be revisited if projectile geometry
changes. The client's immutable group closure must supply exactly
those collider generations. Missing history, another scene or an incorrect
collider generation rejects prediction transactionally. A missing dependency
never becomes a guessed stationary platform.

Remote presentation samples the platform on the common remote timeline. The
local attached rider can display its platform at the owner's predicted gameplay
tick using the same declared trajectory. This display choice does not change
saved state, collision or replay dependencies.

Run `cargo test -p dreamwake_sim --lib platform` for game lifecycle, frozen-sample
replay, detach and restore checks. Generic translation, rotation, collision and
attachment validation are covered by
`cargo test -p engine_core --lib bases_tests`.
