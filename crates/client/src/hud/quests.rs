//! The quest book (L) and the quests pinned on screen. One chapter per age down the left;
//! each chapter's quests as a web, a quest after the ones it follows, joined by lines,
//! gold once done, bright while open, dim until what comes before is done. A quest's page
//! says first why it is worth doing (the reason and the reward), then its routes (any one
//! finishes it) with each goal's progress, then the rewards, and pins it. Up to three
//! pinned quests stand at the right of the screen with their live progress, and drop off a
//! moment after they are done. The quests and their progress are the server's
//! (`voxmar_engine::quest`, docs/quests.md).

use bevy::prelude::*;
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::lang::{def_key, title_from_id};
use voxmar_engine::quest::{Goal, QuestDef, QuestState, Reward};

use super::HudImages;
use super::SLOT;
use super::screens::{GameScreen, ScreenSystems, spawn_hotkeys};
use crate::lang::{Locale, Localized, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, GOLD, MUTED, SCRIM, SHADOW, SOFT, ScrollList, TEXT, UiFont, panel, screen, shadowed, space, spawn_button, spawn_heading, spawn_rule, spawn_scroll_more,
    text,
};

pub struct QuestsPlugin;

impl Plugin for QuestsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QuestView>()
            .add_systems(PostStartup, (spawn, spawn_tracker, open_start_quest))
            .add_systems(Update, ((show, pick, draw).chain().after(ScreenSystems), (retire, track).chain()).run_if(in_state(Screen::InGame)));
    }
}

/// Quests pinned at once, at most.
pub const PINS: usize = 3;
/// Seconds a done quest stays on screen, ticked, before it drops off.
const DONE_STAYS: f32 = 5.0;

/// The chapter open and the quest picked in it.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct QuestView {
    pub chapter: usize,
    pub picked: Option<String>,
}

/// A quest to open the book on at startup (`--quests`, for captures).
#[derive(Resource)]
pub struct StartQuest(pub String);

fn open_start_quest(start: Option<Res<StartQuest>>, launch: Res<Launch>, mut view: ResMut<QuestView>, mut ui: ResMut<UiState>) {
    let Some(start) = start else { return };
    let quests = &launch.content.quests;
    let Some(quest) = quests.get(&start.0).or_else(|| quests.get(&format!("base:{}", start.0))) else { return };
    view.chapter = chapters(&launch.content).iter().position(|age| *age == quest.age).unwrap_or(0);
    view.picked = Some(quest.name.clone());
    ui.quests_open = true;
}

/// Where a quest stands for the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Done,
    Open,
    /// What comes before it is not done yet.
    Waiting,
}

/// The chapters, by age, in the order the quests first name them.
pub fn chapters(content: &Content) -> Vec<String> {
    let mut ages: Vec<String> = Vec::new();
    for quest in content.quests.iter() {
        if !ages.contains(&quest.age) {
            ages.push(quest.age.clone());
        }
    }
    ages
}

fn state<'a>(player: &'a LocalPlayer, name: &str) -> Option<&'a QuestState> {
    player.quests.iter().find(|s| s.name == name)
}

pub fn standing(content: &Content, player: &LocalPlayer, quest: &QuestDef) -> Standing {
    let done = |name: &str| state(player, name).is_some_and(|s| s.done);
    if done(&quest.name) {
        Standing::Done
    } else if content.quests.open(quest, &done) {
        Standing::Open
    } else {
        Standing::Waiting
    }
}

/// A quest's name and its reason, in the player's language.
pub fn title(locale: &Locale, quest: &QuestDef) -> String {
    locale.get_or(&def_key("quest", &quest.name), &quest.title).to_owned()
}

fn why(locale: &Locale, quest: &QuestDef) -> String {
    locale.get_or(&format!("{}.why", def_key("quest", &quest.name)), &quest.why).to_owned()
}

/// An item by a quest's name for it: `any`, `tag:TAG` or an item's name, its package left out
/// or not.
fn item_words(locale: &Locale, content: &Content, name: &str) -> String {
    if name == "any" {
        return locale.get("quest.any").to_owned();
    }
    if let Some(tag) = name.strip_prefix("tag:") {
        return locale.get_or(&format!("tag.{tag}"), &title_from_id(tag)).to_owned();
    }
    let blocks = &content.blocks;
    match blocks.id(name).or_else(|| blocks.id(&format!("base:{name}"))) {
        Some(id) => locale.block(blocks.def(id)),
        None => title_from_id(name),
    }
}

fn creature_words(locale: &Locale, content: &Content, name: &str) -> String {
    if name == "any" {
        return locale.get("quest.any_creature").to_owned();
    }
    match content.creatures.find(name).or_else(|| content.creatures.find(&format!("base:{name}"))) {
        Some(id) => locale.creature(&content.creatures.get(id).def),
        None => title_from_id(name),
    }
}

fn skill_words(locale: &Locale, content: &Content, name: &str) -> String {
    let skills = &content.skills;
    match skills.ids().find(|&id| skills.get(id).name.rsplit(':').next() == Some(name.rsplit(':').next().unwrap_or(name))) {
        Some(id) => locale.skill(skills.get(id)),
        None => title_from_id(name),
    }
}

/// The item a quest is drawn with: what it gives, else what it asks for.
fn quest_item(content: &Content, quest: &QuestDef) -> Option<BlockId> {
    let blocks = &content.blocks;
    let item = |name: &str| blocks.id(name).or_else(|| blocks.id(&format!("base:{name}")));
    let given = quest.rewards.iter().find_map(|r| if let Reward::Item(name, _) = r { item(name) } else { None });
    given.or_else(|| {
        quest.tasks.iter().flatten().find_map(|goal| match goal {
            Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::Have(name, _) | Goal::Pick(name, _) | Goal::Mine(name, _) | Goal::Catch(name, _) => item(name),
            _ => None,
        })
    })
}

/// What a goal asks, in words, with its progress when it counts more than one.
pub fn goal_words(locale: &Locale, content: &Content, goal: &Goal, (have, need): (u32, u32)) -> String {
    let what = match goal {
        Goal::Craft(item, _) => locale.format("quest.goal.craft", &[("item", &item_words(locale, content, item))]),
        Goal::Cook(item, _) => locale.format("quest.goal.cook", &[("item", &item_words(locale, content, item))]),
        Goal::Catch(fish, _) if fish == "Kinds" => locale.get("quest.goal.kinds").to_owned(),
        Goal::Catch(fish, _) => locale.format("quest.goal.catch", &[("item", &item_words(locale, content, fish))]),
        Goal::Kill(creature, _) => locale.format("quest.goal.kill", &[("creature", &creature_words(locale, content, creature))]),
        Goal::Tame(creature, _) => locale.format("quest.goal.tame", &[("creature", &creature_words(locale, content, creature))]),
        Goal::Pick(plant, _) => locale.format("quest.goal.pick", &[("item", &item_words(locale, content, plant))]),
        Goal::Mine(ore, _) => locale.format("quest.goal.mine", &[("item", &item_words(locale, content, ore))]),
        Goal::Visit(_) => locale.get("quest.goal.visit").to_owned(),
        Goal::Discover(page, _) => {
            let page = locale.get(&format!("journal.page.{}", super::journal::page_name(*page))).to_owned();
            locale.format("quest.goal.discover", &[("page", &page)])
        }
        Goal::Level(skill, level) => {
            return locale.format("quest.goal.level", &[("skill", &skill_words(locale, content, skill)), ("level", level)]);
        }
        Goal::Have(item, _) => locale.format("quest.goal.have", &[("item", &item_words(locale, content, item))]),
        Goal::Home => return locale.get("quest.goal.home").to_owned(),
        Goal::Built(name, _) => locale.format("quest.goal.built", &[("item", &item_words(locale, content, name))]),
    };
    if need > 1 { locale.format("quest.progress", &[("goal", &what), ("have", &have.min(need)), ("need", &need)]) } else { what }
}

/// Each goal of the task nearest done, with its progress: (have, need).
fn best_task<'a>(quest: &'a QuestDef, state: Option<&QuestState>) -> Vec<(&'a Goal, (u32, u32))> {
    let progress = |t: usize, g: usize| state.and_then(|s| s.tasks.get(t)?.get(g).copied()).unwrap_or((0, 1));
    let share = |t: usize| -> f32 {
        let goals = &quest.tasks[t];
        goals.iter().enumerate().map(|(g, _)| { let (have, need) = progress(t, g); (have.min(need) as f32) / need.max(1) as f32 }).sum::<f32>() / goals.len().max(1) as f32
    };
    let best = (0..quest.tasks.len()).max_by(|&a, &b| share(a).total_cmp(&share(b))).unwrap_or(0);
    quest.tasks.get(best).map_or_else(Vec::new, |goals| goals.iter().enumerate().map(|(g, goal)| (goal, progress(best, g))).collect())
}

#[derive(Component)]
struct QuestScreen;
#[derive(Component)]
struct ChapterButton(usize);
#[derive(Component)]
struct Web;
#[derive(Component)]
struct QuestNode(String);
#[derive(Component)]
struct Detail;
#[derive(Component)]
struct PinButton;

/// The panel's width, the chapters' column, and the web's height.
const WIDTH: f32 = 540.0 * PX;
const CHAPTERS: f32 = 80.0 * PX;
const WEB_HEIGHT: f32 = 110.0 * PX;
const DETAIL_HEIGHT: f32 = 120.0 * PX;
/// The web's grid: a column per step after the first quest, a row per quest in a column.
const STEP_X: f32 = SLOT + 14.0 * PX;
const STEP_Y: f32 = SLOT + 6.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((QuestScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            card.spawn(Node { width: Val::Percent(100.0), flex_direction: FlexDirection::Column, ..default() }).with_children(|heading| spawn_heading(heading, font, "screen.quests"));
            card.spawn(Node { column_gap: Val::Px(space::GAP), ..default() }).with_children(|columns| {
                columns.spawn((Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), width: Val::Px(CHAPTERS), flex_shrink: 0.0, ..default() }, Chapters));
                columns.spawn(Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: Val::Px(space::GAP), min_width: Val::ZERO, ..default() }).with_children(|right| {
                    let web = Node { height: Val::Px(WEB_HEIGHT), overflow: Overflow::scroll(), padding: UiRect::all(Val::Px(space::GAP)), ..default() };
                    right.spawn((Web, ScrollList, web, Piece::Well.image()));
                    let detail = Node {
                        height: Val::Px(DETAIL_HEIGHT),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(space::HAIR),
                        overflow: Overflow::scroll_y(),
                        ..default()
                    };
                    let detail = right.spawn((Detail, ScrollList, detail)).id();
                    spawn_scroll_more(right, font, detail);
                });
            });
            spawn_rule(card, space::GAP);
            card.spawn((Localized("quest.hint"), text(font, "", BODY, MUTED)));
        });
        spawn_hotkeys(root, GameScreen::Quests);
    });
}

#[derive(Component)]
struct Chapters;

fn show(ui: Res<UiState>, mut root: Query<&mut Visibility, With<QuestScreen>>) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.quests_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

#[allow(clippy::type_complexity)]
fn pick(
    chapters: Query<(&ChapterButton, &Interaction), Changed<Interaction>>,
    nodes: Query<(&QuestNode, &Interaction), Changed<Interaction>>,
    pins: Query<&Interaction, (Changed<Interaction>, With<PinButton>)>,
    mut view: ResMut<QuestView>,
    mut settings: ResMut<Settings>,
) {
    for (chapter, interaction) in &chapters {
        if *interaction == Interaction::Pressed && view.chapter != chapter.0 {
            view.chapter = chapter.0;
            view.picked = None;
        }
    }
    for (node, interaction) in &nodes {
        if *interaction == Interaction::Pressed {
            view.picked = Some(node.0.clone());
        }
    }
    if pins.iter().any(|i| *i == Interaction::Pressed)
        && let Some(name) = view.picked.clone()
    {
        toggle_pin(&mut settings, &name);
    }
}

/// Pins a quest, or unpins it; pinning a fourth does nothing.
pub fn toggle_pin(settings: &mut Settings, name: &str) {
    if let Some(i) = settings.pinned_quests.iter().position(|p| p == name) {
        settings.pinned_quests.remove(i);
    } else if settings.pinned_quests.len() < PINS {
        settings.pinned_quests.push(name.to_owned());
    } else {
        return;
    }
    settings.save();
}

/// Draws the open chapter and the picked quest, again when anything they show changes.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    ui: Res<UiState>,
    view: Res<QuestView>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    images: Res<HudImages>,
    settings: Res<Settings>,
    chapters_column: Query<Entity, With<Chapters>>,
    web: Query<Entity, With<Web>>,
    detail: Query<Entity, With<Detail>>,
    mut drawn: Local<Option<(QuestView, Vec<QuestState>, Vec<String>)>>,
) {
    if !ui.quests_open {
        *drawn = None;
        return;
    }
    let wanted = (view.clone(), player.quests.clone(), settings.pinned_quests.clone());
    if drawn.as_ref() == Some(&wanted) && !locale.is_changed() {
        return;
    }
    *drawn = Some(wanted);
    let content = &launch.content;
    let font = &font.0;
    let ages = chapters(content);
    if let Ok(column) = chapters_column.single() {
        commands.entity(column).despawn_children().with_children(|column| {
            for (i, age) in ages.iter().enumerate() {
                let quests: Vec<&QuestDef> = content.quests.iter().filter(|q| &q.age == age).collect();
                let done = quests.iter().filter(|q| standing(content, &player, q) == Standing::Done).count();
                let node = Node {
                    min_height: Val::Px(14.0 * PX),
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)),
                    ..default()
                };
                let style = ButtonStyle::Secondary;
                let mut button = column.spawn((Button, style, node, style.pieces()[0].image(), ChapterButton(i)));
                if i == view.chapter {
                    button.insert(crate::ui::widgets::Selected);
                }
                button.with_children(|button| {
                    let name = locale.get_or(&format!("quest.age.{age}"), &title_from_id(age)).to_owned();
                    button.spawn((shadowed(font, name, BODY, TEXT), TightLine));
                    button.spawn((text(font, format!("{done}/{}", quests.len()), BODY, MUTED), TightLine));
                });
            }
        });
    }
    let Some(age) = ages.get(view.chapter) else { return };
    let quests: Vec<&QuestDef> = content.quests.iter().filter(|q| &q.age == age).collect();
    // Each quest's column: one past the furthest of those it comes after, in this chapter.
    let mut depth: Vec<usize> = vec![0; quests.len()];
    for _ in 0..quests.len() {
        for (i, quest) in quests.iter().enumerate() {
            let before = quest.after.iter().flat_map(|a| a.split('|')).filter_map(|name| quests.iter().position(|q| q.name == name)).map(|j| depth[j] + 1).max().unwrap_or(0);
            depth[i] = before;
        }
    }
    let mut rows: Vec<usize> = Vec::new();
    let place: Vec<Vec2> = depth
        .iter()
        .map(|&d| {
            if rows.len() <= d {
                rows.resize(d + 1, 0);
            }
            let at = Vec2::new(d as f32 * STEP_X, rows[d] as f32 * STEP_Y);
            rows[d] += 1;
            at
        })
        .collect();
    let centre = |i: usize| place[i] + Vec2::splat(SLOT / 2.0);
    let icon_of = |item: Option<BlockId>| item.and_then(|item| images.icons.get(item.0 as usize).cloned()).unwrap_or_default();
    if let Ok(web) = web.single() {
        commands.entity(web).despawn_children().with_children(|web| {
            if quests.is_empty() {
                web.spawn((Localized("quest.empty"), text(font, "", BODY, MUTED)));
                return;
            }
            // Room for the whole web, so it scrolls.
            let width = (rows.len() as f32) * STEP_X;
            let height = rows.iter().copied().max().unwrap_or(1) as f32 * STEP_Y;
            web.spawn(Node { width: Val::Px(width), height: Val::Px(height), flex_shrink: 0.0, ..default() }).with_children(|area| {
                // The lines first, under the quests.
                for (i, quest) in quests.iter().enumerate() {
                    for name in quest.after.iter().flat_map(|a| a.split('|')) {
                        let Some(j) = quests.iter().position(|q| q.name == name) else { continue };
                        let (from, to) = (centre(j), centre(i));
                        let length = from.distance(to);
                        let middle = (from + to) / 2.0;
                        let done = standing(content, &player, quests[j]) == Standing::Done;
                        let line = Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(middle.x - length / 2.0),
                            top: Val::Px(middle.y - PX / 2.0),
                            width: Val::Px(length),
                            height: Val::Px(PX),
                            ..default()
                        };
                        let turn = UiTransform { rotation: Rot2::radians((to - from).to_angle()), ..default() };
                        area.spawn((line, turn, BackgroundColor(if done { GOLD } else { MUTED.with_alpha(0.6) })));
                    }
                }
                for (i, quest) in quests.iter().enumerate() {
                    let standing = standing(content, &player, quest);
                    let picked = view.picked.as_deref() == Some(quest.name.as_str());
                    let node = Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(place[i].x),
                        top: Val::Px(place[i].y),
                        width: Val::Px(SLOT),
                        height: Val::Px(SLOT),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    };
                    let piece = match (picked, standing) {
                        (true, _) => Piece::WellFocus,
                        (_, Standing::Done) => Piece::Frame,
                        _ => Piece::Well,
                    };
                    area.spawn((Button, crate::ui::widgets::HoverLight, QuestNode(quest.name.clone()), node, piece.image())).with_children(|cell| {
                        let tint = match standing {
                            Standing::Done | Standing::Open => Color::WHITE,
                            Standing::Waiting => Color::srgba(1.0, 1.0, 1.0, 0.35),
                        };
                        cell.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() }, ImageNode::new(icon_of(quest_item(content, quest))).with_color(tint)));
                    });
                }
            });
        });
    }
    // The picked quest: why first, then its routes and their goals, the rewards, and a pin.
    if let Ok(detail) = detail.single() {
        commands.entity(detail).despawn_children().with_children(|detail| {
            let Some(quest) = view.picked.as_deref().and_then(|name| quests.iter().find(|q| q.name == name)) else {
                detail.spawn((Localized("quest.pick"), text(font, "", BODY, MUTED)));
                return;
            };
            let standing = standing(content, &player, quest);
            let state = state(&player, &quest.name);
            detail.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), flex_shrink: 0.0, ..default() }).with_children(|top| {
                top.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, shadowed(font, title(&locale, quest), BODY, GOLD)));
                let status = match standing {
                    Standing::Done => (locale.get("quest.status.done").to_owned(), GOLD),
                    Standing::Open => (locale.get("quest.status.open").to_owned(), ACCENT),
                    Standing::Waiting => (locale.get("quest.status.waiting").to_owned(), MUTED),
                };
                top.spawn((text(font, status.0, BODY, status.1), TightLine));
                let pinned = settings.pinned_quests.contains(&quest.name);
                if standing != Standing::Done && (pinned || settings.pinned_quests.len() < PINS) {
                    top.spawn(Node { width: Val::Px(44.0 * PX), flex_shrink: 0.0, ..default() }).with_children(|cell| {
                        spawn_button(cell, font, if pinned { "quest.unpin" } else { "quest.pin" }, ButtonStyle::Secondary, PinButton);
                    });
                }
            });
            detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, why(&locale, quest), BODY, SOFT)));
            if quest.tasks.len() > 1 {
                detail.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, Localized("quest.routes"), shadowed(font, "", BODY, ACCENT)));
            }
            for (t, task) in quest.tasks.iter().enumerate() {
                if quest.tasks.len() > 1 {
                    detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, locale.format("quest.route", &[("n", &(t + 1))]), BODY, MUTED)));
                }
                for (g, goal) in task.iter().enumerate() {
                    let progress = state.and_then(|s| s.tasks.get(t)?.get(g).copied()).unwrap_or((0, 1));
                    let met = progress.0 >= progress.1 || standing == Standing::Done;
                    let words = goal_words(&locale, content, goal, progress);
                    let mark = if met { "✓ " } else { "• " };
                    detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, format!("{mark}{words}"), BODY, if met { GOLD } else { TEXT })));
                }
            }
            if !quest.rewards.is_empty() {
                let gives: Vec<String> = quest
                    .rewards
                    .iter()
                    .map(|reward| match reward {
                        Reward::Xp(skill, xp) => locale.format("quest.reward.xp", &[("xp", &(*xp as u32)), ("skill", &skill_words(&locale, content, skill))]),
                        Reward::Item(item, count) => locale.format("quest.reward.item", &[("count", count), ("item", &item_words(&locale, content, item))]),
                    })
                    .collect();
                detail.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, Localized("quest.rewards"), shadowed(font, "", BODY, ACCENT)));
                detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, gives.join(locale.get("language.comma")), BODY, SOFT)));
            }
        });
    }
}

/// The pinned quests at the right of the screen.
#[derive(Component)]
struct Tracker;
/// An × that unpins a quest from the tracker itself, while the pointer is free.
#[derive(Component)]
struct Unpin(String);

fn spawn_tracker(mut commands: Commands) {
    let column = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(space::SECTION),
        top: Val::Percent(20.0),
        width: Val::Px(130.0 * PX),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(space::GAP),
        ..default()
    };
    commands.spawn((column, Tracker, GlobalZIndex(0)));
}

/// When each pinned quest was seen done, to let it go a moment later.
#[derive(Default)]
struct Finished(Vec<(String, f32)>);

/// Unpins a quest a moment after it is done.
fn retire(time: Res<Time>, player: Res<LocalPlayer>, mut settings: ResMut<Settings>, mut finished: Local<Finished>) {
    let now = time.elapsed_secs();
    for name in &settings.pinned_quests {
        if state(&player, name).is_some_and(|s| s.done) && !finished.0.iter().any(|(n, _)| n == name) {
            finished.0.push((name.clone(), now));
        }
    }
    let gone: Vec<String> = finished.0.iter().filter(|(_, at)| now - at > DONE_STAYS).map(|(n, _)| n.clone()).collect();
    if !gone.is_empty() {
        settings.pinned_quests.retain(|p| !gone.contains(p));
        finished.0.retain(|(n, _)| !gone.contains(n));
        settings.save();
    }
}

/// Draws the tracker again when the pins or the progress change; an × unpins.
#[allow(clippy::too_many_arguments)]
fn track(
    mut commands: Commands,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    mut settings: ResMut<Settings>,
    tracker: Query<Entity, With<Tracker>>,
    mut visibility: Query<&mut Visibility, With<Tracker>>,
    unpins: Query<(&Unpin, &Interaction), Changed<Interaction>>,
    mut drawn: Local<Option<(Vec<String>, Vec<QuestState>)>>,
) {
    for (unpin, interaction) in &unpins {
        if *interaction == Interaction::Pressed {
            toggle_pin(&mut settings, &unpin.0);
        }
    }
    // Out of the way of a screen, but for the quest book itself.
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open || ui.recipes_open || player.dead;
    if let Ok(mut shown) = visibility.single_mut() {
        shown.set_if_neq(if screen || settings.pinned_quests.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
    }
    let wanted = (settings.pinned_quests.clone(), player.quests.clone());
    if drawn.as_ref() == Some(&wanted) && !locale.is_changed() {
        return;
    }
    *drawn = Some(wanted);
    let Ok(tracker) = tracker.single() else { return };
    let content = &launch.content;
    let font = &font.0;
    commands.entity(tracker).despawn_children().with_children(|tracker| {
        for name in &settings.pinned_quests {
            let Some(quest) = content.quests.get(name) else { continue };
            let state = state(&player, name);
            let done = state.is_some_and(|s| s.done);
            // Each on a faint backing, so its words read over the world.
            let card = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(PX), padding: UiRect::all(Val::Px(space::INNER)), ..default() };
            tracker.spawn((card, BackgroundColor(SHADOW.with_alpha(0.35)))).with_children(|card| {
                card.spawn(Node { justify_content: JustifyContent::SpaceBetween, column_gap: Val::Px(space::INNER), ..default() }).with_children(|top| {
                    top.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, shadowed(font, title(&locale, quest), BODY, GOLD)));
                    top.spawn((Button, Unpin(name.clone()), Node { padding: UiRect::horizontal(Val::Px(PX)), ..default() })).with_child((shadowed(font, "×", BODY, MUTED), TightLine));
                });
                if done {
                    card.spawn(shadowed(font, locale.get("quest.tracker.done").to_owned(), BODY, GOLD));
                    return;
                }
                for (goal, progress) in best_task(quest, state) {
                    let met = progress.0 >= progress.1;
                    let mark = if met { "✓ " } else { "" };
                    card.spawn(shadowed(font, format!("{mark}{}", goal_words(&locale, content, goal, progress)), BODY, if met { GOLD } else { TEXT }));
                }
            });
        }
    });
}
