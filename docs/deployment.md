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
