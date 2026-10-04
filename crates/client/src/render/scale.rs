//! Render scale: the world drawn at fewer pixels than the window has, then stretched over
//! it, while menus, text and the HUD stay sharp at the window's full resolution. Most of a
//! frame's GPU time goes on pixels (shading the terrain, the light shafts, anti-aliasing,
//! the display transform), and a Retina screen has four times the pixels of its size in
//! points, so drawing at three quarters of the width and height costs a little over half.
//!
//! Below full scale the world's camera draws into an image of the scaled size, and a
//! second camera on the window shows that image under the interface, which follows that
//! camera as the highest on the window (Bevy's default for UI). At full scale the world's
//! camera draws to the window itself and the second camera is gone, so it costs nothing.
//! Runs without a window (captures) are left alone.

use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::ui::FocusPolicy;
use bevy::window::{PrimaryWindow, WindowRef};

/// The share of the window's width and height the world is drawn at, 0..=1.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub struct RenderScale(pub f32);

impl Default for RenderScale {
    fn default() -> Self {
        Self(1.0)
    }
}

pub struct ScalePlugin;

impl Plugin for ScalePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderScale>().add_systems(Update, follow_window);
    }
}

/// The camera that shows the scaled world on the window.
#[derive(Component)]
struct ScreenCamera;

/// The interface node, under everything else, that shows the scaled world.
#[derive(Component)]
struct Backdrop;

/// The screen camera and its backdrop, which come and go together.
type ScreenParts = Or<(With<ScreenCamera>, With<Backdrop>)>;

/// Keeps the world's image the scaled size of the window, or the world on the window
/// itself at full scale. `drawn` is the size of the image the world is drawn into, if any.
#[allow(clippy::too_many_arguments)]
fn follow_window(
    mut commands: Commands,
    scale: Res<RenderScale>,
    mut images: ResMut<Assets<Image>>,
    window: Query<&Window, With<PrimaryWindow>>,
    world_camera: Query<Entity, With<Camera3d>>,
    screen: Query<Entity, ScreenParts>,
    mut backdrop: Query<&mut ImageNode, With<Backdrop>>,
    mut drawn: Local<Option<UVec2>>,
) {
    let (Ok(window), Ok(camera)) = (window.single(), world_camera.single()) else { return };
    let size = window.physical_size().max(UVec2::ONE);
    let wanted = (scale.0 < 0.999).then(|| (size.as_vec2() * scale.0.max(0.25)).round().as_uvec2().max(UVec2::ONE));
    if wanted == *drawn {
        return;
    }
    *drawn = wanted;
    let Some(wanted) = wanted else {
        commands.entity(camera).insert(RenderTarget::Window(WindowRef::Primary));
        for entity in &screen {
            commands.entity(entity).despawn();
        }
        return;
    };
    let mut image = Image::new_target_texture(wanted.x, wanted.y, TextureFormat::Rgba8UnormSrgb, None);
    // Smoothly stretched: nearest neighbour would double some texels and not others.
    image.sampler = ImageSampler::linear();
    let image = images.add(image);
    commands.entity(camera).insert(RenderTarget::Image(image.clone().into()));
    if let Ok(mut node) = backdrop.single_mut() {
        node.image = image;
        return;
    }
    let screen = commands
        .spawn((
            ScreenCamera,
            Camera2d,
            Camera { order: 1, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
            Msaa::Off,
        ))
        .id();
    commands.spawn((
        Backdrop,
        ImageNode::new(image),
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        GlobalZIndex(i32::MIN),
        FocusPolicy::Pass,
        UiTargetCamera(screen),
    ));
}
