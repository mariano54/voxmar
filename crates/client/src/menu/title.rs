//! The title screen: the name of the game over the menu backdrop, and the way into
//! everything else.

use bevy::prelude::*;

use super::widgets::{ButtonStyle, ERROR, MUTED, Pressed, TEXT, UiFont, button_row, panel, spawn_button, strong, text};
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
        let wordmark = strong(font, "voxmar", 96.0, TEXT, FontWeight::BOLD);
        let shadow = TextShadow { offset: Vec2::new(0.0, 3.0), color: Color::srgba(0.0, 0.0, 0.0, 0.35) };
        root.spawn((wordmark, shadow, Node { margin: UiRect::bottom(Val::Px(36.0)), ..default() }));

        let mut card = panel(380.0);
        card.0.padding = UiRect::all(Val::Px(20.0));
        card.0.row_gap = Val::Px(12.0);
        card.1 = BackgroundColor(Color::srgba(0.08, 0.09, 0.11, 0.82));
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
                let node = Node { max_width: Val::Px(340.0), margin: UiRect::top(Val::Px(4.0)), ..default() };
                card.spawn((node, text(font, message, 14.0, ERROR)));
            }
        });

        let corner = Node { position_type: PositionType::Absolute, left: Val::Px(16.0), bottom: Val::Px(12.0), ..default() };
        root.spawn((corner, text(font, concat!("voxmar ", env!("CARGO_PKG_VERSION")), 12.0, MUTED)));
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
