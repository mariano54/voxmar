//! A one-line tip the first time the player meets a mechanic: opens the inventory, holds
//! seeds, faces an animal. Each tip comes once, on the moment the player first meets its
//! mechanic, and stays while that lasts (the inventory open, the seeds held), until the
//! player has had time to read it; then never again, remembered in the settings. Tips come
//! one at a time and not close together: one that would crowd another waits for the next
//! time its moment comes.

use bevy::prelude::*;

use crate::interact::Target;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{GOLD, TEXT, UiFont, shadowed, space, text, BODY};

pub struct TipsPlugin;

impl Plugin for TipsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tips>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (notice, show_tip).chain().run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut tips: ResMut<Tips>| *tips = Tips::default());
    }
}

/// A tip stays at least this long, however soon its moment passes, and at most this long
/// while it lasts.
const LEAST: f32 = 6.0;
const MOST: f32 = 25.0;
/// Seconds after one tip goes before another may come.
const APART: f32 = 90.0;
/// How wide a tip's words may run.
const WIDTH: f32 = 150.0 * PX;

/// Every tip, by name: its words are `tip.<name>`.
pub const TIPS: [&str; 11] = ["quests", "inventory", "recipes", "character", "effect", "seed", "food", "creature", "level", "table", "catalogue"];

/// The tip showing, since when and whether its moment still lasts; when the last went; and
/// each mechanic's moment as last seen, to tell when one begins.
#[derive(Resource, Default)]
pub struct Tips {
    pub showing: Option<(&'static str, f32)>,
    lasting: bool,
    gone_at: Option<f32>,
    met: Vec<(&'static str, bool)>,
    /// Skill levels as last seen, to see one go up.
    levels: Option<Vec<u8>>,
}

impl Tips {
    /// Puts the showing tip away at once, as if its time were up long ago (for the gate).
    pub fn dismiss(&mut self) {
        self.showing = None;
        self.gone_at = None;
    }
}

/// A tip a screenshot shows and holds (`--tip`).
#[derive(Resource)]
pub struct HeldTip(pub String);

#[derive(Component)]
struct TipPlate;
#[derive(Component)]
struct TipCorner;
#[derive(Component)]
struct TipWords;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    // Tips in the bottom right corner, clear of the screens in the middle, the hotbar and
    // the chat at the left; at the left while creative mode's catalogue fills the right
    // (the chat is put away while a screen is open).
    let corner = Node { position_type: PositionType::Absolute, right: Val::Px(space::SECTION), bottom: Val::Px(space::SECTION), ..default() };
    let plate = Node {
        width: Val::Px(WIDTH),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(space::HAIR),
        padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
        ..default()
    };
    commands.spawn((corner, GlobalZIndex(1), TipCorner)).with_children(|corner| {
        corner.spawn((plate, Piece::Tooltip.image(), TipPlate, Visibility::Hidden)).with_children(|plate| {
            plate.spawn((shadowed(&font.0, "", BODY, GOLD), crate::lang::Localized("tip.label")));
            plate.spawn((text(&font.0, "", BODY, TEXT), TipWords));
        });
    });
}

/// Each mechanic's moment now, and the tip for one that has just begun, if the player has
/// not seen it and no other is showing or has just gone.
#[allow(clippy::too_many_arguments)]
fn notice(
    time: Res<Time>,
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    target: Res<Target>,
    hovered: Res<super::recipes::HoveredItem>,
    mut settings: ResMut<Settings>,
    mut tips: ResMut<Tips>,
) {
    let content = &launch.content;
    let held = player.inventory.get(player.selected).map(|stack| stack.item);
    let levels: Vec<u8> = content.skills.ids().map(|id| content.skills.level(player.skills.xp(id))).collect();
    let levelled = player.skills_known && tips.levels.as_ref().is_some_and(|before| before.iter().zip(&levels).any(|(was, is)| is > was));
    if player.skills_known {
        tips.levels = Some(levels);
    }
    let met: [(&'static str, bool); 11] = [
        // The first thing a new player needs: where the quests are. It lasts until the book opens.
        ("quests", player.skills_known && !ui.quests_open),
        ("inventory", ui.inventory_open),
        ("recipes", ui.inventory_open && hovered.0.is_some()),
        ("character", ui.character_open),
        ("effect", !player.vitals.effects.is_empty() && player.vitals_at > 0.0),
        ("seed", held.is_some_and(|item| content.crops.planted_by(item).is_some())),
        ("food", held.is_some_and(|item| content.blocks.def(item).tags.iter().any(|tag| tag == "edible"))),
        ("creature", target.creature.is_some()),
        ("level", levelled),
        ("table", player.station.is_some()),
        ("catalogue", ui.inventory_open && player.allow_flight),
    ];
    let now = time.elapsed_secs();
    // The showing tip's moment: does it still last?
    if let Some((name, _)) = tips.showing {
        tips.lasting = met.iter().any(|(m, on)| *m == name && *on);
    }
    let before = std::mem::replace(&mut tips.met, met.to_vec());
    let free = tips.showing.is_none() && tips.gone_at.is_none_or(|gone| now - gone > APART);
    if !free {
        return;
    }
    for (name, on) in met {
        debug_assert!(TIPS.contains(&name), "the tip {name} has no words listed");
        let began = on && !before.iter().any(|(m, was)| *m == name && *was);
        if began && !settings.tips_seen.iter().any(|s| s == name) {
            tips.showing = Some((name, now));
            tips.lasting = true;
            settings.tips_seen.push(name.to_owned());
            settings.save();
            break;
        }
    }
}

/// Shows the tip while its moment lasts, from `LEAST` to `MOST` seconds.
#[allow(clippy::too_many_arguments)]
fn show_tip(
    time: Res<Time>,
    locale: Res<Locale>,
    settings: Res<Settings>,
    mut tips: ResMut<Tips>,
    mut plate: Query<&mut Visibility, With<TipPlate>>,
    mut words: Query<&mut Text, With<TipWords>>,
    mut corner: Query<&mut Node, With<TipCorner>>,
    held: Option<Res<HeldTip>>,
    catalogue: Query<&Visibility, (With<super::creative::CataloguePanel>, Without<TipPlate>)>,
) {
    let now = time.elapsed_secs();
    if let Some(name) = held.and_then(|held| TIPS.iter().find(|name| **name == held.0)) {
        tips.showing = Some((name, now));
    }
    if let Some((_, since)) = tips.showing {
        let shown = now - since;
        if shown > MOST || (shown > LEAST && !tips.lasting) {
            tips.showing = None;
            tips.gone_at = Some(now);
        }
    }
    // Out of the creative catalogue's way.
    let aside = catalogue.iter().any(|v| *v != Visibility::Hidden);
    if let Ok(mut node) = corner.single_mut() {
        let (left, right) = if aside { (Val::Px(space::SECTION), Val::Auto) } else { (Val::Auto, Val::Px(space::SECTION)) };
        if node.left != left {
            node.left = left;
            node.right = right;
        }
    }
    let Ok(mut visibility) = plate.single_mut() else { return };
    visibility.set_if_neq(if tips.showing.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    if let Some((name, _)) = tips.showing
        && let Ok(mut text) = words.single_mut()
    {
        let keys = bound_keys(&settings).map(|(name, key)| (name, key.name(&locale)));
        let args: Vec<(&str, &dyn std::fmt::Display)> = keys.iter().map(|(name, key)| (*name, key as &dyn std::fmt::Display)).collect();
        let wanted = locale.format(&format!("tip.{name}"), &args);
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

/// The keys a tip may name (`{character}` in its words), as bound.
fn bound_keys(settings: &Settings) -> [(&'static str, crate::keys::Key); 5] {
    [("character", settings.character_key), ("recipes", settings.recipes_key), ("uses", settings.uses_key), ("quests", settings.quests_key), ("inventory", settings.inventory_key)]
}

#[cfg(test)]
mod tests {
    use crate::lang::Locale;

    /// Every tip has its words in English and Japanese, naming only the keys it is given.
    #[test]
    fn every_tip_has_words() {
        // This checkout's own words, not the installed game's.
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for code in ["en", "ja"] {
            let locale = Locale::load(&content, code);
            for name in super::TIPS.iter().map(|name| format!("tip.{name}")).chain(["tip.label".to_owned()]) {
                assert!(locale.find(&name).is_some(), "{code}: no words for {name}");
            }
            // Each key a tip names is one it is given.
            let given = super::bound_keys(&crate::settings::Settings::default()).map(|(name, _)| name);
            for name in super::TIPS.iter().map(|name| format!("tip.{name}")) {
                let words = locale.get(&name);
                for placeholder in words.split('{').skip(1).filter_map(|part| part.split_once('}')).map(|(placeholder, _)| placeholder) {
                    assert!(given.contains(&placeholder), "{code}: {name} names {{{placeholder}}}, which no tip is given (see bound_keys)");
                }
            }
        }
    }
}
