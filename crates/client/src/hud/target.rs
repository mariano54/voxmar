//! The target readout, RuneScape-style but small, at the top left of the screen: whatever
//! the crosshair rests on is named there, so the player can always tell what they look at.
//! A block is named within reach; a dropped item (with its count when there are several), a
//! creature or another player, further, by `look.ron`; the nearest wins, and a wall hides
//! what is behind it. Names come from the language files, so a mod's content is named for
//! free, and the words are coloured by rarity.
//!
//! A creature (or the one being fought, for a while after the last blow, when only ground is
//! looked at) also shows its combat level, coloured by how it compares with ours; once struck
//! at, a thin bar of its health underneath. A kept animal says so (and whose, when it is
//! someone else's), as do one still young and one ready to breed, in a quiet line under the name.
//!
//! A station looked at (a drying rack, a compost heap, a kiln's core) says what is on it and
//! about how long it has left, in the same quiet line: the client asks the server about that
//! one block only (`ClientMessage::LookAtStation`), again while it works, and counts down
//! between answers.

use bevy::color::Mix;
use bevy::prelude::*;

use bevy::ecs::system::SystemParam;
use serde::Deserialize;
use voxmar_engine::block::BlockId;
use voxmar_engine::content::ContentFile;
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::physics::raycast;
use voxmar_protocol::{PlayerId, StationLoad};

use crate::breaking::shows_breaking;
use crate::creatures::{Creatures, Keeper, Sighting};
use crate::drops::Drops;
use crate::hud::DebugPlate;
use crate::hud::inventory::rarity_color;
use crate::interact::Target;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::net::{Net, RemotePlayers};
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::space;
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, MUTED, SOFT, TEXT, UiFont, shadowed, text};
use crate::world::ClientWorld;

/// Seconds the creature last struck at stays in the readout after the crosshair leaves it.
const FIGHTING: f32 = 6.0;
/// Level differences this large or more take the full easy or hard colour.
const SPREAD: f32 = 10.0;
/// The level colour of a creature far below us.
const EASY: Color = Color::srgb(0.56, 0.88, 0.46);
/// Seconds between asks about a station still looked at, so the count down keeps true.
const ASK_EVERY: f64 = 5.0;
/// Seconds after a station is used before it is asked about again.
const AFTER_USE: f64 = 0.3;

/// How far each kind of thing is named from, and how big a box a dropped item is aimed at
/// with (`look.ron`, in `core`); a later package changes what it gives. Read with the rest
/// of the content at launch, which a bad value fails.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct LookStyle {
    pub creature: f32,
    pub player: f32,
    pub item: f32,
    pub item_box: f32,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LookFile {
    creature: Option<f32>,
    player: Option<f32>,
    item: Option<f32>,
    item_box: Option<f32>,
}

impl LookStyle {
    pub const FILE: &str = "look.ron";

    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut given = LookFile::default();
        for file in files.iter().filter(|f| f.name == Self::FILE) {
            let next: LookFile = options.from_str(&file.source).map_err(|e| format!("{}/{}: {e}", file.package, Self::FILE))?;
            given = LookFile {
                creature: next.creature.or(given.creature),
                player: next.player.or(given.player),
                item: next.item.or(given.item),
                item_box: next.item_box.or(given.item_box),
            };
        }
        let need = |name: &str, value: Option<f32>| value.ok_or(format!("no package's {} gives {name}", Self::FILE));
        let style = LookStyle { creature: need("creature", given.creature)?, player: need("player", given.player)?, item: need("item", given.item)?, item_box: need("item_box", given.item_box)? };
        for (name, value) in [("creature", style.creature), ("player", style.player), ("item", style.item), ("item_box", style.item_box)] {
            if !(value > 0.0 && value.is_finite()) {
                return Err(format!("{}: {name} must be more than 0 and finite, not {value}", Self::FILE));
            }
        }
        Ok(style)
    }

    /// The furthest anything is named from.
    fn farthest(&self) -> f32 {
        self.creature.max(self.player).max(self.item)
    }
}

pub struct TargetPlugin;

impl Plugin for TargetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Readout>()
            .add_systems(PostStartup, spawn)
            .init_resource::<StationSeen>()
            .add_systems(Update, (ask, show, place).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), hide);
    }
}

/// What the readout shows, if anything: which creature, its name and level as written,
/// and its share of health once struck at.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Readout {
    pub creature: Option<u32>,
    pub name: String,
    /// The name's colour: by rarity for a block or item.
    pub name_colour: Color,
    /// A creature's level, or the count of a stack.
    pub level: String,
    /// The level's colour against our own combat level.
    pub colour: Color,
    pub health: Option<f32>,
    /// Kept, young, ready to breed: each as written, and its colour.
    pub tags: Vec<(String, Color)>,
}

/// Tags shown at most: kept, young, ready.
const TAGS: usize = 3;

/// The server's last word on the station looked at, and the last ask about one.
#[derive(Resource, Default)]
pub struct StationSeen {
    /// Which block, what is on it, its fire's seconds left (a kiln), and when that came.
    told: Option<(IVec3, Option<StationLoad>, Option<u32>, f64)>,
    /// Which block was asked about, as which block, and when.
    asked: Option<(IVec3, BlockId, f64)>,
}

impl StationSeen {
    /// The server's answer about the station at `block` (`ServerMessage::Station`).
    pub fn tell(&mut self, block: IVec3, load: Option<StationLoad>, fire: Option<u32>, now: f64) {
        self.told = Some((block, load, fire, now));
    }
}

/// Whether `block` says something when looked at: a station, or a fuelled structure's core.
fn says_more(launch: &Launch, block: BlockId) -> bool {
    launch.content.stations.of(block).is_some() || launch.content.multiblocks.with_core(block).any(|m| !m.fuels.is_empty())
}

/// Asks the server about the station looked at: once on looking, again when its block
/// changes (loaded, done), soon after it is used (a kiln lit keeps its block), and every
/// few seconds while it works.
fn ask(time: Res<Time>, buttons: Res<ButtonInput<MouseButton>>, net: Option<Res<Net>>, target: Res<Target>, world: Res<ClientWorld>, launch: Res<Launch>, mut seen: ResMut<StationSeen>) {
    let Some(net) = net else { return };
    let Some(at) = target.block_hit.map(|hit| hit.block) else { return };
    let Some(block) = world.block(at).filter(|&b| says_more(&launch, b)) else { return };
    let now = time.elapsed_secs_f64();
    if buttons.just_pressed(MouseButton::Right) {
        // Asked again once the server has had the use: the answer then says what it did.
        seen.asked = Some((at, block, now - ASK_EVERY + AFTER_USE));
        return;
    }
    let fresh = seen.asked.is_none_or(|(pos, was, when)| pos != at || was != block || now - when >= ASK_EVERY);
    if fresh {
        seen.asked = Some((at, block, now));
        net.look_at_station(at);
    }
}

/// Seconds as the plate says them: about so many minutes, or seconds under one.
fn time_left(locale: &Locale, seconds: f64) -> String {
    let seconds = seconds.max(1.0).ceil() as u64;
    if seconds < 60 { locale.format("target.station.seconds", &[("seconds", &seconds)]) } else { locale.format("target.station.minutes", &[("minutes", &seconds.div_ceil(60))]) }
}

/// The quiet line under a station's name: what it holds and makes, and how long it has
/// left; a kiln's fire, or that it is cold. Nothing until the server has said.
/// `ready` when the block itself shows its work done; `name` names an item.
fn station_tags(locale: &Locale, name: impl Fn(BlockId) -> String, at: IVec3, ready: bool, seen: &StationSeen, now: f64) -> Vec<(String, Color)> {
    let Some((_, load, fire, told)) = seen.told.filter(|told| told.0 == at) else { return Vec::new() };
    let gone = now - told;
    let mut tags = Vec::new();
    if let Some(fire) = fire {
        let left = fire as f64 - gone;
        if left > 0.0 {
            tags.push((locale.format("target.station.burning", &[("time", &time_left(locale, left))]), GOLD));
        } else {
            tags.push((locale.get("target.station.cold").to_owned(), MUTED));
        }
    }
    match load {
        Some(load) => {
            tags.push((locale.format("target.station.holds", &[("count", &load.count), ("item", &name(load.item))]), SOFT));
            let left = load.seconds as f64 - gone;
            if ready || left <= 0.0 {
                tags.push((locale.format("target.station.ready", &[("count", &load.makes), ("item", &name(load.output))]), ACCENT));
            } else {
                tags.push((locale.format("target.station.makes", &[("count", &load.makes), ("item", &name(load.output)), ("time", &time_left(locale, left))]), SOFT));
            }
        }
        None if fire.is_none() => tags.push((locale.get("target.station.empty").to_owned(), MUTED)),
        None => {}
    }
    tags
}

#[derive(Component)]
struct ReadoutStrip;
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
    let strip = Node { position_type: PositionType::Absolute, left: Val::Px(space::GAP), top: Val::Px(space::GAP), ..default() };
    let plate = Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Start,
        row_gap: Val::Px(space::HAIR),
        padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
        ..default()
    };
    let line = Node { column_gap: Val::Px(space::INNER), ..default() };
    let bar = Node { align_self: AlignSelf::Stretch, min_width: Val::Px(40.0 * PX), height: Val::Px(4.0 * PX), padding: UiRect::all(Val::Px(PX)), ..default() };
    commands.spawn((strip, ReadoutStrip, GlobalZIndex(1))).with_children(|strip| {
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

/// What the crosshair rests on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Subject {
    Block(BlockId),
    Item(ItemStack),
    Creature(u32),
    Player(PlayerId),
}

/// The nearest of what lies along the look, each with its distance from the eye.
fn nearest(seen: impl IntoIterator<Item = (f32, Subject)>) -> Option<Subject> {
    seen.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, subject)| subject)
}

/// Everything the crosshair can rest on.
#[derive(SystemParam)]
struct Sight<'w> {
    world: Res<'w, ClientWorld>,
    player: Res<'w, LocalPlayer>,
    target: Res<'w, Target>,
    launch: Res<'w, Launch>,
    style: Res<'w, LookStyle>,
    creatures: Res<'w, Creatures>,
    drops: Res<'w, Drops>,
    players: Res<'w, RemotePlayers>,
    figures: Res<'w, crate::figures::Figures>,
}

impl Sight<'_> {
    /// What is nearest along the look: the block within reach, a dropped item, a creature or
    /// a player within their distances, unless a wall (any block, however far) is nearer.
    fn subject(&self) -> Option<Subject> {
        let (eye, look) = (self.player.eye(), self.player.look_direction().normalize_or_zero());
        let style = &self.style;
        let block = self.target.block_hit.and_then(|hit| Some((hit.distance, Subject::Block(self.world.block(hit.block)?))));
        let things = [
            self.creatures.looked_at(&self.launch, eye, look, style.creature).map(|(id, d)| (d, Subject::Creature(id))),
            self.players.looked_at(eye, look, style.player, |id| self.figures.drawn_at(id)).map(|(id, d)| (d, Subject::Player(id))),
            self.drops.looked_at(eye, look, style.item, style.item_box).map(|(stack, d)| (d, Subject::Item(stack))),
        ];
        // Beyond reach no block is targeted, but a wall still hides what is behind it.
        let wall = || {
            raycast(eye, look, style.farthest(), |p| self.world.block(p).is_some_and(|b| shows_breaking(self.world.registry.info(b)))).map_or(f32::MAX, |hit| hit.distance)
        };
        let mut seen: Vec<(f32, Subject)> = block.into_iter().collect();
        if things.iter().flatten().next().is_some() {
            let wall = wall();
            seen.extend(things.into_iter().flatten().filter(|&(d, _)| d < wall));
        }
        nearest(seen)
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn show(
    time: Res<Time>,
    locale: Res<Locale>,
    ui: Res<UiState>,
    sight: Sight,
    seen: Res<StationSeen>,
    mut readout: ResMut<Readout>,
    mut plate: Query<&mut Visibility, (With<ReadoutPlate>, Without<ReadoutBar>)>,
    mut bar: Query<&mut Visibility, (With<ReadoutBar>, Without<ReadoutPlate>)>,
    mut fill: Query<&mut Node, With<ReadoutFill>>,
    mut name: Query<(&mut Text, &mut TextColor), (With<ReadoutName>, Without<ReadoutLevel>, Without<ReadoutTag>)>,
    mut level: Query<(&mut Text, &mut TextColor), (With<ReadoutLevel>, Without<ReadoutName>, Without<ReadoutTag>)>,
    mut tag_row: Query<&mut Node, (With<ReadoutTags>, Without<ReadoutFill>)>,
    mut tag_texts: Query<(&ReadoutTag, &mut Text, &mut TextColor, &mut Visibility), (Without<ReadoutName>, Without<ReadoutLevel>, Without<ReadoutPlate>, Without<ReadoutBar>)>,
    mut dots: Query<(&ReadoutDot, &mut Visibility), (Without<ReadoutTag>, Without<ReadoutPlate>, Without<ReadoutBar>)>,
) {
    let screen_open = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open;
    let (launch, player, creatures, players) = (&sight.launch, &sight.player, &sight.creatures, &sight.players);
    let subject = if screen_open || player.dead {
        None
    } else {
        // Ground or sky looked at while fighting keeps the creature fought in view, with its health.
        match sight.subject() {
            seen @ (None | Some(Subject::Block(_))) => creatures.fighting(time.elapsed_secs(), FIGHTING).map(Subject::Creature).or(seen),
            seen => seen,
        }
    };
    let next = match subject {
        Some(Subject::Creature(id)) => creatures.sighting(id).map(|seen| {
            let def = &launch.content.creatures.get(seen.species).def;
            let ours = player.skills_known.then(|| launch.content.skills.combat_level(&player.skills));
            Readout {
                creature: Some(id),
                name: locale.creature(def),
                name_colour: TEXT,
                level: locale.format("character.level", &[("level", &def.level)]),
                colour: level_colour(def.level, ours),
                health: seen.struck.then(|| seen.health as f32 / def.health.max(1) as f32),
                tags: tags(&locale, players, &seen),
            }
        }),
        Some(Subject::Block(block)) => {
            let def = sight.world.registry.def(block);
            let tags = match sight.target.block_hit {
                Some(hit) if says_more(launch, block) => {
                    let ready = launch.content.stations.of(block).is_some_and(|(_, phase)| phase == voxmar_engine::station::Phase::Ready);
                    let name = |item: BlockId| locale.block(sight.world.registry.def(item));
                    station_tags(&locale, name, hit.block, ready, &seen, time.elapsed_secs_f64())
                }
                _ => Vec::new(),
            };
            Some(Readout { name: locale.block(def), name_colour: rarity_color(def.rarity), tags, ..Readout::default() })
        }
        Some(Subject::Item(stack)) => {
            let def = sight.world.registry.def(stack.item);
            let count = if stack.count > 1 { locale.format("target.count", &[("count", &stack.count)]) } else { String::new() };
            Some(Readout { name: locale.block(def), name_colour: rarity_color(def.rarity), level: count, colour: SOFT, ..Readout::default() })
        }
        Some(Subject::Player(id)) => players.name(id).map(|who| Readout { name: who.to_owned(), name_colour: ACCENT, ..Readout::default() }),
        None => None,
    }
    .unwrap_or_default();
    if *readout == next {
        return;
    }
    if let Ok(mut visibility) = plate.single_mut() {
        visibility.set_if_neq(if next.name.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
    }
    if let Ok(mut visibility) = bar.single_mut() {
        visibility.set_if_neq(if next.health.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    }
    if let Some(share) = next.health
        && let Ok(mut node) = fill.single_mut()
    {
        node.width = Val::Percent(share.clamp(0.0, 1.0) * 100.0);
    }
    if let Ok((mut text, mut color)) = name.single_mut() {
        if text.0 != next.name {
            text.0.clone_from(&next.name);
        }
        color.set_if_neq(TextColor(next.name_colour));
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

/// Keeps the label below the debug overlay while that is open, in the same corner.
fn place(
    debug: Query<(&Visibility, &ComputedNode), With<DebugPlate>>,
    mut strip: Query<&mut Node, With<ReadoutStrip>>,
) {
    let Ok(mut node) = strip.single_mut() else { return };
    let below = debug.single().ok().filter(|(visibility, _)| **visibility != Visibility::Hidden).map_or(0.0, |(_, computed)| computed.size().y * computed.inverse_scale_factor() + space::GAP);
    let top = Val::Px(space::GAP + below);
    if node.top != top {
        node.top = top;
    }
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
    fn a_station_says_what_it_holds_and_counts_down_between_answers() {
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let locale = Locale::load(&content, "en");
        let name = |item: BlockId| if item == BlockId(1) { "Raw Beef".to_owned() } else { "Jerky".to_owned() };
        let at = IVec3::new(4, 60, -2);
        let mut seen = StationSeen::default();
        let words = |seen: &StationSeen, at: IVec3, ready: bool, now: f64| station_tags(&locale, name, at, ready, seen, now).into_iter().map(|(words, _)| words).collect::<Vec<_>>();
        assert!(words(&seen, at, false, 0.0).is_empty(), "nothing until the server says");
        let load = StationLoad { item: BlockId(1), count: 6, output: BlockId(2), makes: 6, seconds: 360 };
        seen.tell(at, Some(load), None, 10.0);
        assert_eq!(words(&seen, at, false, 10.0), ["6 Raw Beef", "6 Jerky in about 6 min"]);
        assert_eq!(words(&seen, at, false, 340.0), ["6 Raw Beef", "6 Jerky in 30 s"], "counted down on the client");
        assert_eq!(words(&seen, at, false, 400.0), ["6 Raw Beef", "6 Jerky ready"]);
        assert_eq!(words(&seen, at, true, 11.0), ["6 Raw Beef", "6 Jerky ready"], "the block says done");
        assert!(words(&seen, at + IVec3::X, false, 10.0).is_empty(), "another block's answer is not this one's");
        seen.tell(at, None, None, 20.0);
        assert_eq!(words(&seen, at, false, 20.0), ["Empty"]);
        seen.tell(at, None, Some(150), 20.0);
        assert_eq!(words(&seen, at, false, 20.0), ["Burning, about 3 min left"]);
        assert_eq!(words(&seen, at, false, 200.0), ["Cold"]);
    }

    #[test]
    fn the_nearest_thing_is_named() {
        let item = ItemStack::new(BlockId(3), 4);
        assert_eq!(nearest([]), None);
        assert_eq!(nearest([(4.0, Subject::Block(BlockId(1)))]), Some(Subject::Block(BlockId(1))));
        // A dropped item before the ground it lies on, a creature beyond reach before nothing.
        assert_eq!(nearest([(3.2, Subject::Block(BlockId(1))), (2.9, Subject::Item(item))]), Some(Subject::Item(item)));
        assert_eq!(nearest([(3.0, Subject::Block(BlockId(1))), (12.5, Subject::Creature(7))]), Some(Subject::Block(BlockId(1))));
        assert_eq!(nearest([(12.5, Subject::Creature(7)), (20.0, Subject::Player(2))]), Some(Subject::Creature(7)));
    }

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.to_owned(), name: LookStyle::FILE.to_owned(), source: source.to_owned() }
    }

    #[test]
    fn the_shipped_distances_load_and_a_mod_changes_only_what_it_gives() {
        let core = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/core").join(LookStyle::FILE)).unwrap();
        let shipped = LookStyle::load(&[file("core", &core)]).unwrap();
        assert!(shipped.creature > 5.0, "creatures are named from further than a block can be mined");
        let modded = LookStyle::load(&[file("core", &core), file("mod", "(item: 3.0)")]).unwrap();
        assert_eq!((modded.item, modded.creature, modded.item_box), (3.0, shipped.creature, shipped.item_box));
        assert!(LookStyle::load(&[]).is_err_and(|e| e.contains("gives creature")), "some package must give each");
        assert!(LookStyle::load(&[file("core", &core), file("mod", "(player: 0.0)")]).is_err_and(|e| e.contains("player")));
        assert!(LookStyle::load(&[file("core", &core), file("mod", "(creature: 1.0, typo: 2.0)")]).is_err());
    }

    #[test]
    fn a_stack_is_counted_in_both_languages() {
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for language in ["en", "ja"] {
            let locale = Locale::load(&content, language);
            assert_eq!(locale.format("target.count", &[("count", &12)]), "×12", "{language}");
        }
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
