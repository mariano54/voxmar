//! The recipe viewer, as NEI does it: over any item in the inventory (pack, hotbar,
//! crafting grid and what it makes) or in the viewer itself, R shows the recipes that make
//! it and U everything it goes into. Each way is drawn as it happens in the game: the
//! crafting grid, a station, a creature with the tool that takes its product, a creature
//! that drops it. Pages turn with the arrows; clicking an item looks up its recipes (a
//! right click, its uses), and Back retraces the steps. Over the inventory, a crafting
//! way has a Lay button: the grid is laid with what it takes, from the pack and, at home,
//! the home's stores (shift: as many sets as a stack holds; `core:lay`).
//!
//! The book comes from the game's content (`voxmar_engine::recipe_book`), so a package's
//! recipes show here as soon as it adds them.

use std::collections::HashSet;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use voxmar_engine::block::BlockId;
use voxmar_engine::craft::{GRID, SIDE};
use voxmar_engine::recipe_book::{Entry, Method, RecipeBook};

use super::{HudImages, SLOT};
use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::player::UiState;
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::Silent;
use crate::ui::widgets::{BODY, ButtonStyle, GOLD, LARGE, MUTED, SCRIM, SOFT, TEXT, UiFont, panel, screen, shadowed, space, spawn_button, text};

/// Ways shown on one page.
pub(crate) const PER_PAGE: usize = 2;
const WIDTH: f32 = 300.0 * PX;

pub struct RecipesPlugin;

impl Plugin for RecipesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecipeViewer>()
            .init_resource::<HoveredItem>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (look_up, click, report, draw, cycle).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut viewer: ResMut<RecipeViewer>, mut ui: ResMut<UiState>| {
                viewer.trail.clear();
                viewer.reported.clear();
                (viewer.drawn, viewer.quests_drawn) = (None, None);
                ui.recipes_open = false;
            });
    }
}

/// A look-up to open once in game, by item name (`--recipes NAME`, `--uses NAME`, for
/// captures).
#[derive(Resource)]
pub struct StartLookup(pub String, pub bool);

/// The item under the pointer on a screen that can look it up, if any: set by the
/// inventory and by the viewer itself.
#[derive(Resource, Default)]
pub struct HoveredItem(pub Option<BlockId>);

/// One look-up: an item, whether its uses or its recipes, and the page showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lookup {
    pub item: BlockId,
    pub uses: bool,
    pub page: usize,
}

/// What the viewer shows: the look-ups made since it opened, the last one showing; empty
/// while it is closed.
#[derive(Resource, Default)]
pub struct RecipeViewer {
    pub trail: Vec<Lookup>,
    book: Option<Arc<RecipeBook>>,
    /// What was drawn last, to draw again only on a change.
    drawn: Option<(Lookup, String, bool, Vec<String>)>,
    /// The item and language the quests row was drawn for: it is drawn again only when
    /// they change, never as a family's recipes cycle, so its links hold still to click.
    quests_drawn: Option<(BlockId, String)>,
    /// Items reported in this game, bounded by the content registry, cleared on leaving.
    reported: HashSet<BlockId>,
}

impl RecipeViewer {
    /// Every way to get and to use each item, once the game's content is read.
    pub fn book(&self) -> Option<&RecipeBook> {
        self.book.as_deref()
    }

    /// Looks `item` up, on top of what is showing.
    pub fn open(&mut self, item: BlockId, uses: bool) {
        if self.trail.last().is_some_and(|last| last.item == item && last.uses == uses) {
            return;
        }
        self.trail.push(Lookup { item, uses, page: 0 });
    }

    /// The ways the current look-up finds.
    pub fn found(&self) -> Vec<&Entry> {
        let (Some(book), Some(lookup)) = (&self.book, self.trail.last()) else { return Vec::new() };
        if lookup.uses { book.using(lookup.item) } else { book.making(lookup.item) }
    }

    /// Whether the quests row needs drawing for `item` in `language`, noting that it is.
    fn quests_stale(&mut self, item: BlockId, language: &str) -> bool {
        if self.quests_drawn.as_ref().is_some_and(|(drawn, code)| *drawn == item && code == language) {
            return false;
        }
        self.quests_drawn = Some((item, language.to_owned()));
        true
    }

    fn pages(&self) -> usize {
        self.found().len().div_ceil(PER_PAGE).max(1)
    }

    fn unreported_item(&mut self) -> Option<BlockId> {
        let item = self.trail.last()?.item;
        self.reported.insert(item).then_some(item)
    }
}

#[derive(Component)]
struct ViewerRoot;
#[derive(Component)]
struct ViewerTitle;
#[derive(Component)]
pub(crate) struct ViewerBody;
#[derive(Component)]
struct ViewerPage;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewerButton {
    Previous,
    Next,
    Back,
}
/// A crafting way's button that lays its recipe into the grid: the item it makes, and its
/// recipes (the first, in content order, and how many variants follow it).
#[derive(Component)]
pub(crate) struct LayButton {
    pub output: BlockId,
    first: u16,
    ways: u8,
}
/// Where the quests that ask for the item looked up are listed.
#[derive(Component)]
struct ViewerQuests;
/// A quest that asks for the item looked up: click to open the quest book on it.
#[derive(Component)]
pub(crate) struct QuestLink(pub(crate) String);
/// Quests listed under a look-up at most; more are counted.
pub(crate) const QUESTS_SHOWN: usize = 4;
/// An item drawn in the viewer: hover to look it up, click for its recipes.
#[derive(Component)]
pub struct ViewerItem(pub BlockId);

/// A grid cell of a family of recipes, which shows each variant's item in turn, a second
/// each: its item and icon change in place, so the card under the pointer (its Lay button,
/// its cells) is never rebuilt for it.
#[derive(Component)]
pub(crate) struct CycleCell(Vec<BlockId>);

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((ViewerRoot, screen(SCRIM), Visibility::Hidden, GlobalZIndex(5))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            let title = Node { justify_content: JustifyContent::Center, margin: UiRect::bottom(Val::Px(space::GAP)), ..default() };
            card.spawn(title).with_child((shadowed(font, "", LARGE, GOLD), ViewerTitle));
            let body = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::GAP), min_height: Val::Px(2.0 * (2.0 * SLOT + space::SECTION * 2.0)), ..default() };
            card.spawn((body, ViewerBody));
            let quests = Node { flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), row_gap: Val::Px(space::HAIR), margin: UiRect::top(Val::Px(space::GAP)), ..default() };
            card.spawn((quests, ViewerQuests));
            let footer = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), margin: UiRect::top(Val::Px(space::SECTION)), ..default() };
            card.spawn(footer).with_children(|footer| {
                let small = |footer: &mut ChildSpawnerCommands, key: &'static str, which: ViewerButton| {
                    footer.spawn(Node { width: Val::Px(24.0 * PX), ..default() }).with_children(|cell| {
                        spawn_button(cell, font, key, ButtonStyle::Secondary, which);
                    });
                };
                small(footer, "recipes.previous", ViewerButton::Previous);
                footer.spawn((Node { flex_grow: 1.0, justify_content: JustifyContent::Center, ..default() }, children![(text(font, "", BODY, SOFT), ViewerPage)]));
                small(footer, "recipes.next", ViewerButton::Next);
                footer.spawn(Node { width: Val::Px(48.0 * PX), ..default() }).with_children(|cell| {
                    spawn_button(cell, font, "recipes.back", ButtonStyle::Secondary, ViewerButton::Back);
                });
            });
            card.spawn((Node { margin: UiRect::top(Val::Px(space::GAP)), ..default() }, Localized("recipes.hint"), text(font, "", BODY, MUTED)));
        });
    });
}

/// R and U over an item look it up; the viewer opens over whatever screen was open.
#[allow(clippy::too_many_arguments)]
fn look_up(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<Settings>,
    launch: Res<Launch>,
    mut hovered: ResMut<HoveredItem>,
    mut viewer: ResMut<RecipeViewer>,
    mut ui: ResMut<UiState>,
    items: Query<(&ViewerItem, &RelativeCursorPosition, &InheritedVisibility)>,
    start: Option<Res<StartLookup>>,
    mut commands: Commands,
) {
    if viewer.book.is_none() {
        // Its world data parsed as the content loaded (`load_content` refuses it otherwise).
        viewer.book = Some(Arc::new(voxmar_server::recipe_book(&launch.content).expect("the world data of loaded content parses")));
    }
    if let Some(StartLookup(name, uses)) = start.as_deref() {
        commands.remove_resource::<StartLookup>();
        match launch.content.blocks.id(name).or_else(|| launch.content.blocks.id(&format!("base:{name}"))) {
            Some(item) => {
                viewer.open(item, *uses);
                ui.recipes_open = true;
            }
            None => warn!("--recipes/--uses: no item called `{name}`"),
        }
    }
    // Over an item in the viewer, it is the one to look up.
    if ui.recipes_open || ui.quests_open {
        hovered.0 = items.iter().find(|(_, cursor, visible)| visible.get() && cursor.cursor_over()).map(|(item, _, _)| item.0);
    }
    if ui.console_open {
        return;
    }
    if let Some(item) = hovered.0 {
        if keys.just_pressed(settings.recipes_key.0) {
            viewer.open(item, false);
        } else if keys.just_pressed(settings.uses_key.0) {
            viewer.open(item, true);
        }
    }
    // Esc (in `player::grab_cursor`) closes the viewer by clearing the flag.
    if !ui.recipes_open && !viewer.trail.is_empty() && !keys.just_pressed(settings.recipes_key.0) && !keys.just_pressed(settings.uses_key.0) {
        viewer.trail.clear();
    }
    ui.recipes_open = !viewer.trail.is_empty();
}

#[allow(clippy::too_many_arguments)]
fn click(
    buttons: Query<(&ViewerButton, &Interaction, &InheritedVisibility), Changed<Interaction>>,
    lays: Query<(&LayButton, &Interaction, &InheritedVisibility), Changed<Interaction>>,
    (keys, net, player, actions): (Res<ButtonInput<KeyCode>>, Res<crate::net::Net>, Res<crate::player::LocalPlayer>, Res<crate::Actions>),
    items: Query<(&ViewerItem, &Interaction, &InheritedVisibility), Changed<Interaction>>,
    mouse: Res<ButtonInput<MouseButton>>,
    hovered_items: Query<(&ViewerItem, &RelativeCursorPosition, &InheritedVisibility)>,
    mut viewer: ResMut<RecipeViewer>,
    mut ui: ResMut<UiState>,
    (links, mut commands): (Query<(&QuestLink, &Interaction, &InheritedVisibility), Changed<Interaction>>, Commands),
) {
    // The quest book's item icons look their item up too, opening the viewer over it.
    if viewer.trail.is_empty() && !ui.quests_open {
        return;
    }
    let pages = viewer.pages();
    for (button, interaction, visible) in &buttons {
        if !visible.get() || *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            ViewerButton::Previous => {
                if let Some(last) = viewer.trail.last_mut() {
                    last.page = (last.page + pages - 1) % pages;
                }
            }
            ViewerButton::Next => {
                if let Some(last) = viewer.trail.last_mut() {
                    last.page = (last.page + 1) % pages;
                }
            }
            ViewerButton::Back => {
                viewer.trail.pop();
                ui.recipes_open = !viewer.trail.is_empty();
            }
        }
    }
    for (item, interaction, visible) in &items {
        if visible.get() && *interaction == Interaction::Pressed {
            viewer.open(item.0, false);
            ui.recipes_open = true;
        }
    }
    // Laid, the viewer closes onto the grid.
    for (lay, interaction, visible) in &lays {
        if visible.get() && *interaction == Interaction::Pressed
            && let Some(action) = actions.0.id("core:lay")
        {
            let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            let count = if shift { voxmar_engine::inventory::MAX_STACK } else { 1 };
            net.propose_from(&player, action, voxmar_engine::action::ActionTarget::Recipe { first: lay.first, ways: lay.ways, count });
            viewer.trail.clear();
            ui.recipes_open = false;
        }
    }
    // A quest asking for the item opens the book on it, the viewer and the screen under it
    // put away.
    for (link, interaction, visible) in &links {
        if visible.get() && *interaction == Interaction::Pressed {
            commands.insert_resource(super::quests::StartQuest(link.0.clone()));
            viewer.trail.clear();
            (ui.recipes_open, ui.inventory_open, ui.character_open, ui.journal_open) = (false, false, false, false);
        }
    }
    // A right click on an item looks up its uses (Interaction answers only the left).
    if mouse.just_pressed(MouseButton::Right)
        && let Some((item, _, _)) = hovered_items.iter().find(|(_, cursor, visible)| visible.get() && cursor.cursor_over())
    {
        viewer.open(item.0, true);
        ui.recipes_open = true;
    }
}

/// Tells the server of each item the viewer is opened on, once (for the quest book's
/// look-up goals).
fn report(mut viewer: ResMut<RecipeViewer>, net: Res<crate::net::Net>) {
    if let Some(item) = viewer.unreported_item() {
        net.looked_up(item);
    }
}

/// The block a player cooks at for a station tag: one they set down (placeable, its own
/// item, not creative only), a `station` first, as the campfire is for `fire`; else the
/// first block tagged at all. Not the first tagged: for `fire` that is an ember scale, a drop
/// that only glows.
fn station_block(content: &voxmar_engine::content::Content, tag: &str) -> Option<voxmar_engine::block::BlockId> {
    let blocks = &content.blocks;
    let set_down = |&b: &voxmar_engine::block::BlockId| blocks.info(b).placeable && blocks.item_of(b) == b && !blocks.def(b).has_tag(voxmar_engine::recipe_book::CREATIVE_ONLY);
    blocks
        .tagged(tag)
        .filter(set_down)
        .find(|&b| blocks.def(b).has_tag("station"))
        .or_else(|| blocks.tagged(tag).find(set_down))
        .or_else(|| blocks.tagged(tag).next())
}

#[cfg(test)]
mod tests {

    /// A fire recipe's card names the campfire a player cooks at, not the first block
    /// tagged `fire` (an ember scale), and a tag no block carries names none.
    #[test]
    fn a_cooking_card_names_the_station_a_player_sets_down() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let name = |tag: &str| super::station_block(&content, tag).map(|b| content.blocks.def(b).name.clone());
        assert_eq!(name("fire").as_deref(), Some("base:campfire"));
        assert_eq!(name("no_such_station"), None);
    }
    use super::*;

    #[test]
    fn reopening_or_turning_a_lookup_reports_each_item_once_per_game() {
        let mut viewer = RecipeViewer::default();
        assert_eq!(viewer.unreported_item(), None);
        viewer.open(BlockId(1), false);
        assert_eq!(viewer.unreported_item(), Some(BlockId(1)));
        assert_eq!(viewer.unreported_item(), None, "redrawing reports nothing");
        viewer.open(BlockId(2), false);
        assert_eq!(viewer.unreported_item(), Some(BlockId(2)));
        viewer.trail.pop();
        assert_eq!(viewer.unreported_item(), None, "Back reports nothing");
        viewer.trail.clear();
        viewer.open(BlockId(1), true);
        assert_eq!(viewer.unreported_item(), None, "reopening and switching to uses report nothing");
        viewer.reported.clear();
        assert_eq!(viewer.unreported_item(), Some(BlockId(1)), "a new game needs its own report");
    }

    /// A family of recipes cycling and the quests asking for it, drawn by the viewer: a
    /// second later the variant shown has turned in place, and nothing under the pointer
    /// was rebuilt (the cards, the Lay button, the quest links); another language draws
    /// the quest links again.
    #[test]
    fn a_cycling_family_turns_in_place_and_keeps_its_buttons_and_links() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let campfire = content.blocks.id("base:campfire").unwrap();
        let book = Arc::new(RecipeBook::new(&content));
        assert!(book.making(campfire).iter().any(|e| matches!(&e.method, Method::Craft { variants, .. } if variants.len() > 1)), "a campfire is made from any wood");
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default())
            .insert_resource(Time::<()>::default())
            .insert_resource(UiState { recipes_open: true, inventory_open: true, ..default() })
            .insert_resource(Launch { name: "tester".into(), host: false, content })
            .insert_resource(Locale::load(&assets.join("content"), "en"))
            .insert_resource(UiFont(Handle::default()))
            .insert_resource(HudImages { icons: Vec::new(), crosshair: Handle::default(), skills: Vec::new(), effects: Vec::new() })
            .insert_resource(crate::player::LocalPlayer::default())
            .insert_resource(RecipeViewer { book: Some(book), trail: vec![Lookup { item: campfire, uses: false, page: 0 }], ..default() })
            .add_systems(Startup, spawn)
            .add_systems(Update, (draw, cycle).chain());
        let links = |app: &mut App| app.world_mut().query_filtered::<Entity, With<QuestLink>>().iter(app.world()).collect::<Vec<_>>();
        let lay = |app: &mut App| app.world_mut().query_filtered::<Entity, With<LayButton>>().iter(app.world()).collect::<Vec<_>>();
        let cards = |app: &mut App| {
            let body = app.world_mut().query_filtered::<Entity, With<ViewerBody>>().single(app.world()).unwrap();
            app.world().get::<Children>(body).map(|c| c.to_vec()).unwrap_or_default()
        };
        let shown = |app: &mut App| app.world_mut().query_filtered::<&ViewerItem, With<CycleCell>>().iter(app.world()).map(|item| item.0).collect::<Vec<_>>();
        app.update();
        let (first_links, first_lay, first_cards, first_shown) = (links(&mut app), lay(&mut app), cards(&mut app), shown(&mut app));
        assert!(!first_links.is_empty() && !first_lay.is_empty() && !first_cards.is_empty() && !first_shown.is_empty(), "links {first_links:?}, lay {first_lay:?}, cards {first_cards:?}, cycling cells {first_shown:?}");
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs(1));
        app.update();
        assert_ne!(shown(&mut app), first_shown, "a second later the family shows another wood");
        assert_eq!((links(&mut app), lay(&mut app), cards(&mut app)), (first_links.clone(), first_lay, first_cards.clone()), "nothing under the pointer was rebuilt");
        // Logs in the pack change what each way still wants: the cards are drawn again,
        // the quest links are not.
        let log = app.world().resource::<Launch>().content.blocks.id("base:oak_log").unwrap();
        app.world_mut().resource_mut::<crate::player::LocalPlayer>().inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(log, 8));
        app.update();
        assert_ne!(cards(&mut app), first_cards, "what the pack holds redraws the cards");
        assert_eq!(links(&mut app), first_links, "and leaves the quest links");
        app.insert_resource(Locale::load(&assets.join("content"), "ja"));
        app.update();
        let again = links(&mut app);
        assert!(!again.is_empty() && again.iter().all(|link| !first_links.contains(link)), "another language draws the links again: {again:?}");
    }

    /// The quests row is drawn once for an item and language: a family's recipes cycling
    /// (another draw of the same lookup) leaves its links in place to be clicked.
    #[test]
    fn the_quests_row_is_drawn_again_only_for_another_item_or_language() {
        let mut viewer = RecipeViewer::default();
        assert!(viewer.quests_stale(BlockId(1), "en"));
        assert!(!viewer.quests_stale(BlockId(1), "en"), "a cycling redraw keeps the links");
        assert!(viewer.quests_stale(BlockId(1), "ja"));
        assert!(viewer.quests_stale(BlockId(2), "ja"));
        viewer.quests_drawn = None;
        assert!(viewer.quests_stale(BlockId(2), "ja"), "reopened after closing, it is drawn");
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    time: Res<Time>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    images: Res<HudImages>,
    mut viewer: ResMut<RecipeViewer>,
    player: Res<crate::player::LocalPlayer>,
    mut root: Query<&mut Visibility, With<ViewerRoot>>,
    mut title: Query<&mut Text, (With<ViewerTitle>, Without<ViewerPage>)>,
    mut page_text: Query<&mut Text, (With<ViewerPage>, Without<ViewerTitle>)>,
    (body, quests_row): (Query<Entity, With<ViewerBody>>, Query<Entity, With<ViewerQuests>>),
) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
    let Some(lookup) = viewer.trail.last().copied() else {
        (viewer.drawn, viewer.quests_drawn) = (None, None);
        return;
    };
    // Families of recipes show each of their variants in turn, a second each (`cycle`
    // turns them in place): drawn on the variant of this second.
    let tick = time.elapsed_secs() as u32;
    // What each crafting way on the page still wants, from what the pack holds (and the
    // home's stores, standing in it).
    let content = &launch.content;
    let lacks = super::makeable::lacks(content, &player);
    let have = super::makeable::within_reach(&player);
    let wants: Vec<Option<(String, Color)>> = viewer
        .found()
        .iter()
        .skip(lookup.page * PER_PAGE)
        .take(PER_PAGE)
        .map(|entry| {
            let Method::Craft { first, ways, .. } = &entry.method else { return None };
            let near = content.recipes.iter().skip(*first).take(*ways).map(|recipe| super::makeable::reach(recipe, &have, player.station.is_some(), &lacks).unwrap_or_else(|| super::makeable::Reach::Short(recipe.short(&have)))).min_by_key(super::makeable::Reach::rank)?;
            let colour = if near == super::makeable::Reach::Ready { GOLD } else { SOFT };
            Some((super::makeable::words(&locale, content, entry.output, &near), colour))
        })
        .collect();
    let key = (lookup, locale.code().to_owned(), ui.inventory_open, wants.iter().flatten().map(|(w, _)| w.clone()).collect::<Vec<_>>());
    if viewer.drawn.as_ref() == Some(&key) {
        return;
    }
    viewer.drawn = Some(key);
    let quests_stale = viewer.quests_stale(lookup.item, locale.code());
    let name = |item: BlockId| locale.block(content.blocks.def(item));
    let found = viewer.found();
    let pages = found.len().div_ceil(PER_PAGE).max(1);
    if let Ok(mut text) = title.single_mut() {
        text.0 = locale.format(if lookup.uses { "recipes.title.uses" } else { "recipes.title.recipes" }, &[("item", &name(lookup.item))]);
    }
    if let Ok(mut text) = page_text.single_mut() {
        text.0 = if found.is_empty() { String::new() } else { locale.format("recipes.page", &[("page", &(lookup.page + 1)), ("pages", &pages)]) };
    }
    let font = &font.0;
    // The quests that ask for it, whichever way it is looked up.
    if let Ok(row) = quests_row.single()
        && quests_stale
    {
        let asking = voxmar_engine::quest::asking(&content.quests, &content.blocks, lookup.item);
        commands.entity(row).despawn_children().with_children(|row| {
            if asking.is_empty() {
                return;
            }
            row.spawn(text(font, locale.get("recipes.quests"), BODY, MUTED));
            for quest in asking.iter().take(QUESTS_SHOWN) {
                let node = Node { height: Val::Px(crate::ui::widgets::BUTTON), padding: UiRect::horizontal(Val::Px(space::INNER)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() };
                row.spawn((Button, ButtonStyle::Secondary, node, ButtonStyle::Secondary.pieces()[0].image(), QuestLink(quest.name.clone())))
                    .with_child(shadowed(font, super::quests::title(&locale, quest), BODY, TEXT));
            }
            if asking.len() > QUESTS_SHOWN {
                row.spawn(text(font, locale.format("recipes.quests.more", &[("count", &(asking.len() - QUESTS_SHOWN))]), BODY, MUTED));
            }
        });
    }
    let Ok(body) = body.single() else { return };
    let entries: Vec<Entry> = found.iter().skip(lookup.page * PER_PAGE).take(PER_PAGE).map(|e| (*e).clone()).collect();
    commands.entity(body).despawn_children().with_children(|body| {
        if entries.is_empty() {
            // An item no way in survival gives says so, rather than to look for it.
            let creative = content.blocks.def(lookup.item).has_tag(voxmar_engine::recipe_book::CREATIVE_ONLY);
            let key = if lookup.uses { "recipes.none.uses" } else if creative { "recipes.none.creative" } else { "recipes.none.recipes" };
            body.spawn(text(font, locale.format(key, &[("item", &name(lookup.item))]), BODY, MUTED));
        }
        for (entry, wants) in entries.iter().zip(wants.iter().cloned().chain(std::iter::repeat(None))) {
            let card = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), padding: UiRect::all(Val::Px(space::GAP)), ..default() };
            body.spawn((card, Piece::Well.image())).with_children(|card| {
                // What goes in, as it is laid out in the game.
                way_inputs(card, font, &images, content, &entry.method, lookup.uses.then_some(lookup.item), Some(tick));
                card.spawn(shadowed(font, "→", LARGE, GOLD));
                // A fixed count sits on the slot's corner, as in the inventory; a range of
                // amounts is too wide for that and goes with the name instead.
                // Burnt food looked up shows the cooking it is left from, burnt: always one,
                // whatever the dish makes.
                let burnt = !lookup.uses && matches!(&entry.method, Method::Cook { burnt: Some(b), .. } if content.blocks.item_of(*b) == lookup.item && entry.output != lookup.item);
                let output = if burnt { lookup.item } else { entry.output };
                let (low, high) = if burnt { (1, 1) } else { entry.count };
                let count = (high > 1 && low == high).then(|| format!("{high}"));
                item_cell(card, font, &images, Some(output), count);
                let title = if low == high { name(output) } else { locale.format("recipes.amount", &[("item", &name(output)), ("low", &low), ("high", &high)]) };
                // How, in words.
                let words = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), flex_grow: 1.0, min_width: Val::ZERO, ..default() };
                card.spawn(words).with_children(|words| {
                    words.spawn(shadowed(font, title, BODY, TEXT));
                    for (line, colour) in describe(&locale, &launch, &entry.method).into_iter().chain(wants) {
                        words.spawn(text(font, line, BODY, colour));
                    }
                });
                // Over the inventory, where its grid is, a crafting way can be laid in it.
                if ui.inventory_open
                    && let Method::Craft { first, ways, .. } = &entry.method
                {
                    let lay = LayButton { output: entry.output, first: *first as u16, ways: (*ways).min(u8::MAX as usize) as u8 };
                    // As wide as its word needs in every language, never squeezed by the
                    // words beside it.
                    card.spawn(Node { min_width: Val::Px(36.0 * PX), flex_shrink: 0.0, ..default() }).with_children(|cell| {
                        let button = spawn_button(cell, font, "recipes.lay", ButtonStyle::Secondary, lay);
                        cell.commands().entity(button).entry::<Node>().and_modify(|mut node| {
                            node.flex_basis = Val::Auto;
                            node.padding = UiRect::horizontal(Val::Px(space::INNER));
                        });
                    });
                }
            });
        }
    });
}

/// What goes into a way, as it is laid out in the game: the crafting grid (two by two, or
/// three by three for what takes a crafting table) and any block it is made near, a cooked
/// item and its station, or the
/// tool that takes a creature's product. A family of recipes shows the variant with `using`
/// if it has one, else, given the second `tick`, that second's and each in turn after it
/// (`CycleCell`), else its first.
pub(super) fn way_inputs(card: &mut ChildSpawnerCommands, font: &Handle<Font>, images: &HudImages, content: &voxmar_engine::content::Content, method: &Method, using: Option<BlockId>, tick: Option<u32>) {
    match method {
        Method::Craft { variants, near, .. } => {
            let wanted = |cells: &&[Option<BlockId>; GRID]| using.is_some_and(|item| cells.contains(&Some(item)));
            let found = variants.iter().find(wanted);
            let cycles = found.is_none() && variants.len() > 1 && tick.is_some();
            let cells = found.unwrap_or(&variants[tick.unwrap_or(0) as usize % variants.len()]);
            let side = if variants.iter().any(|v| (0..GRID).any(|i| v[i].is_some() && (i % SIDE == 2 || i / SIDE == 2))) { SIDE } else { 2 };
            let grid = Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::px(side as u16, SLOT), row_gap: Val::Px(PX), column_gap: Val::Px(PX), ..default() };
            card.spawn(grid).with_children(|grid| {
                for i in (0..GRID).filter(|i| i % SIDE < side && i / SIDE < side) {
                    let cell = item_cell(grid, font, images, cells[i], None);
                    // Every variant fills the same cells (a family shares its shape).
                    if cycles && cells[i].is_some() {
                        grid.commands().entity(cell).insert(CycleCell(variants.iter().map(|v| v[i].unwrap_or(BlockId::AIR)).collect()));
                    }
                }
            });
            // Made near a block: that block beside the grid, as a cooking station is.
            if let Some(block) = near.as_deref().and_then(|tag| content.blocks.tagged(tag).next()) {
                card.spawn(text(font, "+", BODY, MUTED));
                item_cell(card, font, images, Some(block), None);
            }
        }
        Method::Cook { input, uses, with, station, .. } => {
            // What is held, then what else it takes, each with how many if more than one.
            let count = |n: u8| (n > 1).then(|| n.to_string());
            item_cell(card, font, images, Some(*input), count(*uses));
            for &(item, n) in with {
                card.spawn(text(font, "+", BODY, MUTED));
                item_cell(card, font, images, Some(item), count(n));
            }
            if let Some(fire) = station_block(content, station) {
                card.spawn(text(font, "+", BODY, MUTED));
                item_cell(card, font, images, Some(fire), None);
            }
        }
        Method::Product { tool, .. } => {
            if let Some(tool) = tool.as_deref().and_then(|tag| content.blocks.tagged(tag).next()) {
                item_cell(card, font, images, Some(tool), None);
            }
        }
        Method::Process { machines, inputs, .. } => {
            for (i, &(item, n)) in inputs.iter().enumerate() {
                if i > 0 {
                    card.spawn(text(font, "+", BODY, MUTED));
                }
                item_cell(card, font, images, Some(item), (n > 1).then(|| n.to_string()));
            }
            if let Some(&machine) = machines.first() {
                card.spawn(text(font, "+", BODY, MUTED));
                item_cell(card, font, images, Some(machine), None);
            }
        }
        Method::Work { station, input, takes, .. } => {
            item_cell(card, font, images, Some(*input), (*takes > 1).then(|| takes.to_string()));
            card.spawn(text(font, "+", BODY, MUTED));
            item_cell(card, font, images, Some(*station), None);
        }
        // What plants it (or the crop itself, if players don't), or the wild plant foraged.
        Method::Harvest { from, .. } => {
            item_cell(card, font, images, Some(*from), None);
        }
        Method::Break { block, tool, .. } => {
            item_cell(card, font, images, Some(content.blocks.item_of(*block)), None);
            if let Some(tool) = tool {
                card.spawn(text(font, "+", BODY, MUTED));
                item_cell(card, font, images, Some(*tool), None);
            }
        }
        Method::Trap { trap, .. } => {
            item_cell(card, font, images, Some(*trap), None);
        }
        Method::Bees { block, holding, .. } => {
            if let Some(held) = holding {
                item_cell(card, font, images, Some(*held), None);
                card.spawn(text(font, "+", BODY, MUTED));
            }
            item_cell(card, font, images, Some(content.blocks.item_of(*block)), None);
        }
        Method::Chest { chest, .. } => {
            item_cell(card, font, images, Some(*chest), None);
        }
        Method::Trade { wants: Some((want, n)), .. } => {
            let asked = match want {
                voxmar_engine::wander::Want::Item(item) => Some(*item),
                voxmar_engine::wander::Want::Tag(tag) => content.blocks.tagged(tag).next(),
            };
            item_cell(card, font, images, asked, (*n > 1).then(|| n.to_string()));
        }
        Method::Found { tool: Some(tool), .. } => {
            item_cell(card, font, images, Some(*tool), None);
        }
        Method::Burn { from, .. } | Method::Smoulder { from, .. } => {
            item_cell(card, font, images, Some(*from), None);
        }
        Method::Drop { .. } | Method::Catch { .. } | Method::Trade { wants: None, .. } | Method::Reward { .. } | Method::Found { tool: None, .. } => {}
    }
}

/// The words under a way's result: how it is done, and what it takes.
pub(crate) fn describe(locale: &Locale, launch: &Launch, method: &Method) -> Vec<(String, Color)> {
    let content = &launch.content;
    let creature = |id| locale.creature(&content.creatures.get(id).def);
    let block = |id: BlockId| locale.block(content.blocks.def(id));
    // The level a way takes in its skill (past the first), and how often it comes when not
    // always.
    let level = |skill: Option<voxmar_engine::skill::SkillId>, level: u8| skill.filter(|_| level > 1).map(|id| (locale.format("recipes.level", &[("skill", &locale.skill(content.skills.get(id))), ("level", &level)]), SOFT));
    let chance = |chance: f32| (chance < 1.0).then(|| (locale.format("recipes.chance", &[("chance", &((chance * 100.0).round().max(1.0) as u32))]), SOFT));
    match method {
        Method::Craft { shapeless, near, skill, level: at, .. } => {
            let mut lines = vec![(locale.get(if *shapeless { "recipes.shapeless" } else { "recipes.craft" }).to_owned(), MUTED)];
            if let Some(tag) = near {
                let block = content.blocks.tagged(tag).next().map_or_else(|| tag.clone(), block);
                lines.push((locale.format("recipes.near", &[("block", &block)]), SOFT));
            }
            lines.extend(level(*skill, *at));
            lines
        }
        Method::Process { machines, tier, chance: odds, .. } => {
            let machine = machines.first().map_or_else(String::new, |&m| block(m));
            let mut lines = vec![(locale.format("recipes.process", &[("machine", &machine)]), MUTED)];
            if *tier > 0 {
                lines.push((locale.format("recipes.process.tier", &[("tier", tier)]), SOFT));
            }
            lines.extend(chance(*odds));
            lines
        }
        Method::Work { station, skill, level: at, .. } => [(locale.format("recipes.work", &[("station", &block(*station))]), MUTED)].into_iter().chain(level(Some(*skill), *at)).collect(),
        Method::Harvest { crop, forage, chance: odds, skill, level: at, .. } => {
            let crop = block(content.blocks.item_of(*crop));
            [(locale.format(if *forage { "recipes.forage" } else { "recipes.harvest" }, &[("crop", &crop)]), MUTED)].into_iter().chain(chance(*odds)).chain(level(Some(*skill), *at)).collect()
        }
        Method::Break { block: broken, tool, skill, level: at, chance: odds } => {
            let broken = block(content.blocks.item_of(*broken));
            let line = match tool {
                Some(tool) => locale.format("recipes.break.tool", &[("block", &broken), ("tool", &block(*tool))]),
                None => locale.format("recipes.break", &[("block", &broken)]),
            };
            [(line, MUTED)].into_iter().chain(chance(*odds)).chain(level(*skill, *at)).collect()
        }
        Method::Catch { skill, level: at } => [(locale.get("recipes.catch").to_owned(), MUTED)].into_iter().chain(level(*skill, *at)).collect(),
        Method::Cook { station, skill, level, burnt, .. } => {
            let station = station_block(content, station).map_or_else(|| station.clone(), |block| locale.block(content.blocks.def(block)));
            let mut lines = vec![(locale.format("recipes.cook", &[("station", &station)]), MUTED)];
            lines.push((locale.get("recipes.cook.use").to_owned(), SOFT));
            let skill = skill.map(|id| locale.skill(content.skills.get(id)));
            if let Some(skill) = skill {
                lines.push((locale.format("recipes.level", &[("skill", &skill), ("level", level)]), SOFT));
            }
            if let Some(burnt) = burnt {
                lines.push((locale.format("recipes.burnt", &[("item", &locale.block(content.blocks.def(*burnt)))]), MUTED));
            }
            lines
        }
        Method::Product { creature: id, tool } => {
            let line = match tool.as_deref().and_then(|tag| content.blocks.tagged(tag).next()) {
                Some(tool) => locale.format("recipes.product.tool", &[("creature", &creature(*id)), ("tool", &locale.block(content.blocks.def(tool)))]),
                None => locale.format("recipes.product", &[("creature", &creature(*id))]),
            };
            vec![(line, MUTED)]
        }
        Method::Drop { creature: id, chance } => {
            let percent = (chance.min(1.0) * 100.0).round() as u32;
            vec![(locale.format("recipes.drop", &[("creature", &creature(*id)), ("chance", &percent)]), MUTED)]
        }
        Method::Trap { trap, creature: id, chance: odds, skill, level: at } => {
            [(locale.format("recipes.trap", &[("trap", &block(*trap)), ("creature", &creature(*id))]), MUTED)].into_iter().chain(chance(*odds)).chain(level(Some(*skill), *at)).collect()
        }
        Method::Bees { block: bees, holding, chance: odds, skill, level: at } => {
            let bees = block(content.blocks.item_of(*bees));
            let line = match holding {
                Some(held) => locale.format("recipes.bees.holding", &[("block", &bees), ("item", &block(*held))]),
                None => locale.format("recipes.bees", &[("block", &bees)]),
            };
            [(line, MUTED)].into_iter().chain(chance(*odds)).chain(level(Some(*skill), *at)).collect()
        }
        Method::Chest { chest, chance: odds } => [(locale.format("recipes.chest", &[("chest", &block(*chest))]), MUTED)].into_iter().chain(chance(*odds)).collect(),
        Method::Trade { event, wants, chance: odds } => {
            let party = locale.get(&content.wandering.get(*event).title_key()).to_owned();
            let line = match wants {
                Some((want, n)) => {
                    let asked = match want {
                        voxmar_engine::wander::Want::Item(item) => block(*item),
                        voxmar_engine::wander::Want::Tag(tag) => content.blocks.tagged(tag).next().map_or_else(|| tag.clone(), block),
                    };
                    locale.format("recipes.trade", &[("party", &party), ("count", n), ("item", &asked)])
                }
                None => locale.format("recipes.gift", &[("party", &party)]),
            };
            [(line, MUTED)].into_iter().chain(chance(*odds)).collect()
        }
        Method::Reward { quest } => {
            let title = content.quests.get(quest).map_or_else(|| quest.clone(), |q| super::quests::title(locale, q));
            vec![(locale.format("recipes.reward", &[("quest", &title)]), MUTED)]
        }
        Method::Burn { from, relit: false, .. } => vec![(locale.format("recipes.burnt_out", &[("item", &block(*from))]), MUTED)],
        Method::Burn { from, relit: true, fuel: None } => vec![(locale.format("recipes.relit", &[("item", &block(*from))]), MUTED)],
        Method::Burn { from, relit: true, fuel: Some(tag) } => {
            // Named as the kind of thing it takes (`tag.fat`), never one item of many.
            let fuel = locale.get_or(&format!("tag.{tag}"), &voxmar_engine::lang::title_from_id(tag)).to_owned();
            vec![(locale.format("recipes.relit.with", &[("item", &block(*from)), ("fuel", &fuel)]), MUTED)]
        }
        Method::Smoulder { structure, from, early } => {
            let key = if *early { "recipes.smoulder.early" } else { "recipes.smoulder" };
            let structure = locale.get_or(&voxmar_engine::lang::def_key("multiblock", structure), &voxmar_engine::lang::title_from_id(structure)).to_owned();
            vec![(locale.format(key, &[("item", &block(*from)), ("structure", &structure)]), MUTED)]
        }
        Method::Found { tool, skill, level: at } => {
            let tool = tool.map(|tool| (locale.format("recipes.found.tool", &[("tool", &block(tool))]), SOFT));
            [(locale.get("recipes.found").to_owned(), MUTED)].into_iter().chain(tool).chain(level(*skill, *at)).collect()
        }
    }
}

/// A slot showing `item` (or empty), with a count under it if given; hover it to look it
/// up, click it for its recipes.
pub(super) fn item_cell(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, images: &HudImages, item: Option<BlockId>, count: Option<String>) -> Entity {
    let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), flex_shrink: 0.0, ..default() };
    let mut cell = parent.spawn((node, Piece::Well.image()));
    let id = cell.id();
    let Some(item) = item else { return id };
    cell.insert((Button, Silent, ViewerItem(item), RelativeCursorPosition::default(), crate::ui::widgets::HoverLight)).with_children(|cell| {
        let inset = (SLOT - 16.0 * PX) / 2.0;
        let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
        cell.spawn((icon, ImageNode::new(images.icons.get(item.0 as usize).cloned().unwrap_or_default())));
        if let Some(count) = count {
            cell.spawn((Node { position_type: PositionType::Absolute, right: Val::Px(super::COUNT_INSET), bottom: Val::Px(super::COUNT_INSET), ..default() }, shadowed(font, count, BODY, TEXT)));
        }
    });
    id
}

/// Turns each family's cells to this second's variant, in place: the cell's item (what
/// hovering and clicking it look up) and its icon.
fn cycle(time: Res<Time>, images: Res<HudImages>, mut cells: Query<(&CycleCell, &mut ViewerItem, &Children)>, mut icons: Query<&mut ImageNode>) {
    let tick = time.elapsed_secs() as usize;
    for (cycle, mut shown, children) in &mut cells {
        let item = cycle.0[tick % cycle.0.len()];
        if shown.0 == item {
            continue;
        }
        shown.0 = item;
        if let Some(child) = children.iter().find(|&child| icons.contains(child))
            && let Ok(mut icon) = icons.get_mut(child)
        {
            icon.image = images.icons.get(item.0 as usize).cloned().unwrap_or_default();
        }
    }
}
