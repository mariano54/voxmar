//! The character screen (C): how you are on the left (health, how warm you feel
//! and the effects on you, also shown in the inventory), every skill on the right with its level and how far to the next.
//! Clicking a skill opens its guide: what it opens up at each level, from the skill's own
//! guide in skills.ron and every crop that needs it, marked as reached or not.

use bevy::prelude::*;
use voxmar_engine::block::BlockId;
use voxmar_engine::skill::{SkillId, SkillRegistry};

use super::screens::{GameScreen, ScreenSystems, mark_screen};
use super::skills::xp_text;
use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, GOLD, MUTED, Meter, SCRIM, SOFT, ScrollList, TEXT, UiFont, panel, screen, set_meter, shadowed,
    space, spawn_button, spawn_heading, spawn_meter, spawn_rule, spawn_scroll_more, text,
};

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Guide>()
            .init_resource::<GuideView>()
            .add_systems(PostStartup, (spawn, open_start_guide))
            .add_systems(
                Update,
                ((show, pick_skill, draw_skills, draw_guide).chain().after(ScreenSystems), draw_condition, draw_combat).run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnExit(Screen::InGame), |mut guide: ResMut<Guide>, mut view: ResMut<GuideView>| {
                guide.0 = None;
                *view = GuideView::default();
            });
    }
}

/// The skill whose guide is open, if any.
#[derive(Resource, Default)]
pub struct Guide(pub Option<SkillId>);

/// For the gate: how many skills there are, how many tiles the screen has for them, and
/// how many of those lie wholly on screen.
pub fn skill_tiles(world: &mut World) -> (usize, usize, usize) {
    let skills = world.resource::<Launch>().content.skills.ids().count();
    let screen = world.query_filtered::<&ComputedNode, With<CharacterScreen>>().iter(world).next().map(ComputedNode::size);
    let mut tiles = world.query::<(&SkillRow, &ComputedNode, &bevy::ui::UiGlobalTransform, &InheritedVisibility)>();
    let (mut shown, mut inside) = (0, 0);
    for (_, node, place, visible) in tiles.iter(world) {
        if !visible.get() {
            continue;
        }
        shown += 1;
        let (low, high) = (place.translation - node.size() / 2.0, place.translation + node.size() / 2.0);
        if screen.is_some_and(|screen| low.min_element() >= 0.0 && high.x <= screen.x && high.y <= screen.y) {
            inside += 1;
        }
    }
    (skills, shown, inside)
}

/// Within an open guide: the kind of line shown (all when none), and the line whose page
/// is open, by its place in the guide.
#[derive(Resource, Default, Clone, PartialEq, Eq, Debug)]
pub struct GuideView {
    pub category: Option<String>,
    pub entry: Option<usize>,
}

/// A button choosing which kind of guide line shows (all of them for none).
#[derive(Component)]
struct CategoryButton(Option<String>);
/// A guide line, which opens its page.
#[derive(Component)]
struct EntryButton(usize);

/// How many guide lines and kinds of line the open guide shows (for the gate).
pub fn guide_buttons(world: &mut World) -> (usize, usize) {
    let lines = world.query_filtered::<(), With<EntryButton>>().iter(world).count();
    let kinds = world.query_filtered::<(), With<CategoryButton>>().iter(world).count();
    (lines, kinds)
}

/// A skill guide to open at startup, by the skill's name (`--guide`, for captures), with
/// the kind of line and the line's page to show.
#[derive(Resource)]
pub struct StartGuide(pub String);

/// `--guide SKILL[/KIND[/LINE]]`: the skill, then the kind of line shown and the line whose
/// page is open, by its place in the guide (`farming/planting/2`).
fn open_start_guide(start: Option<Res<StartGuide>>, launch: Res<Launch>, mut guide: ResMut<Guide>, mut view: ResMut<GuideView>) {
    let skills = &launch.content.skills;
    let Some(start) = start else { return };
    let mut parts = start.0.split('/');
    let skill = parts.next().unwrap_or_default();
    if let Some(id) = skills.ids().find(|&id| skills.get(id).name.ends_with(skill) || skills.get(id).title.eq_ignore_ascii_case(skill)) {
        guide.0 = Some(id);
        view.category = parts.next().filter(|kind| !kind.is_empty() && *kind != "all").map(str::to_owned);
        view.entry = parts.next().and_then(|line| line.parse().ok());
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

const WIDTH: f32 = 540.0 * PX;
const LEFT: f32 = 160.0 * PX;
/// Skill tiles across: three, so fifteen skills stand in five rows with no scrolling.
const COLUMNS: u16 = 3;
/// The line at the foot of the panel, saying what can be done next.
#[derive(Component)]
struct CharacterHint;
/// The player's combat level, at the foot of the condition section.
#[derive(Component)]
struct CombatLine;
/// The condition section, put away while a guide takes the whole panel.
#[derive(Component)]
struct ConditionColumn;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((CharacterScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            spawn_heading(card, font, "screen.character");
            card.spawn(Node { column_gap: Val::Px(space::SECTION), ..default() }).with_children(|columns| {
                // How you are, in a recessed section of its own.
                let left = Node {
                    width: Val::Px(LEFT),
                    flex_shrink: 0.0,
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(space::INNER),
                    padding: UiRect::all(Val::Px(space::GAP)),
                    ..default()
                };
                columns.spawn((left, Piece::Well.image(), ConditionColumn)).with_children(|left| {
                    spawn_condition(left, font);
                    // Pushed to the bottom of the section.
                    left.spawn((Node { margin: UiRect::top(Val::Auto), ..default() }, CombatLine, shadowed(font, "", BODY, GOLD)));
                    super::prayers::spawn_open_button(left, font);
                });
                // Skills, or one skill's guide laid over them: the tiles set the height, so
                // the panel keeps its size whichever shows, in either language.
                let right = Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, min_width: Val::ZERO, ..default() };
                columns.spawn(right).with_children(|right| {
                    let list = Node {
                        display: Display::Grid,
                        grid_template_columns: RepeatedGridTrack::flex(COLUMNS, 1.0),
                        row_gap: Val::Px(space::GAP),
                        column_gap: Val::Px(space::GAP),
                        align_content: AlignContent::Start,
                        ..default()
                    };
                    right.spawn((SkillList, list));
                    let page = Node {
                        position_type: PositionType::Absolute,
                        top: Val::ZERO,
                        bottom: Val::ZERO,
                        left: Val::ZERO,
                        right: Val::ZERO,
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(space::INNER),
                        padding: UiRect::all(Val::Px(space::GAP)),
                        overflow: Overflow::scroll_y(),
                        display: Display::None,
                        ..default()
                    };
                    let page = right.spawn((GuidePage, ScrollList, page, Piece::Well.image())).id();
                    spawn_scroll_more(right, font, page);
                });
            });
            spawn_rule(card, space::GAP);
            card.spawn((CharacterHint, text(font, "", BODY, MUTED)));
        });
        mark_screen(root, GameScreen::Character);
    });
}

fn show(ui: Res<UiState>, book: Res<super::prayers::PrayerBook>, mut root: Query<&mut Visibility, With<CharacterScreen>>, mut guide: ResMut<Guide>, mut was_open: Local<bool>) {
    if *was_open && !ui.character_open {
        guide.0 = None;
    }
    *was_open = ui.character_open;
    if let Ok(mut visibility) = root.single_mut() {
        // The recipe viewer takes the screen while it is open.
        visibility.set_if_neq(if ui.character_open && !ui.recipes_open && !book.0 { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn pick_skill(
    rows: Query<(&SkillRow, &Interaction), Changed<Interaction>>,
    back: Query<&Interaction, (Changed<Interaction>, With<BackToSkills>)>,
    categories: Query<(&CategoryButton, &Interaction), Changed<Interaction>>,
    lines: Query<(&EntryButton, &Interaction), Changed<Interaction>>,
    mut guide: ResMut<Guide>,
    mut view: ResMut<GuideView>,
) {
    for (row, interaction) in &rows {
        if *interaction == Interaction::Pressed {
            guide.0 = Some(row.0);
            *view = GuideView::default();
        }
    }
    for (category, interaction) in &categories {
        if *interaction == Interaction::Pressed {
            view.category.clone_from(&category.0);
        }
    }
    for (line, interaction) in &lines {
        if *interaction == Interaction::Pressed {
            view.entry = Some(line.0);
        }
    }
    // Back from a line's page to the guide, and from the guide to the skills.
    if back.iter().any(|i| *i == Interaction::Pressed) {
        if view.entry.is_some() {
            view.entry = None;
        } else {
            guide.0 = None;
            *view = GuideView::default();
        }
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
    mut list: Query<(Entity, &mut Visibility, Option<&Children>), With<SkillList>>,
    mut progress: Query<(&SkillProgress, &mut Node), Without<SkillList>>,
    mut hint: Query<&mut Text, With<CharacterHint>>,
    mut condition: Query<&mut Node, (With<ConditionColumn>, Without<SkillProgress>)>,
) {
    let Ok((entity, mut visibility, rows)) = list.single_mut() else { return };
    if locale.is_changed()
        && let Some(rows) = rows
    {
        // Written again in the new language.
        for &row in rows {
            commands.entity(row).despawn();
        }
        return;
    }
    // Hidden under an open guide, but still there: its tiles give the guide its height.
    let shown = ui.character_open;
    visibility.set_if_neq(if guide.0.is_none() { Visibility::Inherited } else { Visibility::Hidden });
    // A guide takes the whole panel; the list shares it with how you are.
    if let Ok(mut node) = condition.single_mut() {
        let display = if guide.0.is_some() { Display::None } else { Display::Flex };
        if node.display != display {
            node.display = display;
        }
    }
    let words = locale.get(if guide.0.is_some() { "character.guide_hint" } else { "character.hint" });
    if let Ok(mut text) = hint.single_mut()
        && text.0 != words
    {
        words.clone_into(&mut text.0);
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
                    row_gap: Val::Px(space::INNER),
                    padding: UiRect::all(Val::Px(space::GAP)),
                    ..default()
                };
                let button = spawn_button(list, &font.0, "", ButtonStyle::Secondary, SkillRow(id));
                list.commands().entity(button).insert(tile).despawn_children().with_children(|tile| {
                    tile.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() }).with_children(|line| {
                        let icon = images.skills.get(id.0 as usize).cloned().unwrap_or_default();
                        line.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::new(icon)));
                        line.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), ..default() }).with_children(|words| {
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

/// Fills the guide when one opens, and again for another kind of line or a line's page.
#[allow(clippy::too_many_arguments)]
fn draw_guide(
    mut commands: Commands,
    guide: Res<Guide>,
    view: Res<GuideView>,
    launch: Res<Launch>,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    locale: Res<Locale>,
    mut page: Query<(Entity, &mut Node, &mut ScrollPosition), With<GuidePage>>,
    mut levels: Query<(&SkillLevel, &mut Text)>,
    mut drawn: Local<Option<(SkillId, GuideView, u8)>>,
    images: Res<super::HudImages>,
) {
    draw_levels(&locale, &launch, &player, &mut levels);
    let Ok((entity, mut node, mut scroll)) = page.single_mut() else { return };
    let display = if guide.0.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    // Drawn again when the level changes, too: the lines reached light up as it rises.
    let wanted = guide.0.map(|id| (id, view.clone(), launch.content.skills.level(player.skills.xp(id))));
    if *drawn == wanted && !locale.is_changed() {
        return;
    }
    *drawn = wanted;
    commands.entity(entity).despawn_children();
    scroll.0 = Vec2::ZERO;
    let Some(id) = guide.0 else { return };
    let skills = &launch.content.skills;
    let def = skills.get(id);
    let (level, _, progress) = standing(&locale, skills, player.skills.xp(id));
    let entries = super::guide::entries(&locale, &launch, id);
    let font = &font.0;
    let fallback = images.skills.get(id.0 as usize).cloned().unwrap_or_default();
    let icon_of = |item: Option<BlockId>| item.and_then(|item| images.icons.get(item.0 as usize).cloned()).unwrap_or_else(|| fallback.clone());
    // A guide line: its mark, the level it opens at and its words, dim until reached.
    let spawn_line = |list: &mut ChildSpawnerCommands, i: usize| {
        let entry = &entries[i];
        let reached = level >= entry.level;
        // A grid, so the line's words wrap to exactly the width their column gets.
        let row = Node {
            display: Display::Grid,
            grid_template_columns: vec![GridTrack::px(16.0 * PX), GridTrack::px(22.0 * PX), GridTrack::flex(1.0)],
            column_gap: Val::Px(space::GAP),
            align_items: AlignItems::Start,
            padding: UiRect::all(Val::Px(space::HAIR)),
            flex_shrink: 0.0,
            ..default()
        };
        list.spawn((Button, crate::ui::widgets::HoverLight, EntryButton(i), row)).with_children(|row| {
            let tint = if reached { Color::WHITE } else { Color::srgba(1.0, 1.0, 1.0, 0.45) };
            row.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() }, ImageNode::new(icon_of(entry.icon)).with_color(tint)));
            row.spawn(text(font, format!("{}", entry.level), BODY, if reached { GOLD } else { MUTED }));
            row.spawn((Node { min_width: Val::ZERO, ..default() }, text(font, entry.words.clone(), BODY, if reached { TEXT } else { MUTED })));
        });
    };
    commands.entity(entity).with_children(|page| {
        let open = view.entry.filter(|&i| i < entries.len());
        let top = Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), flex_shrink: 0.0, ..default() };
        page.spawn(top).with_children(|top| {
            match open {
                // A line's page is headed by the line itself.
                Some(i) => {
                    top.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }).with_children(|title| {
                        title.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::new(icon_of(entries[i].icon))));
                        title.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, shadowed(font, entries[i].words.clone(), BODY, GOLD)));
                    });
                }
                None => {
                    let level = locale.format("character.level", &[("level", &level)]);
                    top.spawn(shadowed(font, format!("{}  {level}", locale.skill(def)), BODY, GOLD));
                }
            }
            top.spawn(Node { width: Val::Px(40.0 * PX), flex_shrink: 0.0, ..default() }).with_children(|cell| {
                spawn_button(cell, font, "character.back", ButtonStyle::Secondary, BackToSkills);
            });
        });
        if let Some(i) = open {
            let (parts, near) = super::guide::page(&locale, &launch, id, &entries, i);
            for part in parts {
                match part {
                    super::guide::Part::Heading(words) => {
                        page.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, shadowed(font, words, BODY, ACCENT)));
                    }
                    super::guide::Part::Line(words) => {
                        page.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, words, BODY, SOFT)));
                    }
                    super::guide::Part::Items(items) => {
                        let row = Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(space::GAP), row_gap: Val::Px(space::HAIR), flex_shrink: 0.0, ..default() };
                        page.spawn(row).with_children(|row| {
                            for item in items {
                                row.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|chip| {
                                    chip.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::new(icon_of(Some(item)))));
                                    chip.spawn(text(font, locale.block(launch.content.blocks.def(item)), BODY, TEXT));
                                });
                            }
                        });
                    }
                }
            }
            if !near.is_empty() {
                page.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, Localized("guide.related"), shadowed(font, "", BODY, ACCENT)));
                for i in near {
                    spawn_line(page, i);
                }
            }
            return;
        }
        page.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, locale.skill_description(def), BODY, SOFT)));
        page.spawn((Node { flex_shrink: 0.0, margin: UiRect::bottom(Val::Px(space::INNER)), ..default() }, text(font, progress, BODY, MUTED)));
        if entries.is_empty() {
            page.spawn((Node { flex_shrink: 0.0, ..default() }, Localized("character.untrained"), text(font, "", BODY, MUTED)));
        }
        let categories = super::guide::categories(&entries);
        let shown: Vec<usize> = (0..entries.len()).filter(|&i| view.category.as_ref().is_none_or(|c| &entries[i].category == c)).collect();
        // With kinds to sort by, they stand down the left, the lines beside them.
        page.spawn(Node { column_gap: Val::Px(space::GAP), align_items: AlignItems::Start, flex_shrink: 0.0, ..default() }).with_children(|split| {
            if !categories.is_empty() {
                let side = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), width: Val::Px(CATEGORIES), flex_shrink: 0.0, ..default() };
                split.spawn(side).with_children(|side| {
                    let all = [(None, locale.get("guide.category.all").to_owned())];
                    for (category, words) in all.into_iter().chain(categories.iter().map(|c| (Some(c.clone()), super::guide::category_name(&locale, c)))) {
                        let node = Node {
                            min_height: Val::Px(BUTTON_SMALL),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)),
                            ..default()
                        };
                        let style = ButtonStyle::Secondary;
                        let mut button = side.spawn((Button, style, node, style.pieces()[0].image(), CategoryButton(category.clone())));
                        if view.category == category {
                            button.insert(crate::ui::widgets::Selected);
                        }
                        button.with_child((shadowed(font, words, BODY, TEXT), crate::lang::TightLine));
                    }
                });
            }
            split.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), flex_grow: 1.0, min_width: Val::ZERO, ..default() }).with_children(|list| {
                for i in shown {
                    spawn_line(list, i);
                }
            });
        });
    });
}

/// The width of a guide's column of kinds, and the least height of their buttons.
const CATEGORIES: f32 = 56.0 * PX;
const BUTTON_SMALL: f32 = 16.0 * PX;

/// How you are, as the character and inventory screens show it: health
/// meters, how warm you feel, and every effect on you with what it does.
#[derive(Component)]
pub struct Condition;
/// The list of effects in a condition panel.
#[derive(Component)]
struct EffectList;
#[derive(Component)]
struct FeltLine;

/// Room for a meter's label: six letters of the monospace fonts.
const LABEL: f32 = 38.0 * PX;

/// A condition panel, filling its parent's width.
pub fn spawn_condition(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    let column = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::INNER), width: Val::Percent(100.0), ..default() };
    parent.spawn((Condition, column)).with_children(|panel| {
        let row = || Node { column_gap: Val::Px(space::INNER), align_items: AlignItems::Center, ..default() };
        panel.spawn(row()).with_children(|row| {
            row.spawn((Node { width: Val::Px(LABEL), flex_shrink: 0.0, ..default() }, Localized("character.health"), text(font, "", BODY, MUTED), TextLayout::no_wrap()));
            spawn_meter(row, font, Piece::BarHealth, Val::Percent(100.0), HealthMeter);
        });
        panel.spawn((FeltLine, text(font, "", BODY, SOFT)));
        panel.spawn((EffectList, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), margin: UiRect::top(Val::Px(space::HAIR)), ..default() }));
    });
}

/// Writes the combat level under the condition, once the server has told our skills.
fn draw_combat(player: Res<LocalPlayer>, launch: Res<Launch>, locale: Res<Locale>, mut line: Query<&mut Text, With<CombatLine>>) {
    if !player.is_changed() && !locale.is_changed() {
        return;
    }
    let words = if player.skills_known {
        locale.format("character.combat_level", &[("level", &launch.content.skills.combat_level(&player.skills))])
    } else {
        String::new()
    };
    for mut text in &mut line {
        if text.0 != words {
            text.0.clone_from(&words);
        }
    }
}

/// Keeps every condition panel current.
#[allow(clippy::too_many_arguments)]
fn draw_condition(
    mut commands: Commands,
    player: Res<LocalPlayer>,
    font: Res<UiFont>,
    health: Query<&Meter, With<HealthMeter>>,
    mut felt: Query<&mut Text, With<FeltLine>>,
    lists: Query<(Entity, Ref<EffectList>)>,
    mut nodes: Query<&mut Node>,
    children: Query<&Children>,
    mut texts: Query<&mut Text, Without<FeltLine>>,
    mut shown: Local<Option<Vec<voxmar_engine::effect::ActiveEffect>>>,
    (locale, launch, images): (Res<Locale>, Res<Launch>, Res<super::HudImages>),
) {
    let most = player.max_health.max(1);
    for meter in &health {
        set_meter(meter, player.health as f32 / most as f32, &format!("{}/{most}", player.health), &mut nodes, &children, &mut texts);
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
            for active in effects {
                let def = launch.content.effects.get(active.effect);
                let buff = def.kind == voxmar_engine::effect::Kind::Buff;
                // Its icon and name, what it does under them; hover it for the details.
                let row = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), ..default() };
                list.spawn((row, Button, super::effects::EffectHover(active.effect), bevy::ui::RelativeCursorPosition::default())).with_children(|row| {
                    let color = if buff { ACCENT } else { crate::ui::widgets::ERROR };
                    row.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|line| {
                        line.spawn(super::effect_icon(&images, active.effect));
                        line.spawn(shadowed(&font.0, locale.effect(def), BODY, color));
                        if active.seconds_left.is_some() {
                            line.spawn((text(&font.0, "", BODY, SOFT), super::EffectTimer(active.effect)));
                        }
                    });
                    row.spawn(text(&font.0, locale.effect_description(def), BODY, MUTED));
                });
            }
        });
    }
}
