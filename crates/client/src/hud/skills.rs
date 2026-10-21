//! Skills on the HUD, RuneScape-style: each bit of experience earned rises at the right of
//! the screen as "+18 Farming" and fades (gains close together add up into one line), a
//! new level is handed to the HUD's moments (`celebrate`: a banner, motes and a jingle),
//! and an action refused for want of a level says what it needs (all of them waiting under the quest book
//! and the journal while one is open). The character screen lists every
//! skill's level and how far it is to the next.

use bevy::prelude::*;
use voxmar_engine::skill::SkillId;

use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::UiState;
use crate::session::Launch;
use crate::ui::skin::PX;
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, SHADOW, UiFont, shadowed};
use crate::ui::widgets::space;

/// Seconds a line stays, the last of them fading out as it rises.
const LINE_SECONDS: f32 = 2.5;
/// How long news of something finished elsewhere stays: long enough to notice mid-task.
const NEWS_SECONDS: f32 = 7.0;
const FADE: f32 = 0.6;
/// Gains in the same skill this close together add up into one line.
const MERGE: f32 = 1.2;
/// How far a line rises over its life, in HUD pixels.
const RISE: f32 = 10.0;
/// Lines shown at once at most: one more than the news the server keeps for a player away.
const LINES: usize = voxmar_protocol::NEWS_LINES;
/// How long a refusal's reason shows under the crosshair, and how far below its middle
/// (of the screen's height): close enough to read without looking away from the block.
const WHY_SECONDS: f32 = 2.5;
const WHY_BELOW: f32 = 6.0;

pub struct SkillHudPlugin;

impl Plugin for SkillHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillToasts>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (show, move_aside).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut toasts: ResMut<SkillToasts>| *toasts = SkillToasts::default());
    }
}

/// A line at the right of the screen.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    kind: LineKind,
    at: f32,
}

#[derive(Clone, Debug, PartialEq)]
enum LineKind {
    /// Experience gained, in tenths.
    Gain { skill: SkillId, tenths: u32 },
    /// Something the player set going finished elsewhere (`ServerMessage::Fired`).
    News { text: String },
}

impl LineKind {
    /// How long the line shows.
    fn life(&self) -> f32 {
        match self {
            LineKind::Gain { .. } => LINE_SECONDS,
            LineKind::News { .. } => NEWS_SECONDS,
        }
    }
}

/// What the skill HUD has to show.
#[derive(Resource, Default)]
pub struct SkillToasts {
    lines: Vec<Line>,
    /// Levels reached that the HUD's moments have not yet taken (`celebrate`).
    reached: Vec<(SkillId, u8)>,
    /// Why the last action was refused, and when, shown under the crosshair.
    why: Option<(String, f32)>,
    /// The clock of the last `show`, so a refusal is showing only while it would be drawn.
    now: f32,
    /// When news came that the interface has not yet sounded.
    fired: Vec<f32>,
}

impl SkillToasts {
    /// `tenths` of experience earned in `skill` at `now` seconds.
    pub fn gained(&mut self, skill: SkillId, tenths: u32, now: f32) {
        if let Some(line) = self.lines.iter_mut().rev().find(|l| now - l.at < MERGE && matches!(l.kind, LineKind::Gain { skill: s, .. } if s == skill)) {
            let LineKind::Gain { tenths: total, .. } = &mut line.kind else { unreachable!("found as a gain") };
            *total += tenths;
            line.at = now;
            return;
        }
        self.push(Line { kind: LineKind::Gain { skill, tenths }, at: now });
    }

    /// A gain line for `skill` that reads exactly `tenths`, kept fresh at `now`: what a view
    /// shown frame after frame (the text-fit tour) holds still, where `gained` would add up.
    pub fn showing_gain(&mut self, skill: SkillId, tenths: u32, now: f32) {
        self.gained(skill, 0, now);
        if let Some(LineKind::Gain { tenths: total, .. }) = self.lines.iter_mut().rev().find(|l| matches!(l.kind, LineKind::Gain { skill: s, .. } if s == skill)).map(|l| &mut l.kind) {
            *total = tenths;
        }
    }

    /// News at `now` of something the player set going, finished elsewhere.
    pub fn news(&mut self, text: String, now: f32) {
        self.fired.push(now);
        self.push(Line { kind: LineKind::News { text }, at: now });
    }

    /// The news lines showing, newest last.
    #[cfg(test)]
    pub fn news_showing(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().filter(|l| self.now - l.at < l.kind.life()).filter_map(|l| if let LineKind::News { text } = &l.kind { Some(text.as_str()) } else { None })
    }

    /// Whether news came since last asked (`take_fired`).
    pub fn has_fired(&self) -> bool {
        !self.fired.is_empty()
    }

    /// When news came since last asked, to hear it by (`ui::sound`).
    pub fn take_fired(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.fired)
    }

    /// `skill` reached `level`.
    pub fn level_up(&mut self, skill: SkillId, level: u8) {
        self.reached.push((skill, level));
    }

    /// The levels reached since last asked.
    pub fn take_reached(&mut self) -> Vec<(SkillId, u8)> {
        std::mem::take(&mut self.reached)
    }

    /// Whether a refusal saying `why` is showing.
    pub fn shows_refusal(&self, why: &str) -> bool {
        self.refusals().any(|w| w == why)
    }

    /// The refusals showing, each saying why: the last one, under the crosshair.
    pub fn refusals(&self) -> impl Iterator<Item = &str> {
        self.why.iter().filter(|(_, at)| self.now - at < WHY_SECONDS).map(|(why, _)| why.as_str())
    }

    /// When the newest refusal showing was last given, to hear it by (`ui::sound`).
    pub fn newest_refusal(&self) -> Option<f32> {
        self.why.as_ref().filter(|(_, at)| self.now - at < WHY_SECONDS).map(|&(_, at)| at)
    }

    /// An action refused, with why, at `now`: it shows under the crosshair in place of the
    /// last; the same reason again only refreshes it.
    pub fn refused(&mut self, why: String, now: f32) {
        self.now = self.now.max(now);
        self.why = Some((why, now));
    }

    /// A line, and room for it: a line already gone goes first, then the oldest gain, and
    /// news only when there is nothing else, so news kept for a player away shows its life.
    fn push(&mut self, line: Line) {
        let now = self.now.max(line.at);
        self.lines.push(line);
        if self.lines.len() > LINES {
            let gone = self.lines.iter().position(|l| now - l.at >= l.kind.life());
            let gain = || self.lines.iter().position(|l| matches!(l.kind, LineKind::Gain { .. }));
            self.lines.remove(gone.or_else(gain).unwrap_or(0));
        }
    }
}

/// Experience as players read it: whole where it is, a decimal where not (12.5).
pub fn xp_text(tenths: u32) -> String {
    if tenths.is_multiple_of(10) { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) }
}

#[derive(Component)]
struct ToastLine(usize);
/// Why the last action was refused, under the crosshair.
#[derive(Component)]
pub struct WhyLine;

/// The toasts' place, at the right of the screen, or its left while
/// creative mode's catalogue fills the right.
#[derive(Component)]
struct Corner;

fn move_aside(ui: Res<UiState>, catalogue: Query<&Visibility, (With<super::creative::CataloguePanel>, Without<Corner>)>, mut corners: Query<(&mut Node, &mut Visibility), With<Corner>>) {
    let aside = catalogue.iter().any(|v| *v != Visibility::Hidden);
    let (left, right, align) = if aside { (Val::Px(space::SECTION), Val::Auto, AlignItems::FlexStart) } else { (Val::Auto, Val::Px(space::SECTION), AlignItems::FlexEnd) };
    // The quest book and the journal fill the screen: notes that came while one is open wait
    // under it, rather than being written over its pages.
    let shown = if ui.quests_open || ui.journal_open { Visibility::Hidden } else { Visibility::Inherited };
    for (mut node, mut visibility) in &mut corners {
        if node.right != right {
            node.left = left;
            node.right = right;
            node.align_items = align;
        }
        visibility.set_if_neq(shown);
    }
}

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let column = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(space::SECTION),
        top: Val::Percent(38.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::FlexEnd,
        row_gap: Val::Px(space::HAIR),
        ..default()
    };
    commands.spawn((column, GlobalZIndex(1), Corner)).with_children(|column| {
        for i in 0..LINES {
            column.spawn((Node::default(), shadowed(font, "", BODY, ACCENT), ToastLine(i), Visibility::Hidden));
        }
    });
    // Under the crosshair, where the player is looking: why a click did nothing.
    let under = Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        top: Val::Percent(50.0 + WHY_BELOW),
        justify_content: JustifyContent::Center,
        ..default()
    };
    commands.spawn((under, GlobalZIndex(1))).with_children(|under| {
        let line = Node { max_width: Val::Percent(60.0), ..default() };
        under.spawn((line, shadowed(font, "", BODY, ERROR), TextLayout::default().with_justify(Justify::Center), WhyLine, Visibility::Hidden));
    });
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn show(
    time: Res<Time>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    mut toasts: ResMut<SkillToasts>,
    mut lines: Query<(&ToastLine, &mut Text, &mut TextColor, &mut TextShadow, &mut Node, &mut Visibility)>,
    mut why_line: Query<(&mut Text, &mut TextColor, &mut TextShadow, &mut Visibility), (With<WhyLine>, Without<ToastLine>)>,
    ui: Res<UiState>,
    player: Res<crate::player::LocalPlayer>,
) {
    let now = time.elapsed_secs();
    let skills = &launch.content.skills;
    toasts.now = now;
    toasts.lines.retain(|line| now - line.at < line.kind.life());
    if let Ok((mut text, mut color, mut shadow, mut visibility)) = why_line.single_mut() {
        // Why a click in the world did nothing: not over a screen, which hides the world.
        let covered = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.recipes_open || player.dead;
        match toasts.why.as_ref().filter(|(_, at)| now - at < WHY_SECONDS && !covered) {
            Some((why, at)) => {
                if text.0 != *why {
                    text.0 = why.clone();
                }
                let alpha = ((WHY_SECONDS - (now - at)) / FADE).clamp(0.0, 1.0);
                color.set_if_neq(TextColor(ERROR.with_alpha(alpha)));
                shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    let fade = |since: f32, life: f32| ((life - since) / FADE).clamp(0.0, 1.0);
    for (ToastLine(i), mut text, mut color, mut shadow, mut node, mut visibility) in &mut lines {
        let Some(line) = toasts.lines.get(*i) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let (words, base) = match &line.kind {
            LineKind::Gain { skill, tenths } => {
                (locale.format("skills.gained", &[("xp", &xp_text(*tenths)), ("skill", &locale.skill(skills.get(*skill)))]), ACCENT)
            }
            LineKind::News { text } => (text.clone(), GOLD),
        };
        if text.0 != words {
            text.0 = words;
        }
        let (since, life) = (now - line.at, line.kind.life());
        let alpha = fade(since, life);
        color.set_if_neq(TextColor(base.with_alpha(alpha)));
        shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
        node.top = Val::Px(-RISE * PX * (since / life));
        visibility.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// News of a firing finished elsewhere stays longer than a gain, and is heard once.
    #[test]
    fn news_outlasts_gains_and_is_heard_once() {
        let mut toasts = SkillToasts::default();
        toasts.gained(SkillId(0), 10, 1.0);
        toasts.news("Your Charcoal Clamp is done".into(), 1.0);
        assert_eq!(toasts.take_fired().len(), 1);
        assert!(toasts.take_fired().is_empty(), "heard once");
        toasts.now = 1.0 + LINE_SECONDS + 0.1;
        toasts.lines.retain(|line| toasts.now - line.at < line.kind.life());
        assert_eq!((toasts.lines.len(), toasts.news_showing().collect::<Vec<_>>()), (1, vec!["Your Charcoal Clamp is done"]), "the gain gone, the news still there");
        toasts.now = 1.0 + NEWS_SECONDS + 0.1;
        assert_eq!(toasts.news_showing().count(), 0);
    }

    /// Gains coming on top of a full screen of news push out other gains, never the news,
    /// and news pushes out news only when nothing else is left.
    #[test]
    fn gains_make_room_among_themselves_not_from_news() {
        let mut toasts = SkillToasts::default();
        for i in 0..LINES - 1 {
            toasts.news(format!("news {i}"), 1.0);
        }
        for skill in 0..3 {
            toasts.gained(SkillId(skill), 10, 1.5);
        }
        toasts.now = 1.6;
        assert_eq!(toasts.news_showing().count(), LINES - 1, "all the news kept");
        assert_eq!(toasts.lines.len(), LINES);
        toasts.news("news 5".into(), 1.7);
        toasts.news("news 6".into(), 1.7);
        assert_eq!(toasts.news_showing().collect::<Vec<_>>(), ["news 1", "news 2", "news 3", "news 4", "news 5", "news 6"], "news alone: the oldest goes");
    }

    #[test]
    fn gains_close_together_add_up() {
        let mut toasts = SkillToasts::default();
        toasts.gained(SkillId(0), 180, 1.0);
        toasts.gained(SkillId(0), 60, 1.5);
        toasts.gained(SkillId(1), 10, 1.6);
        toasts.gained(SkillId(0), 60, 4.0);
        let gains: Vec<_> = toasts.lines.iter().map(|l| l.kind.clone()).collect();
        assert_eq!(
            gains,
            vec![
                LineKind::Gain { skill: SkillId(0), tenths: 240 },
                LineKind::Gain { skill: SkillId(1), tenths: 10 },
                LineKind::Gain { skill: SkillId(0), tenths: 60 },
            ]
        );
        assert_eq!((xp_text(240), xp_text(125)), ("24".to_owned(), "12.5".to_owned()));
    }

    /// A gain shown frame after frame reads the same each frame, kept fresh, where gains
    /// would add up.
    #[test]
    fn a_gain_shown_each_frame_holds_its_total() {
        let mut toasts = SkillToasts::default();
        toasts.gained(SkillId(1), 10, 0.5);
        for frame in 0..30 {
            toasts.showing_gain(SkillId(0), 12_345, 1.0 + frame as f32 / 60.0);
        }
        let gains: Vec<_> = toasts.lines.iter().map(|l| (l.kind.clone(), l.at)).collect();
        assert_eq!(gains, vec![(LineKind::Gain { skill: SkillId(1), tenths: 10 }, 0.5), (LineKind::Gain { skill: SkillId(0), tenths: 12_345 }, 1.0 + 29.0 / 60.0)]);
    }
}
