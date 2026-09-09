# Deployment runbook

Cloudflare Pages serves the browser launcher, JavaScript and other Trunk assets.
R2 serves immutable gzip-compressed WASM. The authoritative server runs on Oracle.
The endpoints below are deployment configuration, not a live-status report.

## Deployment targets

| Target | Configuration |
| --- | --- |
| Browser | [discotd.datab.fun](https://discotd.datab.fun) |
| Pages | Project `discordium-td`, production branch `main` |
| Server | `https://dsdp.datab.fun`; [health endpoint](https://dsdp.datab.fun/healthz) |
| R2 | Bucket `dreamwake-releases`, public asset base `https://assets.datab.fun` |
| Cloudflare account | `d17e5f9b4425c16ef79568b0b4660380` |
| Oracle SSH | `oracle-free` |
| Server service | `discordium-td.service` |
| Server checkout | `/home/ubuntu/projects/discordium-td` |
| Server override | `/etc/systemd/system/discordium-td.service.d/90-dreamwake.conf` |

Use a tested source revision and a new release ID containing only letters,
digits, hyphens or underscores. Keep the previous server binary, systemd override,
Pages deployment ID and corresponding R2 objects available as a rollback pair.
Protocol changes require matching client/server builds; coordinate their promotion.

Commands below run from the repository root unless stated otherwise. Replace
`YOUR_RELEASE_ID` with the chosen identifier. Use the same value on the build host
and Oracle. Rust 1.95+, the WASM target, Trunk, Python 3 and authenticated Wrangler 4
are required.

## Build and package the browser

```sh
RELEASE_ID=YOUR_RELEASE_ID
RELEASE_DIR="target/releases/$RELEASE_ID"
just check
just test
just check-web
just web-build https://dsdp.datab.fun
python3 scripts/package-r2-wasm.py target/web-release "$RELEASE_DIR" \
  --release "$RELEASE_ID"
```

The [packager](../scripts/package-r2-wasm.py) requires a new output directory and
exactly one WASM file with Trunk's root-relative init/preload references. It emits:

- `pages/`: Trunk output with WASM removed and HTML references/integrity updated.
- `r2/dreamwake-<first16-sha256>_bg.wasm.gz`: compressed WASM, named from its decoded bytes.
- `manifest.json`: release ID, bucket/key/URL, checksums, sizes and content metadata.

The manifest does not include source commit, protocol identity or matching server
binary. Keep that pairing with the release artifacts outside the source docs.
The build uses the runtime-oriented `web-release` profile and Binaryen `-O3`;
no additional size-shrinking pass is needed for Pages because WASM is uploaded to R2.

## Upload and verify WASM

```sh
export CLOUDFLARE_ACCOUNT_ID=d17e5f9b4425c16ef79568b0b4660380
R2_KEY=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["key"])' "$RELEASE_DIR/manifest.json")
WASM_GZIP="$RELEASE_DIR/r2/${R2_KEY##*/}.gz"
npx wrangler@4 r2 object put "dreamwake-releases/$R2_KEY" \
  --remote --file "$WASM_GZIP" \
  --content-type application/wasm --content-encoding gzip \
  --cache-control 'public, max-age=31536000, immutable'

ASSET_URL=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["url"])' "$RELEASE_DIR/manifest.json")
curl --fail --silent --show-error --compressed \
  --dump-header "$RELEASE_DIR/asset-headers.txt" \
  --header 'Origin: https://discotd.datab.fun' \
  "$ASSET_URL" --output "$RELEASE_DIR/verified.wasm"
python3 - "$RELEASE_DIR" <<'PY'
import hashlib, json, pathlib, sys
release = pathlib.Path(sys.argv[1])
manifest = json.loads((release / "manifest.json").read_text())
data = (release / "verified.wasm").read_bytes()
assert len(data) == manifest["bytes"]
assert hashlib.sha256(data).hexdigest() == manifest["sha256"]
PY
```

Check the saved headers for `Content-Type: application/wasm`, `Content-Encoding:
gzip`, immutable cache control and `Access-Control-Allow-Origin:
https://discotd.datab.fun`. The checksum must match decoded WASM, not gzip bytes.
Do not promote the launcher if upload or verification fails.

[Bucket CORS configuration](../deploy/r2-cors.json) allows GET/HEAD from the game
origin. When intentionally applying that configuration:

```sh
npx wrangler@4 r2 bucket cors set dreamwake-releases --file deploy/r2-cors.json
```

Preview sites need an explicitly allowed origin on both R2 and the server.
A protocol-breaking candidate also needs a matching isolated server and a browser
build using that server's endpoint.

## Stage and promote the server

On Oracle, stage the tested source at
`/home/ubuntu/projects/deployments/$RELEASE_ID/discordium-td`. Build on the server
or a compatible Linux target; a macOS release binary cannot run on Oracle.

```sh
ssh oracle-free
RELEASE_ID=YOUR_RELEASE_ID
. "$HOME/.cargo/env"
cd "/home/ubuntu/projects/deployments/$RELEASE_ID/discordium-td"
cargo build --release --locked -p dreamwake_server -j 2
install -m 755 target/release/game_server \
  "/home/ubuntu/projects/discordium-td/target/releases/game_server-$RELEASE_ID"
sudo systemctl cat discordium-td
sudo cp /etc/systemd/system/discordium-td.service.d/90-dreamwake.conf \
  "/home/ubuntu/projects/deployments/$RELEASE_ID/previous-service.conf"
sudoedit /etc/systemd/system/discordium-td.service.d/90-dreamwake.conf
```

Change the override's `ExecStart` executable to the staged versioned binary,
retaining its arguments and environment. The package is `dreamwake_server`, but
its binary is `game_server`. Game flags are `--seed` and optional `--lucid`.
Preserve the configured admission limit, normally `--max-clients 8`.

Preserve [server configuration](../engine/server/src/config.rs): TLS certificate/key,
CORS origin, public HTTP/UDP/WebRTC addresses and bind addresses. The configured
TLS paths are `/home/ubuntu/origin-cert.pem` and `/home/ubuntu/origin-key.pem`;
HTTPS uses port 443, native UDP 5000 and WebRTC UDP 5001. Keep private key contents
out of the repository. Public transport addresses must resolve to Oracle, not the
local defaults.

```sh
sudo systemctl daemon-reload
sudo systemctl restart discordium-td
sudo systemctl status discordium-td --no-pager
sudo journalctl -u discordium-td -n 100 --no-pager
curl --fail --silent --show-error https://dsdp.datab.fun/healthz
```

## Promote Pages and validate the pair

Back on the browser build host, retain the previous production deployment ID,
then publish the verified launcher:

```sh
npx wrangler@4 pages deployment list --project-name discordium-td --environment production
npx wrangler@4 pages deploy "$RELEASE_DIR/pages" \
  --project-name discordium-td --branch main
```

Check the production browser with both cold and warm loads: graphics render,
WASM downloads without integrity/CORS errors, and WebRTC receives authoritative
snapshots. Use F6 to inspect decode/transport errors. Join the same game from a
native UDP client and exercise movement, abilities and rewards. HTTP health alone
does not validate either gameplay transport. See [browser validation](bevy-integration.md#validation).

## Rollback and retention

If validation fails, restore the saved server override, reload systemd and restart
the service. In Cloudflare Pages, roll back to the retained matching production
deployment, then repeat health and browser/native connectivity checks. A
client-only change may retain the server only when protocol and behavior remain
compatible.

Never overwrite a published R2 release key. Retain every object referenced by the
active or rollback Pages deployments, including those needed by cached launchers
and open tabs. Rollback must use the retained artifacts rather than rebuilding them.
