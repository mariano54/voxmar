//! Skills on the HUD, RuneScape-style: each bit of experience earned rises at the right of
//! the screen as "+18 Farming" and fades (gains close together add up into one line), a
//! new level is announced in the middle of the screen, and an action refused for want of
//! a level says what it needs. The inventory screen lists every skill's level and how far
//! it is to the next.

use bevy::prelude::*;
use voxmar_engine::skill::{SkillId, SkillRegistry};

use crate::lang::Locale;
use crate::menu::Screen;
use crate::session::Launch;
use crate::ui::skin::PX;
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, LARGE, SHADOW, UiFont, shadowed};

/// Seconds a line stays, the last of them fading out as it rises.
const LINE_SECONDS: f32 = 2.5;
const FADE: f32 = 0.6;
/// Gains in the same skill this close together add up into one line.
const MERGE: f32 = 1.2;
/// How far a line rises over its life, in HUD pixels.
const RISE: f32 = 10.0;
/// Seconds a new level is announced for.
const LEVEL_SECONDS: f32 = 3.5;
/// Lines shown at once at most.
const LINES: usize = 6;

pub struct SkillHudPlugin;

impl Plugin for SkillHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillToasts>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
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
    /// Something refused, and why.
    Refused(String),
}

/// What the skill HUD has to show.
#[derive(Resource, Default)]
pub struct SkillToasts {
    lines: Vec<Line>,
    /// A level just reached, and when.
    level: Option<(SkillId, u8, f32)>,
}

impl SkillToasts {
    /// `tenths` of experience earned in `skill` at `now` seconds.
    pub fn gained(&mut self, skill: SkillId, tenths: u32, now: f32) {
        if let Some(line) = self.lines.iter_mut().rev().find(|l| now - l.at < MERGE && matches!(l.kind, LineKind::Gain { skill: s, .. } if s == skill)) {
            if let LineKind::Gain { tenths: total, .. } = &mut line.kind {
                *total += tenths;
            }
            line.at = now;
            return;
        }
        self.push(Line { kind: LineKind::Gain { skill, tenths }, at: now });
    }

    /// `skill` reached `level` at `now`.
    pub fn level_up(&mut self, skill: SkillId, level: u8, now: f32) {
        self.level = Some((skill, level, now));
    }

    /// An action refused, with why, at `now`; the same reason again only refreshes it.
    pub fn refused(&mut self, why: String, now: f32) {
        if let Some(line) = self.lines.iter_mut().find(|l| l.kind == LineKind::Refused(why.clone())) {
            line.at = now;
            return;
        }
        self.push(Line { kind: LineKind::Refused(why), at: now });
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > LINES {
            self.lines.remove(0);
        }
    }
}

/// Experience as players read it: whole where it is, a decimal where not (12.5).
pub fn xp_text(tenths: u32) -> String {
    if tenths.is_multiple_of(10) { format!("{}", tenths / 10) } else { format!("{}.{}", tenths / 10, tenths % 10) }
}

/// A skill's level and progress to the next, for the inventory screen: "Farming 12 (34%)".
pub fn summary(locale: &Locale, skills: &SkillRegistry, skill: SkillId, xp: u32) -> String {
    let level = skills.level(xp);
    let title = locale.skill(skills.get(skill));
    if level >= skills.max_level() {
        format!("{title} {level}")
    } else {
        format!("{title} {level} ({:.0}%)", skills.progress(xp) * 100.0)
    }
}

#[derive(Component)]
struct ToastLine(usize);
#[derive(Component)]
struct LevelText;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let column = Node {
        position_type: PositionType::Absolute,
        right: Val::Px(8.0 * PX),
        top: Val::Percent(38.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::FlexEnd,
        row_gap: Val::Px(2.0 * PX),
        ..default()
    };
    commands.spawn((column, GlobalZIndex(1))).with_children(|column| {
        for i in 0..LINES {
            column.spawn((Node::default(), shadowed(font, "", BODY, ACCENT), ToastLine(i), Visibility::Hidden));
        }
    });
    let level = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), top: Val::Percent(24.0), justify_content: JustifyContent::Center, ..default() };
    commands.spawn((level, GlobalZIndex(1))).with_child((shadowed(font, "", LARGE, GOLD), LevelText, Visibility::Hidden));
}

#[allow(clippy::type_complexity)]
fn show(
    time: Res<Time>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    mut toasts: ResMut<SkillToasts>,
    mut lines: Query<(&ToastLine, &mut Text, &mut TextColor, &mut TextShadow, &mut Node, &mut Visibility), Without<LevelText>>,
    mut level: Query<(&mut Text, &mut TextColor, &mut TextShadow, &mut Visibility), With<LevelText>>,
) {
    let now = time.elapsed_secs();
    let skills = &launch.content.skills;
    toasts.lines.retain(|line| now - line.at < LINE_SECONDS);
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
            LineKind::Refused(why) => (why.clone(), ERROR),
        };
        if text.0 != words {
            text.0 = words;
        }
        let since = now - line.at;
        let alpha = fade(since, LINE_SECONDS);
        color.set_if_neq(TextColor(base.with_alpha(alpha)));
        shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
        node.top = Val::Px(-RISE * PX * (since / LINE_SECONDS));
        visibility.set_if_neq(Visibility::Inherited);
    }
    if let Ok((mut text, mut color, mut shadow, mut visibility)) = level.single_mut() {
        match toasts.level.filter(|&(_, _, at)| now - at < LEVEL_SECONDS) {
            Some((skill, reached, at)) => {
                let words = locale.format("skills.level_up", &[("skill", &locale.skill(skills.get(skill))), ("level", &reached)]);
                if text.0 != words {
                    text.0 = words;
                }
                let alpha = fade(now - at, LEVEL_SECONDS);
                color.set_if_neq(TextColor(GOLD.with_alpha(alpha)));
                shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
