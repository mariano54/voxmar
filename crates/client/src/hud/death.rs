//! The death screen: the world dimmed and reddened behind a small panel saying so, with a
//! button to respawn (R does the same).

use bevy::prelude::*;
use voxmar_engine::action::ActionTarget;

use crate::Actions;
use crate::lang::Localized;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::ui::widgets::{BODY, ButtonStyle, MUTED, UiFont, button_row, panel, screen, space, spawn_button, spawn_heading, text};

/// The tint over the world while dead: the menus' dark, warmed towards red.
const DEATH_SCRIM: Color = Color::srgba(0.18, 0.02, 0.03, 0.55);
const WIDTH: f32 = 200.0 * crate::ui::skin::PX;

pub struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn).add_systems(Update, (show, respawn).run_if(in_state(Screen::InGame)));
    }
}

#[derive(Component)]
struct DeathScreen;
#[derive(Component)]
struct RespawnButton;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    commands.spawn((DeathScreen, screen(DEATH_SCRIM), Visibility::Hidden, GlobalZIndex(1))).with_children(|root| {
        root.spawn(panel(WIDTH)).with_children(|card| {
            spawn_heading(card, font, "hud.died.title");
            let hint = Node { justify_content: JustifyContent::Center, margin: UiRect::vertical(Val::Px(space::INNER)), ..default() };
            // What dying cost: the three kept items, the rest where you fell for 30 minutes.
            let cost = Node { justify_content: JustifyContent::Center, margin: UiRect::top(Val::Px(space::INNER)), ..default() };
            card.spawn(cost).with_child((Localized("hud.died.cost"), text(font, "", BODY, MUTED)));
            card.spawn(hint).with_child((Localized("hud.died"), text(font, "", BODY, MUTED)));
            card.spawn(button_row(space::GAP)).with_children(|row| {
                spawn_button(row, font, "hud.respawn", ButtonStyle::Primary, RespawnButton);
            });
        });
    });
}

fn show(player: Res<LocalPlayer>, ui: Res<UiState>, mut root: Query<&mut Visibility, With<DeathScreen>>) {
    if let Ok(mut visibility) = root.single_mut() {
        // The menu, opened over it, comes first.
        let shown = player.dead && !ui.menu_open;
        visibility.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// R, or the button, asks the server to respawn.
fn respawn(
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    net: Res<Net>,
    actions: Res<Actions>,
    button: Query<&Interaction, (Changed<Interaction>, With<RespawnButton>)>,
) {
    let clicked = button.iter().any(|interaction| *interaction == Interaction::Pressed);
    if player.dead
        && (clicked || (keys.just_pressed(KeyCode::KeyR) && !ui.console_open))
        && let Some(respawn) = actions.named("core:respawn")
    {
        net.propose(respawn, ActionTarget::None);
    }
}
