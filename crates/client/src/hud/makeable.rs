//! What the player can make, under the inventory's slots: from what is in the pack (and,
//! standing in their home, its stores, as laying a recipe takes from them too), the
//! recipes ready to make now first, then those that want a crafting table, a level or a
//! station, then those nearly there (half or more of what they take is in the pack). A
//! click on one ready to make lays it in the grid (`core:lay`, Shift for as many sets as a
//! stack holds); on any other, the recipe viewer opens on it. The line under the cells
//! says what the one pointed at still wants: the items short and how many, the table, the
//! level, or the station it is made at.
//!
//! It reads the recipe book (`voxmar_engine::recipe_book`), so a package's recipes show
//! here with no more work.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use rustc_hash::FxHashMap;
use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::craft::Recipe;
use voxmar_engine::recipe_book::{Method, RecipeBook};
use voxmar_engine::skill::SkillId;

use super::{HudImages, SLOT};
use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::Silent;
use crate::ui::widgets::{BODY, GOLD, MUTED, ScrollList, shadowed, space, text};

pub struct MakeablePlugin;

impl Plugin for MakeablePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Drawn>().add_systems(Update, (draw, pick, say).chain().after(super::inventory::InventorySystems).run_if(in_state(Screen::InGame)));
    }
}

/// What stands between the player and making a thing, from what is in the pack.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reach {
    /// All it takes is here: a click lays it in the grid.
    Ready,
    /// All it takes is here, but it is made at a crafting table.
    Table,
    /// All it takes is here, but it wants this level in this skill.
    Level(SkillId, u8),
    /// Cooked, fired or smelted at a station (blocks with this tag); what goes in is here.
    Station(String),
    /// Half or more of it is here; these are short, and by how many.
    Short(Vec<(BlockId, u32)>),
}

impl Reach {
    /// The order it is shown in: ready first, then wanting one thing, then short (fewest
    /// short first).
    pub(crate) fn rank(&self) -> u32 {
        match self {
            Reach::Ready => 0,
            Reach::Table => 1,
            Reach::Level(..) => 2,
            Reach::Station(_) => 3,
            Reach::Short(items) => 4 + items.iter().map(|&(_, n)| n).sum::<u32>(),
        }
    }
}

/// A thing the player can make, or nearly: how, and what stands in the way.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Makeable {
    pub output: BlockId,
    /// For a crafting way, its recipes: the first (in content order) and how many follow
    /// it, as the recipe book's entry has them.
    pub recipes: Option<(usize, usize)>,
    pub reach: Reach,
}

/// What the pack holds, by item, for laying: its slots, its pouches, the crafting grid and
/// the pointer (both go back into the pack when a recipe is laid).
pub(crate) fn held(inventory: &voxmar_engine::inventory::Inventory) -> FxHashMap<BlockId, u32> {
    let mut have: FxHashMap<BlockId, u32> = FxHashMap::default();
    for stack in inventory.slots.iter().chain(inventory.craft.iter()).chain(std::iter::once(&inventory.cursor)).flatten() {
        *have.entry(stack.item).or_default() += stack.count as u32;
    }
    for &(item, n) in &inventory.pouched {
        *have.entry(item).or_default() += n as u32;
    }
    have
}

/// What laying a recipe can take from: the pack (`held`) and, while the player stands in
/// their home, what its stores hold, as the server sends it (`Vitals::home_stores`).
pub(crate) fn within_reach(player: &LocalPlayer) -> FxHashMap<BlockId, u32> {
    let mut have = held(&player.inventory);
    for &(item, n) in &player.vitals.home_stores {
        *have.entry(item).or_default() += n;
    }
    have
}

/// How near one crafting recipe is, or None when too little of it is here to show.
/// `lacks` says the level a recipe wants that the player does not have, if any.
pub(crate) fn reach(recipe: &Recipe, have: &FxHashMap<BlockId, u32>, at_table: bool, lacks: &dyn Fn(&Recipe) -> Option<(SkillId, u8)>) -> Option<Reach> {
    let short = recipe.short(have);
    if short.is_empty() {
        return Some(if recipe.needs_table() && !at_table {
            Reach::Table
        } else if let Some((skill, level)) = lacks(recipe) {
            Reach::Level(skill, level)
        } else {
            Reach::Ready
        });
    }
    let cells = recipe.alternatives().iter().flatten().count() as u32;
    let missing: u32 = short.iter().map(|&(_, n)| n).sum();
    (2 * missing <= cells).then_some(Reach::Short(short))
}

/// Everything the player can make or nearly, from the book and what they hold, in the
/// order it is shown; each thing once, by its nearest way.
#[cfg(test)]
pub(crate) fn plan(content: &Content, book: &RecipeBook, have: &FxHashMap<BlockId, u32>, at_table: bool, lacks: &dyn Fn(&Recipe) -> Option<(SkillId, u8)>) -> Vec<Makeable> {
    plan_cooking_from(content, book, have, have, at_table, lacks)
}

/// As `plan`, with what a fire or stove cooks counted from `cooks` alone: a fire cooks
/// what is in hand, never what lies in the home's stores, so raw meat only in the barrel is
/// not shown as cooked at a campfire.
pub(crate) fn plan_cooking_from(content: &Content, book: &RecipeBook, have: &FxHashMap<BlockId, u32>, cooks: &FxHashMap<BlockId, u32>, at_table: bool, lacks: &dyn Fn(&Recipe) -> Option<(SkillId, u8)>) -> Vec<Makeable> {
    let recipes: Vec<&Recipe> = content.recipes.iter().collect();
    let mut found: Vec<Makeable> = Vec::new();
    for entry in book.entries() {
        let near = match &entry.method {
            Method::Craft { variants, first, .. } => recipes
                .iter()
                .skip(*first)
                .take(variants.len())
                .filter_map(|recipe| reach(recipe, have, at_table, lacks))
                .min_by_key(Reach::rank)
                .map(|reach| Makeable { output: entry.output, recipes: Some((*first, variants.len())), reach }),
            // Cooked: from what is in hand, as many as one go takes, with what else it takes
            // (three copper ingots and a tin ore make bronze), or short of those. Any one of
            // a list of what goes with it will do; short of one, the list's first is named.
            Method::Cook { input, uses, with, station, .. } if cooks.get(input).is_some_and(|&n| n > 0) => {
                let count = |item: &BlockId| cooks.get(item).copied().unwrap_or(0);
                let short: Vec<(BlockId, u32)> = std::iter::once((*input, *uses as u32, count(input)))
                    .chain(with.iter().filter_map(|(items, n)| Some((*items.first()?, *n as u32, items.iter().map(count).sum::<u32>()))))
                    .filter_map(|(item, n, have)| (have < n).then(|| (item, n - have)))
                    .collect();
                let reach = if short.is_empty() { Reach::Station(station.clone()) } else { Reach::Short(short) };
                Some(Makeable { output: entry.output, recipes: None, reach })
            }
            _ => None,
        };
        let Some(near) = near else { continue };
        match found.iter_mut().find(|m| m.output == near.output) {
            Some(seen) if near.reach.rank() < seen.reach.rank() => *seen = near,
            Some(_) => {}
            None => found.push(near),
        }
    }
    // Stable: within a rank, in the order the content gives them.
    found.sort_by_key(|m| m.reach.rank());
    found
}

/// The level a recipe wants that `player` lacks, as the server judges it (boosts and
/// schematics counted).
pub(crate) fn lacks<'a>(content: &'a Content, player: &'a LocalPlayer) -> impl Fn(&Recipe) -> Option<(SkillId, u8)> + 'a {
    move |recipe: &Recipe| {
        let (skills, effects, blocks) = (&content.skills, &content.effects, &content.blocks);
        recipe
            .skill
            .filter(|&skill| {
                let boosted = voxmar_engine::level::check_level(skills, effects, &player.skills, &player.vitals.effects, skill);
                !voxmar_engine::level::meets_recipe(recipe, skills.level(player.skills.xp(skill)), boosted, &player.inventory, blocks)
            })
            .map(|skill| (skill, recipe.level))
    }
}

/// The words for what stands in the way of `output` ("Stone Axe: still needs 1 Stick").
pub(crate) fn words(locale: &Locale, content: &Content, output: BlockId, reach: &Reach) -> String {
    let item = locale.block(content.blocks.def(output));
    let block = |tag: &str| content.blocks.tagged(tag).next().map_or_else(|| tag.to_owned(), |b| locale.block(content.blocks.def(b)));
    match reach {
        Reach::Ready => locale.format("make.ready", &[("item", &item)]),
        Reach::Table => locale.format("make.table", &[("item", &item), ("table", &block("crafting_table"))]),
        Reach::Level(skill, level) => locale.format("make.level", &[("item", &item), ("skill", &locale.skill(content.skills.get(*skill))), ("level", level)]),
        Reach::Station(station) => locale.format("make.station", &[("item", &item), ("station", &block(station))]),
        Reach::Short(short) => {
            let items: Vec<String> = short.iter().map(|&(i, n)| locale.format("multiblock.fuel_stack", &[("count", &n), ("item", &locale.block(content.blocks.def(i)))])).collect();
            locale.format("make.short", &[("item", &item), ("items", &items.join(locale.get("language.comma")))])
        }
    }
}

/// The cells' rows shown at once; more scroll.
const ROWS: f32 = 2.0;

/// The section: its title and the line saying what the one pointed at wants, then the
/// cells.
#[derive(Component)]
struct MakeSection;
#[derive(Component)]
pub(crate) struct MakeList;
#[derive(Component)]
pub(crate) struct MakeLine;
/// A thing on the list, by its place in the plan drawn.
#[derive(Component)]
pub(crate) struct MakeCell(pub usize);

/// The plan drawn last, for the clicks and the line.
#[derive(Resource, Default)]
pub(crate) struct Drawn(pub Vec<Makeable>);

/// The section under the inventory's slots, `columns` cells across.
pub(crate) fn spawn(card: &mut ChildSpawnerCommands, font: &Handle<Font>, columns: usize) {
    let width = columns as f32 * SLOT + (columns as f32 - 1.0) * PX;
    let section = Node { width: Val::Px(width), flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), margin: UiRect::top(Val::Px(space::GAP)), ..default() };
    card.spawn((section, MakeSection)).with_children(|section| {
        section.spawn(Node { justify_content: JustifyContent::SpaceBetween, column_gap: Val::Px(space::GAP), ..default() }).with_children(|top| {
            top.spawn((Localized("make.title"), shadowed(font, "", BODY, GOLD)));
        });
        let list = Node {
            display: Display::Grid,
            grid_template_columns: RepeatedGridTrack::px(columns as u16, SLOT),
            grid_auto_rows: vec![GridTrack::px(SLOT)],
            row_gap: Val::Px(PX),
            column_gap: Val::Px(PX),
            height: Val::Px(ROWS * SLOT + (ROWS - 1.0) * PX),
            overflow: Overflow::scroll_y(),
            ..default()
        };
        section.spawn((list, MakeList, ScrollList));
        section.spawn((MakeLine, text(font, "", BODY, MUTED)));
    });
}

/// Draws the list again when what can be made changes.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    images: Res<HudImages>,
    viewer: Res<super::recipes::RecipeViewer>,
    list: Query<Entity, With<MakeList>>,
    mut section: Query<&mut Node, With<MakeSection>>,
    mut drawn: ResMut<Drawn>,
) {
    let (Some(book), Ok(list)) = (viewer.book(), list.single()) else { return };
    if !ui.inventory_open {
        return;
    }
    // An open store takes the crafting grid's place, and with it what is laid there.
    let display = if player.inventory.store.is_some() { Display::None } else { Display::Flex };
    if let Ok(mut node) = section.single_mut()
        && node.display != display
    {
        node.display = display;
    }
    let content = &launch.content;
    let lacks = lacks(content, &player);
    let plan = plan_cooking_from(content, book, &within_reach(&player), &held(&player.inventory), player.station.is_some(), &lacks);
    if drawn.0 == plan {
        return;
    }
    commands.entity(list).despawn_children().with_children(|list| {
        for (i, makeable) in plan.iter().enumerate() {
            let ready = makeable.reach == Reach::Ready;
            let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
            let piece = if ready { Piece::WellFocus } else { Piece::Well };
            list.spawn((Button, Silent, MakeCell(i), RelativeCursorPosition::default(), crate::ui::widgets::HoverLight, node, piece.image())).with_children(|cell| {
                let inset = (SLOT - 16.0 * PX) / 2.0;
                let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                // Not ready, faint: there, but not yet.
                let tint = if ready { Color::WHITE } else { Color::srgba(1.0, 1.0, 1.0, 0.45) };
                cell.spawn((icon, ImageNode::new(images.icons.get(makeable.output.0 as usize).cloned().unwrap_or_default()).with_color(tint)));
            });
        }
    });
    drawn.0 = plan;
}

/// A click on one ready lays it in the grid (Shift: as many sets as a stack holds); on any
/// other, the recipe viewer opens on it.
fn pick(
    cells: Query<(&MakeCell, &Interaction), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    drawn: Res<Drawn>,
    player: Res<LocalPlayer>,
    net: Res<Net>,
    actions: Res<crate::Actions>,
    mut viewer: ResMut<super::recipes::RecipeViewer>,
    mut ui: ResMut<UiState>,
) {
    if !ui.inventory_open || ui.recipes_open || player.inventory.cursor.is_some() {
        return;
    }
    for (cell, interaction) in &cells {
        let (Interaction::Pressed, Some(makeable)) = (interaction, drawn.0.get(cell.0)) else { continue };
        match (&makeable.reach, makeable.recipes, actions.0.id("core:lay")) {
            (Reach::Ready, Some((first, ways)), Some(lay)) => {
                let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
                let count = if shift { voxmar_engine::inventory::MAX_STACK } else { 1 };
                let target = voxmar_engine::action::ActionTarget::Recipe { first: first as u16, ways: ways.min(u8::MAX as usize) as u8, count };
                net.propose_from(&player, lay, target);
            }
            _ => {
                viewer.open(makeable.output, false);
                ui.recipes_open = true;
            }
        }
    }
}

/// The line under the cells: what the one pointed at wants, or how many are ready.
fn say(ui: Res<UiState>, launch: Res<Launch>, locale: Res<Locale>, drawn: Res<Drawn>, cells: Query<(&MakeCell, &RelativeCursorPosition)>, mut line: Query<&mut Text, With<MakeLine>>) {
    if !ui.inventory_open {
        return;
    }
    let Ok(mut line) = line.single_mut() else { return };
    let pointed = cells.iter().find(|(_, cursor)| cursor.cursor_over()).and_then(|(cell, _)| drawn.0.get(cell.0));
    let words = match pointed {
        Some(makeable) => words(&locale, &launch.content, makeable.output, &makeable.reach),
        None if drawn.0.is_empty() => locale.get("make.none").to_owned(),
        None => {
            let ready = drawn.0.iter().filter(|m| m.reach == Reach::Ready).count();
            locale.format("make.summary", &[("ready", &ready)])
        }
    };
    if line.0 != words {
        line.0 = words;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
    }

    fn have(content: &Content, items: &[(&str, u32)]) -> FxHashMap<BlockId, u32> {
        items.iter().map(|&(name, n)| (content.blocks.id(name).unwrap_or_else(|| panic!("no {name}")), n)).collect()
    }

    fn find<'a>(plan: &'a [Makeable], content: &Content, name: &str) -> Option<&'a Makeable> {
        let id = content.blocks.id(name)?;
        plan.iter().find(|m| m.output == id)
    }

    /// Standing in their home, what its stores hold counts as within reach, as the
    /// server lays from it: four planks in the barrel and none in the pack make a table.
    #[test]
    fn the_home_stores_count_toward_what_can_be_made() {
        let content = content();
        let book = RecipeBook::new(&content);
        let none = |_: &Recipe| None;
        let mut player = LocalPlayer::default();
        let planks = content.blocks.id("base:oak_planks").unwrap();
        player.vitals.home_stores = vec![(planks, 4)];
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(planks, 1));
        assert_eq!(within_reach(&player).get(&planks), Some(&5), "the pack's and the stores' together");
        let made = plan(&content, &book, &within_reach(&player), false, &none);
        assert_eq!(find(&made, &content, "base:crafting_table").map(|m| &m.reach), Some(&Reach::Ready));
        player.vitals.home_stores.clear();
        let away = plan(&content, &book, &within_reach(&player), false, &none);
        assert_ne!(find(&away, &content, "base:crafting_table").map(|m| &m.reach), Some(&Reach::Ready), "one plank alone is not a table");
    }

    /// A log in an empty pack makes planks at once; with planks, sticks and a crafting
    /// table are ready, ahead of anything wanting a table; what is too far off is not
    /// shown, and each thing shows once.
    #[test]
    fn what_can_be_made_comes_first_and_what_is_far_off_is_not_shown() {
        let content = content();
        let book = RecipeBook::new(&content);
        let none = |_: &Recipe| None;
        assert!(plan(&content, &book, &FxHashMap::default(), false, &none).is_empty(), "an empty pack makes nothing");
        let plan_log = plan(&content, &book, &have(&content, &[("base:oak_log", 1)]), false, &none);
        assert_eq!(find(&plan_log, &content, "base:oak_planks").map(|m| &m.reach), Some(&Reach::Ready));
        assert_eq!(plan_log[0].reach, Reach::Ready);
        let planks = plan(&content, &book, &have(&content, &[("base:oak_planks", 4), ("base:cobblestone", 3), ("base:stick", 2)]), false, &none);
        for name in ["base:stick", "base:crafting_table"] {
            assert_eq!(find(&planks, &content, name).map(|m| &m.reach), Some(&Reach::Ready), "{name}");
        }
        assert_eq!(find(&planks, &content, "base:stone_pickaxe").map(|m| &m.reach), Some(&Reach::Table), "three by three wants a table");
        let first_table = planks.iter().position(|m| m.reach == Reach::Table).unwrap();
        assert!(planks[..first_table].iter().all(|m| m.reach == Reach::Ready), "ready ones first");
        // At a table, it is ready.
        let at_table = plan(&content, &book, &have(&content, &[("base:cobblestone", 3), ("base:stick", 2)]), true, &none);
        assert_eq!(find(&at_table, &content, "base:stone_pickaxe").map(|m| &m.reach), Some(&Reach::Ready));
        let mut outputs: Vec<BlockId> = planks.iter().map(|m| m.output).collect();
        outputs.sort();
        outputs.dedup();
        assert_eq!(outputs.len(), planks.len(), "each thing once");
        // One stick of a pickaxe's five: too far off to show.
        let stick = plan(&content, &book, &have(&content, &[("base:stick", 1)]), true, &none);
        assert!(find(&stick, &content, "base:stone_pickaxe").is_none());
    }

    /// Short of a little, it says what and how many; wanting a level, which; cooked, where.
    #[test]
    fn what_stands_in_the_way_is_named() {
        let content = content();
        let book = RecipeBook::new(&content);
        let none = |_: &Recipe| None;
        let plan_short = plan(&content, &book, &have(&content, &[("base:cobblestone", 3), ("base:stick", 1)]), true, &none);
        let stick = content.blocks.id("base:stick").unwrap();
        assert_eq!(find(&plan_short, &content, "base:stone_pickaxe").map(|m| &m.reach), Some(&Reach::Short(vec![(stick, 1)])));
        let engineering = content.skills.id("core:engineering").unwrap();
        let lacking = |r: &Recipe| r.skill.filter(|&s| s == engineering && r.level > 0).map(|s| (s, r.level));
        let level = plan(&content, &book, &have(&content, &[("base:cobblestone", 3), ("base:stick", 2)]), true, &lacking);
        assert_eq!(find(&level, &content, "base:stone_pickaxe").map(|m| &m.reach), Some(&Reach::Level(engineering, 1)));
        let locale = Locale::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), "en");
        let line = words(&locale, &content, content.blocks.id("base:stone_pickaxe").unwrap(), &Reach::Short(vec![(stick, 1)]));
        assert!(line.contains("Stone Pickaxe") && line.contains("1 Stick"), "{line}");
        let table = words(&locale, &content, content.blocks.id("base:stone_pickaxe").unwrap(), &Reach::Table);
        assert!(table.contains("Crafting Table"), "{table}");
        // Something cooked, from what goes in it.
        let (input, cooked) = content.cooking.iter().next().map(|(input, recipe)| (input, recipe.output)).unwrap();
        let cook = plan(&content, &book, &[(content.blocks.item_of(input), 1)].into_iter().collect(), false, &none);
        let found = cook.iter().find(|m| m.output == cooked).expect("the cooked thing shows");
        // What a crucible takes besides what is held counts: three copper ingots alone are
        // a tin ore short of bronze, and with one carried, bronze is cooked.
        let (bronze, tin) = (content.blocks.id("base:bronze_ingot").unwrap(), content.blocks.id("base:tin_ore").unwrap());
        let copper = plan(&content, &book, &have(&content, &[("base:copper_ingot", 3)]), false, &none);
        assert_eq!(find(&copper, &content, "base:bronze_ingot").map(|m| &m.reach), Some(&Reach::Short(vec![(tin, 1)])));
        let alloy = plan(&content, &book, &have(&content, &[("base:copper_ingot", 3), ("base:tin_ore", 1)]), false, &none);
        assert!(matches!(alloy.iter().find(|m| m.output == bronze).map(|m| &m.reach), Some(Reach::Station(_))), "{alloy:?}");
        let one = plan(&content, &book, &have(&content, &[("base:copper_ingot", 1), ("base:tin_ore", 1)]), false, &none);
        let copper_ingot = content.blocks.id("base:copper_ingot").unwrap();
        assert_eq!(find(&one, &content, "base:bronze_ingot").map(|m| &m.reach), Some(&Reach::Short(vec![(copper_ingot, 2)])), "one go takes three");
        // Only in the home's stores, it is not shown cooked: a fire cooks from the hand.
        let stored: FxHashMap<BlockId, u32> = [(content.blocks.item_of(input), 1)].into_iter().collect();
        let from_stores = plan_cooking_from(&content, &book, &stored, &FxHashMap::default(), false, &none);
        assert!(from_stores.iter().all(|m| m.output != cooked), "what lies only in the stores is not cooked at a fire");
        let Reach::Station(station) = &found.reach else { panic!("{found:?}") };
        let line = words(&locale, &content, cooked, &found.reach);
        assert!(!line.contains(station.as_str()) || content.blocks.tagged(station).next().is_none(), "the station is named, not its tag: {line}");
        for code in ["en", "ja"] {
            let locale = Locale::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), code);
            for key in ["make.title", "make.none", "make.summary", "make.ready", "make.table", "make.level", "make.station", "make.short"] {
                assert!(locale.find(key).is_some(), "{code} lacks {key}");
            }
        }
    }
}
