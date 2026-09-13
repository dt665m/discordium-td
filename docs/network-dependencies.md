# Network dependency and provenance register

Use [Cargo.lock](../Cargo.lock) as the authoritative complete dependency graph,
registry versions/checksums and resolved Git commits. Build/check with `--locked`.
This register covers the selected networking/simulation foundation and research
source exclusions; it is not a completed legal review of every transitive crate,
platform SDK, asset or distribution notice. The project packages declare
`MIT OR Apache-2.0`. Retain third-party notices required by the selected licenses
in shipped distributions.

## Selected implementation components

Declared licenses below were read from locally resolved Cargo manifests. Registry
checksum values remain in Cargo.lock rather than being duplicated here. License
identifiers are package metadata, not a substitute for reviewing bundled code,
assets, target-specific dependencies and notice files before release.

| Component | Locked version / immutable source | Declared license | Role |
|---|---|---|---|
| Bevy | 0.19.1, crates.io checksum in lock | MIT OR Apache-2.0 | Existing ECS, schedules, headless simulation and presentation adapter |
| renet | 2.0.0, stock crates.io checksum in lock | MIT OR Apache-2.0 | Upstream message delivery, fragmentation and retransmission with stock configuration |
| renetcode | 2.0.0, `https://github.com/dt665m/renet.git`, branch `fix/confirmation-keepalive-starvation`, exact commit in lock | MIT OR Apache-2.0 | Minimal upstreamable handshake correction: application payload sends cannot postpone confirmation retries while the peer is unconfirmed |
| renet-cross | 0.7.0, crates.io checksum in lock | MIT OR Apache-2.0 | Owned native/browser transport integration and secure session bootstrap; published from commit `b5dc558a3e851347225f8c7bc799028089a973ec` |
| bincode | 1.3.3, crates.io checksum in lock | MIT | Existing binary serde codec; schema and bounds remain application responsibilities |
| blake3 | 1.8.7, crates.io checksum in lock | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | Canonical checkpoint and ruleset digests |
| parry3d | 0.30.2, immutable registry version | Apache-2.0 | Capsule collision queries with enhanced determinism; continuation still requires qualification |
| serde | 1.0.229, crates.io checksum in lock | MIT OR Apache-2.0 | Serialization derives |
| miniz_oxide | 0.8.9 and 0.9.1, crates.io checksums in lock | MIT OR Zlib OR Apache-2.0 | Compression; engine_net declares the 0.8 line |
| tokio | 1.53.1, crates.io checksum in lock | MIT | Existing asynchronous transport/runtime support |
| sctp-proto | 0.10.4, `https://github.com/dt665m/sctp-proto.git`, commit `cb94f37991c185fb9cc2fd41fbe92965e2a1f713` | MIT OR Apache-2.0 | Existing transport patch; manifest and lock identify immutable commit |
| str0m | 0.23.1, `https://github.com/dt665m/str0m.git`, commit `591de71f4588b59e4850007414c6315a93943794` | MIT OR Apache-2.0 | Existing WebRTC transport patch; preserve this exact reviewed lock resolution |

The str0m manifest and lock use the same immutable `rev`. A lock protects a locked
build, while the explicit commit also protects regeneration. `rtrb = "0.4"` is a
workspace declaration currently absent from Cargo.lock, not an integrated or
license-reviewed selected dependency. If activated, record its exact resolution
and inspect its license before claiming that gate complete.

The game consumes the published renet-cross crate; a sibling checkout is not
required. Cargo.lock records its registry checksum. Local transport development
may temporarily use a path patch, but a lockfile does not pin local source bytes.

The game patches only `renetcode` from the Renet fork. The renet-cross repository
uses the same patch for its own tests. A crates.io package cannot retain a Git
dependency, and Cargo does not inherit dependency-level patches, so the game's
root override supplies the correction to the published renet-cross dependency.
The branch contains only the confirmation retry correction and its regression
test; packet-budget configuration extensions and vendored Renet copies are not
part of the selected graph. Both lockfiles must resolve the same fix commit.
Prefer stock upstream releases and remove the branch override when the fix is
released. Future Renet changes require a demonstrated correctness need or a
minimal feature suitable for upstream review; game and transport policy stay
outside that repository.

For a full release inventory, run `cargo metadata --locked --format-version 1`
for the release feature/target set and account for every resolved package's
license, source and notices. Verify Git package license files at the exact
checkout above, not a same-version crates.io copy. Do not label a new library
approved merely because a similarly named research project was reviewed.

## Research source boundary

The [source register](../todo/sources.json) and
[specification source bibliography](../todo/ENGINEERING_SPEC.md) distinguish
public documentation, inspected source and proposed algorithms. Their moving
branch URLs are research citations, not approved implementation dependencies.
No external research implementation is imported by this integration contract.

| Source family | Allowed role in this implementation | Code reuse boundary |
|---|---|---|
| Epic Replication Graph public documentation, S09/S51–S62 | Required conceptual spatial architecture: persistent nodes, shared prepare, connection gather and dormancy | Independently implement portable contracts. Public access does not license Epic source copying. No Unreal source, native driver, Iris or Fortnite code is incorporated |
| Valve Source SDK, S01–S05 | Conceptual command/replay/history reference | Excluded from copied/derived implementation pending exact-commit and applicable license approval |
| Unity samples, S23–S26 | Supporting conceptual comparison | Excluded from portable source reuse; handoff identifies Unity Companion License restrictions |
| FishNet, S44–S45 | Capability comparison only | Excluded from implementation derivation; handoff identifies competing-network-library restrictions |
| Quake III, S37–S39 | Historical conceptual comparison | No copied GPL implementation in this selected path; any reuse requires separate compatibility decision |
| Lightyear, S27–S31 | Existing-stack evaluation and regression scenario inspiration | Not currently selected or locked. Reuse requires immutable version, Bevy compatibility and exact license review; do not install a second gameplay-state writer |
| Mirror, GameNetworkingSockets, Quinn, Rapier | Supporting comparison / possible future evaluation | No new integration selected; exact version, license and conformance required before adoption |
| Riot, Respawn, Blizzard talks/articles | Behavioral requirements and measurement rationale | No claim of access to their proprietary source or current service configuration |
| todo/reference_model | Independently authored local contract models supplied with handoff | Test oracle only; no production transport/codec/performance qualification implied |

Release evidence must include the full component inventory, notices and exact
build manifest. Unreviewed source incorporation remains denied. These exclusions
are conservative repository engineering policy, not a general legal opinion
about every possible use of those projects.
