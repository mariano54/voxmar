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
            .add_systems(PostUpdate, splats
                .after(TransformSystems::Propagate)
                .after(bevy::camera::CameraUpdateSystems)
                .before(bevy::ui::UiSystems::Layout)
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                .run_if(in_state(Screen::InGame)))
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
    #[serde(default = "default_scale")]
    pub scale: u32,
}

fn default_scale() -> u32 {
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
    /// Least continuous visible time over a fallen body; zero keeps the normal lifetime.
    #[serde(default)]
    pub fallen_seconds: f32,
    /// How long after the killing blow its mark waits for the target's fall to be drawn
    /// before giving up, past its normal lifetime. The death can reach the client, or the
    /// mark's layout, later than a splat's whole life on a busy machine; zero keeps the
    /// normal lifetime.
    #[serde(default)]
    pub fallen_wait: f32,
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
            if !(value > 0.0 && value.is_finite()) {
                return Err(format!("{FILE}: {name} must be more than 0 and finite, not {value}"));
            }
        }
        let some = [("meter.below", meter.below), ("meter.linger", meter.linger), ("pips.gap", pips.gap), ("splats.fallen_seconds", splats.fallen_seconds), ("splats.fallen_wait", splats.fallen_wait), ("splats.rise", splats.rise), ("splats.gap", splats.gap), ("splats.hurt_above", splats.hurt_above)];
        for (name, value) in some {
            if !(value >= 0.0 && value.is_finite()) {
                return Err(format!("{FILE}: {name} may not be less than 0 or infinite ({value})"));
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

const BLOWS: [Blow; 6] = [Blow::Plain, Blow::Beat, Blow::Flourish, Blow::Counter, Blow::Bleed, Blow::Unaware];
const DEFENDED: [Defended; 6] = [Defended::Hit, Defended::Missed, Defended::Blocked, Defended::Perfect, Defended::Dodged, Defended::Evaded];

pub fn blow_name(blow: Blow) -> &'static str {
    match blow {
        Blow::Plain => "plain",
        Blow::Beat => "beat",
        Blow::Flourish => "flourish",
        Blow::Counter => "counter",
        Blow::Bleed => "bleed",
        Blow::Unaware => "unaware",
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
    /// The server said this exact blow killed its target, rather than an earlier hit.
    pub(crate) killed: bool,
    /// Its text's size, for stacking before it is laid out.
    size: f32,
    /// The middle of the creature's body, kept once the body is gone.
    anchor: Option<Vec3>,
    /// First and most recent continuous draws over a fallen body (game seconds).
    fallen_draws: Option<(f64, f64)>,
    /// Whether it was ever drawn over a fallen body: a killing blow's mark that has not
    /// been waits for the fall past its normal lifetime (`SplatStyle::fallen_wait`), one
    /// that has gets no second start.
    fell: bool,
}

#[derive(Debug, PartialEq)]
enum SplatFrame {
    Gone,
    Hidden,
    Draw { age: f32, over_fallen: f32 },
}

impl HitSplat {
    /// The current continuous visible interval over this mark's fallen target.
    /// Hidden frames clear it; the gate keeps the killed target in view before leaving.
    pub(crate) fn fallen_drawn_for(&self) -> f32 {
        self.fallen_draws.map_or(0.0, |(first, last)| (last - first) as f32)
    }

    pub(crate) fn anchor(&self) -> Option<Vec3> {
        self.anchor
    }

    /// Whether this killing blow's mark still waits for its target's fall to be drawn.
    fn awaiting(&self, now: f64, style: &SplatStyle) -> bool {
        self.killed && style.fallen_seconds > 0.0 && !self.fell && now - self.at < style.fallen_wait as f64
    }

    fn age(&self, now: f64, style: &SplatStyle, fallen: bool) -> f32 {
        let age = ((now - self.at) as f32 / style.seconds).max(0.0);
        if (fallen || self.awaiting(now, style)) && style.fallen_seconds > 0.0
            && self.fallen_draws.is_none_or(|(first, last)| last - first < style.fallen_seconds as f64)
        {
            // Keep a readable, bounded rise and positive opacity while the hold is open.
            age.min(style.fade)
        } else {
            age
        }
    }

    /// Decide lifetime after drawable eligibility. Hidden time resets continuity; a
    /// slow frame that completes the hold is drawn before expiry on the next update. A
    /// killing blow's mark that has not yet been drawn over its fallen target outlives its
    /// normal time (up to `fallen_wait` from the blow), so a death or a layout that came
    /// late still gets its hold; once drawn over the body it gets no second start.
    fn frame(&mut self, now: f64, style: &SplatStyle, drawable: bool, fallen: bool) -> SplatFrame {
        let expired = now - self.at >= style.seconds as f64;
        let holding = drawable && fallen && self.fallen_draws.is_some_and(|(first, last)| last - first < style.fallen_seconds as f64);
        if expired && !holding && !self.awaiting(now, style) {
            return SplatFrame::Gone;
        }
        if !drawable {
            self.fallen_draws = None;
            return SplatFrame::Hidden;
        }
        let age = self.age(now, style, fallen);
        let over_fallen = if fallen {
            self.fell = true;
            let (first, last) = self.fallen_draws.get_or_insert((now, now));
            *last = now;
            (now - *first) as f32
        } else {
            self.fallen_draws = None;
            0.0
        };
        SplatFrame::Draw { age, over_fallen }
    }
}

/// The mark is centred horizontally and stands above its anchor by its laid-out height.
fn splat_in_view(at: Vec2, laid: Vec2, viewport: Vec2) -> bool {
    laid.x > 0.0 && laid.y > 0.0
        && at.x + laid.x * 0.5 > 0.0 && at.x - laid.x * 0.5 < viewport.x
        && at.y > 0.0 && at.y - laid.y < viewport.y
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
    /// The longest, in game seconds, one splat was drawn over a body falling or gone: a
    /// killing blow's. (Time, not frames: a frame may take a quarter second under load.)
    pub over_fallen: f32,
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
    let add = |commands: &mut Commands, creature: Option<u32>, words: String, look: Splat, at: f64, killed: bool| {
        let node = Node { position_type: PositionType::Absolute, ..default() };
        let centred = UiTransform::from_translation(Val2::percent(-50.0, -100.0));
        let size = font_size(look.scale);
        let pixel = size / font_size(1);
        let mark = HitSplat { creature, at, killed, size, anchor: None, fallen_draws: None, fell: false };
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
    for &(creature, damage, blow, at, killed) in fighting.struck.iter().rev().take(new_struck).rev() {
        let words = struck_words(&locale, damage, blow);
        shown.struck += 1;
        shown.drew(&words);
        add(&mut commands, Some(creature), words, style.struck_look(damage, blow), at, killed);
    }
    for &(_, damage, defended, at) in fighting.hurt.iter().rev().take(new_hurt).rev() {
        let words = hurt_words(&locale, damage, defended);
        shown.hurt += 1;
        shown.drew(&words);
        add(&mut commands, None, words, style.hurt_look(damage, defended), at, false);
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
        let Ok((_, mut mark, laid, mut node, mut visibility, children)) = marks.get_mut(entity) else { continue };
        // Stack eviction is unconditional, including a mark held over a fallen body.
        let Some(lift) = lift else {
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
        let age = mark.age(now, splat, fallen);
        let at = base.map(|base| base - Vec2::Y * (lift + age * splat.rise * PX));
        let drawable = !screen && at.is_some_and(|at| splat_in_view(at, laid.size() * laid.inverse_scale_factor(), size));
        let (age, over_fallen) = match mark.frame(now, splat, drawable, fallen) {
            SplatFrame::Gone => {
                commands.entity(entity).despawn();
                continue;
            }
            SplatFrame::Hidden => {
                visibility.set_if_neq(Visibility::Hidden);
                continue;
            }
            SplatFrame::Draw { age, over_fallen } => (age, over_fallen),
        };
        let at = at.expect("a drawable mark has an anchor");
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
        shown.over_fallen = shown.over_fallen.max(over_fallen);
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

    #[test]
    fn killing_splats_draw_the_completion_frame_before_expiring() {
        let style = CombatStyle::load(&assets()).unwrap().splats;
        let mut mark = HitSplat { creature: Some(1), at: 10.0, killed: true, size: 24.0, anchor: None, fallen_draws: None, fell: false };
        let draw = |frame| match frame {
            SplatFrame::Draw { age, over_fallen } => {
                assert!(age < 1.0, "the hold must have positive alpha");
                over_fallen
            }
            other => panic!("a visible fallen mark disappeared: {other:?}"),
        };
        assert_eq!(draw(mark.frame(10.73, &style, true, true)), 0.0);
        assert!((draw(mark.frame(10.95, &style, true, true)) - 0.22).abs() < 1e-5);
        // One long frame crosses both normal expiry and the fallen hold. It must still
        // draw and report its actual continuous interval before the next update removes it.
        assert!((draw(mark.frame(11.20, &style, true, true)) - 0.47).abs() < 1e-5);
        assert_eq!(mark.at, 10.0, "holding does not change stack order or the blow time");
        assert_eq!(mark.frame(11.21, &style, true, true), SplatFrame::Gone);
    }

    #[test]
    fn hidden_fallen_splats_reset_continuity_and_cannot_rearm_after_expiry() {
        let style = CombatStyle::load(&assets()).unwrap().splats;
        let mut mark = HitSplat { creature: Some(1), at: 10.0, killed: true, size: 24.0, anchor: None, fallen_draws: None, fell: false };
        assert!(matches!(mark.frame(10.60, &style, true, true), SplatFrame::Draw { over_fallen: 0.0, .. }));
        assert_eq!(mark.frame(10.80, &style, false, true), SplatFrame::Hidden);
        assert!(matches!(mark.frame(10.85, &style, true, true), SplatFrame::Draw { over_fallen: 0.0, .. }));
        assert!(matches!(mark.frame(10.95, &style, true, true), SplatFrame::Draw { over_fallen, .. } if (over_fallen - 0.10).abs() < 1e-5));
        assert_eq!(mark.frame(11.0, &style, false, true), SplatFrame::Gone, "an expired hidden mark cannot wait indefinitely");
        let mut unseen = HitSplat { fallen_draws: None, ..mark };
        assert!(unseen.fell, "it was drawn over the body once, which is its one start");
        assert_eq!(unseen.frame(10.8, &style, false, true), SplatFrame::Hidden);
        assert_eq!(unseen.frame(11.0, &style, true, true), SplatFrame::Gone, "no revival after the one start");
    }

    #[test]
    fn a_killing_mark_waits_for_a_late_fall_but_only_for_so_long() {
        let style = CombatStyle::load(&assets()).unwrap().splats;
        assert!(style.fallen_wait > style.seconds, "the wait must reach past the normal lifetime");
        let killing = || HitSplat { creature: Some(1), at: 10.0, killed: true, size: 24.0, anchor: None, fallen_draws: None, fell: false };
        // The death reached the client after the mark's whole life: it still gets its hold
        // over the body, and the hold completes (0.25 s of continuous draws) before it goes.
        let mut mark = killing();
        assert!(matches!(mark.frame(10.5, &style, true, false), SplatFrame::Draw { over_fallen: 0.0, .. }));
        assert!(matches!(mark.frame(10.95, &style, true, false), SplatFrame::Draw { over_fallen: 0.0, age, .. } if age > 0.0 && age < 1.0), "waiting, whole and readable");
        assert!(matches!(mark.frame(11.2, &style, true, true), SplatFrame::Draw { over_fallen: 0.0, .. }));
        assert!(matches!(mark.frame(11.45, &style, true, true), SplatFrame::Draw { over_fallen, .. } if (over_fallen - 0.25).abs() < 1e-5));
        assert_eq!(mark.frame(11.5, &style, true, true), SplatFrame::Gone, "the hold is over");
        // Its layout was late, so it was not drawable until after the normal lifetime.
        let mut mark = killing();
        assert_eq!(mark.frame(10.3, &style, false, true), SplatFrame::Hidden);
        assert_eq!(mark.frame(10.6, &style, false, true), SplatFrame::Hidden);
        assert!(matches!(mark.frame(10.95, &style, true, true), SplatFrame::Draw { over_fallen: 0.0, .. }), "the first draw after the normal lifetime still starts the hold");
        // A body that never falls does not keep it for ever, and a blow that did not kill
        // or a mark on the player gets no wait.
        let mut mark = killing();
        assert!(matches!(mark.frame(11.4, &style, true, false), SplatFrame::Draw { .. }));
        assert_eq!(mark.frame(10.0 + style.fallen_wait as f64, &style, true, false), SplatFrame::Gone);
        let mut hit = HitSplat { killed: false, ..killing() };
        assert_eq!(hit.frame(10.95, &style, true, true), SplatFrame::Gone);
    }

    #[test]
    fn ordinary_splats_expire_normally_and_offscreen_marks_do_not_count() {
        let style = CombatStyle::load(&assets()).unwrap().splats;
        for creature in [Some(1), None] {
            let mut mark = HitSplat { creature, at: 10.0, killed: false, size: 24.0, anchor: None, fallen_draws: None, fell: false };
            assert!(matches!(mark.frame(10.8, &style, true, false), SplatFrame::Draw { over_fallen: 0.0, .. }));
            assert_eq!(mark.frame(10.95, &style, true, false), SplatFrame::Gone);
        }
        let viewport = Vec2::new(960.0, 540.0);
        let laid = Vec2::new(40.0, 24.0);
        assert!(splat_in_view(Vec2::new(480.0, 270.0), laid, viewport));
        assert!(splat_in_view(Vec2::new(-10.0, 10.0), laid, viewport), "a partly visible mark counts");
        for at in [Vec2::new(-21.0, 270.0), Vec2::new(981.0, 270.0), Vec2::new(480.0, -1.0), Vec2::new(480.0, 565.0)] {
            assert!(!splat_in_view(at, laid, viewport), "off-screen mark at {at}");
        }
        assert!(!splat_in_view(Vec2::new(480.0, 270.0), Vec2::ZERO, viewport), "layout must exist before a draw counts");
    }

    #[test]
    fn corpse_watch_keeps_the_actual_snail_mark_in_view_before_the_loot_walk() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let style = CombatStyle::load(&assets()).unwrap().splats;
        let viewport = Vec2::new(960.0, 540.0);
        let laid = Vec2::new(40.0, 24.0);
        let anchor = Vec3::new(0.0, 0.2, -1.3);
        let mut camera = Camera::default();
        camera.computed.clip_from_view = PerspectiveProjection {
            fov: 75f32.to_radians(), near: 0.05, aspect_ratio: viewport.x / viewport.y, ..default()
        }.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo { physical_size: viewport.as_uvec2(), scale_factor: 1.0 });
        // The old loot walk judged by the old lifetime: a killing blow's mark that waited for
        // its fall was the late-death fix, a different thing from the watch held here.
        let old_style = SplatStyle { fallen_wait: 0.0, ..style.clone() };
        let simulate = |watch: bool| {
            let style = if watch { &style } else { &old_style };
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin).init_resource::<LocalPlayer>();
            app.world_mut().resource_mut::<LocalPlayer>().position = Vec3::ZERO;
            let entity = app.world_mut().spawn((HitSplat {
                creature: Some(7), at: 10.0, killed: true, size: 24.0, anchor: Some(anchor), fallen_draws: None, fell: false,
            }, Visibility::Inherited)).id();
            // Last used to hand W on and aim at loot's standing height before watching
            // the fallen mark. At close melee range, the corrected watch must bring that target back into view.
            let aim = |world: &mut World, at: Vec3| {
                let mut player = world.resource_mut::<LocalPlayer>();
                let to = at - player.eye();
                player.yaw = (-to.x).atan2(-to.z);
                player.pitch = to.y.atan2(to.xz().length());
            };
            aim(app.world_mut(), Vec3::new(0.0, 1.6, -1.3));
            app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
            let mut longest = 0.0f32;
            let mut completed = false;
            let mut since = None;
            for frame in 0..16 {
                let now = 10.0 + frame as f64 * 0.08;
                let player = app.world().resource::<LocalPlayer>();
                let eye = GlobalTransform::from(Transform::from_translation(player.eye()).with_rotation(player.rotation()));
                let mut mark = app.world_mut().get_mut::<HitSplat>(entity).unwrap();
                let at = camera.world_to_viewport(&eye, anchor).ok()
                    .map(|at| at - Vec2::Y * mark.age(now, &style, true) * style.rise * PX);
                let drawable = at.is_some_and(|at| splat_in_view(at, laid, viewport));
                let visible = match mark.frame(now, &style, drawable, true) {
                    SplatFrame::Draw { over_fallen, .. } => {
                        longest = longest.max(over_fallen);
                        Visibility::Inherited
                    }
                    _ => Visibility::Hidden,
                };
                app.world_mut().entity_mut(entity).insert(visible);
                if watch {
                    if let Some(result) = crate::gate::watch_fallen(app.world_mut(), Some(7), Some(anchor), &mut since, 100.0 + frame as f32 * 0.08) {
                        assert!(result.is_ok(), "the target's visible interval must end the watch");
                        completed = true;
                        break;
                    }
                    app.update();
                    assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
                    assert!(!app.world().resource::<ButtonInput<MouseButton>>().pressed(MouseButton::Left));
                } else {
                    aim(app.world_mut(), Vec3::new(0.0, 1.6, -1.3));
                    // The loot walk closes the final striking distance; its corpse goes
                    // below/behind the camera, as in Strike's original phase 1.
                    app.world_mut().resource_mut::<LocalPlayer>().position.z -= 0.25;
                }
            }
            (longest, completed)
        };
        let (old, _) = simulate(false);
        assert!(old < 0.2, "the old immediate loot view must actually miss the required interval: {old}");
        let (fixed, completed) = simulate(true);
        assert!(completed && fixed >= 0.2, "the corrected watch must see 0.2 game seconds: {fixed}");
    }

    #[test]
    fn corpse_watch_rejects_other_targets_hidden_marks_and_missing_marks() {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin).init_resource::<LocalPlayer>();
        let entity = app.world_mut().spawn((HitSplat {
            creature: Some(8), at: 10.0, killed: true, size: 24.0, anchor: None, fallen_draws: Some((10.0, 10.3)), fell: true,
        }, Visibility::Inherited)).id();
        let anchor = Vec3::new(0.0, 0.2, -1.6);
        let mut since = None;
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), Some(7), Some(anchor), &mut since, 100.0), None, "another killed target cannot satisfy the watch");
        assert_eq!(since, Some(100.0), "the watch starts at death after a long fight, not at the fight's start");
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), Some(7), Some(anchor), &mut since, 106.0), Some(Err(0.0)), "missing marks fail in bounded time");
        app.world_mut().get_mut::<HitSplat>(entity).unwrap().creature = Some(7);
        app.world_mut().entity_mut(entity).insert(Visibility::Hidden);
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), Some(7), Some(anchor), &mut since, 106.0), Some(Err(0.0)), "hidden time is not visibility");
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), None, Some(anchor), &mut since, 106.0), Some(Err(0.0)), "player splats cannot substitute for a killed target");
        app.world_mut().entity_mut(entity).insert(Visibility::Inherited);
        app.world_mut().get_mut::<HitSplat>(entity).unwrap().killed = false;
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), Some(7), Some(anchor), &mut since, 106.0), Some(Err(0.0)), "a missing/admin-removed body is not a server-confirmed killing blow");
    }

    #[test]
    fn corpse_watch_requires_the_killing_mark_even_when_stamps_match_and_never_passes_after_deadline() {
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin).init_resource::<LocalPlayer>();
        let anchor = Vec3::new(0.0, 0.2, -1.6);
        app.world_mut().spawn((HitSplat {
            creature: Some(7), at: 10.0, killed: false, size: 24.0, anchor: Some(anchor), fallen_draws: Some((10.0, 10.3)), fell: true,
        }, Visibility::Inherited));
        let killing = app.world_mut().spawn((HitSplat {
            creature: Some(7), at: 10.0, killed: true, size: 24.0, anchor: Some(anchor), fallen_draws: None, fell: false,
        }, Visibility::Inherited)).id();
        let mut since = None;
        assert_eq!(crate::gate::watch_fallen(app.world_mut(), Some(7), None, &mut since, 100.0), None,
            "a visible earlier hit with the same event stamp cannot substitute for the killing blow");
        app.world_mut().get_mut::<HitSplat>(killing).unwrap().fallen_draws = Some((10.0, 10.3));
        assert!(matches!(crate::gate::watch_fallen(app.world_mut(), Some(7), None, &mut since, 100.3), Some(Ok(seen)) if seen >= 0.2));
        assert!(matches!(crate::gate::watch_fallen(app.world_mut(), Some(7), None, &mut since, 106.0), Some(Err(seen)) if seen >= 0.2),
            "success first observed at/after the deadline cannot pass");
        assert!(matches!(crate::gate::watch_fallen(app.world_mut(), Some(7), None, &mut since, 106.1), Some(Err(_))),
            "a timed out watch cannot rearm on a later direct call");
    }

    #[test]
    fn corpse_watch_observes_deferred_real_splats_and_excludes_unlaid_and_menu_frames() {
        use bevy::camera::{CameraProjection, RenderTargetInfo};
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let content = voxmar_server::load_content(&dir.parent().unwrap()).unwrap();
        let style = CombatStyle::load(&content.files).unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin)
            .init_resource::<Time>().init_resource::<UiState>().init_resource::<LocalPlayer>()
            .init_resource::<Fighting>().init_resource::<Creatures>().init_resource::<UiScale>()
            .init_resource::<Settings>().init_resource::<SplatsShown>()
            .insert_resource(Launch { name: "gate".into(), host: false, content })
            .insert_resource(Locale::load(&dir, "en")).insert_resource(UiFont(Handle::default()))
            .insert_resource(style).add_systems(Update, splats);
        app.world_mut().resource_mut::<LocalPlayer>().position = Vec3::ZERO;
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs(10));
        let anchor = Vec3::new(0.0, 0.2, -1.6);
        let mut camera = Camera::default();
        camera.computed.clip_from_view = PerspectiveProjection {
            fov: 75f32.to_radians(), near: 0.05, aspect_ratio: 960.0 / 540.0, ..default()
        }.get_clip_from_view();
        camera.computed.target_info = Some(RenderTargetInfo { physical_size: UVec2::new(960, 540), scale_factor: 1.0 });
        let eye = app.world().resource::<LocalPlayer>().eye();
        let camera_entity = app.world_mut().spawn((Camera3d::default(), camera,
            GlobalTransform::from(Transform::from_translation(eye).looking_at(anchor, Vec3::Y)))).id();
        app.world_mut().spawn((Node::default(), SplatLayer, ComputedNode { size: Vec2::new(960.0, 540.0), ..default() }));
        app.world_mut().resource_mut::<Fighting>().struck(7, 1, Blow::Plain, 0, false, 10.0);
        app.world_mut().resource_mut::<Fighting>().struck(7, 4, Blow::Plain, 0, true, 10.0);
        app.update();
        let mut query = app.world_mut().query::<(Entity, &HitSplat)>();
        assert_eq!(query.iter(app.world()).count(), 2);
        let entity = query.iter(app.world()).find(|(_, mark)| mark.killed).unwrap().0;
        assert!(query.iter(app.world()).any(|(_, mark)| !mark.killed && mark.at == 10.0), "same-stamp events preserve their own killed flags through real Commands");
        assert_eq!(*app.world().get::<Visibility>(entity).unwrap(), Visibility::Hidden, "Commands spawn after this frame's query");
        assert_eq!(app.world().get::<HitSplat>(entity).unwrap().fallen_drawn_for(), 0.0);
        // Supply the real mark's last body anchor after disappearance. Layout remains
        // zero for its first update; then supply the measured rectangle, as UI layout does.
        app.world_mut().get_mut::<HitSplat>(entity).unwrap().anchor = Some(anchor);
        let mut since = None;
        let step = |app: &mut App, dt: f32, since: &mut Option<f32>| {
            app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(dt));
            app.update();
            let now = app.world().resource::<Time>().elapsed_secs();
            let result = crate::gate::watch_fallen(app.world_mut(), Some(7), None, since, now);
            let player = app.world().resource::<LocalPlayer>();
            let transform = GlobalTransform::from(Transform::from_translation(player.eye()).with_rotation(player.rotation()));
            *app.world_mut().get_mut::<GlobalTransform>(camera_entity).unwrap() = transform;
            result
        };
        assert_eq!(step(&mut app, 0.08, &mut since), None);
        assert_eq!(*app.world().get::<Visibility>(entity).unwrap(), Visibility::Hidden, "zero layout receives no visibility credit");
        // Use virtual-time steps no larger than Bevy's 0.25 s default clamp. Deferred
        // layout can consume most of the ordinary lifetime without earning visible time.
        for dt in [0.25, 0.25, 0.15] {
            assert_eq!(step(&mut app, dt, &mut since), None);
        }
        app.world_mut().get_mut::<ComputedNode>(entity).unwrap().size = Vec2::new(40.0, 24.0);
        assert_eq!(step(&mut app, 0.01, &mut since), None);
        assert_eq!(step(&mut app, 0.05, &mut since), None);
        app.world_mut().resource_mut::<UiState>().menu_open = true;
        assert_eq!(step(&mut app, 0.02, &mut since), None);
        assert_eq!(*app.world().get::<Visibility>(entity).unwrap(), Visibility::Hidden);
        assert_eq!(app.world().get::<HitSplat>(entity).unwrap().fallen_drawn_for(), 0.0, "a menu gap resets continuity");
        app.world_mut().resource_mut::<UiState>().menu_open = false;
        assert_eq!(step(&mut app, 0.02, &mut since), None);
        assert_eq!(step(&mut app, 0.02, &mut since), None, "time before the menu must not accumulate");
        let Some(Ok(seen)) = step(&mut app, 0.25, &mut since) else { panic!("the real system must draw the hold's completion frame across ordinary expiry") };
        assert!(seen >= 0.2);
        assert_eq!(*app.world().get::<Visibility>(entity).unwrap(), Visibility::Inherited);
        let children = app.world().get::<Children>(entity).unwrap();
        assert!(children.iter().any(|child| app.world().get::<TextColor>(child).is_some_and(|colour| colour.0.alpha() > 0.0)), "the completion frame must have readable alpha");
    }

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

    const KINDS: &str = r#"blows: { "plain": (color: (0,0,0)), "beat": (color: (0,0,0)), "flourish": (color: (0,0,0)), "counter": (color: (0,0,0)), "bleed": (color: (0,0,0)), "unaware": (color: (0,0,0)) },
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
            ("seconds: 0.9, fallen_seconds: -1, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.fallen_seconds"),
            ("seconds: 0.9, fallen_seconds: inf, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.fallen_seconds"),
            ("seconds: 0.0, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.seconds"),
            ("seconds: 0.9, rise: -1, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.rise"),
            ("seconds: 0.9, rise: 14, fade: 1.0, gap: 1, most: 6, hurt_above: 70", "splats.fade"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: -2, most: 6, hurt_above: 70", "splats.gap"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: 1, most: 0, hurt_above: 70", "splats.most"),
            ("seconds: inf, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.seconds"),
            ("seconds: 0.9, rise: inf, fade: 0.5, gap: 1, most: 6, hurt_above: 70", "splats.rise"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: -1", "splats.hurt_above"),
            ("seconds: 0.9, rise: 14, fade: 0.5, gap: 1, most: 6, hurt_above: inf", "splats.hurt_above"),
        ] {
            let refused = CombatStyle::load(&[file("core", &with(bad, 0.6))]).unwrap_err();
            assert!(refused.contains(says), "{bad}: {refused}");
        }
        assert!(CombatStyle::load(&[file("core", &with(good, 0.0))]).unwrap_err().contains("flash.seconds"));
        let tiny = style_with(&format!("{good}, {}, flash: (color: (0,0,0), seconds: 0.6)", KINDS.replacen("\"plain\": (color: (0,0,0))", "\"plain\": (color: (0,0,0), scale: 1)", 1)));
        assert!(CombatStyle::load(&[file("core", &tiny)]).unwrap_err().contains("plain splat's scale"));
        // The meter's and pips' sizes: nothing, or never ending, is refused too.
        for (from, to, says) in [("width: 24", "width: 0", "meter.width"), ("width: 24", "width: inf", "meter.width"), ("height: 3,", "height: 0,", "meter.height"), ("height: 3,", "height: inf,", "meter.height"), ("size: 3", "size: 0", "pips.size"), ("size: 3", "size: inf", "pips.size")] {
            let refused = CombatStyle::load(&[file("core", &with(good, 0.6).replacen(from, to, 1))]).unwrap_err();
            assert!(refused.contains(says), "{to}: {refused}");
        }
        let big = KINDS.replacen("\"plain\": (color: (0,0,0))", "\"plain\": (color: (0,0,0), scale: 7)", 1);
        let huge = style_with(&format!("{good}, {big}, flash: (color: (0,0,0), seconds: 0.6)"));
        assert!(CombatStyle::load(&[file("core", &huge)]).unwrap_err().contains("plain splat's scale"));
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
