//! The HUD's moments: a quest finished and a level reached each get a banner at the top of
//! the screen, a burst of motes around the player and their jingle (`ui::sound`), and a
//! milestone level (every tenth, as data says) a grander one of each. How each looks is
//! every package's `celebrations.ron`, read and checked at launch.
//!
//! Moments take turns: one arriving while another shows waits until that one has held the
//! stage for its `hold`, then cuts in. A quest goes ahead of levels still waiting (a quest
//! finished by reaching a level is the bigger news; its level often arrives first, the
//! server checking quests once a second). Levels reached together in one skill show once,
//! as the highest. Nothing here takes the pointer or the keys: play goes on underneath.

use std::collections::VecDeque;

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::content::ContentFile;
use voxmar_engine::quest::QuestState;
use voxmar_engine::skill::SkillId;

use super::skills::SkillToasts;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::render::particles::{Motion, Particle, Particles};
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::sound::UiSound;
use crate::ui::widgets::{ACCENT, BODY, GOLD, LARGE, SHADOW, TEXT, UiFont, shadowed, space};
use voxmar_engine::block::ParticleBlend;

pub const FILE: &str = "celebrations.ron";
/// Moments waiting at most; past it the last in line is let go.
const WAITING: usize = 4;
/// Seconds the banner takes to grow and settle.
const POP_SECONDS: f32 = 0.35;
/// Most motes in one burst, and in one moment.
const MOST_MOTES: u32 = 256;

pub struct CelebratePlugin;

impl Plugin for CelebratePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stage>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, ((take_levels, take_quests), advance, burst, draw).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), leave);
    }
}

/// How the moments look, from every package's `celebrations.ron`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Celebrations {
    /// Levels this many apart are milestones.
    pub milestone_every: u8,
    pub quest: Look,
    pub level: Look,
    pub milestone: Look,
}

/// One moment's look.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    /// Seconds it holds the stage before a waiting moment may cut in.
    pub hold: f32,
    pub banner: Banner,
    #[serde(default)]
    pub bursts: Vec<Burst>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Banner {
    pub seconds: f32,
    pub fade: f32,
    /// The size it grows to as it pops in, as a multiple of its own (1 to 2).
    pub pop: f32,
    /// Its title glimmers.
    #[serde(default)]
    pub shine: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Burst {
    pub count: u32,
    pub colors: Vec<(u8, u8, u8)>,
    pub ring: f32,
    pub height: f32,
    #[serde(default)]
    pub delay: f32,
    pub out: f32,
    pub up: f32,
    #[serde(default)]
    pub gravity: f32,
    pub drag: f32,
    pub size: f32,
    pub life: f32,
    #[serde(default)]
    pub wander: f32,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CelebrationsFile {
    milestone_every: Option<u8>,
    quest: Option<Look>,
    level: Option<Look>,
    milestone: Option<Look>,
}

impl Celebrations {
    /// Reads every package's file in order, a later one changing what it gives, and refuses
    /// a moment missing or with a number that cannot be.
    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = CelebrationsFile::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let given: CelebrationsFile = options.from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            all.milestone_every = given.milestone_every.or(all.milestone_every);
            all.quest = given.quest.or(all.quest);
            all.level = given.level.or(all.level);
            all.milestone = given.milestone.or(all.milestone);
        }
        let missing = |what: &str| format!("no package's {FILE} gives {what}");
        let celebrations = Celebrations {
            milestone_every: all.milestone_every.ok_or_else(|| missing("milestone_every"))?,
            quest: all.quest.ok_or_else(|| missing("quest"))?,
            level: all.level.ok_or_else(|| missing("level"))?,
            milestone: all.milestone.ok_or_else(|| missing("milestone"))?,
        };
        for (name, look) in [("quest", &celebrations.quest), ("level", &celebrations.level), ("milestone", &celebrations.milestone)] {
            look.check().map_err(|why| format!("{FILE}: {name}: {why}"))?;
        }
        Ok(celebrations)
    }

    /// Whether `level` is a milestone (0 apart: none is).
    pub fn milestone_at(&self, level: u8) -> bool {
        self.milestone_every > 0 && level.is_multiple_of(self.milestone_every)
    }

    fn look(&self, moment: &Moment) -> &Look {
        match moment {
            Moment::Quest(_) => &self.quest,
            Moment::Level(_, level) if self.milestone_at(*level) => &self.milestone,
            Moment::Level(..) => &self.level,
        }
    }
}

impl Look {
    fn check(&self) -> Result<(), String> {
        let banner = &self.banner;
        let fine = |x: f32| x.is_finite() && x >= 0.0;
        if !(fine(self.hold) && self.hold <= banner.seconds) {
            return Err(format!("hold {} is not from 0 to the banner's seconds", self.hold));
        }
        if !(fine(banner.seconds) && banner.seconds > 0.0 && fine(banner.fade) && banner.fade <= banner.seconds) {
            return Err("the banner needs seconds over 0 and a fade no longer than them".into());
        }
        if !(1.0..=2.0).contains(&banner.pop) {
            return Err(format!("the banner's pop {} is not from 1 to 2", banner.pop));
        }
        let mut motes = 0;
        for burst in &self.bursts {
            if burst.colors.is_empty() {
                return Err("a burst has no colours".into());
            }
            if ![burst.ring, burst.delay, burst.out, burst.drag, burst.gravity, burst.wander].into_iter().all(fine) || !(burst.height.is_finite() && burst.up.is_finite()) {
                return Err("a burst's distances, speeds and times must be numbers, and all but height and up 0 or more".into());
            }
            if !(burst.size > 0.0 && burst.size <= 1.0 && burst.life > 0.0 && burst.life <= 10.0 && burst.wander <= 0.63) {
                return Err("a burst's motes need a size over 0 to 1, a life over 0 to 10 and a wander up to 0.63".into());
            }
            motes += burst.count;
        }
        if motes > MOST_MOTES {
            return Err(format!("{motes} motes is more than {MOST_MOTES}"));
        }
        Ok(())
    }
}

/// Something worth marking.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Moment {
    /// A quest, by name, finished.
    Quest(String),
    /// A skill reached a level.
    Level(SkillId, u8),
}

/// The moment showing, since when (seconds), and those waiting their turn.
#[derive(Resource, Default)]
pub struct Stage {
    showing: Option<(Moment, f32)>,
    waiting: VecDeque<Moment>,
    /// Bursts still to come: when, around where, and how.
    bursts: Vec<(f32, Burst)>,
    /// Motes thrown so far (the gate's count).
    pub thrown: u32,
    /// The quest states last seen and their revision, to tell which just turned done;
    /// forgotten on leaving, so another world's states are never compared with this one's.
    quests_seen: (u32, Vec<QuestState>),
}

impl Stage {
    /// A quest, by name, finished (the gate's way in; the server's states are the game's).
    pub fn ask_quest(&mut self, name: String) {
        self.ask(Moment::Quest(name));
    }

    /// The level showing, if a level is what shows.
    pub fn showing_level(&self) -> Option<u8> {
        match self.showing {
            Some((Moment::Level(_, level), _)) => Some(level),
            _ => None,
        }
    }

    /// `moment` asks for its turn: a quest ahead of any level waiting, a level behind all.
    /// A level in a skill already waiting raises that one instead; a quest already waiting,
    /// or what is showing now (a level no higher in that skill, the same quest), is not
    /// asked twice.
    fn ask(&mut self, moment: Moment) {
        let shown = match (&self.showing, &moment) {
            (Some((Moment::Level(a, was), _)), Moment::Level(b, level)) => a == b && level <= was,
            (Some((showing, _)), moment) => showing == moment,
            (None, _) => false,
        };
        if shown {
            return;
        }
        let same = |w: &Moment| match (w, &moment) {
            (Moment::Level(a, _), Moment::Level(b, _)) => a == b,
            (a, b) => a == b,
        };
        match self.waiting.iter_mut().find(|w| same(w)) {
            Some(Moment::Level(_, was)) => {
                if let Moment::Level(_, level) = moment {
                    *was = (*was).max(level);
                }
            }
            Some(_) => {}
            None => {
                let at = match moment {
                    Moment::Quest(_) => self.waiting.iter().position(|w| matches!(w, Moment::Level(..))).unwrap_or(self.waiting.len()),
                    Moment::Level(..) => self.waiting.len(),
                };
                self.waiting.insert(at, moment);
                if self.waiting.len() > WAITING {
                    self.waiting.pop_back();
                }
            }
        }
    }

    /// The moment to start `now`, if one waits and the stage is free: nothing showing, or
    /// what shows has held for its turn.
    fn next(&mut self, now: f32, celebrations: &Celebrations) -> Option<&Moment> {
        let free = self.showing.as_ref().is_none_or(|(moment, at)| now - at >= celebrations.look(moment).hold);
        if !free {
            return None;
        }
        let moment = self.waiting.pop_front()?;
        self.showing = Some((moment, now));
        self.showing.as_ref().map(|(moment, _)| moment)
    }
}

/// Leaving the game clears the stage and takes the banner down with it (`draw` runs only in
/// the game).
fn leave(mut stage: ResMut<Stage>, mut row: Query<&mut Visibility, With<BannerRow>>) {
    *stage = Stage::default();
    for mut visibility in &mut row {
        *visibility = Visibility::Hidden;
    }
}

/// Levels the server said were reached.
fn take_levels(mut toasts: ResMut<SkillToasts>, mut stage: ResMut<Stage>) {
    for (skill, level) in toasts.take_reached() {
        stage.ask(Moment::Level(skill, level));
    }
}

/// Quests the server's states say were just finished (not on joining, when the first
/// states arrive).
fn take_quests(player: Res<LocalPlayer>, mut stage: ResMut<Stage>) {
    if player.quests.is_empty() {
        stage.quests_seen = (player.quests_rev, Vec::new());
        return;
    }
    if stage.quests_seen.0 == player.quests_rev {
        return;
    }
    let done: Vec<String> = finished(&stage.quests_seen.1, &player.quests).into_iter().map(str::to_owned).collect();
    for name in done {
        stage.ask(Moment::Quest(name));
    }
    stage.quests_seen = (player.quests_rev, player.quests.clone());
}

/// The quests done in `now` that were not in `before`; none for the first states of a
/// session (nothing `before`), which are what the player already had.
fn finished<'a>(before: &[QuestState], now: &'a [QuestState]) -> Vec<&'a str> {
    if before.is_empty() {
        return Vec::new();
    }
    now.iter().filter(|s| s.done && before.iter().find(|w| w.name == s.name).is_some_and(|w| !w.done)).map(|s| s.name.as_str()).collect()
}

/// Starts the next moment when its turn comes: its jingle, and its bursts around the
/// player.
fn advance(time: Res<Time>, celebrations: Res<Celebrations>, launch: Res<Launch>, mut stage: ResMut<Stage>, mut sounds: MessageWriter<UiSound>) {
    let now = time.elapsed_secs();
    let Some(moment) = stage.next(now, &celebrations).cloned() else { return };
    sounds.write(jingle(&moment, &celebrations, |name| launch.content.quests.get(name).is_some_and(|q| q.capstone)));
    let bursts: Vec<_> = celebrations.look(&moment).bursts.iter().map(|burst| (now + burst.delay, burst.clone())).collect();
    stage.bursts.extend(bursts);
}

/// A moment's jingle: a path's capstone (`capstone`, by the quest's name) its own, a
/// milestone level its own.
fn jingle(moment: &Moment, celebrations: &Celebrations, capstone: impl Fn(&str) -> bool) -> UiSound {
    match moment {
        Moment::Quest(name) if capstone(name) => UiSound::CapstoneDone,
        Moment::Quest(_) => UiSound::QuestDone,
        Moment::Level(_, level) if celebrations.milestone_at(*level) => UiSound::Milestone,
        Moment::Level(..) => UiSound::LevelUp,
    }
}

/// Bursts whose time has come throw their motes, around where the player is then (a later
/// wave rings them where they have walked to, not where the moment began).
fn burst(time: Res<Time>, player: Res<LocalPlayer>, mut stage: ResMut<Stage>, mut particles: ResMut<Particles>, mut seed: Local<u32>) {
    let now = time.elapsed_secs();
    let around = player.position;
    let share = particles.amount;
    let mut thrown = 0;
    stage.bursts.retain(|(at, burst)| {
        if *at > now {
            return true;
        }
        let count = (burst.count as f32 * share).round() as u32;
        for i in 0..count {
            *seed = seed.wrapping_add(1);
            let random = |k: u32| hash(*seed, k);
            // Spread evenly round the ring, a little at random so no two look alike.
            let turn = (i as f32 + random(0) * 0.6) / count.max(1) as f32 * std::f32::consts::TAU;
            let across = Vec3::new(turn.cos(), 0.0, turn.sin());
            particles.spawn(&mote(burst, around, across, [random(1), random(2), random(3)]));
        }
        thrown += count;
        false
    });
    stage.thrown += thrown;
}

/// One mote of `burst`, thrown along `across` from around `feet`, varied by `random`
/// (three numbers 0..1).
fn mote(burst: &Burst, feet: Vec3, across: Vec3, random: [f32; 3]) -> Particle {
    let (r, g, b) = burst.colors[(random[0] * burst.colors.len() as f32) as usize % burst.colors.len()];
    let falls = burst.gravity > 0.0;
    let lift = 0.75 + 0.5 * random[1];
    Particle {
        position: feet + across * burst.ring + Vec3::Y * (burst.height + 0.3 * (random[2] - 0.5)),
        velocity: across * burst.out * lift + Vec3::Y * burst.up * lift,
        motion: if falls { Motion::Ballistic } else { Motion::Rise },
        gravity: burst.gravity,
        drag: burst.drag,
        floor: if falls { feet.y + burst.size * 0.5 } else { f32::MIN },
        size: burst.size * (0.8 + 0.4 * random[2]),
        grow: 0.5,
        wander: burst.wander,
        life: burst.life * (1.0 + 0.5 * random[1]),
        color: [r, g, b, 255],
        glow: true,
        blend: ParticleBlend::Add,
        ..default()
    }
}

/// A number 0..1 from `seed` and `k`, the same each time.
fn hash(seed: u32, k: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9e37_79b9) ^ k.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x & 0xFF_FFFF) as f32 / 0x100_0000 as f32
}

/// The banner and its parts.
#[derive(Component)]
struct BannerRow;
#[derive(Component)]
struct BannerPlate;
#[derive(Component)]
struct BannerIcon;
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerText {
    /// "Quest Complete", "Milestone Reached": over the title, small.
    Heading,
    /// The quest or the skill.
    Title,
    /// The level, or where to claim the reward.
    Detail,
    /// What the level opens.
    Opens,
}

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    // Centred, under the way home at the top of the screen.
    let row = Node { position_type: PositionType::Absolute, top: Val::Percent(14.0), width: Val::Percent(100.0), justify_content: JustifyContent::Center, ..default() };
    let plate = Node {
        align_items: AlignItems::Center,
        column_gap: Val::Px(space::GAP),
        padding: UiRect::axes(Val::Px(space::OUTER), Val::Px(space::GAP)),
        ..default()
    };
    commands.spawn((row, GlobalZIndex(1), BannerRow, Visibility::Hidden)).with_children(|row| {
        row.spawn((plate, Piece::Tooltip.image(), BannerPlate)).with_children(|plate| {
            plate.spawn((Node { width: Val::Px(24.0 * PX), height: Val::Px(24.0 * PX), ..default() }, ImageNode::default(), BannerIcon));
            plate.spawn(Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() }).with_children(|lines| {
                for (line, size, color) in [(BannerText::Heading, BODY, ACCENT), (BannerText::Title, LARGE, GOLD), (BannerText::Detail, BODY, TEXT), (BannerText::Opens, BODY, ACCENT)] {
                    lines.spawn((Node { max_width: Val::Px(160.0 * PX), ..default() }, shadowed(font, "", size, color), line));
                }
            });
        });
    });
}

/// The words of a moment's banner, by line, and the icon beside them.
fn words(moment: &Moment, celebrations: &Celebrations, locale: &Locale, launch: &Launch, images: Option<&super::HudImages>) -> ([String; 4], Handle<Image>) {
    let content = &launch.content;
    match moment {
        Moment::Quest(name) => {
            let Some(quest) = content.quests.get(name) else { return Default::default() };
            let claim = if quest.pays() { locale.get("celebrate.quest.claim").to_owned() } else { String::new() };
            let icon = images.map(|images| super::quests::quest_icon(content, images, quest)).unwrap_or_default();
            ([locale.get("celebrate.quest").to_owned(), super::quests::title(locale, quest), claim, String::new()], icon)
        }
        Moment::Level(skill, level) => {
            let heading = if celebrations.milestone_at(*level) { locale.get("celebrate.milestone").to_owned() } else { String::new() };
            let opened: Vec<String> = super::guide::entries(locale, launch, *skill).into_iter().filter(|e| e.level == *level).map(|e| e.words).collect();
            let opens = match opened.as_slice() {
                [] => String::new(),
                [one] => locale.format("skills.opens", &[("what", one)]),
                [first, rest @ ..] => locale.format("skills.opens_more", &[("what", first), ("count", &rest.len())]),
            };
            let icon = images.and_then(|images| images.skills.get(skill.0 as usize)).cloned().unwrap_or_default();
            ([heading, locale.skill(content.skills.get(*skill)), locale.format("skills.level_up", &[("level", level)]), opens], icon)
        }
    }
}

/// Whether something is up that the banner must not sit over: a screen (the chat line
/// excepted, at the bottom), death's, or the debug overlay.
fn covered(ui: &UiState, dead: bool, debug: bool) -> bool {
    ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open || dead || debug
}

/// How the banner shows `since` seconds into its moment: its opacity and size.
fn banner_at(banner: &Banner, since: f32) -> (f32, f32) {
    let alpha = (since / 0.12).clamp(0.0, 1.0) * ((banner.seconds - since) / banner.fade.max(1e-3)).clamp(0.0, 1.0);
    // Grows past its size and settles back: up to `pop` by a third of the way, home by the end.
    let t = (since / POP_SECONDS).clamp(0.0, 1.0);
    let scale = if t < 1.0 / 3.0 { 0.7 + (banner.pop - 0.7) * (t * 3.0) } else { banner.pop + (1.0 - banner.pop) * ((t - 1.0 / 3.0) * 1.5) };
    (alpha, scale)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn draw(
    time: Res<Time>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    settings: Res<crate::settings::Settings>,
    celebrations: Res<Celebrations>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    images: Option<Res<super::HudImages>>,
    stage: Res<Stage>,
    mut row: Query<&mut Visibility, With<BannerRow>>,
    mut plate: Query<(&mut ImageNode, &mut UiTransform), (With<BannerPlate>, Without<BannerIcon>)>,
    mut icon: Query<(&mut ImageNode, &mut Node), (With<BannerIcon>, Without<BannerPlate>)>,
    mut lines: Query<(&BannerText, &mut Text, &mut TextColor, &mut TextShadow, &mut Node), Without<BannerIcon>>,
    mut drawn: Local<Option<Moment>>,
) {
    let Ok(mut visibility) = row.single_mut() else { return };
    let now = time.elapsed_secs();
    let showing = stage.showing.as_ref().filter(|(moment, at)| now - at < celebrations.look(moment).banner.seconds);
    // Never over an open screen or the debug overlay: it waits under them, and shows what is
    // left of its time when they close.
    let Some((moment, at)) = showing.filter(|_| !covered(&ui, player.dead, settings.debug_overlay)) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let banner = &celebrations.look(moment).banner;
    let since = now - at;
    let (alpha, scale) = banner_at(banner, since);
    let new = drawn.as_ref() != Some(moment);
    if new {
        *drawn = Some(moment.clone());
    }
    let (texts, image) = if new || locale.is_changed() { words(moment, &celebrations, &locale, &launch, images.as_deref()) } else { Default::default() };
    if let Ok((mut backing, mut transform)) = plate.single_mut() {
        backing.color = Color::WHITE.with_alpha(alpha);
        transform.scale = Vec2::splat(scale);
    }
    if let Ok((mut icon, mut node)) = icon.single_mut() {
        if new {
            let display = if image == Handle::default() { Display::None } else { Display::Flex };
            node.display = display;
            icon.image = image;
        }
        icon.color = Color::WHITE.with_alpha(alpha);
    }
    for (line, mut text, mut color, mut shadow, mut node) in &mut lines {
        if new || locale.is_changed() {
            let words = &texts[*line as usize];
            node.display = if words.is_empty() { Display::None } else { Display::Flex };
            text.0.clone_from(words);
        }
        let base = match line {
            BannerText::Heading | BannerText::Opens => ACCENT,
            // A milestone's title glimmers: gold brightening toward white and back.
            BannerText::Title if banner.shine => Color::from(Srgba::from(GOLD).mix(&Srgba::from(TEXT), 0.5 + 0.5 * (since * 5.0).sin())),
            BannerText::Title => GOLD,
            BannerText::Detail => TEXT,
        };
        color.set_if_neq(TextColor(base.with_alpha(alpha)));
        shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
    }
    visibility.set_if_neq(Visibility::Inherited);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Celebrations {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        Celebrations::load(&content.files).unwrap()
    }

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    /// The shipped moments load, every tenth level is a milestone, and a milestone is the
    /// grander: longer, holding longer, with more motes.
    #[test]
    fn the_moments_load_and_milestones_are_grander() {
        let looks = base();
        assert!(looks.milestone_at(10) && looks.milestone_at(90) && !looks.milestone_at(9) && !looks.milestone_at(11));
        let motes = |look: &Look| look.bursts.iter().map(|b| b.count).sum::<u32>();
        let (level, milestone) = (&looks.level, &looks.milestone);
        assert!(milestone.banner.seconds > level.banner.seconds && milestone.hold > level.hold && motes(milestone) > motes(level));
        assert!(motes(&looks.quest) > 0 && motes(level) > 0);
    }

    /// A later package changes a moment whole and leaves the rest; a bad number, or a
    /// moment no package gives, fails the launch.
    #[test]
    fn a_later_package_changes_a_moment_and_bad_ones_fail() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let mut files: Vec<_> = content.files.iter().filter(|f| f.name == FILE).cloned().collect();
        files.push(file("mod", "(milestone_every: 5, quest: (hold: 0.5, banner: (seconds: 1.0, fade: 0.5, pop: 1.0)))"));
        let looks = Celebrations::load(&files).unwrap();
        assert!(looks.milestone_at(15) && looks.quest.bursts.is_empty() && looks.level == base().level);
        for bad in [
            "(quest: (hold: 2.0, banner: (seconds: 1.0, fade: 0.5, pop: 1.0)))",
            "(quest: (hold: 0.5, banner: (seconds: 1.0, fade: 0.5, pop: 3.0)))",
            "(quest: (hold: 0.5, banner: (seconds: 1.0, fade: 0.5, pop: 1.0), bursts: [(count: 9, colors: [], ring: 0, height: 0, out: 0, up: 1, drag: 0, size: 0.1, life: 1)]))",
            "(quest: (hold: 0.5, banner: (seconds: 1.0, fade: 0.5, pop: 1.0), bursts: [(count: 999, colors: [(1, 2, 3)], ring: 0, height: 0, out: 0, up: 1, drag: 0, size: 0.1, life: 1)]))",
        ] {
            let mut files = files.clone();
            files.push(file("bad", bad));
            assert!(Celebrations::load(&files).is_err(), "{bad}");
        }
        assert!(Celebrations::load(&[file("mod", "(milestone_every: 10)")]).is_err());
    }

    /// A path's capstone (the base game's, such as a homestead's lit hearth) rings its own
    /// jingle; any other quest a quest's, and levels theirs.
    #[test]
    fn a_capstone_rings_its_own_jingle() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let capstone = |name: &str| content.quests.get(name).is_some_and(|q| q.capstone);
        let looks = base();
        let rite = content.quests.iter().find(|q| q.capstone).expect("a capstone");
        let plain = content.quests.iter().find(|q| !q.capstone).expect("a quest");
        assert_eq!(jingle(&Moment::Quest(rite.name.clone()), &looks, capstone), UiSound::CapstoneDone, "{}", rite.name);
        assert_eq!(jingle(&Moment::Quest(plain.name.clone()), &looks, capstone), UiSound::QuestDone, "{}", plain.name);
        assert_eq!(jingle(&Moment::Level(SkillId(2), looks.milestone_every), &looks, capstone), UiSound::Milestone);
        assert_eq!(jingle(&Moment::Level(SkillId(2), looks.milestone_every + 1), &looks, capstone), UiSound::LevelUp);
    }

    /// A quest finished by reaching a level shows first, even when the level was told first;
    /// the level cuts in once the quest has held its turn, and nothing more waits after.
    #[test]
    fn moments_take_turns() {
        let looks = base();
        let mut stage = Stage::default();
        stage.ask(Moment::Level(SkillId(2), 5));
        stage.ask(Moment::Quest("first_fire".into()));
        assert_eq!(stage.next(0.0, &looks), Some(&Moment::Quest("first_fire".into())));
        assert_eq!(stage.next(looks.quest.hold - 0.01, &looks), None);
        assert_eq!(stage.next(looks.quest.hold, &looks), Some(&Moment::Level(SkillId(2), 5)));
        assert_eq!(stage.next(100.0, &looks), None);
        // The level showing, told again (or lower), does not come round a second time.
        stage.ask(Moment::Level(SkillId(2), 5));
        stage.ask(Moment::Level(SkillId(2), 4));
        assert!(stage.waiting.is_empty());
        stage.ask(Moment::Level(SkillId(2), 6));
        assert_eq!(stage.waiting, [Moment::Level(SkillId(2), 6)]);
    }

    /// Leaving the game takes a showing banner down and forgets what waited.
    #[test]
    fn leaving_takes_the_banner_down() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut stage = Stage::default();
        stage.ask(Moment::Quest("a".into()));
        world.insert_resource(stage);
        let row = world.spawn((BannerRow, Visibility::Inherited)).id();
        world.run_system_once(leave).unwrap();
        assert_eq!(world.get::<Visibility>(row), Some(&Visibility::Hidden));
        assert!(world.resource::<Stage>().waiting.is_empty());
    }

    /// A quest finished in play is a moment; after leaving, another world whose first states
    /// arrive with that quest done (before the game shows a frame) replays nothing.
    #[test]
    fn another_world_replays_no_quests() {
        use bevy::ecs::system::RunSystemOnce;
        let state = |done| vec![QuestState { name: "a".into(), done, claimed: false, tasks: Vec::new() }];
        let mut world = World::new();
        world.insert_resource(Stage::default());
        let mut player = LocalPlayer::default();
        (player.quests, player.quests_rev) = (state(false), 1);
        world.insert_resource(player);
        world.run_system_once(take_quests).unwrap();
        assert!(world.resource::<Stage>().waiting.is_empty(), "joining is no moment");
        (world.resource_mut::<LocalPlayer>().quests, world.resource_mut::<LocalPlayer>().quests_rev) = (state(true), 2);
        world.run_system_once(take_quests).unwrap();
        assert_eq!(world.resource::<Stage>().waiting, [Moment::Quest("a".into())]);
        world.spawn((BannerRow, Visibility::Inherited));
        world.run_system_once(leave).unwrap();
        // Another save: its first states already hold the quest done.
        let mut player = LocalPlayer::default();
        (player.quests, player.quests_rev) = (state(true), 1);
        world.insert_resource(player);
        world.run_system_once(take_quests).unwrap();
        assert!(world.resource::<Stage>().waiting.is_empty(), "another world's quest done replayed");
    }

    /// The banner keeps off every screen and the debug overlay; play and the chat line
    /// leave it up.
    #[test]
    fn the_banner_keeps_off_open_screens() {
        assert!(!covered(&UiState::default(), false, false));
        assert!(!covered(&UiState { console_open: true, ..default() }, false, false));
        for ui in [
            UiState { inventory_open: true, ..default() },
            UiState { character_open: true, ..default() },
            UiState { journal_open: true, ..default() },
            UiState { quests_open: true, ..default() },
            UiState { menu_open: true, ..default() },
            UiState { recipes_open: true, ..default() },
        ] {
            assert!(covered(&ui, false, false));
        }
        assert!(covered(&UiState::default(), true, false), "dead");
        assert!(covered(&UiState::default(), false, true), "debug overlay");
    }

    /// Levels in one skill reached together show once, as the highest; other skills and a
    /// quest asked twice keep their own single turn, the quest ahead.
    #[test]
    fn levels_together_show_once() {
        let mut stage = Stage::default();
        for moment in [Moment::Level(SkillId(1), 4), Moment::Level(SkillId(1), 6), Moment::Level(SkillId(3), 2), Moment::Level(SkillId(1), 5), Moment::Quest("a".into()), Moment::Quest("a".into())] {
            stage.ask(moment);
        }
        assert_eq!(stage.waiting, [Moment::Quest("a".into()), Moment::Level(SkillId(1), 6), Moment::Level(SkillId(3), 2)]);
    }

    /// Only quests that turn done between two states are moments, and none on joining.
    #[test]
    fn only_quests_just_finished_are_moments() {
        let state = |name: &str, done| QuestState { name: name.into(), done, claimed: false, tasks: Vec::new() };
        let before = [state("a", false), state("b", true), state("c", false)];
        let now = [state("a", true), state("b", true), state("c", false)];
        assert_eq!(finished(&before, &now), ["a"]);
        assert!(finished(&[], &now).is_empty());
    }

    /// The banner pops past its size and settles, fades in quickly and out over its fade.
    #[test]
    fn the_banner_pops_and_fades() {
        let banner = Banner { seconds: 3.0, fade: 0.5, pop: 1.2, shine: false };
        let (start, scale_start) = banner_at(&banner, 0.0);
        assert!(start == 0.0 && scale_start < 1.0);
        assert!((banner_at(&banner, POP_SECONDS / 3.0).1 - 1.2).abs() < 1e-4);
        assert_eq!(banner_at(&banner, 1.0), (1.0, 1.0));
        assert!((banner_at(&banner, 2.75).0 - 0.5).abs() < 1e-4);
        assert_eq!(banner_at(&banner, 3.0).0, 0.0);
    }

    /// Motes start around the player and fly as their burst says: sparks arc and land at the
    /// feet, rising motes drift up; colours come from the burst's.
    #[test]
    fn motes_follow_their_burst() {
        let looks = base();
        let feet = Vec3::new(5.0, 64.0, -3.0);
        for burst in looks.milestone.bursts.iter() {
            let mote = mote(burst, feet, Vec3::X, [0.99, 0.5, 0.5]);
            assert!((mote.position.x - feet.x - burst.ring).abs() < 1e-4);
            assert!(mote.velocity.y > 0.0 && mote.glow);
            assert_eq!(mote.motion == Motion::Ballistic, burst.gravity > 0.0);
            let (r, g, b) = *burst.colors.last().unwrap();
            assert_eq!(mote.color, [r, g, b, 255]);
        }
    }
}
