# Predicted graphics contract

The shared simulation accepts, identifies, ages and expires transient effects.
Network receive code restores authoritative state; it must not create independent
effect lifetimes or identities. Server authority and client prediction both use
`DreamwakePlugin` through `DreamSimulation`.

## Simulation state

The [core graphics plugin](../engine/core/src/plugins/graphics/mod.rs) has no GPU
dependencies. `GraphicsInstance` stores a primitive, position, radius, age,
duration and `GraphicsId`. The identity consists of match epoch, owner, action
sequence and stable slot. Game abilities choose when to emit the current
`RadialPulse` primitive.

`engine_core::GraphicsPlugin` ages effects in the game-supplied schedule and
removes expired entities. Dreamwake assigns `GraphicsStep` to its graphics phase
and saves active effects in the snapshot's `presentations` field. Complete
[restore/replay](netcode.md#snapshots-and-prediction) must preserve both effects
and the gameplay state that creates them.

## Rendering

The [game graphics adapter](../games/dreamwake/client/src/plugins/graphics/adapter.rs)
projects displayed state into `engine_client::graphics::GraphicsFrame` in
`GraphicsSet::Adapt`. Renderers consume it in `GraphicsSet::Render`. Stable visual
IDs let renderers retain animation and correction state without importing game
ability or encounter enums.

Meshes, materials, animation and correction blending belong to the client.
Gameplay lifetime and damage remain in simulation. Continuous feedback may read
predicted actor components directly.

## Adding an effect

1. Validate the action and create effect state only when accepted by simulation.
2. Allocate its identity deterministically from the action and stable slots;
   replay must not allocate random replacement IDs.
3. Map the state into neutral graphics primitives, extending the contract only
   when the visual requires it.
4. Test acceptance/rejection, identity through restore/replay, expiry, reset and
   snapshot transport. Renderer tests cover correction and disappearing state.
