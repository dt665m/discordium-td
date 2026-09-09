# Dreamwake typography

Fira Sans Regular and Fira Sans Bold are embedded directly in the native executable and WebAssembly build. There are no runtime font requests or platform font dependencies.

## Attribution and license

Copyright (c) 2012–2015, The Mozilla Foundation and Telefonica S.A.

These unmodified font files are redistributed under the [SIL Open Font License 1.1](OFL.txt). Retain the font copyright and license when distributing the game. The game code’s license does not replace this font license.

Source: the `bevy_feathers` **0.19.1** crate, `src/assets/fonts/FiraSans-Regular.ttf`, `FiraSans-Bold.ttf`, and `FiraSans-License.txt`. The font files were copied byte-for-byte from that release; `FiraSans-License.txt` is included here as `OFL.txt`.

Upstream source tree: [Bevy v0.19.1 font assets](https://github.com/bevyengine/bevy/tree/v0.19.1/crates/bevy_feathers/src/assets/fonts).

## File verification

| File | SHA-256 |
| --- | --- |
| FiraSans-Regular.ttf | `3dabf3d48bf4599f95cffd92f99ea426a014d5311f52a5eb5ec3af265e97cd97` |
| FiraSans-Bold.ttf | `74b477a3e2b7c745e6dac8c01764c8fd82e53763b47ef1eccd24dce36d2ff71f` |
| OFL.txt | `ce5303dcecb360dab738cc93ecf5d1483855f9fc9347938cc1949beb0eb132b7` |

The Unicode character maps of both included faces were checked for the interface’s `·`, `→`, `−`, `—`, `×`, and `•` characters. All are supported. The unsupported four-point star `✦` in critical-hit labels is replaced with ASCII `*` by the typography plugin before text layout.

For a standalone web distribution, include `OFL.txt` in the output (for example with Trunk’s `copy-file` asset handling), alongside the embedded font bytes in the application.
