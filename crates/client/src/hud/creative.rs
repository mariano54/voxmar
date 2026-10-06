//! Creative mode's catalogue: beside the inventory, every item in the game, a page at a
//! time, sorted into tabs and found by typing any part of its name in English or Japanese
//! (kanji, kana or the reading in kana). Click one to hold one more of it, right click to
//! hold a stack, Shift-click to put a stack into the inventory; held, it goes anywhere a
//! held stack goes. The server gives nothing to a player in survival.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{RelativeCursorPosition, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use voxmar_engine::block::{BlockDef, BlockId, RenderKind};
use voxmar_engine::inventory::MAX_STACK;

use super::{HudImages, SLOT};
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::Silent;
use crate::ui::widgets::{BODY, Focused, HoverLight, MUTED, SOFT, TEXT, TextField, UiFont, shadowed, space, spawn_text_field, text};

pub struct CreativePlugin;

impl Plugin for CreativePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Catalogue>()
            .add_systems(PostStartup, spawn)
            .add_systems(PreUpdate, hold_keys_while_typing.after(bevy::input::InputSystems))
            .add_systems(Update, (index, start_search, show, lay_out, read_input, draw).chain().run_if(in_state(Screen::InGame)));
    }
}

/// The catalogue's tabs, each a kind of item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    All,
    Blocks,
    Plants,
    Food,
    Items,
}

const TABS: [Tab; 5] = [Tab::All, Tab::Blocks, Tab::Plants, Tab::Food, Tab::Items];

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::All => "creative.tab.all",
            Tab::Blocks => "creative.tab.blocks",
            Tab::Plants => "creative.tab.plants",
            Tab::Food => "creative.tab.food",
            Tab::Items => "creative.tab.items",
        }
    }

    /// The tab an item belongs in, besides All.
    fn of(def: &BlockDef) -> Tab {
        let tagged = |tags: &[&str]| def.tags.iter().any(|tag| tags.contains(&tag.as_str()));
        if tagged(&["edible"]) {
            Tab::Food
        } else if def.render == RenderKind::Plant || def.render == RenderKind::Leaves || tagged(&["plant", "flower", "fungus", "bush", "crop", "seed", "sapling", "leaves"]) {
            Tab::Plants
        } else if def.solid {
            Tab::Blocks
        } else {
            Tab::Items
        }
    }
}

/// One item, as the search knows it.
struct Entry {
    item: BlockId,
    tab: Tab,
    /// Everything it is called, in lower case with katakana as hiragana, one name a line.
    words: String,
}

/// A search to open the catalogue on (`--catalogue`), in creative mode.
#[derive(Resource)]
pub struct StartSearch(pub String);

/// What the catalogue shows.
#[derive(Resource, Default)]
pub struct Catalogue {
    entries: Vec<Entry>,
    pub tab: Tab,
    /// What is typed in the search box.
    pub query: String,
    /// The items the tab and the search leave, in the content's order.
    pub found: Vec<BlockId>,
    pub page: usize,
    /// Cells across and down, as the panel has room for.
    columns: usize,
    rows: usize,
    /// The item under the pointer, for its name beside it and the recipe keys.
    pub hovered: Option<BlockId>,
    /// The query and tab `found` was worked out for.
    found_for: Option<(String, Tab)>,
}

impl Catalogue {
    fn per_page(&self) -> usize {
        (self.columns * self.rows).max(1)
    }

    fn pages(&self) -> usize {
        self.found.len().div_ceil(self.per_page()).max(1)
    }

    /// Works out what the tab and the search leave, if either changed.
    pub fn refresh(&mut self) {
        let wanted = (self.query.clone(), self.tab);
        if self.found_for.as_ref() == Some(&wanted) {
            return;
        }
        let terms: Vec<String> = fold(&self.query).split_whitespace().map(str::to_owned).collect();
        self.found = self
            .entries
            .iter()
            .filter(|entry| self.tab == Tab::All || entry.tab == self.tab)
            .filter(|entry| terms.iter().all(|term| entry.words.contains(term.as_str())))
            .map(|entry| entry.item)
            .collect();
        self.page = 0;
        self.found_for = Some(wanted);
    }
}

/// Lower case, and katakana as hiragana, so アイテム finds あいてむ and the other way round.
fn fold(text: &str) -> String {
    text.to_lowercase().chars().map(|c| if ('\u{30A1}'..='\u{30F6}').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c }).collect()
}

/// Puts `item` on the pointer (one, or a stack) or a stack of it into the inventory, if the
/// player is in creative mode.
pub fn take(net: &Net, item: BlockId, held: bool, stack: bool) {
    net.take(item, if stack { MAX_STACK } else { 1 }, held);
}

#[derive(Component)]
struct CataloguePanel;
#[derive(Component)]
struct SearchField;
#[derive(Component)]
struct TabButton(Tab);
#[derive(Component)]
struct TabIcon(Tab);
#[derive(Component)]
struct Shown;
/// Where the cells go: as much of the panel as the rest leaves.
#[derive(Component)]
struct GridArea;
#[derive(Component)]
struct Cells;
#[derive(Component)]
struct Cell(usize);
#[derive(Component)]
struct CellIcon;
#[derive(Component)]
struct PageButton(i32);
#[derive(Component)]
struct PageText;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let panel = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(space::OUTER),
        top: Val::Px(space::OUTER),
        bottom: Val::Px(space::OUTER),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(space::GAP),
        padding: UiRect::all(Val::Px(space::OUTER)),
        ..default()
    };
    // A click on it is a click on the inventory's screen, not one outside to throw with.
    let card = super::inventory::InventoryCard;
    commands.spawn((CataloguePanel, card, panel, Piece::Panel.image(), RelativeCursorPosition::default(), Visibility::Hidden, GlobalZIndex(2))).with_children(|panel| {
        spawn_text_field(panel, font, "", "creative.search", 32, 0, false, SearchField);
        panel.spawn(Node { column_gap: Val::Px(PX), ..default() }).with_children(|tabs| {
            for tab in TABS {
                let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), flex_shrink: 0.0, ..default() };
                tabs.spawn((Button, Silent, HoverLight, TabButton(tab), node, Piece::Well.image())).with_children(|cell| {
                    let inset = (SLOT - 16.0 * PX) / 2.0;
                    let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                    cell.spawn((icon, ImageNode::default(), TabIcon(tab)));
                });
            }
        });
        panel.spawn((shadowed(font, "", BODY, TEXT), TightLine, Shown));
        let area = Node { flex_grow: 1.0, min_height: Val::Px(0.0), overflow: Overflow::clip(), ..default() };
        panel.spawn((area, GridArea, RelativeCursorPosition::default())).with_children(|area| {
            let cells = Node {
                display: Display::Grid,
                row_gap: Val::Px(PX),
                column_gap: Val::Px(PX),
                align_content: AlignContent::Start,
                ..default()
            };
            area.spawn((cells, Cells));
        });
        panel.spawn(Node { align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|pager| {
            for (step, arrow) in [(-1, "◂"), (1, "▸")] {
                let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() };
                pager.spawn((Button, HoverLight, PageButton(step), node, Piece::Well.image())).with_children(|b| {
                    b.spawn((shadowed(font, arrow, BODY, TEXT), TightLine));
                });
                if step < 0 {
                    pager.spawn((text(font, "", BODY, SOFT), TightLine, PageText));
                }
            }
        });
        panel.spawn((text(font, "", BODY, MUTED), crate::lang::Localized("creative.hint")));
    });
}

/// Reads every item's names once the game's content is there.
fn index(launch: Res<Launch>, mut catalogue: ResMut<Catalogue>) {
    if !catalogue.entries.is_empty() {
        return;
    }
    let content_dir = voxmar_server::assets_dir().join("content");
    let languages = [Locale::load(&content_dir, "en"), Locale::load(&content_dir, "ja")];
    let blocks = &launch.content.blocks;
    catalogue.entries = blocks
        .ids()
        .filter(|&id| !id.is_air() && blocks.def(id).render != RenderKind::Invisible)
        .map(|item| {
            let def = blocks.def(item);
            let mut words = vec![def.name.rsplit(':').next().unwrap_or(&def.name).replace('_', " ")];
            for locale in &languages {
                words.push(locale.block(def));
                words.extend(locale.block_reading(def));
            }
            Entry { item, tab: Tab::of(def), words: fold(&words.join("\n")) }
        })
        .collect();
    catalogue.found_for = None;
}

/// `--catalogue`'s search, once the player is in creative mode.
fn start_search(mut commands: Commands, start: Option<Res<StartSearch>>, player: Res<LocalPlayer>, mut catalogue: ResMut<Catalogue>, mut ui: ResMut<UiState>) {
    let Some(start) = start else { return };
    if !player.allow_flight {
        return;
    }
    catalogue.query.clone_from(&start.0);
    ui.inventory_open = true;
    commands.remove_resource::<StartSearch>();
}

/// Up beside the inventory in creative mode.
fn show(
    mut commands: Commands,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    mut panel: Query<&mut Visibility, With<CataloguePanel>>,
    search: Query<Entity, (With<SearchField>, With<Focused>)>,
) {
    let open = ui.inventory_open && !ui.recipes_open && player.allow_flight;
    if let Ok(mut visibility) = panel.single_mut() {
        visibility.set_if_neq(if open { Visibility::Inherited } else { Visibility::Hidden });
    }
    if !open {
        for field in &search {
            commands.entity(field).remove::<Focused>();
        }
    }
}

/// Sizes the panel to the room right of the inventory, and makes as many cells as fit.
#[allow(clippy::too_many_arguments)]
fn lay_out(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<&Camera, With<Camera3d>>,
    scale: Res<UiScale>,
    images: Res<HudImages>,
    mut catalogue: ResMut<Catalogue>,
    inventory: Query<(&UiGlobalTransform, &ComputedNode), (With<super::inventory::InventoryCard>, Without<CataloguePanel>)>,
    mut panel: Query<(&mut Node, &Visibility), With<CataloguePanel>>,
    area: Query<&ComputedNode, With<GridArea>>,
    mut cells: Query<(Entity, &mut Node), (With<Cells>, Without<CataloguePanel>)>,
) {
    let Ok((mut node, visibility)) = panel.single_mut() else { return };
    if *visibility == Visibility::Hidden {
        return;
    }
    let (Some(view), Ok((place, card))) = (crate::ui::view_size(&windows, &cameras), inventory.single()) else { return };
    let view = view / scale.0;
    let right_of_card = (place.translation.x + card.size().x / 2.0) * card.inverse_scale_factor();
    let room = view.x - right_of_card - 2.0 * space::OUTER;
    let columns = (((room - 2.0 * space::OUTER + PX) / (SLOT + PX)).floor() as usize).clamp(1, 12);
    let width = columns as f32 * (SLOT + PX) - PX + 2.0 * space::OUTER;
    if node.width != Val::Px(width) {
        node.width = Val::Px(width);
    }
    let Ok(area) = area.single() else { return };
    let height = area.size().y * area.inverse_scale_factor();
    let rows = (((height + PX) / (SLOT + PX)).floor() as usize).max(1);
    if (columns, rows) == (catalogue.columns, catalogue.rows) {
        return;
    }
    catalogue.columns = columns;
    catalogue.rows = rows;
    let Ok((grid, mut grid_node)) = cells.single_mut() else { return };
    grid_node.grid_template_columns = RepeatedGridTrack::px(columns as u16, SLOT);
    commands.entity(grid).despawn_children().with_children(|grid| {
        for i in 0..columns * rows {
            let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
            grid.spawn((Button, Silent, HoverLight, Cell(i), RelativeCursorPosition::default(), node, Piece::Well.image(), Visibility::Hidden)).with_children(|cell| {
                let inset = (SLOT - 16.0 * PX) / 2.0;
                let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                cell.spawn((icon, ImageNode::new(images.icons.first().cloned().unwrap_or_default()), CellIcon));
            });
        }
    });
}

/// Typing, tabs, pages, the wheel, and clicks on items.
#[allow(clippy::too_many_arguments)]
fn read_input(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    net: Res<Net>,
    mut catalogue: ResMut<Catalogue>,
    panel: Query<(&Visibility, &RelativeCursorPosition), With<CataloguePanel>>,
    search: Query<(Entity, &TextField, &Interaction), With<SearchField>>,
    tabs: Query<(&TabButton, &Interaction), Changed<Interaction>>,
    pages: Query<(&PageButton, &Interaction), Changed<Interaction>>,
    cells: Query<(&Cell, &RelativeCursorPosition, &InheritedVisibility)>,
    mut last_typed: Local<String>,
) {
    let scrolled: f32 = wheel.read().map(|event| if event.unit == MouseScrollUnit::Line { event.y } else { event.y / 32.0 }).sum();
    let Ok((visibility, over_panel)) = panel.single() else { return };
    if *visibility == Visibility::Hidden {
        catalogue.hovered = None;
        return;
    }
    if let Ok((field, typed, interaction)) = search.single() {
        // What is typed replaces the search; a search set otherwise (`--catalogue`, the
        // gate) stands until then.
        if *last_typed != typed.value {
            last_typed.clone_from(&typed.value);
            catalogue.query.clone_from(&typed.value);
        }
        // A click anywhere else stops the typing, so the keys work again.
        if mouse.get_just_pressed().next().is_some() && *interaction == Interaction::None {
            commands.entity(field).remove::<Focused>();
        }
    }
    for (tab, interaction) in &tabs {
        if *interaction == Interaction::Pressed {
            catalogue.tab = tab.0;
        }
    }
    catalogue.refresh();
    let pages_count = catalogue.pages();
    let turn = |by: i32, catalogue: &mut Catalogue| catalogue.page = (catalogue.page as i32 + by).rem_euclid(pages_count as i32) as usize;
    for (page, interaction) in &pages {
        if *interaction == Interaction::Pressed {
            turn(page.0, &mut catalogue);
        }
    }
    if scrolled.abs() >= 0.5 && over_panel.cursor_over() {
        turn(if scrolled > 0.0 { -1 } else { 1 }, &mut catalogue);
    }
    catalogue.page = catalogue.page.min(pages_count - 1);
    let start = catalogue.page * catalogue.per_page();
    catalogue.hovered = cells.iter().find(|(_, cursor, visible)| visible.get() && cursor.cursor_over()).and_then(|(cell, ..)| catalogue.found.get(start + cell.0).copied());
    let Some(item) = catalogue.hovered else { return };
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if mouse.just_pressed(MouseButton::Left) {
        take(&net, item, !shift, shift);
    } else if mouse.just_pressed(MouseButton::Right) {
        take(&net, item, true, true);
    }
}

/// Fills the cells with the page's items, and says what the tab holds.
#[allow(clippy::too_many_arguments)]
fn draw(
    catalogue: Res<Catalogue>,
    locale: Res<Locale>,
    images: Res<HudImages>,
    mut cells: Query<(&Cell, &Children, &mut Visibility)>,
    mut icons: Query<&mut ImageNode, With<CellIcon>>,
    mut tab_icons: Query<(&TabIcon, &mut ImageNode), Without<CellIcon>>,
    mut tab_wells: Query<(&TabButton, &mut ImageNode), (Without<CellIcon>, Without<TabIcon>)>,
    mut shown: Query<&mut Text, (With<Shown>, Without<PageText>)>,
    mut page_text: Query<&mut Text, (With<PageText>, Without<Shown>)>,
) {
    let icon = |item: BlockId| images.icons.get(item.0 as usize).cloned().unwrap_or_default();
    let start = catalogue.page * catalogue.per_page();
    for (cell, children, mut visibility) in &mut cells {
        let item = catalogue.found.get(start + cell.0).copied();
        visibility.set_if_neq(if item.is_some() { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(item) = item
            && let Some(mut image) = children.first().and_then(|&child| icons.get_mut(child).ok())
        {
            let wanted = icon(item);
            if image.image != wanted {
                image.image = wanted;
            }
        }
    }
    for (tab, mut image) in &mut tab_icons {
        let first = catalogue.entries.iter().find(|entry| tab.0 == Tab::All || entry.tab == tab.0).map(|entry| entry.item);
        if let Some(wanted) = first.map(icon)
            && image.image != wanted
        {
            image.image = wanted;
        }
    }
    for (tab, mut image) in &mut tab_wells {
        let wanted = if tab.0 == catalogue.tab { Piece::WellFocus } else { Piece::Well }.image().image;
        if image.image != wanted {
            image.image = wanted;
        }
    }
    let words = if catalogue.found.is_empty() {
        locale.get("creative.none").to_owned()
    } else {
        locale.format("creative.shown", &[("tab", &locale.get(catalogue.tab.key())), ("count", &catalogue.found.len())])
    };
    if let Ok(mut text) = shown.single_mut()
        && text.0 != words
    {
        text.0 = words;
    }
    let words = locale.format("creative.page", &[("page", &(catalogue.page + 1)), ("pages", &catalogue.pages())]);
    if let Ok(mut text) = page_text.single_mut()
        && text.0 != words
    {
        text.0 = words;
    }
}

/// While the search box is being typed in, the game's keys (E closes the inventory, Q
/// throws, 1 to 9 swap) are letters; Esc still closes.
fn hold_keys_while_typing(mut keys: ResMut<ButtonInput<KeyCode>>, search: Query<(), (With<SearchField>, With<Focused>)>) {
    if search.is_empty() {
        return;
    }
    let pressed: Vec<KeyCode> = keys.get_just_pressed().copied().filter(|&key| key != KeyCode::Escape).collect();
    for key in pressed {
        keys.clear_just_pressed(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_katakana() {
        assert_eq!(fold("Oak ログ"), "oak ろぐ");
        assert!(fold("オークの原木\nおーくのげんぼく").contains(&fold("げんぼく")));
    }
}
