//! What a right click would do, shown under the crosshair while it rests on something to
//! use: the key and the action, "Right-click Light" at a cold hearth, "Open" at a door,
//! "Feed" at one of your animals with its food in hand, "Pray" at an altar, "Cast" with a
//! rod at water. The action is the one the click would propose (`interact::block_use`,
//! `interact::cast_use`, `creatures::creature_use`); its words come from `prompts.ron`, where every action names a prompt or says it has none, so a mod's action
//! is never silently promptless.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::Deserialize;
use voxmar_engine::action::ActionRegistry;
use voxmar_engine::content::ContentFile;

use crate::Actions;
use crate::creatures::{Creatures, creature_use};
use crate::interact::{Target, block_use, cast_use};
use crate::lang::{Locale, TightLine};
use crate::menu::Screen;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::ui::skin::{CAP_MIN, PX, Piece};
use crate::ui::widgets::{BODY, TEXT, UiFont, shadowed, space};
use crate::world::ClientWorld;

/// Points under the middle of the screen the prompt hangs, clear of the crosshair.
const BELOW: f32 = 14.0 * PX;

/// The words for each action's prompt, by the action's name (`base:light`), from every
/// package's `prompts.ron`; a later package that names an action replaces its entry.
#[derive(Resource, Clone, Debug, Default)]
pub struct Prompts {
    actions: BTreeMap<String, Prompt>,
}

/// One action's prompt: the lang key of its word, or `none: true` for an action with
/// nothing to say at the crosshair (mining, a hand's own doings, an action the click never
/// proposes). `held` gives other words when the item in hand has a tag (shears at a sheep,
/// "Shear"), `target` when the block looked at has a tag or name (an open door, "Close"),
/// and `ready` when it is a station whose work is done ("Take").
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Prompt {
    prompt: Option<String>,
    none: bool,
    held: BTreeMap<String, String>,
    target: BTreeMap<String, String>,
    ready: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptFile {
    actions: BTreeMap<String, Prompt>,
}

impl Prompts {
    pub const FILE: &str = "prompts.ron";

    pub fn load(files: &[ContentFile]) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut all = Prompts::default();
        for file in files.iter().filter(|f| f.name == Self::FILE) {
            let def: PromptFile = options.from_str(&file.source).map_err(|e| format!("{}/{}: {e}", file.package, Self::FILE))?;
            all.actions.extend(def.actions);
        }
        Ok(all)
    }

    /// Whether every prompt is for an action the content has and says one thing, and every
    /// action has a prompt or `none: true`, both ways, so an action a mod adds is never left
    /// without a word by accident.
    pub fn check(&self, actions: &ActionRegistry) -> Result<(), String> {
        for (name, prompt) in &self.actions {
            if actions.id(name).is_none() {
                return Err(format!("{}: a prompt for `{name}`, which is no action", Self::FILE));
            }
            let words = prompt.prompt.is_some() || !prompt.held.is_empty() || !prompt.target.is_empty() || prompt.ready.is_some();
            if prompt.none == words {
                return Err(format!("{}: `{name}` must give a prompt or say none: true, not both", Self::FILE));
            }
            if words && prompt.prompt.is_none() {
                return Err(format!("{}: `{name}` gives other words but no prompt of its own", Self::FILE));
            }
        }
        if let Some(name) = actions.names().find(|name| !self.actions.contains_key(*name)) {
            return Err(format!("{}: no prompt for `{name}`: give it one, or none: true", Self::FILE));
        }
        Ok(())
    }

    /// The lang key of what `action` says, with an item tagged `held` in hand at a block
    /// tagged or named `target`, `ready` if it is a station whose work is done.
    pub fn key<'a>(&'a self, action: &str, held: &[String], target: &[&str], ready: bool) -> Option<&'a str> {
        let prompt = self.actions.get(action).filter(|p| !p.none)?;
        let by = |map: &'a BTreeMap<String, String>, keys: &[&str]| keys.iter().find_map(|k| map.get(*k)).map(String::as_str);
        let held: Vec<&str> = held.iter().map(String::as_str).collect();
        by(&prompt.held, &held)
            .or_else(|| by(&prompt.target, target))
            .or(prompt.ready.as_deref().filter(|_| ready))
            .or(prompt.prompt.as_deref())
    }

    /// Every lang key the prompts name.
    #[cfg(test)]
    fn keys(&self) -> impl Iterator<Item = &str> {
        self.actions.values().flat_map(|p| p.prompt.iter().chain(p.held.values()).chain(p.target.values()).chain(p.ready.iter())).map(String::as_str)
    }
}

pub struct PromptPlugin;

impl Plugin for PromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn).add_systems(Update, show.after(crate::creatures::aim).run_if(in_state(Screen::InGame))).add_systems(OnExit(Screen::InGame), hide);
    }
}

/// The prompt: a row across the screen under its middle, holding the key's cap and the word.
#[derive(Component)]
pub(crate) struct UsePrompt;
#[derive(Component)]
struct PromptKey;
#[derive(Component)]
pub(crate) struct PromptWord;

fn spawn(mut commands: Commands, font: Res<UiFont>) {
    let font = &font.0;
    let across = Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        top: Val::Percent(50.0),
        margin: UiRect::top(Val::Px(BELOW)),
        justify_content: JustifyContent::Center,
        ..default()
    };
    let row = Node { align_items: AlignItems::Center, column_gap: Val::Px(space::INNER), ..default() };
    let cap = Node { padding: UiRect::axes(Val::Px(space::INNER), Val::Px(space::HAIR)), min_width: Val::Px(CAP_MIN), justify_content: JustifyContent::Center, ..default() };
    commands.spawn((across, UsePrompt, Visibility::Hidden, GlobalZIndex(1))).with_children(|across| {
        across.spawn(row).with_children(|row| {
            row.spawn((cap, Piece::Button.image())).with_child((shadowed(font, "", BODY, TEXT), PromptKey, TightLine));
            row.spawn((shadowed(font, "", BODY, TEXT), PromptWord));
        });
    });
}

/// Finds what a right click would do and says it, or hides the prompt.
#[allow(clippy::too_many_arguments)]
fn show(
    ui: Res<UiState>,
    target: Res<Target>,
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    (launch, actions, creatures, prompts): (Res<Launch>, Res<Actions>, Res<Creatures>, Option<Res<Prompts>>),
    keys: Res<ButtonInput<KeyCode>>,
    locale: Res<Locale>,
    mut prompt: Query<&mut Visibility, With<UsePrompt>>,
    mut key: Query<&mut Text, (With<PromptKey>, Without<PromptWord>)>,
    mut word: Query<&mut Text, (With<PromptWord>, Without<PromptKey>)>,
) {
    let (Ok(mut visibility), Some(prompts)) = (prompt.single_mut(), prompts) else { return };
    let content = &launch.content;
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let held_tags = held.map(|item| content.blocks.def(item).tags.clone()).unwrap_or_default();
    let free = ui.cursor_locked && !player.dead && !(ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.console_open);
    let said = free.then(|| match target.creature {
        Some(id) => {
            let view = creatures.view(id).filter(|_| target.creature_touched)?;
            let mine = view.kept && view.owner.is_some() && view.owner == creatures.me;
            let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            // Whether the button would guard instead is the fight's to say: a prompt is
            // shown only for a use, and a fight's guard has none.
            let action = creature_use(content, view, held, mine, shift, false)?;
            prompts.key(action, &held_tags, &[], false)
        }
        None => {
            // What the block looked at is used for; else, a rod in hand casts at water.
            let used = target.hit.and_then(|hit| Some((block_use(&world, &player, content, &actions.0, &hit)?, hit)));
            let (action, hit) = used.or_else(|| cast_use(&world, &player, content, &actions.0))?;
            let name = &actions.0.get(action)?.name;
            let block = world.block(hit.block)?;
            let def = world.registry.def(block);
            let mut named: Vec<&str> = def.tags.iter().map(String::as_str).collect();
            named.push(&def.name);
            let ready = content.stations.of(block).is_some_and(|(_, phase)| phase == voxmar_engine::station::Phase::Ready);
            prompts.key(name, &held_tags, &named, ready)
        }
    });
    let Some(Some(said)) = said else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let words = locale.get(said);
    if let Ok(mut text) = word.single_mut()
        && text.0 != words
    {
        text.0 = words.to_owned();
    }
    let cap = locale.get("inventory.key.right_click");
    if let Ok(mut text) = key.single_mut()
        && text.0 != cap
    {
        text.0 = cap.to_owned();
    }
    visibility.set_if_neq(Visibility::Inherited);
}

fn hide(mut prompt: Query<&mut Visibility, With<UsePrompt>>) {
    for mut visibility in &mut prompt {
        *visibility = Visibility::Hidden;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    /// Every action has a prompt or says it has none, and every prompt's words are in
    /// every language.
    #[test]
    fn every_action_has_a_prompt_or_says_none_in_every_language() {
        let content = voxmar_server::load_content(&assets()).unwrap();
        let prompts = Prompts::load(&content.files).unwrap();
        prompts.check(&content.actions).unwrap();
        for code in ["en", "ja"] {
            let lang = voxmar_engine::lang::Lang::load(&assets().join("content"), code);
            for key in prompts.keys() {
                assert!(lang.find(key).is_some(), "{code}: no words for {key}");
            }
        }
    }

    /// A prompt missing for an action, or for one that does not exist, or saying both
    /// words and none, fails the load.
    #[test]
    fn a_missing_or_double_prompt_is_refused() {
        let content = voxmar_server::load_content(&assets()).unwrap();
        let mut prompts = Prompts::load(&content.files).unwrap();
        let light = prompts.actions.remove("base:light").unwrap();
        assert!(prompts.check(&content.actions).unwrap_err().contains("base:light"));
        prompts.actions.insert("base:light".into(), Prompt { none: true, ..light.clone() });
        assert!(prompts.check(&content.actions).is_err(), "words and none: true");
        prompts.actions.insert("base:light".into(), light);
        prompts.actions.insert("base:juggle".into(), Prompt { none: true, ..Prompt::default() });
        assert!(prompts.check(&content.actions).unwrap_err().contains("base:juggle"));
    }

    /// An open door says Close, shears at a sheep Shear, a rack done drying Take; else
    /// the action's own word.
    #[test]
    fn the_hand_and_the_block_choose_the_word() {
        let content = voxmar_server::load_content(&assets()).unwrap();
        let prompts = Prompts::load(&content.files).unwrap();
        assert_eq!(prompts.key("base:open", &[], &["door", "door_open"], false), Some("prompt.base.close"));
        assert_eq!(prompts.key("base:open", &[], &["door", "door_shut"], false), Some("prompt.base.open"));
        assert_eq!(prompts.key("base:tend", &["shears".into()], &[], false), Some("prompt.base.shear"));
        assert_eq!(prompts.key("base:tend", &[], &[], false), Some("prompt.base.feed"));
        assert_eq!(prompts.key("base:use_station", &[], &["base:drying_rack_dry"], true), Some("prompt.base.take"));
        assert_eq!(prompts.key("core:mine", &[], &[], false), None);
    }
}
