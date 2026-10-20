//! Local player: mouse look, zoom, predicted movement with voxel collision, and the camera.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::inventory::Inventory;
use voxmar_engine::skill::Skills;
use voxmar_engine::vitals::Vitals;
use voxmar_engine::movement::{self, Body, Controls, Surroundings};
use voxmar_engine::physics::{Aabb, player as dims};

use crate::menu::Screen;
use crate::render::DayCycle;
use crate::session::Launch;
use crate::settings::Settings;
use crate::world::ClientWorld;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .init_resource::<UiState>()
            .init_resource::<Zoom>()
            .init_resource::<SprintFov>()
            .add_systems(
                Update,
                ((grab_cursor, zoom, look, movement, follow_camera).chain().run_if(in_state(Screen::InGame)), set_fov.after(zoom)),
            )
            .add_systems(
                OnExit(Screen::InGame),
                (release_cursor, |mut zoom: ResMut<Zoom>, mut kick: ResMut<SprintFov>| {
                    *zoom = Zoom::default();
                    *kick = SprintFov::default();
                }),
            );
    }
}

#[derive(Resource)]
pub struct LocalPlayer {
    /// Feet position.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub in_water: bool,
    /// Whether the last step was stopped by a wall across (see `movement::Body`).
    pub against_wall: bool,
    pub flying: bool,
    pub sprinting: bool,
    /// Seconds until a held jump may jump again (see `movement::Body`).
    pub jump_delay: f32,
    pub allow_flight: bool,
    pub admin: bool,
    /// Flight speed multiplier, as last agreed with the server.
    pub fly_speed: f32,
    pub health: u8,
    /// The most health the player may have: twice their Hitpoints level, and what effects add.
    pub max_health: u8,
    pub inventory: Inventory,
    /// Experience in every skill, as the server last said, and whether it has said yet.
    pub skills: Skills,
    pub skills_known: bool,
    /// How many times the server has said the player's state (inventory, health, skills):
    /// what was predicted here before a count was taken is the server's once it moves on.
    pub states: u32,
    /// Hunger, warmth and their effects, as the server last said.
    pub vitals: Vitals,
    /// When the vitals last came, in seconds since startup: timed effects count down from it.
    pub vitals_at: f32,
    pub selected: usize,
    /// The crafting table the player is working at (its three-by-three grid shows in the
    /// inventory), until the inventory closes.
    pub station: Option<IVec3>,
    /// A store was asked open (or a locked chest's lock tried): the screen opens when the
    /// server's word of it comes (`hud::inventory`).
    pub store_asked: bool,
    pub dead: bool,
    /// Set once the server has welcomed us.
    pub joined: bool,
    /// The world's seed, as the server told us.
    pub seed: u64,
    /// What the player has discovered, as the server tells it (the journal screen's data).
    pub journal: voxmar_engine::journal::Journal,
    /// Every quest's state, as the server tells it (the quest book's data).
    pub quests: Vec<voxmar_engine::quest::QuestState>,
    /// The last teleport applied, which inputs echo (`PlayerInput::teleport`).
    pub teleports: u32,
    /// Counts the server's word on quests, for screens to redraw only on a change.
    pub quests_rev: u32,
}

impl Default for LocalPlayer {
    fn default() -> Self {
        Self {
            position: Vec3::new(0.5, 100.0, 0.5),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            in_water: false,
            against_wall: false,
            flying: false,
            sprinting: false,
            jump_delay: 0.0,
            allow_flight: false,
            admin: false,
            fly_speed: 1.0,
            health: 20,
            station: None,
            store_asked: false,
            max_health: 20,
            inventory: Inventory::default(),
            skills: Skills::default(),
            skills_known: false,
            states: 0,
            vitals: Vitals::default(),
            vitals_at: 0.0,
            selected: 0,
            dead: false,
            joined: false,
            seed: 0,
            journal: Default::default(),
            quests: Vec::new(),
            teleports: 0,
            quests_rev: 0,
        }
    }
}

impl LocalPlayer {
    pub fn eye(&self) -> Vec3 {
        voxmar_engine::vitals::eye(self.position, dims::EYE_HEIGHT, self.vitals.seated)
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn look_direction(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
}

#[derive(Resource, Default)]
pub struct UiState {
    pub inventory_open: bool,
    /// The character screen: skills, health, hunger and effects.
    pub character_open: bool,
    /// The journal of what the player has found.
    pub journal_open: bool,
    /// The quest book.
    pub quests_open: bool,
    /// The settings menu.
    pub menu_open: bool,
    /// The chat and command line.
    pub console_open: bool,
    /// The recipe viewer, over another screen (`hud::recipes`).
    pub recipes_open: bool,
    pub cursor_locked: bool,
}

/// Esc closes the console, the inventory or the character screen, or toggles the menu. The mouse is free while a
/// screen is open, grabbed again when it closes, and grabbed by clicking into the window.
/// Without a window (`--gate`) all of it works the same, but for grabbing the pointer.
fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<UiState>,
    player: Res<LocalPlayer>,
    mut screen_was_open: Local<bool>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if ui.console_open {
            ui.console_open = false;
        } else if ui.recipes_open {
            ui.recipes_open = false;
        } else if ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open {
            ui.inventory_open = false;
            ui.character_open = false;
            ui.journal_open = false;
            ui.quests_open = false;
        } else {
            ui.menu_open = !ui.menu_open;
        }
    }
    let screen_open = ui.inventory_open || ui.character_open || ui.journal_open || ui.quests_open || ui.menu_open || ui.console_open || ui.recipes_open || player.dead;
    let want_locked = !screen_open && (ui.cursor_locked || *screen_was_open || mouse.just_pressed(MouseButton::Left));
    *screen_was_open = screen_open;
    let changed = want_locked != ui.cursor_locked;
    ui.cursor_locked = want_locked;
    let Ok(mut cursor) = cursor.single_mut() else { return };
    if changed || (want_locked && cursor.grab_mode == CursorGrabMode::None) {
        cursor.grab_mode = if want_locked { CursorGrabMode::Locked } else { CursorGrabMode::None };
        cursor.visible = !want_locked;
    }
}

/// The menus need the mouse.
fn release_cursor(mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Zooming in, like a spyglass: while the zoom key is held the view narrows by the chosen
/// magnification, which the wheel changes, easing in and out. Mouse look slows to match,
/// so the view sweeps across the screen as fast as unzoomed, and with cinematic zoom it
/// follows the mouse smoothly, for steady shots.
#[derive(Resource)]
pub struct Zoom {
    /// Whether the zoom key is held where zooming is allowed.
    pub held: bool,
    /// Magnification while held: the setting's, changed by the wheel since the key went down.
    target: f32,
    /// Magnification shown now, easing towards `target` while held and back to 1 after.
    current: f32,
    /// Where cinematic zoom is taking the view (yaw, pitch), while it is on.
    aim: Option<Vec2>,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { held: false, target: 1.0, current: 1.0, aim: None }
    }
}

impl Zoom {
    /// Magnifications the wheel stays between. The smallest is also the least the setting
    /// allows.
    pub const MIN: f32 = 1.5;
    pub const MAX: f32 = 50.0;
    /// Change in magnification per notch of the wheel.
    const STEP: f32 = 1.15;
    /// Pixels of trackpad scrolling that count as one notch.
    const PIXELS_PER_NOTCH: f32 = 20.0;
    /// How quickly the view zooms in and out, per second.
    const RATE: f32 = 12.0;
    /// How quickly the cinematic view catches up with the mouse, per second.
    const FOLLOW: f32 = 5.0;

    /// The vertical field of view, in radians, for `degrees` unzoomed: narrowed so that
    /// everything at the centre looks magnified, rather than the angle simply divided.
    pub fn fov(&self, degrees: f32) -> f32 {
        2.0 * ((degrees.to_radians() / 2.0).tan() / self.current).atan()
    }

    /// How far cinematic smoothing applies, from 0 unzoomed to 1 from the least
    /// magnification on, so it comes and goes with the zoom without a jump.
    fn smoothing(&self) -> f32 {
        (self.current.ln() / Self::MIN.ln()).clamp(0.0, 1.0)
    }

    /// Holds or releases the zoom, turns it by `notches` of the wheel while held, and eases
    /// the magnification over `dt` seconds. `magnification` is the setting.
    fn update(&mut self, held: bool, magnification: f32, notches: f32, dt: f32) {
        if held && !self.held {
            self.target = magnification;
        }
        self.held = held;
        if held {
            self.target = (self.target * Self::STEP.powf(notches)).clamp(Self::MIN, Self::MAX);
        }
        // Eased in proportion, so every doubling takes as long.
        let goal = if held { self.target } else { 1.0 };
        let eased = goal * (self.current / goal).powf((-Self::RATE * dt).exp());
        self.current = if (eased / goal - 1.0).abs() < 1e-3 { goal } else { eased };
    }

    /// Turns the view by `turn` (yaw, pitch) of mouse movement, already scaled to radians,
    /// over `dt` seconds. With `cinematic`, the view eases towards where the mouse points.
    fn look(&mut self, player: &mut LocalPlayer, turn: Vec2, cinematic: bool, dt: f32) {
        let clamp_pitch = |pitch: f32| pitch.clamp(-1.55, 1.55);
        let smoothing = if cinematic { self.smoothing() } else { 0.0 };
        if smoothing == 0.0 {
            self.aim = None;
            player.yaw -= turn.x;
            player.pitch = clamp_pitch(player.pitch - turn.y);
            return;
        }
        let aim = self.aim.get_or_insert(Vec2::new(player.yaw, player.pitch));
        *aim = Vec2::new(aim.x - turn.x, clamp_pitch(aim.y - turn.y));
        let follow = 1.0 - smoothing * (-Self::FOLLOW * dt).exp();
        player.yaw += (aim.x - player.yaw) * follow;
        player.pitch += (aim.y - player.pitch) * follow;
    }
}

/// Zooms while the zoom key is held, in creative (where flight is allowed) or anywhere if
/// the settings allow it. The wheel changes the magnification while zoomed.
pub fn zoom(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    player: Res<LocalPlayer>,
    mut zoom: ResMut<Zoom>,
) {
    let allowed = ui.cursor_locked && !player.dead && (player.allow_flight || settings.zoom_in_survival);
    let held = allowed && keys.pressed(settings.zoom_key.0);
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / Zoom::PIXELS_PER_NOTCH,
    };
    zoom.update(held, settings.zoom as f32 / 100.0, notches, time.delta_secs());
}

fn look(
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    mut zoom: ResMut<Zoom>,
    mut player: ResMut<LocalPlayer>,
) {
    const SENSITIVITY: f32 = 0.0022;
    let delta = if ui.cursor_locked { motion.delta } else { Vec2::ZERO };
    if delta == Vec2::ZERO && zoom.aim.is_none() {
        return;
    }
    let sensitivity = SENSITIVITY * settings.sensitivity as f32 / 100.0 / zoom.current;
    zoom.look(&mut player, delta * sensitivity, settings.cinematic_zoom, time.delta_secs());
}

/// The view widening a little while sprinting, as Minecraft's does, so speed is felt.
#[derive(Resource, Default)]
pub struct SprintFov {
    /// How far into the kick the view is, 0 to 1.
    amount: f32,
}

impl SprintFov {
    /// How much wider the view is at full kick (Minecraft's is 1.15).
    const WIDEST: f32 = 1.1;
    /// How quickly it widens and narrows back, per second.
    const RATE: f32 = 10.0;

    fn update(&mut self, sprinting: bool, dt: f32) {
        let goal = if sprinting { 1.0 } else { 0.0 };
        self.amount += (goal - self.amount) * (1.0 - (-Self::RATE * dt).exp());
    }

    fn factor(&self) -> f32 {
        1.0 + (Self::WIDEST - 1.0) * self.amount
    }
}

/// The camera's field of view: the setting's, widened while sprinting and narrowed while
/// zoomed.
fn set_fov(
    time: Res<Time>,
    settings: Res<Settings>,
    zoom: Res<Zoom>,
    player: Res<LocalPlayer>,
    mut kick: ResMut<SprintFov>,
    mut camera: Query<&mut Projection, With<Camera3d>>,
) {
    let Ok(mut projection) = camera.single_mut() else { return };
    kick.update(settings.fov_effects && player.sprinting && player.velocity.with_y(0.0).length() > 1.0, time.delta_secs());
    let fov = zoom.fov(settings.fov as f32 * kick.factor());
    if let Projection::Perspective(perspective) = projection.bypass_change_detection()
        && perspective.fov != fov
    {
        perspective.fov = fov;
        projection.set_changed();
    }
}

/// Seconds within which a second press of forward starts a sprint (Minecraft's seven
/// ticks).
const DOUBLE_TAP: f32 = 0.35;

#[allow(clippy::too_many_arguments)]
fn movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    settings: Res<Settings>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    mut player: ResMut<LocalPlayer>,
    mut last_forward: Local<Option<f32>>,
    (net, actions, mut fighting, creatures, target): (Res<crate::net::Net>, Res<crate::Actions>, ResMut<crate::fight::Fighting>, Res<crate::creatures::Creatures>, Res<crate::interact::Target>),
) {
    let player = &mut *player;
    if !player.joined || player.dead {
        return;
    }
    // Hold still until the ground under us has arrived.
    if !world.voxels.contains(ColumnPos::of_block(player.position.floor().as_ivec3())) {
        player.velocity = Vec3::ZERO;
        return;
    }

    let pressed = |key| ui.cursor_locked && keys.pressed(key);
    if ui.cursor_locked && keys.just_pressed(KeyCode::KeyF) && player.allow_flight {
        player.flying = !player.flying;
        player.velocity = Vec3::ZERO;
    }

    let forward = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
    let right = Vec3::new(player.yaw.cos(), 0.0, -player.yaw.sin());
    let mut wish = Vec3::ZERO;
    for (key, dir) in [(KeyCode::KeyW, forward), (KeyCode::KeyS, -forward), (KeyCode::KeyD, right), (KeyCode::KeyA, -right)] {
        if pressed(key) {
            wish += dir;
        }
    }
    // A second tap of forward soon after the first starts a sprint.
    let now = time.elapsed_secs();
    let mut double_tap = false;
    if ui.cursor_locked && keys.just_pressed(KeyCode::KeyW) {
        double_tap = settings.double_tap_sprint && last_forward.is_some_and(|t| now - t < DOUBLE_TAP);
        *last_forward = (!double_tap).then_some(now);
    }
    // A dodge (`fight`): with a direction, a dash that way; alone, a step back. Not in the
    // air (past `dodge_grace` off the ground, as the server judges), nor eating, nor with
    // the guard up, nor sooner than the last allows.
    let combat = &launch.content.combat;
    let clock = time.elapsed_secs_f64();
    if player.on_ground && !player.flying {
        fighting.grounded = clock;
    }
    if ui.cursor_locked
        && keys.just_pressed(settings.dodge_key.0)
        && clock - fighting.grounded <= combat.feel.dodge_grace as f64
        && !player.flying
        && fighting.guard.is_none()
        && now >= target.eating_until
        && clock >= fighting.dodge_ready
        && let Some(dodge_action) = actions.0.id("core:dodge")
    {
        // The heaviest armour worn chooses the dodge; without armour, the first.
        let dodge = combat.dodge_of(&player.inventory.worn);
        let dir = if wish == Vec3::ZERO { -forward } else { wish.normalize() };
        fighting.dash = Some((dir, dodge.distance / dodge.time.max(0.05), dodge.time));
        fighting.dodge_was = fighting.dodge_ready;
        fighting.dodge_ready = clock + dodge.cooldown as f64;
        let held = player.inventory.get(player.selected).map(|s| s.item);
        let weapon = combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill)));
        fighting.cancel(clock, &combat.timing(&weapon), &combat.feel);
        net.propose_from(player, dodge_action, voxmar_engine::action::ActionTarget::None);
    }
    // Guarding, the player walks slower and cannot sprint.
    let guard_speed = fighting.guard.and_then(|_| {
        let held = player.inventory.get(player.selected).map(|s| s.item);
        combat.guard_of(held, &player.inventory.worn, |skill| launch.content.skills.level(player.skills.xp(skill))).map(|g| g.speed)
    });
    // Drawing a shot, the player moves at the class's `drawing_speed` (a bow held drawn
    // walks and sprints slowly; a weapon that strikes is not slowed).
    let drawing_speed = fighting.drawn(clock).then(|| {
        let held = player.inventory.get(player.selected).map(|s| s.item);
        combat.drawing_speed(&combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill))))
    });
    fighting.drawing_pace = drawing_speed;
    let controls = Controls {
        wish: wish.normalize_or_zero(),
        forward: pressed(KeyCode::KeyW) && !pressed(KeyCode::KeyS),
        sprint: guard_speed.is_none() && (pressed(settings.sprint_key.0) || double_tap),
        jump: pressed(KeyCode::Space),
        descend: pressed(KeyCode::ShiftLeft),
    };
    let mut body = Body {
        position: player.position,
        velocity: player.velocity,
        on_ground: player.on_ground,
        in_water: player.in_water,
        flying: player.flying,
        fly_speed: player.fly_speed,
        against_wall: player.against_wall,
        sprinting: player.sprinting,
        jump_delay: player.jump_delay,
    };
    // Quicker or slower on foot as the player's effects say (fruit, a gale).
    let speed = launch.content.effects.modifiers(&player.vitals.effects).speed * guard_speed.unwrap_or(1.0) * drawing_speed.unwrap_or(1.0);
    let tuning = launch.content.movement.on_foot(speed);
    let dt = time.delta_secs().min(0.25);
    let around = Around { world: &world };
    match fighting.dash {
        Some(under_way) => fighting.dash = dash(&mut body, under_way, &around, &tuning, controls, dt),
        None => movement::advance(&mut body, &around, &tuning, controls, dt),
    }
    // A creature's body is not walked into: the player is put back out of it, a little
    // clear, so the view never ends up inside its model.
    let clearance = combat.feel.creature_clearance;
    for aabb in creatures.bodies(&launch) {
        if let Some(out) = out_of(body.position, &aabb, dims::WIDTH / 2.0 + clearance)
            && !Aabb::standing(out, dims::WIDTH, dims::HEIGHT).overlaps(|cell| around.boxes(cell))
        {
            body.position = out;
        }
    }
    player.position = body.position;
    player.velocity = body.velocity;
    player.on_ground = body.on_ground;
    player.in_water = body.in_water;
    player.flying = body.flying;
    player.against_wall = body.against_wall;
    player.sprinting = body.sprinting;
    player.jump_delay = body.jump_delay;
}

/// A dash under way, `(direction, speed, seconds left)`, carries the body `dt` further: at
/// exactly its speed, so it covers its distance at any frame rate, stopping at a wall or at
/// a ledge, so it never drops the player off an edge. Returns what is left of it.
fn dash(body: &mut Body, (dir, speed, left): (Vec3, f32, f32), around: &impl Surroundings, tuning: &movement::Tuning, controls: Controls, dt: f32) -> Option<(Vec3, f32, f32)> {
    // Ground ahead: a block's top at the feet or up to a block below them, in the cell ahead
    // at the feet's height (a slab underfoot is in the feet's own cell) or one of the two under
    // it, under the footprint there (a post's top only where the player would be over it).
    let at = body.position + dir * 0.45;
    let ahead = at.floor().as_ivec3();
    let feet = body.position.y;
    let half = dims::WIDTH / 2.0;
    let ground = (0..=2).any(|d| {
        let cell = ahead - IVec3::Y * d;
        around.boxes(cell).iter().any(|b| {
            let (min, max) = (b.min + cell.as_vec3(), b.max + cell.as_vec3());
            let under = min.x < at.x + half && max.x > at.x - half && min.z < at.z + half && max.z > at.z - half;
            under && (feet - 1.0 - 1e-3..=feet + 1e-3).contains(&max.y)
        })
    });
    // The last frame moves only for the time left of it.
    let step = dt.min(left);
    let now = if ground && dt > 0.0 { speed * step / dt } else { 0.0 };
    body.velocity = (dir * now).with_y(body.velocity.y);
    // Asked for the speed it already has, movement's easing toward it changes nothing.
    let tuning = movement::Tuning { walk_speed: now, sprint_speed: now, water_speed: 1.0, ..tuning.clone() };
    let controls = Controls { wish: if ground { dir } else { Vec3::ZERO }, forward: false, sprint: false, ..controls };
    movement::advance(body, around, &tuning, controls, dt);
    let left = left - step;
    if ground && left > 1e-4 {
        return Some((dir, speed, left));
    }
    // Done: it stops, rather than sliding on.
    body.velocity.x = 0.0;
    body.velocity.z = 0.0;
    None
}

/// Where a player standing at `feet` must move to be `room` clear of `aabb` sideways, if
/// they are not already (or above or below it).
fn out_of(feet: Vec3, aabb: &Aabb, room: f32) -> Option<Vec3> {
    if feet.y + dims::HEIGHT <= aabb.min.y || feet.y >= aabb.max.y {
        return None;
    }
    let (min, max) = (aabb.min - Vec3::new(room, 0.0, room), aabb.max + Vec3::new(room, 0.0, room));
    if feet.x <= min.x || feet.x >= max.x || feet.z <= min.z || feet.z >= max.z {
        return None;
    }
    // Out by the nearest side.
    let sides = [(feet.x - min.x, Vec3::new(min.x, feet.y, feet.z)), (max.x - feet.x, Vec3::new(max.x, feet.y, feet.z)), (feet.z - min.z, Vec3::new(feet.x, feet.y, min.z)), (max.z - feet.z, Vec3::new(feet.x, feet.y, max.z))];
    sides.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, out)| out)
}

/// The client's world as movement sees it.
struct Around<'a> {
    world: &'a ClientWorld,
}

impl Surroundings for Around<'_> {
    fn solid(&self, cell: IVec3) -> bool {
        self.world.is_solid(cell)
    }

    /// As the server judges moves (`Server::boxes`): a shaped block by its boxes, unloaded
    /// space as a whole cube, so the player cannot fall out of the loaded world.
    fn boxes(&self, cell: IVec3) -> &[Aabb] {
        self.world.block(cell).map_or(&voxmar_engine::physics::CUBE, |b| self.world.registry.collision(b))
    }

    fn water(&self, aabb: &Aabb) -> (f32, Vec2) {
        movement::water_around(&self.world.registry, |p| self.world.block(p), aabb)
    }
}

pub(crate) fn follow_camera(
    player: Res<LocalPlayer>,
    world: Res<ClientWorld>,
    mut cycle: ResMut<DayCycle>,
    mut camera: Query<&mut Transform, With<Camera3d>>,
) {
    if let Ok(mut transform) = camera.single_mut() {
        transform.translation = player.eye();
        transform.rotation = player.rotation();
    }
    // Under water when the eye is below the surface of the water around it.
    let eye = player.eye();
    let registry = &world.registry;
    let water = registry.id("base:water").and_then(|b| registry.fluid(b)).map(|f| f.fluid);
    let cell = eye.floor().as_ivec3();
    let in_water = |p: IVec3| world.block(p).and_then(|b| registry.fluid(b)).filter(|s| Some(s.fluid) == water);
    cycle.underwater = in_water(cell).is_some_and(|s| in_water(cell + IVec3::Y).is_some() || eye.y - cell.y as f32 <= s.height());
    if cycle.underwater {
        // The surface above, which the light comes down through (no deeper than this is
        // worth looking up: little light is left by then).
        let mut top = cell;
        while top.y - cell.y < 64 && in_water(top + IVec3::Y).is_some() {
            top += IVec3::Y;
        }
        cycle.water_surface = top.y as f32 + in_water(top).map_or(1.0, |s| s.height());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same case as the server's `an_unloaded_cell_stops_a_player_as_a_whole_cube`: a
    /// cell in a loaded column of air stops nothing; one in the next column, not loaded, and
    /// one under the world stop the player as a whole cube, as the server judges them.
    #[test]
    fn an_unloaded_cell_stops_a_player_as_a_whole_cube() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let mut world = ClientWorld::new(content.blocks, 2, crate::render::terrain::Terrain::detached());
        let loaded = IVec3::new(1, 64, 1);
        world.insert_column(ColumnPos::of_block(loaded), std::sync::Arc::new(voxmar_engine::world::Column::default()));
        let around = Around { world: &world };
        assert!(around.boxes(loaded).is_empty());
        for unloaded in [loaded + IVec3::X * voxmar_engine::coords::CHUNK_SIZE, IVec3::new(1, -1, 1)] {
            assert_eq!(around.boxes(unloaded), &voxmar_engine::physics::CUBE, "{unloaded}");
        }
    }

    /// Flat ground below y 0.
    struct Floor;

    impl Surroundings for Floor {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y < 0
        }

        fn water(&self, _: &Aabb) -> (f32, Vec2) {
            (0.0, Vec2::ZERO)
        }
    }

    #[test]
    fn a_dash_covers_its_distance_at_any_frame_rate() {
        // The shipped movement numbers (which now hold more than a test names), with the ones
        // this test fixes.
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let tuning = movement::Tuning {
            walk_speed: 4.7,
            sprint_speed: 6.0,
            sprint_jump_boost: 0.0,
            jump_velocity: 9.0,
            gravity: 32.0,
            terminal_velocity: 78.0,
            water_speed: 0.4,
            fly_speed: 10.0,
            fly_sprint_speed: 20.0,
            ..content.movement.on_foot(1.0)
        };
        // The light dodge: 3 blocks in 0.25 s.
        for fps in [20.0, 30.0, 60.0, 144.0, 240.0] {
            let start = Vec3::new(0.5, 0.0, 0.5);
            let mut body = Body { position: start, velocity: Vec3::ZERO, on_ground: true, in_water: false, flying: false, fly_speed: 1.0, against_wall: false, sprinting: false, jump_delay: 0.0 };
            let mut under_way = Some((Vec3::X, 3.0 / 0.25, 0.25));
            let mut frames = 0;
            while let Some(d) = under_way {
                under_way = dash(&mut body, d, &Floor, &tuning, Controls::default(), 1.0 / fps);
                frames += 1;
            }
            let went = (body.position - start).with_y(0.0).length();
            assert!((went - 3.0).abs() < 0.02, "{fps} fps: {went} blocks in {frames} frames");
            assert_eq!(body.velocity.with_y(0.0), Vec3::ZERO, "it stops at the end");
        }
    }

    /// A floor of slabs (the lower half of y 0) with nothing under it, ending at x 2.
    struct Slabs;

    const SLAB: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 0.5, 1.0) }];

    impl Surroundings for Slabs {
        fn solid(&self, cell: IVec3) -> bool {
            cell.y == 0 && cell.x < 2
        }

        fn boxes(&self, cell: IVec3) -> &[Aabb] {
            if self.solid(cell) { &SLAB } else { &[] }
        }

        fn water(&self, _: &Aabb) -> (f32, Vec2) {
            (0.0, Vec2::ZERO)
        }
    }

    /// On a floor of slabs over air (an upper storey, a bridge) a dash goes on as on whole
    /// blocks, and stops at the floor's edge rather than dropping off it.
    #[test]
    fn a_dash_runs_along_a_slab_floor_and_stops_at_its_edge() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let tuning = content.movement.on_foot(1.0);
        let start = Vec3::new(-5.5, 0.5, 0.5);
        let mut body = Body { position: start, velocity: Vec3::ZERO, on_ground: true, in_water: false, flying: false, fly_speed: 1.0, against_wall: false, sprinting: false, jump_delay: 0.0 };
        let mut under_way = Some((Vec3::X, 3.0 / 0.25, 0.25));
        while let Some(d) = under_way {
            under_way = dash(&mut body, d, &Slabs, &tuning, Controls::default(), 1.0 / 60.0);
        }
        let went = body.position.x - start.x;
        assert!((went - 3.0).abs() < 0.02, "dashed {went} blocks along the slabs");
        // From near the edge it stops short of dropping off.
        body.position = Vec3::new(1.0, 0.5, 0.5);
        let mut under_way = Some((Vec3::X, 3.0 / 0.25, 0.25));
        while let Some(d) = under_way {
            under_way = dash(&mut body, d, &Slabs, &tuning, Controls::default(), 1.0 / 60.0);
        }
        assert!(body.position.x < 2.0 && body.position.y == 0.5, "dashed off the edge to {}", body.position);
    }

    /// A floor of whole blocks ending at x = 2, and beyond its edge, a block lower, a post
    /// standing in the far corner of the cell: its top is level with the floor but not under
    /// a player passing the near side.
    struct PostBeyond;

    const WHOLE: [Aabb; 1] = [Aabb { min: Vec3::ZERO, max: Vec3::ONE }];
    const CORNER_POST: [Aabb; 1] = [Aabb { min: Vec3::new(0.0, 0.0, 0.8), max: Vec3::new(0.2, 1.0, 1.0) }];

    impl Surroundings for PostBeyond {
        fn solid(&self, cell: IVec3) -> bool {
            !self.boxes(cell).is_empty()
        }

        fn boxes(&self, cell: IVec3) -> &[Aabb] {
            match (cell.x, cell.y, cell.z) {
                (x, -1, 0) if x < 2 => &WHOLE,
                (2, -1, 0) => &CORNER_POST,
                _ => &[],
            }
        }

        fn water(&self, _: &Aabb) -> (f32, Vec2) {
            (0.0, Vec2::ZERO)
        }
    }

    /// A post's top is ground only where the player is over it: a dash past the near side
    /// of one stops at the floor's edge rather than running off it.
    #[test]
    fn a_dash_stops_at_an_edge_beside_a_post_it_is_not_over() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let tuning = content.movement.on_foot(1.0);
        let mut body = Body { position: Vec3::new(1.0, 0.0, 0.3), velocity: Vec3::ZERO, on_ground: true, in_water: false, flying: false, fly_speed: 1.0, against_wall: false, sprinting: false, jump_delay: 0.0 };
        let mut under_way = Some((Vec3::X, 3.0 / 0.25, 0.25));
        while let Some(d) = under_way {
            under_way = dash(&mut body, d, &PostBeyond, &tuning, Controls::default(), 1.0 / 60.0);
        }
        assert!(body.position.x < 2.0 && body.position.y == 0.0, "dashed off the edge past the post to {}", body.position);
    }

    #[test]
    fn zoom_eases_in_and_out_and_the_wheel_changes_it() {
        let mut zoom = Zoom::default();
        assert_eq!(zoom.fov(70.0), 70f32.to_radians());
        // Held for a second, the view reaches the setting's magnification.
        for _ in 0..60 {
            zoom.update(true, 4.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(zoom.current, 4.0);
        let half = (35f32.to_radians().tan() / 4.0).atan();
        assert!((zoom.fov(70.0) - 2.0 * half).abs() < 1e-6);
        // Scrolling up zooms further, within limits.
        zoom.update(true, 4.0, 2.0, 0.0);
        assert!((zoom.target - 4.0 * Zoom::STEP * Zoom::STEP).abs() < 1e-4);
        zoom.update(true, 4.0, 1000.0, 0.0);
        assert_eq!(zoom.target, Zoom::MAX);
        // Released, it eases back, and the next hold starts from the setting again.
        for _ in 0..60 {
            zoom.update(false, 4.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(zoom.current, 1.0);
        zoom.update(true, 4.0, 0.0, 0.0);
        assert_eq!(zoom.target, 4.0);
    }

    #[test]
    fn cinematic_zoom_follows_the_mouse_smoothly_and_only_while_zoomed() {
        let mut player = LocalPlayer::default();
        let mut zoom = Zoom::default();
        // Unzoomed, looking is direct even with cinematic zoom on.
        zoom.look(&mut player, Vec2::new(0.1, 0.0), true, 1.0 / 60.0);
        assert_eq!(player.yaw, -0.1);
        zoom.current = 4.0;
        zoom.look(&mut player, Vec2::new(0.1, 0.0), true, 1.0 / 60.0);
        assert!(player.yaw > -0.2 && player.yaw < -0.1, "eases towards the aim");
        for _ in 0..300 {
            zoom.look(&mut player, Vec2::ZERO, true, 1.0 / 60.0);
        }
        assert!((player.yaw + 0.2).abs() < 1e-4, "catches up");
        // Without cinematic zoom the view turns at once.
        zoom.look(&mut player, Vec2::new(0.1, 0.0), false, 1.0 / 60.0);
        assert!((player.yaw + 0.3).abs() < 1e-4);
    }
}
