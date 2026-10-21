//! The journal (J): everything there is to discover, a page for each kind (animals,
//! monsters, plants, fish, biomes, ores, drops and more). Each entry is an outline with a
//! hint until the player finds it, then its picture and name; studied, it adds where it
//! lives; mastered, everything. The server keeps what the player has found
//! (`voxmar_engine::journal`, docs/journal.md); this draws it.

use bevy::prelude::*;
use voxmar_engine::block::BlockId;
use voxmar_engine::creature::Kind;
use voxmar_engine::journal::{Page, Stage};
use voxmar_engine::lang::{def_key, title_from_id};

use super::HudImages;
use super::screens::{GameScreen, ScreenSystems, mark_screen};
use super::SLOT;
use crate::lang::{Locale, Localized, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    ButtonStyle, GOLD, MUTED, SCRIM, SOFT, ScrollList, TEXT, UiFont, panel, screen, shadowed, space, spawn_heading, spawn_rule, spawn_scroll_more, text, BODY,
};

pub struct JournalPlugin;

impl Plugin for JournalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<JournalView>()
            .init_resource::<JournalDrawn>()
            .add_systems(PostStartup, (spawn, open_start_page))
            .add_systems(Update, (show, pick, draw).chain().after(ScreenSystems).run_if(in_state(Screen::InGame)));
    }
}

/// The page open, and the entry picked on it.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct JournalView {
    pub page: usize,
    pub picked: Option<String>,
}

/// The page last drawn, and in which language (for the gate, which reads each page once
/// it is there).
#[derive(Resource, Default)]
pub struct JournalDrawn(pub Option<(usize, String)>);

/// A page to open at startup, by its name (`--journal plants`, for captures), and an
/// entry to pick on it.
#[derive(Resource)]
pub struct StartPage(pub String);

/// `--journal PAGE[/ENTRY]`: the page, and the entry picked on it (`animals/base:deer`).
fn open_start_page(start: Option<Res<StartPage>>, mut view: ResMut<JournalView>, mut ui: ResMut<UiState>) {
    let Some(start) = start else { return };
    let (page, entry) = start.0.split_once('/').map_or((start.0.as_str(), None), |(page, entry)| (page, Some(entry)));
    if let Some(i) = Page::ALL.iter().position(|p| page_name(*p) == page) {
        view.page = i;
        view.picked = entry.map(str::to_owned);
        ui.journal_open = true;
    }
}

/// A page's name, as its words' keys know it.
pub fn page_name(page: Page) -> &'static str {
    match page {
        Page::Animals => "animals",
        Page::Monsters => "monsters",
        Page::Bosses => "bosses",
        Page::Breeds => "breeds",
        Page::Plants => "plants",
        Page::Fish => "fish",
        Page::Biomes => "biomes",
        Page::Places => "places",
        Page::Sky => "sky",
        Page::Ores => "ores",
        Page::Collection => "collection",
        Page::Music => "music",
        Page::Books => "books",
        Page::Discoveries => "discoveries",
    }
}

/// One thing to find: its content name, its item (for its picture), what it is called,
/// and a hint at where to look.
pub struct Findable {
    pub name: String,
    pub item: Option<BlockId>,
    pub title: String,
    pub hint: String,
    /// What it shows once studied: where it lives.
    pub studied: Option<String>,
}

/// A biome's name (or a biome tag's), in the player's language.
fn biome_words(locale: &Locale, name: &str) -> String {
    let local = name.strip_prefix("tag:").unwrap_or(name);
    locale.get_or(&def_key("biome", local), &title_from_id(local)).to_owned()
}

/// Where a creature lives: the biome its habitat likes best, if it has one.
pub(crate) fn creature_home(locale: &Locale, def: &voxmar_engine::creature::CreatureDef) -> Option<String> {
    def.habitat.biomes.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|(biome, _)| biome_words(locale, biome))
}

/// Everything there is to find on `page`, in the content's order.
pub fn findables(locale: &Locale, launch: &Launch, biomes: &[String], page: Page) -> Vec<Findable> {
    let content = &launch.content;
    let blocks = &content.blocks;
    let biome_name = |name: &str| biome_words(locale, name);
    // Bosses have their own page (`Page::Bosses`), whatever their kind.
    let creatures = |kinds: &[Kind], bosses: bool| -> Vec<Findable> {
        let registry = &content.creatures;
        (0..registry.len())
            .map(|i| registry.get(voxmar_engine::creature::CreatureId(i as u16)))
            .filter(|creature| kinds.contains(&creature.def.kind) && creature.def.boss == bosses)
            .map(|creature| {
                let def = &creature.def;
                let home = creature_home(locale, def);
                let hint = match &home {
                    Some(home) => locale.format("journal.hint.lives_in", &[("biome", home)]),
                    None => locale.get(&format!("journal.hint.{}", page_name(page))).to_owned(),
                };
                let item = creature.loot.first().map(|drop| drop.item);
                Findable { name: def.name.clone(), item, title: locale.creature(def), hint, studied: home }
            })
            .collect()
    };
    let items = |ids: Vec<BlockId>| -> Vec<Findable> {
        let hint = locale.get(&format!("journal.hint.{}", page_name(page))).to_owned();
        let mut seen: Vec<BlockId> = Vec::new();
        ids.into_iter()
            .filter(|id| {
                let new = !seen.contains(id);
                seen.push(*id);
                new
            })
            .map(|id| {
                let def = blocks.def(id);
                Findable { name: def.name.clone(), item: Some(id), title: locale.block(def), hint: hint.clone(), studied: None }
            })
            .collect()
    };
    let ids = || blocks.ids().filter(|&id| blocks.listed(id));
    match page {
        Page::Animals => creatures(&[Kind::Animal], false),
        Page::Monsters => creatures(&[Kind::Monster, Kind::Spirit], false),
        Page::Bosses => creatures(&[Kind::Animal, Kind::Monster, Kind::Spirit], true),
        Page::Fish => items(content.fishing.iter().map(|catch| catch.item).collect()),
        Page::Plants => {
            let wild = ids().filter(|&id| {
                let def = blocks.def(id);
                !blocks.info(id).solid && ["plant", "flower", "herb", "fungus"].iter().any(|tag| def.has_tag(tag))
            });
            let ripe = content.crops.iter().map(|(_, crop)| crop.ripe());
            items(wild.chain(ripe).collect())
        }
        Page::Ores => items(ids().filter(|&id| { let def = blocks.def(id); def.has_tag("ore") || def.has_tag("gem") || (def.has_tag("stone") && def.has_tag("natural")) }).collect()),
        Page::Collection => {
            let registry = &content.creatures;
            items((0..registry.len()).flat_map(|i| registry.get(voxmar_engine::creature::CreatureId(i as u16)).loot.iter().map(|drop| drop.item)).collect())
        }
        Page::Biomes => {
            let hint = locale.get("journal.hint.biomes").to_owned();
            biomes.iter().map(|name| Findable { name: name.clone(), item: None, title: biome_name(name), hint: hint.clone(), studied: None }).collect()
        }
        // Their finds come later (bosses, breeds, places, sky, music, books, discoveries).
        _ => Vec::new(),
    }
}

#[derive(Component)]
struct JournalScreen;
#[derive(Component)]
struct PageButton(usize);
#[derive(Component)]
struct PageCount(usize);
#[derive(Component)]
struct Entries;
#[derive(Component)]
struct EntryButton(String);
/// What the picked entry is, under the entries.
#[derive(Component)]
struct Detail;

/// The panel's width, and the column of pages down its left.
const WIDTH: f32 = 540.0 * PX;
const PAGES: f32 = 120.0 * PX;
/// The entries' area's height: the same whatever the page, so the panel never jumps.
const HEIGHT: f32 = 200.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((JournalScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            card.spawn(Node { width: Val::Percent(100.0), flex_direction: FlexDirection::Column, ..default() }).with_children(|heading| spawn_heading(heading, font, "screen.journal"));
            card.spawn(Node { column_gap: Val::Px(space::GAP), ..default() }).with_children(|columns| {
                let side = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), width: Val::Px(PAGES), flex_shrink: 0.0, ..default() };
                columns.spawn(side).with_children(|side| {
                    for (i, page) in Page::ALL.into_iter().enumerate() {
                        let node = Node {
                            min_height: Val::Px(14.0 * PX),
                            flex_shrink: 0.0,
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)),
                            ..default()
                        };
                        let style = ButtonStyle::Secondary;
                        side.spawn((Button, style, node, style.pieces()[0].image(), PageButton(i))).with_children(|button| {
                            button.spawn((Localized(page_key(page)), shadowed(font, "", BODY, TEXT), TightLine));
                            button.spawn((text(font, "", BODY, MUTED), TightLine, PageCount(i)));
                        });
                    }
                });
                let right = Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: Val::Px(space::GAP), min_width: Val::ZERO, ..default() };
                columns.spawn(right).with_children(|right| {
                    let list = Node {
                        height: Val::Px(HEIGHT),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(space::HAIR),
                        padding: UiRect::all(Val::Px(space::GAP)),
                        overflow: Overflow::scroll_y(),
                        ..default()
                    };
                    let list = right.spawn((Entries, ScrollList, list, Piece::Well.image())).id();
                    spawn_scroll_more(right, font, list);
                    let detail = Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(space::HAIR), min_height: Val::Px(40.0 * PX), ..default() };
                    right.spawn((Detail, detail));
                });
            });
            spawn_rule(card, space::GAP);
            card.spawn((Localized("journal.hint"), text(font, "", BODY, MUTED)));
        });
        mark_screen(root, GameScreen::Journal);
    });
}

fn page_key(page: Page) -> &'static str {
    match page {
        Page::Animals => "journal.page.animals",
        Page::Monsters => "journal.page.monsters",
        Page::Bosses => "journal.page.bosses",
        Page::Breeds => "journal.page.breeds",
        Page::Plants => "journal.page.plants",
        Page::Fish => "journal.page.fish",
        Page::Biomes => "journal.page.biomes",
        Page::Places => "journal.page.places",
        Page::Sky => "journal.page.sky",
        Page::Ores => "journal.page.ores",
        Page::Collection => "journal.page.collection",
        Page::Music => "journal.page.music",
        Page::Books => "journal.page.books",
        Page::Discoveries => "journal.page.discoveries",
    }
}

fn show(ui: Res<UiState>, mut root: Query<&mut Visibility, With<JournalScreen>>) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.journal_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn pick(pages: Query<(&PageButton, &Interaction), Changed<Interaction>>, entries: Query<(&EntryButton, &Interaction), Changed<Interaction>>, mut view: ResMut<JournalView>) {
    for (page, interaction) in &pages {
        if *interaction == Interaction::Pressed && view.page != page.0 {
            view.page = page.0;
            view.picked = None;
        }
    }
    for (entry, interaction) in &entries {
        if *interaction == Interaction::Pressed {
            view.picked = Some(entry.0.clone());
        }
    }
}

/// The world's biomes by name, from the generator's content.
fn biome_names(generator: &crate::render::lod::WorldGenerator) -> Vec<String> {
    generator.content().biomes.iter().map(|biome| biome.name.clone()).collect()
}

/// Draws the open page, again when the player finds something or picks an entry.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    ui: Res<UiState>,
    view: Res<JournalView>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    images: Res<HudImages>,
    generator: Res<crate::render::lod::WorldGenerator>,
    entries: Query<Entity, With<Entries>>,
    detail: Query<Entity, With<Detail>>,
    mut buttons: Query<(Entity, &PageButton, Has<crate::ui::widgets::Selected>)>,
    mut counts: Query<(&PageCount, &mut Text)>,
    mut drawn: Local<Option<(JournalView, usize)>>,
    mut done: ResMut<JournalDrawn>,
) {
    if !ui.journal_open {
        *drawn = None;
        return;
    }
    let found_total = player.journal.entries().count();
    let wanted = (view.clone(), found_total);
    if drawn.as_ref() == Some(&wanted) && !locale.is_changed() {
        return;
    }
    *drawn = Some(wanted);
    done.0 = Some((view.page, locale.code().to_owned()));
    let biomes = biome_names(&generator);
    // Each page's count: found of how many.
    for (count, mut text) in &mut counts {
        let page = Page::ALL[count.0];
        let all = findables(&locale, &launch, &biomes, page).len();
        let found = player.journal.found(page);
        let words = if all == 0 { String::new() } else { format!("{found}/{all}") };
        if text.0 != words {
            text.0 = words;
        }
    }
    for (entity, button, selected) in &mut buttons {
        match (button.0 == view.page, selected) {
            (true, false) => {
                commands.entity(entity).insert(crate::ui::widgets::Selected);
            }
            (false, true) => {
                commands.entity(entity).remove::<crate::ui::widgets::Selected>();
            }
            _ => {}
        }
    }
    let page = Page::ALL[view.page];
    let things = findables(&locale, &launch, &biomes, page);
    let font = &font.0;
    let icon_of = |item: Option<BlockId>| item.and_then(|item| images.icons.get(item.0 as usize).cloned()).unwrap_or_default();
    if let Ok(list) = entries.single() {
        commands.entity(list).despawn_children().with_children(|list| {
            if things.is_empty() {
                list.spawn((Localized("journal.empty"), text(font, "", BODY, MUTED)));
                return;
            }
            // Pictures in rows, outlines (dark) until found.
            list.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(PX), row_gap: Val::Px(PX), flex_shrink: 0.0, ..default() }).with_children(|grid| {
                for thing in &things {
                    let entry = player.journal.get(page, &thing.name);
                    let picked = view.picked.as_deref() == Some(thing.name.as_str());
                    let node = Node { width: Val::Px(SLOT), height: Val::Px(SLOT), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() };
                    let piece = if picked { Piece::WellFocus } else { Piece::Well };
                    grid.spawn((Button, crate::ui::widgets::HoverLight, EntryButton(thing.name.clone()), node, piece.image())).with_children(|cell| {
                        let tint = if entry.is_some() { Color::WHITE } else { Color::srgba(0.0, 0.0, 0.0, 0.85) };
                        match thing.item {
                            Some(item) => {
                                cell.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() }, ImageNode::new(icon_of(Some(item))).with_color(tint)));
                            }
                            // No picture of its own: its first letter, or a question mark.
                            None => {
                                let mark = if entry.is_some() { thing.title.chars().next().map(String::from).unwrap_or_default() } else { "?".into() };
                                cell.spawn((shadowed(font, mark, BODY, if entry.is_some() { GOLD } else { MUTED }), TightLine));
                            }
                        }
                    });
                }
            });
        });
    }
    // The picked entry: its name and stage once found, else its hint.
    if let Ok(detail) = detail.single() {
        commands.entity(detail).despawn_children().with_children(|detail| {
            let Some(thing) = view.picked.as_deref().and_then(|name| things.iter().find(|t| t.name == name)) else {
                detail.spawn((Localized("journal.pick"), text(font, "", BODY, MUTED)));
                return;
            };
            match player.journal.get(page, &thing.name) {
                Some(entry) => {
                    let stage = match entry.stage {
                        Stage::Seen => "journal.stage.seen",
                        Stage::Studied => "journal.stage.studied",
                        Stage::Mastered => "journal.stage.mastered",
                    };
                    detail.spawn(shadowed(font, thing.title.clone(), BODY, GOLD));
                    let words = if entry.count == 0 {
                        locale.get(stage).to_owned()
                    } else {
                        format!("{}{}{}", locale.get(stage), locale.get("language.dot"), locale.format("journal.count", &[("count", &entry.count)]))
                    };
                    detail.spawn(text(font, words, BODY, SOFT));
                    if entry.stage >= Stage::Studied
                        && let Some(home) = &thing.studied
                    {
                        detail.spawn(text(font, locale.format("journal.hint.lives_in", &[("biome", home)]), BODY, TEXT));
                    }
                }
                None => {
                    detail.spawn(shadowed(font, locale.get("journal.unknown").to_owned(), BODY, MUTED));
                    detail.spawn(text(font, thing.hint.clone(), BODY, SOFT));
                }
            }
        });
    }
}
