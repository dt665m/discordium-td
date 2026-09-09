# Production deployment

The browser client is at <https://discotd.datab.fun>, hosted by Cloudflare Pages
project `discordium-td` (production branch `main`). Its compiled HTTP endpoint is
<https://dsdp.datab.fun>, which proxies HTTPS to the Oracle Cloud server.

The server is Oracle instance `instance-20260219-1602`, public IP `161.118.201.238`.
The maintainer's SSH alias is `oracle-free` (`ubuntu@161.118.201.238`). The checkout
is `/home/ubuntu/projects/discordium-td`. Source Rust's environment with
`. ~/.cargo/env` in non-interactive SSH sessions.

## Server operations

The systemd unit is `/etc/systemd/system/discordium-td.service` and runs as `ubuntu`
with permission to bind HTTPS port 443. Native UDP uses port 5000 and WebRTC uses
UDP port 5001; these transports connect directly to the public IP.

```bash
ssh oracle-free
sudo systemctl status discordium-td
sudo journalctl -u discordium-td -n 100 --no-pager
sudo systemctl restart discordium-td
```

The unit sets the public HTTPS endpoint and allows the browser origin
`https://discotd.datab.fun`. The origin TLS files are `/home/ubuntu/origin-cert.pem`
and `/home/ubuntu/origin-key.pem`; keep their contents out of the repository.

Build with Rust 1.95 or newer using `cargo build --release --locked -p game_server
-j 2`. Copy the result to a versioned path in `target/releases/`, update the unit's
`ExecStart` and `TD_BUILD_REVISION`, then run `sudo systemctl daemon-reload` and
restart the service. Preserve the previous binary until the deployed client has
connected successfully. For rollback, restore the previous `ExecStart` and restart.

Public health check: `curl -fsS https://dsdp.datab.fun/healthz`.

## Browser publishing

### Dreamwake

Server and browser updated on 2026-09-08 with published `renet-cross` 0.6.1.
The server binary is `game_server-dreamwake-061-d1bc4aef48db` with eight slots.
The production browser is Pages release `2560e159.discordium-td.pages.dev`,
served at `https://discotd.datab.fun`. Both builds use registry Renet 2.0.0 and
renet-cross 0.6.1, with the application-root SCTP pin at
`cb94f37991c185fb9cc2fd41fbe92965e2a1f713`. See `docs/netcode.md`.

The server polls receives at 1 ms, generates Renet packets once per 60 Hz server
tick, and sends snapshots at 20 Hz. F6 reports upstream Renet's unchanged loss
estimate. Production WebRTC measured 0.0% after the SCTP fix, with matching
fingerprints for 1,000 messages in each direction; simultaneous native UDP also
measured 0%. Gameplay cast/dash and public HTTPS health were verified. Temporary
packet tracing was disabled after the comparison.

The current source snapshot is
`/home/ubuntu/projects/deployments/dreamwake-061-d1bc4aef48db/discordium-td/`.
It resolves renet-cross from crates.io and SCTP from the exact Git revision;
no sibling dependency checkouts are required. The saved `previous-service.conf`
in its parent directory restores server `game_server-dreamwake-sctp-9dfe50f4660b`,
whose matching browser release is `7dc7eb6e.discordium-td.pages.dev`.

The earlier server `game_server-dreamwake-stock-81d3704710c0` is still available
for rollback with the current browser, though it predates the SCTP correction.
Its saved override is
`/home/ubuntu/projects/deployments/dreamwake-trace-139c4c573a99/previous-service.conf`.

The previous release (`650a9a2e-ae0e-4fb2-85ea-90c64ede579c` and server
`game_server-dreamwake-ack-46df573a56d5`) used a Renet patch and must not be
restored as the standard fallback. The last earlier upstream-Renet pair is
Pages `5d29041d-0b73-494d-93de-f839f674ab75` and server
`game_server-dreamwake-aaa98f06c4f5`; its override is preserved at
`/home/ubuntu/projects/deployments/dreamwake-ack-46df573a56d5/previous-service.conf`.
That fallback predates the cadence correction. Restore its override to
`discordium-td.service.d/90-dreamwake.conf`, reload systemd, and restart only
with the matching Pages release.

Build the roguelite client with its production bootstrap endpoint:

```bash
just dreamwake-web-build https://dsdp.datab.fun
CLOUDFLARE_ACCOUNT_ID=d17e5f9b4425c16ef79568b0b4660380 \
  npx wrangler@4 pages deploy target/dreamwake-pages \
  --project-name discordium-td --branch main --commit-dirty=true
```

The corresponding server must run with `--dreamwake --max-clients 8`; Dreamwake
has a separate protocol from the tower-defense game. Publish both sides together.
The Oracle host has Rust `1.95.0` installed alongside its older default toolchain;
use `cargo +1.95.0 build --release --locked -p game_server -j 2` there.

For a source snapshot deployment, keep the staged source under
`/home/ubuntu/projects/deployments/<revision>` and its binary under
`/home/ubuntu/projects/discordium-td/target/releases/game_server-<revision>`.
The systemd override `discordium-td.service.d/90-dreamwake.conf` selects that
binary and mode while retaining the existing TLS, CORS, and network settings.
Removing that override restores the base unit's previous binary after
`systemctl daemon-reload` and a service restart. Roll back Pages to the matching
previous deployment at the same time.

### Original tower-defense client

From the repository root:

```bash
cd game_client
NO_COLOR=false TD_WEB_HTTP_BASE=https://dsdp.datab.fun \
  trunk build --release --cargo-profile web-release --config Trunk.toml
cd ..
npx wrangler@4 whoami
CLOUDFLARE_ACCOUNT_ID=d17e5f9b4425c16ef79568b0b4660380 \
  npx wrangler@4 pages deploy game_client/dist \
  --project-name discordium-td --branch main --commit-hash "$(git rev-parse HEAD)"
```

The wasm-specific Bevy feature list omits unused plugins to fit Pages' 25 MiB
per-file limit. Check the generated wasm size before publishing. Keep the previous
Pages deployment available for rollback, and coordinate server/client updates when
the network protocol changes. After deployment, verify an actual WebRTC connection
from the production browser client as well as the HTTP health check.
