# Dreamwake typography

Fira Sans Regular and Fira Sans Bold are embedded directly in the native executable and WebAssembly build. There are no runtime font requests or platform font dependencies.

## Attribution and license

Copyright (c) 2012–2015, The Mozilla Foundation and Telefonica S.A.

These unmodified font files are redistributed under the [SIL Open Font License 1.1](OFL.txt). Retain the font copyright and license when distributing the game. The game code’s license does not replace this font license.

Source: the unmodified `FiraSans-Regular.ttf`, `FiraSans-Bold.ttf`, and
`FiraSans-License.txt` from [Bevy v0.19.1 font assets](https://github.com/bevyengine/bevy/tree/v0.19.1/crates/bevy_feathers/src/assets/fonts).
The license file is included here as `OFL.txt`.

The [typography plugin](../../../src/plugins/ui/typography.rs) embeds the fonts
and replaces unsupported critical-hit stars with ASCII `*` before layout.
The [Trunk entry point](../../../index.html) copies `OFL.txt` into browser builds.
