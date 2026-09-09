# Dreamwake staging deployment — 2026-09-09

Deployed source commit `afc51b4e1c0e6cf3bcbd4592b74a897a1d9737e7` to
`oracle-free` and Cloudflare Pages project `discordium-td` (branch `main`).

- Browser: https://discotd.datab.fun
- Pages release: https://41f3db49.discordium-td.pages.dev
- Server health: https://dsdp.datab.fun/healthz
- Oracle source: `/home/ubuntu/projects/deployments/dreamwake-afc51b4/discordium-td`
- Installed binary: `/home/ubuntu/projects/discordium-td/target/releases/game_server-dreamwake-afc51b4`
- Service: `discordium-td.service`, override `discordium-td.service.d/90-dreamwake.conf`

The server was built with Rust 1.95.0, `--release --locked -p dreamwake_server -j 2`.
The override now supplies `ENGINE_CORS_ALLOWED_ORIGINS`, `ENGINE_HTTP_TLS_CERT`,
`ENGINE_HTTP_TLS_KEY`, and `ENGINE_PUBLIC_HTTP_BASE`, and unsets the inherited
`TD_` settings. TLS files, the browser origin, eight-player limit, HTTPS port 443,
native UDP port 5000, and WebRTC port 5001 are preserved. The retired `--dreamwake`
flag is removed.

## Browser packaging

`just web-build https://dsdp.datab.fun` succeeded, but its 26,291,587-byte WASM
exceeded Pages' 26,214,400-byte limit. Apply these two additional passes using
Binaryen `wasm-opt` 132 to the generated WASM before publishing:

```sh
wasm-opt INPUT.wasm -Oz --converge --enable-bulk-memory \
  --enable-nontrapping-float-to-int --strip-debug --strip-dwarf \
  --strip-producers -o intermediate.wasm
wasm-opt intermediate.wasm --flatten --rereloop -Oz --converge \
  --enable-bulk-memory --enable-nontrapping-float-to-int --strip-debug \
  --strip-dwarf --strip-producers -o OUTPUT.wasm
```

The published WASM is `dreamwake-c2d590521df96c15_bg.wasm`: 26,088,332 bytes
(8,632,731 bytes with Python's default gzip compression). Its filename uses the
first 16 SHA-256 hex digits. Both HTML references and the SHA-384 preload integrity
value were updated for the optimized bytes. The original oversized WASM was
removed from the upload directory. No gameplay source changes were needed.

## Verification

- Formatting, `just check` (including architecture), and all workspace tests passed.
- Browser release and Oracle server release builds passed.
- Public health returned HTTP 200 with the correct browser CORS origin.
- Systemd runs the versioned binary with zero automatic restarts.
- Native UDP autoplay and the deployed browser joined the same two-player game;
  native capture showed successful authoritative progression and shared rewards.
- Browser F6 showed WebRTC connected, approximately 97 ms RTT, 0.0% estimated loss,
  250 received snapshots, zero decode/transport errors, and zero browser drops.
- Browser reward selection advanced successfully after the native test exited.

## Matching rollback pair

Preserve and restore both sides together:

- Previous server: `game_server-dreamwake-061-d1bc4aef48db`
- Previous override: `/home/ubuntu/projects/deployments/dreamwake-afc51b4/previous-service.conf`
- Previous Pages deployment: `2560e159-a502-42de-a4bf-dadef59f8fad`

Restore the saved override to `/etc/systemd/system/discordium-td.service.d/90-dreamwake.conf`,
run `sudo systemctl daemon-reload`, restart `discordium-td`, and roll Pages back
to that matching deployment. The previous client is incompatible with the new
authoritative protocol.

## R2 WASM migration (same game/server release)

The follow-up Pages release is `https://6a847e05.discordium-td.pages.dev`.
`discotd.datab.fun` serves HTML and JavaScript from Pages, while WASM is served
from the dedicated R2 bucket `dreamwake-releases` through `assets.datab.fun`.
Oracle remains on `game_server-dreamwake-afc51b4`; it was not restarted.

The published object is:

`dreamwake/releases/afc51b4e1c0e6cf3bcbd4592b74a897a1d9737e7/dreamwake-4ba4cc2fb7c13229_bg.wasm`

This is the original Trunk WASM before emergency shrinking: 26,291,587 decoded
bytes, exceeding Pages' 25 MiB limit. It is uploaded as 8,692,584 gzip bytes with
`Content-Type: application/wasm`, `Content-Encoding: gzip`, and
`Cache-Control: public, max-age=31536000, immutable`. The decoded SHA-256 is
`4ba4cc2fb7c132291a8535362873525562fb2a5ffa849910c2c27a1088737d74`.
Both the init URL and preload URL/integrity were updated. Pages contains no WASM.

The bucket's checked-in CORS configuration is `deploy/r2-cors.json`; it allows
GET/HEAD from `https://discotd.datab.fun`. HTTPS download, gzip decoding, MIME,
CORS, and the full decoded checksum were verified. Live browser rendering and
WebRTC worked, with 218 snapshots, approximately 101 ms RTT, zero decode and
transport errors, and zero browser drops. Public Oracle health still returns OK.
A browser reload also rendered successfully with no console errors or warnings.
The packaging helper `scripts/package-r2-wasm.py` reproduced the deployed HTML.
Local artifacts and manifests are under
`target/deployments/dreamwake-afc51b4-r2{,-verified}/`.

Browser cache headers are active. Edge responses currently report `DYNAMIC`;
zone Cache Rules API access returned 403 with the current CLI credentials. A
Cache Rule for the release paths remains to be configured with suitable access.
This does not block R2 loading or browser caching.

To roll back only this hosting migration, restore Pages release
`41f3db49.discordium-td.pages.dev` and keep the current Oracle binary. That release
contains the size-optimized WASM and uses the same game protocol. For rollback
of the game refactor itself, use the older matching pair documented above.
