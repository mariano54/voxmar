//! How a fight shows on screen, from [`crate::fight::Fighting`]: under the crosshair the
//! swing meter, filling over the weapon's cooldown and lighting on the beat, with the
//! rhythm pips beneath it (all lit, the next blow on the beat is a flourish); and
//! hitsplats, the number each blow did rising and fading over the body of the creature it
//! struck (following it as it falls, so the killing blow shows too), or above the hotbar
//! for those that came at the player, coloured by what kind of blow it was or what came of
//! it (a miss, a block, a perfect block, a dodge), ringed by two dark pixels so they read over
//! fur and grass, stacked newest lowest so quick blows never cover each other. A blow that hurts the player also flashes what the health bar
//! lost. All of it follows the design wiki's combat feel (the meter, pips and hitsplats;
//! damage numbers can be switched off, and the guard put on a toggle, in the settings).
//!
//! Sizes, colours and timings are the packages' (`combat_hud.ron`, in `core`), read and
//! checked with the rest of the content at launch; a later package changes what it gives.

use std::collections::VecDeque;

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::combat::{Blow, Defended};
use voxmar_engine::content::ContentFile;

use crate::creatures::Creatures;
use crate::fight::Fighting;
use crate::hud::HealthMeter;
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, font_size};
use crate::ui::widgets::{Meter, TRAIL, UiFont, text};

pub struct CombatHudPlugin;

impl Plugin for CombatHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SplatsShown>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (meter, flash).run_if(in_state(Screen::InGame)))
            .add_systems(PostUpdate, splats.after(TransformSystems::Propagate).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), hide);
    }
}

/// The file a package styles fights' marks in.
pub const FILE: &str = "combat_hud.ron";

/// The splats' words kept for the gate to sample.
const RECENT: usize = 16;

/// An sRGB colour, 0 to 255 a channel.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn color(self) -> Color {
        Color::srgb_u8(self.0, self.1, self.2)
    }
}

/// The swing meter under the crosshair: its size and place in interface pixels, its
/// colours (`frame` the pixel round it and its pips), and the seconds it stays after the
/// last blow either way.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterStyle {
    pub width: f32,
    pub height: f32,
    pub below: f32,
    pub frame: Rgb,
    pub empty: Rgb,
    pub fill: Rgb,
    /// Lit while a blow would be on the beat.
    pub beat: Rgb,
    /// The end of the track a click is held for the beat in.
    pub buffer: Rgb,
    pub linger: f32,
}

/// The rhythm pips under the meter: each one's size and the gap between, and their
/// colours unlit, lit, and all lit (the next blow on the beat a flourish).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipStyle {
    pub size: f32,
    pub gap: f32,
    pub off: Rgb,
    pub on: Rgb,
    pub ready: Rgb,
}

/// One kind of hitsplat: its colour, and its size in screen pixels a font pixel (2 as body
/// text, 3 as headings, 4 by default for a number read at a glance over fur; whole numbers
/// keep the pixel font crisp).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Splat {
    pub color: Rgb,
    #[serde(default = "plain")]
    pub scale: u32,
}

fn plain() -> u32 {
    4
}

/// What a blow that hurts the player does to the health bar: what it lost shows in `color`
/// at first, fading back to the bar's own over `seconds`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashStyle {
    pub color: Rgb,
    pub seconds: f32,
}

/// Hitsplats: the seconds each shows, how far it rises meanwhile and the share of them it
/// stays whole before fading, the room between those stacked on one creature and how many
/// stand at most (the oldest goes early), how high over the bottom of the hotbar (wherever
/// Raise HUD puts it) the player's own stand,
/// each kind's look by its name, and the health bar's flash when a blow hurts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplatStyle {
    pub seconds: f32,
    pub rise: f32,
    pub fade: f32,
    pub gap: f32,
    pub most: usize,
    pub hurt_above: f32,
    pub blows: std::collections::BTreeMap<String, Splat>,
    pub defended: std::collections::BTreeMap<String, Splat>,
    pub flash: FlashStyle,
}

/// What a package gives in `combat_hud.ron`; each part replaces the one before, but for
/// the splats' kinds, which each replace their own.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StyleFile {
    meter: Option<MeterStyle>,
    pips: Option<PipStyle>,
    splats: Option<SplatStyle>,
}

/// How fights show, from every package's `combat_hud.ron`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct CombatStyle {
    pub meter: MeterStyle,
    pub pips: PipStyle,
    pub splats: SplatStyle,
}

impl CombatStyle {
    /// Reads every package's file in order, a later one changing what it gives, and refuses
    /// a style missing a kind or with a size or time that cannot be.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let (mut meter, mut pips, mut splats): (Option<MeterStyle>, Option<PipStyle>, Option<SplatStyle>) = (None, None, None);
        for file in files.iter().filter(|f| f.name == FILE) {
            let given: StyleFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            meter = given.meter.or(meter);
            pips = given.pips.or(pips);
            if let Some(given) = given.splats {
                splats = Some(match splats {
                    Some(mut was) => {
                        was.blows.extend(given.blows);
                        was.defended.extend(given.defended);
                        SplatStyle { blows: was.blows, defended: was.defended, ..given }
                    }
                    None => given,
                });
            }
        }
        let style = CombatStyle {
            meter: meter.ok_or(format!("no package's {FILE} gives the meter"))?,
            pips: pips.ok_or(format!("no package's {FILE} gives the pips"))?,
            splats: splats.ok_or(format!("no package's {FILE} gives the splats"))?,
        };
        for kind in BLOWS.iter().map(|b| blow_name(*b)) {
            style.splats.blows.get(kind).ok_or(format!("{FILE}: no splat for a {kind} blow"))?;
        }
        for kind in DEFENDED.iter().map(|d| defended_name(*d)) {
            style.splats.defended.get(kind).ok_or(format!("{FILE}: no splat for a blow {kind}"))?;
        }
        style.check()?;
        Ok(style)
    }

    /// Sizes and times that must be more than nothing, those that may be nothing but not
    /// less, the fade's share, the stack's height and the splats' scales.
    fn check(&self) -> Result<(), String> {
        let (meter, pips, splats) = (&self.meter, &self.pips, &self.splats);
        let positive = [("meter.width", meter.width), ("meter.height", meter.height), ("pips.size", pips.size), ("splats.seconds", splats.seconds), ("splats.flash.seconds", splats.flash.seconds)];
        for (name, value) in positive {
            if !(value > 0.0) {
                return Err(format!("{FILE}: {name} must be more than 0, not {value}"));
            }
        }
        let some = [("meter.below", meter.below), ("meter.linger", meter.linger), ("pips.gap", pips.gap), ("splats.rise", splats.rise), ("splats.gap", splats.gap), ("splats.hurt_above", splats.hurt_above)];
        for (name, value) in some {
            if !(value >= 0.0) {
                return Err(format!("{FILE}: {name} may not be less than 0 ({value})"));
            }
        }
        if !(0.0..1.0).contains(&splats.fade) {
            return Err(format!("{FILE}: splats.fade is the share of a splat's time before it fades, from 0 to under 1, not {}", splats.fade));
        }
        if splats.most == 0 {
            return Err(format!("{FILE}: splats.most must be at least 1"));
        }
        for (kind, splat) in splats.blows.iter().chain(&splats.defended) {
            if !(2..=6).contains(&splat.scale) {
                return Err(format!("{FILE}: the {kind} splat's scale must be from 2 to 6, not {}", splat.scale));
            }
        }
        Ok(())
    }

    pub fn blow(&self, blow: Blow) -> Splat {
        self.splats.blows[blow_name(blow)]
    }

    pub fn defended(&self, defended: Defended) -> Splat {
        self.splats.defended[defended_name(defended)]
    }
}

const BLOWS: [Blow; 5] = [Blow::Plain, Blow::Beat, Blow::Flourish, Blow::Counter, Blow::Bleed];
const DEFENDED: [Defended; 6] = [Defended::Hit, Defended::Missed, Defended::Blocked, Defended::Perfect, Defended::Dodged, Defended::Evaded];

pub fn blow_name(blow: Blow) -> &'static str {
    match blow {
        Blow::Plain => "plain",
        Blow::Beat => "beat",
        Blow::Flourish => "flourish",
        Blow::Counter => "counter",
        Blow::Bleed => "bleed",
    }
}

pub fn defended_name(defended: Defended) -> &'static str {
    match defended {
        Defended::Hit => "hit",
        Defended::Missed => "missed",
        Defended::Blocked => "blocked",
        Defended::Perfect => "perfect",
        Defended::Dodged => "dodged",
        Defended::Evaded => "evaded",
    }
}

/// The words of a splat for the player's blow: the damage, or Miss for none.
pub fn struck_words(locale: &Locale, damage: u16, blow: Blow) -> String {
    if damage == 0 && !matches!(blow, Blow::Bleed) {
        return locale.get("combat.splat.miss").to_owned();
    }
    locale.format(&format!("combat.splat.blow.{}", blow_name(blow)), &[("damage", &damage)])
}

/// The words of a splat for a blow at the player: a block that stopped it all, or a hit
/// the armour took whole, says so, with no "-0".
pub fn hurt_words(locale: &Locale, damage: u16, defended: Defended) -> String {
    match (defended, damage) {
        (Defended::Blocked, 0) => locale.get("combat.splat.defended.blocked_all").to_owned(),
        (Defended::Hit, 0) => locale.get("combat.splat.defended.unhurt").to_owned(),
        _ => locale.format(&format!("combat.splat.defended.{}", defended_name(defended)), &[("damage", &damage)]),
    }
}

impl CombatStyle {
    /// How the splat for the player's blow looks: by its kind, or as a miss if it did
    /// nothing (a bleed always did).
    pub fn struck_look(&self, damage: u16, blow: Blow) -> Splat {
        if damage == 0 && blow != Blow::Bleed { self.defended(Defended::Missed) } else { self.blow(blow) }
    }

    /// How the splat for a blow at the player looks: by what came of it, or as a miss if a
    /// hit did no harm.
    pub fn hurt_look(&self, damage: u16, defended: Defended) -> Splat {
        if damage == 0 && defended == Defended::Hit { self.defended(Defended::Missed) } else { self.defended(defended) }
    }
}

/// The meter and its pips.
#[derive(Component)]
pub struct MeterRoot;
#[derive(Component)]
struct MeterFill;
#[derive(Component)]
struct MeterBuffer;
#[derive(Component)]
struct Pip(u8);
/// Where splats are drawn.
#[derive(Component)]
struct SplatLayer;
/// A hitsplat: what it says of whom, and when it came (app seconds).
#[derive(Component)]
pub struct HitSplat {
    /// The creature struck, or none for a blow at the player.
    pub creature: Option<u32>,
    pub at: f64,
    /// Its text's size, for stacking before it is laid out.
    size: f32,
    /// The middle of the creature's body, kept once the body is gone.
    anchor: Option<Vec3>,
    /// Frames it was drawn over a body falling or gone.
    over_fallen: u32,
}

/// A splat's words, in its kind's colour (whole, before it fades).
#[derive(Component)]
pub struct SplatWords(pub Color);
/// One of the dark copies ringing a splat's words, so they read over fur and grass alike
/// (as stack counts are ringed, `hud::spawn_count`).
#[derive(Component)]
struct SplatRing;

/// How many font pixels the ring stands out from the words.
const RING_WIDTH: i32 = 2;

/// Where the ring's copies stand from the words, in font pixels: every spot within the ring's
/// width but the words' own and the square's corners, which rounds it.
fn ring() -> impl Iterator<Item = (f32, f32)> {
    (-RING_WIDTH..=RING_WIDTH)
        .flat_map(|x| (-RING_WIDTH..=RING_WIDTH).map(move |y| (x, y)))
        .filter(|&(x, y)| (x, y) != (0, 0) && x.abs() + y.abs() < 2 * RING_WIDTH)
        .map(|(x, y)| (x as f32, y as f32))
}

/// What the splats have drawn so far: for the gate to check against what the server told.
#[derive(Resource, Default)]
pub struct SplatsShown {
    /// How many splats were drawn for the player's blows, and for blows at the player.
    pub struck: u32,
    pub hurt: u32,
    /// The words of the last few drawn, newest last.
    pub recent: VecDeque<String>,
    /// Whether the meter has shown in a fight, and the most pips it lit.
    pub meter: bool,
    pub pips: u8,
    /// How many blows that hurt the player have flashed the health bar.
    pub flashes: u32,
    /// The most frames one splat was drawn over a body falling or gone: a killing blow's.
    pub over_fallen: u32,
    struck_seen: u32,
    hurt_seen: u32,
    /// When the blow that last flashed the health bar came (app seconds).
    flashed_at: Option<f64>,
}

impl SplatsShown {
    /// How many blows each way the splats have taken in, drawn or not.
    pub fn seen(&self) -> (u32, u32) {
        (self.struck_seen, self.hurt_seen)
    }

    fn drew(&mut self, words: &str) {
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(words.to_owned());
    }
}

/// A frame one interface pixel wide round what it holds.
fn framed(frame: Rgb) -> (Node, BackgroundColor) {
    (Node { padding: UiRect::all(Val::Px(PX)), ..default() }, BackgroundColor(frame.color()))
}

fn spawn(mut commands: Commands, launch: Res<Launch>, style: Res<CombatStyle>) {
    let (meter, pips) = (style.meter, style.pips);
    // Its top `below` under the middle of the screen, where the crosshair is.
    let column = Node {
        position_type: PositionType::Absolute,
        left: Val::Percent(50.0),
        top: Val::Percent(50.0),
        margin: UiRect::top(Val::Px(meter.below * PX)),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        ..default()
    };
    let centred = UiTransform::from_translation(Val2::percent(-50.0, 0.0));
    commands.spawn((column, centred, MeterRoot, Visibility::Hidden, GlobalZIndex(0))).with_children(|column| {
        column.spawn(framed(meter.frame)).with_children(|frame| {
            let track = Node { width: Val::Px(meter.width * PX), height: Val::Px(meter.height * PX), ..default() };
            frame.spawn((track, BackgroundColor(meter.empty.color()))).with_children(|track| {
                let full = Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() };
                track.spawn((full.clone(), BackgroundColor(meter.buffer.color()), MeterBuffer));
                track.spawn((full, BackgroundColor(meter.fill.color()), MeterFill));
            });
        });
        let row = Node { column_gap: Val::Px(pips.gap * PX), margin: UiRect::top(Val::Px(pips.gap * PX)), ..default() };
        column.spawn(row).with_children(|row| {
            for i in 0..launch.content.combat.feel.pips {
                row.spawn(framed(meter.frame)).with_children(|frame| {
                    frame.spawn((Node { width: Val::Px(pips.size * PX), height: Val::Px(pips.size * PX), ..default() }, BackgroundColor(pips.off.color()), Pip(i)));
                });
            }
        });
    });
    let layer = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() };
    commands.spawn((layer, SplatLayer, GlobalZIndex(0)));
}

/// The meter and pips, while a fight is on: a blow either way lately, a swing under way,
/// or pips lit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn meter(
    time: Res<Time>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    style: Res<CombatStyle>,
    fighting: Res<Fighting>,
    mut shown: ResMut<SplatsShown>,
    mut root: Query<&mut Visibility, With<MeterRoot>>,
    mut fill: Query<(&mut Node, &mut BackgroundColor), (With<MeterFill>, Without<MeterBuffer>)>,
    mut buffer: Query<&mut Node, (With<MeterBuffer>, Without<MeterFill>)>,
    mut pips: Query<(&Pip, &mut BackgroundColor), Without<MeterFill>>,
) {
    let Ok(mut visibility) = root.single_mut() else { return };
    let now = time.elapsed_secs_f64();
    let combat = &launch.content.combat;
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let timing = combat.timing(&combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill))));
    let last = fighting.struck.iter().map(|s| s.3).chain(fighting.hurt.iter().map(|h| h.3)).fold(f64::MIN, f64::max);
    let full = fighting.rhythm.meter(now, &timing);
    let lit = fighting.rhythm.pips(now, &combat.feel);
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open || player.dead;
    let fight = now - last < style.meter.linger as f64 || fighting.rhythm.pending().is_some() || lit > 0;
    visibility.set_if_neq(if fight && !screen { Visibility::Inherited } else { Visibility::Hidden });
    if !fight || screen {
        return;
    }
    shown.meter = true;
    shown.pips = shown.pips.max(lit);
    // On the beat: from the meter full until the beat's window closes.
    let on_beat = fighting.rhythm.beat(&timing).is_some_and(|beat| now >= beat && now - beat <= timing.beat as f64);
    if let Ok((mut node, mut color)) = fill.single_mut() {
        let width = Val::Percent(full * 100.0);
        if node.width != width {
            node.width = width;
        }
        color.set_if_neq(BackgroundColor(if on_beat { style.meter.beat } else { style.meter.fill }.color()));
    }
    // The buffer before the beat, where a click is held for it, shows at the track's end.
    if let Ok(mut node) = buffer.single_mut() {
        let share = (timing.buffer / timing.cooldown.max(1e-3)).clamp(0.0, 1.0);
        let left = Val::Percent((1.0 - share) * 100.0);
        if node.left != left {
            node.left = left;
            node.width = Val::Percent(share * 100.0);
        }
    }
    let ready = lit >= combat.feel.pips;
    for (pip, mut color) in &mut pips {
        let look = if pip.0 >= lit {
            style.pips.off
        } else if ready {
            style.pips.ready
        } else {
            style.pips.on
        };
        color.set_if_neq(BackgroundColor(look.color()));
    }
}

/// The splats' working lists, kept from frame to frame so none is allocated anew.
#[derive(Default)]
struct Scratch {
    entities: Vec<Entity>,
    stack: Vec<(Option<u32>, f64, f32)>,
    order: Vec<usize>,
    lifted: Vec<Option<f32>>,
}

/// Spawns a splat for each blow the server answered since, moves each with what it
/// marks, rises and fades it, and lets it go.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn splats(
    mut commands: Commands,
    time: Res<Time>,
    (ui, player): (Res<UiState>, Res<LocalPlayer>),
    launch: Res<Launch>,
    (locale, font): (Res<Locale>, Res<UiFont>),
    style: Res<CombatStyle>,
    fighting: Res<Fighting>,
    creatures: Res<Creatures>,
    scale: Res<UiScale>,
    settings: Res<Settings>,
    mut shown: ResMut<SplatsShown>,
    mut scratch: Local<Scratch>,
    (camera, layer): (Query<(&Camera, &GlobalTransform), With<Camera3d>>, Query<(Entity, &ComputedNode), With<SplatLayer>>),
    mut marks: Query<(Entity, &mut HitSplat, &ComputedNode, &mut Node, &mut Visibility, &Children)>,
    mut texts: Query<(&mut TextColor, Option<&SplatWords>), Or<(With<SplatWords>, With<SplatRing>)>>,
) {
    let Ok((layer, area)) = layer.single() else { return };
    let now = time.elapsed_secs_f64();
    let splat = &style.splats;
    // New since last time: the tails of what the server said.
    let new_struck = (fighting.struck_count - shown.struck_seen.min(fighting.struck_count)) as usize;
    let new_hurt = (fighting.hurt_count - shown.hurt_seen.min(fighting.hurt_count)) as usize;
    shown.struck_seen = fighting.struck_count;
    shown.hurt_seen = fighting.hurt_count;
    // With damage numbers off, none are drawn (and those already up go as they would).
    let (new_struck, new_hurt) = if settings.damage_numbers { (new_struck, new_hurt) } else { (0, 0) };
    let add = |commands: &mut Commands, creature: Option<u32>, words: String, look: Splat, at: f64| {
        let node = Node { position_type: PositionType::Absolute, ..default() };
        let centred = UiTransform::from_translation(Val2::percent(-50.0, -100.0));
        let size = font_size(look.scale);
        let pixel = size / font_size(1);
        let mark = HitSplat { creature, at, size, anchor: None, over_fallen: 0 };
        commands.entity(layer).with_children(|layer| {
            layer.spawn((node, centred, mark, Visibility::Hidden)).with_children(|splat| {
                for (x, y) in ring() {
                    let at = Node { position_type: PositionType::Absolute, left: Val::Px(x * pixel), top: Val::Px(y * pixel), ..default() };
                    splat.spawn((at, text(&font.0, words.clone(), size, super::OUTLINE), TightLine, SplatRing, crate::ui::fit::Decoration));
                }
                let colour = look.color.color();
                splat.spawn((text(&font.0, words, size, colour), TightLine, ZIndex(1), SplatWords(colour)));
            });
        });
    };
    for &(creature, damage, blow, at) in fighting.struck.iter().rev().take(new_struck).rev() {
        let words = struck_words(&locale, damage, blow);
        shown.struck += 1;
        shown.drew(&words);
        add(&mut commands, Some(creature), words, style.struck_look(damage, blow), at);
    }
    for &(_, damage, defended, at) in fighting.hurt.iter().rev().take(new_hurt).rev() {
        let words = hurt_words(&locale, damage, defended);
        shown.hurt += 1;
        shown.drew(&words);
        add(&mut commands, None, words, style.hurt_look(damage, defended), at);
    }
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open || player.dead;
    let Ok((camera, eye)) = camera.single() else { return };
    let size = area.size() * area.inverse_scale_factor();
    let hotbar = Vec2::new(size.x / 2.0, size.y - splat.hurt_above * PX - super::bottom_offset(&settings));
    let Scratch { entities, stack, order, lifted } = &mut *scratch;
    entities.clear();
    stack.clear();
    for (entity, mark, laid, ..) in &marks {
        entities.push(entity);
        stack.push((mark.creature, mark.at, (laid.size().y * laid.inverse_scale_factor()).max(mark.size)));
    }
    lifts(stack, splat.gap * PX, splat.most, order, lifted);
    for (&entity, &lift) in entities.iter().zip(lifted.iter()) {
        let Ok((_, mut mark, _, mut node, mut visibility, children)) = marks.get_mut(entity) else { continue };
        let age = (now - mark.at) as f32 / splat.seconds;
        // Gone with its time, or pushed off the top of a full stack.
        let Some(lift) = lift.filter(|_| age < 1.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        // Over the middle of the creature's body, alive or falling (or where it last was),
        // or over the hotbar for the player's own.
        let (base, fallen) = match mark.creature {
            Some(id) => {
                let body = creatures.centre(&launch, id);
                if let Some((centre, _)) = body {
                    mark.anchor = Some(centre);
                }
                let at = mark.anchor.and_then(|anchor| camera.world_to_viewport(eye, anchor).ok()).map(|at| at / scale.0);
                (at, body.is_none_or(|(_, falling)| falling))
            }
            None => (Some(hotbar), false),
        };
        let Some(base) = base.filter(|_| !screen) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let at = base - Vec2::Y * (lift + age * splat.rise * PX);
        node.left = Val::Px(at.x);
        node.top = Val::Px(at.y);
        // Whole for the first part of its time, then fading out, its ring with it.
        let alpha = if age < splat.fade { 1.0 } else { 1.0 - (age - splat.fade) / (1.0 - splat.fade) };
        for &child in children {
            if let Ok((mut colour, words)) = texts.get_mut(child) {
                let whole = words.map_or(super::OUTLINE, |w| w.0);
                colour.set_if_neq(TextColor(whole.with_alpha(whole.alpha() * alpha)));
            }
        }
        visibility.set_if_neq(Visibility::Inherited);
        if fallen {
            mark.over_fallen += 1;
            shown.over_fallen = shown.over_fallen.max(mark.over_fallen);
        }
    }
}

/// How far above its place each splat stands, for splats given as (whom, when, height),
/// into `lifts` (with `order` to work in): those on one creature (or on the player) stack
/// upward, newest lowest and each older one its newer neighbour's height and `gap` higher,
/// so blows close together never cover each other; as they all rise alike, an older one
/// stays above a newer. Past the `most` newest on one, a splat stands nowhere (None): the
/// oldest leave early.
pub fn lifts(splats: &[(Option<u32>, f64, f32)], gap: f32, most: usize, order: &mut Vec<usize>, lifts: &mut Vec<Option<f32>>) {
    order.clear();
    order.extend(0..splats.len());
    // By whom, then newest first (the later told first, of two told at once).
    order.sort_by(|&a, &b| splats[a].0.cmp(&splats[b].0).then(splats[b].1.total_cmp(&splats[a].1)).then(b.cmp(&a)));
    lifts.clear();
    lifts.resize(splats.len(), None);
    let mut above = (None, 0.0, 0);
    for &i in order.iter() {
        let (whom, _, height) = splats[i];
        if above.0 != Some(whom) {
            above = (Some(whom), 0.0, 0);
        }
        if above.2 < most {
            lifts[i] = Some(above.1);
        }
        above.1 += height + gap;
        above.2 += 1;
    }
}

/// The health bar's colour for what was lost, `age` seconds after a blow hurt the player:
/// the flash's at first, the bar's own once it is over.
pub fn flash_colour(flash: &FlashStyle, age: f32) -> Color {
    let share = (age / flash.seconds).clamp(0.0, 1.0);
    Color::from(Srgba::from(flash.color.color()).mix(&Srgba::from(TRAIL), share))
}

/// A blow that hurt the player, a block that let some through too, flashes what the health
/// bar lost, then lets it drain as any loss does.
fn flash(
    time: Res<Time>,
    style: Res<CombatStyle>,
    fighting: Res<Fighting>,
    mut shown: ResMut<SplatsShown>,
    health: Query<&Meter, With<HealthMeter>>,
    mut colours: Query<&mut BackgroundColor>,
) {
    let Ok(meter) = health.single() else { return };
    let last = fighting.hurt.iter().filter(|h| h.1 > 0).map(|h| h.3).reduce(f64::max);
    if let Some(last) = last
        && shown.flashed_at.is_none_or(|was| last > was)
    {
        shown.flashed_at = Some(last);
        shown.flashes += 1;
    }
    let age = shown.flashed_at.map_or(f32::INFINITY, |at| (time.elapsed_secs_f64() - at) as f32);
    let colour = flash_colour(&style.splats.flash, age);
    if let Ok(mut background) = colours.get_mut(meter.trail()) {
        background.set_if_neq(BackgroundColor(colour));
    }
}

/// Leaving the game: the meter hidden, the splats gone, the health bar's own colour back.
fn hide(
    mut commands: Commands,
    mut root: Query<&mut Visibility, With<MeterRoot>>,
    marks: Query<Entity, With<HitSplat>>,
    health: Query<&Meter, With<HealthMeter>>,
    mut colours: Query<&mut BackgroundColor>,
) {
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = Visibility::Hidden;
    }
    for mark in &marks {
        commands.entity(mark).despawn();
    }
    if let Ok(meter) = health.single()
        && let Ok(mut background) = colours.get_mut(meter.trail())
    {
        background.0 = TRAIL;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    fn assets() -> Vec<ContentFile> {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        content.files.iter().cloned().collect()
    }

    /// A whole style but for `splats`, which is `(…)` around the given fields.
    fn style_with(splats: &str) -> String {
        format!(
            r#"(meter: (width: 24, height: 3, below: 12, frame: (0,0,0), empty: (0,0,0), fill: (0,0,0), beat: (0,0,0), buffer: (0,0,0), linger: 3.0),
            pips: (size: 3, gap: 2, off: (0,0,0), on: (0,0,0), ready: (0,0,0)),
            splats: ({splats}))"#
        )
    }

    const KINDS: &str = r#"blows: { "plain": (color: (0,0,0)), "beat": (color: (0,0,0)), "flourish": (color: (0,0,0)), "counter": (color: (0,0,0)), "bleed": (color: (0,0,0)) },
        defended: { "hit": (color: (0,0,0)), "missed": (color: (0,0,0)), "blocked": (color: (0,0,0)), "perfect": (color: (0,0,0)), "dodged": (color: (0,0,0)), "evaded": (color: (0,0,0)) }"#;

    #[test]
    fn the_games_style_has_every_kind_and_a_later_package_changes_one() {
        let files = assets();
        let style = CombatStyle::load(&files).unwrap();
        assert!(style.meter.width > 0.0 && style.splats.seconds > 0.0);
        let mut modded = files.clone();
        modded.push(file("mod", r#"(splats: (seconds: 2.0, rise: 30, fade: 0.25, gap: 1, most: 4, hurt_above: 64, blows: { "flourish": (color: (1, 2, 3), scale: 5) }, defended: {}, flash: (color: (9, 9, 9), seconds: 1.0)))"#));
        let changed = CombatStyle::load(&modded).unwrap();
        assert_eq!(changed.blow(Blow::Flourish), Splat { color: Rgb(1, 2, 3), scale: 5 });
        assert_eq!(changed.splats.flash, FlashStyle { color: Rgb(9, 9, 9), seconds: 1.0 });
        assert_eq!(changed.blow(Blow::Plain), style.blow(Blow::Plain), "kinds the mod does not give stay");
        assert_eq!((changed.splats.seconds, changed.splats.most), (2.0, 4));
        assert_eq!(changed.meter, style.meter);
    }

    #[test]
    fn a_style_missing_a_kind_is_refused() {
        let source = style_with(r#"seconds: 1.0, rise: 10, fade: 0.5, gap: 1, most: 6, hurt_above: 60, blows: { "plain": (color: (0,0,0)) }, defended: {}, flash: (color: (0,0,0), seconds: 0.5)"#);
        let refused = CombatStyle::load(&[file("core", &source)]).unwrap_err();
        assert!(refused.contains("no splat for a beat blow"), "{refused}");
    }

    /// Times and sizes that cannot be are refused at load, naming what is wrong, rather than
    /// clamped where they are used.
    #[test]
    fn a_style_with_impossible_numbers_is_refused() {
        let good = "seconds: 0.9, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70";
        let with = |numbers: &str, flash: f32| style_with(&format!("{numbers}, {KINDS}, flash: (color: (0,0,0), seconds: {flash})"));
        assert!(CombatStyle::load(&[file("core", &with(good, 0.6))]).is_ok());
        for (bad, says) in [
            ("seconds: 0.0, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.seconds"),
            ("seconds: 0.9, rise: -1, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.rise"),
            ("seconds: 0.9, rise: 14, fade: 1.0, gap: 1, most: 6, hurt_above: 70", "splats.fade"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: -2, most: 6, hurt_above: 70", "splats.gap"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: 1, most: 0, hurt_above: 70", "splats.most"),
        ] {
            let refused = CombatStyle::load(&[file("core", &with(bad, 0.6))]).unwrap_err();
            assert!(refused.contains(says), "{bad}: {refused}");
        }
        assert!(CombatStyle::load(&[file("core", &with(good, 0.0))]).unwrap_err().contains("flash.seconds"));
        let tiny = style_with(&format!("{good}, {}, flash: (color: (0,0,0), seconds: 0.6)", KINDS.replacen("\"plain\": (color: (0,0,0))", "\"plain\": (color: (0,0,0), scale: 1)", 1)));
        assert!(CombatStyle::load(&[file("core", &tiny)]).unwrap_err().contains("plain splat's scale"));
        let negative_linger = with(good, 0.6).replace("linger: 3.0", "linger: -1.0");
        assert!(CombatStyle::load(&[file("core", &negative_linger)]).unwrap_err().contains("meter.linger"));
    }

    /// Splats on one creature stack upward, newest lowest, apart from another creature's
    /// and the player's own; two told at once still stand apart; past the most, the oldest
    /// stand nowhere.
    #[test]
    fn splats_on_one_creature_stack_newest_lowest() {
        let splats = [(Some(7), 1.0, 16.0), (Some(7), 1.2, 32.0), (None, 1.1, 16.0), (Some(7), 1.1, 16.0), (Some(9), 1.0, 16.0), (None, 1.1, 16.0)];
        let (mut order, mut lifted) = (Vec::new(), Vec::new());
        lifts(&splats, 2.0, 6, &mut order, &mut lifted);
        // Creature 7: newest (1.2, 32 high) at 0, then 1.1 above it, then 1.0 above that.
        assert_eq!((lifted[1], lifted[3], lifted[0]), (Some(0.0), Some(34.0), Some(52.0)));
        assert_eq!(lifted[4], Some(0.0), "another creature's stack starts on its own");
        // The player's two, told at once: the later told lowest.
        assert_eq!((lifted[5], lifted[2]), (Some(0.0), Some(18.0)));
        lifts(&splats, 2.0, 2, &mut order, &mut lifted);
        assert_eq!((lifted[1], lifted[3], lifted[0]), (Some(0.0), Some(34.0), None), "the oldest of three leaves a stack of two");
    }

    /// A blow that hurts shows the flash's colour on what the bar lost, fading to the bar's
    /// own, and stays there once the flash is over.
    #[test]
    fn a_hurt_flashes_the_health_bar_then_fades_to_its_own_colour() {
        let flash = FlashStyle { color: Rgb(236, 76, 64), seconds: 0.6 };
        let srgba = |c: Color| Srgba::from(c);
        assert_eq!(srgba(flash_colour(&flash, 0.0)), srgba(flash.color.color()));
        assert_eq!(srgba(flash_colour(&flash, 0.6)), srgba(TRAIL));
        assert_eq!(srgba(flash_colour(&flash, f32::INFINITY)), srgba(TRAIL), "no blow yet");
        let half = srgba(flash_colour(&flash, 0.3));
        let (from, to) = (srgba(flash.color.color()), srgba(TRAIL));
        assert!((half.green - (from.green + to.green) / 2.0).abs() < 0.01 && (half.alpha - (from.alpha + to.alpha) / 2.0).abs() < 0.01, "{half:?}");
    }

    /// Every blow and every outcome reads in each language, with its damage where it has
    /// one; a bleed says it is one, and a block that stopped it all shows no number.
    #[test]
    fn splats_have_words_in_every_language() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for code in ["en", "ja"] {
            let locale = Locale::load(&dir, code);
            for blow in BLOWS {
                let words = struck_words(&locale, 12, blow);
                assert!(words.contains("12"), "{code}: {blow:?} reads {words:?}");
            }
            assert_ne!(struck_words(&locale, 12, Blow::Bleed), struck_words(&locale, 12, Blow::Plain), "{code}: a bleed reads as a plain blow");
            assert!(!struck_words(&locale, 0, Blow::Plain).contains('0'), "{code}: a miss shows a number");
            for defended in DEFENDED {
                for damage in [0, 7] {
                    let words = hurt_words(&locale, damage, defended);
                    assert!(!words.is_empty() && !words.contains("combat.") && !words.contains("-0"), "{code}: {defended:?} for {damage} reads {words:?}");
                }
            }
            assert!(hurt_words(&locale, 7, Defended::Hit).contains('7') && hurt_words(&locale, 7, Defended::Blocked).contains('7'));
        }
    }
}
