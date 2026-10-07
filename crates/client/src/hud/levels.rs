//! The creature under the crosshair, out to `SIGHT` blocks, shows its combat level over its
//! head, coloured against ours as the target readout colours it (green for weaker, gold for
//! even, red for stronger), with a bar of its health beneath. Only that one, and only in
//! plain sight: a solid block in the way hides it.

use bevy::prelude::*;

use crate::creatures::Creatures;
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{BODY, UiFont, shadowed, space};

pub struct LevelsPlugin;

impl Plugin for LevelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn).add_systems(PostUpdate, label.after(TransformSystems::Propagate).run_if(in_state(Screen::InGame)));
    }
}

/// How far off a creature pointed at shows its level, in blocks.
const SIGHT: f32 = 16.0;
/// How far over its head the level sits, in blocks.
const ABOVE: f32 = 0.45;
/// The health bar's width and height.
const BAR: Vec2 = Vec2::new(22.0 * PX, 4.0 * PX);

/// The level and health over the creature pointed at.
#[derive(Component)]
pub struct LevelLabel;
#[derive(Component)]
struct LevelText;
#[derive(Component)]
struct LevelHealth;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let plate = Node {
        position_type: PositionType::Absolute,
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: Val::Px(space::HAIR),
        ..default()
    };
    // Centred over the head, its foot at the point projected.
    let centred = UiTransform::from_translation(Val2::percent(-50.0, -100.0));
    commands.spawn((plate, centred, LevelLabel, Visibility::Hidden, GlobalZIndex(0))).with_children(|plate| {
        plate.spawn((shadowed(&font.0, "", BODY, Color::WHITE), TightLine, LevelText));
        let track = Node { width: Val::Px(BAR.x), height: Val::Px(BAR.y), padding: UiRect::all(Val::Px(PX)), ..default() };
        plate.spawn((track, Piece::Well.image())).with_child((Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, Piece::BarHealth.image(), LevelHealth));
    });
}

#[allow(clippy::too_many_arguments)]
fn label(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    creatures: Res<Creatures>,
    world: Res<crate::world::ClientWorld>,
    scale: Res<UiScale>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut plate: Query<(&mut Node, &mut Visibility), With<LevelLabel>>,
    mut words: Query<(&mut Text, &mut TextColor), With<LevelText>>,
    mut health: Query<&mut Node, (With<LevelHealth>, Without<LevelLabel>)>,
) {
    let Ok((mut node, mut visibility)) = plate.single_mut() else { return };
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open || ui.recipes_open || player.dead;
    // The nearest creature along the crosshair, before any solid block (grass is seen through).
    let (eye_at, look) = (player.eye(), player.look_direction().normalize_or_zero());
    let block = voxmar_engine::physics::raycast(eye_at, look, SIGHT, |p| world.block(p).is_some_and(|b| world.registry.info(b).solid)).map_or(f32::MAX, |hit| hit.distance);
    let pointed = creatures.pointed(&launch, eye_at, look, SIGHT, block).filter(|_| !screen).and_then(|id| {
        let (_, kind, feet) = creatures.living().find(|(living, ..)| *living == id)?;
        Some((kind, feet, creatures.sighting(id)?.health))
    });
    let Some((kind, feet, now)) = pointed else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let Ok((camera, eye)) = camera.single() else { return };
    let def = &launch.content.creatures.get(kind).def;
    let head = feet + Vec3::Y * (def.body.size.1 + ABOVE);
    // In the window's points; the interface is laid out at its own scale.
    let Ok(at) = camera.world_to_viewport(eye, head).map(|at| at / scale.0) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    node.left = Val::Px(at.x);
    node.top = Val::Px(at.y);
    let ours = player.skills_known.then(|| launch.content.skills.combat_level(&player.skills));
    if let Ok((mut text, mut color)) = words.single_mut() {
        let level = locale.format("character.level_short", &[("level", &def.level)]);
        if text.0 != level {
            text.0 = level;
        }
        color.set_if_neq(TextColor(super::target::level_colour(def.level, ours)));
    }
    if let Ok(mut fill) = health.single_mut() {
        let share = Val::Percent(now as f32 / def.health.max(1) as f32 * 100.0);
        if fill.width != share {
            fill.width = share;
        }
    }
    visibility.set_if_neq(Visibility::Inherited);
}
