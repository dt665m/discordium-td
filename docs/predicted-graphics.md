# Predicted graphics contract

The shared simulation accepts, identifies, ages and expires transient effects.
Network receive code restores authoritative state; it must not create independent
effect lifetimes or identities. Server authority and admitted owner replay execute
`DreamwakePlugin` through `DreamSimulation`. The client retains one reusable
simulation runner; rollback history contains cloneable owner checkpoints, not
copies of a Bevy world.

The owner checkpoint includes the owner's latent actor state, projectiles,
companions, delayed actions and graphics. The shared player, memory, companion,
projectile and graphics systems advance these components during prediction.
Adding a skill using these capabilities does not require a second implementation
in the network client. New kinds of simulation state still need capture, restore,
bounded serialization and declared dependencies before they can participate in
rollback.

Scoped replay excludes undisclosed actors, enemy decisions, encounter progression
and authoritative hit resolution. It never restores the server's global allocator
or random stream. These boundaries select authority-only responsibilities; they
do not select a list of skills that are allowed to animate.

Owner transient storage is bounded to 64 projectiles, 16 companions, eight
delayed casts and 32 graphics instances. Shared admission reserves companion and
delayed spawns, and companion firing observes the projectile budget. These limits
apply to authority and replay alike. Starfall retains its smaller cast budget
below. The checkpoint stores bounded component collections in a canonical binary
field rather than allocating a separate prediction schema for each skill.

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

Public presentation replicas carry motion and lifetime capabilities. A known
linear projectile advances from its replicated speed and remaining lifetime,
including when only one sample arrives. Unknown future actor movement uses a
bounded velocity projection. Effect age, flashes, windup, companion lifetime and
damage-number age advance in gameplay presentation time; an authoritative pause
stops those clocks and resuming excludes the paused interval. These projections
cannot create hits or change simulation state.

Dreamwake renders friendly radial pulses as expanding teal/gold rings and crystals.
Enemy pulses use a red ground burst that starts at the committed target and radius,
then contracts; subsequent actor movement does not move that impact. Anonymous
coarse-cell events use small neutral rising motes, without a tactical ground ring
or a friendly color. Their approximate region is not a damage footprint.

## Projectile identity and event delivery

[Starfall flight](../games/dreamwake/simulation/src/starfall.rs), companion bolts
and delayed casts use the shared projectile and delayed-action components.
The owner checkpoint contains only that owner's continuation state. It excludes
enemy state, hit identities, pending impact decisions, the global allocator and
the match RNG.
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
committed. All cast sound and camera cues come from the shared simulation's
accepted action verdicts and use `OneShot` delivery, so replay does not repeat
them. Existing projectile bindings use the full action key and a stable spawn
ordinal for each projectile or reserved Echo. Those tokens use `SimulationOwned`
delivery: server wall-clock
time cannot expire a paused gameplay object. Simulation removal, rejection,
reconciliation or scope reset releases the token. Retirement never crosses a
still-live command, and the journal retains its fixed record and memory limits.

A late `BindRetired` records the authoritative entity without recreating an old
ghost. The complete owner checkpoint supplies all owner transients; separately
delivered public owner projectiles, companions and effects cannot create a second
copy. Existing binding retirement fences still suppress an older retained shot.
Network receive never spawns a replacement visual. A newly observed checkpoint
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

Do not add an ability-name branch to the client predictor. Compose the shared
simulation capabilities and provide the state needed to restore them.
