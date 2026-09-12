# Client UI composition

`engine_client::ui::EngineUiPlugin` supplies passive labels, meters and
screen-facing world UI. Install it alongside Bevy's UI plugins. Games own the
values, palette, fonts and gameplay decisions. UI stays in the client engine;
headless `engine_core` simulations do not load rendering features.

## World billboards

Spawn a root with `WorldUi`, `WorldUiAnchor` and an explicit `UiTargetCamera`.
Compose its contents with normal Bevy UI or the engine's `label`, `meter` and
`meter_with_marker` scene functions:

```rust
use bevy::prelude::*;
use engine_client::ui::{WorldUi, WorldUiAnchor, WorldUiOwner, meter};

commands.spawn_scene(bsn! {
    WorldUi
    template_value(WorldUiAnchor::Entity {
        entity: actor,
        offset: Vec3::Y * 2.4,
    })
    Node { width: px(60), height: px(5) }
    Children [meter(percent(100), 5.0, 0.75, Color::srgb(1.0, 0.3, 0.32), Color::BLACK)]
}).insert((WorldUiOwner(actor), UiTargetCamera(camera)));
```

The UI root is independent of the actor's transform hierarchy. `WorldUiOwner`
uses the engine's existing Bevy ownership relationship to remove the UI when the
actor despawns; do not attach the root using `ChildOf(actor)`. Entity anchors
follow the current global position, including ancestors, with a world-space
offset that does not rotate or scale with the actor. `WorldUiAnchor::World`
accepts a position directly for snapshot-driven or transient labels.

Projection runs after camera updates and before UI content/layout, computing
fresh transforms without waiting for transform propagation. Root coordinates
account for the camera viewport, display scale and `UiScale`. The default
`UiTransform` centers the entire node, including dynamically sized text. Sizes
remain in UI pixels through camera rotation and zoom; normal `UiScale` still
applies. These are overlay elements and are not occluded by scene geometry.

The plugin owns billboard `Visibility`, hiding invalid, offscreen or unprojectable
anchors and inactive/missing cameras. Games can control additional visibility
through `Node::display`. The shared scenes pass pointer input through to the game.

## Meter updates

`meter_with_marker` puts the supplied game marker and `MeterFraction` on the fill
child. Update `MeterFraction` with `set_if_neq`; `UiSet::Widgets` clamps it to
0–1 and updates the left-aligned fill before layout. Non-finite values render
empty. A full meter remains colored. Background and fill are UI colors, so scene
lighting and actor facing cannot darken or rotate them.

Games normally adapt values in `Update`. Adapters running in `PostUpdate` should
order themselves before `UiSet::Widgets` or `UiSet::Billboards` as appropriate.

## Optional debug UI

Add `engine_client::ui::DebugUiPlugin` separately to get frame diagnostics. Supply
`ui::debug::DebugUiSettings` before installation to select the toggle key and
palette. `DebugUiState::visible` controls the passive panel. Attach game-specific
rows to `DebugUiRoot` in `PostStartup`, after the plugin creates the root in
`Startup`. Frame sampling uses Bevy diagnostics; gameplay and network metric
formatting stay with their adapters.

Dreamwake composes health billboards and HUD meters with these primitives.
Custom renderers can mark actor roots with `dreamwake_client::ui::OverheadAnchor`
to align meters with their displayed poses; otherwise the UI follows snapshot
positions. The F3 panel extends the engine frame metrics with
game/network/prediction rows, the current 32-metre graph cell and disclosed prop
count. F6 network tools include **Graph cells**, **Enemy aggro** rings for
disclosed enemies, and the two spatial playtest routes. The camera follows the
Traveler across the full demo map.
