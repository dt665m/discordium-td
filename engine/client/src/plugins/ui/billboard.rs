use bevy::{prelude::*, transform::helper::TransformHelper, ui::FocusPolicy};

/// A passive root UI node centered on a world anchor. Supply `UiTargetCamera`
/// explicitly and do not parent this node to a world entity: use `WorldUiOwner`
/// for lifetime ownership. Size and text stay in UI pixels, independent of zoom.
/// The default -50% translation centers even dynamically sized labels.
#[derive(Component, Clone, Copy, Default)]
#[require(Node = Node { position_type: PositionType::Absolute, ..default() },
    Visibility = Visibility::Hidden, FocusPolicy = FocusPolicy::Pass,
    Pickable = Pickable::IGNORE,
    UiTransform = UiTransform::from_translation(Val2::percent(-50., -50.)))]
pub struct WorldUi;

/// Position in canonical world space. Entity offsets are added after resolving
/// the full hierarchy, so rotating/scaling the actor never rotates the offset.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum WorldUiAnchor {
    World(Vec3),
    Entity { entity: Entity, offset: Vec3 },
}

impl Default for WorldUiAnchor {
    fn default() -> Self {
        Self::World(Vec3::ZERO)
    }
}

pub(super) fn project_world_ui(
    cameras: Query<&Camera>,
    transforms: TransformHelper,
    scale: Res<UiScale>,
    mut roots: Query<
        (
            &WorldUiAnchor,
            Option<&UiTargetCamera>,
            &mut Node,
            &mut Visibility,
        ),
        With<WorldUi>,
    >,
) {
    for (anchor, target, mut node, mut visibility) in &mut roots {
        // An explicit Node supplied by a scene may replace the required default.
        if node.position_type != PositionType::Absolute {
            node.position_type = PositionType::Absolute;
        }
        let position = (|| {
            let camera_entity = target?.0;
            let camera = cameras.get(camera_entity).ok()?;
            let camera_transform = transforms.compute_global_transform(camera_entity).ok()?;
            let world = match *anchor {
                WorldUiAnchor::World(position) => position,
                WorldUiAnchor::Entity { entity, offset } => {
                    transforms
                        .compute_global_transform(entity)
                        .ok()?
                        .translation()
                        + offset
                }
            };
            project(camera, &camera_transform, world, scale.0)
        })();
        visibility.set_if_neq(if position.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if let Some(position) = position {
            if node.left != px(position.x) || node.top != px(position.y) {
                node.left = px(position.x);
                node.top = px(position.y);
            }
        }
    }
}

pub(super) fn project(
    camera: &Camera,
    transform: &GlobalTransform,
    world: Vec3,
    scale: f32,
) -> Option<Vec2> {
    if !camera.is_active || !scale.is_finite() || scale <= 0. {
        return None;
    }
    let viewport = camera.logical_viewport_rect()?;
    let screen = camera.world_to_viewport(transform, world).ok()?;
    if !screen.is_finite() || !viewport.contains(screen) {
        return None;
    }
    // Bevy 0.19 returns target coordinates including the viewport origin. UI
    // roots use viewport-local logical coordinates, further divided by UiScale.
    Some((screen - viewport.min) / scale)
}
