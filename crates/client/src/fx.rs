//! What the player's actions look like as they do them, the visual twin of the sounds of
//! actions (`sfx`): dust settling round a block set down, crumbs falling as food is bitten,
//! leaves springing off a crop picked, soil kicked up by a hoe or a seed, chips off a table
//! or a station worked, steam off food on the fire, sparks off flint, motes at an altar, and
//! a glint over what the grid made.
//!
//! `action_fx.ron` names how every action shows: an effect, another part of the game that
//! shows it (breaking, the fight), or nothing; the launch fails on an action it says nothing
//! of, and on one it names that the content lacks, as `sounds.ron` does for sounds. Each
//! effect is data too: where it shows, what its particles look like and how they move,
//! spawned through `render::particles` and scaled by the particle setting.

use bevy::prelude::*;
use serde::Deserialize;
use std::collections::BTreeMap;
use voxmar_engine::action::{ActionId, ActionRegistry, ActionTarget};
use voxmar_engine::block::{BlockId, ParticleBlend, RenderKind};
use voxmar_engine::content::ContentFile;
use voxmar_engine::coords::Face;

use crate::breaking::{Dice, EffectSystems, light_at, piece};
use crate::menu::Screen;
use crate::net::Net;
use crate::player::LocalPlayer;
use crate::render::particles::{Look, Motion, Particle, Particles, TexturePieces};
use crate::session::Launch;
use crate::world::ClientWorld;

const FILE: &str = "action_fx.ron";

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pending>()
            .init_resource::<FxSeen>()
            .add_systems(Startup, load)
            .add_systems(Update, (show_actions, spawn_pending, fade_glints).chain().in_set(EffectSystems).run_if(in_state(Screen::InGame)));
    }
}

/// Every package's `action_fx.ron`: how each action shows, and the effects by name. A later
/// package replaces an action or an effect it names.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ActionFx {
    pub(crate) actions: BTreeMap<String, Shows>,
    pub(crate) effects: BTreeMap<String, Effect>,
}

/// How one action shows: one of the three.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Shows {
    pub(crate) effect: Option<String>,
    pub(crate) shown: Option<ShownBy>,
    pub(crate) none: bool,
    /// Its effect waits for the server to turn the block aimed at, for an action the client
    /// does not show itself (clay pressed on): refused, or not turned within
    /// [`CONFIRM_WITHIN`], it shows nothing.
    pub(crate) confirmed: bool,
}

/// How long a `confirmed` effect waits for the server to turn its block, in seconds.
const CONFIRM_WITHIN: f32 = 1.5;

/// Where an action's look comes from when not from an effect here.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum ShownBy {
    /// The block's own pieces as it is mined and breaks (`breaking`).
    Breaking,
    /// The fight's own marks and flashes.
    Combat,
}

/// Where an effect shows.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum At {
    /// The block just set: the cell on the aimed face of the block aimed at.
    Placed,
    /// The block aimed at, out of the face aimed at.
    Target,
    /// The top of the block aimed at.
    Top,
    /// Just before and below the eyes.
    Mouth,
    /// Over what the grid made, on the inventory screen.
    Made,
}

/// What an effect's particles look like.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub(crate) enum Seen {
    /// Pieces of the block's texture where it shows.
    Block,
    /// Specks the colour of the item in hand.
    Held,
    /// Specks of one colour.
    Color((u8, u8, u8)),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Effect {
    pub(crate) at: At,
    pub(crate) look: Seen,
    #[serde(default = "one")]
    pub(crate) count: u32,
    #[serde(default = "one")]
    pub(crate) bursts: u32,
    #[serde(default)]
    pub(crate) every: f32,
    /// Blocks a second outwards, and upwards.
    #[serde(default)]
    pub(crate) speed: f32,
    #[serde(default)]
    pub(crate) lift: f32,
    /// Out round the block's foot rather than every way.
    #[serde(default)]
    pub(crate) ring: bool,
    /// Starts this far above where it shows, to fall onto it.
    #[serde(default)]
    pub(crate) drop: f32,
    #[serde(default)]
    pub(crate) gravity: f32,
    #[serde(default)]
    pub(crate) drag: f32,
    /// Carried up, as smoke is, rather than thrown.
    #[serde(default)]
    pub(crate) rise: bool,
    /// Its width at the end of its life, as a multiple of its start.
    #[serde(default = "full")]
    pub(crate) grow: f32,
    /// Smallest and largest width, shortest and longest life.
    pub(crate) size: (f32, f32),
    pub(crate) life: (f32, f32),
    #[serde(default = "opaque")]
    pub(crate) alpha: u8,
    #[serde(default)]
    pub(crate) glow: bool,
}

fn one() -> u32 {
    1
}

fn full() -> f32 {
    1.0
}

fn opaque() -> u8 {
    255
}

impl Effect {
    /// How its particles are drawn: glowing ones add their light, a see-through rising one
    /// is a veil (steam), the rest are cut out like pieces of blocks.
    fn blend(&self) -> ParticleBlend {
        if self.glow {
            ParticleBlend::Add
        } else if self.alpha < 255 {
            ParticleBlend::Veil
        } else {
            ParticleBlend::Cutout
        }
    }
}

impl ActionFx {
    pub(crate) fn load(files: &[ContentFile]) -> Result<Self, String> {
        let mut all = ActionFx::default();
        for file in files.iter().filter(|f| f.name == FILE) {
            let def: ActionFx = ron::from_str(&file.source).map_err(|e| format!("{}/{FILE}: {e}", file.package))?;
            all.actions.extend(def.actions);
            all.effects.extend(def.effects);
        }
        Ok(all)
    }

    /// Whether every action the content has says how it shows, and every one named here is
    /// an action, says one thing, and names an effect there is, within sensible bounds.
    pub(crate) fn check(&self, actions: &ActionRegistry) -> Result<(), String> {
        for (name, shows) in &self.actions {
            if actions.id(name).is_none() {
                return Err(format!("{FILE}: a look for `{name}`, which is no action"));
            }
            let says = [shows.effect.is_some(), shows.shown.is_some(), shows.none].into_iter().filter(|&s| s).count();
            if says != 1 {
                return Err(format!("{FILE}: `{name}` says {says} of effect, shown and none; it says one"));
            }
            if let Some(effect) = &shows.effect
                && !self.effects.contains_key(effect)
            {
                return Err(format!("{FILE}: `{name}` shows `{effect}`, which is no effect"));
            }
            if shows.confirmed && shows.effect.is_none() {
                return Err(format!("{FILE}: `{name}` is `confirmed` with no effect to wait with"));
            }
        }
        for (name, effect) in &self.effects {
            let fine = (1..=64).contains(&effect.count)
                && (1..=8).contains(&effect.bursts)
                && (0.0..=2.0).contains(&effect.every)
                && effect.size.0 >= 0.0
                && effect.size.0 <= effect.size.1
                && effect.size.1 <= 1.0
                && effect.life.0 > 0.0
                && effect.life.0 <= effect.life.1
                && effect.life.1 <= 5.0
                && (0.0..=8.0).contains(&effect.grow)
                && [effect.speed, effect.lift, effect.drop, effect.gravity, effect.drag].iter().all(|v| (0.0..=40.0).contains(v));
            if !fine {
                return Err(format!(
                    "{FILE}: effect `{name}` is out of bounds (count 1 to 64, bursts 1 to 8, every up to 2 s, size up to 1 block, life up to 5 s, grow up to 8, speeds up to 40)"
                ));
            }
        }
        let missing: Vec<&str> = actions.names().filter(|name| !self.actions.contains_key(*name)).collect();
        if !missing.is_empty() {
            return Err(format!("{FILE}: no look for {}; give each an `effect`, `shown` or `none: true`", missing.join(", ")));
        }
        Ok(())
    }
}

/// The effect each action shows, by id, with its name.
#[derive(Resource, Default)]
struct Shown(Vec<(ActionId, String, Effect, bool)>);

/// Particles each effect has spawned, by its name, for the gate to count.
#[derive(Resource, Default)]
pub(crate) struct FxSeen(pub(crate) BTreeMap<String, u32>);

fn load(mut commands: Commands, launch: Res<Launch>) {
    let fx = ActionFx::load(&launch.content.files).unwrap_or_else(|e| {
        error!("no action effects: {e}");
        ActionFx::default()
    });
    let shown = fx
        .actions
        .iter()
        .filter_map(|(name, shows)| {
            let effect = shows.effect.as_ref()?;
            Some((launch.content.actions.id(name)?, effect.clone(), fx.effects.get(effect)?.clone(), shows.confirmed))
        })
        .collect();
    commands.insert_resource(Shown(shown));
}

/// Bursts still to come of effects begun: the effect, where, the block and colour it
/// shows, and when.
#[derive(Resource, Default)]
struct Pending(Vec<Burst>);

#[derive(Clone, Debug)]
struct Burst {
    name: String,
    effect: Effect,
    at: Vec3,
    /// The cell whose texture and light it shows, and the face it flies off.
    cell: IVec3,
    face: Face,
    held: Option<BlockId>,
    when: f32,
    /// For a `confirmed` effect: the block aimed at, what it held when proposed, and until
    /// when it waits for that to change.
    waits: Option<(IVec3, Option<BlockId>, f32)>,
}

/// Where an effect shows for an action aimed at `target`, and the cell and face it is of.
/// A block set down went into the cell in front of the aimed face, or, the aimed block being
/// replaceable (grass, snow, flowers, water), into that block's own cell: whichever holds a
/// block now (`world` shows the placing at once).
fn place(effect: &Effect, target: ActionTarget, player: &LocalPlayer, world: &ClientWorld) -> Option<(Vec3, IVec3, Face)> {
    let eye = player.eye();
    match (effect.at, target) {
        (At::Mouth, _) => {
            let ahead = player.look_direction() * Vec3::new(1.0, 0.0, 1.0);
            Some((eye + ahead.normalize_or_zero() * 0.35 - Vec3::Y * 0.3, eye.floor().as_ivec3(), Face::PosY))
        }
        (At::Placed, ActionTarget::Block { block, face }) => {
            let front = block + face.normal();
            let shows = |cell: IVec3| world.block(cell).is_some_and(|b| world.registry.info(b).render != RenderKind::Invisible);
            let cell = if shows(front) { front } else { block };
            Some((cell.as_vec3() + Vec3::new(0.5, 0.05, 0.5), cell, Face::PosY))
        }
        (At::Target, ActionTarget::Block { block, face }) => Some((block.as_vec3() + 0.5 + face.normal().as_vec3() * 0.55, block, face)),
        (At::Top, ActionTarget::Block { block, .. }) => Some((block.as_vec3() + Vec3::new(0.5, 1.02, 0.5), block, Face::PosY)),
        _ => None,
    }
}

/// Every action proposed shows its effect, the first burst at once.
#[allow(clippy::too_many_arguments)]
fn show_actions(net: Option<Res<Net>>, shown: Option<Res<Shown>>, player: Res<LocalPlayer>, world: Res<ClientWorld>, time: Res<Time>, mut pending: ResMut<Pending>, mut glints: Commands, slots: Query<Entity, With<crate::hud::inventory::OutputSlot>>) {
    let (Some(net), Some(shown)) = (net, shown) else { return };
    let now = time.elapsed_secs();
    let held = player.inventory.slots.get(player.selected).copied().flatten().map(|stack| stack.item);
    for (action, target) in net.take_shown() {
        let Some((_, name, effect, confirmed)) = shown.0.iter().find(|(id, ..)| *id == action) else { continue };
        if effect.at == At::Made {
            for slot in &slots {
                glint(&mut glints, slot, effect);
            }
            continue;
        }
        let Some((at, cell, face)) = place(effect, target, &player, &world) else { continue };
        let waits = match target {
            ActionTarget::Block { block, .. } if *confirmed => Some((block, world.block(block), now + CONFIRM_WITHIN)),
            _ => None,
        };
        for burst in 0..effect.bursts {
            pending.0.push(Burst { name: name.clone(), effect: effect.clone(), at, cell, face, held, when: now + burst as f32 * effect.every, waits });
        }
    }
}

/// Starts each `confirmed` burst whose block has turned since it was proposed, its bursts
/// counted from now; drops one the server never turned in time.
fn confirm(pending: &mut Vec<Burst>, now: f32, block: impl Fn(IVec3) -> Option<BlockId>) {
    pending.retain_mut(|b| match b.waits {
        Some((cell, before, until)) if block(cell) != before => {
            b.when += now - (until - CONFIRM_WITHIN);
            b.waits = None;
            true
        }
        Some((.., until)) => now <= until,
        None => true,
    });
}

/// Spawns the bursts that are due.
#[allow(clippy::too_many_arguments)]
fn spawn_pending(
    time: Res<Time>,
    world: Res<ClientWorld>,
    launch: Res<Launch>,
    pieces: Option<Res<TexturePieces>>,
    mut particles: ResMut<Particles>,
    mut pending: ResMut<Pending>,
    mut seen: ResMut<FxSeen>,
    mut dice: Local<Dice>,
) {
    let now = time.elapsed_secs();
    confirm(&mut pending.0, now, |cell| world.block(cell));
    let (due, later): (Vec<Burst>, Vec<Burst>) = pending.0.drain(..).partition(|b| b.when <= now && b.waits.is_none());
    pending.0 = later;
    for burst in due {
        for particle in burst_particles(&burst, &world, &launch.content.blocks, pieces.as_deref(), particles.amount, &mut dice) {
            particles.spawn(&particle);
            *seen.0.entry(burst.name.clone()).or_default() += 1;
        }
    }
}

/// The particles of one burst: as many of `count` as the particle setting's `amount` lets
/// through.
fn burst_particles(burst: &Burst, world: &ClientWorld, blocks: &voxmar_engine::block::BlockRegistry, pieces: Option<&TexturePieces>, amount: f32, dice: &mut Dice) -> Vec<Particle> {
    let effect = &burst.effect;
    let light = light_at(world, burst.cell + burst.face.normal());
    // The block a Block look shows: what stands there now (set, or still there), else
    // nothing to show.
    let block = world.block(burst.cell).filter(|&b| {
        let info = world.registry.info(b);
        info.render != RenderKind::Invisible
    });
    let mut out = Vec::new();
    for _ in 0..effect.count {
        if dice.roll() >= amount {
            continue;
        }
        let around = dice.roll() * std::f32::consts::TAU;
        let (position, outwards) = if effect.ring {
            let side = Vec3::new(around.cos(), 0.0, around.sin());
            (burst.at + side * 0.5, side)
        } else {
            let jitter = dice.spread() * Vec3::new(0.25, 0.1, 0.25);
            (burst.at + jitter + Vec3::Y * effect.drop, dice.spread().normalize_or(Vec3::Y) * Vec3::new(1.0, 0.3, 1.0) + burst.face.normal().as_vec3() * 0.5)
        };
        let velocity = outwards * effect.speed * (0.5 + dice.roll()) + Vec3::Y * effect.lift * (0.6 + 0.6 * dice.roll());
        let size = effect.size.0 + (effect.size.1 - effect.size.0) * dice.roll();
        let life = effect.life.0 + (effect.life.1 - effect.life.0) * dice.roll();
        let mut particle = match (effect.look, block, pieces) {
            (Seen::Block, Some(block), Some(pieces)) => piece(world, pieces, dice, burst.cell, world.registry.info(block), burst.face, light, position, velocity, size),
            (Seen::Block, ..) => continue,
            (Seen::Held, ..) => {
                let Some(item) = burst.held else { continue };
                let [r, g, b] = blocks.def(item).color;
                let shade = 0.85 + 0.3 * dice.roll();
                let tone = |c: u8| (c as f32 * shade).min(255.0) as u8;
                Particle { position, velocity, look: Look::Speck, color: [tone(r), tone(g), tone(b), effect.alpha], light, ..default() }
            }
            (Seen::Color((r, g, b)), ..) => Particle { position, velocity, look: Look::Speck, color: [r, g, b, effect.alpha], light, ..default() },
        };
        particle.size = size;
        particle.life = life;
        particle.grow = effect.grow;
        particle.drag = effect.drag;
        particle.blend = effect.blend();
        particle.glow = effect.glow;
        if effect.rise {
            particle.motion = Motion::Rise;
            particle.gravity = -effect.gravity;
            particle.wander = 0.15;
        } else {
            particle.motion = Motion::Ballistic;
            particle.gravity = effect.gravity;
            // Where it comes to rest by its own fall: a Block look's piece came with the floor
            // a breaking chip's fall would reach, under other gravity and drag.
            particle.floor = crate::breaking::landing_floor_with(world, position, velocity, effect.gravity, effect.drag) + size * 0.35;
        }
        out.push(particle);
    }
    out
}

/// A glint over the grid's output: a soft square of light that swells and fades.
#[derive(Component)]
struct Glint {
    born: f32,
    life: f32,
    color: Color,
}

fn glint(commands: &mut Commands, slot: Entity, effect: &Effect) {
    let (r, g, b) = match effect.look {
        Seen::Color(rgb) => rgb,
        _ => (255, 255, 255),
    };
    let color = Color::srgba_u8(r, g, b, effect.alpha);
    let node = Node { position_type: PositionType::Absolute, left: Val::Px(0.0), right: Val::Px(0.0), top: Val::Px(0.0), bottom: Val::Px(0.0), ..default() };
    commands.entity(slot).with_child((Glint { born: f32::NAN, life: effect.life.1, color }, node, BackgroundColor(color.with_alpha(0.0))));
}

fn fade_glints(mut commands: Commands, time: Res<Time>, mut glints: Query<(Entity, &mut Glint, &mut BackgroundColor, &mut UiTransform)>) {
    let now = time.elapsed_secs();
    for (entity, mut glint, mut background, mut transform) in &mut glints {
        if glint.born.is_nan() {
            glint.born = now;
        }
        let t = ((now - glint.born) / glint.life.max(0.01)).clamp(0.0, 1.0);
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        // Up fast, then away: brightest a fifth of the way in.
        let shine = if t < 0.2 { t / 0.2 } else { 1.0 - (t - 0.2) / 0.8 };
        background.0 = glint.color.with_alpha(glint.color.alpha() * 0.6 * shine);
        transform.scale = Vec2::splat(1.0 + 0.25 * t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> voxmar_engine::content::Content {
        voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap()
    }

    /// Every action says how it shows in the base package; a package naming an action the
    /// content lacks, or an effect there is not, or saying two things, fails; and so does an
    /// action, a mod's too, that says nothing.
    #[test]
    fn every_action_says_how_it_shows() {
        let content = content();
        let fx = ActionFx::load(&content.files).unwrap();
        assert_eq!(fx.check(&content.actions), Ok(()));
        let file = |source: &str| ContentFile { package: "zmod".into(), name: FILE.into(), source: source.into() };
        let base = content.files.iter().find(|f| f.package == "base" && f.name == FILE).unwrap().clone();
        for wrong in [
            r#"(actions: {"core:juggle": (none: true)})"#,
            r#"(actions: {"core:eat": (none: true, shown: Some(Breaking))})"#,
            r#"(actions: {"core:eat": ()})"#,
            r#"(actions: {"core:eat": (effect: Some("fireworks"))})"#,
            r#"(effects: {"settle": (at: Placed, look: Block, count: 0, size: (0.1, 0.1), life: (1.0, 1.0))})"#,
            r#"(effects: {"settle": (at: Placed, look: Block, size: (0.2, 0.1), life: (1.0, 1.0))})"#,
            r#"(effects: {"settle": (at: Placed, look: Block, size: (0.1, 0.1), life: (1.0, 9.0))})"#,
        ] {
            let more = ActionFx::load(&[base.clone(), file(wrong)]).unwrap();
            assert!(more.check(&content.actions).is_err(), "{wrong}");
        }
        let mut more = ActionRegistry::default();
        for (namespace, source) in [("core", "core/actions.ron"), ("base", "base/actions.ron")] {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
            more.extend(namespace, &std::fs::read_to_string(dir.join(source)).unwrap()).unwrap();
        }
        more.extend("zmod", r#"(actions: [(name: "juggle", duration: Instant, effects: [])])"#).unwrap();
        let unseen = ActionFx::load(std::slice::from_ref(&base)).unwrap().check(&more);
        assert!(unseen.as_ref().is_err_and(|e| e.contains("zmod:juggle")), "{unseen:?}");
        let seen = ActionFx::load(&[base, file(r#"(actions: {"zmod:juggle": (effect: Some("sparks"))})"#)]).unwrap();
        assert_eq!(seen.check(&more), Ok(()));
    }

    /// The everyday actions the first days are made of show something: placing, eating,
    /// crafting, gathering and working; mining by its breaking.
    #[test]
    fn everyday_actions_are_seen() {
        let content = content();
        let fx = ActionFx::load(&content.files).unwrap();
        for name in ["core:place", "core:eat", "core:craft", "base:pick", "base:till", "base:plant", "base:use_table", "base:use_station", "base:cook", "base:light"] {
            assert!(fx.actions[name].effect.is_some(), "{name} shows nothing");
        }
        assert_eq!(fx.actions["core:mine"].shown, Some(ShownBy::Breaking));
    }

    /// Each effect lands where it should: dust round the foot of the block set, crumbs
    /// before the eyes, steam over the fire, leaves out of the face aimed at; and a glint
    /// is no place in the world.
    #[test]
    fn effects_show_where_the_action_is() {
        let fx = ActionFx::load(&content().files).unwrap();
        let player = LocalPlayer { position: Vec3::new(0.5, 64.0, 0.5), ..default() };
        let target = ActionTarget::Block { block: IVec3::new(3, 63, 0), face: Face::PosY };
        let content = content();
        let mut world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        world.insert_column(voxmar_engine::coords::ColumnPos::of_block(IVec3::new(3, 63, 0)), std::sync::Arc::new(voxmar_engine::world::Column::default()));
        let planks = content.blocks.expect("base:oak_planks");
        world.set_block(IVec3::new(3, 64, 0), planks);
        let at = |world: &ClientWorld, name: &str| place(&fx.effects[name], target, &player, world).map(|(at, cell, _)| (at, cell));
        assert_eq!(at(&world, "settle"), Some((Vec3::new(3.5, 64.05, 0.5), IVec3::new(3, 64, 0))));
        assert_eq!(at(&world, "steam").unwrap().0, Vec3::new(3.5, 64.02, 0.5));
        assert_eq!(at(&world, "picked").unwrap(), (Vec3::new(3.5, 64.05, 0.5), IVec3::new(3, 63, 0)));
        let (mouth, _) = at(&world, "crumbs").unwrap();
        assert!(mouth.y < player.eye().y && mouth.distance(player.eye()) < 0.6, "{mouth}");
        assert_eq!(at(&world, "made"), None);
        assert_eq!(place(&fx.effects["settle"], ActionTarget::None, &player, &world), None);
        // Set into a replaceable block's own cell (aiming at grass), the dust is there.
        world.set_block(IVec3::new(3, 64, 0), voxmar_engine::block::BlockId::AIR);
        world.set_block(IVec3::new(3, 63, 0), planks);
        assert_eq!(at(&world, "settle"), Some((Vec3::new(3.5, 63.05, 0.5), IVec3::new(3, 63, 0))));
    }

    /// A confirmed burst waits for its block to turn, then counts its bursts from then; one
    /// whose block never turns (clay refused) is dropped once the wait runs out.
    #[test]
    fn a_confirmed_burst_waits_for_its_block() {
        let content = content();
        let fx = ActionFx::load(&content.files).unwrap();
        assert!(fx.actions["base:coat"].confirmed, "clay pressed on shows once the server turns the panel");
        let (panel, daubed) = (content.blocks.expect("base:oak_planks"), content.blocks.expect("base:stone"));
        let cell = IVec3::new(0, 64, 0);
        let burst = |when: f32| Burst { name: "settle".into(), effect: fx.effects["settle"].clone(), at: Vec3::ZERO, cell, face: Face::PosY, held: None, when, waits: Some((cell, Some(panel), 10.0 + CONFIRM_WITHIN)) };
        let mut pending = vec![burst(10.0), burst(10.25)];
        confirm(&mut pending, 10.5, |_| Some(panel));
        assert!(pending.iter().all(|b| b.waits.is_some()), "still the panel: it waits");
        confirm(&mut pending, 11.0, |_| Some(daubed));
        assert_eq!(pending.iter().map(|b| (b.when, b.waits.is_none())).collect::<Vec<_>>(), vec![(11.0, true), (11.25, true)], "turned: counted from then");
        let mut refused = vec![burst(10.0)];
        confirm(&mut refused, 10.0 + CONFIRM_WITHIN + 0.1, |_| Some(panel));
        assert!(refused.is_empty(), "never turned, it shows nothing");
    }

    /// A burst gives its count at full particles, none with particles off, and its look:
    /// specks of the held item's colour, glowing sparks that add their light, steam that
    /// rises as a veil.
    #[test]
    fn bursts_look_as_named() {
        let content = content();
        let fx = ActionFx::load(&content.files).unwrap();
        let world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        let burst = |name: &str, held: Option<BlockId>| Burst { name: name.into(), effect: fx.effects[name].clone(), at: Vec3::new(0.5, 70.0, 0.5), cell: IVec3::new(0, 69, 0), face: Face::PosY, held, when: 0.0, waits: None };
        let bread = content.blocks.expect("base:cooked_small_game");
        let mut dice = Dice::default();
        let crumbs = burst_particles(&burst("crumbs", Some(bread)), &world, &content.blocks, None, 1.0, &mut dice);
        assert_eq!(crumbs.len(), fx.effects["crumbs"].count as usize);
        let [r, g, b] = content.blocks.def(bread).color;
        assert!(crumbs.iter().all(|p| p.color[0].abs_diff(r) < 60 && p.color[1].abs_diff(g) < 60 && p.color[2].abs_diff(b) < 60 && p.motion == Motion::Ballistic));
        assert!(burst_particles(&burst("crumbs", Some(bread)), &world, &content.blocks, None, 0.0, &mut dice).is_empty());
        assert!(burst_particles(&burst("crumbs", None), &world, &content.blocks, None, 1.0, &mut dice).is_empty(), "nothing held, no crumbs");
        let sparks = burst_particles(&burst("sparks", None), &world, &content.blocks, None, 1.0, &mut dice);
        assert!(!sparks.is_empty() && sparks.iter().all(|p| p.glow && p.blend == ParticleBlend::Add));
        let steam = burst_particles(&burst("steam", None), &world, &content.blocks, None, 1.0, &mut dice);
        assert!(!steam.is_empty() && steam.iter().all(|p| p.motion == Motion::Rise && p.blend == ParticleBlend::Veil && p.velocity.y > 0.0));
        // A Block look over air shows nothing.
        assert!(burst_particles(&burst("settle", None), &world, &content.blocks, None, 1.0, &mut dice).is_empty());
        // Over a block, its pieces come to rest where their own fall takes them, not where a
        // breaking chip's would: the floor a step below the burst on open ground beside it.
        let mut world = world;
        world.insert_column(voxmar_engine::coords::ColumnPos::of_block(IVec3::ZERO), std::sync::Arc::new(voxmar_engine::world::Column::default()));
        let stone = content.blocks.expect("base:stone");
        for x in 0..16 {
            for z in 0..16 {
                world.set_block(IVec3::new(x, 60, z), stone);
            }
        }
        world.set_block(IVec3::new(0, 69, 0), stone);
        let solid = crate::render::particles::TexturePieces::new(&[crate::textures::Rgba { width: 16, height: 16, pixels: vec![255; 1024] }], &[]);
        let effect = &fx.effects["settle"];
        let settle = burst_particles(&burst("settle", None), &world, &content.blocks, Some(&solid), 1.0, &mut dice);
        assert!(!settle.is_empty());
        for p in &settle {
            let own = crate::breaking::landing_floor_with(&world, p.position, p.velocity, effect.gravity, effect.drag) + p.size * 0.35;
            assert_eq!((p.gravity, p.drag, p.floor), (effect.gravity, effect.drag, own), "a settle piece falls and lands by the effect's own numbers");
        }
    }

    /// Writes `target/fx_contact_sheet.png`: every effect from the side, its particles where
    /// their paths take them at 0.05, 0.2, 0.4 and 0.7 s, as specks in their colours (pieces
    /// of a block in that block's own colour), on a grid of block squares. A sketch of the
    /// motion, worked out as the shader does, not a render.
    #[test]
    #[ignore]
    fn fx_contact_sheet() {
        let content = content();
        let fx = ActionFx::load(&content.files).unwrap();
        let world = ClientWorld::new(content.blocks.clone(), 2, crate::render::terrain::Terrain::detached());
        let sample = |name: &str| match name {
            "settle" => "base:oak_planks",
            "picked" => "base:wheat",
            "soil" => "base:dirt",
            _ => "base:crafting_table",
        };
        let held = content.blocks.expect("base:cooked_small_game");
        let times = [0.05f32, 0.2, 0.4, 0.7];
        let (cell, rows) = (220u32, fx.effects.len() as u32);
        let mut sheet = image::RgbImage::from_pixel(cell * times.len() as u32, cell * rows, image::Rgb([28, 30, 36]));
        let mut dice = Dice::default();
        for (row, (name, effect)) in fx.effects.iter().enumerate() {
            let burst = Burst { name: name.clone(), effect: effect.clone(), at: Vec3::new(0.5, 1.0, 0.5), cell: IVec3::ZERO, face: Face::PosY, held: Some(held), when: 0.0, waits: None };
            // A Block look shows pieces; drawn here in the block's own colour.
            let mut look = effect.clone();
            if look.look == Seen::Block {
                let [r, g, b] = content.blocks.def(content.blocks.expect(sample(name))).color;
                look.look = Seen::Color((r, g, b));
            }
            let particles: Vec<Particle> = (0..effect.bursts).flat_map(|_| burst_particles(&Burst { effect: look.clone(), ..burst.clone() }, &world, &content.blocks, None, 1.0, &mut dice)).collect();
            for (col, &t) in times.iter().enumerate() {
                let (ox, oy) = (col as u32 * cell, row as u32 * cell);
                // Block grid: 2 blocks across, the block it shows at the bottom middle.
                let scale = cell as f32 / 2.0;
                for y in 0..cell {
                    for x in 0..cell {
                        if x % (scale as u32) == 0 || y % (scale as u32) == 0 {
                            sheet.put_pixel(ox + x, oy + y, image::Rgb([48, 52, 60]));
                        }
                    }
                }
                for p in &particles {
                    let mut v = p.velocity;
                    let mut at = p.position;
                    let step = 1.0 / 240.0;
                    let mut s = 0.0;
                    while s < t.min(p.life) {
                        let g = if p.motion == Motion::Rise { -p.gravity.abs().max(0.0) } else { p.gravity };
                        v.y -= g * step;
                        v *= 1.0 - p.drag * step;
                        at += v * step;
                        if p.motion == Motion::Ballistic && at.y < p.floor.max(1.0) {
                            at.y = p.floor.max(1.0);
                            v = Vec3::ZERO;
                        }
                        s += step;
                    }
                    if t > p.life {
                        continue;
                    }
                    let fade = (1.0 - t / p.life).clamp(0.0, 1.0).sqrt() * p.color[3] as f32 / 255.0;
                    let px = ((at.x - 0.5) * scale + cell as f32 / 2.0) as i32;
                    let py = (cell as f32 - (at.y - 0.5) * scale) as i32;
                    let r = ((p.size * p.grow.max(0.3).powf(t / p.life)) * scale / 2.0).max(1.5) as i32;
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (x, y) = (px + dx, py + dy);
                            if dx * dx + dy * dy > r * r || x < 0 || y < 0 || x >= cell as i32 || y >= cell as i32 {
                                continue;
                            }
                            let pixel = sheet.get_pixel_mut(ox + x as u32, oy + y as u32);
                            for c in 0..3 {
                                let over = p.color[c] as f32;
                                pixel.0[c] = if p.glow { (pixel.0[c] as f32 + over * fade).min(255.0) as u8 } else { (pixel.0[c] as f32 * (1.0 - fade) + over * fade) as u8 };
                            }
                        }
                    }
                }
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fx_contact_sheet.png");
        sheet.save(&path).unwrap();
        println!("wrote {} ({} effects: {})", path.display(), rows, fx.effects.keys().cloned().collect::<Vec<_>>().join(", "));
    }
}
