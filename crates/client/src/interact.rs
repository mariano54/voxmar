//! Block targeting, breaking, placing and using held items, and hotbar selection.
//!
//! Edits are proposed to the server as actions and predicted locally for responsiveness;
//! if the server disagrees it replies with the authoritative state, which overwrites ours.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use voxmar_engine::action::{ActionContext, ActionId, ActionTarget, Effect, Rejection, Requirement, placement_cell, supported_by};
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
                (select_slot.after(player::zoom), find_target, crate::creatures::aim, pick_block, break_and_place, crate::fight::guard, draw_target)
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
    block_hit: Option<RayHit>,
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
    /// Seconds until the next block may be mined after one breaks, as Minecraft's five
    /// ticks, so sweeping across a meadow does not mow it flat.
    mine_cooldown: f32,
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
    } else if let Some(slot) = (HOTBAR_SLOTS..SLOTS).find(|&slot| holds(slot)) {
        let selected = player.selected;
        let count = player.inventory.get(slot).map_or(0, |s| s.count);
        if player.inventory.move_items(slot, selected, count) {
            let target = ActionTarget::Slots { from: slot as u8, to: selected as u8, count };
            net.propose(actions.0.expect("core:transfer"), target);
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

    fn machines(&self) -> Option<&voxmar_engine::machine::MachineRegistry> {
        Some(&self.content.machines)
    }

    fn combat(&self) -> Option<&voxmar_engine::combat::CombatRegistry> {
        Some(&self.content.combat)
    }
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

    if let Some(hit) = mining {
        let mine = actions.0.expect("core:mine");
        let def = actions.0.get(mine).expect("core:mine is defined");
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
                && let Err(Rejection::Unmet(need @ (Requirement::BlockLevel | Requirement::CropLevel | Requirement::ToolGood))) = def.validate(&context, action_target) =>
            {
                if let Some(words) = refusal(&world, &launch.content, &locale, hit.block, need) {
                    toasts.refused(words, time.elapsed_secs());
                }
            }
            _ if target.mine_cooldown <= 0.0 && def.validate(&context, action_target).is_ok() => {
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
        // A machine takes what it burns or works from the hand, or gives what it made; the
        // server says how it is doing. Anything else in hand is placed against it.
        if let (Some(hit), Some(use_machine)) = (hit, actions.0.id("base:use_machine")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            if actions.0.get(use_machine).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                net.propose_from(&player, use_machine, action_target);
                target.place_cooldown = 0.4;
                return;
            }
        }
        // A multiblock's core is lit (the server says whether the structure around it
        // stands); a sleeping bag is slept in; a locked chest's lock is picked.
        for name in ["base:light", "base:sleep", "base:pick_lock"] {
            if let (Some(hit), Some(action)) = (hit, actions.0.id(name)) {
                let context = ClientContext { world: &world, player: &player, content };
                let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
                // With something it smelts in hand, a crucible smelts it (below) instead.
                let cooks = actions.0.id("base:cook").and_then(|cook| actions.0.get(cook)).is_some_and(|def| def.validate(&context, action_target).is_ok());
                if !cooks && actions.0.get(action).is_some_and(|def| def.validate(&context, action_target).is_ok()) {
                    net.propose_from(&player, action, action_target);
                    target.used_right = true;
                    return;
                }
            }
        }
        // Using what a station cooks on that station cooks it, before eating or placing.
        if let (Some(hit), Some(cook)) = (hit, actions.0.id("base:cook")) {
            let context = ClientContext { world: &world, player: &player, content };
            let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
            match actions.0.get(cook).map(|def| def.validate(&context, action_target)) {
                Some(Ok(())) => {
                    net.propose_from(&player, cook, action_target);
                    target.used_right = true;
                    target.place_cooldown = 0.4;
                    return;
                }
                Some(Err(Rejection::Unmet(Requirement::CookingLevel))) => {
                    let item = context.actor_held();
                    let station = world.block(hit.block).map(|b| world.registry.def(b).tags.clone()).unwrap_or_default();
                    if let (Some(recipe), Some(skill)) = { let recipe = item.and_then(|i| content.cooking.for_item(i, &station)); (recipe, recipe.and_then(|r| r.skill)) } {
                        let skill = locale.skill(content.skills.get(skill));
                        let words = locale.format("skills.needs", &[("skill", &skill), ("level", &recipe.def.level)]);
                        toasts.refused(words, time.elapsed_secs());
                    }
                    return;
                }
                _ => {}
            }
        }
        let held = player.inventory.get(player.selected).map(|s| s.item);
        let food = held.is_some_and(|item| world.registry.info(item).food > 0);
        let placeable = held.is_some_and(|item| world.registry.info(item).placeable);
        // With a creature under the crosshair, a right click tends it instead (`creatures::aim`).
        if food && target.creature.is_none() && (hit.is_none() || !placeable) {
            let eat = actions.0.expect("core:eat");
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
                Some(Err(Rejection::Unmet(Requirement::Hurt))) => toasts.refused(locale.get("skills.full").into(), time.elapsed_secs()),
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
            if let Some(water) = water {
                let action_target = ActionTarget::Block { block: water.block, face: water.face };
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
    if mouse.pressed(MouseButton::Right) && target.place_cooldown <= 0.0 {
        target.place_cooldown = 0.2;
        let action_target = ActionTarget::Block { block: hit.block, face: hit.face };
        let content = &launch.content;
        let context = ClientContext { world: &world, player: &player, content };
        let valid = |id: ActionId| actions.0.get(id).map(|def| def.validate(&context, action_target));
        // A ripe crop that fruits again is picked, whatever is in hand; an item with an
        // action of its own (a seed plants) does that; anything else is placed.
        let held = context.actor_held();
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
                Some(Err(Rejection::Unmet(Requirement::SkillLevel))) => {
                    if let Some(crop) = held.and_then(|item| content.crops.planted_by(item)).map(|id| content.crops.get(id)) {
                        let skill = locale.skill(content.skills.get(crop.skill));
                        let words = locale.format("skills.needs", &[("skill", &skill), ("level", &crop.def.level)]);
                        toasts.refused(words, time.elapsed_secs());
                    }
                }
                _ => {}
            }
            return;
        }
        let place = actions.0.expect("core:place");
        let allowed = actions.0.get(place).is_some_and(|def| def.validate(&context, action_target).is_ok());
        if let Some(item) = held.filter(|_| allowed) {
            let cell = placement_cell(&context, hit.block, hit.face);
            net.propose_from(&player, place, action_target);
            cues.write(SoundCue::for_block(&world, item, Cue::Place));
            world.set_block(cell, item);
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

/// What to tell a player whose click a block refused: the skill level it asks for, or the
/// least tool that would do.
fn refusal(world: &ClientWorld, content: &Content, locale: &Locale, block: IVec3, need: Requirement) -> Option<String> {
    // A ripe crop above the player's Farming level.
    if need == Requirement::CropLevel {
        let (crop, _) = content.crops.stage_of(world.block(block)?)?;
        let crop = content.crops.get(crop);
        let skill = locale.skill(content.skills.get(crop.skill));
        return Some(locale.format("skills.needs", &[("skill", &skill), ("level", &crop.def.level)]));
    }
    let rule = content.tools.rule(world.block(block)?)?;
    match need {
        Requirement::BlockLevel => {
            let skill = locale.skill(content.skills.get(rule.skill?));
            Some(locale.format("skills.needs", &[("skill", &skill), ("level", &rule.level)]))
        }
        _ => {
            let tool = content.tools.first_of(rule.tool?, rule.tier)?;
            Some(locale.format("tools.needs", &[("tool", &locale.block(world.registry.def(tool)))]))
        }
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
