//! The prayer book and the strip over the hotbar. The book is a page of the character screen
//! (its Prayers button opens it): the prayer points as a meter, then every prayer in the
//! content's order with the Magic level it opens at, what it drains and what it does, and
//! its quick key if it has one. A click turns a prayer on or off, as its quick key does
//! (`fight::quick_prayers`); the server decides, and the book shows what it says. The strip
//! shows the points and the prayers on while any is on or the points are not full.
//!
//! The words come from the prayer's data (`voxmar_engine::prayer`): the effects' numbers and
//! the rules' wards are filled into the language's `prayer.effect.*` lines, so a mod's
//! prayer reads right without words of its own beyond its name (`prayer.<package>.<name>`).

use bevy::prelude::*;
use voxmar_engine::lang::Lang;
use voxmar_engine::prayer::{Effect, PrayerDef, PrayerId, PrayerRegistry};

use super::screens::{GameScreen, ScreenSystems, mark_screen};
use crate::fight::Fighting;
use crate::lang::{Locale, Localized};
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, ERROR, GOLD, MUTED, Meter, SCRIM, SOFT, ScrollList, TEXT, UiFont, panel, screen, set_meter, shadowed, space, spawn_button,
    spawn_heading, spawn_meter, spawn_rule, spawn_scroll_more, text,
};

pub struct PrayersPlugin;

impl Plugin for PrayersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PrayerBook>().add_systems(PostStartup, spawn_book).add_systems(
            Update,
            (open_and_close, show_book, pick, draw_rows, draw_points, draw_strip).chain().after(ScreenSystems).run_if(in_state(Screen::InGame)),
        );
        app.add_systems(OnExit(Screen::InGame), |mut book: ResMut<PrayerBook>| book.0 = false);
    }
}

/// Whether the prayer book is the page of the character screen showing.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PrayerBook(pub bool);

#[derive(Component)]
struct BookScreen;
/// The scrolling list of prayers.
#[derive(Component)]
struct BookList;
/// A prayer's row, which turns it on or off when clicked.
#[derive(Component)]
pub struct BookRow(pub PrayerId);
/// The meter of prayer points on the page.
#[derive(Component)]
struct BookMeter;
#[derive(Component)]
struct BookBack;
/// The Prayers button on the character screen.
#[derive(Component)]
pub struct PrayersButton;

/// The strip over the hotbar: the points as a meter, and the prayers on.
#[derive(Component)]
pub struct PrayerStrip;
#[derive(Component)]
struct StripMeter;
#[derive(Component)]
struct StripNames;

const WIDTH: f32 = 540.0 * PX;
/// How tall the list of prayers is before it scrolls: five or six rows.
const LIST_HEIGHT: f32 = 170.0 * PX;
/// The column at a row's right: Magic level, drain and quick key.
const SIDE: f32 = 64.0 * PX;
/// Room for the meter's label.
const LABEL: f32 = 38.0 * PX;

/// The Prayers button, in the character screen's left column.
pub fn spawn_open_button(parent: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    parent.spawn(Node { width: Val::Percent(100.0), ..default() }).with_children(|row| {
        spawn_button(row, font, "character.prayers", ButtonStyle::Secondary, PrayersButton);
    });
}

/// The strip over the hotbar, `width` wide, hidden until a prayer is on or the points are down.
pub fn spawn_strip(parent: &mut ChildSpawnerCommands, font: &Handle<Font>, width: f32) {
    let strip = Node {
        width: Val::Px(width),
        display: Display::None,
        align_items: AlignItems::Center,
        column_gap: Val::Px(space::INNER),
        margin: UiRect::bottom(Val::Px(space::INNER)),
        ..default()
    };
    parent.spawn((PrayerStrip, strip)).with_children(|strip| {
        strip.spawn((Node { width: Val::Px(width * 0.4), flex_shrink: 0.0, ..default() },)).with_children(|cell| {
            spawn_meter(cell, font, Piece::Bar, Val::Percent(100.0), StripMeter);
        });
        strip.spawn((Node { flex_grow: 1.0, min_width: Val::ZERO, ..default() }, shadowed(font, "", BODY, ACCENT), StripNames));
    });
}

fn spawn_book(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((BookScreen, screen(SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            spawn_heading(card, font, "screen.prayers");
            card.spawn(Node { column_gap: Val::Px(space::INNER), align_items: AlignItems::Center, margin: UiRect::bottom(Val::Px(space::GAP)), ..default() }).with_children(|row| {
                row.spawn((Node { width: Val::Px(LABEL), flex_shrink: 0.0, ..default() }, Localized("prayers.points"), text(font, "", BODY, MUTED), TextLayout::no_wrap()));
                spawn_meter(row, font, Piece::Bar, Val::Percent(100.0), BookMeter);
            });
            let list = Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(space::HAIR),
                height: Val::Px(LIST_HEIGHT),
                padding: UiRect::all(Val::Px(space::GAP)),
                overflow: Overflow::scroll_y(),
                ..default()
            };
            let list = card.spawn((BookList, ScrollList, list, Piece::Well.image())).id();
            spawn_scroll_more(card, font, list);
            spawn_rule(card, space::GAP);
            card.spawn(Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: Val::Px(space::GAP), ..default() }).with_children(|foot| {
                foot.spawn((Node { flex_grow: 1.0, flex_basis: Val::ZERO, min_width: Val::ZERO, ..default() }, Localized("prayers.hint"), text(font, "", BODY, MUTED)));
                foot.spawn(Node { width: Val::Px(40.0 * PX), flex_shrink: 0.0, ..default() }).with_children(|cell| {
                    spawn_button(cell, font, "character.back", ButtonStyle::Secondary, BookBack);
                });
            });
        });
        mark_screen(root, GameScreen::Character);
    });
}

/// The button opens the book, Back closes it, and it closes with the character screen.
fn open_and_close(
    ui: Res<UiState>,
    open: Query<&Interaction, (Changed<Interaction>, With<PrayersButton>)>,
    back: Query<&Interaction, (Changed<Interaction>, With<BookBack>)>,
    mut book: ResMut<PrayerBook>,
) {
    if !ui.character_open {
        if book.0 {
            book.0 = false;
        }
        return;
    }
    if open.iter().any(|i| *i == Interaction::Pressed) {
        book.0 = true;
    }
    if back.iter().any(|i| *i == Interaction::Pressed) {
        book.0 = false;
    }
}

fn show_book(ui: Res<UiState>, book: Res<PrayerBook>, mut root: Query<&mut Visibility, With<BookScreen>>) {
    if let Ok(mut visibility) = root.single_mut() {
        visibility.set_if_neq(if ui.character_open && book.0 && !ui.recipes_open { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// The Magic level the player prays at, by their experience.
fn magic_level(launch: &Launch, player: &LocalPlayer) -> u8 {
    launch.content.prayers.skill.map_or(1, |skill| launch.content.skills.level(player.skills.xp(skill)))
}

/// A click on a prayer turns it on, or off if it is on; the server decides.
fn pick(rows: Query<(&BookRow, &Interaction), Changed<Interaction>>, launch: Res<Launch>, player: Res<LocalPlayer>, fighting: Res<Fighting>, net: Res<Net>) {
    let level = magic_level(&launch, &player);
    for (row, interaction) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let on = fighting.prayers.on.contains(&row.0);
        if on || launch.content.prayers.get(row.0).is_some_and(|def| def.level <= level) {
            net.pray(row.0, !on);
        }
    }
}

/// What a prayer does, in the language: each effect's line, with the data's numbers.
pub fn describe(lang: &Lang, book: &PrayerRegistry, def: &PrayerDef) -> String {
    let percent = |share: f32| format!("{}", (share * 100.0).round());
    let rules = &book.rules;
    def.effects
        .iter()
        .map(|effect| match effect {
            Effect::Boost { stat, share } => lang.format(&format!("prayer.effect.boost.{}", stat.key()), &[("percent", &percent(*share))]),
            Effect::Ward { styles } => {
                let names: Vec<String> = styles.iter().map(|style| lang.get(&format!("item.style.{}", style.key())).to_owned()).collect();
                lang.format("prayer.effect.ward", &[("percent", &percent(rules.ward)), ("styles", &names.join(lang.get("language.comma")))])
            }
            Effect::Keep(more) => lang.format("prayer.effect.keep", &[("count", more)]),
            Effect::Mending(times) => lang.format("prayer.effect.mending", &[("times", times)]),
            Effect::LastBreath { below, heal } => lang.format("prayer.effect.last_breath", &[("below", &percent(*below)), ("heal", &percent(*heal))]),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A drain in points a minute, whole where it is.
fn drain_text(drain: f32) -> String {
    if drain.fract().abs() < 0.05 { format!("{}", drain.round()) } else { format!("{drain:.1}") }
}

/// What the book last drew: the Magic level, the prayers on and the quick keys.
#[derive(PartialEq)]
struct Drawn {
    level: u8,
    on: Vec<PrayerId>,
    keys: Vec<String>,
}

/// Fills the list with a row for each prayer while the book shows, and again when the level,
/// what is on, the quick keys or the language change.
#[allow(clippy::too_many_arguments)]
fn draw_rows(
    mut commands: Commands,
    book: Res<PrayerBook>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    player: Res<LocalPlayer>,
    fighting: Res<Fighting>,
    settings: Res<Settings>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    list: Query<(Entity, Ref<BookList>)>,
    mut drawn: Local<Option<Drawn>>,
) {
    let Ok((entity, added)) = list.single() else { return };
    if !(book.0 && ui.character_open) {
        return;
    }
    let prayers = &launch.content.prayers;
    // The quick key each prayer has, if any, as the player names it.
    let keys: Vec<String> = settings
        .prayer_keys
        .iter()
        .zip(&settings.quick_prayers)
        .map(|(key, name)| if prayers.id(name).is_some() { key.name(&locale) } else { String::new() })
        .collect();
    let level = magic_level(&launch, &player);
    let state = Drawn { level, on: fighting.prayers.on.clone(), keys };
    if drawn.as_ref() == Some(&state) && !locale.is_changed() && !added.is_added() {
        return;
    }
    let font = &font.0;
    commands.entity(entity).despawn_children().with_children(|list| {
        if prayers.is_empty() {
            list.spawn((Localized("prayers.empty"), text(font, "", BODY, MUTED)));
        }
        for (id, def) in prayers.all() {
            let reached = level >= def.level;
            let on = state.on.contains(&id);
            let quick = settings.quick_prayers.iter().position(|name| prayers.id(name) == Some(id)).map(|i| state.keys[i].clone()).filter(|key| !key.is_empty());
            let (name_color, words_color) = if on {
                (ACCENT, SOFT)
            } else if reached {
                (TEXT, SOFT)
            } else {
                (MUTED, MUTED)
            };
            let row = Node {
                display: Display::Grid,
                grid_template_columns: vec![GridTrack::flex(1.0), GridTrack::px(SIDE)],
                column_gap: Val::Px(space::GAP),
                align_items: AlignItems::Start,
                padding: UiRect::all(Val::Px(space::HAIR)),
                flex_shrink: 0.0,
                ..default()
            };
            list.spawn((Button, crate::ui::widgets::HoverLight, BookRow(id), row)).with_children(|row| {
                row.spawn(Node { flex_direction: FlexDirection::Column, min_width: Val::ZERO, ..default() }).with_children(|words| {
                    words.spawn(shadowed(font, locale.prayer(&def.name), BODY, name_color));
                    words.spawn(text(font, describe(&locale, prayers, def), BODY, words_color));
                });
                row.spawn(Node { flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, ..default() }).with_children(|side| {
                    let magic = locale.format("prayers.magic_level", &[("level", &def.level)]);
                    side.spawn(text(font, magic, BODY, if reached { GOLD } else { ERROR }));
                    side.spawn(text(font, locale.format("prayers.drain", &[("drain", &drain_text(def.drain))]), BODY, MUTED));
                    if on {
                        side.spawn(shadowed(font, locale.get("prayers.on"), BODY, ACCENT));
                    }
                    if let Some(key) = quick {
                        side.spawn(text(font, key, BODY, GOLD));
                    }
                });
            });
        }
    });
    *drawn = Some(state);
}

/// The points as the server last said them, on the page's meter.
fn draw_points(
    book: Res<PrayerBook>,
    ui: Res<UiState>,
    fighting: Res<Fighting>,
    meter: Query<&Meter, With<BookMeter>>,
    mut nodes: Query<&mut Node>,
    children: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    if !(book.0 && ui.character_open) {
        return;
    }
    let Ok(meter) = meter.single() else { return };
    let prayers = &fighting.prayers;
    set_meter(meter, prayers.points as f32 / prayers.most.max(1) as f32, &format!("{}/{}", prayers.points, prayers.most), &mut nodes, &children, &mut texts);
}

/// The strip over the hotbar: shown while a prayer is on or the points are not full, with the
/// points and the names of the prayers on.
#[allow(clippy::too_many_arguments)]
fn draw_strip(
    fighting: Res<Fighting>,
    locale: Res<Locale>,
    launch: Res<Launch>,
    mut strip: Query<&mut Node, (With<PrayerStrip>, Without<StripMeter>)>,
    meter: Query<&Meter, With<StripMeter>>,
    mut nodes: Query<&mut Node, Without<PrayerStrip>>,
    children: Query<&Children>,
    mut texts: Query<&mut Text, Without<StripNames>>,
    mut names: Query<&mut Text, With<StripNames>>,
) {
    let prayers = &fighting.prayers;
    let wanted = prayers.most > 0 && (!prayers.on.is_empty() || prayers.points < prayers.most);
    if let Ok(mut node) = strip.single_mut() {
        let display = if wanted { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    if !wanted {
        return;
    }
    if let Ok(meter) = meter.single() {
        set_meter(meter, prayers.points as f32 / prayers.most.max(1) as f32, &format!("{}/{}", prayers.points, prayers.most), &mut nodes, &children, &mut texts);
    }
    let words = prayers
        .on
        .iter()
        .filter_map(|&id| launch.content.prayers.get(id))
        .map(|def| locale.prayer(&def.name))
        .collect::<Vec<_>>()
        .join(locale.get("language.comma"));
    if let Ok(mut text) = names.single_mut()
        && text.0 != words
    {
        text.0 = words;
    }
}

/// For the gate: how many prayer rows the book has drawn.
pub fn book_rows(world: &mut World) -> usize {
    world.query_filtered::<(), With<BookRow>>().iter(world).count()
}

/// For the gate: the Prayers button on the character screen, and `prayer`'s row in the book.
pub fn open_button(world: &mut World) -> Option<Entity> {
    world.query_filtered::<Entity, With<PrayersButton>>().iter(world).next()
}

pub fn row_of(world: &mut World, prayer: PrayerId) -> Option<Entity> {
    world.query::<(Entity, &BookRow)>().iter(world).find(|(_, row)| row.0 == prayer).map(|(entity, _)| entity)
}

/// The names the strip shows, if it is showing.
pub fn strip_names(world: &mut World) -> Option<String> {
    let shown = world.query_filtered::<&Node, With<PrayerStrip>>().iter(world).any(|node| node.display != Display::None);
    shown.then(|| world.query_filtered::<&Text, With<StripNames>>().iter(world).map(|t| t.0.clone()).collect::<Vec<_>>().join(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    /// Every prayer of the game reads in both languages, with its numbers from the data and no
    /// word left unfilled.
    #[test]
    fn every_prayer_is_described_in_both_languages_from_its_data() {
        let content = voxmar_server::load_content(&assets()).unwrap();
        let book = &content.prayers;
        for code in ["en", "ja"] {
            let lang = Lang::load(&assets().join("content"), code);
            for (_, def) in book.all() {
                let words = describe(&lang, book, def);
                assert!(!words.is_empty() && !words.contains('{') && !words.contains("prayer.effect"), "{code}: {} reads `{words}`", def.name);
            }
        }
        let english = Lang::load(&assets().join("content"), "en");
        let find = |name: &str| book.get(book.id(name).unwrap()).unwrap();
        assert_eq!(describe(&english, book, find("base:bark_skin")), "Defence +5%.");
        assert_eq!(describe(&english, book, find("base:ward_melee")), "Stops 80% of stab, slash, crush blows.");
        assert_eq!(describe(&english, book, find("base:keepsake")), "Death keeps 1 more item.");
        assert_eq!(describe(&english, book, find("base:mending")), "Health comes back 2 times as fast.");
    }

    #[test]
    fn drains_read_whole_where_they_are() {
        assert_eq!([drain_text(5.0), drain_text(2.0), drain_text(0.5), drain_text(1.5)], ["5", "2", "0.5", "1.5"]);
    }
}
