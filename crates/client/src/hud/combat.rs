//! How a fight shows on screen, from [`crate::fight::Fighting`]: under the crosshair the
//! swing meter, filling over the weapon's cooldown and lighting on the beat, with the
//! rhythm pips beneath it (all lit, the next blow on the beat is a flourish); and
//! hitsplats, the number each blow did rising and fading over the creature it struck, or
//! above the hotbar for those that came at the player, coloured by what kind of blow it
//! was or what came of it (a miss, a block, a perfect block, a dodge).
//!
//! Sizes, colours and timings are the packages' (`combat_hud.ron`, in `core`); a later
//! package changes what it gives.

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::combat::{Blow, Defended};
use voxmar_engine::content::ContentFile;

use crate::creatures::Creatures;
use crate::fight::Fighting;
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::PX;
use crate::ui::widgets::{BODY, UiFont, shadowed};

pub struct CombatHudPlugin;

impl Plugin for CombatHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SplatsShown>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, meter.run_if(in_state(Screen::InGame)))
            .add_systems(PostUpdate, splats.after(TransformSystems::Propagate).run_if(in_state(Screen::InGame)));
    }
}

/// The file a package styles fights' marks in.
pub const FILE: &str = "combat_hud.ron";

/// An sRGB colour, 0 to 255 a channel.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn color(self) -> Color {
        Color::srgb_u8(self.0, self.1, self.2)
    }
}

/// The swing meter under the crosshair: its size and place in interface pixels, its
/// colours, and the seconds it stays after the last blow either way.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterStyle {
    pub width: f32,
    pub height: f32,
    pub below: f32,
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

/// One kind of hitsplat: its colour, and how many times the body text's size it is.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Splat {
    pub color: Rgb,
    #[serde(default = "one")]
    pub size: u8,
}

fn one() -> u8 {
    1
}

/// Hitsplats: the seconds each shows and how far it rises meanwhile, how far apart
/// those on one creature stand, how high over the hotbar the player's own stand, and each
/// kind's look by its name.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplatStyle {
    pub seconds: f32,
    pub rise: f32,
    pub spread: f32,
    pub hurt_above: f32,
    pub blows: std::collections::BTreeMap<String, Splat>,
    pub defended: std::collections::BTreeMap<String, Splat>,
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
    /// Reads every package's file in order, a later one changing what it gives.
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
        Ok(style)
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

/// The words of a splat for a blow at the player.
pub fn hurt_words(locale: &Locale, damage: u16, defended: Defended) -> String {
    locale.format(&format!("combat.splat.defended.{}", defended_name(defended)), &[("damage", &damage)])
}

/// The meter and its pips.
#[derive(Component)]
pub struct MeterRoot;
#[derive(Component)]
struct MeterFill;
#[derive(Component)]
struct MeterBuffer;
#[derive(Component)]
struct MeterTrack;
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
    /// Its place among those shown on the same one, for spreading them.
    slot: u8,
}

/// The hitsplats drawn so far, ever, by what they said: for the gate to check against
/// what the server told.
#[derive(Resource, Default)]
pub struct SplatsShown {
    pub struck: Vec<(u16, Blow, String)>,
    pub hurt: Vec<(u16, Defended, String)>,
    /// Whether the meter has shown in a fight, and the most pips it lit.
    pub meter: bool,
    pub pips: u8,
    struck_seen: u32,
    hurt_seen: u32,
}

fn spawn(mut commands: Commands, launch: Res<Launch>) {
    let style = match CombatStyle::load(&launch.content.files) {
        Ok(style) => style,
        Err(e) => {
            error!("combat marks: {e}");
            return;
        }
    };
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
        let track = Node { width: Val::Px(meter.width * PX), height: Val::Px(meter.height * PX), ..default() };
        column.spawn((track, BackgroundColor(meter.empty.color()), MeterTrack)).with_children(|track| {
            let full = Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() };
            track.spawn((full.clone(), BackgroundColor(meter.buffer.color()), MeterBuffer));
            track.spawn((full, BackgroundColor(meter.fill.color()), MeterFill));
        });
        let row = Node { column_gap: Val::Px(pips.gap * PX), margin: UiRect::top(Val::Px(pips.gap * PX)), ..default() };
        column.spawn(row).with_children(|row| {
            for i in 0..launch.content.combat.feel.pips {
                row.spawn((Node { width: Val::Px(pips.size * PX), height: Val::Px(pips.size * PX), ..default() }, BackgroundColor(pips.off.color()), Pip(i)));
            }
        });
    });
    let layer = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() };
    commands.spawn((layer, SplatLayer, GlobalZIndex(0)));
    commands.insert_resource(style);
}

/// The meter and pips, while a fight is on: a blow either way lately, a swing under way,
/// or pips lit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn meter(
    time: Res<Time>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    style: Option<Res<CombatStyle>>,
    fighting: Res<Fighting>,
    mut shown: ResMut<SplatsShown>,
    mut root: Query<&mut Visibility, With<MeterRoot>>,
    mut fill: Query<(&mut Node, &mut BackgroundColor), (With<MeterFill>, Without<MeterBuffer>)>,
    mut buffer: Query<&mut Node, (With<MeterBuffer>, Without<MeterFill>)>,
    mut pips: Query<(&Pip, &mut BackgroundColor), Without<MeterFill>>,
) {
    let (Some(style), Ok(mut visibility)) = (style, root.single_mut()) else { return };
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

/// Spawns a splat for each blow the server answered since, moves each with what it
/// marks, rises and fades it, and lets it go.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn splats(
    mut commands: Commands,
    time: Res<Time>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    style: Option<Res<CombatStyle>>,
    fighting: Res<Fighting>,
    creatures: Res<Creatures>,
    scale: Res<UiScale>,
    mut shown: ResMut<SplatsShown>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    layer: Query<(Entity, &ComputedNode), With<SplatLayer>>,
    mut marks: Query<(Entity, &HitSplat, &mut Node, &mut TextColor, &mut Visibility)>,
) {
    let (Some(style), Ok((layer, area))) = (style, layer.single()) else { return };
    let now = time.elapsed_secs_f64();
    let splat = &style.splats;
    // New since last time: the tails of what the server said.
    let new_struck = (fighting.struck_count - shown.struck_seen.min(fighting.struck_count)) as usize;
    let new_hurt = (fighting.hurt_count - shown.hurt_seen.min(fighting.hurt_count)) as usize;
    shown.struck_seen = fighting.struck_count;
    shown.hurt_seen = fighting.hurt_count;
    let mut on_each: std::collections::HashMap<Option<u32>, u8> = std::collections::HashMap::new();
    for (_, mark, ..) in &marks {
        *on_each.entry(mark.creature).or_default() += 1;
    }
    let mut add = |commands: &mut Commands, creature: Option<u32>, words: String, look: Splat, at: f64| {
        let slot = on_each.entry(creature).or_default();
        let node = Node { position_type: PositionType::Absolute, ..default() };
        let centred = UiTransform::from_translation(Val2::percent(-50.0, -100.0));
        let mark = HitSplat { creature, at, slot: *slot };
        *slot += 1;
        commands.entity(layer).with_child((node, centred, shadowed(&font.0, words, BODY * look.size.max(1) as f32, look.color.color()), TightLine, mark, Visibility::Hidden));
    };
    for &(creature, damage, blow, at) in fighting.struck.iter().rev().take(new_struck).rev() {
        let words = struck_words(&locale, damage, blow);
        shown.struck.push((damage, blow, words.clone()));
        let look = if damage == 0 && blow != Blow::Bleed { style.defended(Defended::Missed) } else { style.blow(blow) };
        add(&mut commands, Some(creature), words, look, at);
    }
    for &(_, damage, defended, at) in fighting.hurt.iter().rev().take(new_hurt).rev() {
        let words = hurt_words(&locale, damage, defended);
        shown.hurt.push((damage, defended, words.clone()));
        add(&mut commands, None, words, style.defended(defended), at);
    }
    let screen = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open || player.dead;
    let Ok((camera, eye)) = camera.single() else { return };
    let size = area.size() * area.inverse_scale_factor();
    for (entity, mark, mut node, mut color, mut visibility) in &mut marks {
        let age = (now - mark.at) as f32 / splat.seconds.max(1e-3);
        if age >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        // Over the creature's head, or over the hotbar for the player's own.
        let base = match mark.creature {
            Some(id) => creatures.living().find(|(living, ..)| *living == id).and_then(|(_, kind, feet)| {
                let head = feet + Vec3::Y * launch.content.creatures.get(kind).def.body.size.1;
                camera.world_to_viewport(eye, head).ok().map(|at| at / scale.0)
            }),
            None => Some(Vec2::new(size.x / 2.0, size.y - splat.hurt_above * PX)),
        };
        let Some(base) = base.filter(|_| !screen) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        // Spread side to side by its place, so blows close together do not cover each other.
        let side = match mark.slot % 3 {
            0 => 0.0,
            1 => -1.0,
            _ => 1.0,
        };
        let at = base + Vec2::new(side * splat.spread * PX, -age * splat.rise * PX);
        node.left = Val::Px(at.x);
        node.top = Val::Px(at.y);
        // Full for the first half, then fading out.
        let alpha = if age < 0.5 { 1.0 } else { 1.0 - (age - 0.5) * 2.0 };
        let faded = color.0.with_alpha(alpha);
        if color.0 != faded {
            color.0 = faded;
        }
        visibility.set_if_neq(Visibility::Inherited);
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

    #[test]
    fn the_games_style_has_every_kind_and_a_later_package_changes_one() {
        let files = assets();
        let style = CombatStyle::load(&files).unwrap();
        assert!(style.meter.width > 0.0 && style.splats.seconds > 0.0);
        let mut modded = files.clone();
        modded.push(file("mod", r#"(splats: (seconds: 2.0, rise: 30, spread: 8, hurt_above: 64, blows: { "flourish": (color: (1, 2, 3), size: 2) }, defended: {}))"#));
        let changed = CombatStyle::load(&modded).unwrap();
        assert_eq!(changed.blow(Blow::Flourish), Splat { color: Rgb(1, 2, 3), size: 2 });
        assert_eq!(changed.blow(Blow::Plain), style.blow(Blow::Plain), "kinds the mod does not give stay");
        assert_eq!(changed.splats.seconds, 2.0);
        assert_eq!(changed.meter, style.meter);
    }

    #[test]
    fn a_style_missing_a_kind_is_refused() {
        let source = r#"(meter: (width: 24, height: 3, below: 12, empty: (0,0,0), fill: (0,0,0), beat: (0,0,0), buffer: (0,0,0), linger: 3.0),
            pips: (size: 3, gap: 2, off: (0,0,0), on: (0,0,0), ready: (0,0,0)),
            splats: (seconds: 1.0, rise: 10, spread: 8, hurt_above: 60, blows: { "plain": (color: (0,0,0)) }, defended: {}))"#;
        assert!(CombatStyle::load(&[file("core", source)]).is_err());
    }

    /// Every blow and every outcome reads in each language, with its damage where it has
    /// one.
    #[test]
    fn splats_have_words_in_every_language() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for code in ["en", "ja"] {
            let locale = Locale::load(&dir, code);
            for blow in BLOWS {
                let words = struck_words(&locale, 12, blow);
                assert!(words.contains("12"), "{code}: {blow:?} reads {words:?}");
            }
            assert!(!struck_words(&locale, 0, Blow::Plain).contains('0'), "{code}: a miss shows a number");
            for defended in DEFENDED {
                let words = hurt_words(&locale, 7, defended);
                assert!(!words.is_empty() && !words.contains("combat."), "{code}: {defended:?} reads {words:?}");
            }
            assert!(hurt_words(&locale, 7, Defended::Hit).contains('7') && hurt_words(&locale, 7, Defended::Blocked).contains('7'));
        }
    }
}
