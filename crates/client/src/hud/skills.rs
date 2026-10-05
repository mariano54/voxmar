//! Skills on the HUD, RuneScape-style: each bit of experience earned rises at the right of
//! the screen as "+18 Farming" and fades (gains close together add up into one line), a
//! new level is noted in a small plate above them with the skill's icon, and an action
//! refused for want of a level says what it needs. The character screen lists every
//! skill's level and how far it is to the next.

use bevy::prelude::*;
use voxmar_engine::skill::SkillId;

use crate::lang::Locale;
use crate::menu::Screen;
use crate::session::Launch;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, SHADOW, TEXT, UiFont, shadowed};

/// Seconds a line stays, the last of them fading out as it rises.
const LINE_SECONDS: f32 = 2.5;
const FADE: f32 = 0.6;
/// Gains in the same skill this close together add up into one line.
const MERGE: f32 = 1.2;
/// How far a line rises over its life, in HUD pixels.
const RISE: f32 = 10.0;
/// Seconds a new level is noted for, and how long its plate takes to appear.
const LEVEL_SECONDS: f32 = 4.0;
const APPEAR: f32 = 0.2;
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

#[derive(Component)]
struct ToastLine(usize);
/// The plate noting a new level, and its parts.
#[derive(Component)]
struct LevelPlate;
#[derive(Component)]
struct LevelIcon;
#[derive(Component)]
struct LevelSkill;
#[derive(Component)]
struct LevelReached;

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
    // Just above the experience lines, so the two read as one quiet corner.
    let corner = Node { position_type: PositionType::Absolute, right: Val::Px(8.0 * PX), bottom: Val::Percent(63.0), ..default() };
    let plate = Node {
        align_items: AlignItems::Center,
        column_gap: Val::Px(3.0 * PX),
        padding: UiRect::new(Val::Px(3.0 * PX), Val::Px(4.0 * PX), Val::Px(2.0 * PX), Val::Px(2.0 * PX)),
        ..default()
    };
    commands.spawn((corner, GlobalZIndex(1))).with_children(|corner| {
        corner.spawn((plate, Piece::Tooltip.image(), LevelPlate, Visibility::Hidden)).with_children(|plate| {
            plate.spawn((Node { width: Val::Px(16.0 * PX), height: Val::Px(16.0 * PX), ..default() }, ImageNode::default(), LevelIcon));
            plate.spawn(Node { flex_direction: FlexDirection::Column, ..default() }).with_children(|lines| {
                lines.spawn((shadowed(font, "", BODY, GOLD), LevelSkill));
                lines.spawn((shadowed(font, "", BODY, TEXT), LevelReached));
            });
        });
    });
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn show(
    time: Res<Time>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    images: Option<Res<super::HudImages>>,
    mut toasts: ResMut<SkillToasts>,
    mut lines: Query<(&ToastLine, &mut Text, &mut TextColor, &mut TextShadow, &mut Node, &mut Visibility), Without<LevelPlate>>,
    mut plate: Query<(&mut Visibility, &mut ImageNode), (With<LevelPlate>, Without<LevelIcon>)>,
    mut icon: Query<&mut ImageNode, (With<LevelIcon>, Without<LevelPlate>)>,
    mut skill_text: Query<(&mut Text, &mut TextColor, &mut TextShadow), (With<LevelSkill>, Without<ToastLine>, Without<LevelReached>)>,
    mut reached_text: Query<(&mut Text, &mut TextColor, &mut TextShadow), (With<LevelReached>, Without<ToastLine>, Without<LevelSkill>)>,
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
    let Ok((mut visibility, mut backing)) = plate.single_mut() else { return };
    let Some((skill, reached, at)) = toasts.level.filter(|&(_, _, at)| now - at < LEVEL_SECONDS) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // In quickly, out slowly.
    let since = now - at;
    let alpha = (since / APPEAR).min(1.0) * fade(since, LEVEL_SECONDS);
    backing.color = Color::WHITE.with_alpha(alpha);
    if let Ok(mut icon) = icon.single_mut() {
        if let Some(image) = images.as_ref().and_then(|images| images.skills.get(skill.0 as usize))
            && icon.image != *image
        {
            icon.image = image.clone();
        }
        icon.color = Color::WHITE.with_alpha(alpha);
    }
    let words = [
        (locale.skill(skills.get(skill)), GOLD),
        (locale.format("skills.level_up", &[("level", &reached)]), TEXT),
    ];
    for (query_text, (words, base)) in [skill_text.single_mut().ok(), reached_text.single_mut().ok()].into_iter().zip(words) {
        let Some((mut text, mut color, mut shadow)) = query_text else { continue };
        if text.0 != words {
            text.0 = words;
        }
        color.set_if_neq(TextColor(base.with_alpha(alpha)));
        shadow.color = SHADOW.with_alpha(SHADOW.alpha() * alpha);
    }
    visibility.set_if_neq(Visibility::Inherited);
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
