//! The character screen (C): how you are on the left (health, hunger, how warm you feel
//! and the effects on you, also shown in the inventory), every skill on the right with its level and how far to the next.
//! Clicking a skill opens its guide: what it opens up at each level, from the skill's own
//! guide in skills.ron and every crop that needs it, marked as reached or not.

use bevy::prelude::*;
use voxmar_engine::skill::{SkillId, SkillRegistry};

use super::screens::{GameScreen, ScreenSystems, spawn_hotkeys};
use super::skills::xp_text;
use crate::lang::{Locale, Localized};
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
            .add_systems(PostStartup, (spawn, open_start_guide))
            .add_systems(
                Update,
                ((show, pick_skill, draw_skills, draw_guide).chain().after(ScreenSystems), draw_condition).run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnExit(Screen::InGame), |mut guide: ResMut<Guide>| guide.0 = None);
    }
}

/// The skill whose guide is open, if any.
#[derive(Resource, Default)]
struct Guide(Option<SkillId>);

/// A skill guide to open at startup, by the skill's name (`--guide`, for captures).
#[derive(Resource)]
pub struct StartGuide(pub String);

fn open_start_guide(start: Option<Res<StartGuide>>, launch: Res<Launch>, mut guide: ResMut<Guide>) {
    let skills = &launch.content.skills;
    if let Some(id) = start.and_then(|start| skills.ids().find(|&id| skills.get(id).name.ends_with(&start.0) || skills.get(id).title.eq_ignore_ascii_case(&start.0))) {
        guide.0 = Some(id);
    }
}

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

const WIDTH: f32 = 400.0 * PX;
const LEFT: f32 = 150.0 * PX;
const LIST_HEIGHT: f32 = 170.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((CharacterScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            spawn_heading(card, font, "screen.character");
            card.spawn(Node { column_gap: Val::Px(8.0 * PX), ..default() }).with_children(|columns| {
                // How you are.
                let left = Node { width: Val::Px(LEFT), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0 * PX), ..default() };
                columns.spawn(left).with_children(|left| spawn_condition(left, font));
                // Skills, or one skill's guide.
                let right = Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, min_width: Val::ZERO, ..default() };
                columns.spawn(right).with_children(|right| {
                    let list = Node {
                        display: Display::Grid,
                        grid_template_columns: RepeatedGridTrack::flex(2, 1.0),
                        row_gap: Val::Px(3.0 * PX),
                        column_gap: Val::Px(3.0 * PX),
                        align_content: AlignContent::Start,
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
            card.spawn((Localized("character.hint"), text(font, "", BODY, MUTED)));
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
fn standing(locale: &Locale, skills: &SkillRegistry, xp: u32) -> (u8, f32, String) {
    let level = skills.level(xp);
    if level >= skills.max_level() {
        return (level, 1.0, locale.format("character.xp", &[("xp", &xp_text(xp))]));
    }
    let next = skills.xp_for(level + 1);
    (level, skills.progress(xp), locale.format("character.xp_of", &[("xp", &xp_text(xp)), ("next", &xp_text(next))]))
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
    images: Res<super::HudImages>,
    locale: Res<Locale>,
    mut list: Query<(Entity, &mut Node, Option<&Children>), With<SkillList>>,
    mut progress: Query<(&SkillProgress, &mut Node), Without<SkillList>>,
) {
    let Ok((entity, mut node, rows)) = list.single_mut() else { return };
    if locale.is_changed()
        && let Some(rows) = rows
    {
        // Written again in the new language.
        for &row in rows {
            commands.entity(row).despawn();
        }
        return;
    }
    let shown = ui.character_open && guide.0.is_none();
    let display = if shown { Display::Grid } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if !shown {
        return;
    }
    let skills = &launch.content.skills;
    if rows.is_none_or(|rows| rows.is_empty()) {
        // A tile per skill: its icon, name and level, and a thin line of progress.
        commands.entity(entity).with_children(|list| {
            for id in skills.ids() {
                let tile = Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0 * PX),
                    padding: UiRect::new(Val::Px(4.0 * PX), Val::Px(4.0 * PX), Val::Px(3.0 * PX), Val::Px(4.0 * PX)),
                    ..default()
                };
                let button = spawn_button(list, &font.0, "", ButtonStyle::Secondary, SkillRow(id));
                list.commands().entity(button).insert(tile).despawn_children().with_children(|tile| {
                    tile.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0 * PX), ..default() }).with_children(|line| {
                        let icon = images.skills.get(id.0 as usize).cloned().unwrap_or_default();
                        line.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::new(icon)));
                        line.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|words| {
                            words.spawn(shadowed(&font.0, locale.skill(skills.get(id)), BODY, TEXT));
                            words.spawn((text(&font.0, "", BODY, GOLD), SkillLevel(id)));
                        });
                    });
                    let track = Node { height: Val::Px(3.0 * PX), padding: UiRect::all(Val::Px(PX)), ..default() };
                    tile.spawn((track, Piece::Well.image())).with_child((
                        Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                        Piece::Bar.image(),
                        SkillProgress(id),
                    ));
                });
            }
        });
        return;
    }
    for (bar, mut node) in &mut progress {
        let (_, share, _) = standing(&locale, skills, player.skills.xp(bar.0));
        let width = Val::Percent(share * 100.0);
        if node.width != width {
            node.width = width;
        }
    }
}

/// The thin line of progress to a skill's next level.
#[derive(Component)]
struct SkillProgress(SkillId);

/// A skill's level in its row.
#[derive(Component)]
struct SkillLevel(SkillId);

/// Writes each skill row's level ("Lv 12").
fn draw_levels(locale: &Locale, launch: &Launch, player: &LocalPlayer, levels: &mut Query<(&SkillLevel, &mut Text)>) {
    for (level, mut text) in levels.iter_mut() {
        let value = locale.format("character.level", &[("level", &launch.content.skills.level(player.skills.xp(level.0)))]);
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
    locale: Res<Locale>,
    mut page: Query<(Entity, &mut Node), With<GuidePage>>,
    mut levels: Query<(&SkillLevel, &mut Text)>,
    mut drawn: Local<Option<SkillId>>,
) {
    draw_levels(&locale, &launch, &player, &mut levels);
    let Ok((entity, mut node)) = page.single_mut() else { return };
    let display = if guide.0.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if *drawn == guide.0 && !locale.is_changed() {
        return;
    }
    *drawn = guide.0;
    commands.entity(entity).despawn_children();
    let Some(id) = guide.0 else { return };
    let skills = &launch.content.skills;
    let def = skills.get(id);
    let (level, _, progress) = standing(&locale, skills, player.skills.xp(id));
    let steps = guide_steps(&locale, &launch, id);
    let font = &font.0;
    commands.entity(entity).with_children(|page| {
        page.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }).with_children(|top| {
            let level = locale.format("character.level", &[("level", &level)]);
            top.spawn(shadowed(font, format!("{}  {level}", locale.skill(def)), BODY, GOLD));
            top.spawn(Node { width: Val::Px(40.0 * PX), ..default() }).with_children(|cell| {
                spawn_button(cell, font, "character.back", ButtonStyle::Secondary, BackToSkills);
            });
        });
        page.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, locale.skill_description(def), BODY, SOFT)));
        page.spawn((Node { flex_shrink: 0.0, margin: UiRect::bottom(Val::Px(2.0 * PX)), ..default() }, text(font, progress, BODY, MUTED)));
        if steps.is_empty() {
            page.spawn((Node { flex_shrink: 0.0, ..default() }, Localized("character.untrained"), text(font, "", BODY, MUTED)));
        }
        for (at, line) in steps {
            let reached = level >= at;
            let row = Node { column_gap: Val::Px(4.0 * PX), flex_shrink: 0.0, ..default() };
            page.spawn(row).with_children(|row| {
                row.spawn(text(font, if reached { "◆" } else { "◇" }, BODY, if reached { ACCENT } else { MUTED }));
                row.spawn((Node { width: Val::Px(22.0 * PX), flex_shrink: 0.0, ..default() }, text(font, format!("{at}"), BODY, if reached { GOLD } else { MUTED })));
                let fill = Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() };
                row.spawn((fill, text(font, line, BODY, if reached { TEXT } else { MUTED })));
            });
        }
    });
}

/// What a skill opens up, by level: its own guide in skills.ron, and every crop it
/// needs to sow.
fn guide_steps(locale: &Locale, launch: &Launch, skill: SkillId) -> Vec<(u8, String)> {
    let content = &launch.content;
    let def = content.skills.get(skill);
    let mut steps: Vec<(u8, String)> = def.guide.iter().enumerate().map(|(i, step)| (step.level, locale.skill_guide(def, i))).collect();
    for (_, crop) in content.crops.iter().filter(|(_, crop)| crop.skill == skill && crop.def.sown) {
        steps.push((crop.def.level, locale.format("character.sow", &[("crop", &locale.block_named(&content.blocks, &crop.def.name))])));
    }
    steps.sort_by_key(|(level, _)| *level);
    steps
}

/// How you are, as the character and inventory screens show it: health and hunger
/// meters, how warm you feel, and every effect on you with what it does.
#[derive(Component)]
pub struct Condition;
/// The list of effects in a condition panel.
#[derive(Component)]
struct EffectList;
#[derive(Component)]
struct HungerMeter;
#[derive(Component)]
struct FeltLine;

/// Room for a meter's label: six letters of the monospace fonts.
const LABEL: f32 = 38.0 * PX;

/// A condition panel, filling its parent's width.
pub fn spawn_condition(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    let column = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0 * PX), width: Val::Percent(100.0), ..default() };
    parent.spawn((Condition, column)).with_children(|panel| {
        let row = || Node { column_gap: Val::Px(3.0 * PX), align_items: AlignItems::Center, ..default() };
        panel.spawn(row()).with_children(|row| {
            row.spawn((Node { width: Val::Px(LABEL), flex_shrink: 0.0, ..default() }, Localized("character.health"), text(font, "", BODY, MUTED)));
            spawn_meter(row, font, Piece::BarHealth, Val::Percent(100.0), HealthMeter);
        });
        panel.spawn(row()).with_children(|row| {
            row.spawn((Node { width: Val::Px(LABEL), flex_shrink: 0.0, ..default() }, Localized("character.hunger"), text(font, "", BODY, MUTED)));
            spawn_meter(row, font, Piece::BarHunger, Val::Percent(100.0), HungerMeter);
        });
        panel.spawn((FeltLine, text(font, "", BODY, SOFT)));
        panel.spawn((EffectList, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(2.0 * PX), ..default() }));
    });
}

/// Keeps every condition panel current.
#[allow(clippy::too_many_arguments)]
fn draw_condition(
    mut commands: Commands,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    health: Query<&Meter, With<HealthMeter>>,
    hunger: Query<&Meter, (With<HungerMeter>, Without<HealthMeter>)>,
    mut felt: Query<&mut Text, With<FeltLine>>,
    lists: Query<(Entity, Ref<EffectList>)>,
    mut nodes: Query<&mut Node>,
    children: Query<&Children>,
    mut texts: Query<&mut Text, Without<FeltLine>>,
    mut shown: Local<Option<Vec<voxmar_engine::vitals::Effect>>>,
    locale: Res<Locale>,
) {
    for meter in &health {
        set_meter(meter, player.health as f32 / 20.0, &format!("{}/20", player.health), &mut nodes, &children, &mut texts);
    }
    let max = voxmar_engine::vitals::MAX_HUNGER;
    for meter in &hunger {
        let value = player.vitals.hunger;
        set_meter(meter, value as f32 / max as f32, &format!("{value}/{max}"), &mut nodes, &children, &mut texts);
    }
    let line = locale.format("character.feels_like", &[("temperature", &format!("{:.0}", player.vitals.felt))]);
    for mut text in &mut felt {
        if text.0 != line {
            text.0.clone_from(&line);
        }
    }
    let effects = &player.vitals.effects;
    let fresh = lists.iter().any(|(_, list)| list.is_added());
    if !fresh && !locale.is_changed() && shown.as_ref() == Some(effects) {
        return;
    }
    *shown = Some(effects.clone());
    for (entity, _) in &lists {
        commands.entity(entity).despawn_children().with_children(|list| {
            if effects.is_empty() {
                list.spawn((Localized("character.no_effects"), text(&font.0, "", BODY, MUTED)));
            }
            for &effect in effects {
                list.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|row| {
                    let color = if effect.is_buff() { ACCENT } else { crate::ui::widgets::ERROR };
                    row.spawn(shadowed(&font.0, format!("{} {}", if effect.is_buff() { "▴" } else { "▾" }, locale.effect(effect)), BODY, color));
                    row.spawn(text(&font.0, locale.effect_description(effect), BODY, MUTED));
                });
            }
        });
    }
}
