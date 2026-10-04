//! Menus: the title screen, the world list, world creation and joining a server, shown
//! over a slowly turning view of a demo world. The settings panel (`crate::settings`)
//! doubles as the title screen's settings.

mod background;
mod create;
mod join;
mod loading;
mod title;
mod worlds;

use bevy::prelude::*;

pub use join::connect;
pub use worlds::worlds_dir;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<Screen>()
            .add_computed_state::<InMenu>()
            .init_resource::<Notice>()
            .add_plugins((
                background::BackgroundPlugin,
                title::TitlePlugin,
                worlds::WorldsPlugin,
                create::CreatePlugin,
                join::JoinPlugin,
                loading::LoadingPlugin,
            ))
            .add_systems(PostStartup, open_first_screen)
            .add_systems(Update, back_on_escape.run_if(in_state(InMenu)));
    }
}

/// What the player is looking at.
#[derive(States, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Screen {
    /// Until startup has prepared textures, materials and fonts.
    #[default]
    Boot,
    Title,
    /// Singleplayer: the saved worlds.
    Worlds,
    CreateWorld,
    /// Multiplayer: joining a server.
    Join,
    /// The settings panel, opened from the title screen.
    Settings,
    /// Playing (or joining) a world.
    InGame,
}

impl Screen {
    /// Parses a screen name given on the command line.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "title" => Screen::Title,
            "worlds" => Screen::Worlds,
            "create" => Screen::CreateWorld,
            "join" => Screen::Join,
            "settings" => Screen::Settings,
            _ => return None,
        })
    }

    /// The screen Esc and Back lead to.
    fn back(self) -> Option<Screen> {
        match self {
            Screen::Worlds | Screen::Join | Screen::Settings => Some(Screen::Title),
            Screen::CreateWorld => Some(Screen::Worlds),
            Screen::Boot | Screen::Title | Screen::InGame => None,
        }
    }
}

/// Any menu screen, over the menu background.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InMenu;

impl ComputedStates for InMenu {
    type SourceStates = Screen;

    fn compute(screen: Screen) -> Option<Self> {
        (!matches!(screen, Screen::Boot | Screen::InGame)).then_some(InMenu)
    }
}

/// The screen to show once startup is done.
#[derive(Resource)]
pub struct FirstScreen(pub Screen);

fn open_first_screen(first: Option<Res<FirstScreen>>, mut next: ResMut<NextState<Screen>>) {
    next.set(first.map_or(Screen::Title, |first| first.0));
}

/// A message for the player from a session that ended badly (for example, the server's
/// reason for refusing them), shown by the next screen.
#[derive(Resource, Default)]
pub struct Notice(pub Option<String>);

fn back_on_escape(keys: Res<ButtonInput<KeyCode>>, screen: Res<State<Screen>>, mut next: ResMut<NextState<Screen>>) {
    if keys.just_pressed(KeyCode::Escape)
        && let Some(back) = screen.back()
    {
        next.set(back);
    }
}
