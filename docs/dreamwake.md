# Dreamwake

Dreamwake is the canonical game under `games/dreamwake/`. It composes the reusable
plugins under `engine/`; see [architecture](architecture.md). Dreamwake always starts with its procedural game graphics.
The default-enabled `debug-tools` feature adds an F6 menu toggle for gizmo rendering
(initially off); use `--no-default-features` to omit those development controls.

Dreamwake is a cooperative, top-down 3D action roguelite built on this repository's Rust, Bevy 0.19.1, Renet, and renet-cross stack. Play Vesper, the Moonbound Traveler, through ten rooms across three dreamscapes. Each player develops a separate Memory build while the party shares enemies, encounter progress, and the final boss.

The default admission limit is **eight players**. Hosts can configure **1–1,024** with `--max-clients`. Eight simultaneous UDP clients have been verified; the upper configuration limit is not a measured gameplay or performance capacity.

## Start playing

For the hosted friends playtest, open [Dreamwake](https://discotd.datab.fun).
Everyone uses the same link and joins the same eight-slot party. Invite companions
before entering; each player readies in the lobby. The first connected player
leads the party and selects the normal or Lucid run. This deployment uses the
Oracle server at `https://dsdp.datab.fun`.

Run commands from the repository root. Install Rust 1.95 or newer and `just`; browser builds also require Trunk and the `wasm32-unknown-unknown` target.

### Native: host a party

```bash
just dreamwake
```

Choose **Host** in the opening menu, then choose the normal or Lucid dream. Companions join the displayed invitation address. Everyone selects Ready before the first encounter begins.

Host starts the actual `dreamwake_server` entry point in a separate thread, then connects the local player through the same authenticated transport as other clients. Keep the host application open to keep this server running. Its ports are HTTP bootstrap **18082**, UDP **15002**, and WebRTC **15003**.

To host immediately with a different capacity:

```bash
cargo run -p dreamwake_client -- --host --max-clients 12
```

A companion joins an existing server with:

```bash
cargo run -p dreamwake_client -- --http-base http://HOST_ADDRESS:18082
```

Replace `HOST_ADDRESS` with the host's reachable address. Native clients use UDP; browser clients use WebRTC. They can join the same party.

### Dedicated server

```bash
just dreamwake-server
```

This starts Dreamwake with eight slots. Use `just dreamwake-server 12` for twelve slots. The dedicated defaults are HTTP **8080**, UDP **5000**, and WebRTC **5001**. A local native client connects with:

```bash
cargo run -p dreamwake_client -- --http-base http://127.0.0.1:8080
```

For other computers, advertise the address they can reach. For example, replace the example LAN address below with the server's address:

```bash
cargo run -p dreamwake_server -- --max-clients 8 \
  --http-bind 0.0.0.0:8080 \
  --udp-bind 0.0.0.0:5000 \
  --webrtc-bind 0.0.0.0:5001 \
  --public-http-base http://192.168.1.50:8080 \
  --public-udp-addr 192.168.1.50:5000 \
  --public-webrtc-addr 192.168.1.50:5001
```

The HTTP and selected game transport ports must be reachable by the clients. For reproducible encounters, add `--dream-seed 8192`. `--dream-lucid` initializes the challenge option. There is no party leader. Anyone can enter the dream without waiting for other connected players; friends can join the ongoing run. Anyone can start another run after defeat or victory. Mid-run restart and pause are available only when playing solo.

### Browser client

Start a Dreamwake server first, then:

```bash
rustup target add wasm32-unknown-unknown
just dreamwake-web
```

Open [the local browser client](http://127.0.0.1:1421). It fills the browser canvas and defaults to the server at `http://127.0.0.1:8080`. To join another server, use the URL's encoded `server` parameter, for example:

[Browser client with an explicit server](http://127.0.0.1:1421/?server=http%3A%2F%2F192.168.1.50%3A8080)

Replace the example address before joining. A browser runs the client only; start the server through native Host or the dedicated command. Click the game canvas to focus keyboard input and enable browser audio.

To build the native release binary:

```bash
just dreamwake-build
```

The executable is `target/release/dreamwake` (`dreamwake.exe` on Windows). The game graphics are enabled by default. Development builds expose an optional gizmo renderer in the F6 menu.

Earlier archives under `target/dreamwake-release/` predate this consolidation.
Rebuild from the current workspace before sharing a client/server pair; see the
[deployment guide](deployment.md) for current build commands and configuration.

## Controls and party flow

| Action | Keyboard / mouse | Gamepad |
| --- | --- | --- |
| Move | WASD | Left stick |
| Aim | Mouse cursor | Right stick |
| Basic strike | Hold left mouse button | Hold right trigger |
| Dash | Space | Left bumper |
| Memories, slots 1–4 | Q / E / R / F | A / X / Y / B |
| Choose reward 1–3 | 1 / 2 / 3, or click a card | A / X / Y |
| Select Memory target slot | Z / X / C / V, or click a slot | D-pad left / right in menus |
| Ready / continue | Enter, or click Ready | A in the lobby / transition |
| Build | Tab | Back / Select |
| Pause / menu | Escape | Start |
| Mute / unmute | M, or menu button | Menu button with pointer |
| Zoom | + / − | — |
| Restart | F5 while paused or after an ending; host only | Host's menu button with pointer |

A dash has a 1.15-second recovery and a brief invulnerability window. Basic strikes briefly lock movement; aim before committing. Enemy warning circles mark committed attacks, so moving out before the warning ends avoids their area damage.

Each player chooses their own rewards and target slots. Finishing your choices waits for companions; after all choices are complete, everyone readies for the next room. Disconnected players do not block readiness.

A downed Traveler cannot move or attack. Surviving companions can finish the room to revive them at 50% maximum health. The run ends in defeat when the whole party is down. Any Traveler can begin another run from the ending screen.

Menus pause a one-player session. **The world continues while multiplayer participants open menus.** Rearrange Memories between encounters; a paused solo session also supports swapping. Gamepad bindings are implemented, but physical-controller play has not been verified.

## Vesper and the Memory build

Vesper begins with 220 health and Crescent, Starfall, Nova, and Aegis. The third basic strike is empowered: when it connects, it restores **5 health** and removes **0.5 seconds** from every Memory's remaining cooldown. This rewards alternating aimed basic attacks with abilities.

The values below are base rank-one values before Memory power, rank, critical hits, or Essences.

| Memory | Effect | Base cooldown |
| --- | --- | --- |
| Crescent | Directional sweep for 48 damage with knockback; 5-unit reach. | 3.3 s |
| Starfall | Aimed projectile for 38 damage. | 2.5 s |
| Nova | A 5-unit radial explosion for 52 damage. | 5.5 s |
| Riftstep | Blink up to 6 units toward your aim, gain brief protection, and deal 36 damage around arrival. | 4.5 s |
| Aegis | Gain 55 shield for 7 seconds and repel nearby enemies for 22 damage. | 7.5 s |
| Dream Wisp | A following summon lasts 9 seconds and fires 15-damage bolts at nearby enemies. | 10 s |

A Memory reward can replace any slot. Choosing the same Memory already in that slot increases its rank instead. Each additional rank adds 25% of base power, up to **rank 8**. Replacing a Memory resets its rank to one and carries its attached Essence into the replacement. Swapping moves the whole Memory, including rank and Essence.

Each Memory holds one Essence. Reward comparisons explain the change on the selected slot before you commit.

| Essence | What changes |
| --- | --- |
| Twin | Crescent and Nova repeat after 0.18 s at 60% power. Starfall fires three full-power projectiles. Riftstep adds a delayed Nova for 31.2 base damage. Aegis gains 50% more shield. Wisp summons two companions. |
| Echo | Repeat the Memory after 0.55 s at 65% power without another cooldown. Riftstep blinks again; Wisp creates a second summon. |
| Vast | Most areas grow 55%. Starfall grows wider and pierces four targets. Riftstep travels 9.3 units. Aegis grants 85.25 base shield. Wisps gain 18-unit targeting and piercing bolts. |
| Frost | Hits slow by 55% for 2.5 s. Already slowed enemies take 20% more Memory damage. |
| Leech | Restore health equal to 12% of actual damage dealt. Aegis also heals 18 base health immediately. |
| Haste | The attached Memory's cooldown is 38% shorter. Wisps fire every 0.42 s instead of 0.70 s; Aegis lasts 10 s instead of 7 s. |

Memory rewards are labeled Rare, Essences Epic, and level-up blessings Common. Room stat blessings are Rare. Leveling also adds 10 maximum health and restores 25 health to a living Traveler; the next reward sequence includes additional blessing choices.

| Blessing | Improvement |
| --- | --- |
| Keen Edge | +30 percentage points basic attack power. |
| Deep Resonance | +28 percentage points Memory damage and shielding power. |
| Featherstep | 14% more movement speed, capped at 13 units/second; refresh dash. |
| Perfect Lucidity | +15 percentage points critical chance, capped at 85%; critical damage is 190%. |
| Quickening | 18% shorter cooldowns, including current recovery; global cooldown scaling bottoms out at 28% of base before Haste. |
| Heart of the Dream | +50 maximum health and restore 75 health. |
| Moonwoven | +10 percentage points damage reduction, capped at 60%, plus 35 shield. |

Sanctuaries restore an additional 40% maximum health beyond the normal 10% room-entry recovery. Spend **45 shards** to raise a selected Memory by one rank, then choose a free blessing or continue. Purchases belong to that player; shards and experience are awarded to each party member for shared kills.

## The run

The ten-room route includes normal encounters, three elite challenges, two sanctuaries, and the Somnarch's final arena. The game graphics give the three dreamscapes distinct palettes with floating terrain, luminous plants, crystals, and ruins. Seeds vary encounter names, enemy combinations, reinforcement placement, reward choices, and which rooms contain elites. Later fights introduce additional reinforcement waves.

| Enemy | Behavior |
| --- | --- |
| Hollow | Approaches and commits a warned melee strike. |
| Stargazer | Maintains distance and launches aimed projectiles. |
| Skitter | Warns, then lunges toward the committed target position. |
| Cantor | Marks an area attack and restores nearby enemies' health. |
| Oathbreaker | Elite with a larger committed attack and outward projectile burst. |
| The Somnarch | Three phases with warning circles, expanding radial projectile patterns, summoned enemies, and recovery windows for counterattacks. |

Enemy health scales with the connected party. **Lucid Dream** adds enemies, raises enemy health by 28% and incoming damage by 22%, and awards 50% more shards. Restarting changes the run seed.

## Architecture, verification, and scope

The dedicated server runs the shared ECS simulation at **60 Hz** and publishes compressed full snapshots at **20 Hz**. Native UDP and browser WebRTC use the same authority. Clients predict through the shared simulation with a bounded input history, then reconcile to server snapshots. Rewards, room transitions, and endings come from the server. Reliable action sequences provide stable dash, cast, and repeat-effect identities.

Dreamwake applies attacks to the current server state; it does not implement historical hit rewind. See [Dreamwake netcode follow-ups](history/dreamwake-netcode-followups.md) for the historical distinction from the retired tower-defense networking path.

Relevant checks:

```bash
just check
cargo test -p dreamwake_sim
cargo test -p dreamwake_server
cargo test -p dreamwake_client
```

The automated coverage includes complete seeded runs, a complete two-player run over real UDP through all boss phases, eight-client UDP state/ownership checks, packet-loss recovery and rejoin, independent rewards, team defeat/revival, all Memory–Essence combinations, and snapshot replay. These checks do not establish 1,024-player performance.

Before consolidation, browser QA completed a normal seed-29 run with two real WebRTC players in
**115.8 seconds**, reaching Victory with **110 kills** through all three boss
phases. Automated input drove combat for the run, so this does not claim fully
manual combat for the entire run. Late-combat mean FPS was **107–119** on an
Apple M3 Max using local WebGL2; that is a local measurement, not a general
performance guarantee. The QA pass also confirmed Q/E/R/F Memory casts, dash,
pause/restart/mute, target V, and the second reward choice, including Vast
Aegis changing base shield from 55 to 85.25. Reward, sanctuary, and build
layouts were checked at 800×600 and 1280×720. Physical-gamepad support and
every screen size remain unverified.

The [official launch trailer](https://www.youtube.com/watch?v=sUfcplBV6WU)
was played and inspected for area effects and camera presentation as an
inspiration reference; this does not claim production parity.

Permanent unlocks, additional Travelers, and user-configurable control rebinding are not included. Progression is specific to the current run. The visual meshes and effects are procedural, and the sound effects and music are original synthesized audio.

The inspiration reference is the [official Shape of Dreams press kit](https://zebrapartners.com/neowiz-shape-of-dreams-press-kit/). Dreamwake uses its own setting, characters, assets, and rules. Engine references: [Bevy 0.19 release](https://bevy.org/news/bevy-0-19/) and [0.18 → 0.19 migration guide](https://bevy.org/learn/migration-guides/0-18-to-0-19/).

### Network tools (F6)

F6 opens the shared network graphs and original `bevy-net-debug` conditioner
controls in one scrollable panel. F3 has no network binding. The canonical client loads
`engine_client::network_tools::NetworkToolsPlugin`; game-specific
telemetry stays in their adapters. Dreamwake attaches the plugin's conditioner
handle to its actual UDP or WebRTC transport, including reconnects.

Connect with conditioning Off and let baseline RTT settle before choosing a
150/300 ms target. Delay, jitter, packet loss and outage controls affect real
transport packets. Hiding F6 preserves these settings; an on-screen notice
remains while impairment is active. Use Off to restore the connection.

Graphs show transport RTT, input-ack jitter, packet loss and send/receive rates.
Unavailable samples are gaps. Dreamwake uses full snapshots and prediction/replay,
so delta-baseline and interpolation rows are marked N/A; correction distance is
not yet sampled. This integration does not include the retired TD recorder/bridge.
