//! The target readout, RuneScape-style but small: the creature under the crosshair (or the
//! one being fought, for a while after the last blow) named at the top of the screen with
//! its combat level, coloured by how it compares with ours; once struck at, a thin bar of
//! its health underneath. A kept animal says so (and whose, when it is someone else's), as
//! do one still young and one ready to breed, in a quiet line under the name.

use bevy::color::Mix;
use bevy::prelude::*;

use crate::creatures::{Creatures, Keeper, Sighting};
use crate::net::RemotePlayers;
use crate::interact::Target;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, MUTED, SOFT, TEXT, UiFont, shadowed, text};
use crate::ui::widgets::space;

/// Seconds the creature last struck at stays in the readout after the crosshair leaves it.
const FIGHTING: f32 = 6.0;
/// Level differences this large or more take the full easy or hard colour.
const SPREAD: f32 = 10.0;
/// The level colour of a creature far below us.
const EASY: Color = Color::srgb(0.56, 0.88, 0.46);

pub struct TargetPlugin;

impl Plugin for TargetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Readout>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), hide);
    }
}

/// What the readout shows, if anything: which creature, its name and level as written,
/// and its share of health once struck at.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Readout {
    pub creature: Option<u32>,
    pub name: String,
    pub level: String,
    /// The level's colour against our own combat level.
    pub colour: Color,
    pub health: Option<f32>,
    /// Kept, young, ready to breed: each as written, and its colour.
    pub tags: Vec<(String, Color)>,
}

/// Tags shown at most: kept, young, ready.
const TAGS: usize = 3;

#[derive(Component)]
struct ReadoutPlate;
#[derive(Component)]
struct ReadoutName;
#[derive(Component)]
struct ReadoutLevel;
#[derive(Component)]
struct ReadoutBar;
#[derive(Component)]
struct ReadoutFill;
#[derive(Component)]
struct ReadoutTags;
#[derive(Component)]
struct ReadoutTag(usize);
#[derive(Component)]
struct ReadoutDot(usize);

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let strip = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), top: Val::Px(space::GAP), justify_content: JustifyContent::Center, ..default() };
    let plate = Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: Val::Px(space::HAIR),
        padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
        ..default()
    };
    let line = Node { column_gap: Val::Px(space::INNER), ..default() };
    let bar = Node { align_self: AlignSelf::Stretch, min_width: Val::Px(40.0 * PX), height: Val::Px(4.0 * PX), padding: UiRect::all(Val::Px(PX)), ..default() };
    commands.spawn((strip, GlobalZIndex(1))).with_children(|strip| {
        strip.spawn((plate, Piece::Tooltip.image(), ReadoutPlate, Visibility::Hidden)).with_children(|plate| {
            plate.spawn(line).with_children(|line| {
                line.spawn((shadowed(font, "", BODY, TEXT), ReadoutName));
                line.spawn((shadowed(font, "", BODY, GOLD), ReadoutLevel));
            });
            let tags = Node { column_gap: Val::Px(space::INNER), display: Display::None, ..default() };
            plate.spawn((tags, ReadoutTags)).with_children(|tags| {
                for i in 0..TAGS {
                    // A dot before every tag but the first.
                    if i > 0 {
                        tags.spawn((text(font, "·", BODY, MUTED), ReadoutDot(i)));
                    }
                    tags.spawn((shadowed(font, "", BODY, SOFT), ReadoutTag(i)));
                }
            });
            plate.spawn((bar, Piece::Well.image(), ReadoutBar)).with_child((
                Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                Piece::BarHealth.image(),
                ReadoutFill,
            ));
        });
    });
}

/// A creature's level colour against ours: gold when even, greener the weaker it is and
/// redder the stronger, as RuneScape colours its levels.
pub fn level_colour(theirs: u8, ours: Option<u8>) -> Color {
    let Some(ours) = ours else { return GOLD };
    let difference = theirs as f32 - ours as f32;
    let towards = if difference > 0.0 { ERROR } else { EASY };
    GOLD.mix(&towards, (difference.abs() / SPREAD).min(1.0))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn show(
    time: Res<Time>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    (ui, player): (Res<UiState>, Res<LocalPlayer>),
    target: Res<Target>,
    creatures: Res<Creatures>,
    mut readout: ResMut<Readout>,
    mut plate: Query<&mut Visibility, (With<ReadoutPlate>, Without<ReadoutBar>)>,
    mut bar: Query<&mut Visibility, (With<ReadoutBar>, Without<ReadoutPlate>)>,
    mut fill: Query<&mut Node, With<ReadoutFill>>,
    mut name: Query<&mut Text, (With<ReadoutName>, Without<ReadoutLevel>, Without<ReadoutTag>)>,
    mut level: Query<(&mut Text, &mut TextColor), (With<ReadoutLevel>, Without<ReadoutName>, Without<ReadoutTag>)>,
    mut tag_row: Query<&mut Node, (With<ReadoutTags>, Without<ReadoutFill>)>,
    mut tag_texts: Query<(&ReadoutTag, &mut Text, &mut TextColor, &mut Visibility), (Without<ReadoutName>, Without<ReadoutLevel>, Without<ReadoutPlate>, Without<ReadoutBar>)>,
    mut dots: Query<(&ReadoutDot, &mut Visibility), (Without<ReadoutTag>, Without<ReadoutPlate>, Without<ReadoutBar>)>,
    players: Res<RemotePlayers>,
) {
    let screen_open = ui.inventory_open || ui.character_open || ui.journal_open || ui.menu_open;
    let creature = target.creature.or_else(|| creatures.fighting(time.elapsed_secs(), FIGHTING)).filter(|_| !screen_open && !player.dead);
    let sighting = creature.and_then(|id| Some((id, creatures.sighting(id)?)));
    let next = match sighting {
        Some((id, seen)) => {
            let def = &launch.content.creatures.get(seen.species).def;
            let ours = player.skills_known.then(|| launch.content.skills.combat_level(&player.skills));
            Readout {
                creature: Some(id),
                name: locale.creature(def),
                level: locale.format("character.level", &[("level", &def.level)]),
                colour: level_colour(def.level, ours),
                health: seen.struck.then(|| seen.health as f32 / def.health.max(1) as f32),
                tags: tags(&locale, &players, &seen),
            }
        }
        None => Readout::default(),
    };
    if *readout == next {
        return;
    }
    if let Ok(mut visibility) = plate.single_mut() {
        visibility.set_if_neq(if next.creature.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
    if let Ok(mut visibility) = bar.single_mut() {
        visibility.set_if_neq(if next.health.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
    if let Some(share) = next.health
        && let Ok(mut node) = fill.single_mut()
    {
        node.width = Val::Percent(share.clamp(0.0, 1.0) * 100.0);
    }
    if let Ok(mut text) = name.single_mut()
        && text.0 != next.name
    {
        text.0.clone_from(&next.name);
    }
    if let Ok((mut text, mut color)) = level.single_mut() {
        if text.0 != next.level {
            text.0.clone_from(&next.level);
        }
        color.set_if_neq(TextColor(next.colour));
    }
    if let Ok(mut row) = tag_row.single_mut() {
        let display = if next.tags.is_empty() { Display::None } else { Display::Flex };
        if row.display != display {
            row.display = display;
        }
    }
    for (ReadoutDot(i), mut visibility) in &mut dots {
        visibility.set_if_neq(if *i < next.tags.len() { Visibility::Inherited } else { Visibility::Hidden });
    }
    for (ReadoutTag(i), mut text, mut color, mut visibility) in &mut tag_texts {
        match next.tags.get(*i) {
            Some((words, colour)) => {
                if text.0 != *words {
                    text.0.clone_from(words);
                }
                color.set_if_neq(TextColor(*colour));
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    *readout = next;
}

/// The quiet line under a creature's name: kept (and by whom, when not by us), young, and
/// ready to breed.
fn tags(locale: &Locale, players: &RemotePlayers, seen: &Sighting) -> Vec<(String, Color)> {
    let mut tags = Vec::new();
    match seen.keeper {
        Some(Keeper::Other(owner)) => match players.name(owner) {
            Some(name) => tags.push((locale.format("target.kept_by", &[("owner", &name)]), SOFT)),
            None => tags.push((locale.get("target.kept").to_owned(), SOFT)),
        },
        Some(Keeper::Us | Keeper::Away) => tags.push((locale.get("target.kept").to_owned(), SOFT)),
        None => {}
    }
    if seen.young {
        tags.push((locale.get("target.young").to_owned(), SOFT));
    }
    if seen.ready {
        tags.push((locale.get("target.ready").to_owned(), ACCENT));
    }
    tags
}

fn hide(mut readout: ResMut<Readout>, mut plate: Query<&mut Visibility, With<ReadoutPlate>>) {
    *readout = Readout::default();
    if let Ok(mut visibility) = plate.single_mut() {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_young_and_ready_animals_say_so() {
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let locale = Locale::load(&content, "en");
        let players = RemotePlayers::default();
        let seen = |keeper, young, ready| Sighting { species: voxmar_engine::creature::CreatureId(0), health: 3, struck: false, keeper, young, ready };
        let words = |seen: Sighting| tags(&locale, &players, &seen).into_iter().map(|(words, _)| words).collect::<Vec<_>>();
        assert!(words(seen(None, false, false)).is_empty());
        assert_eq!(words(seen(Some(Keeper::Us), true, true)), ["Kept", "Young", "Ready to breed"]);
        // Someone else's, with them away: still kept, without a name to give.
        assert_eq!(words(seen(Some(Keeper::Other(7)), false, false)), ["Kept"]);
    }

    #[test]
    fn levels_redden_above_ours_and_green_below() {
        assert_eq!(level_colour(12, Some(12)), GOLD);
        assert_eq!(level_colour(40, Some(3)), ERROR);
        assert_eq!(level_colour(1, Some(30)), EASY);
        assert_eq!(level_colour(5, None), GOLD);
        let (between, gold) = (level_colour(8, Some(3)).to_srgba(), GOLD.to_srgba());
        assert!(between.green < gold.green && between.green > ERROR.to_srgba().green, "five above ours is between: {between:?}");
    }
}
