//! The quest book (L) and the quests pinned on screen. One chapter per age down the left;
//! each chapter's quests as a web, a quest after the ones it follows, joined by lines:
//! gold once done, lit while open, dim while what comes first is not done. A quest from
//! another chapter that comes first stands at the web's left, dim, to show the way in. A
//! web bigger than its well is a canvas: dragged by its empty space (or scrolled either
//! way), it opens on the quest picked, and its lines are cut at the well's edge. A
//! quest's page says first why it is worth doing (the reason and the reward), then what
//! comes first if anything, its routes (any one finishes it) with each goal's progress,
//! and its rewards, and pins it while it is open. The quests pinned (`core/quests.ron`'s
//! tracker says how many) stand at the right of the screen with their live progress, and
//! drop off a moment after they are done. The quests and their progress are the server's
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
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::UiSound;
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, GOLD, MUTED, SCRIM, SHADOW, SOFT, ScrollList, TEXT, UiFont, button_row, panel, screen, shadowed, space, spawn_button, spawn_heading, spawn_rule, spawn_scroll_more,
    text,
};

pub struct QuestsPlugin;

impl Plugin for QuestsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QuestView>()
            .add_systems(PostStartup, (spawn, spawn_tracker))
            .add_systems(OnEnter(Screen::InGame), keep_known_pins)
            .add_systems(Update, ((open_start, choose_on_open, show, pick, claim, turn_ways, keep_web_in_range, drag_web, draw, fit_lines.after(crate::ui::widgets::scroll_lists)).chain().after(ScreenSystems), (seed_pins, retire, track).chain(), hear_quests).run_if(in_state(Screen::InGame)));
    }
}

/// The chapter open and the quest picked in it.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct QuestView {
    pub chapter: usize,
    pub picked: Option<String>,
    /// Which way of making an item each goal's inline recipe shows (a goal by its quest,
    /// task and place), where it has several.
    pub ways: std::collections::BTreeMap<String, usize>,
}

/// A quest to open the book on (`--quests`, for captures).
#[derive(Resource)]
pub struct StartQuest(pub String);

/// Quests to pin on joining (`--pin`, for captures), as the book's Pin would.
#[derive(Resource)]
pub struct StartPins(pub Vec<String>);

/// A quest by a name with its package or without (`torchlight`, `base:torchlight`).
fn find<'a>(content: &'a Content, name: &str) -> Option<&'a QuestDef> {
    content.quests.get(name).or_else(|| content.quests.get(&format!("base:{name}")))
}

/// Opens the book on `--quests`'s quest, and pins `--pin`'s, once the game is up.
fn open_start(
    mut commands: Commands,
    start: Option<Res<StartQuest>>,
    pins: Option<Res<StartPins>>,
    launch: Res<Launch>,
    mut view: ResMut<QuestView>,
    mut ui: ResMut<UiState>,
    mut settings: ResMut<Settings>,
) {
    let content = &launch.content;
    if let Some(pins) = pins {
        for name in &pins.0 {
            if let Some(quest) = find(content, name)
                && !settings.pinned_quests.contains(&quest.name)
            {
                toggle_pin(&mut settings, content, &quest.name);
            }
        }
        commands.remove_resource::<StartPins>();
    }
    let Some(start) = start else { return };
    if let Some(quest) = find(content, &start.0) {
        view.chapter = chapters(content).iter().position(|age| *age == quest.age).unwrap_or(0);
        view.picked = Some(quest.name.clone());
        ui.quests_open = true;
    }
    commands.remove_resource::<StartQuest>();
}

/// What `Settings::tips_seen` remembers once a new player's pins have been looked at.
const START_PINS_SEEN: &str = "start_pins";

/// A new player (nothing done, nothing pinned) starts with the content's `start_pins`
/// pinned, so the first thing on screen is what to do. Once for this install, the first
/// time a world's quests arrive, however that went: a player who unpins them keeps their
/// choice in every world after.
fn seed_pins(player: Res<LocalPlayer>, launch: Res<Launch>, mut settings: ResMut<Settings>) {
    if player.quests.is_empty() || settings.tips_seen.iter().any(|seen| seen == START_PINS_SEEN) {
        return;
    }
    settings.tips_seen.push(START_PINS_SEEN.to_owned());
    if settings.pinned_quests.is_empty() && player.quests.iter().all(|quest| !quest.done) {
        let content = &launch.content;
        for name in content.quests.start_pins() {
            toggle_pin(&mut settings, content, name);
        }
    }
    settings.save();
}

/// Only quests the content has stay pinned, and no more than the tracker shows.
fn keep_known_pins(launch: Res<Launch>, mut settings: ResMut<Settings>) {
    let content = &launch.content;
    let most = content.quests.tracker().pins;
    let before = settings.pinned_quests.clone();
    settings.pinned_quests.retain(|name| content.quests.get(name).is_some());
    settings.pinned_quests.truncate(most);
    if settings.pinned_quests != before {
        settings.save();
    }
}

/// Opened, the book shows the first unfinished pinned quest, else the first open one, in
/// its chapter.
fn choose_on_open(ui: Res<UiState>, player: Res<LocalPlayer>, launch: Res<Launch>, settings: Res<Settings>, mut view: ResMut<QuestView>, mut was_open: Local<bool>) {
    let opened = ui.quests_open && !*was_open;
    *was_open = ui.quests_open;
    if !opened || view.picked.is_some() {
        return;
    }
    let content = &launch.content;
    let pinned = settings.pinned_quests.iter().filter_map(|name| content.quests.get(name)).find(|quest| standing(content, &player, quest) != Standing::Done);
    let chosen = pinned.or_else(|| content.quests.iter().find(|quest| standing(content, &player, quest) == Standing::Open));
    if let Some(quest) = chosen {
        view.chapter = chapters(content).iter().position(|age| *age == quest.age).unwrap_or(0);
        view.picked = Some(quest.name.clone());
    }
}

/// Where a quest stands for the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Done,
    Open,
    /// What comes first is not done yet.
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

/// Whether a quest is finished and its rewards wait to be claimed.
pub fn claimable(player: &LocalPlayer, name: &str) -> bool {
    state(player, name).is_some_and(|s| s.done && !s.claimed)
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

/// The package a quest belongs to, where the names it gives are looked up first.
fn package(quest: &QuestDef) -> &str {
    quest.name.split_once(':').map_or("base", |(package, _)| package)
}

/// A content name as a quest gives it: its own package's first, then as written.
fn qualified(package: &str, name: &str) -> [String; 2] {
    [if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") }, name.to_owned()]
}

fn item_id(content: &Content, package: &str, name: &str) -> Option<BlockId> {
    qualified(package, name).iter().find_map(|name| content.blocks.item_named(name))
}

/// An item by a quest's name for it: `any`, `tag:TAG` or an item's name.
fn item_words(locale: &Locale, content: &Content, package: &str, name: &str) -> String {
    if name == "any" {
        return locale.get("quest.any").to_owned();
    }
    if let Some(tag) = name.strip_prefix("tag:") {
        return locale.get_or(&format!("tag.{tag}"), &title_from_id(tag)).to_owned();
    }
    match item_id(content, package, name) {
        Some(id) => locale.block(content.blocks.def(id)),
        None => title_from_id(name),
    }
}

fn creature_words(locale: &Locale, content: &Content, package: &str, name: &str) -> String {
    if name == "any" {
        return locale.get("quest.any_creature").to_owned();
    }
    match qualified(package, name).iter().find_map(|name| content.creatures.find(name)) {
        Some(id) => locale.creature(&content.creatures.get(id).def),
        None => title_from_id(name),
    }
}

fn multiblock_words(locale: &Locale, content: &Content, package: &str, name: &str) -> String {
    let found = qualified(package, name).into_iter().find(|name| content.multiblocks.get(name).is_some());
    let name = found.unwrap_or_else(|| name.to_owned());
    locale.get_or(&def_key("multiblock", &name), &title_from_id(&name)).to_owned()
}

fn skill_id(content: &Content, name: &str) -> Option<voxmar_engine::skill::SkillId> {
    let skills = &content.skills;
    let local = name.rsplit(':').next().unwrap_or(name);
    skills.ids().find(|&id| skills.get(id).name.rsplit(':').next() == Some(local))
}

fn skill_words(locale: &Locale, content: &Content, name: &str) -> String {
    match skill_id(content, name) {
        Some(id) => locale.skill(content.skills.get(id)),
        None => title_from_id(name),
    }
}

/// What a quest is drawn with: the item it gives, else the item it asks for, else the
/// skill it trains.
pub(crate) fn quest_icon(content: &Content, images: &HudImages, quest: &QuestDef) -> Handle<Image> {
    let package = package(quest);
    let item = |name: &str| item_id(content, package, name);
    let given = quest.rewards.iter().find_map(|r| if let Reward::Item(name, _) = r { item(name) } else { None });
    let asked = || {
        quest.tasks.iter().flatten().find_map(|goal| match goal {
            Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::CookAt(name, _, _) | Goal::Have(name, _) | Goal::Pick(name, _) | Goal::Mine(name, _) | Goal::Catch(name, _) | Goal::LookUp(name) => item(name),
            _ => None,
        })
    };
    if let Some(icon) = given.or_else(asked).and_then(|id| images.icons.get(id.0 as usize)) {
        return icon.clone();
    }
    let skill = quest
        .rewards
        .iter()
        .find_map(|r| if let Reward::Xp(name, _) = r { skill_id(content, name) } else { None })
        .or_else(|| quest.tasks.iter().flatten().find_map(|goal| if let Goal::Level(name, _) = goal { skill_id(content, name) } else { None }));
    skill.and_then(|id| images.skills.get(id.0 as usize).cloned()).unwrap_or_default()
}

/// What a goal asks, in words, with its progress when it counts more than one.
pub fn goal_words(locale: &Locale, content: &Content, quest: &QuestDef, goal: &Goal, (have, need): (u32, u32)) -> String {
    let package = package(quest);
    let what = match goal {
        Goal::Craft(item, _) => locale.format("quest.goal.craft", &[("item", &item_words(locale, content, package, item))]),
        Goal::CookAt(item, station, _) => locale.format("quest.goal.cook_at", &[("item", &item_words(locale, content, package, item)), ("station", &item_words(locale, content, package, station))]),
        Goal::Cook(item, _) => locale.format("quest.goal.cook", &[("item", &item_words(locale, content, package, item))]),
        Goal::Catch(fish, _) if fish == "Kinds" => locale.get("quest.goal.kinds").to_owned(),
        Goal::Catch(fish, _) => locale.format("quest.goal.catch", &[("item", &item_words(locale, content, package, fish))]),
        Goal::Kill(creature, _) => locale.format("quest.goal.kill", &[("creature", &creature_words(locale, content, package, creature))]),
        Goal::Tame(creature, _) => locale.format("quest.goal.tame", &[("creature", &creature_words(locale, content, package, creature))]),
        Goal::Lured(creature, _) => locale.format("quest.goal.lured", &[("creature", &creature_words(locale, content, package, creature))]),
        Goal::Pick(plant, _) => locale.format("quest.goal.pick", &[("item", &item_words(locale, content, package, plant))]),
        Goal::Mine(ore, _) => locale.format("quest.goal.mine", &[("item", &item_words(locale, content, package, ore))]),
        Goal::Visit(_) => locale.get("quest.goal.visit").to_owned(),
        Goal::Discover(page, _) => {
            let page = locale.get(&format!("journal.page.{}", super::journal::page_name(*page))).to_owned();
            locale.format("quest.goal.discover", &[("page", &page)])
        }
        Goal::Kinds(page, want, _) => {
            let page_words = locale.get(&format!("journal.page.{}", super::journal::page_name(*page))).to_owned();
            if want == "any" {
                locale.format("quest.goal.kinds_of", &[("page", &page_words)])
            } else {
                let which = if matches!(page, voxmar_engine::journal::Page::Animals | voxmar_engine::journal::Page::Monsters) { creature_words(locale, content, package, want) } else { item_words(locale, content, package, want) };
                locale.format("quest.goal.kinds_of_which", &[("page", &page_words), ("item", &which)])
            }
        }
        Goal::Level(skill, level) => {
            return locale.format("quest.goal.level", &[("skill", &skill_words(locale, content, skill)), ("level", level)]);
        }
        Goal::Have(item, _) => locale.format("quest.goal.have", &[("item", &item_words(locale, content, package, item))]),
        Goal::Home => return locale.get("quest.goal.home").to_owned(),
        Goal::Thriving(..) => locale.get("quest.goal.thriving").to_owned(),
        Goal::Met(kind, _) if kind == "Kinds" => locale.get("quest.goal.met_kinds").to_owned(),
        Goal::Met(kind, _) if kind == "any" => locale.get("quest.goal.met_any").to_owned(),
        Goal::Met(kind, _) => locale.format("quest.goal.met", &[("who", &locale.get_or(&format!("{}.title", def_key("wander", kind)), &title_from_id(kind)).to_owned())]),
        Goal::Prayed(prayer, _) => {
            let prayer = if prayer == "any" { locale.get("quest.any").to_owned() } else { locale.get_or(&def_key("prayer", prayer), &title_from_id(prayer)).to_owned() };
            locale.format("quest.goal.prayed", &[("prayer", &prayer)])
        }
        Goal::Offered(item, _) => locale.format("quest.goal.offered", &[("item", &item_words(locale, content, package, item))]),
        Goal::Opened(chest, _) => locale.format("quest.goal.opened", &[("item", &item_words(locale, content, package, chest))]),
        Goal::Fertilised(item, _) => locale.format("quest.goal.fertilised", &[("item", &item_words(locale, content, package, item))]),
        Goal::Ate(food, _) => locale.format("quest.goal.ate", &[("item", &item_words(locale, content, package, food))]),
        Goal::Laid(item, _) => locale.format("quest.goal.laid", &[("item", &item_words(locale, content, package, item))]),
        Goal::Watering(block, _) => locale.format("quest.goal.watering", &[("item", &item_words(locale, content, package, block))]),
        Goal::Felt(effect) => {
            let words = qualified(package, effect).iter().find_map(|name| content.effects.find(name)).map(|id| content.effects.get(id)).map_or_else(|| title_from_id(effect), |def| locale.get_or(&def_key("effect", &def.name), &def.title).to_owned());
            return locale.format("quest.goal.felt", &[("effect", &words)]);
        }
        Goal::Built(name, _) => match qualified(package, name).iter().find_map(|name| content.effects.step(name)) {
            // A step of a ladder (a home's) is reached, not built.
            Some(step) => locale.format("quest.goal.step", &[("step", &locale.get_or(&def_key("effect", &step.name), &step.title))]),
            None => locale.format("quest.goal.built", &[("item", &multiblock_words(locale, content, package, name))]),
        },
        Goal::Lit(name, _) => {
            let fuels = qualified(package, name).iter().find_map(|name| content.multiblocks.get(name)).map(|m| {
                m.fuels.iter().map(|&(item, count)| locale.format("multiblock.fuel_stack", &[("count", &count), ("item", &locale.block(content.blocks.def(item)))])).collect::<Vec<_>>().join(locale.get("multiblock.fuel_or"))
            }).unwrap_or_default();
            locale.format("quest.goal.lit", &[("item", &multiblock_words(locale, content, package, name)), ("fuels", &fuels)])
        },
        Goal::LookUp(item) => locale.format("quest.goal.lookup", &[("item", &item_words(locale, content, package, item))]),
    };
    if need > 1 { locale.format("quest.progress", &[("goal", &what), ("have", &have.min(need)), ("need", &need)]) } else { what }
}

/// A chapter's web: its quests, then those of other chapters that come first; where each
/// stands, in steps (its column, then its row); and how many columns and rows it takes.
pub(crate) struct Layout<'a> {
    pub quests: Vec<&'a QuestDef>,
    /// How many of `quests` are the chapter's own.
    pub ours: usize,
    pub cells: Vec<Vec2>,
    pub columns: usize,
    pub tallest: usize,
}

/// Lays out chapter `age`'s web: each quest a column past the furthest of those it comes
/// after (those from other chapters in the first), each column ordered by the rows of
/// those its quests come after, so lines cross as little as may be, and centred on the
/// tallest.
pub(crate) fn layout<'a>(content: &'a Content, age: &str) -> Layout<'a> {
    let mut quests: Vec<&QuestDef> = content.quests.iter().filter(|q| q.age == age).collect();
    let mut outside: Vec<&QuestDef> = Vec::new();
    for quest in &quests {
        for first in before(content, quest) {
            if first.age != age && !outside.iter().any(|o| o.name == first.name) {
                outside.push(first);
            }
        }
    }
    let ours = quests.len();
    quests.extend(outside);
    // Each quest's column: one past the furthest of those it comes after; those outside
    // the chapter stand in the first.
    let mut depth: Vec<usize> = vec![0; quests.len()];
    for _ in 0..quests.len() {
        for i in 0..ours {
            depth[i] = before(content, quests[i]).iter().filter_map(|first| quests.iter().position(|q| q.name == first.name)).map(|j| depth[j] + 1).max().unwrap_or(0);
        }
    }
    // Each column's quests in the order of the rows of those they come after, so lines
    // cross as little as may be; each column centred on the tallest.
    let columns = depth.iter().copied().max().map_or(1, |d| d + 1);
    let mut row: Vec<f32> = vec![0.0; quests.len()];
    let mut tallest = 1;
    for d in 0..columns {
        let mut column: Vec<usize> = (0..quests.len()).filter(|&i| depth[i] == d).collect();
        let key = |i: usize| -> f32 {
            let firsts: Vec<f32> = if i < ours { before(content, quests[i]).iter().filter_map(|first| quests.iter().position(|q| q.name == first.name)).map(|j| row[j]).collect() } else { Vec::new() };
            if firsts.is_empty() { f32::MAX } else { firsts.iter().sum::<f32>() / firsts.len() as f32 }
        };
        let keys: Vec<(usize, f32)> = column.iter().map(|&i| (i, key(i))).collect();
        column.sort_by(|a, b| {
            let (ka, kb) = (keys.iter().find(|k| k.0 == *a).unwrap().1, keys.iter().find(|k| k.0 == *b).unwrap().1);
            ka.total_cmp(&kb).then(a.cmp(b))
        });
        for (r, &i) in column.iter().enumerate() {
            row[i] = r as f32;
        }
        tallest = tallest.max(column.len());
    }
    for d in 0..columns {
        let count = depth.iter().filter(|&&x| x == d).count();
        let shift = (tallest - count) as f32 / 2.0;
        for i in (0..quests.len()).filter(|&i| depth[i] == d) {
            row[i] += shift;
        }
    }
    let cells = (0..quests.len()).map(|i| Vec2::new(depth[i] as f32, row[i])).collect();
    Layout { quests, ours, cells, columns, tallest }
}

/// A goal's line: its mark (met, or not yet) and its words.
fn goal_line(locale: &Locale, content: &Content, quest: &QuestDef, goal: &Goal, progress: (u32, u32), met: bool) -> String {
    let mark = locale.get(if met { "quest.mark.done" } else { "quest.mark.todo" });
    format!("{mark} {}", goal_words(locale, content, quest, goal, progress))
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

/// The quests a quest comes after, each way it may (`a|b` is either).
fn before<'a>(content: &'a Content, quest: &QuestDef) -> Vec<&'a QuestDef> {
    quest.after.iter().flat_map(|a| a.split('|')).filter_map(|name| content.quests.get(name)).collect()
}

#[derive(Component)]
struct QuestScreen;
#[derive(Component)]
struct ChapterButton(usize);
#[derive(Component)]
struct Chapters;
#[derive(Component)]
pub(crate) struct Web;
/// The web's quests and lines, inside the well, at its margins (which centre a web smaller
/// than the well).
#[derive(Component)]
pub(crate) struct Canvas {
    margin: Vec2,
}
/// A line from one quest to the next, between their centres on the canvas, drawn only as
/// far as the well shows it: the interface clips only what is square to it, so a slanted
/// line is cut to the well here instead of poking out of the book.
#[derive(Component)]
pub(crate) struct WebLine {
    from: Vec2,
    to: Vec2,
}
#[derive(Component)]
struct QuestNode(String);
#[derive(Component)]
struct Detail;
#[derive(Component)]
struct PinButton;
/// Claims the picked quest's rewards.
#[derive(Component)]
pub(crate) struct ClaimButton;
/// An arrow turning to another way of making what a goal asks for.
#[derive(Component)]
struct WayStep {
    key: String,
    step: i32,
    of: usize,
}

/// How much of the window the book takes across and down, in percent: most of it, growing
/// with the window.
const SHARE: Vec2 = Vec2::new(92.0, 76.0);
/// The chapters' column, and the sidebar at the right with the picked quest's page.
const CHAPTERS: f32 = 80.0 * PX;
const SIDEBAR: f32 = 180.0 * PX;
/// A quest on the web: its cell, and the item or skill drawn in it at one and a half times
/// a slot's (each of its 16 texels three screen pixels), small enough for a big chapter to
/// show much of itself at once.
const CELL: f32 = 28.0 * PX;
const ICON: f32 = 24.0 * PX;
/// The quests' least and most step across and down the web; it spreads to fill its well
/// between them, and a web too big for the well is a canvas dragged about inside it.
const STEP_LEAST: Vec2 = Vec2::new(CELL + 12.0 * PX, CELL + 6.0 * PX);
const STEP_MOST: Vec2 = Vec2::new(CELL + 90.0 * PX, CELL + 40.0 * PX);
/// How far the picked quest's glow reaches past it.
const GLOW: f32 = 3.0 * PX;
/// How thick the lines between quests are.
const LINE: f32 = 2.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((QuestScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        let (mut card, image) = panel(0.0);
        card.width = Val::Percent(SHARE.x);
        card.height = Val::Percent(SHARE.y);
        root.spawn((card, image)).with_children(|card| {
            card.spawn(Node { width: Val::Percent(100.0), flex_direction: FlexDirection::Column, ..default() }).with_children(|heading| spawn_heading(heading, font, "screen.quests"));
            // The chapters, the web filling what is left, and the picked quest's page at the
            // right.
            card.spawn(Node { flex_grow: 1.0, min_height: Val::ZERO, column_gap: Val::Px(space::GAP), ..default() }).with_children(|columns| {
                let chapters = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), width: Val::Px(CHAPTERS), flex_shrink: 0.0, overflow: Overflow::scroll_y(), ..default() };
                columns.spawn((chapters, Chapters));
                let web = Node {
                    flex_grow: 1.0,
                    flex_basis: Val::ZERO,
                    min_width: Val::ZERO,
                    overflow: Overflow::scroll(),
                    padding: UiRect::all(Val::Px(space::GAP)),
                    ..default()
                };
                columns.spawn((Web, ScrollList, web, Piece::Well.image()));
                columns.spawn(Node { width: Val::Px(SIDEBAR), flex_shrink: 0.0, flex_direction: FlexDirection::Column, ..default() }).with_children(|side| {
                    let detail = Node { flex_grow: 1.0, min_height: Val::ZERO, flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), overflow: Overflow::scroll_y(), ..default() };
                    let detail = side.spawn((Detail, ScrollList, detail)).id();
                    spawn_scroll_more(side, font, detail);
                });
            });
            spawn_rule(card, space::GAP);
            card.spawn((Localized("quest.hint"), text(font, "", BODY, MUTED)));
        });
        spawn_hotkeys(root, GameScreen::Quests);
    });
}

fn show(ui: Res<UiState>, mut root: Query<&mut Visibility, With<QuestScreen>>) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.quests_open && !ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

#[allow(clippy::type_complexity)]
fn pick(
    launch: Res<Launch>,
    chapters_pressed: Query<(&ChapterButton, &Interaction), Changed<Interaction>>,
    nodes: Query<(&QuestNode, &Interaction), Changed<Interaction>>,
    pins: Query<&Interaction, (Changed<Interaction>, With<PinButton>)>,
    mut view: ResMut<QuestView>,
    mut settings: ResMut<Settings>,
) {
    let content = &launch.content;
    for (chapter, interaction) in &chapters_pressed {
        if *interaction == Interaction::Pressed && view.chapter != chapter.0 {
            view.chapter = chapter.0;
            view.picked = None;
        }
    }
    for (node, interaction) in &nodes {
        if *interaction == Interaction::Pressed
            && let Some(quest) = content.quests.get(&node.0)
        {
            // A quest from another chapter opens there.
            view.chapter = chapters(content).iter().position(|age| *age == quest.age).unwrap_or(view.chapter);
            view.picked = Some(quest.name.clone());
        }
    }
    if pins.iter().any(|i| *i == Interaction::Pressed)
        && let Some(name) = view.picked.clone()
    {
        toggle_pin(&mut settings, content, &name);
    }
}

/// The Claim button asks the server to pay the picked quest's rewards; the quest screen
/// hears the answer with the quests' next states.
fn claim(buttons: Query<&Interaction, (Changed<Interaction>, With<ClaimButton>)>, view: Res<QuestView>, player: Res<LocalPlayer>, net: Option<Res<Net>>) {
    if buttons.iter().any(|i| *i == Interaction::Pressed)
        && let (Some(name), Some(net)) = (view.picked.as_deref(), net)
        && claimable(&player, name)
    {
        net.claim_quest(name);
    }
}

/// The sound of a quest's rewards claimed (coins), told by the server's next states against
/// the last (not on joining, when the first states arrive). A quest finished is one of the
/// HUD's moments, which plays its jingle (`celebrate`).
fn hear_quests(player: Res<LocalPlayer>, mut sounds: MessageWriter<UiSound>, mut last: Local<(u32, Vec<QuestState>)>) {
    // Cleared on joining: the next states are the first of a session.
    if player.quests.is_empty() {
        *last = (player.quests_rev, Vec::new());
        return;
    }
    if last.0 == player.quests_rev {
        return;
    }
    let (_, claimed) = quest_changes(&last.1, &player.quests);
    if claimed {
        sounds.write(UiSound::QuestClaim);
    }
    *last = (player.quests_rev, player.quests.clone());
}

/// Whether, from `before` to `now`, a quest was finished, and whether one's rewards were
/// claimed. The first states of a session (nothing `before`) are what the
/// player already had.
fn quest_changes(before: &[QuestState], now: &[QuestState]) -> (bool, bool) {
    if before.is_empty() {
        return (false, false);
    }
    let was = |name: &str| before.iter().find(|s| s.name == name);
    let done = now.iter().any(|s| s.done && was(&s.name).is_some_and(|w| !w.done));
    let claimed = now.iter().any(|s| s.claimed && was(&s.name).is_some_and(|w| w.done && !w.claimed));
    (done, claimed)
}

/// The arrows by an inline recipe turn to its other ways.
fn turn_ways(steps: Query<(&WayStep, &Interaction), Changed<Interaction>>, mut view: ResMut<QuestView>) {
    for (way, interaction) in &steps {
        if *interaction == Interaction::Pressed {
            let now = view.ways.get(&way.key).copied().unwrap_or(0);
            view.ways.insert(way.key.clone(), (now as i32 + way.step).rem_euclid(way.of as i32) as usize);
        }
    }
}

/// Pins a quest, or unpins it; pinning past the tracker's room does nothing.
pub fn toggle_pin(settings: &mut Settings, content: &Content, name: &str) {
    if toggle(&mut settings.pinned_quests, content, name) {
        settings.save();
    }
}

/// Pins `name` or unpins it in `pins`: whether anything changed. A quest the content does
/// not have, or one past the tracker's room, is not pinned.
fn toggle(pins: &mut Vec<String>, content: &Content, name: &str) -> bool {
    if let Some(i) = pins.iter().position(|p| p == name) {
        pins.remove(i);
    } else if pins.len() < content.quests.tracker().pins && content.quests.get(name).is_some() {
        pins.push(name.to_owned());
    } else {
        return false;
    }
    true
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
    viewer: Res<super::recipes::RecipeViewer>,
    chapters_column: Query<Entity, With<Chapters>>,
    mut web: Query<(Entity, &ComputedNode, &mut ScrollPosition), With<Web>>,
    detail: Query<Entity, With<Detail>>,
    mut drawn: Local<Option<(QuestView, u32, Vec<String>, bool, IVec2)>>,
) {
    if !ui.quests_open {
        *drawn = None;
        return;
    }
    // The web's room inside its well, as laid out for this window: drawn again when the
    // window changes it.
    let room = web.single().map_or(Vec2::ZERO, |(_, computed, _)| (computed.size() * computed.inverse_scale_factor() - Vec2::splat(2.0 * space::GAP)).max(Vec2::ZERO));
    let wanted = (view.clone(), player.quests_rev, settings.pinned_quests.clone(), viewer.book().is_some(), room.as_ivec2());
    if drawn.as_ref() == Some(&wanted) && !locale.is_changed() {
        return;
    }
    // Opened, or turned to another chapter, the canvas comes to the quest picked: again
    // once the well has its size, for the book's first frame has none to centre in.
    let arrived = drawn.as_ref().is_none_or(|(was, .., was_room)| was.chapter != view.chapter || *was_room == IVec2::ZERO);
    *drawn = Some(wanted);
    let content = &launch.content;
    let font = &font.0;
    let ages = chapters(content);
    if let Ok(column) = chapters_column.single() {
        commands.entity(column).despawn_children().with_children(|column| {
            for (i, age) in ages.iter().enumerate() {
                let quests: Vec<&QuestDef> = content.quests.iter().filter(|q| &q.age == age).collect();
                let done = quests.iter().filter(|q| standing(content, &player, q) == Standing::Done).count();
                let waiting = quests.iter().any(|q| claimable(&player, &q.name));
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
                    let count = locale.format("quest.chapter.count", &[("done", &done), ("all", &quests.len())]);
                    // A gold "!" marks a chapter with rewards waiting to be claimed.
                    button.spawn((text(font, if waiting { format!("! {count}") } else { count }, BODY, if waiting { GOLD } else { MUTED }), TightLine));
                });
            }
        });
    }
    let Some(age) = ages.get(view.chapter) else { return };
    let Layout { quests, ours, cells, columns, tallest } = layout(content, age);
    // Spread to fill the well, within reason; a web too big for it scrolls.
    let step = Vec2::new(room.x / columns as f32, room.y / tallest as f32).clamp(STEP_LEAST, STEP_MOST);
    let place = |i: usize| cells[i] * step + (step - Vec2::splat(CELL)) / 2.0;
    let centre = |i: usize| place(i) + Vec2::splat(CELL / 2.0);
    if let Ok((web, _, mut scroll)) = web.single_mut() {
        let size = Vec2::new(columns as f32, tallest as f32) * step;
        // Centred in the well by its margins; a web too big for the well has none that way,
        // and is dragged about in it rather than cut off.
        let margin = ((room - size) / 2.0).max(Vec2::ZERO);
        if arrived {
            let at = view.picked.as_deref().and_then(|name| quests.iter().position(|q| q.name == name)).map_or(Vec2::ZERO, |i| centre(i) - room / 2.0);
            scroll.0 = at.clamp(Vec2::ZERO, (size - room).max(Vec2::ZERO));
        }
        commands.entity(web).despawn_children().with_children(|web| {
            if ours == 0 {
                web.spawn((Localized("quest.empty"), text(font, "", BODY, MUTED)));
                return;
            }
            let canvas = Node { width: Val::Px(size.x), height: Val::Px(size.y), flex_shrink: 0.0, margin: UiRect { left: Val::Px(margin.x), top: Val::Px(margin.y), ..default() }, ..default() };
            web.spawn((canvas, Canvas { margin })).with_children(|area| {
                // The lines first, under the quests; each is fitted to what the well shows
                // of it as the canvas moves (`fit_lines`).
                for i in 0..ours {
                    for first in before(content, quests[i]) {
                        let Some(j) = quests.iter().position(|q| q.name == first.name) else { continue };
                        let done = standing(content, &player, quests[j]) == Standing::Done;
                        let line = Node { position_type: PositionType::Absolute, display: Display::None, height: Val::Px(LINE), ..default() };
                        area.spawn((line, UiTransform::default(), WebLine { from: centre(j), to: centre(i) }, BackgroundColor(if done { GOLD } else { MUTED.with_alpha(0.7) })));
                    }
                }
                for (i, quest) in quests.iter().enumerate() {
                    let standing = standing(content, &player, quest);
                    let at = place(i);
                    let node = Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(at.x),
                        top: Val::Px(at.y),
                        width: Val::Px(CELL),
                        height: Val::Px(CELL),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    };
                    // Gold when done, lit while open, plain and dim while waiting.
                    let piece = match standing {
                        Standing::Done => Piece::Frame,
                        Standing::Open => Piece::WellFocus,
                        Standing::Waiting => Piece::Well,
                    };
                    let tint = if standing == Standing::Waiting || i >= ours { Color::srgba(1.0, 1.0, 1.0, 0.35) } else { Color::WHITE };
                    let picked = view.picked.as_deref() == Some(quest.name.as_str());
                    // A finished quest with rewards to claim glows gold; the quest picked glows
                    // in the accent, whatever it stands at, under its ring.
                    if claimable(&player, &quest.name) {
                        let glow = Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(at.x - GLOW),
                            top: Val::Px(at.y - GLOW),
                            width: Val::Px(CELL + 2.0 * GLOW),
                            height: Val::Px(CELL + 2.0 * GLOW),
                            ..default()
                        };
                        area.spawn((glow, BackgroundColor(GOLD.with_alpha(0.55))));
                    }
                    if picked {
                        let glow = Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(at.x - GLOW),
                            top: Val::Px(at.y - GLOW),
                            width: Val::Px(CELL + 2.0 * GLOW),
                            height: Val::Px(CELL + 2.0 * GLOW),
                            ..default()
                        };
                        area.spawn((glow, BackgroundColor(ACCENT.with_alpha(0.45))));
                    }
                    area.spawn((Button, crate::ui::widgets::HoverLight, QuestNode(quest.name.clone()), node, piece.image())).with_children(|cell| {
                        cell.spawn((Node { width: Val::Px(ICON), height: Val::Px(ICON), ..default() }, ImageNode::new(quest_icon(content, &images, quest)).with_color(tint)));
                        // The quest picked, ringed.
                        if picked {
                            let ring = Node { position_type: PositionType::Absolute, left: Val::Px(-2.0 * PX), top: Val::Px(-2.0 * PX), width: Val::Px(CELL + 4.0 * PX), height: Val::Px(CELL + 4.0 * PX), ..default() };
                            cell.spawn((ring, Piece::Selection.image()));
                        }
                    });
                }
            });
        });
    }
    // The picked quest: why first, what comes first, its routes and goals, its rewards.
    if let Ok(detail) = detail.single() {
        commands.entity(detail).despawn_children().with_children(|detail| {
            let Some(quest) = view.picked.as_deref().and_then(|name| content.quests.get(name)) else {
                detail.spawn((Localized("quest.pick"), text(font, "", BODY, MUTED)));
                return;
            };
            let standing = standing(content, &player, quest);
            let state = state(&player, &quest.name);
            // Its name and where it stands, then what can be done with it: claim its rewards,
            // pin it.
            detail.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexStart, column_gap: Val::Px(space::GAP), flex_shrink: 0.0, ..default() }).with_children(|top| {
                top.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, shadowed(font, title(&locale, quest), BODY, GOLD)));
                let status = match standing {
                    Standing::Done if claimable(&player, &quest.name) => (locale.get("quest.status.ready").to_owned(), GOLD),
                    Standing::Done => (locale.get("quest.status.done").to_owned(), GOLD),
                    Standing::Open => (locale.get("quest.status.open").to_owned(), ACCENT),
                    Standing::Waiting => (locale.get("quest.status.waiting").to_owned(), MUTED),
                };
                top.spawn((text(font, status.0, BODY, status.1), TightLine));
            });
            let ready = claimable(&player, &quest.name);
            // Only an open quest is followed on screen.
            let pinned = settings.pinned_quests.contains(&quest.name);
            let pinnable = pinned || (standing == Standing::Open && settings.pinned_quests.len() < content.quests.tracker().pins);
            if ready || pinnable {
                detail.spawn(Node { flex_shrink: 0.0, ..button_row(space::HAIR) }).with_children(|row| {
                    if ready {
                        spawn_button(row, font, "quest.claim", ButtonStyle::Primary, ClaimButton);
                    }
                    if pinnable {
                        spawn_button(row, font, if pinned { "quest.unpin" } else { "quest.pin" }, ButtonStyle::Secondary, PinButton);
                    }
                });
            }
            detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, why(&locale, quest), BODY, SOFT)));
            // What it gives, one line each: an item with its count, or experience in a skill
            // with the skill's mark.
            if !quest.rewards.is_empty() {
                let claimed = state.is_some_and(|s| s.claimed);
                detail.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, Localized(if claimed { "quest.rewards.claimed" } else { "quest.rewards" }), shadowed(font, "", BODY, if claimed { MUTED } else { ACCENT })));
                for reward in &quest.rewards {
                    detail.spawn(Node { flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() }).with_children(|row| {
                        let words = match reward {
                            Reward::Item(item, count) => {
                                super::recipes::item_cell(row, font, &images, item_id(content, package(quest), item), (*count > 1).then(|| count.to_string()));
                                locale.format("quest.reward.item", &[("count", count), ("item", &item_words(&locale, content, package(quest), item))])
                            }
                            Reward::Xp(skill, xp) => {
                                let mark = skill_id(content, skill).and_then(|id| images.skills.get(id.0 as usize).cloned()).unwrap_or_default();
                                let inset = (SLOT - 16.0 * PX) / 2.0;
                                row.spawn((Node { width: Val::Px(SLOT), height: Val::Px(SLOT), padding: UiRect::all(Val::Px(inset)), flex_shrink: 0.0, ..default() }, Piece::Well.image())).with_children(|cell| {
                                    cell.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() }, ImageNode::new(mark)));
                                });
                                locale.format("quest.reward.xp", &[("xp", &(*xp as u32)), ("skill", &skill_words(&locale, content, skill))])
                            }
                        };
                        row.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, text(font, words, BODY, if claimed { MUTED } else { TEXT })));
                    });
                }
            }
            let firsts = before(content, quest);
            if !firsts.is_empty() {
                let names: Vec<String> = firsts.iter().map(|q| title(&locale, q)).collect();
                let line = locale.format("quest.after", &[("list", &names.join(locale.get("language.comma")))]);
                let color = if standing == Standing::Waiting { TEXT } else { MUTED };
                detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, line, BODY, color)));
            }
            // What it asks: its goals, or its routes when there are several.
            let asks = if quest.tasks.len() > 1 { "quest.routes" } else { "quest.goals" };
            detail.spawn((Node { flex_shrink: 0.0, margin: UiRect::top(Val::Px(space::INNER)), ..default() }, Localized(asks), shadowed(font, "", BODY, ACCENT)));
            for (t, task) in quest.tasks.iter().enumerate() {
                if quest.tasks.len() > 1 {
                    detail.spawn((Node { flex_shrink: 0.0, ..default() }, text(font, locale.format("quest.route", &[("n", &(t + 1))]), BODY, MUTED)));
                }
                for (g, goal) in task.iter().enumerate() {
                    let progress = state.and_then(|s| s.tasks.get(t)?.get(g).copied()).unwrap_or((0, 1));
                    let met = progress.0 >= progress.1 || standing == Standing::Done;
                    // Waiting, its goals are what is ahead, not progress under way.
                    let color = match (standing, met) {
                        (Standing::Waiting, _) => MUTED,
                        (_, true) => GOLD,
                        _ => TEXT,
                    };
                    let line = text(font, goal_line(&locale, content, quest, goal, progress, met), BODY, color);
                    // The item it asks for, to click for its recipes (a right click, its uses).
                    let asked = match goal {
                        Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::CookAt(name, _, _) | Goal::Have(name, _) | Goal::Pick(name, _) | Goal::Mine(name, _) | Goal::Catch(name, _) | Goal::LookUp(name) => item_id(content, package(quest), name),
                        _ => None,
                    };
                    let Some(item) = asked else {
                        detail.spawn((Node { flex_shrink: 0.0, ..default() }, line));
                        continue;
                    };
                    detail.spawn(Node { flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() }).with_children(|row| {
                        super::recipes::item_cell(row, font, &images, Some(item), None);
                        row.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, line));
                    });
                    // How to make it, the way the recipe viewer draws it, for what a goal
                    // makes.
                    if matches!(goal, Goal::Craft(..) | Goal::Cook(..) | Goal::CookAt(..) | Goal::Have(..))
                        && let Some(book) = viewer.book()
                    {
                        let ways = book.making(item);
                        if !ways.is_empty() {
                            let key = format!("{}/{t}/{g}", quest.name);
                            let shown = view.ways.get(&key).copied().unwrap_or(0) % ways.len();
                            let entry = ways[shown];
                            way_card(detail, font, &images, content, &locale, entry, (key, shown, ways.len()));
                        }
                    }
                }
            }
        });
    }
}

/// The part of the line from `from` to `to` inside `view`, if any (Liang and Barsky's way:
/// the line as `from + t (to - from)`, `t` narrowed by each edge in turn).
pub(crate) fn clip_line(from: Vec2, to: Vec2, view: Rect) -> Option<(Vec2, Vec2)> {
    let d = to - from;
    let (mut enter, mut leave) = (0.0_f32, 1.0_f32);
    for (p, q) in [(-d.x, from.x - view.min.x), (d.x, view.max.x - from.x), (-d.y, from.y - view.min.y), (d.y, view.max.y - from.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            enter = enter.max(q / p);
        } else {
            leave = leave.min(q / p);
        }
    }
    (enter < leave).then(|| (from + d * enter, from + d * leave))
}

/// Fits each line to what the well shows of the canvas, as it is dragged about or drawn
/// anew: the part inside, turned along the line, or nothing. The well is narrowed by half
/// a line's thickness so a cut end's corners stay in it too.
fn fit_lines(web: Query<(Ref<ComputedNode>, Ref<ScrollPosition>), With<Web>>, canvas: Query<&Canvas>, mut lines: Query<(Ref<WebLine>, &mut Node, &mut UiTransform)>) {
    let Ok((computed, scroll)) = web.single() else { return };
    if !computed.is_changed() && !scroll.is_changed() && !lines.iter().any(|(line, ..)| line.is_added()) {
        return;
    }
    let Ok(canvas) = canvas.single() else { return };
    // Where the canvas stands this frame: as scrolled now, within its edges as the layout
    // will hold it.
    let scale = computed.inverse_scale_factor();
    let room = (computed.size() * scale - Vec2::splat(2.0 * space::GAP)).max(Vec2::ZERO);
    let max = (computed.content_size() - computed.size()).max(Vec2::ZERO) * scale;
    let corner = scroll.0.clamp(Vec2::ZERO, max).floor() - canvas.margin;
    let view = Rect::from_corners(corner, corner + room).inflate(-LINE / 2.0);
    for (line, mut node, mut turn) in &mut lines {
        let Some((from, to)) = clip_line(line.from, line.to, view) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        let (length, middle) = (from.distance(to), (from + to) / 2.0);
        let fitted = Node { display: Display::Flex, left: Val::Px(middle.x - length / 2.0), top: Val::Px(middle.y - LINE / 2.0), width: Val::Px(length), ..node.clone() };
        if *node != fitted {
            *node = fitted;
        }
        let rotation = Rot2::radians((line.to - line.from).to_angle());
        if turn.rotation != rotation {
            turn.rotation = rotation;
        }
    }
}

/// Holds the web's scroll within what its layout lets it move, as dragging, the wheel and
/// `fit_lines` reckon it: the canvas centred on a quest by `draw` may ask a little past it
/// (the well's padding), and a first drag from there would jump.
fn keep_web_in_range(mut web: Query<(&ComputedNode, &mut ScrollPosition), With<Web>>) {
    let Ok((computed, mut scroll)) = web.single_mut() else { return };
    if computed.content_size() == Vec2::ZERO {
        return;
    }
    let max = (computed.content_size() - computed.size()).max(Vec2::ZERO) * computed.inverse_scale_factor();
    let held = scroll.0.clamp(Vec2::ZERO, max);
    if scroll.0 != held {
        scroll.0 = held;
    }
}

/// Where a drag of the web began, and where the pointer was last.
#[derive(Default)]
struct Drag {
    held: bool,
    last: Option<Vec2>,
}

/// Dragging the web's empty canvas with the left button held moves it under the pointer,
/// within its edges, so the web never leaves the well; a press on a quest picks it instead.
/// The pointer moves in the window's points, the canvas in the interface's, `UiScale` of
/// them to a point.
fn drag_web(
    buttons: Res<ButtonInput<MouseButton>>,
    scale: Res<UiScale>,
    mut moved: MessageReader<CursorMoved>,
    quests: Query<&Interaction, With<QuestNode>>,
    mut web: Query<(&mut ScrollPosition, &bevy::ui::RelativeCursorPosition, &ComputedNode), With<Web>>,
    mut drag: Local<Drag>,
) {
    let mut delta = Vec2::ZERO;
    for event in moved.read() {
        if let Some(last) = drag.last {
            delta += event.position - last;
        }
        drag.last = Some(event.position);
    }
    let Ok((mut scroll, cursor, computed)) = web.single_mut() else { return };
    if buttons.just_pressed(MouseButton::Left) {
        drag.held = cursor.cursor_over() && quests.iter().all(|i| *i == Interaction::None);
    }
    if !buttons.pressed(MouseButton::Left) {
        drag.held = false;
    }
    if drag.held && delta != Vec2::ZERO {
        let max = (computed.content_size() - computed.size()).max(Vec2::ZERO) * computed.inverse_scale_factor();
        let next = (scroll.0 - delta / scale.0).clamp(Vec2::ZERO, max);
        if scroll.0 != next {
            scroll.0 = next;
        }
    }
}

/// One way of making an item, small, under the goal that asks for it: what goes in, an
/// arrow, what comes out, and, where there are several ways, arrows to turn to the next
/// with "1/3" between them.
fn way_card(detail: &mut ChildSpawnerCommands, font: &Handle<Font>, images: &HudImages, content: &Content, locale: &Locale, entry: &voxmar_engine::recipe_book::Entry, (key, shown, of): (String, usize, usize)) {
    let card = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), padding: UiRect::all(Val::Px(space::HAIR)), flex_shrink: 0.0, align_self: AlignSelf::FlexStart, ..default() };
    detail.spawn((card, Piece::Well.image())).with_children(|card| {
        super::recipes::way_inputs(card, font, images, content, &entry.method, None, 0);
        card.spawn(shadowed(font, "→", BODY, GOLD));
        let (low, high) = entry.count;
        super::recipes::item_cell(card, font, images, Some(entry.output), (high > 1 && low == high).then(|| high.to_string()));
        if of > 1 {
            let arrow = |card: &mut ChildSpawnerCommands, label: &str, step: i32| {
                let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() };
                card.spawn((Button, crate::ui::widgets::HoverLight, WayStep { key: key.clone(), step, of }, node, Piece::Well.image())).with_children(|b| {
                    b.spawn((shadowed(font, label.to_owned(), BODY, TEXT), TightLine));
                });
            };
            arrow(card, "◂", -1);
            card.spawn((text(font, locale.format("recipes.page", &[("page", &(shown + 1)), ("pages", &of)]), BODY, SOFT), TightLine));
            arrow(card, "▸", 1);
        }
    });
}

/// The pinned quests at the right of the screen.
#[derive(Component)]
pub struct Tracker;
/// An × that unpins a quest from the tracker itself, while the pointer is free.
#[derive(Component)]
struct Unpin(String);

fn spawn_tracker(mut commands: Commands) {
    // High at the right, clear of the skill toasts and the level plate below it.
    let column = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(space::SECTION),
        top: Val::Percent(8.0),
        width: Val::Px(130.0 * PX),
        max_height: Val::Percent(28.0),
        overflow: Overflow::clip(),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(space::GAP),
        ..default()
    };
    commands.spawn((column, Tracker, GlobalZIndex(0)));
}

/// When each pinned quest was seen done, and how long it has shown so since.
#[derive(Default)]
struct Finished(Vec<(String, f32)>);

/// Unpins a quest once it has shown done on screen for a moment, and pins what follows it
/// (`follow_ons`) where the tracker's data asks.
fn retire(time: Res<Time>, player: Res<LocalPlayer>, launch: Res<Launch>, tracker: Query<&Visibility, With<Tracker>>, mut settings: ResMut<Settings>, mut finished: Local<Finished>) {
    let shown = tracker.iter().any(|v| *v != Visibility::Hidden);
    let done = |name: &str| state(&player, name).is_some_and(|s| s.done);
    let content = &launch.content;
    let gone = finished.tick(&settings.pinned_quests, done, shown, time.delta_secs(), content.quests.tracker().done_stays);
    if !gone.is_empty() {
        settings.pinned_quests.retain(|p| !gone.contains(p));
        if content.quests.tracker().follow_on {
            let next = follow_ons(content, &settings.pinned_quests, &gone, &done);
            settings.pinned_quests.extend(next);
        }
        settings.save();
    }
}

/// What to pin after the quests `gone` drop off done, beside those still `pinned`, as far
/// as the tracker has room: the open quests that come after one of them, in the book's
/// order; opening none, and leaving nothing pinned, the first open quest of the age the
/// first of them is in, so the next step is always on screen.
pub(crate) fn follow_ons(content: &Content, pinned: &[String], gone: &[String], done: &dyn Fn(&str) -> bool) -> Vec<String> {
    let room = content.quests.tracker().pins.saturating_sub(pinned.len());
    let open = |quest: &&QuestDef| !done(&quest.name) && content.quests.open(quest, done) && !pinned.contains(&quest.name);
    let mut next: Vec<String> = content.quests.iter().filter(open).filter(|quest| before(content, quest).iter().any(|first| gone.contains(&first.name))).map(|quest| quest.name.clone()).take(room).collect();
    if next.is_empty()
        && pinned.is_empty()
        && let Some(age) = gone.first().and_then(|name| content.quests.get(name)).map(|quest| quest.age.clone())
    {
        next.extend(content.quests.iter().filter(open).find(|quest| quest.age == age).map(|quest| quest.name.clone()));
    }
    next
}

impl Finished {
    /// Counts `seconds` for each pinned quest done, while the tracker is `shown`: those
    /// shown done longer than `stays`, which then drop off. One unpinned meanwhile (its ×)
    /// is forgotten, so it never drops off and pins what follows it.
    fn tick(&mut self, pinned: &[String], done: impl Fn(&str) -> bool, shown: bool, seconds: f32, stays: f32) -> Vec<String> {
        self.0.retain(|(n, _)| pinned.contains(n));
        for name in pinned {
            if done(name) && !self.0.iter().any(|(n, _)| n == name) {
                self.0.push((name.clone(), 0.0));
            }
        }
        if shown {
            for (_, seen) in &mut self.0 {
                *seen += seconds;
            }
        }
        let gone: Vec<String> = self.0.iter().filter(|(_, seen)| *seen > stays).map(|(n, _)| n.clone()).collect();
        self.0.retain(|(n, _)| !gone.contains(n));
        gone
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
    mut drawn: Local<Option<(Vec<String>, u32)>>,
) {
    let content = &launch.content;
    for (unpin, interaction) in &unpins {
        if *interaction == Interaction::Pressed {
            toggle_pin(&mut settings, content, &unpin.0);
        }
    }
    // Out of the way of a screen, but for the quest book itself.
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open || ui.recipes_open || player.dead;
    if let Ok(mut shown) = visibility.single_mut() {
        shown.set_if_neq(if screen || settings.pinned_quests.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
    }
    let wanted = (settings.pinned_quests.clone(), player.quests_rev);
    if drawn.as_ref() == Some(&wanted) && !locale.is_changed() {
        return;
    }
    *drawn = Some(wanted);
    let Ok(tracker) = tracker.single() else { return };
    let font = &font.0;
    let backing = SHADOW.with_alpha(content.quests.tracker().backing_alpha);
    commands.entity(tracker).despawn_children().with_children(|tracker| {
        for name in &settings.pinned_quests {
            let Some(quest) = content.quests.get(name) else { continue };
            let state = state(&player, name);
            let done = state.is_some_and(|s| s.done);
            let card = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(PX), padding: UiRect::all(Val::Px(space::INNER)), flex_shrink: 0.0, ..default() };
            tracker.spawn((card, BackgroundColor(backing))).with_children(|card| {
                card.spawn(Node { justify_content: JustifyContent::SpaceBetween, column_gap: Val::Px(space::INNER), ..default() }).with_children(|top| {
                    top.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, shadowed(font, title(&locale, quest), BODY, GOLD)));
                    top.spawn((Button, Unpin(name.clone()), Node { padding: UiRect::horizontal(Val::Px(PX)), ..default() })).with_child((shadowed(font, "×", BODY, MUTED), TightLine));
                });
                if done {
                    let key = if claimable(&player, name) { "quest.tracker.ready" } else { "quest.tracker.done" };
                    card.spawn((shadowed(font, locale.get(key).to_owned(), BODY, GOLD), TrackerLine));
                    return;
                }
                for (goal, progress) in best_task(quest, state) {
                    let met = progress.0 >= progress.1;
                    card.spawn((shadowed(font, goal_line(&locale, content, quest, goal, progress, met), BODY, if met { GOLD } else { TEXT }), TrackerLine));
                }
            });
        }
    });
}

/// A line of the tracker: a goal's progress, or a quest done (for the gate to read).
#[derive(Component)]
pub struct TrackerLine;

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
    }

    /// Every name a quest gives (items, creatures, structures, skills) is one the content
    /// has, so no page falls back to a made-up name; and every quest has a picture.
    #[test]
    fn every_quest_names_what_the_content_has() {
        let content = content();
        let mut wrong = Vec::new();
        for quest in content.quests.iter() {
            let package = package(quest);
            let item = |name: &str| name == "any" || name.starts_with("tag:") || item_id(&content, package, name).is_some();
            for goal in quest.tasks.iter().flatten() {
                let known = match goal {
                    Goal::Craft(name, _) | Goal::Cook(name, _) | Goal::CookAt(name, _, _) | Goal::Have(name, _) | Goal::Pick(name, _) | Goal::Mine(name, _) | Goal::LookUp(name) => item(name),
                    Goal::Catch(name, _) => name == "Kinds" || item(name),
                    Goal::Kinds(page, name, _) => match page {
                        voxmar_engine::journal::Page::Animals | voxmar_engine::journal::Page::Monsters => name == "any" || qualified(package, name).iter().any(|name| content.creatures.find(name).is_some()),
                        voxmar_engine::journal::Page::Plants | voxmar_engine::journal::Page::Ores | voxmar_engine::journal::Page::Fish => item(name),
                        _ => name == "any",
                    },
                    Goal::Kill(name, _) | Goal::Tame(name, _) | Goal::Lured(name, _) => name == "any" || qualified(package, name).iter().any(|name| content.creatures.find(name).is_some()),
                    Goal::Built(name, _) => qualified(package, name).iter().any(|name| content.multiblocks.get(name).is_some() || content.effects.step(name).is_some()),
                    Goal::Lit(name, _) => qualified(package, name).iter().any(|name| content.multiblocks.get(name).is_some()),
                    Goal::Level(skill, _) => skill_id(&content, skill).is_some(),
                    _ => true,
                };
                if !known {
                    wrong.push(format!("{}: {goal:?}", quest.name));
                }
            }
            for reward in &quest.rewards {
                let known = match reward {
                    Reward::Xp(skill, _) => skill_id(&content, skill).is_some(),
                    Reward::Item(name, _) => item(name),
                };
                if !known {
                    wrong.push(format!("{}: {reward:?}", quest.name));
                }
            }
            let pictured = quest.rewards.iter().any(|r| matches!(r, Reward::Item(..) | Reward::Xp(..)))
                || quest.tasks.iter().flatten().any(|g| matches!(g, Goal::Craft(..) | Goal::Cook(..) | Goal::CookAt(..) | Goal::Have(..) | Goal::Pick(..) | Goal::Mine(..) | Goal::Catch(..) | Goal::Level(..)));
            if !pictured {
                wrong.push(format!("{}: nothing to draw it with", quest.name));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// The chimes follow the server's states: one as a quest turns done, another as a done
    /// one turns claimed, and none for the states a session starts with.
    #[test]
    fn quest_sounds_follow_the_states_the_server_sends() {
        let state = |name: &str, done: bool, claimed: bool| QuestState { name: name.into(), done, claimed, tasks: Vec::new() };
        let start = vec![state("a", true, true), state("b", false, false), state("c", true, false)];
        assert_eq!(quest_changes(&[], &start), (false, false), "joining is not a quest finished");
        assert_eq!(quest_changes(&start, &start), (false, false));
        let finished = vec![state("a", true, true), state("b", true, false), state("c", true, false)];
        assert_eq!(quest_changes(&start, &finished), (true, false));
        let claimed = vec![state("a", true, true), state("b", true, false), state("c", true, true)];
        assert_eq!(quest_changes(&finished, &claimed), (false, true));
        // Finished and claimed at once (it pays nothing) is a chime only.
        let free = vec![state("a", true, true), state("b", true, true), state("c", true, false)];
        assert_eq!(quest_changes(&start, &free), (true, false));
    }

    #[test]
    fn pins_toggle_up_to_the_trackers_room_and_only_known_quests() {
        let content = content();
        let most = content.quests.tracker().pins;
        let names: Vec<String> = content.quests.iter().map(|q| q.name.clone()).take(most + 1).collect();
        let mut pins = Vec::new();
        for name in &names[..most] {
            assert!(toggle(&mut pins, &content, name));
        }
        assert!(!toggle(&mut pins, &content, &names[most]), "pinned past the room");
        assert_eq!(pins.len(), most);
        assert!(toggle(&mut pins, &content, &names[0]), "unpinning");
        assert!(!pins.contains(&names[0]));
        assert!(!toggle(&mut pins, &content, "base:no_such_quest"));
        assert_eq!(pins.len(), most - 1);
    }

    /// A pinned quest that drops off done leaves what it opens pinned, within the tracker's
    /// room, never a quest done or one still waiting on another; opening nothing, and with
    /// nothing else pinned, the next open quest of its age.
    #[test]
    fn what_a_done_quest_opens_is_pinned_in_its_place() {
        let content = content();
        let room = content.quests.tracker().pins;
        // Some quest whose follower waits on it alone.
        let (first, follower) = content
            .quests
            .iter()
            .find_map(|quest| (quest.after.len() == 1 && !quest.after[0].contains('|')).then(|| (quest.after[0].clone(), quest.name.clone())))
            .and_then(|(after, name)| content.quests.get(&after).map(|q| (q.name.clone(), name)))
            .expect("a quest after one other");
        // Done: the first and everything before it.
        let mut finished = vec![first.clone()];
        let mut i = 0;
        while i < finished.len() {
            let quest = content.quests.get(&finished[i]).unwrap();
            for b in before(&content, quest) {
                if !finished.contains(&b.name) {
                    finished.push(b.name.clone());
                }
            }
            i += 1;
        }
        let done = |name: &str| finished.iter().any(|f| f == name);
        let next = follow_ons(&content, &[], std::slice::from_ref(&first), &done);
        assert!(next.contains(&follower), "{follower} follows {first}: {next:?}");
        assert!(next.len() <= room);
        for name in &next {
            let quest = content.quests.get(name).unwrap();
            assert!(!done(name) && content.quests.open(quest, &done), "{name} is open");
        }
        // A full tracker takes none.
        let full: Vec<String> = (0..room).map(|i| format!("base:pinned_{i}")).collect();
        assert!(follow_ons(&content, &full, std::slice::from_ref(&first), &done).is_empty());
        // Opening nothing new: the age's next open quest, only with nothing else pinned.
        let last = content.quests.iter().find(|q| content.quests.iter().all(|other| !before(&content, other).iter().any(|b| b.name == q.name))).expect("a quest nothing follows");
        let none = |name: &str| name == last.name;
        let fallback = follow_ons(&content, &[], std::slice::from_ref(&last.name), &none);
        assert_eq!(fallback.len(), 1, "{fallback:?}");
        assert_eq!(content.quests.get(&fallback[0]).unwrap().age, last.age);
        assert!(follow_ons(&content, &["base:other".to_owned()], std::slice::from_ref(&last.name), &none).is_empty());
    }

    #[test]
    fn lines_are_cut_to_the_well() {
        let view = Rect::new(0.0, 0.0, 100.0, 50.0);
        // Inside, whole; across an edge, cut at it; across a corner, cut at both; outside,
        // nothing; along an edge, square to it, nothing past the ends.
        assert_eq!(clip_line(Vec2::new(10.0, 10.0), Vec2::new(90.0, 40.0), view), Some((Vec2::new(10.0, 10.0), Vec2::new(90.0, 40.0))));
        assert_eq!(clip_line(Vec2::new(50.0, 25.0), Vec2::new(150.0, 25.0), view), Some((Vec2::new(50.0, 25.0), Vec2::new(100.0, 25.0))));
        let (from, to) = clip_line(Vec2::new(-50.0, -25.0), Vec2::new(150.0, 75.0), view).unwrap();
        assert!(from.distance(Vec2::new(0.0, 0.0)) < 1e-4 && to.distance(Vec2::new(100.0, 50.0)) < 1e-4, "{from} {to}");
        assert_eq!(clip_line(Vec2::new(-10.0, 60.0), Vec2::new(200.0, 300.0), view), None);
        assert_eq!(clip_line(Vec2::new(-10.0, 80.0), Vec2::new(-5.0, -80.0), view), None);
        assert_eq!(clip_line(Vec2::new(20.0, -30.0), Vec2::new(20.0, 30.0), view), Some((Vec2::new(20.0, 0.0), Vec2::new(20.0, 30.0))));
        // Every point of a cut line lies in the well, whichever way it runs.
        for (from, to) in [(Vec2::new(130.0, -40.0), Vec2::new(-30.0, 90.0)), (Vec2::new(70.0, 120.0), Vec2::new(10.0, -3.0))] {
            let (a, b) = clip_line(from, to, view).unwrap();
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let p = a.lerp(b, t);
                assert!(view.inflate(1e-3).contains(p), "{p} of {from}..{to}");
            }
        }
    }

    #[test]
    fn a_done_quest_counts_down_only_while_shown() {
        let pinned = vec!["base:a".to_owned(), "base:b".to_owned()];
        let done = |name: &str| name == "base:a";
        let mut finished = Finished::default();
        // Hidden behind a screen for a long while: nothing drops off.
        for _ in 0..100 {
            assert!(finished.tick(&pinned, done, false, 0.5, 5.0).is_empty());
        }
        // Shown: it stays its five seconds, then goes, and the quest not done stays.
        for _ in 0..10 {
            assert!(finished.tick(&pinned, done, true, 0.5, 5.0).is_empty());
        }
        assert_eq!(finished.tick(&pinned, done, true, 0.5, 5.0), vec!["base:a".to_owned()]);
        assert!(finished.0.is_empty());
        // Done, then unpinned by its × before its time: it never drops off as done.
        assert!(finished.tick(&pinned, done, true, 0.5, 5.0).is_empty());
        let left = vec!["base:b".to_owned()];
        for _ in 0..20 {
            assert!(finished.tick(&left, done, true, 0.5, 5.0).is_empty());
        }
        assert!(finished.0.is_empty());
    }

    /// No two quests of a web stand in one place, and a quest from another chapter that
    /// comes first is drawn, in the first column.
    #[test]
    fn webs_keep_quests_apart_and_show_the_way_in() {
        let content = content();
        for age in chapters(&content) {
            let web = layout(&content, &age);
            for (i, a) in web.cells.iter().enumerate() {
                for b in &web.cells[i + 1..] {
                    assert!(a.distance(*b) >= 1.0, "{age}: two quests within a step of each other at {a} and {b}");
                }
                assert!(a.x < web.columns as f32 && a.y < web.tallest as f32, "{age}: {a} outside {}×{}", web.columns, web.tallest);
            }
            for i in web.ours..web.quests.len() {
                assert_eq!(web.cells[i].x, 0.0, "{age}: {} from another chapter stands past the first column", web.quests[i].name);
            }
        }
        let farm = layout(&content, "farm");
        let hearth = farm.quests.iter().position(|q| q.name == "base:homestead_hearth").expect("the farm's web shows the hearth that opens it");
        assert!(hearth >= farm.ours);
    }

    /// Each chapter has a name in Japanese, and the tracker's numbers come from the data.
    #[test]
    fn chapters_are_named_and_the_tracker_is_data() {
        let content = content();
        let ja = Locale::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), "ja");
        for age in chapters(&content) {
            assert!(ja.find(&format!("quest.age.{age}")).is_some(), "no Japanese name for the {age} chapter");
        }
        assert_eq!(content.quests.tracker(), voxmar_engine::quest::Tracker { pins: 3, done_stays: 5.0, backing_alpha: 0.5, follow_on: true });
    }
}
