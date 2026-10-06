//! The recipe viewer, as NEI does it: over any item in the inventory (pack, hotbar,
//! crafting grid and what it makes) or in the viewer itself, R shows the recipes that make
//! it and U everything it goes into. Each way is drawn as it happens in the game: the
//! crafting grid, a station, a creature with the tool that takes its product, a creature
//! that drops it. Pages turn with the arrows; clicking an item looks up its recipes (a
//! right click, its uses), and Back retraces the steps.
//!
//! The book comes from the game's content (`voxmar_engine::recipe_book`), so a package's
//! recipes show here as soon as it adds them.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use voxmar_engine::block::BlockId;
use voxmar_engine::craft::GRID;
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
const PER_PAGE: usize = 2;
const WIDTH: f32 = 300.0 * PX;

pub struct RecipesPlugin;

impl Plugin for RecipesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecipeViewer>()
            .init_resource::<HoveredItem>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (look_up, click, draw).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut viewer: ResMut<RecipeViewer>, mut ui: ResMut<UiState>| {
                viewer.trail.clear();
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
    drawn: Option<(Lookup, String, u32)>,
}

impl RecipeViewer {
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

    fn pages(&self) -> usize {
        self.found().len().div_ceil(PER_PAGE).max(1)
    }
}

#[derive(Component)]
struct ViewerRoot;
#[derive(Component)]
struct ViewerTitle;
#[derive(Component)]
struct ViewerBody;
#[derive(Component)]
struct ViewerPage;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ViewerButton {
    Previous,
    Next,
    Back,
}
/// An item drawn in the viewer: hover to look it up, click for its recipes.
#[derive(Component)]
struct ViewerItem(BlockId);

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((ViewerRoot, screen(SCRIM), Visibility::Hidden, GlobalZIndex(5))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            let title = Node { justify_content: JustifyContent::Center, margin: UiRect::bottom(Val::Px(space::GAP)), ..default() };
            card.spawn(title).with_child((shadowed(font, "", LARGE, GOLD), ViewerTitle));
            let body = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::GAP), min_height: Val::Px(2.0 * (2.0 * SLOT + space::SECTION * 2.0)), ..default() };
            card.spawn((body, ViewerBody));
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
    items: Query<(&ViewerItem, &RelativeCursorPosition)>,
    start: Option<Res<StartLookup>>,
    mut commands: Commands,
) {
    if viewer.book.is_none() {
        viewer.book = Some(Arc::new(RecipeBook::new(&launch.content)));
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
    if ui.recipes_open {
        hovered.0 = items.iter().find(|(_, cursor)| cursor.cursor_over()).map(|(item, _)| item.0);
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

fn click(
    buttons: Query<(&ViewerButton, &Interaction), Changed<Interaction>>,
    items: Query<(&ViewerItem, &Interaction, &RelativeCursorPosition), Changed<Interaction>>,
    mouse: Res<ButtonInput<MouseButton>>,
    hovered_items: Query<(&ViewerItem, &RelativeCursorPosition)>,
    mut viewer: ResMut<RecipeViewer>,
    mut ui: ResMut<UiState>,
) {
    if viewer.trail.is_empty() {
        return;
    }
    let pages = viewer.pages();
    for (button, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
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
    for (item, interaction, _) in &items {
        if *interaction == Interaction::Pressed {
            viewer.open(item.0, false);
        }
    }
    // A right click on an item looks up its uses (Interaction answers only the left).
    if mouse.just_pressed(MouseButton::Right)
        && let Some((item, _)) = hovered_items.iter().find(|(_, cursor)| cursor.cursor_over())
    {
        viewer.open(item.0, true);
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
    mut root: Query<&mut Visibility, With<ViewerRoot>>,
    mut title: Query<&mut Text, (With<ViewerTitle>, Without<ViewerPage>)>,
    mut page_text: Query<&mut Text, (With<ViewerPage>, Without<ViewerTitle>)>,
    body: Query<Entity, With<ViewerBody>>,
) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
    let Some(lookup) = viewer.trail.last().copied() else {
        viewer.drawn = None;
        return;
    };
    // Families of recipes show each of their variants in turn, a second each.
    let cycles = viewer.found().iter().skip(lookup.page * PER_PAGE).take(PER_PAGE).any(|e| matches!(&e.method, Method::Craft { variants, .. } if variants.len() > 1));
    let tick = time.elapsed_secs() as u32;
    let key = (lookup, locale.code().to_owned(), if cycles { tick } else { 0 });
    if viewer.drawn.as_ref() == Some(&key) {
        return;
    }
    viewer.drawn = Some(key);
    let content = &launch.content;
    let name = |item: BlockId| locale.block(content.blocks.def(item));
    let found = viewer.found();
    let pages = found.len().div_ceil(PER_PAGE).max(1);
    if let Ok(mut text) = title.single_mut() {
        text.0 = locale.format(if lookup.uses { "recipes.title.uses" } else { "recipes.title.recipes" }, &[("item", &name(lookup.item))]);
    }
    if let Ok(mut text) = page_text.single_mut() {
        text.0 = if found.is_empty() { String::new() } else { locale.format("recipes.page", &[("page", &(lookup.page + 1)), ("pages", &pages)]) };
    }
    let Ok(body) = body.single() else { return };
    let font = &font.0;
    let entries: Vec<Entry> = found.iter().skip(lookup.page * PER_PAGE).take(PER_PAGE).map(|e| (*e).clone()).collect();
    commands.entity(body).despawn_children().with_children(|body| {
        if entries.is_empty() {
            let key = if lookup.uses { "recipes.none.uses" } else { "recipes.none.recipes" };
            body.spawn(text(font, locale.format(key, &[("item", &name(lookup.item))]), BODY, MUTED));
        }
        for entry in &entries {
            let card = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), padding: UiRect::all(Val::Px(space::GAP)), ..default() };
            body.spawn((card, Piece::Well.image())).with_children(|card| {
                // What goes in, as it is laid out in the game.
                match &entry.method {
                    Method::Craft { variants, .. } => {
                        // Looking up a use, the variant with that item; else they take turns.
                        let wanted = |cells: &&[Option<BlockId>; GRID]| lookup.uses && cells.contains(&Some(lookup.item));
                        let cells = variants.iter().find(wanted).unwrap_or(&variants[tick as usize % variants.len()]);
                        let grid = Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::px(2, SLOT), row_gap: Val::Px(PX), column_gap: Val::Px(PX), ..default() };
                        card.spawn(grid).with_children(|grid| {
                            for cell in cells.iter().take(GRID) {
                                item_cell(grid, font, &images, *cell, None);
                            }
                        });
                    }
                    Method::Cook { input, station, .. } => {
                        item_cell(card, font, &images, Some(*input), None);
                        if let Some(fire) = content.blocks.tagged(station).next() {
                            card.spawn(text(font, "+", BODY, MUTED));
                            item_cell(card, font, &images, Some(fire), None);
                        }
                    }
                    Method::Product { tool, .. } => {
                        if let Some(tool) = tool.as_deref().and_then(|tag| content.blocks.tagged(tag).next()) {
                            item_cell(card, font, &images, Some(tool), None);
                        }
                    }
                    Method::Drop { .. } => {}
                }
                card.spawn(shadowed(font, "→", LARGE, GOLD));
                // A fixed count sits on the slot's corner, as in the inventory; a range of
                // amounts is too wide for that and goes with the name instead.
                let (low, high) = entry.count;
                let count = (high > 1 && low == high).then(|| format!("{high}"));
                item_cell(card, font, &images, Some(entry.output), count);
                let title = if low == high { name(entry.output) } else { locale.format("recipes.amount", &[("item", &name(entry.output)), ("low", &low), ("high", &high)]) };
                // How, in words.
                let words = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), flex_grow: 1.0, min_width: Val::ZERO, ..default() };
                card.spawn(words).with_children(|words| {
                    words.spawn(shadowed(font, title, BODY, TEXT));
                    for (line, colour) in describe(&locale, &launch, &entry.method) {
                        words.spawn(text(font, line, BODY, colour));
                    }
                });
            });
        }
    });
}

/// The words under a way's result: how it is done, and what it takes.
fn describe(locale: &Locale, launch: &Launch, method: &Method) -> Vec<(String, Color)> {
    let content = &launch.content;
    let creature = |id| locale.creature(&content.creatures.get(id).def);
    match method {
        Method::Craft { shapeless, .. } => vec![(locale.get(if *shapeless { "recipes.shapeless" } else { "recipes.craft" }).to_owned(), MUTED)],
        Method::Cook { station, level, burnt, .. } => {
            let station = content.blocks.tagged(station).next().map_or_else(|| station.clone(), |block| locale.block(content.blocks.def(block)));
            let mut lines = vec![(locale.format("recipes.cook", &[("station", &station)]), MUTED)];
            let skill = content.cooking.skill.map(|id| locale.skill(content.skills.get(id)));
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
    }
}

/// A slot showing `item` (or empty), with a count under it if given; hover it to look it
/// up, click it for its recipes.
fn item_cell(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, images: &HudImages, item: Option<BlockId>, count: Option<String>) {
    let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), flex_shrink: 0.0, ..default() };
    let mut cell = parent.spawn((node, Piece::Well.image()));
    let Some(item) = item else { return };
    cell.insert((Button, Silent, ViewerItem(item), RelativeCursorPosition::default(), crate::ui::widgets::HoverLight)).with_children(|cell| {
        let inset = (SLOT - 16.0 * PX) / 2.0;
        let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
        cell.spawn((icon, ImageNode::new(images.icons.get(item.0 as usize).cloned().unwrap_or_default())));
        if let Some(count) = count {
            cell.spawn((Node { position_type: PositionType::Absolute, right: Val::Px(super::COUNT_INSET), bottom: Val::Px(super::COUNT_INSET), ..default() }, shadowed(font, count, BODY, TEXT)));
        }
    });
}
