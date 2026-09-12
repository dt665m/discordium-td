use super::{billboard::project, *};
use bevy::camera::{CameraProjection, RenderTargetInfo, Viewport};

#[test]
fn scene_size_does_not_override_billboard_layout() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::asset::AssetPlugin::default(),
        bevy::scene::ScenePlugin,
        EngineUiPlugin,
    ))
    .add_systems(Startup, |mut commands: Commands| {
        let value = "label".to_string();
        let anchor = WorldUiAnchor::World(Vec3::ZERO);
        commands.spawn_scene(bsn! {
            WorldUi
            template_value(anchor)
            Node { width: px(80), height: px(12) }
            Children [(Text(value))]
        });
    });
    app.update();
    let mut roots = app
        .world_mut()
        .query_filtered::<(&Node, &UiTransform, &Visibility, &Pickable), With<WorldUi>>();
    let (node, transform, visibility, pickable) = roots.single(app.world()).unwrap();
    assert_eq!(node.position_type, PositionType::Absolute);
    assert_eq!(node.width, px(80));
    assert_eq!(transform.translation, Val2::percent(-50., -50.));
    assert_eq!(*visibility, Visibility::Hidden);
    assert!(!pickable.should_block_lower);
}

fn camera() -> Camera {
    let mut projection = PerspectiveProjection::default();
    projection.update(800., 600.);
    let mut camera = Camera {
        viewport: Some(Viewport {
            physical_position: UVec2::new(100, 80),
            physical_size: UVec2::new(800, 600),
            ..default()
        }),
        ..default()
    };
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(1200, 900),
        scale_factor: 2.,
    });
    camera.computed.clip_from_view = projection.get_clip_from_view();
    camera
}

#[test]
fn projection_accounts_for_viewport_origin_dpi_and_ui_scale() {
    let camera = camera();
    let point = project(
        &camera,
        &GlobalTransform::IDENTITY,
        Vec3::new(0., 0., -5.),
        2.,
    )
    .unwrap();
    assert!(point.abs_diff_eq(Vec2::new(100., 75.), 0.001));
    assert!(project(&camera, &GlobalTransform::IDENTITY, Vec3::Z, 1.).is_none());
    assert!(
        project(
            &camera,
            &GlobalTransform::IDENTITY,
            Vec3::new(100., 0., -5.),
            1.
        )
        .is_none()
    );
    let mut inactive = camera;
    inactive.is_active = false;
    assert!(project(&inactive, &GlobalTransform::IDENTITY, -Vec3::Z, 1.).is_none());
}

#[test]
fn anchors_use_fresh_parent_transforms_and_world_space_offsets() {
    let mut app = App::new();
    app.add_plugins(EngineUiPlugin);
    let parent = app
        .world_mut()
        .spawn(Transform::from_rotation(Quat::from_rotation_y(0.7)))
        .id();
    let camera = app
        .world_mut()
        .spawn((camera(), Transform::default(), ChildOf(parent)))
        .id();
    let actor_parent = app.world_mut().spawn(Transform::from_xyz(0., 0., -6.)).id();
    let actor = app
        .world_mut()
        .spawn((
            Transform::from_rotation(Quat::from_rotation_z(1.)),
            ChildOf(actor_parent),
        ))
        .id();
    let root = app
        .world_mut()
        .spawn((
            WorldUi,
            WorldUiAnchor::Entity {
                entity: actor,
                offset: Vec3::Y,
            },
            UiTargetCamera(camera),
        ))
        .id();
    // Change both hierarchies without propagating GlobalTransform.
    app.world_mut()
        .get_mut::<Transform>(parent)
        .unwrap()
        .rotation = Quat::from_rotation_y(0.1);
    app.world_mut()
        .get_mut::<Transform>(actor_parent)
        .unwrap()
        .translation
        .x = 0.5;
    app.update();
    let expected = project(
        app.world().get::<Camera>(camera).unwrap(),
        &GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(0.1))),
        Vec3::new(0.5, 1., -6.),
        1.,
    )
    .unwrap();
    let node = app.world().get::<Node>(root).unwrap();
    assert_eq!(node.left, px(expected.x));
    assert_eq!(node.top, px(expected.y));
    assert_eq!(
        *app.world().get::<Visibility>(root).unwrap(),
        Visibility::Inherited
    );
    app.world_mut().despawn(actor_parent);
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(root).unwrap(),
        Visibility::Hidden
    );
}

#[test]
fn owner_cleanup_and_meter_fill_boundaries() {
    let mut app = App::new();
    app.add_plugins(EngineUiPlugin);
    let owner = app.world_mut().spawn_empty().id();
    let root = app.world_mut().spawn((WorldUi, WorldUiOwner(owner))).id();
    let fill = app
        .world_mut()
        .spawn((MeterFraction(0.), Node::default()))
        .id();
    for (fraction, percentage) in [
        (-1., 0.),
        (0., 0.),
        (0.5, 50.),
        (1., 100.),
        (2., 100.),
        (f32::NAN, 0.),
    ] {
        app.world_mut().get_mut::<MeterFraction>(fill).unwrap().0 = fraction;
        app.update();
        assert_eq!(
            app.world().get::<Node>(fill).unwrap().width,
            percent(percentage)
        );
    }
    app.world_mut().despawn(owner);
    assert!(app.world().get_entity(root).is_err());
}
