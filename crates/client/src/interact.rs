//! Block targeting, breaking, placing and using held items, and hotbar selection.
//!
//! Edits are proposed to the server as actions and predicted locally for responsiveness;
//! if the server disagrees it replies with the authoritative state, which overwrites ours.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use voxmar_engine::action::{ActionContext, ActionId, ActionTarget, Effect, Rejection, Requirement, placed_block, placement_cell, supported_by};
use voxmar_engine::block::{BlockId, BlockInfo, BlockRegistry};
use voxmar_engine::content::Content;
use voxmar_engine::crop::CropRegistry;
use voxmar_engine::skill::SkillId;
use voxmar_engine::inventory::{HOTBAR_SLOTS, SLOTS};
use voxmar_engine::physics::{Aabb, RayHit, player as dims, raycast};

use crate::lang::Locale;
use crate::Actions;
use crate::breaking::{BlockBroken, shows_breaking};
use crate::hud::skills::SkillToasts;
use crate::session::Launch;
use crate::net::Net;
use crate::player::{self, LocalPlayer, UiState, Zoom};
use crate::sfx::{Cue, SoundCue};
use crate::world::ClientWorld;

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Target>()
            .init_resource::<crate::fight::Fighting>()
            .add_systems(
                Update,
                (select_slot.after(player::zoom), find_target, crate::creatures::aim, pick_block, break_and_place, crate::fight::guard, crate::fight::quick_prayers, draw_target)
                    .chain()
                    .in_set(InteractSystems)
                    .run_if(in_state(crate::menu::Screen::InGame)),
            );
    }
}

/// Targeting, mining and placing, which effects of them run after.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InteractSystems;

#[derive(Resource, Default)]
pub struct Target {
    pub hit: Option<RayHit>,
    /// The block under the crosshair whether or not a creature is in front of it.
    pub block_hit: Option<RayHit>,
    /// The creature under the crosshair, nearer than any block (see `creatures::aim`).
    pub creature: Option<u32>,
    /// The right press this frame did something (a table, a bed, cooking, eating), so it
    /// raises no guard (`fight`).
    pub used_right: bool,
    /// Block being broken and progress in 0..1.
    pub breaking: Option<(IVec3, f32)>,
    /// Until when (seconds of game time) the player is eating: no dodge before.
    pub eating_until: f32,
    /// The target the mining in progress was proposed at, which its `Finished` names.
    mining: Option<ActionTarget>,
    mining_seconds: f32,
    place_cooldown: f32,
    /// When (seconds of game time) the line cast last comes up, and another may be cast.
    fishing_until: f32,
    /// When (seconds of game time) what is cooking comes off the fire, and more may go on.
    cooking_until: f32,
    /// The right button has been held since its press was spent on something else than
    /// setting a block down (a cook put on or refused, a clamp lit, a seat sat on): what is
    /// in hand (what is cooking or came off, the campfire that lit the clamp) is never set
    /// down by it until it is let go.
    press_spent: bool,
    /// Seconds until the next block may be mined after one breaks, as Minecraft's five
    /// ticks, so sweeping across a meadow does not mow it flat.
    mine_cooldown: f32,
    /// A smouldering structure the player was warned that breaking opens early (its core),
    /// and until when (seconds of game time) a fresh click there goes ahead.
    open_early: Option<(IVec3, f32)>,
}

impl Target {
    /// How long the block being mined takes to break, as worked out when the mining began.
    pub fn mining_seconds(&self) -> Option<f32> {
        self.mining.is_some().then_some(self.mining_seconds)
    }
}

/// How long after the warning a second click opens a smouldering structure early.
const OPEN_EARLY_SECONDS: f32 = 4.0;

/// The smouldering structure breaking `at` would open before its time, by its core, and
/// the warning that says what that costs: what it makes now, not what it would.
fn opens_early(world: &ClientWorld, content: &voxmar_engine::content::Content, locale: &Locale, at: IVec3) -> Option<(IVec3, String)> {
    let (core, structure) = content.multiblocks.smoulder_round(at, |p| world.block(p))?;
    let smoulder = structure.smoulder.as_ref()?;
    if !world.block(core).is_some_and(|b| smoulder.is_burning(b)) {
        return None;
    }
    let name = |block: BlockId| locale.block(content.blocks.def(content.blocks.info(block).drops));
    let title = locale.get_or(&voxmar_engine::lang::def_key("multiblock", &structure.name), &structure.title).to_owned();
    Some((core, locale.format("multiblock.open_early", &[("structure", &title), ("early", &name(smoulder.early)), ("done", &name(smoulder.done))])))
}

/// The number keys pick a hotbar slot, and so does the wheel unless it is zooming.
fn select_slot(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiState>,
    zoom: Res<Zoom>,
    mut player: ResMut<LocalPlayer>,
) {
    if !ui.cursor_locked {
        return;
    }
    const DIGITS: [KeyCode; HOTBAR_SLOTS] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if let Some(slot) = DIGITS.iter().position(|k| keys.just_pressed(*k)) {
        player.selected = slot;
    }
    if scroll.delta.y.abs() > 0.0 && !zoom.held {
        let step = if scroll.delta.y > 0.0 { HOTBAR_SLOTS - 1 } else { 1 };
        player.selected = (player.selected + step) % HOTBAR_SLOTS;
    }
}

/// The middle button picks the block looked at: it selects the hotbar slot holding it, or
/// swaps it into the selected slot from the rest of the inventory, as in Minecraft.
fn pick_block(
    mouse: Res<ButtonInput<MouseButton>>,
    ui: Res<UiState>,
    target: Res<Target>,
    world: Res<ClientWorld>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut player: ResMut<LocalPlayer>,
) {
    if !ui.cursor_locked || !mouse.just_pressed(MouseButton::Middle) {
        return;
    }
    let Some(block) = target.hit.and_then(|hit| world.block(hit.block)) else { return };
    let item = world.registry.info(block).drops;
    let item = if item.is_air() { block } else { item };
    let holds = |slot: usize| player.inventory.get(slot).is_some_and(|s| s.item == item);
    if let Some(slot) = (0..HOTBAR_SLOTS).find(|&slot| holds(slot)) {
        player.selected = slot;
    } else if let Some(slot) = (HOTBAR_SLOTS..SLOTS).find(|&slot| holds(slot))
        && let Some(transfer) = actions.named("core:transfer")
    {
        let selected = player.selected;
        let count = player.inventory.get(slot).map_or(0, |s| s.count);
        if player.inventory.move_items(slot, selected, count) {
            let target = ActionTarget::Slots { from: slot as u8, to: selected as u8, count };
            net.propose(transfer, target);
        }
    }
}

fn find_target(world: Res<ClientWorld>, player: Res<LocalPlayer>, mut target: ResMut<Target>) {
    // Water is looked through; ice and other solid see-through blocks can be mined.
    target.hit = raycast(player.eye(), player.look_direction(), dims::REACH, |p| {
        world.block(p).is_some_and(|b| shows_breaking(world.registry.info(b)))
    });
    target.block_hit = target.hit;
}

/// The client's view for checking actions before proposing them, using the same rules
/// the server applies.
struct ClientContext<'a> {
    world: &'a ClientWorld,
    player: &'a LocalPlayer,
    content: &'a Content,
}

impl ActionContext for ClientContext<'_> {
    fn block(&self, pos: IVec3) -> Option<BlockId> {
        self.world.block(pos)
    }

    fn info(&self, block: BlockId) -> &BlockInfo {
        self.world.registry.info(block)
    }

    fn blocks(&self) -> &BlockRegistry {
        &self.world.registry
    }

    fn crops(&self) -> &CropRegistry {
        &self.content.crops
    }

    fn actor_level(&self, skill: SkillId) -> u8 {
        voxmar_engine::level::check_level(&self.content.skills, &self.content.effects, &self.player.skills, &self.player.vitals.effects, skill)
    }

    fn actor_own_level(&self, skill: SkillId) -> u8 {
        self.content.skills.level(self.player.skills.xp(skill))
    }

    fn actor_eye(&self) -> Vec3 {
        self.player.eye()
    }

    fn actor_alive(&self) -> bool {
        !self.player.dead
    }

    fn actor_held(&self) -> Option<BlockId> {
        self.player.inventory.get(self.player.selected).map(|s| s.item)
    }

    fn actor_pouched(&self) -> Vec<BlockId> {
        self.player.inventory.pouched.iter().map(|&(item, _)| item).collect()
    }

    fn actor_carried(&self) -> Vec<BlockId> {
        self.player.inventory.slots.iter().flatten().map(|s| s.item).collect()
    }

    fn actor_hurt(&self) -> bool {
        self.player.health < self.player.max_health
    }

    fn actor_mining_speed(&self) -> f32 {
        self.content.effects.modifiers(&self.player.vitals.effects).mining
    }

    fn occupied(&self, cell: IVec3) -> bool {
        Aabb::standing(self.player.position, dims::WIDTH, dims::HEIGHT).intersects(|c| c == cell)
    }

    fn cooking(&self) -> Option<&voxmar_engine::cooking::CookingRegistry> {
        Some(&self.content.cooking)
    }

    fn tools(&self) -> Option<&voxmar_engine::tool::ToolRegistry> {
        Some(&self.content.tools)
    }

    fn flames(&self) -> Option<&voxmar_engine::flame::Flames> {
        Some(&self.content.flames)
    }

    fn machines(&self) -> Option<&voxmar_engine::machine::MachineRegistry> {
        Some(&self.content.machines)
    }

    fn multiblocks(&self) -> Option<&voxmar_engine::multiblock::MultiblockRegistry> {
        Some(self.content.multiblocks.as_ref())
    }

    fn bees(&self) -> Option<&voxmar_engine::bees::Bees> {
        Some(&self.content.bees)
    }
    fn stations(&self) -> Option<&voxmar_engine::station::Stations> {
        Some(&self.content.stations)
    }
    fn traps(&self) -> Option<&voxmar_engine::trap::Traps> {
        Some(&self.content.traps)
    }

    fn combat(&self) -> Option<&voxmar_engine::combat::CombatRegistry> {
        Some(&self.content.combat)
    }
}

/// Whether a right click with food in hand eats it: not with a creature under the
/// crosshair (it is tended), not over a block the food would be placed on, and not over
/// ground the food is a seed for (`sowing`, the plant action's check there): rice on
/// farmland is sown, or refused for the sowing's reason (a Farming level), never eaten or
/// answered with "you are at full health".
fn eats(food: bool, placeable: bool, on_creature: bool, on_block: bool, sowing: Option<&Result<(), Rejection>>) -> bool {
    let sows = matches!(sowing, Some(Ok(()) | Err(Rejection::Unmet(Requirement::SkillLevel))));
    food && !on_creature && (!on_block || !placeable) && !sows
}

/// Mining proposes `core:mine` and shows progress over the same duration the server
/// uses; releasing the button or looking away cancels it. Placing proposes `core:place`.
/// Both are predicted locally and corrected by the server if it disagrees.
#[allow(clippy::too_many_arguments)]
fn break_and_place(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut ui: ResMut<UiState>,
    net: Res<Net>,
    actions: Res<Actions>,
    mut world: ResMut<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut target: ResMut<Target>,
    mut cues: MessageWriter<SoundCue>,
    mut broken: MessageWriter<BlockBroken>,
    (launch, mut toasts, locale): (Res<Launch>, ResMut<SkillToasts>, Res<Locale>),
) {
    target.place_cooldown -= time.delta_secs();
    target.mine_cooldown -= time.delta_secs();
    if !mouse.pressed(MouseButton::Right) {
        target.press_spent = false;
    }
    let hit = target.hit.filter(|_| ui.cursor_locked && !player.dead);
    // A creature wandering across the block being mined does not stop the mining: only a
    // fresh click strikes it (`creatures::aim`).
    let still_mining = target.block_hit.filter(|h| target.breaking.is_some_and(|(pos, _)| pos == h.block));
    let mining = hit.or(still_mining).filter(|_| ui.cursor_locked && !player.dead && mouse.pressed(MouseButton::Left));

    // Stop mining when the button is released or the target changes.
    if let Some((pos, _)) = target.breaking
        && mining.is_none_or(|hit| hit.block != pos)
    {
        target.breaking = None;
        net.cancel();
    }

    if let Some(hit) = mining
        && let Some(mine) = actions.named("core:mine")
        && let Some(def) = actions.0.get(mine)
    {
        let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
        let context = ClientContext { world: &world, player: &player, content: &launch.content };
        match target.breaking {
            Some((pos, progress)) if pos == hit.block => {
                let step = time.delta_secs() / target.mining_seconds.max(0.05);
                let block = world.block(pos).unwrap_or(BlockId::AIR);
                // A tap sound four times a second while mining; the last one is the break.
                let taps = (target.mining_seconds * 4.0).max(1.0);
                if progress + step < 1.0 && ((progress + step) * taps).floor() > (progress * taps).floor() {
                    cues.write(SoundCue::for_block(&world, block, Cue::Hit));
                }
                let progress = progress + step;
                if progress >= 1.0 {
                    // The server commits at the same moment by the time the proposal reached
                    // it; predict it, tell the server it shows done here (it completes or
                    // undoes it, whatever comes next), and wait Minecraft's five ticks.
                    target.mine_cooldown = 0.25;
                    net.finished(target.mining.take().unwrap_or(action_target));
                    predict_break(&mut world, &player, &launch.content, hit.block, &mut broken);
                    target.breaking = None;
                } else {
                    target.breaking = Some((pos, progress));
                }
            }
            // A block that asks for a skill level or a better tool says so on a fresh click.
            _ if mouse.just_pressed(MouseButton::Left)
                && let Err(Rejection::Unmet(need)) = def.validate(&context, action_target)
                && let Some(words) = refusal(&context, &locale, Some(hit.block), need) =>
            {
                toasts.refused(words, time.elapsed_secs());
            }
            // A piece of a structure still smouldering (a lit clamp's earth or flue) opens it
            // early, spoiling what it chars: the first fresh click says so, and only one
            // more soon after goes ahead.
            _ if target.mine_cooldown <= 0.0
                && let Some((core, words)) = opens_early(&world, &launch.content, &locale, hit.block)
                && target.open_early.is_none_or(|(warned, until)| warned != core || time.elapsed_secs() > until || !mouse.just_pressed(MouseButton::Left)) =>
            {
                if mouse.just_pressed(MouseButton::Left) {
                    target.open_early = Some((core, time.elapsed_secs() + OPEN_EARLY_SECONDS));
                    toasts.refused(words, time.elapsed_secs());
                }
            }
            _ if target.mine_cooldown <= 0.0 && def.validate(&context, action_target).is_ok() => {
                target.open_early = None;
                target.mining_seconds = def.seconds(&context, action_target);
                target.mining = Some(action_target);
                net.propose_from(&player, mine, action_target);
                // Plants and other blocks that break at once go now, as on the server,
                // whose word would otherwise arrive before a predicted break.
                if target.mining_seconds <= 0.0 {
                    predict_break(&mut world, &player, &launch.content, hit.block, &mut broken);
                    target.mine_cooldown = 0.25;
                } else {
                    target.breaking = Some((hit.block, 0.0));
                }
            }
            _ => {}
        }
    }

    // Food in hand is eaten when it would not be placed: an item, or nothing in reach to
    // put it on. Eating takes a moment, so it starts on the press, not again while held.
    // (`used_right` was cleared by `creatures::aim`, which runs first.)
    if mouse.just_pressed(MouseButton::Right) && ui.cursor_locked && !player.dead {
        let content = &launch.content;
        // A crafting table opens the inventory with its three-by-three grid.
        if let (Some(hit), Some(use_table)) = (hit, actions.0.id("base:use_table")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            if actions.0.get(use_table).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                net.propose_from(&player, use_table, action_target);
                target.used_right = true;
                player.station = Some(hit.block);
                ui.inventory_open = true;
                return;
            }
        }
        // A store (a chest, a cupboard, a jar) opens beside the inventory once the server
        // says what it holds.
        if let (Some(hit), Some(open_store)) = (hit, actions.0.id("base:open_store")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            if actions.0.get(open_store).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                net.propose_from(&player, open_store, action_target);
                target.used_right = true;
                player.store_asked = true;
                target.press_spent = true;
                return;
            }
        }
        // A machine takes what it burns or works from the hand, or gives what it made; the
        // server says how it is doing. Anything else in hand is placed against it, on a press
        // of its own: what is left in hand after one it took from is not laid on it.
        if let (Some(hit), Some(use_machine)) = (hit, actions.0.id("base:use_machine")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            if actions.0.get(use_machine).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                net.propose_from(&player, use_machine, action_target);
                target.place_cooldown = 0.4;
                target.press_spent = true;
                return;
            }
        }
        // A multiblock's core is lit (the server says whether the structure around it
        // stands); a sleeping bag is slept in; a seat is sat on; a locked chest's lock is picked; bees give a
        // swarm to a skep held up to them, or a full skep its honey; a home station is
        // loaded or emptied; a sprung trap's catch is taken; a fertiliser is spread on a
        // growing crop; a wattle panel is daubed with clay in hand (as the server turns it); a
        // door is opened or shut, as the server turns it (never predicted: it holds the
        // collision); a torch's stub is lit again by a firebrand or torch.
        // One of these plainly meant (a door, a hive, a station aimed at, a fertiliser in
        // hand) but refused says why, unless something else below takes the click.
        let mut unmet = None;
        for name in ["base:light", "base:relight", "base:sleep", "base:sit", "base:pick_lock", "base:tend_bees", "base:use_station", "base:take_from_trap", "base:fertilise", "base:coat", "base:open"] {
            if let (Some(hit), Some(action)) = (hit, actions.0.id(name)) {
                let context = ClientContext { world: &world, player: &player, content };
                let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
                // With something it smelts in hand, a crucible smelts it (below) instead.
                let cooks = actions.0.id("base:cook").and_then(|cook| actions.0.get(cook)).is_some_and(|def| def.validate(&context, action_target).is_ok());
                match actions.0.get(action).map(|def| def.validate(&context, action_target)) {
                    Some(Ok(())) if !cooks => {
                        net.propose_from_with(&player, action, action_target, worked(&context, hit.block));
                        target.used_right = true;
                        target.press_spent = true;
                        // A lock picked opens the chest's slots.
                        player.store_asked |= name == "base:pick_lock";
                        // The clay a coat spends stays in hand until the server turns the
                        // wall, so the held press must not lay it as a block meanwhile.
                        if name == "base:coat" {
                            target.place_cooldown = 0.4;
                        }
                        return;
                    }
                    Some(Err(Rejection::Unmet(need))) if unmet.is_none() && meant(&context, hit.block, need) => {
                        unmet = refusal(&context, &locale, Some(hit.block), need);
                    }
                    _ => {}
                }
            }
        }
        if let (Some(why), Some(hit)) = (unmet, hit) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            let takes = |name: &str| actions.0.id(name).and_then(|id| actions.0.get(id)).is_some_and(|def| def.validate(&context, action_target).is_ok());
            let held = context.actor_held();
            let own = held.and_then(|item| world.registry.def(item).action.as_deref()).is_some_and(|name| takes(name));
            let placed = held.is_some_and(|item| world.registry.info(item).placeable) && takes("core:place");
            let food = held.is_some_and(|item| world.registry.info(item).food > 0);
            if !(takes("base:cook") || takes("base:pick") || own || placed || food) {
                toasts.refused(why, time.elapsed_secs());
                return;
            }
        }
        // Using what a station cooks on that station cooks it, before eating or placing.
        if let (Some(hit), Some(cook)) = (hit, actions.0.id("base:cook")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            match actions.0.get(cook).map(|def| def.validate(&context, action_target)) {
                // One thing at a time: a click while it cooks neither restarts it nor eats.
                Some(Ok(())) if time.elapsed_secs() < target.cooking_until => {
                    target.press_spent = true;
                    return;
                }
                Some(Ok(())) => {
                    let seconds = actions.0.get(cook).map_or(0.0, |def| def.seconds(&context, action_target));
                    net.propose_from_with(&player, cook, action_target, worked(&context, hit.block));
                    target.used_right = true;
                    target.place_cooldown = 0.4;
                    target.cooking_until = time.elapsed_secs() + seconds + 0.2;
                    target.press_spent = true;
                    return;
                }
                // Below its level, or with no tool about (an axe for the chopping block): it
                // says why, and what is held is not set down on the station instead.
                Some(Err(Rejection::Unmet(need @ (Requirement::CookingLevel | Requirement::CookingTool)))) => {
                    if let Some(words) = refusal(&context, &locale, Some(hit.block), need) {
                        toasts.refused(words, time.elapsed_secs());
                    }
                    target.press_spent = true;
                    return;
                }
                _ => {}
            }
        }
        let held = player.inventory.get(player.selected).map(|s| s.item);
        let food = held.is_some_and(|item| world.registry.info(item).food > 0);
        let placeable = held.is_some_and(|item| world.registry.info(item).placeable);
        // A seed that is also food (rice) on ground it can be sown in is sown, below.
        let sowing = hit.zip(actions.0.id("base:plant")).and_then(|(hit, plant)| {
            let context = ClientContext { world: &world, player: &player, content };
            actions.0.get(plant).map(|def| def.validate(&context, ActionTarget::Block { block: hit.block, face: hit.face }))
        });
        // With a creature under the crosshair, a right click tends it instead (`creatures::aim`).
        if eats(food, placeable, target.creature.is_some(), hit.is_some(), sowing.as_ref()) {
            let Some(eat) = actions.named("core:eat") else { return };
            let context = ClientContext { world: &world, player: &player, content };
            match actions.0.get(eat).map(|def| def.validate(&context, ActionTarget::None)) {
                Some(Ok(())) => {
                    // Eating takes the player's hands: the server drops (or, if nearly
                    // done, completes) any mining it replaces, and so does the client.
                    net.propose_from(&player, eat, ActionTarget::None);
                    if let Some(voxmar_engine::action::Duration::Seconds(seconds)) = actions.0.get(eat).map(|def| def.duration) {
                        target.eating_until = time.elapsed_secs() + seconds;
                    }
                    target.used_right = true;
                    target.breaking = None;
                    target.mine_cooldown = 1.2;
                    target.place_cooldown = 1.2;
                }
                Some(Err(Rejection::Unmet(need))) => {
                    if let Some(words) = refusal(&context, &locale, None, need) {
                        toasts.refused(words, time.elapsed_secs());
                    }
                }
                _ => {}
            }
            return;
        }
        // Armour or a shield in hand goes on, swapped for what was worn there, unless a
        // creature or a ripe crop is looked at (it is tended or picked).
        let ripe = hit.zip(actions.0.id("base:pick")).is_some_and(|(hit, pick)| {
            let context = ClientContext { world: &world, player: &player, content };
            actions.0.get(pick).is_some_and(|def| def.validate(&context, ActionTarget::Block { block: hit.block, face: hit.face }).is_ok())
        });
        if let Some(at) = held.and_then(voxmar_engine::inventory::worn_slot).filter(|_| target.creature.is_none() && !ripe) {
            let to = voxmar_engine::inventory::WORN + at;
            let slots = ActionTarget::Slots { from: player.selected as u8, to: to as u8, count: 1 };
            net.propose_from(&player, actions.0.expect("core:transfer"), slots);
            let selected = player.selected;
            player.inventory.move_items(selected, to, 1);
            target.used_right = true;
            target.place_cooldown = 0.4;
            return;
        }
    }
    // A rod in hand casts into the water looked at, again each time the line comes up while
    // the button is held.
    if mouse.pressed(MouseButton::Right) && ui.cursor_locked && !player.dead && time.elapsed_secs() >= target.fishing_until {
        let content = &launch.content;
        let fish = actions.0.id("base:fish").filter(|_| player.inventory.get(player.selected).is_some_and(|s| world.registry.def(s.item).has_tag("rod")));
        if let Some(fish) = fish {
            let water = raycast(player.eye(), player.look_direction(), voxmar_engine::action::CAST, |p| {
                world.block(p).is_some_and(|b| world.registry.info(b).solid || world.registry.info(b).fluid.is_some())
            });
            let context = ClientContext { world: &world, player: &player, content };
            // Cast at no water, or water it cannot reach: said once per click.
            let say_why = |toasts: &mut SkillToasts| {
                if mouse.just_pressed(MouseButton::Right)
                    && let Some(words) = refusal(&context, &locale, None, Requirement::Fishable)
                {
                    toasts.refused(words, time.elapsed_secs());
                }
            };
            if water.is_none() {
                say_why(&mut toasts);
            }
            if let Some(water) = water {
                let action_target = ActionTarget::Block { block: water.block, face: water.face };
                if actions.0.get(fish).is_some_and(|def| def.validate(&context, action_target).is_err()) {
                    say_why(&mut toasts);
                }
                if actions.0.get(fish).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                    let seconds = actions.0.get(fish).map_or(5.0, |def| def.seconds(&context, action_target));
                    net.propose_from(&player, fish, action_target);
                    target.fishing_until = time.elapsed_secs() + seconds + 0.2;
                    cues.write(SoundCue::for_block(&world, water_block(&world, water.block), Cue::Hit));
                }
            }
            return;
        }
    }
    let Some(hit) = hit else { return };
    // Held on through a cook, or after lighting or using something, nothing repeats: neither
    // the log on the chopping block nor the firewood it gives is laid on it, nor the campfire
    // that lit a clamp in its flue.
    let cooking = target.press_spent || time.elapsed_secs() < target.cooking_until;
    if mouse.pressed(MouseButton::Right) && target.place_cooldown <= 0.0 && !cooking {
        target.place_cooldown = 0.2;
        let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
        let content = &launch.content;
        let context = ClientContext { world: &world, player: &player, content };
        let valid = |id: ActionId| actions.0.get(id).map(|def| def.validate(&context, action_target));
        // A ripe crop that fruits again is picked, whatever is in hand; an item with an
        // action of its own (a seed plants) does that; anything else is placed.
        let held = context.actor_held();
        // A ripe crop above the player's Farming level says so, and is not built on.
        if let Some(Some(Err(Rejection::Unmet(need @ Requirement::CropLevel)))) = actions.0.id("base:pick").map(valid) {
            if let Some(words) = refusal(&context, &locale, Some(hit.block), need) {
                toasts.refused(words, time.elapsed_secs());
            }
            return;
        }
        let pick = actions.0.id("base:pick").filter(|&id| valid(id).is_some_and(|v| v.is_ok()));
        // A seed pouch in hand sows on ground that takes its seeds, and empties elsewhere.
        let sow = actions.0.id("base:plant").filter(|&id| held.is_some_and(|h| world.registry.def(h).has_tag("pouch")) && valid(id).is_some_and(|v| v.is_ok()));
        let own = held.and_then(|item| world.registry.def(item).action.as_deref()).and_then(|name| actions.0.id(name));
        if let Some(action) = pick.or(sow).or(own) {
            match valid(action) {
                Some(Ok(())) => {
                    net.propose_from(&player, action, action_target);
                    let effects = actions.0.get(action).map_or(Vec::new(), |def| def.effects.clone());
                    predict_use(&mut world, &mut player, content, &effects, hit.block, &mut cues);
                }
                // Why the item's own action failed, once per click (the button repeats).
                Some(Err(Rejection::Unmet(need))) if mouse.just_pressed(MouseButton::Right) => {
                    if let Some(words) = refusal(&context, &locale, Some(hit.block), need) {
                        toasts.refused(words, time.elapsed_secs());
                    }
                }
                _ => {}
            }
            return;
        }
        let Some(place) = actions.named("core:place") else { return };
        let placing = actions.0.get(place).map(|def| def.validate(&context, action_target));
        let allowed = placing.as_ref().is_some_and(Result::is_ok);
        // A block in hand that cannot go there says why, once per click.
        if let Some(Err(Rejection::Unmet(need))) = placing
            && held.is_some_and(|item| world.registry.info(item).placeable)
            && mouse.just_pressed(MouseButton::Right)
            && let Some(words) = refusal(&context, &locale, Some(hit.block), need)
        {
            toasts.refused(words, time.elapsed_secs());
        }
        if let Some(item) = held.filter(|_| allowed) {
            let cell = placement_cell(&context, hit.block, hit.face);
            let placed = placed_block(&context, item, hit.block, hit.face);
            net.propose_from(&player, place, action_target);
            cues.write(SoundCue::for_block(&world, item, Cue::Place));
            world.set_block(cell, placed);
            if let Some(upper) = content.blocks.upper(placed) {
                world.set_block(cell + IVec3::Y, upper);
            }
            let selected = player.selected;
            player.inventory.take_one(selected);
        }
    }
}

/// Predicts what using an item or picking a crop does, as far as the client can know it:
/// a sown seed's sprout, a picked crop gone back to regrowing. Its harvest and experience
/// come from the server.
fn predict_use(world: &mut ClientWorld, player: &mut LocalPlayer, content: &Content, effects: &[Effect], target: IVec3, cues: &mut MessageWriter<SoundCue>) {
    for effect in effects {
        match effect {
            Effect::Plant => {
                let selected = player.selected;
                let Some(seed) = (ClientContext { world, player, content }).actor_seed() else { continue };
                let Some(crop) = content.crops.planted_by(seed) else { continue };
                let sprout = content.crops.get(crop).stages[0];
                if player.inventory.get(selected).is_some_and(|s| s.item == seed) {
                    player.inventory.take_one(selected);
                } else {
                    player.inventory.take_pouched(seed);
                }
                world.set_block(target + IVec3::Y, sprout);
                cues.write(SoundCue::for_block(world, sprout, Cue::Place));
            }
            Effect::Till => {
                if let Some(farmland) = world.registry.id("base:farmland") {
                    world.set_block(target, farmland);
                    cues.write(SoundCue::for_block(world, farmland, Cue::Place));
                }
            }
            Effect::Pick => {
                let Some((crop, _)) = world.block(target).and_then(|b| content.crops.stage_of(b)) else { continue };
                let crop = content.crops.get(crop);
                let Some(regrow) = crop.def.regrow else { continue };
                cues.write(SoundCue::for_block(world, crop.ripe(), Cue::Break));
                world.set_block(target, crop.stages[regrow as usize]);
            }
            _ => {}
        }
    }
}

/// The water block at `pos`, for its splash.
fn water_block(world: &ClientWorld, pos: IVec3) -> BlockId {
    world.block(pos).unwrap_or(BlockId::AIR)
}

/// What an action at `block` is done with, to be heard by (`sounds.ron`'s `ways`): the block
/// worked (a hearthstone lit, a drying rack loaded, a pit kiln fed), not what is in hand,
/// so holding a station's item never sounds as loading one.
fn worked(context: &ClientContext, block: IVec3) -> Vec<BlockId> {
    context.world.block(block).into_iter().collect()
}

/// What to tell a player whose click was refused because `need` failed, at `block` if it
/// aimed at one: the words `refusals.ron` gives it, with the skill and level it asks for or
/// the least tool that would do; none for a requirement that is silent there.
/// Whether a right click on `block` plainly meant the action whose requirement `need`
/// failed: the target is what it asks about (a door, a hive, a station) or the hand holds
/// what it spreads (a fertiliser). A click meant for something else says nothing.
fn meant(context: &ClientContext, block: IVec3, need: Requirement) -> bool {
    let Some(at) = context.world.block(block) else { return false };
    match need {
        Requirement::Swaps => !voxmar_engine::action::swapping(context, block).is_empty(),
        // Clay held to a panel someone leans on: plainly meant as a coat.
        Requirement::CoatClear => context.blocks().coating(at).is_some_and(|(with, _)| context.actor_held() == Some(with)),
        Requirement::Relights => context.content.flames.relit(&context.content.blocks, at).is_some(),
        Requirement::Bees => context.bees().is_some_and(|bees| bees.takes.iter().any(|take| take.block == at)),
        Requirement::Station => context.stations().is_some_and(|stations| stations.of(at).is_some()),
        Requirement::Fertilisable => context.actor_held().zip(context.stations()).is_some_and(|(item, stations)| stations.fertiliser(item).is_some()),
        _ => false,
    }
}

fn refusal(context: &ClientContext, locale: &Locale, block: Option<IVec3>, need: Requirement) -> Option<String> {
    let (world, content) = (context.world, context.content);
    let key = content.refusals.says(need)?;
    let level = |skill: SkillId, level: u8| Some(locale.format(key, &[("skill", &locale.skill(content.skills.get(skill))), ("level", &level)]));
    match need {
        // A ripe crop above the player's Farming level: the target, or one standing on it.
        Requirement::CropLevel => {
            let block = block?;
            let cell = std::iter::once(block).chain(supported_by(context, block)).find(|&c| !voxmar_engine::action::crop_level_met(context, c))?;
            let (crop, _) = content.crops.stage_of(world.block(cell)?)?;
            let crop = content.crops.get(crop);
            level(crop.skill, crop.def.level)
        }
        Requirement::BlockLevel => {
            let rule = content.tools.rule(world.block(block?)?)?;
            level(rule.skill?, rule.level)
        }
        Requirement::ToolGood => {
            let rule = content.tools.rule(world.block(block?)?)?;
            let tool = content.least_tool(rule.tool?, rule.tier)?;
            Some(locale.format(key, &[("tool", &locale.block(world.registry.def(tool)))]))
        }
        // The held seed's crop.
        Requirement::SkillLevel => {
            let crop = context.actor_seed().and_then(|seed| content.crops.planted_by(seed)).map(|id| content.crops.get(id))?;
            level(crop.skill, crop.def.level)
        }
        // The tool the held item's recipe at the station aimed at asks for: the first item
        // that is one.
        Requirement::CookingTool => {
            let station = world.block(block?).map(|b| world.registry.def(b).tags.clone()).unwrap_or_default();
            let tag = content.cooking.for_item(context.actor_held()?, &station)?.def.tool.clone()?;
            let tool = content.blocks.tagged(&tag).next()?;
            Some(locale.format(key, &[("tool", &locale.block(world.registry.def(tool)))]))
        }
        // The held item's recipe at the station aimed at.
        Requirement::CookingLevel => {
            let station = world.block(block?).map(|b| world.registry.def(b).tags.clone()).unwrap_or_default();
            let recipe = content.cooking.for_item(context.actor_held()?, &station)?;
            level(recipe.skill?, recipe.def.level)
        }
        _ => Some(locale.get(key).to_owned()),
    }
}

/// Breaks a block as the server will, with any plant that loses its ground; each sounds
/// and bursts (see `breaking`).
fn predict_break(world: &mut ClientWorld, player: &LocalPlayer, content: &Content, pos: IVec3, broken: &mut MessageWriter<BlockBroken>) {
    let plants = supported_by(&ClientContext { world, player, content }, pos);
    for pos in std::iter::once(pos).chain(plants) {
        if let Some(block) = world.block(pos).filter(|b| !b.is_air()) {
            broken.write(BlockBroken { pos, block });
        }
        world.set_block(pos, BlockId::AIR);
    }
}

fn draw_target(target: Res<Target>, mut gizmos: Gizmos) {
    match target.hit {
        Some(hit) => {
            let center = hit.block.as_vec3() + 0.5;
            gizmos.cube(Transform::from_translation(center).with_scale(Vec3::splat(1.004)), Color::srgba(0.0, 0.0, 0.0, 0.8));
        }
        // An invisible outline keeps the outline's pipelines in use from the first frame. On
        // Metal, Bevy compiles a pipeline on the render thread the first time something
        // needs it, and the first block the player looked at held that frame for 60 ms. It
        // is drawn as the outline is, a cube: a plain line warms only one of the two
        // pipelines the cube needs, and a world whose first block looked at came only after
        // loading compiled the other mid-play.
        None => gizmos.cube(Transform::from_scale(Vec3::ZERO), Color::NONE),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use voxmar_engine::action::ActionRegistry;
    use voxmar_engine::coords::ColumnPos;
    use voxmar_engine::world::Column;

    use super::*;
    use crate::render::terrain::Terrain;

    /// `break_and_place` alone, with the shipped content but `actions`, the player looking
    /// at the block beside their feet with the cursor locked.
    fn app(actions: Arc<ActionRegistry>) -> (App, IVec3, voxmar_protocol::ServerConnection) {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (connection, server_side) = voxmar_protocol::local_pair();
        let mut world = ClientWorld::new(content.blocks.clone(), 2, Terrain::detached());
        world.insert_column(ColumnPos::of_block(IVec3::ZERO), Arc::new(Column::default()));
        let player = LocalPlayer { position: Vec3::new(0.5, 100.0, 0.5), ..default() };
        let block = IVec3::new(1, 100, 0);
        let target = Target { hit: Some(RayHit { block, face: voxmar_engine::coords::Face::NegX, distance: 1.0 }), ..default() };
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(UiState { cursor_locked: true, ..default() })
            .insert_resource(Net::new(connection, None, "tester".into(), content.fingerprint(), None))
            .insert_resource(Actions(actions))
            .insert_resource(world)
            .insert_resource(player)
            .insert_resource(target)
            .add_message::<SoundCue>()
            .add_message::<BlockBroken>()
            .insert_resource(Locale::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"), "en"))
            .insert_resource(Launch { name: "tester".into(), host: false, content })
            .init_resource::<SkillToasts>()
            .add_systems(Update, break_and_place);
        (app, block, server_side)
    }

    /// A clamp lit from a campfire in hand, the button held on: the campfire is not set down
    /// in its flue after.
    #[test]
    fn holding_on_after_lighting_a_clamp_lays_nothing() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, _, server_side) = app(content.actions.clone());
        let clamp = content.multiblocks.iter().find(|m| m.smoulder.is_some()).expect("a clamp").clone();
        let core = IVec3::new(10, 100, 10);
        for (at, piece) in clamp.example() {
            app.world_mut().resource_mut::<ClientWorld>().set_block(core + at, piece);
        }
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block: core, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(content.blocks.expect("base:campfire"), 1));
        player.selected = 0;
        player.position = Vec3::new(10.5, 101.0, 8.5);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        for _ in 0..20 {
            app.update();
        }
        let proposed: Vec<ActionId> = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect();
        assert_eq!(proposed, [content.actions.expect("base:light")], "lit, and the campfire kept in hand");
    }

    /// A clamp's earth right clicked from the floor beside it, a campfire in hand, the
    /// button held on: lit, and the campfire never laid on its earth.
    #[test]
    fn a_clamp_clicked_on_its_earth_is_lit_not_built_on() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, _, server_side) = app(content.actions.clone());
        let clamp = content.multiblocks.iter().find(|m| m.smoulder.is_some()).expect("a clamp").clone();
        let core = IVec3::new(10, 100, 10);
        for (at, piece) in clamp.example() {
            app.world_mut().resource_mut::<ClientWorld>().set_block(core + at, piece);
        }
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block: core - IVec3::Z, face: voxmar_engine::coords::Face::NegZ, distance: 2.5 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(content.blocks.expect("base:campfire"), 1));
        player.selected = 0;
        player.position = Vec3::new(10.5, 99.0, 6.5);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        for _ in 0..20 {
            app.update();
        }
        let proposed: Vec<ActionId> = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect();
        assert_eq!(proposed, [content.actions.expect("base:light")], "lit, and the campfire kept in hand");
    }

    /// A lit clamp's earth clicked to dig: the first click only warns what opening it now
    /// costs, holding on does nothing more, and a second click soon after digs; an unlit
    /// clamp digs at once.
    #[test]
    fn opening_a_lit_clamp_early_asks_twice() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let mine = content.actions.expect("core:mine");
        let clamp = content.multiblocks.iter().find(|m| m.smoulder.is_some()).expect("a clamp").clone();
        let smoulder = clamp.smoulder.clone().unwrap();
        for lit in [true, false] {
            let (mut app, _, server_side) = app(content.actions.clone());
            let core = IVec3::new(10, 100, 10);
            for (at, piece) in clamp.example() {
                app.world_mut().resource_mut::<ClientWorld>().set_block(core + at, piece);
            }
            if lit {
                app.world_mut().resource_mut::<ClientWorld>().set_block(core, smoulder.burning);
            }
            app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block: core - IVec3::Z, face: voxmar_engine::coords::Face::NegZ, distance: 2.5 });
            app.world_mut().resource_mut::<LocalPlayer>().position = Vec3::new(10.5, 99.0, 6.5);
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
            let click = |app: &mut App, hold: usize| {
                app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
                app.update();
                app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
                for _ in 0..hold {
                    app.update();
                }
                app.world_mut().resource_mut::<ButtonInput<MouseButton>>().release(MouseButton::Left);
                app.update();
                std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter(|m| matches!(m, voxmar_protocol::ClientMessage::Propose { action, .. } if *action == mine)).count()
            };
            let first = click(&mut app, 10);
            let warned: Vec<String> = app.world().resource::<SkillToasts>().refusals().map(str::to_owned).collect();
            if lit {
                assert_eq!(first, 0, "the first click on a lit clamp digs nothing, held or not");
                assert!(matches!(warned.as_slice(), [why] if why.contains("Part-Burnt Wood") && why.contains("not Charcoal")), "{warned:?}");
                assert_eq!(click(&mut app, 0), 1, "a second click soon after digs");
            } else {
                assert_eq!(first, 1, "an unlit clamp digs at once");
                assert!(warned.is_empty(), "{warned:?}");
            }
        }
    }

    /// A chest opened with a block in hand, the button held on while its screen comes: the
    /// block is not laid against it.
    #[test]
    fn holding_on_after_opening_a_chest_lays_nothing() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, content.blocks.expect("base:chest"));
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(content.blocks.expect("base:cobblestone"), 4));
        player.selected = 0;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        for _ in 0..20 {
            app.update();
        }
        let proposed: Vec<ActionId> = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect();
        assert_eq!(proposed, [content.actions.expect("base:open_store")], "opened, and nothing laid against it");
    }

    /// A block put into a hopper, the button held on: it goes in, and nothing is laid
    /// against the hopper after.
    #[test]
    fn holding_on_after_using_a_machine_lays_nothing() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, content.blocks.expect("base:hopper"));
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(content.blocks.expect("base:cobblestone"), 4));
        player.selected = 0;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        for _ in 0..20 {
            app.update();
        }
        let proposed: Vec<ActionId> = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect();
        assert_eq!(proposed, [content.actions.expect("base:use_machine")], "put in, and nothing laid against it");
    }

    /// Content that leaves `core:mine` out (a partial mod): holding the button does
    /// nothing, frame after frame, and proposes nothing.
    #[test]
    fn holding_mine_without_a_mine_action_does_nothing() {
        let (mut app, block, server_side) = app(Arc::new(ActionRegistry::default()));
        let stone = app.world().resource::<Launch>().content.blocks.expect("base:stone");
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, stone);
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Left);
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(app.world().resource::<ClientWorld>().block(block), Some(stone));
        let proposed = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter(|m| matches!(m, voxmar_protocol::ClientMessage::Propose { .. })).count();
        assert_eq!(proposed, 0);
    }

    /// A ripe crop that fruits again, above the player's Farming level: right click says
    /// the level it needs and proposes nothing.
    #[test]
    fn a_ripe_crop_above_the_players_level_says_what_it_needs() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        let (_, crop) = content.crops.iter().find(|(_, c)| c.def.regrow.is_some() && c.def.level > 1).expect("a crop that fruits again above level 1");
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, crop.ripe());
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        let level = crop.def.level.to_string();
        let refusals: Vec<String> = app.world().resource::<SkillToasts>().refusals().into_iter().map(str::to_owned).collect();
        assert!(refusals.iter().any(|why| why.contains("Farming") && why.contains(&level)), "{refusals:?}");
        let proposed = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter(|m| matches!(m, voxmar_protocol::ClientMessage::Propose { .. })).count();
        assert_eq!(proposed, 0);
    }

    /// A log held to a chopping block with an axe about splits on the press; the button held
    /// on lays nothing there, neither the log while it splits nor the firewood once it is
    /// in hand. Let go and pressed again, the firewood is laid.
    #[test]
    fn holding_on_through_a_split_lays_nothing() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        let (cook, place) = (content.actions.expect("base:cook"), content.actions.expect("core:place"));
        let id = |name: &str| content.blocks.expect(name);
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, id("base:chopping_block"));
        // Aimed at its top, where what is in hand could be laid.
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(id("base:oak_log"), 1));
        player.inventory.slots[1] = Some(voxmar_engine::inventory::ItemStack::new(id("base:stone_axe"), 1));
        player.selected = 0;
        let proposed = |server_side: &voxmar_protocol::ServerConnection| -> Vec<ActionId> {
            std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect()
        };
        // Thirty milliseconds a frame, by the game's clock alone.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        let frames = |app: &mut App, n: usize| {
            for _ in 0..n {
                app.update();
            }
        };
        // Pressed for one frame, then held: the press is spent after the first frame.
        let hold = |app: &mut App, n: usize| {
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
            app.update();
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
            frames(app, n - 1);
        };
        hold(&mut app, 20);
        assert_eq!(proposed(&server_side), [cook], "the split, and the log not laid");
        // Split: the firewood in the emptied hand, the cook's time over, the button still down.
        app.world_mut().resource_mut::<LocalPlayer>().inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(id("base:firewood"), 2));
        app.world_mut().resource_mut::<Target>().cooking_until = 0.0;
        frames(&mut app, 20);
        assert_eq!(proposed(&server_side), [], "held on, the firewood is not laid");
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release(MouseButton::Right);
        mouse.clear();
        frames(&mut app, 2);
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        frames(&mut app, 2);
        assert!(proposed(&server_side).contains(&place), "let go and pressed again, it is laid");
    }

    /// A second click while the split still runs is held too: once the cook is over, the
    /// log is not laid on the chopping block while that click is held on.
    #[test]
    fn a_click_while_the_split_runs_lays_nothing_after_it() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        let cook = content.actions.expect("base:cook");
        let id = |name: &str| content.blocks.expect(name);
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, id("base:chopping_block"));
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(id("base:oak_log"), 2));
        player.inventory.slots[1] = Some(voxmar_engine::inventory::ItemStack::new(id("base:stone_axe"), 1));
        player.selected = 0;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        let proposed = |server_side: &voxmar_protocol::ServerConnection| -> Vec<ActionId> {
            std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter_map(|m| match m { voxmar_protocol::ClientMessage::Propose { action, .. } => Some(action), _ => None }).collect()
        };
        let click = |app: &mut App| {
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
            app.update();
            app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        };
        click(&mut app);
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release(MouseButton::Right);
        mouse.clear();
        app.update();
        assert_eq!(proposed(&server_side), [cook], "the split begun");
        // Clicked again while it runs, and held on past its end.
        click(&mut app);
        app.world_mut().resource_mut::<Target>().cooking_until = 0.0;
        for _ in 0..20 {
            app.update();
        }
        assert_eq!(proposed(&server_side), [], "held on past the split, the log is not laid");
    }

    /// A log held on the chopping block with no axe about says it needs one and is not laid
    /// on it, however long the button is held.
    #[test]
    fn a_split_refused_for_want_of_an_axe_lays_nothing() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (mut app, block, server_side) = app(content.actions.clone());
        let id = |name: &str| content.blocks.expect(name);
        app.world_mut().resource_mut::<ClientWorld>().set_block(block, id("base:chopping_block"));
        app.world_mut().resource_mut::<Target>().hit = Some(RayHit { block, face: voxmar_engine::coords::Face::PosY, distance: 1.0 });
        let mut player = app.world_mut().resource_mut::<LocalPlayer>();
        player.inventory.slots[0] = Some(voxmar_engine::inventory::ItemStack::new(id("base:oak_log"), 1));
        player.selected = 0;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(30)));
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().press(MouseButton::Right);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
        for _ in 0..20 {
            app.update();
        }
        let proposed: Vec<_> = std::iter::from_fn(|| server_side.try_recv().ok().flatten()).filter(|m| matches!(m, voxmar_protocol::ClientMessage::Propose { .. })).collect();
        assert!(proposed.is_empty(), "nothing proposed, the log not laid: {proposed:?}");
    }

    /// Food is eaten on a right click unless the click tends a creature, places the food,
    /// or sows it: rice over farmland is sown (or refused for its Farming level), over
    /// grass it is eaten.
    #[test]
    fn a_seed_that_is_food_is_sown_where_it_can_be_and_eaten_elsewhere() {
        let sown = Ok(());
        let too_low = Err(Rejection::Unmet(Requirement::SkillLevel));
        let not_ground = Err(Rejection::Unmet(Requirement::Plantable));
        assert!(!eats(true, false, false, true, Some(&sown)), "rice on farmland is sown");
        assert!(!eats(true, false, false, true, Some(&too_low)), "rice on farmland below its level is refused as a sowing");
        assert!(eats(true, false, false, true, Some(&not_ground)), "rice on grass is eaten");
        assert!(eats(true, false, false, false, None), "rice in the air is eaten");
        assert!(!eats(true, false, true, true, Some(&not_ground)), "a creature is tended");
        assert!(!eats(true, true, false, true, Some(&not_ground)), "a placeable food is placed");
        assert!(!eats(false, false, false, true, Some(&not_ground)), "what is no food is not eaten");
    }
}
