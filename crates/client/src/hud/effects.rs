//! What an effect is, on hover: over its chip on the HUD (with a screen open, so the
//! pointer is free) or its line in the character screen, a small plate beside the pointer
//! gives its name and what it does.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;
use voxmar_engine::effect::{ActiveEffect, EffectId, Kind};

use crate::lang::Locale;
use crate::menu::Screen;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, MUTED, SOFT, UiFont, shadowed, space, text};

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn)
            .add_systems(Update, show.run_if(in_state(Screen::InGame)))
            .add_systems(PreUpdate, show_effects.after(crate::net::receive).run_if(resource_exists::<ShowEffects>));
    }
}

/// Timed effects a screenshot shows the player having (`--effect`), by name and seconds;
/// the server is not told, so they do nothing.
#[derive(Resource)]
pub struct ShowEffects(pub Vec<(String, u16)>);

/// The effect whose plate a screenshot shows over its chip, as if the pointer were there
/// (`--effect-tip`).
#[derive(Resource)]
pub struct PinnedTip(pub String);

/// Something that shows an effect, to say what it is on hover.
#[derive(Component, Clone, Copy)]
pub struct EffectHover(pub EffectId);

#[derive(Component)]
struct EffectTip;
#[derive(Component)]
struct EffectTipName;
#[derive(Component)]
struct EffectTipWhat;
#[derive(Component)]
struct EffectTipLeft;
#[derive(Component)]
struct EffectTipIcon;

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
        // Its icon beside its name, as on its chip.
        tip.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|head| {
            head.spawn((Node { width: Val::Px(10.0 * PX), height: Val::Px(10.0 * PX), flex_shrink: 0.0, ..default() }, ImageNode::default(), EffectTipIcon));
            head.spawn((shadowed(font, "", BODY, ACCENT), EffectTipName));
        });
        tip.spawn((text(font, "", BODY, SOFT), EffectTipWhat));
        tip.spawn((text(font, "", BODY, MUTED), EffectTipLeft));
    });
}

#[allow(clippy::type_complexity)]
fn show(
    locale: Res<Locale>,
    launch: Res<crate::session::Launch>,
    window: Query<&Window, With<PrimaryWindow>>,
    hovered: Query<(&EffectHover, &RelativeCursorPosition, &InheritedVisibility, &bevy::ui::UiGlobalTransform, &ComputedNode)>,
    pinned: Option<Res<PinnedTip>>,
    mut tip: Query<(&mut Node, &mut Visibility), With<EffectTip>>,
    mut name: Query<(&mut Text, &mut TextColor), (With<EffectTipName>, Without<EffectTipWhat>)>,
    mut what: Query<&mut Text, (With<EffectTipWhat>, Without<EffectTipName>, Without<EffectTipLeft>)>,
    mut left: Query<(&mut Text, &mut Node), (With<EffectTipLeft>, Without<EffectTipName>, Without<EffectTipWhat>, Without<EffectTip>)>,
    mut icon: Query<&mut ImageNode, With<EffectTipIcon>>,
    (scale, time, player, images): (Res<UiScale>, Res<Time>, Res<crate::player::LocalPlayer>, Res<super::HudImages>),
) {
    let Ok((mut node, mut visibility)) = tip.single_mut() else { return };
    let over = hovered.iter().find(|(_, cursor, visible, ..)| visible.get() && cursor.cursor_over()).map(|(effect, ..)| effect.0);
    // The pointer is in the window's points; the interface is laid out at its own scale.
    let at = window.single().ok().and_then(Window::cursor_position).map(|at| at / scale.0);
    let pinned = pinned.as_ref().and_then(|p| launch.content.effects.find(&p.0)).and_then(|id| {
        let (_, _, _, place, size) = hovered.iter().find(|(effect, _, visible, ..)| effect.0 == id && visible.get())?;
        Some((id, (place.translation + size.size() * Vec2::new(0.0, 0.5)) * size.inverse_scale_factor))
    });
    let (Some(id), Some(at)) = (over.or(pinned.map(|p| p.0)), at.or(pinned.map(|p| p.1))) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let effect = launch.content.effects.get(id);
    if let Ok(mut image) = icon.single_mut() {
        let wanted = images.effect(id);
        if image.image != wanted {
            image.image = wanted;
        }
    }
    // A timed effect says how long it has left; one that lasts while its cause does, nothing.
    if let Ok((mut text, mut line)) = left.single_mut() {
        let words = super::seconds_left(&player, id, time.elapsed_secs()).map(|s| locale.format("effect.left", &[("time", &super::clock(s))]));
        let display = if words.is_some() { Display::Flex } else { Display::None };
        if line.display != display {
            line.display = display;
        }
        let words = words.unwrap_or_default();
        if text.0 != words {
            text.0 = words;
        }
    }
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

/// Adds `--effect`'s timed effects to the player's, each time the server says what theirs are.
fn show_effects(shown: Res<ShowEffects>, launch: Res<crate::session::Launch>, time: Res<Time>, mut player: ResMut<crate::player::LocalPlayer>, mut since: Local<Option<f32>>) {
    let since = *since.get_or_insert(time.elapsed_secs());
    let missing: Vec<ActiveEffect> = shown
        .0
        .iter()
        .filter_map(|(name, seconds)| {
            let effect = launch.content.effects.find(name)?;
            let left = (*seconds as f32 - (player.vitals_at - since).max(0.0)).max(0.0) as u16;
            Some(ActiveEffect { effect, seconds_left: Some(left) })
        })
        .filter(|a| !player.vitals.effects.iter().any(|b| b.effect == a.effect))
        .collect();
    if !missing.is_empty() {
        let effects = &mut player.vitals.effects;
        effects.splice(0..0, missing);
        effects.sort_by_key(|a| (launch.content.effects.get(a.effect).kind == Kind::Debuff, a.effect));
    }
}
