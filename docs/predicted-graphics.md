# Predicted graphics contract

The shared simulation accepts, identifies, ages and expires transient effects.
Network receive code restores authoritative state; it must not create independent
effect lifetimes or identities. Server authority uses `DreamwakePlugin` through
`DreamSimulation`; the admitted owner predictor shares the corresponding game
acceptance and transition functions with complete permitted dependencies.

## Simulation state

The [core graphics plugin](../engine/core/src/plugins/graphics/mod.rs) has no GPU
dependencies. `GraphicsInstance` stores a primitive, position, radius, age,
duration and `GraphicsId`. The identity consists of match epoch, owner, action
sequence and stable slot. Scoped actions also include source/control epochs,
stream and actor generation, so reconnects or ownership changes cannot alias a
still-retained effect. Game abilities choose radial pulses, oriented beams or
orbs; renderers supply their appearance.

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

Dreamwake renders friendly radial pulses as expanding teal/gold rings and crystals.
Enemy pulses use a red ground burst that starts at the committed target and radius,
then contracts; subsequent actor movement does not move that impact. Anonymous
coarse-cell events use small neutral rising motes, without a tactical ground ring
or a friendly color. Their approximate region is not a damage footprint.

## Starfall prediction and event delivery

[Starfall flight](../games/dreamwake/simulation/src/starfall.rs) shares cast
acceptance, cooldown activation, spawn specifications and engine projectile
integration between authority and the owner predictor. Its owner checkpoint
contains only that owner's flight and pending Echo state. It excludes enemy
state, hit identities, damage decisions, the global allocator and the match RNG.
Static environment sweeps are shared; enemy and cover contacts remain
authoritative and remove or correct the predicted projectile through checkpoints.
The rotating support is below even Vast Starfall's sphere; the clearance test
must continue to pass when changing projectile or support dimensions.

Each owner may have eight live or reserved Starfall projectiles. Twin requires
three available slots and Echo reserves two; an entire cast is refused before
activating its cooldown when its slots are unavailable. Echo's future authority
identity is allocated at acceptance, saved with its delay, and reused at spawn.
The server reserves its replication identity without publishing an actor and
retires a canceled reservation through the ordinary graph lifecycle.

The [client event adapter](../games/dreamwake/client/src/plugins/network/predicted_events.rs)
uses the engine's bounded `EventJournal` after a complete prediction update has
committed. The full action key and a stable spawn ordinal identify each projectile
or reserved Echo. Sound and camera cues use `OneShot` delivery, so replay does not
repeat them. Projectile tokens use `SimulationOwned` delivery: server wall-clock
time cannot expire a paused gameplay object. Simulation removal, rejection,
reconciliation or scope reset releases the token. Retirement never crosses a
still-live command, and the journal retains its fixed record and memory limits.

A late `BindRetired` records the authoritative entity without recreating an old
ghost. The renderer suppresses duplicate public projectiles using that binding;
network receive never spawns a replacement visual. A newly observed checkpoint
object's journal origin is attributed to its admitted baseline, while an existing
identity keeps its original attribution. This metadata is not an authoritative
shot timestamp; combat traces retain the execution clocks.

An accepted binding can arrive before the checkpoint that removes an older
projectile. A strictly newer generation at the same entity index proves that
the server retired the old incarnation, so the journal atomically cancels its
remaining presentation and binds the new event. The old tombstone prevents
delayed prediction or duplicate outcomes from resurrecting it. Equal-generation
conflicts, bindings from an older generation to a different event, and rebinding
one event to a different entity still fail. The replacement appears only from
its own committed simulation state; receipt alone cannot spawn it.

## Adding an effect

1. Validate the action and create effect state only when accepted by simulation.
2. Allocate its identity deterministically from the action and stable slots;
   replay must not allocate random replacement IDs.
3. Map the state into neutral graphics primitives, extending the contract only
   when the visual requires it.
4. Test acceptance/rejection, identity through restore/replay, expiry, reset and
   snapshot transport. Renderer tests cover correction and disappearing state.
