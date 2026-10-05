//! The character screen (C): how you are on the left (health, hunger, warmth and the
//! effects on you), every skill on the right with its level and how far to the next.
//! Clicking a skill opens its guide: what it opens up at each level, from the skill's own
//! guide in skills.ron and every crop that needs it, marked as reached or not.

use bevy::prelude::*;
use voxmar_engine::skill::{SkillId, SkillRegistry};

use super::screens::{GameScreen, ScreenSystems, spawn_hotkeys};
use super::skills::xp_text;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, GOLD, MUTED, Meter, SCRIM, SOFT, ScrollList, TEXT, UiFont, panel, screen, set_meter, shadowed,
    spawn_button, spawn_heading, spawn_meter, spawn_rule, text,
};

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Guide>()
            .add_systems(PostStartup, spawn)
            .add_systems(
                Update,
                (show, pick_skill, draw_skills, draw_guide, draw_condition).chain().after(ScreenSystems).run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnExit(Screen::InGame), |mut guide: ResMut<Guide>| guide.0 = None);
    }
}

/// The skill whose guide is open, if any.
#[derive(Resource, Default)]
struct Guide(Option<SkillId>);

#[derive(Component)]
struct CharacterScreen;
/// The list of skills, shown while no guide is open.
#[derive(Component)]
struct SkillList;
/// A skill's row in the list.
#[derive(Component)]
struct SkillRow(SkillId);
/// The guide, shown in place of the list.
#[derive(Component)]
struct GuidePage;
#[derive(Component)]
struct BackToSkills;
#[derive(Component)]
struct HealthMeter;
#[derive(Component)]
struct ConditionLines;

const WIDTH: f32 = 400.0 * PX;
const LEFT: f32 = 130.0 * PX;
const LIST_HEIGHT: f32 = 170.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((CharacterScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            spawn_heading(card, font, "Character");
            card.spawn(Node { column_gap: Val::Px(8.0 * PX), ..default() }).with_children(|columns| {
                // How you are.
                let left = Node { width: Val::Px(LEFT), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0 * PX), ..default() };
                columns.spawn(left).with_children(|left| {
                    left.spawn(text(font, "Health", BODY, MUTED));
                    spawn_meter(left, font, Piece::BarHealth, Val::Percent(100.0), HealthMeter);
                    left.spawn((Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0 * PX), margin: UiRect::top(Val::Px(3.0 * PX)), ..default() }, ConditionLines));
                });
                // Skills, or one skill's guide.
                let right = Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, min_width: Val::ZERO, ..default() };
                columns.spawn(right).with_children(|right| {
                    let list = Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0 * PX),
                        height: Val::Px(LIST_HEIGHT),
                        overflow: Overflow::scroll_y(),
                        ..default()
                    };
                    right.spawn((SkillList, ScrollList, list));
                    let page = Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0 * PX),
                        height: Val::Px(LIST_HEIGHT),
                        overflow: Overflow::scroll_y(),
                        display: Display::None,
                        ..default()
                    };
                    right.spawn((GuidePage, ScrollList, page));
                });
            });
            spawn_rule(card, 4.0 * PX);
            card.spawn(text(font, "Click a skill for its guide: what each level opens up.", BODY, MUTED));
        });
        spawn_hotkeys(root, GameScreen::Character);
    });
}

fn show(ui: Res<UiState>, mut root: Query<&mut Visibility, With<CharacterScreen>>, mut guide: ResMut<Guide>, mut was_open: Local<bool>) {
    if *was_open && !ui.character_open {
        guide.0 = None;
    }
    *was_open = ui.character_open;
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.character_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn pick_skill(
    rows: Query<(&SkillRow, &Interaction), Changed<Interaction>>,
    back: Query<&Interaction, (Changed<Interaction>, With<BackToSkills>)>,
    mut guide: ResMut<Guide>,
) {
    for (row, interaction) in &rows {
        if *interaction == Interaction::Pressed {
            guide.0 = Some(row.0);
        }
    }
    if back.iter().any(|i| *i == Interaction::Pressed) {
        guide.0 = None;
    }
}

/// A skill's level and the share of the way to the next, and the text over its meter.
fn standing(skills: &SkillRegistry, xp: u32) -> (u8, f32, String) {
    let level = skills.level(xp);
    if level >= skills.max_level() {
        return (level, 1.0, format!("{} xp", xp_text(xp)));
    }
    let next = skills.xp_for(level + 1);
    (level, skills.progress(xp), format!("{} / {} xp", xp_text(xp), xp_text(next)))
}

/// Fills the skill list when the screen opens or the guide closes, and keeps the levels
/// current while it shows.
#[allow(clippy::too_many_arguments)]
fn draw_skills(
    mut commands: Commands,
    ui: Res<UiState>,
    guide: Res<Guide>,
    launch: Res<Launch>,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    mut list: Query<(Entity, &mut Node, Option<&Children>), With<SkillList>>,
    meters: Query<(&SkillRow, &Meter)>,
    mut nodes: Query<&mut Node, Without<SkillList>>,
    children: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    let Ok((entity, mut node, rows)) = list.single_mut() else { return };
    let shown = ui.character_open && guide.0.is_none();
    let display = if shown { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if !shown {
        return;
    }
    let skills = &launch.content.skills;
    if rows.is_none_or(|rows| rows.is_empty()) {
        commands.entity(entity).with_children(|list| {
            for id in skills.ids() {
                let row = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(PX), padding: UiRect::axes(Val::Px(5.0 * PX), Val::Px(4.0 * PX)), flex_shrink: 0.0, ..default() };
                let button = spawn_button(list, &font.0, "", ButtonStyle::Secondary, SkillRow(id));
                list.commands().entity(button).insert(row).despawn_children().with_children(|row| {
                    row.spawn(Node { justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|line| {
                        line.spawn(shadowed(&font.0, skills.get(id).title.clone(), BODY, TEXT));
                        line.spawn((text(&font.0, "", BODY, GOLD), SkillLevel(id)));
                    });
                    spawn_meter(row, &font.0, Piece::Bar, Val::Percent(100.0), SkillRow(id));
                });
            }
        });
        return;
    }
    for (row, meter) in &meters {
        let (_, share, label) = standing(skills, player.skills.xp(row.0));
        set_meter(meter, share, &label, &mut nodes, &children, &mut texts);
    }
}

/// A skill's level in its row.
#[derive(Component)]
struct SkillLevel(SkillId);

/// Writes each skill row's level ("Lv 12").
fn draw_levels(launch: &Launch, player: &LocalPlayer, levels: &mut Query<(&SkillLevel, &mut Text)>) {
    for (level, mut text) in levels.iter_mut() {
        let value = format!("Lv {}", launch.content.skills.level(player.skills.xp(level.0)));
        if text.0 != value {
            text.0 = value;
        }
    }
}

/// Fills the guide when one opens.
#[allow(clippy::too_many_arguments)]
fn draw_guide(
    mut commands: Commands,
    guide: Res<Guide>,
    launch: Res<Launch>,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    mut page: Query<(Entity, &mut Node), With<GuidePage>>,
    mut levels: Query<(&SkillLevel, &mut Text)>,
    mut drawn: Local<Option<SkillId>>,
) {
    draw_levels(&launch, &player, &mut levels);
    let Ok((entity, mut node)) = page.single_mut() else { return };
    let display = if guide.0.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if *drawn == guide.0 {
        return;
    }
    *drawn = guide.0;
    commands.entity(entity).despawn_children();
    let Some(id) = guide.0 else { return };
    let skills = &launch.content.skills;
    let def = skills.get(id);
    let (level, _, progress) = standing(skills, player.skills.xp(id));
    let steps = guide_steps(&launch, id);
    let font = &font.0;
    commands.entity(entity).with_children(|page| {
        page.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }).with_children(|top| {
            top.spawn(shadowed(font, format!("{}  Lv {level}", def.title), BODY, GOLD));
            top.spawn(Node { width: Val::Px(40.0 * PX), ..default() }).with_children(|cell| {
                spawn_button(cell, font, "◂ Back", ButtonStyle::Secondary, BackToSkills);
            });
        });
        page.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, def.description.clone(), BODY, SOFT)));
        page.spawn((Node { flex_shrink: 0.0, margin: UiRect::bottom(Val::Px(2.0 * PX)), ..default() }, text(font, progress, BODY, MUTED)));
        if steps.is_empty() {
            page.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, "Nothing trains this yet. It comes with later updates.", BODY, MUTED)));
        }
        for (at, line) in steps {
            let reached = level >= at;
            let row = Node { column_gap: Val::Px(4.0 * PX), flex_shrink: 0.0, ..default() };
            page.spawn(row).with_children(|row| {
                row.spawn(text(font, if reached { "◆" } else { "◇" }, BODY, if reached { ACCENT } else { MUTED }));
                row.spawn((Node { width: Val::Px(22.0 * PX), flex_shrink: 0.0, ..default() }, text(font, format!("{at}"), BODY, if reached { GOLD } else { MUTED })));
                row.spawn((Node { min_width: Val::ZERO, ..default() }, text(font, line, BODY, if reached { TEXT } else { MUTED })));
            });
        }
    });
}

/// What a skill opens up, by level: its own guide in skills.ron, and every crop it
/// needs to sow.
fn guide_steps(launch: &Launch, skill: SkillId) -> Vec<(u8, String)> {
    let content = &launch.content;
    let mut steps: Vec<(u8, String)> = content.skills.get(skill).guide.iter().map(|step| (step.level, step.text.clone())).collect();
    for (_, crop) in content.crops.iter().filter(|(_, crop)| crop.skill == skill && crop.def.sown) {
        let name = super::inventory::item_name(&crop.def.name);
        steps.push((crop.def.level, format!("Sow and harvest {name}")));
    }
    steps.sort_by_key(|(level, _)| *level);
    steps
}

/// Health, and the lines on how you are.
#[allow(clippy::too_many_arguments)]
fn draw_condition(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    health: Query<&Meter, With<HealthMeter>>,
    mut nodes: Query<&mut Node>,
    children: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    if !ui.character_open {
        return;
    }
    if let Ok(meter) = health.single() {
        let max = 20.0;
        set_meter(meter, player.health as f32 / max, &format!("{}/{}", player.health, max as u8), &mut nodes, &children, &mut texts);
    }
}
