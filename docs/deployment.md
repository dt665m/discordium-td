# Deployment plan: Oracle server and versioned R2 browser releases

Status: current deployment plan. This supersedes the Pages-only deployment
approach and its emergency WASM shrinking procedure. WASM delivery through R2 is implemented for `afc51b4`. Pages continues to host
the HTML and JavaScript; broader asset migration remains future work.

## Verified account access

On 2026-09-09, Cloudflare's zone API confirmed that active zone `datab.fun`
belongs to account `d17e5f9b4425c16ef79568b0b4660380`
(`Dt665m@gmail.com's Account`). An authenticated `wrangler r2 bucket list`
against that account succeeded and returned an existing bucket. R2 bucket creation, object upload, custom-domain attachment, and CORS updates
have now succeeded with the current credentials. The dedicated bucket is
`dreamwake-releases`, served by `assets.datab.fun` with minimum TLS 1.2.
The CLI credentials cannot read/manage zone Cache Rules (HTTP 403).

## Current release and operational references

- Browser: https://discotd.datab.fun
- Server health: https://dsdp.datab.fun/healthz
- Oracle SSH alias: `oracle-free`; systemd service: `discordium-td`
- [Current release, build commands, validation, and matching rollback pair](history/deployment-2026-09-09.md)
- [Earlier deployment history](history/deployment-2026-09-08.md)

The deployment records describe completed releases, not alternative plans.
Keep the current client/server pair available until R2 delivery is verified.
Shared server configuration uses `ENGINE_` environment variables. Preserve TLS,
CORS, public addresses, and the eight-player limit when promoting a new binary.
Build clients and servers from the same release when the wire protocol changes.

## Architecture

Keep `discotd.datab.fun` on Pages for the small HTML launcher. Store the browser
build's WASM, JavaScript modules, and external assets together in R2 beneath an
immutable release prefix such as `/dreamwake/releases/<release-id>/`. Serve R2
through the dedicated custom domain `assets.datab.fun`. Browser caching is
enabled with immutable one-year headers. Edge caching remains pending: responses
currently report `CF-Cache-Status: DYNAMIC`; configure a Cache Rule making this
host's `/dreamwake/releases/` paths eligible for cache using origin cache headers. Keep Oracle's authoritative server at `dsdp.datab.fun`.

Pages documents a 25 MiB per-file limit and explicitly recommends R2 for larger
files. Use an R2 custom domain; the `r2.dev` development endpoint is not the
release delivery path. Start with direct R2 delivery; introduce a Worker only
if measured requirements call for custom routing or content negotiation.

## Current WASM packaging and publishing

Build with `just web-build https://dsdp.datab.fun`, then package the Trunk output:

```sh
python3 scripts/package-r2-wasm.py target/web-release target/releases/RELEASE_ID \
  --release RELEASE_ID
```

Use a new immutable release identifier and output directory each time. The
command produces `pages/`, `r2/`, and `manifest.json`; it preserves JavaScript on
Pages and updates both WASM references and the preload integrity hash. For the
initial migration only, `--wasm target/wasm-opt/release/dreamwake_bg.wasm` selected
the matching original build instead of the emergency size-optimized file.

Upload the gzip file from `r2/` to the manifest's `bucket/key` with Wrangler:

```sh
CLOUDFLARE_ACCOUNT_ID=d17e5f9b4425c16ef79568b0b4660380 \
  npx wrangler r2 object put BUCKET/KEY --remote --file WASM_GZIP_PATH \
  --content-type application/wasm --content-encoding gzip \
  --cache-control 'public, max-age=31536000, immutable'
```

Verify the public URL using `curl --compressed` with
`Origin: https://discotd.datab.fun`; verify the decoded SHA-256 against the
manifest before deploying `pages/` with Wrangler Pages. The upload directory
must contain no WASM. The checked-in `deploy/r2-cors.json` allows GET/HEAD from
the game origin only. Keep R2 objects for all retained Pages releases.

## Remaining implementation sequence

1. Add the remaining edge Cache Rule to the existing asset domain. The bucket,
   custom domain, and game-origin GET/HEAD CORS are configured. Decide how approved preview origins
   will be supported before enabling preview playtests. Use upload credentials
   scoped to the release bucket and keep them outside Git.
2. Add a repeatable packaging command. Build once, collect all generated assets,
   and create a release manifest containing commit, protocol identity, build
   settings, filenames, hashes, sizes, and matching server release. Generate the
   Pages launcher with absolute release-specific asset URLs. Preserve module
   import paths, preload URLs, and integrity hashes; ensure Bevy's future
   external asset paths resolve to the release prefix as well.
3. Upload assets before publishing the launcher. Set correct MIME types
   (`application/wasm`, JavaScript, etc.) and immutable cache headers. Verify
   object hashes, CORS, streaming WASM compilation, and cache behavior. Benchmark
   HTTP compression and validate Content-Encoding against delivered bytes;
   merely uploading a `.gz` or `.br` file is not sufficient.
4. Validate the candidate browser/server pair before promotion where practical.
   Use an isolated candidate server endpoint for protocol-breaking releases;
   do not test an incompatible preview against the shared active server.
   Promote the Oracle binary and Pages launcher in a coordinated release step.
   Record both identities; this is not an atomic switch across both providers.
5. Automate rollback of the matching server and Pages deployment. Retain every
   R2 prefix referenced by a retained Pages release; garbage collection must
   protect active and rollback releases and allow for cached launchers/open tabs.

Upload/verification failure must stop before the active launcher or server is
changed. A failed post-promotion smoke test should restore the previous pair.
Publish immutable release URLs directly in each Pages deployment rather than
using a mutable `latest` manifest that can mix incompatible assets.

## Optimization policy

The initial R2 deployment uses the original WASM without the emergency
post-build shrinking passes. Benchmark the current size-oriented build against a performance-
oriented release profile using comparable gameplay and browser conditions.
Choose settings from download time, startup/compile time, frame time, and memory
measurements. The recent optimization passes were not proven to hurt runtime
performance; the objective is to remove the hosting limit from that tradeoff.

Keep size reporting and regression budgets as diagnostics, but do not reject
R2 assets at the Pages 25 MiB threshold. Verify applicable R2 and CDN object/cache
limits when implementing; storage capacity and cacheability are separate limits.

## Acceptance criteria

- A real browser build larger than 25 MiB loads from the asset domain.
- Correct MIME, CORS, integrity, compression, and cache responses are verified.
- Cold and warm loads render and establish WebRTC with no decoding errors.
- Native UDP and browser clients share authoritative gameplay successfully.
- Upload failure leaves the active release intact; rollback restores a tested
  client/server pair without re-uploading or rebuilding assets.
- Release commands, infrastructure configuration, retention rules, and build
  performance measurements are documented and reproducible.

## Sources

- [Pages limits](https://developers.cloudflare.com/pages/platform/limits/)
- [R2 public buckets and custom domains](https://developers.cloudflare.com/r2/buckets/public-buckets/)
- [R2 CORS](https://developers.cloudflare.com/r2/buckets/cors/)
- [R2 cache configuration](https://developers.cloudflare.com/cache/interaction-cloudflare-products/r2/)
