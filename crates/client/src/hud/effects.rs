//! What an effect is, on hover: over its chip on the HUD (with a screen open, so the
//! pointer is free) or its line in the character screen, a small plate beside the pointer
//! gives its name and what it does.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;
use voxmar_engine::effect::{EffectId, Kind};

use crate::lang::Locale;
use crate::menu::Screen;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, SOFT, UiFont, shadowed, space, text};

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn).add_systems(Update, show.run_if(in_state(Screen::InGame)));
    }
}

/// Something that shows an effect, to say what it is on hover.
#[derive(Component, Clone, Copy)]
pub struct EffectHover(pub EffectId);

#[derive(Component)]
struct EffectTip;
#[derive(Component)]
struct EffectTipName;
#[derive(Component)]
struct EffectTipWhat;

/// How far the plate keeps from the pointer, and how wide its words may run.
const OFFSET: f32 = 8.0 * PX;
const WIDTH: f32 = 120.0 * PX;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let tip = Node {
        position_type: PositionType::Absolute,
        max_width: Val::Px(WIDTH),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(space::HAIR),
        padding: UiRect::all(Val::Px(space::GAP)),
        ..default()
    };
    commands.spawn((EffectTip, tip, Piece::Tooltip.image(), Visibility::Hidden, GlobalZIndex(6))).with_children(|tip| {
        tip.spawn((shadowed(font, "", BODY, ACCENT), EffectTipName));
        tip.spawn((text(font, "", BODY, SOFT), EffectTipWhat));
    });
}

#[allow(clippy::type_complexity)]
fn show(
    locale: Res<Locale>,
    launch: Res<crate::session::Launch>,
    window: Query<&Window, With<PrimaryWindow>>,
    hovered: Query<(&EffectHover, &RelativeCursorPosition, &InheritedVisibility)>,
    mut tip: Query<(&mut Node, &mut Visibility), With<EffectTip>>,
    mut name: Query<(&mut Text, &mut TextColor), (With<EffectTipName>, Without<EffectTipWhat>)>,
    mut what: Query<&mut Text, (With<EffectTipWhat>, Without<EffectTipName>)>,
    scale: Res<UiScale>,
) {
    let Ok((mut node, mut visibility)) = tip.single_mut() else { return };
    let over = hovered.iter().find(|(_, cursor, visible)| visible.get() && cursor.cursor_over()).map(|(effect, _, _)| effect.0);
    let at = window.single().ok().and_then(Window::cursor_position);
    let (Some(effect), Some(at)) = (over.map(|id| launch.content.effects.get(id)), at) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // The pointer is in the window's points; the interface is laid out at its own scale.
    let at = at / scale.0;
    node.left = Val::Px(at.x + OFFSET);
    node.top = Val::Px(at.y + OFFSET);
    if let Ok((mut text, mut color)) = name.single_mut() {
        let words = locale.effect(effect);
        if text.0 != words {
            text.0 = words;
        }
        color.set_if_neq(TextColor(if effect.kind == Kind::Buff { ACCENT } else { ERROR }));
    }
    if let Ok(mut text) = what.single_mut() {
        let words = locale.effect_description(effect);
        if text.0 != words {
            text.0 = words;
        }
    }
    visibility.set_if_neq(Visibility::Inherited);
}
