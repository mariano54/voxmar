//! Discovering what the game does as it happens. An effect that comes on shows its name and
//! what it does at the top of the screen for a few seconds. The first time the player meets
//! a mechanic (opens the inventory, holds seeds, faces an animal), a one-line tip says what
//! can be done with it; each tip shows once, remembered in the settings.

use bevy::prelude::*;
use voxmar_engine::effect::{EffectId, Kind};

use super::HudImages;
use crate::interact::Target;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::settings::Settings;
use crate::ui::skin::{PX, Piece};
use crate::ui::widgets::{ACCENT, BODY, ERROR, GOLD, SOFT, TEXT, UiFont, shadowed, space, text};

pub struct TipsPlugin;

impl Plugin for TipsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tips>()
            .add_systems(PostStartup, spawn)
            .add_systems(Update, (announce_effects, notice, show_tip, age_popups).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut tips: ResMut<Tips>| *tips = Tips::default());
    }
}

/// Seconds an effect's pop-up stays, and a tip.
const POPUP_SECONDS: f32 = 4.0;
const TIP_SECONDS: f32 = 7.0;
/// Pop-ups showing at once; more wait their turn.
const POPUPS: usize = 3;
/// How wide a pop-up's and a tip's words may run.
const WIDTH: f32 = 150.0 * PX;

/// Every tip, by name: its words are `tip.<name>`.
pub const TIPS: [&str; 10] = ["inventory", "recipes", "character", "effect", "seed", "food", "creature", "level", "table", "catalogue"];

/// The tips waiting to show, the one showing and since when, and the effects seen.
#[derive(Resource, Default)]
pub struct Tips {
    queue: Vec<&'static str>,
    pub showing: Option<(&'static str, f32)>,
    /// The effects on the player as last seen, once the server has said what they are.
    effects: Option<Vec<EffectId>>,
    /// Skill levels as last seen, to see one go up.
    levels: Option<Vec<u8>>,
}

#[derive(Component)]
struct Popups;
/// An effect's pop-up, and when it came.
#[derive(Component)]
pub struct Popup(pub EffectId, f32);
#[derive(Component)]
struct TipPlate;
#[derive(Component)]
struct TipCorner;
#[derive(Component)]
struct TipWords;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    // Effects across the top, under the sky's middle.
    let column = Node {
        position_type: PositionType::Absolute,
        top: Val::Percent(12.0),
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: Val::Px(space::INNER),
        ..default()
    };
    commands.spawn((column, Popups, GlobalZIndex(1)));
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

/// A pop-up for each effect that comes on (not those the player joined with).
#[allow(clippy::too_many_arguments)]
fn announce_effects(
    mut commands: Commands,
    time: Res<Time>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    locale: Res<Locale>,
    font: Res<UiFont>,
    images: Res<HudImages>,
    mut tips: ResMut<Tips>,
    column: Query<Entity, With<Popups>>,
    shown: Query<&Popup>,
    shows: Option<Res<super::effects::ShowEffects>>,
) {
    // Before the server has said, the player has no effects to compare with.
    if player.vitals_at <= 0.0 {
        return;
    }
    let now: Vec<EffectId> = player.vitals.effects.iter().map(|a| a.effect).collect();
    // Effects a screenshot shows (`--effect`) come on after the ones the player joined with.
    let shown_on = |id: &EffectId| shows.as_ref().is_some_and(|shows| shows.0.iter().any(|(name, _)| launch.content.effects.find(name) == Some(*id)));
    let joined: Vec<EffectId> = now.iter().copied().filter(|id| !shown_on(id)).collect();
    let Some(before) = tips.effects.replace(now.clone()).or_else(|| (!joined.is_empty() || shows.is_some()).then_some(joined)) else { return };
    let Ok(column) = column.single() else { return };
    let mut showing = shown.iter().count();
    for &id in now.iter().filter(|id| !before.contains(id)) {
        if showing >= POPUPS {
            break;
        }
        showing += 1;
        let effect = launch.content.effects.get(id);
        let color = if effect.kind == Kind::Buff { ACCENT } else { ERROR };
        let plate = Node {
            width: Val::Px(WIDTH),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(space::HAIR),
            padding: UiRect::axes(Val::Px(space::GAP), Val::Px(space::INNER)),
            ..default()
        };
        commands.entity(column).with_children(|column| {
            column.spawn((plate, Piece::Tooltip.image(), Popup(id, time.elapsed_secs()))).with_children(|plate| {
                plate.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() }).with_children(|head| {
                    head.spawn(super::effect_icon(&images, id));
                    head.spawn(shadowed(&font.0, locale.effect(effect), BODY, color));
                });
                plate.spawn(text(&font.0, locale.effect_description(effect), BODY, SOFT));
            });
        });
    }
}

fn age_popups(mut commands: Commands, time: Res<Time>, popups: Query<(Entity, &Popup)>, shows: Option<Res<super::effects::ShowEffects>>) {
    // Held while a screenshot or the gate shows effects (`--effect`).
    if shows.is_some() {
        return;
    }
    for (entity, popup) in &popups {
        if time.elapsed_secs() - popup.1 > POPUP_SECONDS {
            commands.entity(entity).despawn();
        }
    }
}

/// Queues the tip for each mechanic the player meets for the first time.
#[allow(clippy::too_many_arguments)]
fn notice(
    ui: Res<UiState>,
    player: Res<LocalPlayer>,
    launch: Res<Launch>,
    target: Res<Target>,
    hovered: Res<super::recipes::HoveredItem>,
    settings: Res<Settings>,
    mut tips: ResMut<Tips>,
) {
    let content = &launch.content;
    let held = player.inventory.get(player.selected).map(|stack| stack.item);
    let levels: Vec<u8> = content.skills.ids().map(|id| content.skills.level(player.skills.xp(id))).collect();
    let levelled = player.skills_known && tips.levels.as_ref().is_some_and(|before| before.iter().zip(&levels).any(|(was, is)| is > was));
    if player.skills_known {
        tips.levels = Some(levels);
    }
    let met = [
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
    for (name, now) in met {
        debug_assert!(TIPS.contains(&name), "the tip {name} has no words listed");
        let seen = settings.tips_seen.iter().any(|s| s == name);
        if now && !seen && !tips.queue.contains(&name) && tips.showing.is_none_or(|(showing, _)| showing != name) {
            tips.queue.push(name);
        }
    }
}

/// Shows the waiting tips one at a time, each remembered as seen once shown.
fn show_tip(
    time: Res<Time>,
    locale: Res<Locale>,
    mut settings: ResMut<Settings>,
    mut tips: ResMut<Tips>,
    mut plate: Query<&mut Visibility, With<TipPlate>>,
    mut words: Query<&mut Text, With<TipWords>>,
    mut corner: Query<&mut Node, With<TipCorner>>,
    catalogue: Query<&Visibility, (With<super::creative::CataloguePanel>, Without<TipPlate>)>,
) {
    let now = time.elapsed_secs();
    if tips.showing.is_some_and(|(_, since)| now - since > TIP_SECONDS) {
        tips.showing = None;
    }
    if tips.showing.is_none() && !tips.queue.is_empty() {
        let name = tips.queue.remove(0);
        tips.showing = Some((name, now));
        if !settings.tips_seen.iter().any(|s| s == name) {
            settings.tips_seen.push(name.to_owned());
            settings.save();
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
        let wanted = locale.get(&format!("tip.{name}")).to_owned();
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lang::Locale;

    /// Every tip has its words in English and Japanese.
    #[test]
    fn every_tip_has_words() {
        // This checkout's own words, not the installed game's.
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        for code in ["en", "ja"] {
            let locale = Locale::load(&content, code);
            for name in super::TIPS.iter().map(|name| format!("tip.{name}")).chain(["tip.label".to_owned()]) {
                assert!(locale.find(&name).is_some(), "{code}: no words for {name}");
            }
        }
    }
}
