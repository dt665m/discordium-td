# Warcraft-Style Lane Defense (Hero + Tower Defense) Spec

## 1. Purpose

Define a production-ready blueprint for building a Warcraft 3-style lane defense game where:

- Enemies march down lanes.
- Players control heroes.
- Towers and hero abilities protect objectives.
- The server is authoritative for all game-critical simulation.
- Networking supports both native UDP clients and browser WebRTC clients through `renet_cross`.

## 2. Product Goals

- Deliver a playable multiplayer lane defense loop with hero control.
- Keep simulation authoritative on server to prevent client-side cheating.
- Support mixed transport in one match server:
- Native clients over UDP.
- Browser clients over WebRTC.
- Keep client code thin: input + rendering + prediction for local feel.
- Make the architecture extensible for additional maps, heroes, and modes.

## 3. Non-Goals (Phase 1)

- Full Warcraft RPG systems (inventory trees, item shops, quests).
- Matchmaking and account backend.
- Replay system.
- Full anti-cheat kernel/attestation.
- Dynamic navmesh/pathfinding at runtime.

## 4. Core Game Design

## 4.1 Match Structure

- Mode: Co-op defense.
- Players: 1-6.
- Map: 2-4 fixed lanes with predefined waypoint paths.
- Objective: prevent enemies from reaching lane-end objectives.
- Lose condition: team life reaches 0.
- Win condition: survive all waves or defeat boss wave.

## 4.2 Entities

- Hero:
- Player-controlled unit.
- Abilities, cooldowns, health, mana/energy.
- Can move between lanes and cast support/damage skills.
- Enemy:
- AI-driven lane unit following waypoints.
- Types: grunt, tank, runner, boss.
- Tower:
- Static defender placed on build nodes.
- Attack logic server-side.
- Objective/Core:
- Team-owned target at each lane end.
- Build Node:
- Pre-authored tower placement location.
- Projectile/Effect (optional entity in phase 1):
- Server-simulated hit/projectile metadata.

## 4.3 Wave Loop

- Preconfigured wave table defines:
- Spawn time offsets.
- Enemy archetype + count per lane.
- Gold/xp reward values.
- Every wave spawn is server scheduled (not client timed).
- Between waves: short prep window for tower builds/upgrades.

## 4.4 Economy

- Team gold pool or per-player gold (pick one and lock early; default: per-player).
- Gold awarded on enemy kills and periodic wave completion bonus.
- Tower build/upgrade costs fixed in data table.

## 5. Networking and Authority Model

## 5.1 Authority Rules

Server owns:

- Simulation tick/time.
- Hero movement truth.
- Enemy pathing and combat.
- Tower targeting/fire/damage.
- Wave spawning.
- Health, mana, cooldowns, gold.
- Win/lose resolution.

Client owns:

- Input capture.
- Visual interpolation/prediction.
- UI state derived from server snapshots/deltas.

## 5.2 Tick Rate

- Server fixed tick: 30 Hz.
- Client render: uncapped or display refresh.
- Client network send cadence: 30-60 Hz for command packets.
- All authoritative actions keyed by server tick.

## 5.3 Command Model

- Client sends input commands with:
- `seq` (u32)
- `client_time` (optional)
- command payload (move/cast/build/upgrade/sell/etc.)
- Server processes latest valid command per client at each tick.
- Stale/out-of-order commands dropped via sequence checks.

## 6. How to Use This Crate (`renet_cross`)

## 6.1 Crate Role

Use `renet_cross` as the network transport layer + SDP helper layer:

- `MixedServerTransport`
- `UdpNetcodeServerTransport`
- `WebRtcNetcodeServerTransport`
- `accept_offer_and_add_peer(...)`
- `accept_offer_axum_json(...)` with `axum` feature
- `MonotonicClientIdAllocator`
- `SessionCreateResponse`

Your gameplay simulation remains your own module/crate and is called from the server tick loop.

## 6.2 Server Bootstrap Pattern

1. Create one `RenetServer`.
2. Create UDP transport with `UdpNetcodeServerTransport`.
3. Create WebRTC transport with `WebRtcNetcodeServerTransport`.
4. Compose with `MixedServerTransport`.
5. Run HTTP bootstrap/signaling endpoints:

- `POST /api/session/new`
- `POST /api/webrtc/offer/:client_id`
1. In game loop:

- `server.update(dt)`
- `transport.update(dt, &mut server)`
- process renet events/messages
- run authoritative sim tick
- `transport.send_packets(&mut server)`

## 6.3 HTTP + SDP Hook Contract

- `POST /api/session/new` response:
- `SessionCreateResponse { client_id, udp_addr, webrtc_offer_url, ... }`
- `POST /api/webrtc/offer/:client_id` request:
- `SdpHttpOfferRequest { sdp }`
- response:
- `SdpHttpAnswerResponse { client_id, sdp }`

Behavior:

- Reject duplicate offer `client_id` with HTTP `409`.
- Invalid SDP as `400`.
- ICE/RTC setup failure as `422`.

## 6.4 Authentication Note

Phase 1 dev mode can use unsecure netcode auth for speed.
Production must switch to signed connect tokens and scoped session issuance.

## 7. Server Architecture (Reference)

## 7.1 Processes

- Single process (phase 1):
- Axum HTTP server thread/task for bootstrap + signaling + health.
- Authoritative game loop thread/task.

## 7.2 Shared Runtime State

- `Arc<Mutex<RenetServer>>`
- `Arc<Mutex<MixedServerTransport>>`
- `Arc<Mutex<SessionRegistry>>`
- `Arc<MonotonicClientIdAllocator>`
- `MatchState` (simulation state, preferably not behind mutex if loop owns it)

## 7.3 Subsystems

- Networking:
- Transport updates and packet IO.
- Session/Signaling:
- HTTP endpoints, client id issuance, offer acceptance.
- Simulation:
- ECS or custom data-oriented world state.
- Broadcast:
- Delta/snapshot build and send.
- Observability:
- metrics/logging for RTT/loss/queue pressure/tick time.

## 8. Game State and Data Model

Use deterministic-friendly plain structs for authoritative state.

```rust
type EntityId = u64;
type ClientId = u64;
type Tick = u32;

struct MatchState {
    tick: Tick,
    phase: MatchPhase,
    team_life: i32,
    players: HashMap<ClientId, HeroState>,
    enemies: HashMap<EntityId, EnemyState>,
    towers: HashMap<EntityId, TowerState>,
    projectiles: HashMap<EntityId, ProjectileState>,
    lane_paths: Vec<LanePath>,
    wave_runtime: WaveRuntime,
    gold: HashMap<ClientId, u32>,
}
```

Lock these invariants:

- IDs generated server-side only.
- No client can create/delete entities directly.
- All combat resolution in one server tick pipeline order.

## 9. Protocol Spec (Renet Payloads)

Use `serde` + binary serialization (current project uses `bincode`).

## 9.1 Client -> Server (`Unreliable` mostly)

- `MoveCommand { seq, dir: [f32; 2] }`
- `CastAbilityCommand { seq, ability_id, target_pos/entity_id }`
- `BuildTowerCommand { seq, node_id, tower_type }` (can be `ReliableOrdered`)
- `UpgradeTowerCommand { seq, tower_id }` (reliable)
- `SellTowerCommand { seq, tower_id }` (reliable)
- `PingCommand { seq }` (optional)

## 9.2 Server -> Client

- `JoinSnapshot` on connect/reconnect (`ReliableOrdered`)
- `WorldDelta` every tick or when changed (`Unreliable`)
- `ReliableGameEvent` for critical one-shot outcomes (`ReliableOrdered`)
- wave start, objective damaged, defeat/victory, tower build accepted/rejected.

## 9.3 Channel Mapping

- `DefaultChannel::Unreliable`:
- movement commands
- high-frequency state deltas
- `DefaultChannel::ReliableOrdered`:
- join snapshot
- build/upgrade/sell confirmations
- phase transitions and critical events

## 10. Authoritative Tick Pipeline

Run each fixed tick in this order:

1. Collect network inputs for all clients.
2. Validate commands:

- ownership checks
- cooldown/resource checks
- map/node legality
1. Apply accepted player commands.
2. Advance enemy AI/path movement.
3. Advance hero movement.
4. Resolve tower targeting and attacks.
5. Resolve projectile/effect hits.
6. Apply damage/deaths/rewards.
7. Process objective damage and win/lose checks.
8. Spawn scheduled wave units.
9. Build `WorldDelta` + reliable event queue.
10. Send outbound packets.

Do not reorder without explicit reason; this determines gameplay feel and fairness.

## 11. Prediction/Reconciliation Guidance

- Predict only local hero movement on client.
- Do not predict enemy AI, tower damage, or economy.
- Server sends `last_processed_input_seq` for each client.
- Client drops acked commands and replays pending commands from authoritative hero state.
- Keep reconciliation smoothing bounded to avoid visible snapping.

## 12. Server Validation Rules (Anti-Cheat Baseline)

- Ignore any command for a different `client_id`.
- Clamp movement vectors to unit circle.
- Reject cast/build when cooldown/resource requirements fail.
- Reject tower build unless node is valid, empty, and affordable.
- Enforce ability ranges and targeting rules server-side.
- Never trust client-reported damage, gold, xp, or kill events.

## 13. Content/Data Authoring

Store gameplay constants in external data files (JSON/TOML/RON):

- Hero archetypes.
- Enemy archetypes.
- Tower archetypes.
- Ability definitions.
- Wave definitions per map.
- Lane waypoint lists.

Load on server startup and send minimal needed read-only metadata to clients.

## 14. Minimal Milestones

## M1: Network + Empty Arena

- Server accepts UDP + WebRTC clients simultaneously.
- Players can connect/disconnect cleanly.
- Hero move commands round-trip with prediction/reconciliation.

## M2: Single Lane Combat

- One lane, one enemy archetype, one tower type.
- Wave spawns and objective damage works.
- Victory/defeat condition works.

## M3: Multi-Lane + Hero Abilities

- 2-4 lanes.
- Hero abilities with cooldowns and targeting.
- Build/upgrade/sell flow.

## M4: Polish + Production Prep

- Metrics dashboards/log structure.
- Better session/auth model (signed tokens).
- Reconnect handling.
- Deployment profile for TLS + HTTP signaling.

## 15. Test Plan

## 15.1 Unit Tests

- Movement integration and bounds clamping.
- Tower target selection correctness.
- Damage and death resolution order.
- Wave scheduler timing.
- Build legality checks.
- Economy update correctness.

## 15.2 Transport/Integration Tests

- Mixed transport coexistence in one server.
- Connect/disconnect events for UDP and WebRTC.
- SDP endpoint duplicate-id and invalid-SDP paths.
- No cross-send between transport backends.

## 15.3 Simulation Consistency

- Deterministic step test for fixed seed and scripted commands.
- Reconciliation test with dropped/reordered movement commands.

## 16. Operational Guidance

- Expose `GET /healthz`.
- Track:
- tick duration (p50/p95/p99)
- connected clients
- input queue depth
- RTT/loss by client
- WebRTC ingest pressure
- Log at `info` for lifecycle, `debug` for sampled network details.

## 17. Default Decisions Locked by This Spec

- Genre mode: co-op lane defense with controllable heroes.
- Server authority: strict for all combat/economy/pathing.
- Tick rate: 30 Hz server fixed.
- Mixed transport: UDP + WebRTC in same authoritative server.
- Signaling: SDP-over-HTTP with non-trickle offer/answer for phase 1.
- Serialization: binary payloads over renet channels.
- Client prediction scope: local hero movement only.

## 18. Handoff Checklist for New Project

- Create crate/module layout:
- `game_server` (authoritative sim + transport glue)
- `game_shared` (protocol + shared enums/types)
- `game_client` (render/input/prediction)
- Add `renet_cross` dependency.
- Implement HTTP bootstrap/signaling endpoints.
- Implement authoritative tick pipeline from Section 10.
- Implement shared protocol from Section 9.
- Add milestone tests from Section 15.
