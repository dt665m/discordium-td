# Predicted presentation contract

Transient effects are accepted, identified, aged, and expired in the shared
simulation. Network receive code restores authoritative state; it does not spawn
meshes or invent effect identities. Client prediction and server authority both
use the simulation installed by `dreamwake_sim::DreamwakePlugin` through the
`DreamSimulation` wrapper.

`engine_core::PresentationInstance` contains a neutral primitive, position,
radius, age, duration, and stable identity. Its `PresentationId` consists of
match epoch, owner, action sequence, and stable slot. `RadialPulse` is the current
primitive; game abilities choose when to emit it.

The generic `PresentationPlugin` ages instances in the schedule supplied by the
game and despawns expired effects. Dreamwake assigns its `PresentationStep` set
to the ordered presentation phase of `DreamStep` and stores instances directly
as ECS components. Snapshots
include the active instances for complete restore/replay. They also save the
engine's authoritative action, motor, combat/status, loadout, projectile, summon,
and delayed-action state alongside game payloads. Restoring only positions or
visible effects would lose cooldowns, hit history, or pending execution and make
replay diverge.

The game client projects the displayed snapshot into
`engine_client::presentation::PresentationFrame`. Renderers consume this frame
after `PresentationSet::Adapt`, in `PresentationSet::Render`. Stable visual IDs
let replacement renderers retain per-effect animation/correction state without
coupling to ability or encounter enums. The development gizmo renderer is stateless; it draws the current presented frame
directly when enabled in the debug menu. Selecting gizmos hides the procedural graphics; switching back restores them
with their current correction and animation state.

To add a mechanic:

1. Validate and execute it in game simulation, composing the relevant engine
   mechanics. Keep named ability definitions, balance, and effect mappings in
   the game. Create presentation state only for accepted actions.
2. Allocate identities deterministically from the action and stable slots;
   replay must not allocate random replacement IDs.
3. Map its presented state into neutral primitives in the game adapter, or
   extend the renderer-neutral contract when the visual requires it.
4. Cover accepted/rejected actions, replay identity, expiry, reset, and snapshot
   transport. Renderer tests should cover correction and disappearing state.

Continuous feedback can read predicted actor state. Rendering owns meshes,
materials, animation, and correction blending; gameplay lifetime and damage stay
in simulation. See [networking](netcode.md) for the authoritative data path.

The previous TD contract and historical hit-rewind implementation are recorded
in [the archived TD document](history/td-predicted-presentation.md).
