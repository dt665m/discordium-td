# Dreamwake play guide

Dreamwake is a cooperative, top-down action roguelite. Play Vesper, the Moonbound
Traveler, through ten rooms across three dreamscapes. Each player develops a
separate Memory build while the party shares enemies, encounters and the final boss.

## Start playing

Open [Dreamwake](https://discotd.datab.fun) for the hosted browser game.
For local development, install Rust 1.95 or newer and `just`, and run commands
from the repository root.

### Native party

```sh
just play       # Host and connect immediately
just dreamwake  # Open the Host/Join menu
```

Keep the host application open for the party to remain connected. Companions
join its reachable address:

```sh
just client http://HOST_ADDRESS:18082
```

Replace `HOST_ADDRESS` with the host's address. Native hosting uses HTTP bootstrap
port **18082**, UDP **15002**, and WebRTC **15003**. To host with a different
capacity, run `cargo run -p dreamwake_client -- --host --max-clients 12`.

### Dedicated server

```sh
just server
just client http://127.0.0.1:8080
```

The default capacity is eight; `just server 12` allows twelve players.
The admission setting accepts 1–1,024, which is a configuration bound rather than
a performance rating. Dedicated defaults are HTTP **8080**, UDP **5000** and
WebRTC **5001**.

For remote clients, advertise reachable addresses. Replace this example LAN IP
with the server's IP:

```sh
cargo run -p dreamwake_server -- --max-clients 8 \
  --http-bind 0.0.0.0:8080 \
  --udp-bind 0.0.0.0:5000 \
  --webrtc-bind 0.0.0.0:5001 \
  --public-http-base http://192.168.1.50:8080 \
  --public-udp-addr 192.168.1.50:5000 \
  --public-webrtc-addr 192.168.1.50:5001
```

The HTTP and selected game transport ports must be reachable. Use `--seed 8192`
for reproducible encounters and `--lucid` to initialize the challenge option.
See [deployment](deployment.md) for TLS, CORS and public hosting.

### Browser client

Install Trunk, start a server, then run:

```sh
just wasm-target
just web-dev
```

Open [the local client](http://127.0.0.1:1420). It connects to
`http://127.0.0.1:8080`. To select another server, use
`just web-dev http://192.168.1.50:8080`; the address is compiled into the client.
Browser configuration does not use URL parameters.

Browser clients use WebRTC and can share a party with native UDP clients.
The browser runs the client only. Click the canvas to focus keyboard input and
enable audio.

### Native release

`just dreamwake-build` creates `target/release/dreamwake`
(`dreamwake.exe` on Windows). Procedural game graphics are enabled by default.
The default-enabled `debug-tools` feature adds Gizmos and autoplay controls to
F6; build with `--no-default-features` to omit those extras.

## Controls

| Action | Keyboard / mouse | Gamepad |
| --- | --- | --- |
| Move | WASD | Left stick |
| Aim | Mouse cursor | Right stick |
| Basic strike | Hold left mouse button | Hold right trigger |
| Dreamlance | Right mouse button | Right bumper |
| Channeled beam | Hold middle mouse button | Hold left trigger |
| Dash | Space | Left bumper |
| Charged surge | Hold G, then release | Hold left stick button, then release |
| Memories, slots 1–4 | Q / E / R / F | A / X / Y / B |
| Choose reward 1–3 | 1 / 2 / 3, or click a card | A / X / Y |
| Select Memory target slot | Z / X / C / V, or click a slot | D-pad left / right in menus |
| Start / ready / continue | Enter, or screen button | A at start / transition; screen button in sanctuary |
| Build | Tab | Back / Select |
| Pause / menu | Escape | Start |
| Mute / unmute | M, or menu button | Menu button with pointer |
| Zoom | + / − | — |
| Performance metrics | F3 | — |
| Network tools | F6; graph cells, Gizmos and autoplay require `debug-tools` | — |
| Restart | F5 while paused or after an ending; Enter after an ending | A after an ending; menu button while paused |

Basic strikes briefly lock movement, so aim before committing. Dash provides a
brief invulnerability window. Enemy warning circles mark committed area attacks;
leave the area before the warning ends.

### Charged surge

Hold the charge control for at least six simulation ticks, then release to surge
in the release aim direction. Charge reaches its maximum after 36 ticks and
stays held until release or cancellation. A valid release spends 30 of 100
stamina, starts a 90-tick cooldown and consumes a versioned 12-tick movement
curve spanning 3–7.5 metres. Idle stamina regenerates at 18 per second. Releasing
before the minimum cancels without spending stamina. Pause and reward phases
freeze charge timing, cooldown and regeneration.

Stun, immediate dash, a successful Blink, death and loss of active ownership
interrupt the charge. Interruption preserves any stamina already spent and the
remaining cooldown. Reliable begin/release/cancel edges identify the same
charge episode and ownership stream; an old stream cannot release a new charge.

The shared simulation saves stamina, episode, phase, curve version, execution
cursor and release direction in complete and owner checkpoints. Holding follows
moving support; release detaches with the existing departure-velocity policy.
Each curve sample goes through the same capsule sweep as ordinary motion.
Blocking a sample consumes that sample, while a failed collision query preserves
the prior motion, resource and phase state. Owner prediction restores and replays
these fields; rendering reads the resulting actor state. Other players' public
views do not disclose stamina or cooldown.

## Party flow

Any connected Traveler can select **Enter the Dream** or **Lucid Dream** to begin.
There is no party leader or initial readiness vote. Friends can join an ongoing run.

Each player chooses their own rewards and Memory target slots. Once everyone has
finished, each player selects **Ready to Cross** for the next room. In a sanctuary,
**Continue** skips your remaining blessing choice. Disconnected players do not
block progression.

A downed Traveler cannot move or attack. Surviving companions can finish the room
to revive them at 50% maximum health. The run ends in defeat when everyone is down.

Any Traveler can start another run after victory or defeat. Restarting changes
the seed. Mid-run restart and simulation pause require a solo session; the world
continues when multiplayer participants open menus. Rearrange Memories between
encounters or while paused in solo play.

## Memories and rewards

Vesper starts with Crescent, Starfall, Nova and Aegis. Every third basic strike is
empowered; a connecting hit heals Vesper and reduces all Memory cooldowns.

| Memory | Role |
| --- | --- |
| Crescent | Directional sweep with knockback |
| Starfall | Aimed projectile |
| Nova | Radial explosion |
| Riftstep | Blink toward your aim and strike the arrival area |
| Aegis | Temporary shield and nearby knockback damage |
| Dream Wisp | Following companion that fires at nearby enemies |

A Memory reward replaces the selected slot. Selecting its existing Memory raises
its rank instead, up to rank eight. Replacements start at rank one and retain the
slot's Essence. Swapping moves the entire Memory, including rank and Essence.

Each Memory holds one Essence:

| Essence | Effect |
| --- | --- |
| Twin | Repeats or multiplies attacks, strengthens Aegis, or adds another Wisp |
| Echo | Repeats the Memory after a delay without another cooldown |
| Vast | Increases area, reach, projectile piercing or shielding |
| Frost | Slows enemies and increases Memory damage against slowed targets |
| Leech | Heals from damage; Aegis also heals immediately |
| Haste | Shortens cooldowns; Wisps fire faster and Aegis lasts longer |

Blessings improve basic attacks, Memory power, movement, critical chance,
cooldowns, health or defense. Leveling raises maximum health and adds blessing
choices. The in-game descriptions show current values and compare the reward
with the selected slot.

Sanctuaries heal the party and let each player spend **45 shards** to raise a
selected Memory by one rank before choosing a free blessing. Shared kills award
shards and experience to each party member. Progression lasts for the current run.

## Encounters

The demo arena has a 128-metre radius, with collision and visible floor dimensions
supplied by the game simulation. Forty-five veil shutters and their scoped markers
occupy a 32-metre grid across the arena, retaining the original central shutter.
Their opening, removal and visibility use the normal replicated cover pipeline.
Each shutter rests for 90 simulation ticks, then travels vertically for 30 ticks
to the opposite endpoint. The same authoritative height drives display and shot
collision; travelers and enemies can cross the magical surface.
Forty-five training enemies occupy the surrounding grid. An enemy detects an
active, living Traveler within 24 metres, including the boundary, and retains
that target as they move farther away. It keeps chasing and attacking that
Traveler without switching to a nearer distraction. Windup and recovery lock
movement. When the target leaves, becomes inactive or is defeated, the enemy
acquires the nearest eligible Traveler within the detection radius, breaking
distance ties by stable Traveler ID, or waits for someone to approach.
Training enemies do not hold encounters open or grant encounter rewards. Their
role, home position, activation state and retained target remain private
authoritative components and survive full checkpoint restore;
visible enemies use the ordinary public enemy scopes independently of aggro.

The game retains at most 128 heroes, including pending/inactive travelers, plus
45 training enemies and at most 32 encounter spawns over each entire room. With
45 covers, this bounds combat history to 250 poses per frame. This is a storage
safety limit; the normal demo profile remains eight active players. Full snapshots
keep their 8 MiB byte cap, with 524,288 structural nodes and 2 MiB of aggregate
key/text data for the bounded population and 32 retained frames.
Heroes start each new run with 10,000 health so movement and multiplayer testing
can continue comfortably. Damage, healing,
progression and defeat retain their normal rules.

The ten-room route includes normal encounters, three elite challenges, two
sanctuaries and the Somnarch's final arena. Seeds vary encounters, reinforcements,
reward choices and elite placement.

| Enemy | Behavior |
| --- | --- |
| Hollow | Approaches and commits a warned melee strike |
| Stargazer | Keeps distance and fires aimed projectiles |
| Skitter | Warns, then lunges at a committed target position |
| Cantor | Marks an area attack and heals nearby enemies |
| Oathbreaker | Larger committed attack and outward projectile burst |
| The Somnarch | Three phases with area warnings, radial projectiles, summons and recovery windows |

Enemy health scales with the connected party. **Lucid Dream** adds enemies,
raises their health and damage, and awards more shards.

## Diagnostics

F3 shows passive performance and network metrics. F6 opens network graphs and
packet conditioning in every build. Connect with
conditioning **Off**, let baseline RTT settle, then choose delay, loss or outage
settings. Closing F6 preserves those settings; select **Off** to restore normal
transport.

See [networking](netcode.md) for metric definitions, prediction and verification,
and [architecture](architecture.md) for the engine/game ownership contract.
