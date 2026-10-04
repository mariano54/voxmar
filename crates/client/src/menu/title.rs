//! The title screen: the name of the game over the menu backdrop, and the way into
//! everything else.

use bevy::prelude::*;

use crate::ui::skin::{PX, font_size};
use crate::ui::widgets::{
    ACCENT, BODY, ButtonStyle, ERROR, GOLD, MUTED, Pressed, UiFont, button_row, panel, shadowed, spawn_button, text,
};
use super::{Notice, Screen};

pub struct TitlePlugin;

impl Plugin for TitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Title), spawn).add_systems(Update, click.run_if(in_state(Screen::Title)));
    }
}

#[derive(Component, Clone, Copy)]
enum Action {
    Singleplayer,
    Multiplayer,
    Settings,
    Quit,
}

fn spawn(mut commands: Commands, font: Res<UiFont>, mut notice: ResMut<Notice>) {
    let font = &font.0;
    // Darken towards the bottom, behind the buttons, leaving the sky clear.
    let shade = LinearGradient::to_bottom(vec![
        ColorStop::new(Color::NONE, Val::Percent(25.0)),
        ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.45), Val::Percent(100.0)),
    ]);
    let root = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    };
    commands.spawn((DespawnOnExit(Screen::Title), root, BackgroundGradient::from(shade))).with_children(|root| {
        // The name in big gold pixels, between teal diamonds.
        let title = Node { align_items: AlignItems::Center, column_gap: Val::Px(6.0 * PX), margin: UiRect::bottom(Val::Px(18.0 * PX)), ..default() };
        root.spawn(title).with_children(|title| {
            title.spawn(shadowed(font, "◆", font_size(4), ACCENT));
            title.spawn(shadowed(font, "VOXMAR", font_size(10), GOLD));
            title.spawn(shadowed(font, "◆", font_size(4), ACCENT));
        });

        let mut card = panel(200.0 * PX);
        card.0.row_gap = Val::Px(4.0 * PX);
        root.spawn(card).with_children(|card| {
            card.spawn(button_row(0.0)).with_children(|row| {
                spawn_button(row, font, "Singleplayer", ButtonStyle::Primary, Action::Singleplayer);
            });
            card.spawn(button_row(0.0)).with_children(|row| {
                spawn_button(row, font, "Multiplayer", ButtonStyle::Secondary, Action::Multiplayer);
            });
            card.spawn(button_row(0.0)).with_children(|row| {
                spawn_button(row, font, "Settings", ButtonStyle::Secondary, Action::Settings);
                spawn_button(row, font, "Quit", ButtonStyle::Secondary, Action::Quit);
            });
            if let Some(message) = notice.0.take() {
                let node = Node { max_width: Val::Px(170.0 * PX), margin: UiRect::top(Val::Px(4.0)), ..default() };
                card.spawn((node, text(font, message, BODY, ERROR)));
            }
        });

        let corner = Node { position_type: PositionType::Absolute, left: Val::Px(6.0 * PX), bottom: Val::Px(5.0 * PX), ..default() };
        root.spawn((corner, shadowed(font, concat!("voxmar ", env!("CARGO_PKG_VERSION")), BODY, MUTED)));
    });
}

fn click(buttons: Query<(&Action, &Interaction), Pressed>, mut next: ResMut<NextState<Screen>>, mut exit: MessageWriter<AppExit>) {
    for (action, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            Action::Singleplayer => next.set(Screen::Worlds),
            Action::Multiplayer => next.set(Screen::Join),
            Action::Settings => next.set(Screen::Settings),
            Action::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}
