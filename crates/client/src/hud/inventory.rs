//! The inventory screen, with Minecraft's mouse and keyboard handling:
//!
//! - click picks a stack up, puts it down, merges it or swaps it;
//! - right click (or Control-click) picks up half a stack, or puts down one item;
//! - drag a held stack across slots to spread it evenly (left button) or one by one
//!   (right button), previewed while dragging;
//! - double click gathers every stack of the item onto the pointer;
//! - shift click sends a stack between the hotbar and the rest, and shift double click
//!   sends every stack of that item;
//! - 1 to 9 over a slot swaps it with that hotbar slot;
//! - Q over a slot throws one item out into the world, Ctrl+Q the stack, and a click
//!   outside the panel throws the held stack (a right click, one item of it);
//! - hovering an item names it, coloured by rarity, with what it is good for, and for a
//!   seed the level it takes to plant;
//! - under the slots, every skill's level and how far it is to the next;
//! - an open store (a chest, a cupboard, a jar) shows its slots in place of the crafting
//!   grid, and shift click sends stacks between it and the pack.
//!
//! Each gesture becomes moves of the engine's inventory rules (`voxmar_engine::inventory`),
//! proposed to the server as `core:transfer`, `core:quick_move`, `core:stow` and `core:drop` actions and
//! predicted here at once. Closing the screen puts a held stack back.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::block::{BlockDef, Rarity};
use voxmar_engine::craft::GRID;
use voxmar_engine::inventory::{LAST, WORN, CRAFT, CURSOR, HOTBAR_SLOTS, Inventory, ItemStack, SLOTS, STORE, STORE_SLOTS};

use super::{SLOT, slot_contents};
use crate::lang::{Locale, Localized, TightLine};
use crate::Actions;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::{Silent, UiSound};
use crate::session::Launch;
use crate::ui::widgets::{BODY, ERROR, GOLD, LARGE, MUTED, SCRIM, SOFT, TEXT, UiFont, panel, screen, shadowed, spawn_heading, text};
use crate::ui::widgets::space;
use crate::world::ClientWorld;

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShownInventory>()
            .init_resource::<Gesture>()
            .init_resource::<NearTags>()
            .add_systems(PostStartup, spawn)
            .add_systems(
                Update,
                ((toggle, fit_grid, fit_store, click, find_near, craft, number_keys, preview).chain().in_set(InventorySystems).after(super::screens::ScreenSystems), (follow_pointer, tooltip, tap, hover_item, show_worn_glyphs).after(InventorySystems))
                    .run_if(in_state(Screen::InGame)),
            )
            .add_systems(OnExit(Screen::InGame), |mut gesture: ResMut<Gesture>, mut tips: Query<&mut Visibility, With<Tooltip>>| {
                *gesture = Gesture::default();
                for mut tip in &mut tips {
                    *tip = Visibility::Hidden;
                }
            });
    }
}

/// The tags recipes are made near (a chopping block's) that some block within the player's
/// reach has, as the server judges it: kept while the inventory is open.
#[derive(Resource, Default)]
pub(crate) struct NearTags(pub Vec<String>);

impl NearTags {
    pub(crate) fn has(&self, tag: &str) -> bool {
        self.0.iter().any(|t| t == tag)
    }
}

fn find_near(ui: Res<UiState>, world: Res<ClientWorld>, player: Res<LocalPlayer>, launch: Res<Launch>, mut near: ResMut<NearTags>) {
    if !ui.inventory_open {
        return;
    }
    let tags = launch.content.near_tags();
    let found = voxmar_engine::craft::tags_near(&tags, &launch.content.blocks, player.eye(), |pos| world.block(pos));
    if near.0 != found {
        near.0 = found;
    }
}

/// Reading the inventory screen's input and working out what it shows.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InventorySystems;

/// The inventory as the screen shows it: the player's, with a drag in progress spread.
#[derive(Resource, Default)]
pub struct ShownInventory(pub Inventory);

/// Seconds between two clicks on a slot for them to be a double click.
const DOUBLE_CLICK: f32 = 0.3;

/// What the mouse is in the middle of.
#[derive(Resource, Default)]
struct Gesture {
    /// A held stack being spread: by the left button (evenly) or the right (one each),
    /// over these slots so far.
    drag: Option<(MouseButton, Vec<usize>)>,
    /// The last left click: its slot, when, whether it picked a stack up and, for a shift
    /// click, the item it sent.
    last_click: Option<LastClick>,
}

#[derive(Clone, Copy)]
struct LastClick {
    slot: usize,
    at: f32,
    picked_up: bool,
    sent: Option<voxmar_engine::block::BlockId>,
}

#[derive(Component)]
struct InventoryScreen;
#[derive(Component)]
struct InventorySlot(usize);
/// The faint mark in an empty worn well of what it takes.
#[derive(Component)]
pub(crate) struct WornGlyph(pub(crate) usize);

/// The mark for worn slot `slot`, if it is one, by `combat::Slot`'s order.
fn worn_glyph(slot: usize) -> Option<Piece> {
    let at = slot.checked_sub(WORN).filter(|_| slot <= LAST)?;
    [Piece::WornHead, Piece::WornBody, Piece::WornLegs, Piece::WornHands, Piece::WornFeet, Piece::WornShield, Piece::WornCharm].get(at).copied()
}

/// What packages restyle of the interface (`interface.ron`, in `core`); a later package
/// changes what it gives. Read with the rest of the content at launch, which a bad value
/// fails.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InterfaceStyle {
    /// How strongly an empty worn well shows what it takes, 0 to 1.
    pub worn_glyph_alpha: f32,
    /// Blocks from home within which the way home is not shown (`hud::home`).
    pub home_hidden_within: f32,
    /// Blocks from home beyond which the way home shows by day too.
    pub home_shown_beyond: f32,
    /// When the night it shows through starts and ends, in ticks after sunrise.
    pub home_dusk: u64,
    pub home_dawn: u64,
    /// Seconds the mark takes to fade in or out.
    pub home_fade: f32,
    /// The item whose icon marks home, if a package names one.
    pub home_icon: Option<String>,
    /// A quest's icon on the book's web at each zoom, smallest first, in interface pixels:
    /// whole multiples of its 16 texels' half, so the pixel art stays sharp.
    pub book_zooms: Vec<f32>,
    /// Which of `book_zooms` the book opens at.
    pub book_zoom_at: usize,
    /// How much of the window the book takes across, in percent; down, it takes all from
    /// under the bar of the screens' keys to the HUD at the bottom.
    pub book_width: f32,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InterfaceFile {
    worn_glyph_alpha: Option<f32>,
    home_hidden_within: Option<f32>,
    home_shown_beyond: Option<f32>,
    home_dusk: Option<u64>,
    home_dawn: Option<u64>,
    home_fade: Option<f32>,
    home_icon: Option<String>,
    book_zooms: Option<Vec<f32>>,
    book_zoom_at: Option<usize>,
    book_width: Option<f32>,
}

impl InterfaceStyle {
    pub const FILE: &str = "interface.ron";

    pub fn load(files: &[voxmar_engine::content::ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let (mut alpha, mut hidden, mut icon) = (None, None, None);
        let (mut beyond, mut dusk, mut dawn, mut fade) = (None, None, None, None);
        let (mut zooms, mut zoom_at, mut width) = (None, None, None);
        for file in files.iter().filter(|f| f.name == Self::FILE) {
            let given: InterfaceFile = options.from_str(&file.source).map_err(|e| format!("{}/{}: {e}", file.package, Self::FILE))?;
            alpha = given.worn_glyph_alpha.or(alpha);
            hidden = given.home_hidden_within.or(hidden);
            beyond = given.home_shown_beyond.or(beyond);
            dusk = given.home_dusk.or(dusk);
            dawn = given.home_dawn.or(dawn);
            fade = given.home_fade.or(fade);
            icon = given.home_icon.or(icon);
            zooms = given.book_zooms.or(zooms);
            zoom_at = given.book_zoom_at.or(zoom_at);
            width = given.book_width.or(width);
        }
        let worn_glyph_alpha = alpha.ok_or(format!("no package's {} gives worn_glyph_alpha", Self::FILE))?;
        if !(0.0..=1.0).contains(&worn_glyph_alpha) {
            return Err(format!("{}: worn_glyph_alpha {worn_glyph_alpha} is not from 0 to 1", Self::FILE));
        }
        let home_hidden_within = hidden.ok_or(format!("no package's {} gives home_hidden_within", Self::FILE))?;
        if !(home_hidden_within >= 0.0 && home_hidden_within.is_finite()) {
            return Err(format!("{}: home_hidden_within {home_hidden_within} is not 0 or more", Self::FILE));
        }
        fn given<T>(value: Option<T>, name: &str) -> Result<T, String> {
            value.ok_or(format!("no package's {} gives {name}", InterfaceStyle::FILE))
        }
        let home_shown_beyond: f32 = given(beyond, "home_shown_beyond")?;
        if !(home_shown_beyond >= home_hidden_within && home_shown_beyond.is_finite()) {
            return Err(format!("{}: home_shown_beyond {home_shown_beyond} is not home_hidden_within ({home_hidden_within}) or more", Self::FILE));
        }
        let (home_dusk, home_dawn): (u64, u64) = (given(dusk, "home_dusk")?, given(dawn, "home_dawn")?);
        let day = voxmar_protocol::TICKS_PER_DAY;
        if home_dusk >= day || home_dawn >= day || home_dusk == home_dawn {
            return Err(format!("{}: home_dusk {home_dusk} and home_dawn {home_dawn} are not two times of day under {day}", Self::FILE));
        }
        let home_fade: f32 = given(fade, "home_fade")?;
        if !(0.0..=5.0).contains(&home_fade) {
            return Err(format!("{}: home_fade {home_fade} is not from 0 to 5 seconds", Self::FILE));
        }
        let book_zooms: Vec<f32> = given(zooms, "book_zooms")?;
        let sharp = |icon: &f32| *icon > 0.0 && (icon / 8.0).fract() == 0.0;
        if book_zooms.is_empty() || !book_zooms.iter().all(sharp) || !book_zooms.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(format!("{}: book_zooms {book_zooms:?} are not sizes from small to big, each a multiple of 8", Self::FILE));
        }
        let book_zoom_at: usize = given(zoom_at, "book_zoom_at")?;
        if book_zoom_at >= book_zooms.len() {
            return Err(format!("{}: book_zoom_at {book_zoom_at} is not one of the {} book_zooms", Self::FILE, book_zooms.len()));
        }
        let book_width: f32 = given(width, "book_width")?;
        if !(50.0..=100.0).contains(&book_width) {
            return Err(format!("{}: book_width {book_width} is not from 50 to 100 percent", Self::FILE));
        }
        Ok(InterfaceStyle { worn_glyph_alpha, home_hidden_within, home_shown_beyond, home_dusk, home_dawn, home_fade, home_icon: icon, book_zooms, book_zoom_at, book_width })
    }
}

/// Each worn well's mark shows, faintly, while nothing is worn there.
fn show_worn_glyphs(shown: Res<ShownInventory>, style: Res<InterfaceStyle>, mut glyphs: Query<(&WornGlyph, &mut Visibility, &mut ImageNode)>) {
    let faint = Color::WHITE.with_alpha(style.worn_glyph_alpha);
    for (glyph, mut visibility, mut image) in &mut glyphs {
        visibility.set_if_neq(if shown.0.get(glyph.0).is_some() { Visibility::Hidden } else { Visibility::Inherited });
        if image.color != faint {
            image.color = faint;
        }
    }
}

/// A slot a screenshot shows the tooltip of, as if the pointer were over it (`--hover-slot`).
#[derive(Resource)]
pub struct HoverSlot(pub usize);

/// The panel (and creative mode's catalogue beside it); clicks outside them throw the held
/// stack.
#[derive(Component)]
pub(crate) struct InventoryCard;
/// The held stack, drawn at the pointer.
#[derive(Component)]
struct PointerStack;
#[derive(Component)]
struct Tooltip;
/// How wide the tooltip grows.
const TIP_WIDTH: f32 = 180.0 * PX;

/// One tooltip line's box: as wide as the tooltip's inside at most, so a line that wraps is
/// measured, and the tooltip's backing grows, by every line it wraps to.
fn tip_line() -> Node {
    Node { max_width: Val::Px(TIP_WIDTH - 2.0 * space::GAP), ..default() }
}
#[derive(Component)]
pub(crate) struct TooltipLines;
/// What the crafting grid makes: click to craft one onto the pointer, shift-click to craft
/// as many as fit into the inventory.
#[derive(Component)]
pub(crate) struct OutputSlot;
#[derive(Component)]
struct OutputIcon;
/// The words beside the crafting grid's result: its name, then how to take it.
#[derive(Component)]
struct CraftName;
#[derive(Component)]
struct CraftHint;
/// Room for those words, wrapped.
const CRAFT_WORDS: f32 = 120.0 * PX;
#[derive(Component)]
struct OutputCount;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let columns = HOTBAR_SLOTS as f32;
    let grid_width = columns * SLOT + (columns - 1.0) * PX;
    commands.spawn((InventoryScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        // Room for the worn wells, two across, beside the crafting square.
        let worn_width = 2.0 * SLOT + PX + space::SECTION;
        let mut card = panel(grid_width + 76.0 * PX + worn_width);
        card.0.align_items = AlignItems::Center;
        root.spawn((card, InventoryCard, RelativeCursorPosition::default())).with_children(|card| {
            card.spawn(Node { width: Val::Percent(100.0), flex_direction: FlexDirection::Column, ..default() })
                .with_children(|heading| spawn_heading(heading, font, "screen.inventory"));
            let grid = |top: f32| Node {
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::px(HOTBAR_SLOTS as u16, SLOT),
                row_gap: Val::Px(PX),
                column_gap: Val::Px(PX),
                margin: UiRect::top(Val::Px(top)),
                ..default()
            };
            let spawn_placed = |grid: &mut ChildSpawnerCommands, slot: usize, grid_row: GridPlacement, grid_column: GridPlacement| {
                let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), grid_row, grid_column, ..default() };
                grid.spawn((Button, Silent, InventorySlot(slot), RelativeCursorPosition::default(), node, Piece::Well.image())).with_children(|cell| {
                    // A worn well shows faintly what it takes, under the piece worn in it.
                    if let Some(glyph) = worn_glyph(slot) {
                        let inset = (SLOT - 16.0 * PX) / 2.0;
                        let at = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                        cell.spawn((at, glyph.image(), WornGlyph(slot)));
                    }
                    slot_contents(cell, font, slot)
                });
            };
            let spawn_slot = |grid: &mut ChildSpawnerCommands, slot: usize| spawn_placed(grid, slot, GridPlacement::auto(), GridPlacement::auto());
            // An open store's slots and its name, in place of the crafting grid (`fit_store`).
            let store = Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(space::HAIR),
                margin: UiRect::bottom(Val::Px(space::GAP)),
                display: Display::None,
                ..default()
            };
            card.spawn((store, StoreSection)).with_children(|section| {
                section.spawn((shadowed(font, "", BODY, TEXT), StoreName));
                section.spawn(grid(0.0)).with_children(|grid| (STORE..STORE + STORE_SLOTS).for_each(|slot| spawn_slot(grid, slot)));
            });
            // The crafting grid and what it makes, above the pack.
            let crafting = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), margin: UiRect::bottom(Val::Px(space::GAP)), ..default() };
            card.spawn(crafting).with_children(|row| {
                // What is worn: head and body, legs and hands, feet and shield in pairs, and a
                // charm beside them at the middle row, so the wells stand no taller than a
                // crafting table's grid and the screen keeps its height.
                let worn = Node {
                    display: Display::Grid,
                    grid_template_columns: RepeatedGridTrack::px(3, SLOT),
                    row_gap: Val::Px(PX),
                    column_gap: Val::Px(PX),
                    margin: UiRect::right(Val::Px(space::SECTION)),
                    ..default()
                };
                row.spawn(worn).with_children(|grid| {
                    for slot in WORN..=LAST {
                        let at = slot - WORN;
                        let (row, column) = if slot == LAST { (2, 3) } else { (at as i16 / 2 + 1, at as i16 % 2 + 1) };
                        spawn_placed(grid, slot, GridPlacement::start(row), GridPlacement::start(column));
                    }
                });
                // The grid, its arrow, what it makes and its words: an open store's slots
                // take their place (`fit_store`), while what is worn stays.
                let craft = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() };
                row.spawn((craft, CraftRow)).with_children(|craft| {
                    let square = Node {
                        display: Display::Grid,
                        grid_template_columns: RepeatedGridTrack::px(2, SLOT),
                        row_gap: Val::Px(PX),
                        column_gap: Val::Px(PX),
                        ..default()
                    };
                    // Two by two, or three by three at a crafting table (`fit_grid`).
                    craft.spawn((square, CraftSquare)).with_children(|grid| {
                        for cell in 0..GRID {
                            let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
                            grid.spawn((Button, Silent, InventorySlot(CRAFT + cell), CraftCell(cell), RelativeCursorPosition::default(), node, Piece::Well.image()))
                                .with_children(|slot| slot_contents(slot, font, CRAFT + cell));
                        }
                    });
                    craft.spawn(shadowed(font, "→", LARGE, GOLD));
                    let node = Node { width: Val::Px(SLOT + 4.0 * PX), height: Val::Px(SLOT + 4.0 * PX), ..default() };
                    craft.spawn((Button, Silent, OutputSlot, RelativeCursorPosition::default(), node, Piece::Frame.image())).with_children(|cell| {
                        let inset = (SLOT + 4.0 * PX - 16.0 * PX) / 2.0;
                        let icon = Node { position_type: PositionType::Absolute, left: Val::Px(inset), top: Val::Px(inset), width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() };
                        cell.spawn((icon, ImageNode::default(), Visibility::Hidden, OutputIcon));
                        let corner = Node { position_type: PositionType::Absolute, right: Val::Px(super::COUNT_INSET + 2.0 * PX), bottom: Val::Px(super::COUNT_INSET + 2.0 * PX), ..default() };
                        super::spawn_count(cell, font, corner, OutputCount);
                    });
                    // What the grid makes and how to take it, or how to find a recipe.
                    let words = Node { width: Val::Px(CRAFT_WORDS), flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), ..default() };
                    craft.spawn(words).with_children(|words| {
                        words.spawn((shadowed(font, "", BODY, TEXT), CraftName));
                        words.spawn((text(font, "", BODY, MUTED), CraftHint));
                    });
                });
            });
            card.spawn(grid(0.0)).with_children(|grid| (HOTBAR_SLOTS..SLOTS).for_each(|slot| spawn_slot(grid, slot)));
            card.spawn(grid(5.0 * PX)).with_children(|grid| (0..HOTBAR_SLOTS).for_each(|slot| spawn_slot(grid, slot)));
            // What the pack can make, ready first.
            super::makeable::spawn(card, font, HOTBAR_SLOTS);
            spawn_legend(card, font);
        });
        super::screens::mark_screen(root, super::screens::GameScreen::Inventory);
        // The held stack belongs to the inventory; the tooltip also serves recipe and
        // quest icons, so it must live outside this screen's visibility hierarchy.
        let pointer = Node { position_type: PositionType::Absolute, width: Val::Px(SLOT), height: Val::Px(SLOT), ..default() };
        root.spawn((PointerStack, pointer, GlobalZIndex(3))).with_children(|cell| slot_contents(cell, font, CURSOR));
    });
    let tip = Node {
        position_type: PositionType::Absolute,
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(PX),
        max_width: Val::Px(TIP_WIDTH),
        padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
        ..default()
    };
    commands.spawn((Tooltip, TooltipLines, tip, Piece::Tooltip.image(), Visibility::Hidden, GlobalZIndex(10)));
}

/// A gesture's effect on the inventory.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Move {
    Items { from: usize, to: usize, count: u8 },
    Quick(usize),
    Stow,
    /// Throw items out of a slot (or off the pointer) into the world.
    Throw { slot: usize, count: u8 },
}

/// Predicts a move and proposes it to the server, which checks it by the same rules.
fn send(net: &Net, actions: &Actions, inventory: &mut Inventory, action: Move) {
    let name = match action {
        Move::Items { .. } => "core:transfer",
        Move::Quick(_) => "core:quick_move",
        Move::Stow => "core:stow",
        Move::Throw { .. } => "core:drop",
    };
    // Nothing is predicted for an action the content does not have.
    let Some(id) = actions.named(name) else { return };
    let (moved, target) = match action {
        Move::Items { from, to, count } => (inventory.move_items(from, to, count), ActionTarget::Slots { from: from as u8, to: to as u8, count }),
        Move::Quick(slot) => (inventory.quick_move(slot), ActionTarget::Slot(slot as u8)),
        // Closing the screen closes a store too, whether or not anything was held.
        Move::Stow => (inventory.stow_cursor() | inventory.close_store(), ActionTarget::None),
        Move::Throw { slot, count } => (inventory.take(slot, count).is_some(), ActionTarget::Stack { slot: slot as u8, count }),
    };
    if moved {
        net.propose(id, target);
    }
}

/// Shows what the crafting grid makes, and crafts it when clicked.
#[allow(clippy::too_many_arguments)]
fn craft(
    ui: Res<UiState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    launch: Res<crate::session::Launch>,
    images: Res<super::HudImages>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    near: Res<NearTags>,
    output: Query<&RelativeCursorPosition, With<OutputSlot>>,
    mut icon: Query<(&mut ImageNode, &mut Visibility), With<OutputIcon>>,
    mut count: Query<&mut Text, (With<OutputCount>, Without<CraftName>, Without<CraftHint>)>,
    mut name: Query<&mut Text, (With<CraftName>, Without<OutputCount>, Without<CraftHint>)>,
    mut hint: Query<&mut Text, (With<CraftHint>, Without<OutputCount>, Without<CraftName>)>,
    locale: Res<Locale>,
) {
    if !ui.inventory_open {
        return;
    }
    let (recipes, skills) = (launch.content.recipes.clone(), launch.content.skills.clone());
    // As the server allows it: bigger than two by two only at a table, and with the
    // recipe's skill level.
    let at_table = player.station.is_some();
    let (effects, blocks) = (&launch.content.effects, &launch.content.blocks);
    let short = |r: &voxmar_engine::craft::Recipe| {
        r.skill.filter(|&s| {
            let boosted = voxmar_engine::level::check_level(&skills, effects, &player.skills, &player.vitals.effects, s);
            !voxmar_engine::level::meets_recipe(r, skills.level(player.skills.xp(s)), boosted, &player.inventory, blocks)
        })
    };
    // And a later age's only once its rite is done.
    let closed = |r: &voxmar_engine::craft::Recipe| super::makeable::closed(&launch.content, &player, r);
    let allowed = |r: &voxmar_engine::craft::Recipe| closed(r).is_none() && (at_table || !r.needs_table()) && short(r).is_none();
    let near = |tag: &str| near.has(tag);
    // A worn tool and what mends it, by the block it is mended at, mend it, as the server
    // does before any recipe.
    let tools = launch.content.tools.clone();
    let mending = tools.mended(&player.inventory.craft, near);
    // At the level its recipe asks, as the server checks it.
    let level = |skill| (skills.level(player.skills.xp(skill)), voxmar_engine::level::check_level(&skills, effects, &player.skills, &player.vitals.effects, skill));
    let mend_short = mending.and_then(|m| voxmar_engine::level::lacks_for_mend(&tools, m.item, level, &player.inventory, blocks));
    let mended = mending.filter(|_| mend_short.is_none());
    let found = recipes.find_near(&player.inventory.grid(), near).filter(|_| mending.is_none());
    let locked = found.and_then(closed);
    let lacking = mend_short.or_else(|| found.and_then(|r| short(r).map(|skill| (skill, r.level))));
    let made = mended.map(|m| (m.item, 1)).or_else(|| found.filter(|r| allowed(r)).map(|r| (r.output, r.count)));
    let filled = player.inventory.grid().iter().any(Option::is_some);
    // How to look a recipe up is in the key row under the screen, where no item's name
    // reaches it; here only what to do with what the grid makes.
    let (title, how) = match made {
        _ if let Some(age) = &locked => {
            let age = locale.get(&format!("quest.age.{age}")).to_owned();
            (locale.format("inventory.craft.age", &[("age", &age)]), String::new())
        }
        Some((item, _)) if let Some(mended) = mended => {
            let uses = tools.uses(item);
            let after = locale.format("inventory.craft.mend", &[("left", &uses.saturating_sub(mended.wear)), ("uses", &uses)]);
            (locale.block(launch.content.blocks.def(item)), after)
        }
        _ if let Some((skill, level)) = lacking => {
            let skill = locale.skill(skills.get(skill));
            (locale.format("skills.needs", &[("skill", &skill), ("level", &level)]), String::new())
        }
        Some((item, _)) => (locale.block(launch.content.blocks.def(item)), locale.get("inventory.craft.take").to_owned()),
        None if filled => (locale.get("inventory.craft.nothing").to_owned(), String::new()),
        None => (locale.get("inventory.craft").to_owned(), String::new()),
    };
    for (query, words) in [(name.single_mut().ok(), title), (hint.single_mut().ok(), how)] {
        if let Some(mut text) = query
            && text.0 != words
        {
            text.0 = words;
        }
    }
    if let Ok((mut image, mut visibility)) = icon.single_mut() {
        match made {
            Some((item, _)) => {
                let handle = &images.icons[item.0 as usize];
                if image.image != *handle {
                    image.image = handle.clone();
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    if let Ok(mut text) = count.single_mut() {
        let value = made.filter(|(_, n)| *n > 1).map_or(String::new(), |(_, n)| n.to_string());
        if text.0 != value {
            text.0 = value;
        }
    }
    let clicked = mouse.just_pressed(MouseButton::Left) && output.single().is_ok_and(|cursor| cursor.cursor_over());
    if !clicked || made.is_none() {
        return;
    }
    let all = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let Some(craft) = actions.named("core:craft") else { return };
    let mut inventory = player.inventory.clone();
    // What it is made of says how it sounds: flint knapped, clay shaped, wood whittled.
    let with = player.inventory.grid().into_iter().flatten().collect();
    let done = match mended {
        Some(mended) => inventory.mend(mended, all),
        None => craft_grid(&mut inventory, &recipes, allowed, near, all) > 0,
    };
    if done {
        player.inventory = inventory;
        // Heard as crafting (`sounds.ron`), not as a click.
        net.propose_with(craft, ActionTarget::Slot(if all { 0 } else { 1 }), with);
    }
}

/// Crafts what `inventory`'s grid makes (all it can if `all`), as the server will: only a
/// recipe `allowed`, one made near a block only `near` it, and crafting all only while the
/// grid still makes the recipe it made first. Returns how many were made.
fn craft_grid(inventory: &mut Inventory, recipes: &voxmar_engine::craft::RecipeRegistry, allowed: impl Fn(&voxmar_engine::craft::Recipe) -> bool, near: impl Fn(&str) -> bool + Copy, all: bool) -> u32 {
    let first = recipes.find_near(&inventory.grid(), near).filter(|r| allowed(r));
    inventory.craft(recipes.while_making(first, near), all)
}

/// The crafting grid's square, and each of its cells by number (`craft::GRID`).
#[derive(Component)]
struct CraftSquare;
#[derive(Component)]
pub(crate) struct CraftCell(pub usize);

/// Lays the crafting grid out two by two, or three by three while the player works at a
/// crafting table. What the bigger grid held goes back into the inventory with the rest
/// when the screen closes.
fn fit_grid(player: Res<LocalPlayer>, mut square: Query<&mut Node, With<CraftSquare>>, mut cells: Query<(&CraftCell, &mut Node), Without<CraftSquare>>) {
    let side = if player.station.is_some() { voxmar_engine::craft::SIDE } else { 2 };
    if let Ok(mut node) = square.single_mut() {
        let columns = vec![RepeatedGridTrack::px(side as u16, SLOT)];
        if node.grid_template_columns != columns {
            node.grid_template_columns = columns;
        }
    }
    for (cell, mut node) in &mut cells {
        let shown = cell.0 % voxmar_engine::craft::SIDE < side && cell.0 / voxmar_engine::craft::SIDE < side;
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

/// The crafting grid, its arrow, what it makes and its words: an open store's slots take
/// their place, while what is worn stays beside them.
#[derive(Component)]
struct CraftRow;
#[derive(Component)]
struct StoreSection;
#[derive(Component)]
struct StoreName;

/// Shows an open store's slots, as many as it has, and its name, in place of the crafting
/// grid; or the grid, with no store open.
#[allow(clippy::type_complexity)]
fn fit_store(
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    mut rows: ParamSet<(Query<&mut Node, With<CraftRow>>, Query<&mut Node, With<StoreSection>>, Query<(&InventorySlot, &mut Node)>)>,
    mut name: Query<&mut Text, With<StoreName>>,
) {
    let store = player.inventory.store.as_ref();
    let (craft, section) = if store.is_some() { (Display::None, Display::Flex) } else { (Display::Flex, Display::None) };
    for mut node in &mut rows.p0() {
        show(&mut node, craft);
    }
    for mut node in &mut rows.p1() {
        show(&mut node, section);
    }
    let shown = store.map_or(0, |s| s.slots.len());
    for (slot, mut node) in &mut rows.p2() {
        if slot.0 >= STORE {
            show(&mut node, if slot.0 - STORE < shown { Display::Flex } else { Display::None });
        }
    }
    if let (Some(store), Ok(mut text)) = (store, name.single_mut()) {
        let words = locale.block(launch.content.blocks.def(store.block));
        if text.0 != words {
            text.0 = words;
        }
    }
}

/// Sets a node's display only when it changes, so the screen is not laid out again.
fn show(node: &mut Node, display: Display) {
    if node.display != display {
        node.display = display;
    }
}

/// Shows the screen while it is open (`screens` opens and closes it). Closing puts back
/// what the pointer holds. A store the player asked open opens it, once the server says
/// what it holds; one open on the server that the player did not ask for (the screen
/// closed before the word came) is closed.
fn toggle(
    mut ui: ResMut<UiState>,
    mut player: ResMut<LocalPlayer>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut gesture: ResMut<Gesture>,
    mut root: Query<&mut Visibility, With<InventoryScreen>>,
    mut was_open: Local<bool>,
) {
    if player.dead {
        ui.inventory_open = false;
    }
    if *was_open && !ui.inventory_open {
        *gesture = Gesture::default();
        // Walking away from a crafting table, as closing the screen does.
        player.station = None;
        player.store_asked = false;
        send(&net, &actions, &mut player.inventory, Move::Stow);
    } else if !ui.inventory_open && player.inventory.store.is_some() && !player.dead {
        if std::mem::take(&mut player.store_asked) {
            ui.inventory_open = true;
        } else {
            send(&net, &actions, &mut player.inventory, Move::Stow);
        }
    }
    *was_open = ui.inventory_open;
    if let Ok(mut visibility) = root.single_mut() {
        // The recipe viewer takes the screen while it is open.
        visibility.set_if_neq(if ui.inventory_open && !ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// The slot under the pointer.
fn hovered(slots: &Query<(&InventorySlot, &RelativeCursorPosition)>) -> Option<usize> {
    slots.iter().find(|(_, cursor)| cursor.cursor_over()).map(|(slot, _)| slot.0)
}

/// Whether a drag could drop some of `held` into `slot`.
fn accepts(inventory: &Inventory, slot: usize, held: ItemStack) -> bool {
    inventory.get(slot).is_none_or(|s| s.item == held.item && s.count < voxmar_engine::inventory::max_stack(held.item))
}

/// The moves that finish a drag over `slots`: an even share (left button) or one item
/// (right) into each, as far as each has room.
fn spread(inventory: &Inventory, button: MouseButton, slots: &[usize]) -> Vec<Move> {
    let Some(held) = inventory.cursor else { return Vec::new() };
    let each = if button == MouseButton::Left { held.count / slots.len().max(1) as u8 } else { 1 };
    slots.iter().map(|&to| Move::Items { from: CURSOR, to, count: each.max(1) }).collect()
}

/// Every stack of `item` the pointer can take, fullest last so part stacks go first, as
/// moves onto the pointer.
fn gather(inventory: &Inventory, item: voxmar_engine::block::BlockId) -> Vec<Move> {
    let mut sources: Vec<(u8, usize)> =
        (0..SLOTS).filter_map(|slot| inventory.get(slot).filter(|s| s.item == item).map(|s| (s.count, slot))).collect();
    sources.sort();
    sources.into_iter().map(|(count, from)| Move::Items { from, to: CURSOR, count }).collect()
}

#[allow(clippy::too_many_arguments)]
fn click(
    time: Res<Time>,
    ui: Res<UiState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    mut gesture: ResMut<Gesture>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    card: Query<(&RelativeCursorPosition, &InheritedVisibility), With<InventoryCard>>,
) {
    if !ui.inventory_open {
        return;
    }
    let now = time.elapsed_secs();
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    // Control-click is the right click of one-button mice and trackpads on the Mac.
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let over = hovered(&slots);
    let inventory = &mut player.inventory;

    // A drag in progress picks up every slot the pointer crosses that can take some.
    if let Some((button, dragged)) = &mut gesture.drag {
        let button = *button;
        if let (Some(slot), Some(held)) = (over, inventory.cursor)
            && !dragged.contains(&slot)
            && accepts(inventory, slot, held)
            && dragged.len() < held.count as usize
        {
            dragged.push(slot);
        }
        if mouse.just_released(button) {
            let dragged = std::mem::take(dragged);
            gesture.drag = None;
            let moves = match (dragged.as_slice(), inventory.cursor) {
                // The server may have taken the stack meanwhile.
                ([], _) | (_, None) => Vec::new(),
                // A drag that stayed on one slot is a plain click on it.
                (&[slot], Some(held)) => {
                    let count = if button == MouseButton::Right && accepts(inventory, slot, held) { 1 } else { held.count };
                    vec![Move::Items { from: CURSOR, to: slot, count }]
                }
                (slots, Some(_)) => spread(inventory, button, slots),
            };
            for action in moves {
                send(&net, &actions, inventory, action);
            }
        }
        return;
    }

    let left = mouse.just_pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right) || (left && control);
    let left = left && !control;
    // Outside the panel, a click throws the held stack, a right click one item of it.
    let outside = !card.is_empty() && !card.iter().any(|(card, visible)| visible.get() && card.cursor_over());
    if outside
        && (left || right)
        && let Some(held) = inventory.cursor
    {
        let count = if right { 1 } else { held.count };
        send(&net, &actions, inventory, Move::Throw { slot: CURSOR, count });
        return;
    }
    let Some(slot) = over.filter(|_| left || right) else { return };
    let double = gesture.last_click.filter(|c| left && c.slot == slot && now - c.at < DOUBLE_CLICK);
    let mut record = LastClick { slot, at: now, picked_up: false, sent: None };

    if left && shift {
        // Shift double click: every stack of the item just sent goes too.
        if let Some(item) = double.and_then(|c| c.sent) {
            let section = if slot < HOTBAR_SLOTS { 0..HOTBAR_SLOTS } else { HOTBAR_SLOTS..SLOTS };
            let matching: Vec<usize> = section.filter(|&s| inventory.get(s).is_some_and(|stack| stack.item == item)).collect();
            for other in matching {
                send(&net, &actions, inventory, Move::Quick(other));
            }
        } else {
            record.sent = inventory.get(slot).map(|s| s.item);
            send(&net, &actions, inventory, Move::Quick(slot));
        }
    } else if let Some(held) = inventory.cursor {
        if double.is_some_and(|c| c.picked_up) {
            // The second click of a double click gathers the item onto the pointer.
            for action in gather(inventory, held.item) {
                send(&net, &actions, inventory, action);
            }
        } else {
            let button = if right { MouseButton::Right } else { MouseButton::Left };
            gesture.drag = Some((button, vec![slot]));
        }
    } else if let Some(stack) = inventory.get(slot) {
        let count = if right { stack.count.div_ceil(2) } else { stack.count };
        send(&net, &actions, inventory, Move::Items { from: slot, to: CURSOR, count });
        record.picked_up = left;
    }
    if left {
        gesture.last_click = Some(record);
    }
}

/// 1 to 9 over a slot swap it with that hotbar slot.
fn number_keys(
    ui: Res<UiState>,
    keys: Res<ButtonInput<KeyCode>>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
    gesture: Res<Gesture>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
) {
    const DIGITS: [KeyCode; HOTBAR_SLOTS] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if !ui.inventory_open || gesture.drag.is_some() || player.inventory.cursor.is_some() {
        return;
    }
    // Q throws one item from the slot under the pointer, Ctrl+Q all of it.
    if keys.just_pressed(KeyCode::KeyQ)
        && let Some(slot) = hovered(&slots)
        && let Some(stack) = player.inventory.get(slot)
    {
        let whole = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        send(&net, &actions, &mut player.inventory, Move::Throw { slot, count: if whole { stack.count } else { 1 } });
        return;
    }
    let (Some(slot), Some(key)) = (hovered(&slots), DIGITS.iter().position(|&k| keys.just_pressed(k))) else { return };
    let inventory = &mut player.inventory;
    let action = match (inventory.get(slot), inventory.get(key)) {
        _ if slot == key => return,
        (Some(stack), _) => Move::Items { from: slot, to: key, count: stack.count },
        (None, Some(stack)) => Move::Items { from: key, to: slot, count: stack.count },
        (None, None) => return,
    };
    send(&net, &actions, inventory, action);
}

/// What the screen shows: the inventory, with a drag over several slots already spread.
fn preview(player: Res<LocalPlayer>, gesture: Res<Gesture>, mut shown: ResMut<ShownInventory>) {
    let mut inventory = player.inventory.clone();
    if let Some((button, slots)) = &gesture.drag
        && (slots.len() > 1 || *button == MouseButton::Right)
    {
        for action in spread(&inventory.clone(), *button, slots) {
            if let Move::Items { from, to, count } = action {
                inventory.move_items(from, to, count);
            }
        }
    }
    if shown.0 != inventory {
        shown.0 = inventory;
    }
}

/// Keeps the held stack centred on the pointer, and lights the slot under it.
fn follow_pointer(
    ui: Res<UiState>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut held: Query<&mut Node, With<PointerStack>>,
    mut slots: Query<(&InventorySlot, &RelativeCursorPosition, &mut ImageNode)>,
    gesture: Res<Gesture>,
    scale: Res<UiScale>,
) {
    if !ui.inventory_open {
        return;
    }
    if let (Ok(window), Ok(mut node)) = (window.single(), held.single_mut())
        && let Some(at) = window.cursor_position()
    {
        // The pointer is in the window's points, the interface laid out at its own scale.
        let at = at / scale.0;
        node.left = Val::Px(at.x - SLOT / 2.0);
        node.top = Val::Px(at.y - SLOT / 2.0);
    }
    let dragged = gesture.drag.as_ref().map_or(&[][..], |(_, slots)| slots.as_slice());
    for (slot, cursor, mut image) in &mut slots {
        let piece = if dragged.contains(&slot.0) {
            Piece::WellFocus
        } else if cursor.cursor_over() {
            Piece::WellHover
        } else {
            Piece::Well
        };
        if image.image != piece.handle() {
            image.image = piece.handle();
        }
    }
}

/// A light tap whenever the pointer picks up or puts down items; picking up half a stack
/// (a right click, this frame or the last) is a split.
fn tap(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut sounds: MessageWriter<UiSound>,
    mut held: Local<Option<ItemStack>>,
    mut right: Local<[bool; 2]>,
) {
    let control = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    *right = [mouse.just_pressed(MouseButton::Right) || (mouse.just_pressed(MouseButton::Left) && control), right[0]];
    let now = player.inventory.cursor.filter(|_| ui.inventory_open);
    if now != *held {
        if ui.inventory_open {
            sounds.write(if held.is_none() && right.iter().any(|r| *r) { UiSound::Split } else { UiSound::Item });
        }
        *held = now;
    }
}

/// The colour an item's name is written in, from its rarity.
pub fn rarity_color(rarity: Rarity) -> Color {
    match rarity {
        Rarity::Common => TEXT,
        Rarity::Uncommon => Color::srgb(1.0, 0.89, 0.48),
        Rarity::Rare => Color::srgb(0.45, 0.94, 1.0),
        Rarity::VeryRare => Color::srgb(0.82, 0.58, 1.0),
        Rarity::Legendary => GOLD,
    }
}

/// The lines of an item's tooltip after its name: its rarity, what it is good for (from
/// its tags), and the package it comes from.
fn details(locale: &Locale, def: &BlockDef, ids: bool) -> Vec<(String, Color)> {
    let mut lines = Vec::new();
    let rarity = match def.rarity {
        Rarity::Common => None,
        Rarity::Uncommon => Some("rarity.uncommon"),
        Rarity::Rare => Some("rarity.rare"),
        Rarity::VeryRare => Some("rarity.very_rare"),
        Rarity::Legendary => Some("rarity.legendary"),
    };
    if let Some(rarity) = rarity {
        lines.push((locale.get(rarity).to_owned(), rarity_color(def.rarity)));
    }
    let mut uses: Vec<String> = Vec::new();
    for tag in &def.tags {
        match tag.as_str() {
            "edible" => uses.push(locale.get("item.edible").into()),
            "fuel" => uses.push(locale.get("item.fuel").into()),
            "light" => uses.push(locale.get("item.light").into()),
            _ => {
                if let Some(property) = tag.strip_prefix("alchemy:") {
                    let name = locale.get_or(&format!("alchemy.{property}"), &property.replace('_', " ")).to_owned();
                    uses.push(locale.format("item.alchemy", &[("property", &name)]));
                }
            }
        }
    }
    if !uses.is_empty() {
        lines.push((uses.join(locale.get("language.dot")), SOFT));
    }
    if def.has_tag("stripped") {
        lines.push((locale.get("item.stripped").to_owned(), SOFT));
    }
    let package = def.name.split(':').next().unwrap_or("");
    lines.push((if ids { def.name.clone() } else { package_name(locale, package) }, MUTED));
    lines
}

/// What a package is called, in the player's language (`package.<id>`); a mod that names
/// itself nowhere goes by its id.
pub fn package_name(locale: &Locale, package: &str) -> String {
    locale.get_or(&format!("package.{package}"), package).to_owned()
}

/// Names joined as the language lists them, at most `most` of them and how many more.
fn listed(locale: &Locale, names: Vec<String>, most: usize) -> String {
    let comma = locale.get("language.comma");
    if names.len() <= most {
        return names.join(comma);
    }
    let (shown, rest) = names.split_at(most);
    locale.format("tooltip.more", &[("list", &shown.join(comma)), ("count", &rest.len())])
}

/// Where an item comes from and what it goes into, for its tooltip: "Comes from: crafting,
/// a junglefowl" and "Used in: Oak Planks, Campfire and 4 more".
pub fn origins_and_uses(locale: &Locale, content: &voxmar_engine::content::Content, book: &voxmar_engine::recipe_book::RecipeBook, item: voxmar_engine::block::BlockId) -> Vec<String> {
    use voxmar_engine::recipe_book::Method;
    let mut from: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if !from.contains(&name) {
            from.push(name);
        }
    };
    for entry in book.making(item) {
        match &entry.method {
            Method::Craft { .. } => add(locale.get("tooltip.from.craft").to_owned()),
            Method::Cook { .. } => add(locale.get("tooltip.from.cook").to_owned()),
            Method::Product { creature, .. } | Method::Drop { creature, .. } => add(locale.creature(&content.creatures.get(*creature).def)),
            Method::Process { .. } => add(locale.get("tooltip.from.machine").to_owned()),
            Method::Work { .. } => add(locale.get("tooltip.from.station").to_owned()),
            Method::Harvest { .. } => add(locale.get("tooltip.from.harvest").to_owned()),
            Method::Break { .. } => add(locale.get("tooltip.from.break").to_owned()),
            Method::Catch { .. } => add(locale.get("tooltip.from.fishing").to_owned()),
            Method::Trap { .. } => add(locale.get("tooltip.from.trap").to_owned()),
            Method::Bees { .. } => add(locale.get("tooltip.from.bees").to_owned()),
            Method::Chest { .. } => add(locale.get("tooltip.from.chest").to_owned()),
            Method::Trade { .. } => add(locale.get("tooltip.from.trade").to_owned()),
            Method::Reward { .. } => add(locale.get("tooltip.from.quest").to_owned()),
            Method::Found { .. } => add(locale.get("tooltip.from.found").to_owned()),
        }
    }
    let mut into: Vec<String> = Vec::new();
    for entry in book.using(item) {
        let name = locale.block(content.blocks.def(entry.output));
        if entry.output != item && !into.contains(&name) {
            into.push(name);
        }
    }
    let mut lines = Vec::new();
    if !from.is_empty() {
        lines.push(locale.format("tooltip.comes_from", &[("list", &listed(locale, from, 3))]));
    }
    if !into.is_empty() {
        lines.push(locale.format("tooltip.used_in", &[("list", &listed(locale, into, 3))]));
    }
    lines
}

/// For a seed, what it plants and the level that takes, red while the player is below it.
fn seed_line(
    locale: &Locale,
    content: &voxmar_engine::content::Content,
    player: &LocalPlayer,
    item: voxmar_engine::block::BlockId,
) -> Option<(String, Color)> {
    let crop = content.crops.get(content.crops.planted_by(item)?);
    let skill = locale.skill(content.skills.get(crop.skill));
    let ready = content.skills.level(player.skills.xp(crop.skill)) >= crop.def.level;
    let words = locale.format("item.plants", &[("crop", &locale.block_named(&content.blocks, &crop.def.name)), ("skill", &skill), ("level", &crop.def.level)]);
    Some((words, if ready { SOFT } else { ERROR }))
}

/// A reaping hoe's patch: the ripe crops it takes with the one it cuts (`reap:N`).
pub(crate) fn reap_line(locale: &Locale, content: &voxmar_engine::content::Content, item: voxmar_engine::block::BlockId) -> Option<(String, Color)> {
    let reach = content.tools.tool(Some(item)).reap;
    let side = 2 * reach as u32 + 1;
    (reach > 0).then(|| (locale.format("item.reaps", &[("side", &side)]), SOFT))
}

/// How much faster a tool works than a bare hand, on what it is made for (`speed:X`): none
/// for one no faster (a rod, which mines nothing).
pub(crate) fn tool_line(locale: &Locale, content: &voxmar_engine::content::Content, item: voxmar_engine::block::BlockId) -> Option<(String, Color)> {
    let tool = content.tools.tool(Some(item));
    let speed = format!("{}", (tool.speed * 10.0).round() / 10.0);
    (tool.kind.is_some() && tool.speed > 1.0).then(|| (locale.format("item.tool_speed", &[("speed", &speed)]), SOFT))
}

/// How a weapon fights: its class, style and the level that wields it (red while the
/// player's is short of it), then its max hit at their level and how often it strikes, and
/// its reach if it is not the usual.
/// How many uses a tool or weapon has left of all it lasts, red when nearly worn out, and
/// what mends it where; or how long what burns has left.
fn wear_lines(locale: &Locale, content: &voxmar_engine::content::Content, item: voxmar_engine::block::BlockId, wear: u16) -> Vec<(String, Color)> {
    // A light: how much of its burn it has left set down, and that a settlement tends it.
    let flame = content.flames.burn_life(item);
    if flame > 0 {
        let per_day = voxmar_engine::flame::WEAR_PER_DAY as f32;
        let days = |wear: u16| format!("{:.1}", wear as f32 / per_day);
        let color = if worn_low(content, item, wear) { ERROR } else { SOFT };
        let mut lines = vec![(locale.format("item.flame", &[("left", &days(flame.saturating_sub(wear))), ("days", &days(flame))]), color)];
        if content.flames.tended(item) {
            lines.push((locale.get("item.flame.tended").to_owned(), MUTED));
        }
        return lines;
    }
    let burns = content.tools.burns(item);
    if burns > 0 {
        let color = if worn_low(content, item, wear) { ERROR } else { SOFT };
        return vec![(locale.format("item.burns", &[("left", &burns.saturating_sub(wear))]), color)];
    }
    let uses = content.tools.uses(item);
    if uses == 0 {
        return Vec::new();
    }
    let left = uses.saturating_sub(wear);
    let color = if worn_low(content, item, wear) { ERROR } else { SOFT };
    let mut lines = vec![(locale.format("item.uses", &[("left", &left), ("uses", &uses)]), color)];
    // Named by the tag that mends it (Planks, not one wood's), and by the kind of block
    // it is mended at.
    if let Some(&by) = content.tools.mends(item).first() {
        let by = match content.tools.mend_by_tag(item) {
            Some(tag) => locale.get(&format!("item.mend.tag.{tag}")).to_owned(),
            None => locale.block(content.blocks.def(by)),
        };
        let station = locale.get(&format!("item.mend.near.{}", content.tools.mend_near(item))).to_owned();
        lines.push((locale.format("item.mend", &[("item", &by), ("station", &station)]), MUTED));
    }
    lines
}

/// The wear `item` lasts: a tool's uses, a firebrand's seconds, or a light's burn set down
/// (`flame::WEAR_PER_DAY` a game day); 0 for what never wears.
pub fn life(content: &voxmar_engine::content::Content, item: voxmar_engine::block::BlockId) -> u16 {
    content.tools.life(item).max(content.flames.burn_life(item))
}

/// Whether a tool worn by `wear` (a firebrand burnt by it, a light picked up burning) is
/// down to the share where it warns (`tool::Wear::warn`).
pub fn worn_low(content: &voxmar_engine::content::Content, item: voxmar_engine::block::BlockId, wear: u16) -> bool {
    let uses = life(content, item);
    uses > 0 && wear > 0 && uses.saturating_sub(wear) <= (uses as f32 * content.tools.wear().warn).ceil() as u16
}

fn weapon_lines(locale: &Locale, content: &voxmar_engine::content::Content, player: &LocalPlayer, item: voxmar_engine::block::BlockId) -> Vec<(String, Color)> {
    let Some(weapon) = content.combat.weapon(item) else { return Vec::new() };
    let (Some(class), Some(skill)) = (weapon.class, weapon.skill) else { return Vec::new() };
    let class = content.combat.class(class);
    let level = content.skills.level(player.skills.xp(skill));
    let ready = level >= weapon.level;
    let what = locale.format(
        "item.weapon",
        &[
            ("class", &locale.get_or(&format!("item.class.{}", class.name), &class.name)),
            ("style", &locale.get(&format!("item.style.{}", weapon.style.key()))),
            ("skill", &locale.skill(content.skills.get(skill))),
            ("level", &weapon.level),
        ],
    );
    let hit = locale.format("item.weapon_hit", &[("hit", &weapon.max_hit(level)), ("seconds", &format!("{:.1}", weapon.cooldown))]);
    let mut lines = vec![(what, if ready { SOFT } else { ERROR }), (hit, SOFT)];
    // Its reach, when it is not the usual (a spear's further, a dagger's shorter).
    if (weapon.reach - content.combat.feel.reach).abs() > 0.05 {
        lines.push((locale.format("item.weapon_reach", &[("reach", &format!("{:.1}", weapon.reach))]), SOFT));
    }
    // What it shoots, for a sling or a bow: the first item it takes.
    if let Some(&ammo) = content.combat.ammo(weapon).first() {
        lines.push((locale.format("item.weapon_ammo", &[("ammo", &locale.block(content.blocks.def(ammo)))]), SOFT));
    }
    lines
}

/// What a piece of armour or a shield is: its slot and the Defence level that wears it (red
/// while the player is below it, with why it then gives nothing), its armour and weight,
/// and the styles it is strong against; or that a charm is one.
pub(crate) fn armour_lines(locale: &Locale, content: &voxmar_engine::content::Content, player: &LocalPlayer, item: voxmar_engine::block::BlockId) -> Vec<(String, Color)> {
    let combat = &content.combat;
    // A charm says only where it goes: what it does is its effect's, shown while worn.
    if voxmar_engine::inventory::worn_slot(item) == Some(voxmar_engine::combat::Slot::Charm.index()) {
        return vec![(locale.get("item.charm").to_owned(), SOFT), (locale.get("item.charm_wear").to_owned(), SOFT)];
    }
    let defence = content.skills.ids().find(|&id| content.skills.get(id).name == "core:defence");
    let level = defence.map_or(1, |id| content.skills.level(player.skills.xp(id)));
    // What it is, the Defence it needs (red above the player's), then what it stops.
    let (head, needs, armour, guard, resists) = if let Some(piece) = combat.piece(item) {
        let set = combat.set(piece.set);
        let name = locale.get_or(&format!("armour_set.{}", set.name), &voxmar_engine::lang::title_from_id(&set.name)).to_owned();
        let weight = locale.get_or(&format!("item.weight.{}", piece.weight.key()), piece.weight.key()).to_owned();
        (locale.format("item.armour", &[("set", &name), ("weight", &weight)]), piece.level, piece.armour, None, set.resists.clone())
    } else if let Some(shield) = combat.shield(item) {
        (locale.get("item.armour_shield").to_owned(), shield.level, shield.armour, Some(shield.guard), Vec::new())
    } else {
        return Vec::new();
    };
    let ready = level >= needs;
    let mut lines = vec![(head, SOFT)];
    // What anyone can wear needs no saying.
    if needs > 1 {
        lines.push((locale.format("item.armour_needs", &[("level", &needs)]), if ready { SOFT } else { ERROR }));
    }
    if armour > 0 {
        lines.push((locale.format("item.armour_value", &[("armour", &armour)]), SOFT));
    }
    if let Some(guard) = guard {
        let stop = (guard.stop * 100.0).round() as u32;
        lines.push((locale.format("item.armour_guard", &[("stop", &stop), ("arc", &(guard.arc.round() as u32))]), SOFT));
    }
    if !resists.is_empty() {
        let styles: Vec<String> = resists.iter().map(|style| locale.get(&format!("item.style.{}", style.key())).to_owned()).collect();
        lines.push((locale.format("item.armour_resists", &[("styles", &styles.join(locale.get("language.comma")))]), SOFT));
    }
    // Below its level a shield neither guards nor gives armour (the weapon guards instead).
    if !ready {
        lines.push((locale.get(if guard.is_some() { "item.armour_low_shield" } else { "item.armour_low" }).to_owned(), ERROR));
    }
    lines
}

/// What a pouch carries, a line a kind, and how to empty it.
fn pouch_lines(locale: &Locale, registry: &voxmar_engine::block::BlockRegistry, player: &LocalPlayer, item: voxmar_engine::block::BlockId) -> Vec<(String, Color)> {
    if !registry.def(item).has_tag("pouch") {
        return Vec::new();
    }
    let carried: Vec<(String, Color)> = player
        .inventory
        .pouched
        .iter()
        .filter(|&&(held, _)| voxmar_engine::inventory::pouch_for(held) == Some(item))
        .map(|&(held, count)| (locale.format("pouch.holds", &[("count", &count), ("item", &locale.block(registry.def(held)))]), SOFT))
        .collect();
    let empty = carried.is_empty().then(|| (locale.get("pouch.nothing").to_owned(), MUTED));
    // Empty, it says what fills it: what it holds (`holds:<kind>`), picked up.
    let fills = carried.is_empty().then(|| registry.def(item).tags.iter().find_map(|tag| tag.strip_prefix("holds:")).and_then(|kind| locale.find(&format!("pouch.fills.{kind}"))).map(|words| (words.to_owned(), MUTED))).flatten();
    carried.into_iter().chain(empty).chain(fills).chain([(locale.get("pouch.hint").to_owned(), MUTED)]).collect()
}

/// The mouse and key moves the inventory takes, as key caps with what each does, two to a
/// row under the slots.
const LEGEND: [(&str, &str); 6] = [
    ("inventory.key.right_click", "inventory.hint.half"),
    ("inventory.key.drag", "inventory.hint.spread"),
    ("inventory.key.double_click", "inventory.hint.gather"),
    ("inventory.key.shift_click", "inventory.hint.move"),
    ("inventory.key.numbers", "inventory.hint.swap"),
    ("inventory.key.throw", "inventory.hint.throw"),
];

fn spawn_legend(card: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    let legend = Node {
        display: Display::Grid,
        grid_template_columns: RepeatedGridTrack::auto(2),
        column_gap: Val::Px(space::SECTION),
        row_gap: Val::Px(space::HAIR),
        margin: UiRect::top(Val::Px(space::SECTION)),
        ..default()
    };
    card.spawn(legend).with_children(|legend| {
        for (key, hint) in LEGEND {
            legend.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|entry| {
                let cap = Node { padding: UiRect::all(Val::Px(space::INNER)), min_width: Val::Px(crate::ui::skin::CAP_MIN), justify_content: JustifyContent::Center, ..default() };
                entry.spawn((cap, Piece::Button.image())).with_child((Localized(key), shadowed(font, "", BODY, SOFT), TightLine));
                entry.spawn((Localized(hint), text(font, "", BODY, MUTED)));
            });
        }
    });
}

/// Tells the recipe viewer which item the pointer is over: a slot's, or what the crafting
/// grid makes.
fn hover_item(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    output: Query<&RelativeCursorPosition, With<OutputSlot>>,
    mut over: ResMut<super::recipes::HoveredItem>,
    window: Query<&Window, With<PrimaryWindow>>,
    catalogue: Res<super::creative::Catalogue>,
    near: Res<NearTags>,
    (make, plan): (Query<(&super::makeable::MakeCell, &RelativeCursorPosition)>, Res<super::makeable::Drawn>),
) {
    // Without a pointer (no window: `--gate`, `--capture`) nothing is under it to change.
    if ui.recipes_open || window.single().ok().and_then(Window::cursor_position).is_none() {
        return;
    }
    let made = || output.iter().any(RelativeCursorPosition::cursor_over).then(|| launch.content.recipes.find_near(&player.inventory.grid(), |tag| near.has(tag)).map(|r| r.output)).flatten();
    // A thing on the Can Make list looks up as the item it makes.
    let planned = || make.iter().find(|(_, cursor)| cursor.cursor_over()).and_then(|(cell, _)| plan.0.get(cell.0)).map(|makeable| makeable.output);
    let item = if ui.inventory_open { hovered(&slots).and_then(|slot| player.inventory.get(slot)).map(|stack| stack.item).or_else(made).or_else(planned).or(catalogue.hovered) } else { None };
    if over.0 != item {
        over.0 = item;
    }
}

/// Names the item under the pointer, beside it, while nothing is held.
#[allow(clippy::too_many_arguments)]
fn tooltip(
    mut commands: Commands,
    ui: Res<UiState>,
    font: Res<UiFont>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    settings: Res<Settings>,
    gesture: Res<Gesture>,
    window: Query<&Window, With<PrimaryWindow>>,
    slots: Query<(&InventorySlot, &RelativeCursorPosition)>,
    mut tip: Query<(Entity, &mut Node, &mut Visibility, &ComputedNode), With<Tooltip>>,
    mut shown: Local<Option<(voxmar_engine::block::BlockId, bool, bool, u32, u16)>>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    (keys, scale, catalogue): (Res<ButtonInput<KeyCode>>, Res<UiScale>, Res<super::creative::Catalogue>),
    mut japanese: Local<Option<Locale>>,
    (viewer, forced, places, viewer_items): (
        Res<super::recipes::RecipeViewer>,
        Option<Res<HoverSlot>>,
        Query<(&InventorySlot, &ComputedNode, &bevy::ui::UiGlobalTransform)>,
        Query<(&super::recipes::ViewerItem, &RelativeCursorPosition, &InheritedVisibility)>,
    ),
) {
    let Ok((entity, mut node, mut visibility, computed)) = tip.single_mut() else { return };
    let browsing = ui.recipes_open || ui.quests_open;
    let pointed = hovered(&slots).or(forced.as_ref().map(|f| f.0)).filter(|_| !browsing && ui.inventory_open && player.inventory.cursor.is_none() && gesture.drag.is_none());
    // An empty worn slot says what goes in it.
    let empty_worn = pointed.filter(|slot| (WORN..=LAST).contains(slot) && player.inventory.get(*slot).is_none());
    let viewer_item = browsing.then(|| viewer_items.iter().find(|(_, cursor, visible)| visible.get() && cursor.cursor_over()).map(|(item, ..)| item.0)).flatten();
    let item = viewer_item.or_else(|| pointed
        .and_then(|slot| player.inventory.get(slot))
        .map(|stack| stack.item)
        .or(catalogue.hovered.filter(|_| !browsing && ui.inventory_open && player.inventory.cursor.is_none())));
    let Some(item) = item else {
        *shown = None;
        let Some(slot) = empty_worn else {
            visibility.set_if_neq(Visibility::Hidden);
            return;
        };
        let key = voxmar_engine::combat::Slot::ALL[slot - WORN].key();
        let name = locale.get_or(&format!("item.slot.{key}"), key).to_owned();
        commands.entity(entity).despawn_children().with_children(|lines| {
            lines.spawn((shadowed(&font.0, name, BODY, TEXT), tip_line()));
            lines.spawn((text(&font.0, locale.get("item.slot.empty").to_owned(), BODY, MUTED), tip_line()));
        });
        visibility.set_if_neq(Visibility::Inherited);
        place_tip(&mut node, computed, window.single().ok(), scale.0, forced.as_ref().map(|f| f.0), &places);
        return;
    };
    // Shift shows how the name reads: in Japanese, the kana over the name; in another
    // language, the Japanese name with its kana under it.
    let reading = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    // What a pouch carries changes its lines.
    let carried: u32 = player.inventory.pouched.iter().map(|&(_, n)| n as u32 * 31 + 1).sum();
    // So does how worn the pointed tool is.
    let wear = pointed.filter(|_| viewer_item.is_none()).and_then(|slot| player.inventory.get(slot)).map_or(0, |stack| stack.wear);
    let key = (item, settings.debug_overlay, reading, carried, wear);
    if *shown != Some(key) || locale.is_changed() {
        *shown = Some(key);
        let def = world.registry.def(item);
        let readings: Vec<String> = if !reading {
            Vec::new()
        } else if locale.code() == "ja" {
            locale.block_reading(def).into_iter().collect()
        } else {
            let japanese = japanese.get_or_insert_with(|| Locale::load(&voxmar_server::assets_dir().join("content"), "ja"));
            [Some(japanese.block(def)), japanese.block_reading(def)].into_iter().flatten().collect()
        };
        let in_japanese = locale.code() == "ja";
        commands.entity(entity).despawn_children().with_children(|lines| {
            if in_japanese {
                for line in &readings {
                    lines.spawn((text(&font.0, line, BODY, SOFT), tip_line()));
                }
            }
            lines.spawn((shadowed(&font.0, locale.block(def), BODY, rarity_color(def.rarity)), tip_line()));
            if !in_japanese {
                for line in &readings {
                    lines.spawn((text(&font.0, line, BODY, SOFT), tip_line()));
                }
            }
            // Where it comes from and what it goes into, before the rest.
            let book = viewer.book().map(|book| origins_and_uses(&locale, &launch.content, book, item)).unwrap_or_default();
            let lines_after = seed_line(&locale, &launch.content, &player, item)
                .into_iter()
                .chain(tool_line(&locale, &launch.content, item))
                .chain(wear_lines(&locale, &launch.content, item, wear))
                .chain(weapon_lines(&locale, &launch.content, &player, item))
                .chain(armour_lines(&locale, &launch.content, &player, item))
                .chain(pouch_lines(&locale, &world.registry, &player, item))
                .chain(reap_line(&locale, &launch.content, item))
                .chain(book.into_iter().map(|line| (line, SOFT)))
                .chain(details(&locale, def, settings.debug_overlay));
            for (line, color) in lines_after {
                lines.spawn((text(&font.0, line, BODY, color), tip_line()));
            }
            // How to look it up.
            let keys = [("recipes", settings.recipes_key.name(&locale)), ("uses", settings.uses_key.name(&locale))];
            lines.spawn((text(&font.0, locale.format("tooltip.lookup", &[(keys[0].0, &keys[0].1), (keys[1].0, &keys[1].1)]), BODY, MUTED), tip_line()));
        });
    }
    visibility.set_if_neq(Visibility::Inherited);
    place_tip(&mut node, computed, window.single().ok(), scale.0, forced.as_ref().map(|f| f.0), &places);
}

/// Puts the tooltip up and to the right of the pointer, kept on screen; without a pointer,
/// beside the slot a screenshot names. The pointer is in the window's points, the
/// interface laid out at its own scale.
fn place_tip(node: &mut Node, computed: &ComputedNode, window: Option<&Window>, scale: f32, forced: Option<usize>, places: &Query<(&InventorySlot, &ComputedNode, &bevy::ui::UiGlobalTransform)>) {
    let size = computed.size() * computed.inverse_scale_factor();
    if let Some(window) = window
        && let Some(at) = window.cursor_position()
    {
        let (at, width) = (at / scale, window.width() / scale);
        node.left = Val::Px((at.x + 6.0 * PX).min(width - size.x - 2.0 * PX));
        node.top = Val::Px((at.y - size.y - 2.0 * PX).max(2.0 * PX));
    } else if let Some(slot) = forced
        && let Some((_, cell, place)) = places.iter().find(|(s, ..)| s.0 == slot)
    {
        let at = (place.translation + cell.size() / 2.0) * cell.inverse_scale_factor();
        node.left = Val::Px(at.x + 2.0 * PX);
        node.top = Val::Px((at.y - size.y - cell.size().y * cell.inverse_scale_factor()).max(2.0 * PX));
    }
}

#[cfg(test)]
mod tests {
    use voxmar_engine::block::BlockId;

    use super::*;

    #[test]
    fn the_interface_style_comes_from_data_and_a_later_package_changes_it() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let mut files: Vec<_> = content.files.iter().cloned().collect();
        assert_eq!(InterfaceStyle::load(&files).unwrap().worn_glyph_alpha, 0.25);
        let file = |source: &str| voxmar_engine::content::ContentFile { package: "mod".into(), name: InterfaceStyle::FILE.into(), source: source.into() };
        files.push(file("(worn_glyph_alpha: 0.5)"));
        assert_eq!(InterfaceStyle::load(&files).unwrap().worn_glyph_alpha, 0.5);
        let style = InterfaceStyle::load(&files).unwrap();
        assert_eq!((style.home_hidden_within, style.home_icon.as_deref()), (8.0, Some("base:hearthstone")), "core's distance, base's icon");
        assert_eq!((style.home_shown_beyond, style.home_dusk, style.home_dawn, style.home_fade), (48.0, 16500, 23500, 0.5));
        // Each way home number is checked as it loads.
        assert_eq!((style.book_zooms.len(), style.book_zoom_at, style.book_width), (5, 1, 98.0));
        // As is each of the quest book's: sharp zooms from small to big, opening at one of
        // them, across most of the window.
        for bad in [
            "(home_hidden_within: -1.0)",
            "(home_shown_beyond: 4.0)",
            "(home_dusk: 24000)",
            "(home_dawn: 16500)",
            "(home_fade: -0.5)",
            "(home_fade: 9.0)",
            "(book_zooms: [])",
            "(book_zooms: [16.0, 20.0])",
            "(book_zooms: [24.0, 16.0])",
            "(book_zoom_at: 5)",
            "(book_width: 40.0)",
            "(book_width: 101.0)",
        ] {
            files.push(file(bad));
            assert!(InterfaceStyle::load(&files).is_err(), "{bad}");
            files.pop();
        }
        files.push(file("(worn_glyph_alpha: 2.0)"));
        assert!(InterfaceStyle::load(&files).is_err());
    }

    /// Below its level a shield says it neither guards nor gives armour (a body piece only
    /// the armour), and what anyone can wear says nothing of the level it needs.
    #[test]
    fn a_tool_says_how_fast_it_works() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        for code in ["en", "ja"] {
            let locale = Locale::load(&assets.join("content"), code);
            let line = |name: &str| tool_line(&locale, &content, content.blocks.id(name).unwrap()).map(|(line, _)| line);
            assert_eq!(line("base:copper_pickaxe"), Some(locale.format("item.tool_speed", &[("speed", &"7")])), "{code}");
            assert_eq!(line("base:flint_axe"), Some(locale.format("item.tool_speed", &[("speed", &"2")])), "{code}");
            assert_eq!(line("base:oak_log"), None, "{code}: a log is no tool");
            assert_eq!(line("base:fishing_rod"), None, "{code}: a rod mines nothing, so claims no speed");
        }
    }

    #[test]
    fn armour_below_its_level_says_what_it_does_not_do() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let player = LocalPlayer::default();
        for code in ["en", "ja"] {
            let locale = Locale::load(&assets.join("content"), code);
            let lines = |name: &str| armour_lines(&locale, &content, &player, content.blocks.id(name).unwrap()).into_iter().map(|(line, _)| line).collect::<Vec<_>>();
            let (copper, shirt, wooden) = (lines("base:copper_shield"), lines("base:chainmail_shirt"), lines("base:wooden_shield"));
            assert_eq!(copper.last().map(String::as_str), Some(locale.get("item.armour_low_shield")), "{code}: {copper:?}");
            assert_eq!(shirt.last().map(String::as_str), Some(locale.get("item.armour_low")), "{code}: {shirt:?}");
            let needs_one = locale.format("item.armour_needs", &[("level", &1)]);
            assert!(!wooden.contains(&needs_one) && !wooden.iter().any(|line| line == locale.get("item.armour_low_shield")), "{code}: {wooden:?}");
        }
    }

    /// No item's tooltip, in any language, has a line that is only a package's id: every
    /// word on it is the language's (the package by its name, Voxmar).
    #[test]
    fn tooltips_name_packages_not_their_ids() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let blocks = &content.blocks;
        let packages: std::collections::BTreeSet<&str> = (0..blocks.len()).filter_map(|i| blocks.def(BlockId(i as u16)).name.split(':').next()).collect();
        for code in ["en", "ja"] {
            let locale = Locale::load(&assets.join("content"), code);
            for id in (0..blocks.len()).map(|i| BlockId(i as u16)) {
                let def = blocks.def(id);
                for (line, _) in details(&locale, def, false) {
                    assert!(!packages.contains(line.as_str()), "{code}: {}'s tooltip has a line reading only {line:?}", def.name);
                }
            }
            assert_ne!(package_name(&locale, "base"), "base");
            assert_eq!(package_name(&locale, "some_mod"), "some_mod", "{code}: a mod that names itself nowhere goes by its id");
        }
    }

    fn held(count: u8) -> Inventory {
        let mut inventory = Inventory::default();
        inventory.cursor = Some(ItemStack::new(BlockId(1), count));
        inventory
    }

    /// Crafting all on a grid that, once the first recipe is made, would make another: the
    /// client makes as many as the server does, and nothing else.
    #[test]
    fn crafting_all_here_makes_what_the_server_makes_when_the_grid_turns_into_another_recipe() {
        use std::time::Duration;
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let server = voxmar_server::spawn(voxmar_server::ServerConfig { view_distance: 2, ..Default::default() }, content.clone()).unwrap();
        let (connection, server_side) = voxmar_protocol::local_pair();
        server.connections().send(server_side).unwrap();
        let net = Net::new(connection, Some(server), "crafter".into(), content.fingerprint(), None);
        let mut seen = Inventory::default();
        // Given up after 60 of the server's seconds: its ticks, and before its first its
        // thread's CPU time, as it opens the world. By the wall's clock, a server starting
        // with a share of a core on a loaded machine ran out of it before it had begun.
        let server = net.server().expect("singleplayer");
        let mut wait_for = |done: &dyn Fn(&Inventory) -> bool| -> Inventory {
            let (ticks, mut cpu) = (server.ticks(), server.cpu_time());
            let mut starting = Duration::ZERO;
            // A server that neither ticks nor works would never use up its seconds: ten
            // times them by the wall's clock fail the wait too.
            let began = std::time::Instant::now();
            while !done(&seen) {
                assert!(began.elapsed() < Duration::from_secs(600), "the server neither ticked nor worked its way to that inventory in 600 s of the wall's clock: {seen:?}");
                if server.ticks() == 0 {
                    let now = server.cpu_time();
                    starting += cpu.zip(now).map_or(Duration::ZERO, |(then, now)| now.saturating_sub(then));
                    cpu = now.or(cpu);
                }
                let waited = starting.as_secs_f32() + (server.ticks() - ticks) as f32 / voxmar_protocol::TICKS_PER_SECOND as f32;
                assert!(waited < 60.0, "the server never sent that inventory in 60 of its seconds: {seen:?}");
                match net.try_recv() {
                    Ok(Some(voxmar_protocol::ServerMessage::PlayerState { inventory, .. })) => seen = inventory,
                    Ok(_) => std::thread::sleep(Duration::from_millis(1)),
                    Err(_) => panic!("the server went away"),
                }
            }
            seen.clone()
        };
        let planks = content.blocks.expect("base:oak_planks");
        net.command("/clear");
        net.command("/give oak_planks 6");
        wait_for(&|i| i.get(0) == Some(ItemStack::new(planks, 6)) && i.slots.iter().flatten().count() == 1);
        // Down the left two planks a cell, one a cell beside them: a crafting table; what is
        // left, a plank over a plank, would make sticks.
        let transfer = content.actions.expect("core:transfer");
        for (cell, count) in [(0, 2), (3, 2), (1, 1), (4, 1)] {
            net.propose(transfer, ActionTarget::Slots { from: 0, to: (CRAFT + cell) as u8, count });
        }
        let before = wait_for(&|i| i.grid().iter().flatten().count() == 4 && i.get(0).is_none());
        let mut predicted = before.clone();
        let made = craft_grid(&mut predicted, &content.recipes, |r: &voxmar_engine::craft::Recipe| !r.needs_table(), |_| false, true);
        net.propose(content.actions.expect("core:craft"), ActionTarget::Slot(0));
        let crafted = wait_for(&|i| *i != before);
        assert_eq!(made, 1, "one table");
        assert_eq!(predicted, crafted, "the client's craft differs from the server's");
        let stick = content.blocks.expect("base:stick");
        assert!(!crafted.slots.iter().flatten().any(|s| s.item == stick));
    }

    #[test]
    fn drags_spread_evenly_or_one_by_one() {
        let inventory = held(10);
        let moves = spread(&inventory, MouseButton::Left, &[0, 1, 2]);
        assert_eq!(moves, [0, 1, 2].map(|to| Move::Items { from: CURSOR, to, count: 3 }));
        let moves = spread(&inventory, MouseButton::Right, &[4, 5]);
        assert_eq!(moves, [4, 5].map(|to| Move::Items { from: CURSOR, to, count: 1 }));
        // What is left over stays on the pointer.
        let mut after = inventory.clone();
        for action in spread(&inventory, MouseButton::Left, &[0, 1, 2]) {
            let Move::Items { from, to, count } = action else { unreachable!() };
            after.move_items(from, to, count);
        }
        assert_eq!(after.cursor.map(|s| s.count), Some(1));
    }

    #[test]
    fn double_clicks_gather_part_stacks_first() {
        let mut inventory = held(5);
        inventory.slots[3] = Some(ItemStack::new(BlockId(1), 64));
        inventory.slots[7] = Some(ItemStack::new(BlockId(1), 20));
        inventory.slots[8] = Some(ItemStack::new(BlockId(2), 20));
        let moves = gather(&inventory, BlockId(1));
        assert_eq!(moves, vec![Move::Items { from: 7, to: CURSOR, count: 20 }, Move::Items { from: 3, to: CURSOR, count: 64 }]);
        for action in moves {
            let Move::Items { from, to, count } = action else { unreachable!() };
            inventory.move_items(from, to, count);
        }
        assert_eq!(inventory.cursor.map(|s| s.count), Some(64));
        assert_eq!(inventory.get(3).map(|s| s.count), Some(25));
    }

    /// English spells the button one way, as the key chip does ("Right-click"), in every
    /// package's words.
    #[test]
    fn english_spells_right_click_one_way() {
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for package in std::fs::read_dir(&content).unwrap().flatten() {
            let Ok(words) = std::fs::read_to_string(package.path().join("lang/en.ron")) else { continue };
            for line in words.lines().filter(|line| !line.trim_start().starts_with("//")) {
                assert!(!line.to_lowercase().contains("right click"), "{}: {line}", package.path().display());
            }
        }
    }
}
