# Canonical game and reusable engine

The workspace provides reusable Bevy plugins under `engine/` and develops
Dreamwake under `games/dreamwake/` as the canonical game.

The engine supports deterministic headless simulation, reusable actor mechanics,
stable presentation identities, bounded network messages, UDP/WebRTC sessions,
client prediction, camera/input projection, and replaceable graphics. Game rules,
content, catalogs, progression and art never become engine dependencies.

Dreamwake remains a cooperative action roguelite with server-authoritative
combat, per-player builds, shared encounters, local prediction/reconciliation,
and native/browser clients. Its current 60 Hz simulation, 20 Hz snapshots,
eight-player default, ten-room progression, Memory abilities and procedural
art are game choices, not engine requirements.

The default client uses prototype graphics; the prior art is an optional plugin.
A new renderer should be selectable without changing gameplay or the protocol.
A new game should compose the engine and supply its own simulation systems,
input/actions, snapshot types and presentation adapter.

Acceptance checks:

- Engine crates have no game dependency or game-specific branding in source.
- Health is owned by one authoritative component on each actor; snapshots
  project it and restore complete state for prediction.
- An unrelated headless prototype composes the same simulation plugins.
- Alternative renderers consume a shared presentation contract.
- Cooperative gameplay, rejection, replay, reconnect and real network behavior
  remain covered by tests.
- The native workspace and browser target compile.

See [architecture](docs/architecture.md), [networking](docs/netcode.md), and
[predicted presentation](docs/predicted-presentation.md). The original lane-defense
specification and application are preserved in checkpoint `e834361`.
