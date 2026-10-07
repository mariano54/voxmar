//! Each creature near enough to fight shows its combat level over it, coloured against
//! ours as the target readout colours it: green for weaker, gold for even, red for stronger.

use bevy::prelude::*;
use rustc_hash::FxHashMap;

use crate::creatures::Creatures;
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::widgets::{BODY, UiFont, shadowed};

pub struct LevelsPlugin;

impl Plugin for LevelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, label.after(TransformSystems::Propagate).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), forget);
    }
}

/// How near, in blocks, a creature shows its level.
const NEAR: f32 = 16.0;
/// How far over its head the level sits, in blocks.
const ABOVE: f32 = 0.55;
/// Room a label takes, for keeping labels apart: "Lv 50" and its shadow.
const LABEL_WIDTH: f32 = 30.0 * crate::ui::skin::PX;
const LABEL_HEIGHT: f32 = 10.0 * crate::ui::skin::PX;

/// A creature's level over it, by the creature's id.
#[derive(Component)]
pub struct LevelLabel(pub u32);

fn forget(mut commands: Commands, labels: Query<Entity, With<LevelLabel>>) {
    for label in &labels {
        commands.entity(label).despawn();
    }
}

#[allow(clippy::too_many_arguments)]
fn label(
    mut commands: Commands,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    creatures: Res<Creatures>,
    scale: Res<UiScale>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut labels: Query<(Entity, &LevelLabel, &mut Node, &mut Text, &mut TextColor, &mut Visibility)>,
) {
    let Ok((camera, eye)) = camera.single() else { return };
    // Out of the way of a screen.
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open || ui.recipes_open || player.dead;
    let ours = player.skills_known.then(|| launch.content.skills.combat_level(&player.skills));
    let species = &launch.content.creatures;
    let mut near: Vec<(u32, Vec2, u8, f32)> = Vec::new();
    if !screen {
        for (id, kind, feet) in creatures.living() {
            let def = &species.get(kind).def;
            let head = feet + Vec3::Y * (def.body.size.1 + ABOVE);
            let distance = head.distance(eye.translation());
            if distance > NEAR {
                continue;
            }
            // In the window's points; the interface is laid out at its own scale.
            if let Ok(at) = camera.world_to_viewport(eye, head) {
                near.push((id, at / scale.0, def.level, distance));
            }
        }
    }
    // Nearer creatures' labels where they are; a farther one that would cover another
    // moves up above it.
    near.sort_by(|a, b| a.3.total_cmp(&b.3));
    let size = Vec2::new(LABEL_WIDTH, LABEL_HEIGHT);
    let mut taken: Vec<Rect> = Vec::new();
    let mut wanted: FxHashMap<u32, (Vec2, u8)> = FxHashMap::default();
    for (id, mut at, level, _) in near {
        for _ in 0..4 {
            let rect = Rect::from_center_size(at - Vec2::Y * size.y / 2.0, size);
            if !taken.iter().any(|other| !other.intersect(rect).is_empty()) {
                break;
            }
            at.y -= size.y;
        }
        taken.push(Rect::from_center_size(at - Vec2::Y * size.y / 2.0, size));
        wanted.insert(id, (at, level));
    }
    let mut placed: Vec<u32> = Vec::new();
    for (entity, label, mut node, mut text, mut color, mut visibility) in &mut labels {
        let Some(&(at, level)) = wanted.get(&label.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        placed.push(label.0);
        let words = locale.format("character.level", &[("level", &level)]);
        if text.0 != words {
            text.0 = words;
        }
        color.set_if_neq(TextColor(super::target::level_colour(level, ours)));
        // Centred over the head: half its own width to the left.
        node.left = Val::Px(at.x);
        node.top = Val::Px(at.y);
        visibility.set_if_neq(Visibility::Inherited);
    }
    for (id, (at, level)) in wanted {
        if placed.contains(&id) {
            continue;
        }
        let node = Node { position_type: PositionType::Absolute, left: Val::Px(at.x), top: Val::Px(at.y), ..default() };
        let words = locale.format("character.level", &[("level", &level)]);
        let colour = super::target::level_colour(level, ours);
        commands.spawn((node, shadowed(&font.0, words, BODY, colour), TightLine, LevelLabel(id), UiTransform::from_translation(Val2::percent(-50.0, -100.0)), GlobalZIndex(0)));
    }
}
