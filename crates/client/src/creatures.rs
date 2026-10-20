//! Creatures on the client: what the server says lives nearby, drawn as smooth low-poly
//! bodies (engine `creature::volume`, Mariano's pick, 2026-10-06) built from their species'
//! parts, one mesh a species, bending at one joint a part, in a pixel skin painted through
//! the body for each one (`creature::skin::paint_volume`, `creature_skin.wgsl`). They are
//! carried on smoothly between the server's updates and animated from how they move: legs
//! swing with their stride (hoppers and bounding runners move their pairs together), the
//! body bobs with each step and breathes at rest, heads dip to graze and turn to watch a
//! player who comes near, ears twitch, tails sway (a kept creature's faster when its keeper
//! is near), wings flap, a struck creature flashes red and a dead one topples and sinks
//! away. Left click on one strikes it (`core:attack`).
//!
//! The bodies are ordinary Bevy skinned meshes outside the renderer (docs/rendering.md,
//! "Keeping room for what comes next"), unlit, shaded flat by face in their shader, their
//! colour darkened by the light where they stand (sky and block light, as dropped items
//! take theirs).

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::{AsBindGroup, Buffer, ShaderType};
use bevy::shader::ShaderRef;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use rustc_hash::FxHashMap;
use voxmar_engine::action::ActionTarget;
use voxmar_engine::combat::Blow;
use voxmar_engine::creature::brain::Pose;
use voxmar_engine::creature::skin::{self, FACES};
use voxmar_engine::creature::volume::{self, BodyMesh, Shape};
use voxmar_engine::creature::{self, CreatureId, Part, Style, Swing};
use voxmar_engine::physics::Aabb;
use voxmar_protocol::{CreatureGone, CreatureView};

use crate::Actions;
use crate::creature_motion::{self as motion, Motion};
use crate::creature_sounds::{CreatureSound, CreatureSounds, Kind as Sound};
use crate::breaking::sky_light;
use crate::drops::block_light;
use crate::interact::Target;
use crate::menu::Screen;
use crate::net::Net;
use crate::player::{LocalPlayer, UiState};
use crate::session::Launch;
use crate::world::ClientWorld;

/// How quickly the shown position catches up with where the creature should be, per
/// second, and the furthest it runs ahead of the server's last word, in seconds.
const FOLLOW: f32 = 14.0;
const AHEAD: f32 = 0.25;
/// Seconds a struck creature flashes red.
const FLASH: f32 = 0.3;
/// Hearts shown over a creature being tamed or ready to breed.
const HEARTS: usize = 3;
/// Blocks within which a creature turns its head to watch the player, and how far it turns.
const WATCHES_WITHIN: f32 = 9.0;
const WATCH_TURN: f32 = 1.0;
/// How far a boss rears up at its fullest, in radians (`shown.rear`).
const REAR: f32 = 0.45;

/// A small bird's flight: blocks it rises and dips each bound, beats a second, how far its
/// wings swing out from folded (radians), how much longer they reach spread, and how fast
/// one that dies in the air falls, in blocks a second.
const UNDULATE: f32 = 0.18;
const WINGBEATS: f32 = 24.0;
const WING_SPREAD: f32 = 1.35;
const WING_REACH: f32 = 1.6;
const FALLS: f32 = 6.0;
/// The most blocks one that died in the air falls before it goes anyway (over an unloaded
/// column, or the void).
const FALLS_MOST: f32 = 48.0;

/// Where a flier is in its bounding flight: its rise (−1..1) and how hard it is beating its
/// wings (0..1), climbing on a burst of wingbeats and dipping in a glide, each bird to its own
/// time.
fn flight_bound(now: f32, id: u32) -> (f32, f32) {
    let phase = (now * 0.9 + creature::unit_from(id as u64) * 7.0).fract();
    let beating = if phase < 0.55 { 1.0 } else { ((0.62 - phase) / 0.07).clamp(0.0, 1.0) };
    ((phase * std::f32::consts::TAU).sin(), beating)
}

/// The heart's pixels, a row a string: `#` is heart, `+` its highlight.
const HEART: [&str; 7] = ["         ", " ##   ## ", "#+##.####", "#+#######", " ####### ", "  #####  ", "    #    "];

/// A heart drawn in pixels, white (tinted by its material), clear around it.
fn heart_image() -> Image {
    let (w, h) = (HEART[0].len() as u32, HEART.len() as u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for row in HEART {
        for c in row.chars() {
            data.extend_from_slice(match c {
                '#' => &[255, 255, 255, 255],
                '+' | '.' => &[255, 235, 235, 255],
                _ => &[0, 0, 0, 0],
            });
        }
    }
    let extent = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let mut image = Image::new(extent, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    image.sampler = nearest();
    image
}

/// One clear texel of skin, worn while the real one is painted.
pub(crate) fn blank_skin() -> Image {
    Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D3, vec![0; 4], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

fn nearest() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor { mag_filter: ImageFilterMode::Nearest, min_filter: ImageFilterMode::Nearest, ..default() })
}

/// Removes a creature's model and its hearts.
fn despawn(commands: &mut Commands, shown: &Shown) {
    commands.entity(shown.entity).despawn();
    for heart in &shown.hearts {
        commands.entity(*heart).despawn();
    }
}

/// Creatures built a frame at most; more coming into view wait for the next frames.
const SPAWNS: usize = 3;
/// Creatures further than this are not drawn.
const DRAWN_WITHIN: f32 = 96.0;
/// Steps a creature's light is rounded to, so the slow change of daylight rewrites its
/// material now and then rather than every frame.
pub(crate) const LIGHT_STEPS: f32 = 128.0;

pub struct CreaturesPlugin;

/// The gate retains its eye fixture's CPU pixels after their upload to the GPU. Other
/// skins give up their pixels as usual, so normal play pays no memory cost for the check.
#[derive(Resource)]
pub(crate) struct InspectEyes;

impl Plugin for CreaturesPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "creature_skin.wgsl");
        app.add_plugins(MaterialPlugin::<SkinMaterial>::default())
            .init_resource::<Creatures>()
            .add_systems(Update, show.in_set(ShowCreatures).run_if(in_state(Screen::InGame)))
            .add_systems(OnExit(Screen::InGame), |mut commands: Commands, mut creatures: ResMut<Creatures>| {
                for shown in creatures.shown.values() {
                    despawn(&mut commands, shown);
                }
                *creatures = Creatures::default();
            });
    }
}

/// A creature's material: unlit, its colour from its own skin, painted through its body's
/// volume and snapped to the block pixel grid (`creature_skin.wgsl`).
pub type SkinMaterial = ExtendedMaterial<StandardMaterial, CreatureSkin>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct CreatureSkin {
    #[texture(100, dimension = "3d")]
    #[sampler(101)]
    pub skin: Handle<Image>,
    #[uniform(102)]
    pub frame: SkinFrame,
    /// The air between the eye and the creature, as the terrain's fog reads it
    /// (`render::atmosphere::SharedAtmosphere`).
    #[storage(103, read_only, buffer)]
    pub atmosphere: Buffer,
}

/// Where the skin's volume lies in model space.
#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct SkinFrame {
    /// The volume's corner, and texels a block.
    pub origin: Vec4,
    /// Its size in blocks.
    pub size: Vec4,
}

impl MaterialExtension for CreatureSkin {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxmar/creature_skin.wgsl".into()
    }
}

/// A species' body: its mesh, weighted to one joint per part, and its shape for painting.
pub(crate) struct SpeciesBody {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) binds: Handle<SkinnedMeshInverseBindposes>,
    shape: std::sync::Arc<Shape>,
    /// The skin's volume: its corner and size in texels.
    pub(crate) origin: Vec3,
    pub(crate) dims: [u32; 3],
}

/// A boss's wind-up begun: which creature, its blow's name (`rootmother.grasp`, or its
/// name alone for its plain attack), seconds until the blow, and how far from its middle
/// the blow reaches (blocks).
#[derive(Clone, Debug, PartialEq)]
pub struct Told {
    pub id: u32,
    pub key: String,
    pub windup: f32,
    pub reach: f32,
    /// The style its blow is struck in, whose tell any creature without one of its own
    /// gives.
    pub style: voxmar_engine::combat::Style,
    /// First seen already winding up, some of it gone already.
    pub late: bool,
}

/// How the watched creature (`Creatures::watch`) was drawn moving, for the gate: how far
/// its feet slid while they were down (blocks, every foot added up, over `planted` foot
/// frames), how far its body went (over `frames`), the fastest it turned (radians a
/// second), its yaw, and how far it showed running away.
#[derive(Clone, Copy, Debug, Default)]
pub struct Watched {
    pub slid: f32,
    pub planted: u32,
    pub went: f32,
    pub frames: u32,
    pub fastest_turn: f32,
    pub yaw: f32,
    pub flee: f32,
    /// Dying, as last drawn: degrees rolled onto its side, its scale, and how far it has
    /// sunk (blocks).
    pub death_roll: f32,
    pub death_scale: f32,
    pub death_sink: f32,
    /// How big its last flinch was (`fight_fx::flinch_size`).
    pub flinch_size: f32,
}

impl Watched {
    /// How far a foot slid while down for each block the body went: 0 for feet that stay
    /// where they are put.
    pub fn slip(&self) -> f32 {
        let slid = self.slid / self.planted.max(1) as f32;
        let went = self.went / self.frames.max(1) as f32;
        slid / went.max(1e-4)
    }
}

/// Where one foot is held (`Shown::holds`).
#[derive(Clone, Copy, Default)]
struct Hold {
    down: Option<(Vec3, i64)>,
    lifted: Option<(f32, i64)>,
}

/// A creature as the client shows it.
struct Shown {
    view: CreatureView,
    /// When the view arrived, in seconds since startup.
    told_at: f32,
    /// When it last showed a fight (attacking, a tell, or hurt), in seconds since startup.
    fought_at: Option<f32>,
    position: Vec3,
    yaw: f32,
    entity: Entity,
    /// Its body's mesh, and one joint per part, which the animation turns.
    body: Entity,
    joints: Vec<(Entity, Part)>,
    /// Its skin, which is darkened by the light and flashed red, and the skin being painted
    /// for it, until it is ready.
    material: Handle<SkinMaterial>,
    painting: Option<Task<Image>>,
    skin: Handle<Image>,
    /// Stride phase, in cycles.
    stride: f32,
    /// How fast it is turning, radians a second (to its left positive).
    spin: f32,
    /// How far it shows running away, 0 ..= 1.
    flee: f32,
    /// Its pace over the ground, blocks a second, smoothed.
    pace: f32,
    /// While the gate watches it: which of its strides each joint's foot was down in, in
    /// the pose written last frame and the one before (a slow frame can step over a whole
    /// swing, from one stride's footfall to the next's), and where the frame before drew
    /// each foot and the body.
    planted: Vec<Option<i64>>,
    /// Each joint's foot: the ground it is down on and in which stride, and the turn it was
    /// lifted at and in which stride.
    holds: Vec<Hold>,
    planted_before: Vec<Option<i64>>,
    feet: Vec<Option<Vec3>>,
    drawn_at: Option<Vec3>,
    /// Seconds until it may make an idle noise or call again.
    quiet: f32,
    flash: f32,
    /// Seconds since it died, if it did; the side it falls to (away from the eye), and
    /// what its fall did, for the gate: how far it strayed in heights, whether the eye was
    /// ever inside it.
    dying: Option<f32>,
    fall_side: f32,
    death_strayed: f32,
    death_around_eye: bool,
    /// While it falls, what is drawn: the furthest any joint of its body went from the
    /// middle of where it stood, in its heights, and the most any joint's scale strayed from
    /// standing (a stretched mesh shows there first).
    death_spread: f32,
    death_scale: f32,
    death_samples: u32,
    /// The longest step of game time a frame took while it tipped over (a slow frame there
    /// is what leaves fewer samples).
    death_slowest: f32,
    /// Seconds since it died by the game's clock, hit-stop or not: when it goes up in smoke
    /// with its loot (`Death::drops_after`), as the server lets the loot out.
    since_death: f32,
    /// Whether we have struck at it, so the target readout shows its health.
    struck: bool,
    /// Smoothed head pitch and turn, and body crouch.
    nod: f32,
    look: f32,
    crouch: f32,
    /// How far it rears up, 0 ..= 1: a boss's tell (`moves::Tell::rear`), or rearing
    /// into a new phase of its fight.
    rear: f32,
    /// How far a flier's wings are spread, 0 (folded, on the ground) ..= 1 (in flight).
    spread: f32,
    /// Blocks one that died in the air has fallen so far.
    fell: f32,
    /// Hearts over it, made when first needed: trust filling while it is tamed, and all
    /// red while a kept one is ready to breed.
    hearts: Vec<Entity>,
    /// How a blow shows on it (`fight_fx`): seconds it stays frozen, and of that freeze at
    /// first; seconds of white flash left; seconds into its flinch, and the way it flinches
    /// (away from the striker, level).
    stopped: f32,
    stop_from: f32,
    white: f32,
    flinch: Option<f32>,
    flinch_away: Vec3,
    /// How big its flinch is (`fight_fx::flinch_size`).
    flinch_size: f32,
    /// Seconds into its wind-up while it winds up, and into its strike once the blow lands;
    /// whether this wind-up has struck already (the client times it, the server's word
    /// that the wind-up is over comes after).
    coiling: Option<f32>,
    striking: Option<f32>,
    coil_struck: bool,
    /// What its wind-up showed so far, for the gate (`StrikeRead`): where it stood as the
    /// wind-up began, the furthest the server had it from there, how deep it coiled; and
    /// how much of its recover stance its strike has shown.
    coil_from: Vec3,
    coil_far: f32,
    /// How much of the way to where the server has it a body planted as `combat_fx.ron`
    /// says would still have to go, frame by frame through this wind-up (1 at its start).
    coil_kept: f32,
    coil_slid: f32,
    coil_deepest: f32,
    recover_most: f32,
    /// Seconds into its wind-up by the game's clock when its blow struck: how deep its coil
    /// had got, whatever frames drew it (a slow frame may skip the deepest of it).
    coil_reached: f32,
    /// Whether the server's word struck it rather than the wind-up's own time.
    coil_word: bool,
    /// Whether its blow has struck and what its wind-up showed is still to be written down.
    read_due: bool,
    /// Whether its puff of smoke has gone up as it vanishes.
    poofed: bool,
    /// The move (`moves`, from 1) it winds up and strikes with, 0 for its plain attack:
    /// what it showed as the wind-up began, kept through the strike after.
    move_shown: u8,
    /// Seconds of game time into its stagger (`STAGGERED`), while it shows one, and how far
    /// into its stagger stance it is (`fight_fx::stagger_depth`); its entry in
    /// `Creatures::stagger_reads`.
    staggered: Option<f32>,
    stagger_depth: f32,
    stagger_read: Option<usize>,
}

/// What one stagger showed, for the gate: whose it was, whether its blow had been shown as
/// it came (a perfect block parries a blow that came; a daze cuts a wind-up short, which
/// must not read as the blow landing), the most of its stagger stance it showed (0 to 1),
/// the frames drawn while it reeled, and the move (0 its plain attack) whose wind-up or
/// strike it was in, if any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaggerRead {
    pub id: u32,
    pub struck: bool,
    pub deepest: f32,
    pub frames: u32,
    pub interrupted: Option<u8>,
}

/// What a creature's new view does to its wind-up, from the one before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    /// Told it is staggered: any wind-up is over, and `struck` says whether its blow had
    /// already been shown.
    Staggered { struck: bool },
    WindsUp,
    /// Its wind-up ended at the server's word before its own time: the blow shows now.
    Strikes,
    /// Its wind-up ended, its blow shown already (or it is dying).
    Ends,
    Same,
}

/// The `Turn` from view `before` to `after`, for a creature `coiling` (its blow shown by its
/// own time if `coil_struck`), `striking` (its strike still showing) or `dying`.
fn turn(before: &CreatureView, after: &CreatureView, coiling: bool, coil_struck: bool, striking: bool, dying: bool) -> Turn {
    let attacking = |view: &CreatureView| view.pose == Pose::Attacking;
    let staggered = |view: &CreatureView| view.tell == voxmar_protocol::STAGGERED;
    if staggered(after) && !staggered(before) {
        Turn::Staggered { struck: coil_struck || striking }
    } else if attacking(after) && !attacking(before) {
        Turn::WindsUp
    } else if !attacking(after) && attacking(before) {
        if !coil_struck && coiling && !dying { Turn::Strikes } else { Turn::Ends }
    } else {
        Turn::Same
    }
}


/// What one strike's wind-up showed, written down as its blow struck: how far its body slid
/// while it coiled, how far it might have with its feet planted as `combat_fx.ron` says (it
/// follows where the server has it that much more slowly), and how deep it had coiled (0 to
/// 1) by the game's clock as the blow struck.
/// Which creature and strike it was (its kind's name and the strike's style), its wind-up
/// and how far into it the blow struck (seconds), and whether the server's word struck it
/// before the wind-up's own time did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrikeRead {
    pub slid: f32,
    pub planted: f32,
    pub coiled: f32,
    pub who: String,
    pub style: &'static str,
    pub windup: f32,
    pub reached: f32,
    pub by_word: bool,
}

impl Shown {
    /// What its wind-up showed: how far it slid, and how far it might have planted as
    /// `combat_fx.ron` says: following where the server had it (never further from where it
    /// began than `coil_far`) that much more slowly than `FOLLOW`, frame by frame as they
    /// came (`coil_kept`), it gets no further than this, with a little to spare.
    fn coil_read(&self, who: &str, style: &'static str, windup: f32) -> StrikeRead {
        let planted = self.coil_far * (1.0 - self.coil_kept) + 0.02;
        StrikeRead { slid: self.coil_slid, planted, coiled: self.coil_deepest, who: who.into(), style, windup, reached: self.coil_reached, by_word: self.coil_word }
    }
}

/// Every creature the server has told us about.
#[derive(Resource, Default)]
pub struct Creatures {
    shown: FxHashMap<u32, Shown>,
    /// Views and departures received since they were last applied.
    inbox: Vec<CreatureView>,
    left: Vec<CreatureGone>,
    /// Creatures that came into view but wait for a later frame to be built (`SPAWNS`).
    waiting: Vec<CreatureView>,
    /// Creatures our blows landed on since the last frame, to cry out.
    cries: Vec<u32>,
    /// Barks the server told of since the last frame (`ServerMessage::Bark`): who, and which.
    barks: Vec<(u32, voxmar_engine::creature::Bark)>,
    /// Our blows the server answered since the last frame: at whom, for how much, what kind,
    /// and whether it killed. Each jolts its creature (`fight_fx`).
    jolts: Vec<(u32, u16, Blow, bool)>,
    /// Bodies vanishing since `fight_fx` last looked: where their middle is, how tall they
    /// are and their coat's colour, for the puff of smoke they go in.
    pub poofs: Vec<(Vec3, f32, [u8; 3])>,
    /// Creatures staggered since `fight_fx` last looked: over their head, how tall they are
    /// and their coat, for the sound and the dizzy sparks.
    pub staggers: Vec<(Vec3, f32, [u8; 3])>,
    /// Staggers seen since joining, and what each showed, for the gate.
    pub staggers_seen: u32,
    pub stagger_reads: Vec<StaggerRead>,
    /// Blows creatures were seen to strike since joining (their wind-up ended in a snap),
    /// for the gate.
    pub strikes: u32,
    /// Bosses' wind-ups begun this frame, for `fight_fx` to tell.
    pub told: Vec<Told>,
    /// What each of those strikes showed (`StrikeRead`), for the gate's check that a strike
    /// reads: planted and coiled deep, then standing out its recovery.
    pub reads: Vec<StrikeRead>,
    /// How much of its recover stance each strike that stood its recovery out showed at
    /// most (0 to 1).
    pub recoveries: Vec<f32>,
    /// The creature whose stride the gate watches, and what it saw; and the step of game
    /// time it is drawn on each frame, whatever the frame took, so a slow machine sees as
    /// many poses in a stride as a fast one (the gate moves it that far a frame).
    pub watch: Option<u32>,
    pub watch_step: Option<f32>,
    pub watched: Watched,
    /// How each creature seen to die fell, for the gate's check.
    pub falls: Vec<Fall>,
    /// Each species' body, made when the first of it is seen.
    bodies: FxHashMap<CreatureId, SpeciesBody>,
    /// The skin a body wears until its own is painted (it is hidden meanwhile).
    blank: Option<Handle<Image>>,
    /// A shaded cube for hearts, and their colours: full and empty.
    heart: Option<(Handle<Mesh>, Handle<StandardMaterial>, Handle<StandardMaterial>)>,
    /// Creatures seen to die since joining.
    pub deaths: u32,
    /// Blows of ours the server took since joining, how many landed, and the most one did.
    pub blows: u32,
    pub landed: u32,
    pub hardest: u16,
    /// The creature we last struck at, and when, in seconds since startup.
    fighting: Option<(u32, f32)>,
    /// Our own player's id, to know our kept creatures.
    pub me: Option<voxmar_protocol::PlayerId>,
}

/// How a dead creature's body fell: how far from where it stood, in its heights, and
/// whether the eye was ever inside it (it is hidden then, never drawn around the camera).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fall {
    pub strayed: f32,
    pub around_eye: bool,
    /// As drawn: the furthest a joint went from where it stood, in heights, and the most a
    /// joint's scale strayed from standing.
    pub spread: f32,
    pub scale: f32,
    /// Joint samples taken while it tipped over, and how many joints it has.
    pub samples: u32,
    pub joints: u32,
    /// The longest step of game time, in seconds, any frame took while it tipped over, and
    /// how long its tip over lasts (its `death.fall`).
    pub slowest: f32,
    pub tip: f32,
}

/// A creature as the target readout shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sighting {
    pub species: CreatureId,
    pub health: u16,
    /// Whether we have struck at it: its health is shown only then, as in RuneScape.
    pub struck: bool,
    /// Who keeps it, if anyone: ourselves, someone else (by id, while they are here), or
    /// someone away.
    pub keeper: Option<Keeper>,
    pub young: bool,
    /// Fed and looking for a mate.
    pub ready: bool,
}

/// Who keeps a kept creature, as the target readout tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keeper {
    Us,
    Other(voxmar_protocol::PlayerId),
    Away,
}

impl Creatures {
    /// Creature `id` barked, as the server says: heard next frame, if it is shown.
    pub fn bark(&mut self, id: u32, bark: voxmar_engine::creature::Bark) {
        self.barks.push((id, bark));
    }

    /// The barks to hear now, of creatures shown; those of creatures told of but not yet
    /// built (with their first view, or behind the spawns a frame allows) wait for them,
    /// and those of creatures not here at all are dropped.
    fn barks_heard(&mut self) -> Vec<(u32, voxmar_engine::creature::Bark)> {
        let (heard, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.barks).into_iter().partition(|(id, _)| self.shown.contains_key(id));
        let coming = |id: u32| self.waiting.iter().chain(&self.inbox).any(|view| view.id == id);
        self.barks = rest.into_iter().filter(|&(id, _)| coming(id)).collect();
        heard
    }

    /// Takes what the server sent.
    pub fn receive(&mut self, changed: Vec<CreatureView>, gone: Vec<CreatureGone>) {
        self.inbox.extend(changed);
        // Gone drops what was told of it before and not yet shown: the gone are taken a
        // frame before the views, so an older view would bring it back for good (a creature
        // that moved and was dispersed by the next message, both in one frame). A view told
        // after the gone (one coming back into sight) stays.
        self.inbox.retain(|view| !gone.iter().any(|g| g.id == view.id));
        self.left.extend(gone);
    }

    /// The server took a blow of ours at `id`: it landed for `damage`, or missed.
    pub fn struck(&mut self, id: u32, damage: u16, blow: Blow, killed: bool, now: f32) {
        self.blows += 1;
        self.jolts.push((id, damage, blow, killed));
        self.fighting = Some((id, now));
        if let Some(shown) = self.shown.get_mut(&id) {
            shown.struck = true;
        }
        if damage > 0 {
            self.landed += 1;
            self.hardest = self.hardest.max(damage);
            if let Some(shown) = self.shown.get_mut(&id) {
                shown.flash = FLASH;
            }
            // Every blow that lands is a cry, the killing one too: a full charge can kill
            // a small creature outright, with no health drop seen before it died.
            self.cries.push(id);
        }
    }

    /// Creature `id` as the target readout shows it; one dying still is, at no health.
    /// Every living creature shown, by id: its species and where its feet are.
    pub fn living(&self) -> impl Iterator<Item = (u32, CreatureId, Vec3)> + '_ {
        self.shown.iter().filter(|(_, shown)| shown.dying.is_none()).map(|(&id, shown)| (id, shown.view.species, shown.position))
    }

    /// Whether creature `id` is in a fight at `now` (seconds since startup): winding up or
    /// charging, or it attacked, showed a tell or was hurt within the last `calm_after`
    /// seconds (a roaming boss's fight music waits for one, and ends once it has calmed:
    /// one nicked that goes back to grazing is no fight, though it never mends).
    pub fn stirred(&self, id: u32, now: f32, calm_after: f32) -> bool {
        self.shown.get(&id).is_some_and(|shown| in_fight(&shown.view, shown.fought_at, now, calm_after))
    }

    pub fn sighting(&self, id: u32) -> Option<Sighting> {
        let shown = self.shown.get(&id)?;
        let health = if shown.dying.is_some() { 0 } else { shown.view.health };
        let view = &shown.view;
        let keeper = view.kept.then(|| match view.owner {
            Some(owner) if Some(owner) == self.me => Keeper::Us,
            Some(owner) => Keeper::Other(owner),
            None => Keeper::Away,
        });
        Some(Sighting { species: view.species, health, struck: shown.struck, keeper, young: view.young, ready: view.ready })
    }

    /// The creature we struck at in the last `within` seconds before `now`, while it is
    /// still shown (alive, or falling).
    pub fn fighting(&self, now: f32, within: f32) -> Option<u32> {
        self.fighting.filter(|&(id, at)| now - at < within && self.shown.contains_key(&id)).map(|(id, _)| id)
    }

    /// How many creatures are shown, alive.
    pub fn alive(&self) -> usize {
        self.shown.values().filter(|s| s.dying.is_none()).count()
    }

    /// The creatures shown alive, by id: species and where they stand.
    pub fn views(&self) -> impl Iterator<Item = &CreatureView> {
        self.shown.values().filter(|s| s.dying.is_none()).map(|s| &s.view)
    }

    /// The gate checks both eyes in the actual painted skin worn by a summoned shambler.
    /// None means its body or asynchronous skin has not reached the client yet.
    pub(crate) fn front_eyes(&self, launch: &Launch, images: &Assets<Image>) -> Option<Result<String, String>> {
        let species = launch.content.creatures.find("base:shambler")?;
        let shown = self.shown.values().find(|s| s.view.species == species && s.dying.is_none())?;
        if shown.painting.is_some() {
            return None;
        }
        let image = images.get(&shown.skin)?;
        let pixels = image.data.as_ref()?;
        let body = self.bodies.get(&species)?;
        let head = shown.joints.iter().map(|(_, p)| p).find(|p| p.role == creature::Role::Head)?;
        let def = &launch.content.creatures.get(species).def.body;
        let Some(radius) = def.skin.eye.map(|r| r * def.scale) else {
            return Some(Err("the front-eye fixture has no skin.eye radius".into()));
        };
        let [w, h, d] = body.dims.map(|n| n as usize);
        let mut eyes = [[0usize; 3]; 2];
        for (side, counts) in eyes.iter_mut().enumerate() {
            let sign = if side == 0 { -1.0 } else { 1.0 };
            let eye = Vec3::new(sign * head.size.x * 0.24, head.size.y * 0.1, -head.size.z * 0.5);
            for z in 0..d {
                for y in 0..h {
                    for x in 0..w {
                        let at = body.origin + (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / skin::TEXELS;
                        let local = head.rest.inverse() * (at - head.centre);
                        let delta = (local - eye).abs();
                        if delta.x > radius || delta.y > radius || delta.z > 1.0 / skin::TEXELS {
                            continue;
                        }
                        let at = ((z * h + y) * w + x) * 4;
                        let c = &pixels[at..at + 4];
                        if c[3] == 0 {
                            continue;
                        }
                        counts[0] += usize::from(c[..3].iter().all(|&v| v > 170) && c[0].abs_diff(c[2]) < 8);
                        counts[1] += usize::from(c[1] > 140 && c[1] > c[0] && c[2] < 130);
                        counts[2] += usize::from(c[..3].iter().all(|&v| v < 30));
                    }
                }
            }
        }
        let detail = format!("sclera/catchlight, iris, pupil texels on each front eye: {eyes:?}");
        Some(if eyes.iter().all(|counts| counts.iter().all(|&n| n > 0)) { Ok(detail) } else { Err(detail) })
    }

    /// The box a creature is drawn in, by its species' size and its own scale.
    /// The living creature along a ray from `eye`, within `reach` and nearer than `block`.
    pub fn pointed(&self, launch: &Launch, eye: Vec3, look: Vec3, reach: f32, block: f32) -> Option<u32> {
        self.shown
            .keys()
            .filter_map(|&id| Some((id, ray_box(eye, look, &self.aabb(launch, id)?)?)))
            .filter(|&(_, d)| d <= reach && d < block)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Every living creature's body, for the player to bump against (`player::movement`).
    pub fn bodies<'a>(&'a self, launch: &'a Launch) -> impl Iterator<Item = Aabb> + 'a {
        self.shown.keys().filter_map(move |&id| self.aabb(launch, id))
    }

    /// The middle of creature `id`'s body while it is shown, alive or falling, and whether
    /// it is falling: for marks that follow a body to its end (the killing blow's hitsplat).
    /// Whether creature `id` shows a blow now: frozen, flashing white or flinching.
    pub fn jolted(&self, id: u32) -> bool {
        self.shown.get(&id).is_some_and(|shown| shown.stopped > 0.0 || shown.white > 0.0 || shown.flinch.is_some())
    }

    /// Creature `id`'s death as shown: seconds since it died, and whether it has gone up in
    /// smoke.
    pub fn dying(&self, id: u32) -> Option<(f32, bool)> {
        let shown = self.shown.get(&id)?;
        shown.dying.map(|t| (t, shown.poofed))
    }

    /// Seconds into creature `id`'s flinch from a blow, while it flinches.
    pub fn flinching(&self, id: u32) -> Option<f32> {
        self.shown.get(&id)?.flinch
    }

    /// Whether creature `id` is shown, alive, and winding up a blow.
    pub fn winding(&self, id: u32) -> bool {
        self.shown.get(&id).is_some_and(|shown| shown.dying.is_none() && shown.coiling.is_some())
    }

    /// Whether creature `id` is shown and alive, not falling.
    pub fn standing(&self, id: u32) -> bool {
        self.shown.get(&id).is_some_and(|shown| shown.dying.is_none())
    }

    /// Creature `id` while it is shown, alive or falling: its species, where its feet are
    /// and its scale.
    pub fn body(&self, id: u32) -> Option<(CreatureId, Vec3, f32)> {
        let shown = self.shown.get(&id)?;
        Some((shown.view.species, shown.position, shown.view.scale))
    }

    pub fn centre(&self, launch: &Launch, id: u32) -> Option<(Vec3, bool)> {
        let shown = self.shown.get(&id)?;
        let height = launch.content.creatures.get(shown.view.species).def.body.size.1 * shown.view.scale;
        Some((shown.position + Vec3::Y * height * 0.5, shown.dying.is_some()))
    }

    /// The creature nearest along the look ray within `within` blocks of the eye, and how far.
    pub fn looked_at(&self, launch: &Launch, eye: Vec3, look: Vec3, within: f32) -> Option<(u32, f32)> {
        self.shown
            .keys()
            .filter_map(|&id| Some((id, ray_box(eye, look, &self.aabb(launch, id)?)?)))
            .filter(|&(_, distance)| distance <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub(crate) fn aabb(&self, launch: &Launch, id: u32) -> Option<Aabb> {
        let shown = self.shown.get(&id).filter(|s| s.dying.is_none())?;
        let size = launch.content.creatures.get(shown.view.species).def.body.size;
        Some(Aabb::standing(shown.position, size.0 * shown.view.scale, size.1 * shown.view.scale))
    }
}

/// How bright each face is drawn, as the sun lights a box from above: top brightest, then
/// the sides, the bottom darkest. In [`FACES`] order.
const SHADES: [f32; 6] = [1.0, 0.5, 0.8, 0.7, 0.75, 0.85];

/// A unit cube centred on the origin with its faces shaded and its texture coordinates in
/// `rects` of a `layout`-sized texture (each face its whole patch), the corners laid out
/// as the skin painter lays its texels ([`FACES`]).
fn part_mesh(rects: &[skin::Rect; 6], layout: (u32, u32)) -> Mesh {
    let (mut positions, mut normals, mut colours, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for ((face, rect), shade) in FACES.iter().zip(rects).zip(SHADES) {
        let base = positions.len() as u32;
        // Corners counter-clockwise seen from outside: (u, v) of (0,1), (1,1), (1,0), (0,0).
        for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            let corner = face.normal * 0.5 + face.u * (u - 0.5) + face.v * (v - 0.5);
            positions.push(corner.to_array());
            normals.push(face.normal.to_array());
            colours.push([shade, shade, shade, 1.0]);
            uvs.push([(rect.x as f32 + u * rect.w as f32) / layout.0 as f32, (rect.y as f32 + v * rect.h as f32) / layout.1 as f32]);
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// A whole-texture patch on every face, for hearts and warming.
fn whole(size: (u32, u32)) -> [skin::Rect; 6] {
    [skin::Rect { x: 0, y: 0, w: size.0, h: size.1 }; 6]
}

/// A species' body as a skinned mesh: rest positions (in the vertex colour, for the skin),
/// normals, and each vertex's four joints and weights.
fn body_mesh(body: &BodyMesh) -> Mesh {
    let rest: Vec<[f32; 4]> = body.positions.iter().map(|p| [p[0], p[1], p[2], 1.0]).collect();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, body.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, body.normals.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, rest)
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(body.joints.clone()))
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, body.weights.clone())
        .with_inserted_indices(Indices::U32(body.indices.clone()))
}

/// Builds a species' body from its parts.
pub(crate) fn species_body(model: &creature::Model, parts: &[Part], meshes: &mut Assets<Mesh>, binds: &mut Assets<SkinnedMeshInverseBindposes>) -> SpeciesBody {
    let anatomy = creature::anatomy::of(model, parts);
    let body = volume::build(parts, anatomy.as_ref());
    let shape = std::sync::Arc::new(Shape::new(parts, anatomy.as_ref()));
    // Each joint sits at its part's pivot; at rest it moves nothing.
    let inverse: Vec<Mat4> = parts.iter().map(|p| Mat4::from_translation(-p.pivot)).collect();
    let texel = 1.0 / skin::TEXELS;
    let origin = ((body.min / texel).floor() - 1.0) * texel;
    let dims = (((body.max - origin) / texel).ceil() + 1.0).as_uvec3().to_array();
    SpeciesBody { mesh: meshes.add(body_mesh(&body)), binds: binds.add(SkinnedMeshInverseBindposes::from(inverse)), shape, origin, dims }
}

/// Paints an individual's skin through its species' body volume, off the main thread, so
/// a herd coming into view never holds a frame: the body shows once its skin is ready.
pub(crate) fn paint_skin(def: creature::BodyDef, parts: Vec<Part>, body: &SpeciesBody, seed: u32, inspect: bool) -> Task<Image> {
    let (shape, origin, dims) = (body.shape.clone(), body.origin, body.dims);
    AsyncComputeTaskPool::get().spawn(async move {
        let shell = shape.cell * 0.75 + 1.5 / skin::TEXELS;
        // The part each point belongs to, at the body's own surface distance.
        let pixels = skin::paint_volume(&def, &parts, seed, origin, dims, shell, |p| shape.nearest(p).map(|(i, _)| (i, shape.distance(p))));
        let [w, h, d] = dims;
        let extent = Extent3d { width: w, height: h, depth_or_array_layers: d };
        let usage = if inspect { RenderAssetUsages::all() } else { RenderAssetUsages::RENDER_WORLD };
        let mut image = Image::new(extent, TextureDimension::D3, pixels, TextureFormat::Rgba8UnormSrgb, usage);
        image.sampler = nearest();
        image
    })
}

/// What is drawn of bodies: whether each is shown, and where the last frame put its joints.
#[derive(bevy::ecs::system::SystemParam)]
struct Drawn<'w, 's> {
    visible: Query<'w, 's, &'static mut Visibility>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    inspect: Option<Res<'w, InspectEyes>>,
    /// How blows, strikes and deaths show (`combat_fx.ron`); without it, they show plainly.
    look: Option<Res<'w, crate::fight_fx::FightLook>>,
    /// How bodies walk, run, turn, stand about and flee (`creature_motion.ron`).
    motion: Res<'w, Motion>,
}

/// The light creatures are lit by and the air they are fogged by, as the terrain's.
#[derive(bevy::ecs::system::SystemParam)]
struct Air<'w> {
    light: Res<'w, crate::render::SceneLight>,
    atmosphere: Res<'w, crate::render::atmosphere::SharedAtmosphere>,
}

/// How long its legs are, at full size: the mean height of its walking legs' hips (feet
/// stand at the origin), or a crawler's legs' (which reach out as far as they are high).
fn legs_length(parts: &[Part]) -> f32 {
    let (sum, n) = parts
        .iter()
        .filter_map(|p| match (p.swing, p.role) {
            (Swing::Leg { .. }, creature::Role::Leg) | (Swing::Scuttle { .. }, creature::Role::Leg) => Some(p.pivot.y),
            _ => None,
        })
        .fold((0.0, 0), |(s, n), y| (s + y, n + 1));
    if n == 0 { 0.0 } else { sum / n as f32 }
}

/// Its shortest walking leg's length, at full size.
fn shortest_leg(parts: &[Part]) -> f32 {
    parts.iter().filter(|p| matches!(p.swing, Swing::Leg { .. }) && p.role == creature::Role::Leg).map(|p| p.pivot.y).fold(f32::INFINITY, f32::min).min(1e3)
}

/// The near plane's reach: a box this much closer than the eye is already cut open.
const NEAR_PLANE: f32 = 0.1;

/// Whether the eye is inside a living body: inside one of its part boxes (grown by the near
/// plane), in the creature's own frame. A fallen body lies on its side, so a dying one counts
/// within its height of where it stood.
fn eye_inside(parts: &[Part], eye: Vec3, position: Vec3, yaw: f32, scale: f32, height: f32, dying: bool) -> bool {
    let from = eye - position;
    if dying {
        return from.with_y(0.0).length() < height + NEAR_PLANE && (-0.2..height + 0.2).contains(&from.y);
    }
    let local = Quat::from_rotation_y(yaw).inverse() * from / scale.max(1e-3);
    let grow = NEAR_PLANE / scale.max(1e-3);
    parts.iter().any(|part| {
        let inside = part.rest.inverse() * (local - part.centre);
        let half = part.size * 0.5 + Vec3::splat(grow);
        inside.abs().cmple(half).all()
    })
}

/// Whether `item` is a tool that takes one of `species`' products (shears, a pail): which.
fn product_taken_by(species: &creature::Creature, item: voxmar_engine::block::BlockId, blocks: &voxmar_engine::block::BlockRegistry) -> Option<usize> {
    let tags = &blocks.def(item).tags;
    species.def.products.iter().position(|p| p.tool.as_ref().is_some_and(|t| tags.contains(t)))
}

/// An animal just tended, waiting on the server's word to answer: until `until`, it calls
/// (a bleat, a low) once `watched` grows in the pack (its product taken), or munches once
/// it shrinks (the food in hand eaten). Refused, nothing changes and it is silent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tended {
    id: u32,
    watched: voxmar_engine::block::BlockId,
    had: u32,
    answer: Sound,
    until: f32,
    /// The player's actions proposed, and the harmless among them (tending, looking at
    /// or letting go of an animal: [`Tending::harmless`]), as of its own: one proposed
    /// since that is not harmless (seeds sown, a carrot eaten) may change the count itself,
    /// so it no longer waits.
    proposals: u64,
    harmless: u64,
}

/// The animals tended waiting on the server's word, and how many harmless actions the
/// player has proposed this session (see [`Tended::proposals`]).
#[derive(Resource, Default)]
pub struct Tending {
    waiting: Vec<Tended>,
    harmless: u64,
}

/// A new session counts its proposals afresh: nothing tended in the last one waits on.
pub fn forget_tending_on_rejoin(net: Option<Res<Net>>, mut tending: ResMut<Tending>) {
    if net.is_some_and(|net| net.is_added()) {
        *tending = Tending::default();
    }
}

/// Seconds a tended animal waits on the server's word to answer.
const TEND_ANSWER_WITHIN: f32 = 3.0;

impl Tended {
    /// Tending `species` with `held`, of which (or of whose product) the pack has
    /// `count(item)`.
    fn new(id: u32, species: &creature::Creature, held: voxmar_engine::block::BlockId, blocks: &voxmar_engine::block::BlockRegistry, count: impl Fn(voxmar_engine::block::BlockId) -> u32, now: f32) -> Self {
        let (watched, answer) = match product_taken_by(species, held, blocks) {
            Some(product) => (species.products[product], Sound::Idle),
            None => (held, Sound::Graze),
        };
        Tended { id, watched, had: count(watched), answer, until: now + TEND_ANSWER_WITHIN, proposals: 0, harmless: 0 }
    }

    /// Whether the player has proposed something not harmless since this one, as of
    /// `proposals` proposed and `harmless` of them (counts that only grow within a session).
    fn something_else_since(&self, proposals: u64, harmless: u64) -> bool {
        proposals.saturating_sub(self.proposals) > harmless.saturating_sub(self.harmless)
    }

    /// Whether the pack now holding `count` of what is watched says it was done, as the
    /// tests check each tend's direction (`answer_tended` counts how many at once).
    #[cfg(test)]
    fn done(&self, count: u32) -> bool {
        match self.answer {
            Sound::Idle => count > self.had,
            _ => count < self.had,
        }
    }
}

/// The animals among `waiting` the server has now done, in the order they were tended: as
/// the server takes them in turn, a watched item's count rising answers the earliest still
/// waiting on it (a product's count says nothing of how many gave it), and falling by `n`
/// answers the `n` earliest (each meal is one of the food); those after wait on from the
/// new count. Ones past their time, or with something else done `meanwhile`, are dropped
/// unanswered.
fn answer_tended(waiting: &mut Vec<Tended>, now: f32, meanwhile: impl Fn(&Tended) -> bool, count: impl Fn(voxmar_engine::block::BlockId) -> u32) -> Vec<Tended> {
    waiting.retain(|t| now <= t.until && !meanwhile(t));
    let mut answered = Vec::new();
    let mut seen = Vec::new();
    let mut i = 0;
    while i < waiting.len() {
        let watched = waiting[i].watched;
        if seen.contains(&watched) {
            i += 1;
            continue;
        }
        seen.push(watched);
        let (held, had) = (count(watched), waiting[i].had);
        let answers = match waiting[i].answer {
            Sound::Idle if held > had => 1,
            Sound::Idle => 0,
            _ => had.saturating_sub(held) as usize,
        };
        let mut taken = 0;
        let mut j = i;
        while j < waiting.len() {
            if waiting[j].watched != watched {
                j += 1;
            } else if taken < answers {
                answered.push(waiting.remove(j));
                taken += 1;
            } else {
                if taken > 0 {
                    waiting[j].had = held;
                }
                j += 1;
            }
        }
        if taken == 0 {
            i += 1;
        }
    }
    answered
}

/// A sound a creature makes now.
fn sound(kind: Sound, id: u32, view: &CreatureView, position: Vec3, speed: f32) -> CreatureSound {
    CreatureSound { kind, id, species: view.species, position, speed, scale: view.scale, young: view.young, answering: false, chatter: false }
}

/// Applies what the server sent, then moves and animates every creature, and handles
/// aiming at and striking them.
/// Where the creatures are shown each frame: what they did is heard after it
/// (`creature_sounds`).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShowCreatures;

/// One frame of `dt` on a creature's clocks: a hit-stop (`stopped`) holds its body still,
/// but not its wind-up, whose blow comes by the server's clock whatever is drawn meanwhile
/// (a freeze rounds up to whole frames, so on slow ones blows landing through a wind-up
/// would otherwise leave it barely coiled as it struck). The time its body moves by.
fn frame_clocks(stopped: &mut f32, coiling: Option<&mut f32>, dt: f32) -> f32 {
    let frozen = *stopped > 0.0;
    *stopped = (*stopped - dt).max(0.0);
    if let Some(coil) = coiling {
        *coil += dt;
    }
    if frozen { 0.0 } else { dt }
}

/// How far into its wind-up a creature was when the server's word that its blow came
/// arrived, on a frame `dt` after the last: the word came in somewhere over that frame, which
/// its coil has not counted yet.
fn word_reached(coil: f32, dt: f32) -> f32 {
    coil + dt
}

#[allow(clippy::too_many_arguments)]
fn show(
    mut commands: Commands,
    time: Res<Time>,
    launch: Res<Launch>,
    world: Res<ClientWorld>,
    air: Air,
    player: Res<LocalPlayer>,
    mut creatures: ResMut<Creatures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut skins: ResMut<Assets<SkinMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut transforms: Query<&mut Transform>,
    mut looks: Query<&mut MeshMaterial3d<StandardMaterial>>,
    mut drawn: Drawn,
    mut sounds: ResMut<CreatureSounds>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    // Their pipelines (a skinned box, and a heart's clipped one) compile as the world loads
    // (`render::warm`), not when the first creature comes into view.
    // Nothing is drawn until the renderer has made the air they are fogged by.
    let Some(atmosphere) = air.atmosphere.get() else { return };
    let scene_light = &air.light;
    let (heart_mesh, heart_full, heart_empty) = creatures
        .heart
        .get_or_insert_with(|| {
            let image = images.add(heart_image());
            let mesh = meshes.add(part_mesh(&whole((1, 1)), (1, 1)));
            // A body's pipeline: a skinned box with a one-texel skin.
            let cube = BodyMesh {
                positions: FACES.iter().flat_map(|f| [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(a, b)| (f.normal * 0.5 + f.u * a + f.v * b).to_array())).collect(),
                normals: FACES.iter().flat_map(|f| [f.normal.to_array(); 4]).collect(),
                joints: vec![[0; 4]; 24],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 24],
                indices: (0..6u32).flat_map(|f| [0, 2, 1, 0, 3, 2].map(|i| f * 4 + i)).collect(),
                ..default()
            };
            let pixel = Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D3, vec![255; 4], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
            let body = skins.add(SkinMaterial { base: StandardMaterial { unlit: true, ..default() }, extension: CreatureSkin { skin: images.add(pixel), frame: SkinFrame { origin: Vec4::new(-1.0, -1.0, -1.0, skin::TEXELS), size: Vec4::splat(2.0) }, atmosphere: atmosphere.clone() } });
            let warm_binds = binds.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY]));
            crate::render::warm::warm_skinned(&mut commands, meshes.add(body_mesh(&cube)), body, warm_binds, DespawnOnExit(Screen::InGame));
            let heart = |colour: Color| StandardMaterial { base_color: colour, base_color_texture: Some(image.clone()), alpha_mode: AlphaMode::Mask(0.5), unlit: true, ..default() };
            let full = materials.add(heart(Color::srgb(0.95, 0.16, 0.24)));
            let empty = materials.add(heart(Color::srgb(0.3, 0.27, 0.3)));
            // Hearts cast no shadow, but a clipped material's depth pipeline is warmed too, in
            // case any pass draws them.
            crate::render::warm::warm(&mut commands, mesh.clone(), full.clone(), true, DespawnOnExit(Screen::InGame));
            (mesh, full, empty)
        })
        .clone();
    let registry = &launch.content.creatures;

    let creatures = &mut *creatures;
    for gone in std::mem::take(&mut creatures.left) {
        creatures.waiting.retain(|view| view.id != gone.id);
        match creatures.shown.get_mut(&gone.id) {
            Some(shown) if gone.died => {
                (shown.dying, shown.since_death) = (Some(0.0), 0.0);
                // The killing blow freezes it a moment, flashing, before it tips over.
                if let Some(look) = drawn.look.as_deref() {
                    shown.stopped = shown.stopped.max(look.death.stop);
                    shown.stop_from = shown.stopped;
                    shown.white = look.hit.white;
                }
                (shown.coiling, shown.striking, shown.flinch) = (None, None, None);
                sounds.push(sound(Sound::Death, gone.id, &shown.view, shown.position, 0.0));
                creatures.deaths += 1;
            }
            Some(shown) => {
                despawn(&mut commands, shown);
                if let Some(shown) = creatures.shown.remove(&gone.id) {
                    if shown.painting.is_none() {
                images.remove(&shown.skin);
            }
                    skins.remove(&shown.material);
                }
            }
            // Never shown here (it died out of sight): nothing to cry.
            None if gone.died => debug!("creature {} died unseen: no death cry", gone.id),
            None => {}
        }
    }
    let cried = std::mem::take(&mut creatures.cries);
    for &id in &cried {
        if let Some(shown) = creatures.shown.get(&id) {
            sounds.push(sound(Sound::Hurt, id, &shown.view, shown.position, 0.0));
        }
    }
    for (id, bark) in creatures.barks_heard() {
        if let Some(shown) = creatures.shown.get(&id) {
            sounds.push(sound(Sound::of_bark(bark), id, &shown.view, shown.position, 0.0));
        }
    }
    // Our blows that landed: a freeze, a white flash and a flinch away from us, as large as
    // the share of its health the blow took (`combat_fx.ron`'s `hit`). Bleeding only reddens.
    let eye = player.eye();
    let fx = drawn.look.as_deref();
    let moves = &*drawn.motion;
    let red_seconds = fx.map_or(FLASH, |fx| fx.hit.red.max(1e-3));
    for (id, damage, blow, killed) in std::mem::take(&mut creatures.jolts) {
        let (Some(look), Some(shown)) = (fx, creatures.shown.get_mut(&id)) else { continue };
        if damage == 0 {
            continue;
        }
        shown.flash = red_seconds;
        if blow == Blow::Bleed {
            continue;
        }
        let def = &registry.get(shown.view.species).def;
        let share = f32::from(damage) / f32::from(def.health.max(1));
        let stop = crate::fight_fx::stop_for(look, blow, share, killed);
        if stop > shown.stopped {
            (shown.stopped, shown.stop_from) = (stop, stop);
        }
        shown.white = look.hit.white;
        if shown.dying.is_none() && !killed {
            shown.flinch = Some(0.0);
            shown.flinch_away = (shown.position - eye).with_y(0.0).normalize_or(Vec3::X);
            let scale = shown.view.scale;
            shown.flinch_size = crate::fight_fx::flinch_size(&look.hit.weigh, (def.body.size.0 * scale, def.body.size.1 * scale), share);
            let size = shown.flinch_size;
            if creatures.watch == Some(id) {
                creatures.watched.flinch_size = size;
            }
        }
    }
    let mut built = 0;
    let views: Vec<CreatureView> = std::mem::take(&mut creatures.waiting).into_iter().chain(std::mem::take(&mut creatures.inbox)).collect();
    for view in views {
        if let Some(shown) = creatures.shown.get_mut(&view.id) {
            // Hurt by someone else's blow (ours cry as the server answers them, below).
            if view.health < shown.view.health && !cried.contains(&view.id) {
                shown.flash = FLASH;
                sounds.push(sound(Sound::Hurt, view.id, &view, shown.position, 0.0));
            }
            // What it does now, heard as it starts.
            if view.pose != shown.view.pose {
                let kind = match view.pose {
                    Pose::Grazing => Some(Sound::Graze),
                    Pose::Alert => Some(Sound::Alert),
                    Pose::Resting => Some(Sound::Rest),
                    Pose::Attacking => Some(Sound::Attack),
                    Pose::Idle | Pose::Crouched => None,
                };
                if let Some(kind) = kind {
                    sounds.push(sound(kind, view.id, &view, shown.position, 0.0));
                }
            }
            if view.ready && !shown.view.ready {
                sounds.push(sound(Sound::Ready, view.id, &view, shown.position, 0.0));
            }
            // A wind-up starts its coil; its end is the blow, struck now unless the wind-up's
            // own time struck it already. Staggered, its wind-up is over and it reels
            // (`fight_fx`'s stagger): after its blow if that showed (a perfect block parries
            // a blow that came), else with none (a daze cuts the wind-up short).
            match turn(&shown.view, &view, shown.coiling.is_some(), shown.coil_struck, shown.striking.is_some(), shown.dying.is_some()) {
                Turn::Staggered { struck } => {
                    let interrupted = if shown.coiling.is_some() || shown.striking.is_some() { Some(shown.move_shown) } else { None };
                    shown.coiling = None;
                    // A strike still showing keeps its move's look until it is out.
                    if shown.striking.is_none() {
                        shown.move_shown = 0;
                    }
                    shown.staggered = Some(0.0);
                    if shown.dying.is_none() {
                        let def = &registry.get(view.species).def;
                        let height = def.body.size.1 * view.scale;
                        let forward = Quat::from_rotation_y(shown.yaw) * Vec3::NEG_Z;
                        let head = shown.position + forward * def.body.size.0 * view.scale * 0.35 + Vec3::Y * height * 1.1;
                        let coat = def.body.colours.first().copied().unwrap_or([200, 200, 200]);
                        creatures.staggers.push((head, height, coat));
                    }
                    creatures.staggers_seen += 1;
                    shown.stagger_read = Some(creatures.stagger_reads.len());
                    creatures.stagger_reads.push(StaggerRead { id: view.id, struck, deepest: 0.0, frames: 0, interrupted });
                }
                Turn::WindsUp => wind_up(shown, &view, registry, launch.content.combat.feel.creature_windup, false, &mut creatures.told),
                Turn::Strikes => {
                    shown.striking = Some(0.0);
                    shown.coil_reached = word_reached(shown.coiling.unwrap_or(0.0), dt);
                    (shown.read_due, shown.coil_word) = (true, true);
                    creatures.strikes += 1;
                    shown.coiling = None;
                }
                Turn::Ends => shown.coiling = None,
                Turn::Same => {}
            }
            if shows_fight(Some(&shown.view), &view) {
                shown.fought_at = Some(now);
            }
            shown.view = view;
            shown.told_at = now;
            continue;
        }
        if view.species.0 as usize >= registry.len() {
            continue;
        }
        // A herd coming into view at once is built a few a frame: painting a skin and
        // spawning a model took a frame 4.5 ms longer for a handful.
        if built == SPAWNS || creatures.waiting.iter().any(|waiting| waiting.id == view.id) {
            creatures.waiting.push(view);
            continue;
        }
        built += 1;
        let species = registry.get(view.species);
        let body = creatures.bodies.entry(view.species).or_insert_with(|| species_body(&species.def.body.model, &species.parts, &mut meshes, &mut binds));
        // Painted as its genes show: its coat colours and markings.
        let genes = &species.def.genes;
        let traits = voxmar_engine::genes::express(genes, &voxmar_engine::genes::unpack(genes, &view.genes));
        let painting = Some(paint_skin(species.def.body.with(&traits), species.parts.clone(), body, view.seed, drawn.inspect.is_some() && species.def.name == "base:shambler"));
        let skin = creatures.blank.get_or_insert_with(|| images.add(blank_skin())).clone();
        let size = Vec3::from_array(body.dims.map(|n| n as f32)) / skin::TEXELS;
        let frame = SkinFrame { origin: body.origin.extend(skin::TEXELS), size: size.extend(0.0) };
        let material = skins.add(SkinMaterial { base: StandardMaterial { unlit: true, ..default() }, extension: CreatureSkin { skin: skin.clone(), frame, atmosphere: atmosphere.clone() } });
        let entity = commands.spawn((Transform::from_translation(view.position), Visibility::default(), DespawnOnExit(Screen::InGame))).id();
        let joints: Vec<(Entity, Part)> = species.parts.iter().map(|&part| (commands.spawn((Transform::from_translation(part.pivot), ChildOf(entity))).id(), part)).collect();
        let skinned = SkinnedMesh { inverse_bindposes: body.binds.clone(), joints: joints.iter().map(|(e, _)| *e).collect() };
        let body = commands.spawn((Mesh3d(body.mesh.clone()), MeshMaterial3d(material.clone()), skinned, Transform::default(), Visibility::default(), ChildOf(entity))).id();
        let fought_at = shows_fight(None, &view).then_some(now);
        creatures.shown.insert(
            view.id,
            Shown {
                view,
                told_at: now,
                fought_at,
                position: view.position,
                yaw: view.yaw,
                entity,
                body,
                joints,
                material,
                painting,
                skin,
                stride: 0.0,
                spin: 0.0,
                flee: 0.0,
                pace: 0.0,
                planted: Vec::new(),
                holds: Vec::new(),
                planted_before: Vec::new(),
                feet: Vec::new(),
                drawn_at: None,
                quiet: 2.0 + creature::unit_from(creature::mix(view.seed as u64, 77)) * 8.0,
                flash: 0.0,
                dying: None,
                fall_side: 0.0,
                death_strayed: 0.0,
                death_around_eye: false,
                death_spread: 0.0,
                death_scale: 0.0,
                death_samples: 0,
                death_slowest: 0.0,
                since_death: 0.0,
                struck: false,
                nod: 0.0,
                look: 0.0,
                crouch: 0.0,
                rear: 0.0,
                spread: 0.0,
                fell: 0.0,
                hearts: Vec::new(),
                stopped: 0.0,
                stop_from: 0.0,
                white: 0.0,
                flinch: None,
                flinch_away: Vec3::X,
                flinch_size: 1.0,
                coiling: None,
                striking: None,
                coil_struck: false,
                coil_from: Vec3::ZERO,
                coil_far: 0.0,
                coil_kept: 1.0,
                coil_slid: 0.0,
                coil_deepest: 0.0,
                recover_most: 0.0,
                coil_reached: 0.0,
                coil_word: false,
                read_due: false,
                poofed: false,
                move_shown: 0,
                // One first seen staggered reels from the start, unheard and with no sparks.
                staggered: (view.tell == voxmar_protocol::STAGGERED).then_some(0.0),
                stagger_depth: 0.0,
                stagger_read: None,
            },
        );
        // One first seen already winding up (come into view, or joined, mid-blow) coils and
        // is told at once.
        if view.pose == Pose::Attacking
            && let Some(shown) = creatures.shown.get_mut(&view.id)
        {
            wind_up(shown, &view, registry, launch.content.combat.feel.creature_windup, true, &mut creatures.told);
        }
    }

    let me = creatures.me;
    let mut finished = Vec::new();
    let (mut poofs, mut strikes, mut reads, mut recoveries) = (Vec::new(), 0, Vec::new(), Vec::new());
    let feel = &launch.content.combat.feel;
    for (&id, shown) in creatures.shown.iter_mut() {
        let def = &registry.get(shown.view.species).def;
        let stepped = creatures.watch_step.filter(|_| creatures.watch == Some(id));
        let dt = stepped.unwrap_or(dt);
        // A blow freezes it a moment (a hit-stop): it holds still where it was, shaking,
        // and its flinch, flash and fall wait for the freeze to end.
        let (frame, frozen) = (dt, shown.stopped > 0.0);
        let shake = if frozen { shown.stopped / shown.stop_from.max(1e-3) } else { 0.0 };
        let dt = frame_clocks(&mut shown.stopped, shown.coiling.as_mut(), dt);
        // Carry on along its velocity a little past the last word, then ease toward it.
        let ahead = if stepped.is_some() { 0.0 } else { (now - shown.told_at).min(AHEAD) };
        let target = shown.view.position + shown.view.velocity.with_y(0.0) * ahead;
        let before = shown.position;
        // A boss's move strikes in its own time and style: the lash a swipe, the grasp a
        // heavy crush.
        let attack = match shown.move_shown {
            0 => def.attack.as_ref(),
            index => def.moves.get(index as usize - 1).map(|m| &m.attack).or(def.attack.as_ref()),
        };
        let windup = attack.and_then(|a| a.windup).unwrap_or(feel.creature_windup).max(0.05);
        let recovery = attack.and_then(|a| a.recovery).unwrap_or(feel.creature_recovery).max(0.0);
        let strike = fx.zip(attack).map(|(fx, attack)| fx.strike(attack.style));
        // Winding up it plants its feet, following where the server has it only that much;
        // as the blow snaps out it darts the rest of the way.
        let coiled = shown.coiling.is_some() && !shown.coil_struck;
        let follow = match strike {
            Some(strike) if coiled => FOLLOW * strike.plant,
            Some(strike) if shown.striking.is_some_and(|s| s < strike.snap * 2.0) => FOLLOW * strike.dart,
            _ => FOLLOW,
        };
        shown.position = shown.position.lerp(target, (follow * dt).min(1.0));
        if coiled {
            let plant = strike.map_or(1.0, |strike| strike.plant);
            shown.coil_kept *= 1.0 - (FOLLOW * plant * dt).min(1.0);
            shown.coil_far = shown.coil_far.max(target.distance(shown.coil_from));
            shown.coil_slid = shown.coil_slid.max(shown.position.distance(shown.coil_from));
        }
        if shown.position.distance(target) > 6.0 {
            shown.position = target;
        }
        // It turns smoothly, no faster than its kind can (a little faster, to keep up with
        // the server), its head ahead of the turn and its body leaning into it.
        let most = def.movement.turn.to_radians() * moves.turn.share;
        (shown.yaw, shown.spin) = motion::turn_toward(shown.yaw, shown.spin, shown.view.yaw, most, moves.turn.ease, dt);
        let turn_left = (shown.view.yaw - shown.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        // Its pace, smoothed (the shown position eases after the server's, a little
        // unevenly), and held through a freeze so its legs hold where they were.
        let moved = (shown.position - before).with_y(0.0).length();
        if dt > 0.0 {
            shown.pace += (moved / dt - shown.pace) * (10.0 * dt).min(1.0);
        }
        let speed = shown.pace;
        let scale = shown.view.scale;
        let height = def.body.size.1 * scale;
        // Its stride, worked out from its legs, so a foot on the ground stays put: the
        // ground it covered this frame over a stride's length (`creature_motion`). Turning
        // on the spot, it steps round.
        let gaits = moves.gaits(&def.name, def.movement.style);
        let parts = &registry.get(shown.view.species).parts;
        let hip = legs_length(parts) * scale;
        let stretched = 1.0 + moves.flee.stretch * shown.flee;
        let fleeing = motion::run_share(speed, def.movement.walk) > 0.5
            && shown.view.pose != Pose::Attacking
            && shown.coiling.is_none()
            && shown.dying.is_none()
            && (def.attack.is_none() || shown.view.velocity.with_y(0.0).dot((shown.position - eye).with_y(0.0)) > 0.0);
        shown.flee += (f32::from(u8::from(fleeing)) - shown.flee) * (6.0 * dt).min(1.0);
        if shown.dying.is_some() {
            shown.flee = 0.0;
        }
        let pivoting = shown.spin.abs() * def.body.size.0.max(0.3) * scale * moves.turn.step;
        let going = speed + pivoting * (1.0 - (speed / def.movement.walk.max(0.2)).min(1.0));
        let (gait, paired) = motion::gait_at(&gaits, going, def.movement.walk, 1.0 + (moves.flee.reach - 1.0) * shown.flee);
        // Half the ground each foot covers while it is down; never more than its shortest
        // leg can reach, so no leg is asked to reach past straight out (and slide).
        let travel = (hip * gait.swing.sin()).min(shortest_leg(parts) * scale * stretched * 0.95);
        let before_step = (shown.stride * 2.0).floor();
        if travel > 1e-3 {
            let turned = going - speed;
            shown.stride += (moved + turned * dt) / motion::stride_length(travel, &gait);
        }
        // A footfall each half stride, while it walks on something.
        if (shown.stride * 2.0).floor() != before_step && speed > 0.3 && shown.dying.is_none() && def.movement.style != Style::Drifter {
            sounds.push(sound(Sound::Step, id, &shown.view, shown.position, speed));
        }
        // Now and then, at its own pace, a contented noise, or a call to its kind (by the
        // hour, as its voice says).
        shown.quiet -= dt;
        if shown.quiet <= 0.0 && shown.dying.is_none() {
            let roll = creature::unit_from(creature::mix(shown.view.seed as u64, (now * 10.0) as u64));
            sounds.push(CreatureSound { chatter: true, ..sound(Sound::Idle, id, &shown.view, shown.position, speed) });
            shown.quiet = 6.0 + roll * 18.0;
        }
        shown.flash = (shown.flash - dt).max(0.0);
        // Its wind-up and strike (`combat_fx.ron`'s `strikes`, by how it attacks): coiled
        // through the wind-up, snapping forward as the blow lands (the wind-up's time, or
        // the server's word, whichever comes first), settling back and standing out its
        // recovery.
        if let Some(coil) = shown.coiling {
            if coil >= windup && !shown.coil_struck && shown.dying.is_none() {
                shown.coil_struck = true;
                shown.striking = Some(0.0);
                shown.coil_reached = coil;
                (shown.read_due, shown.coil_word) = (true, false);
                strikes += 1;
            }
        }
        let struck_before = shown.striking;
        if let Some(striking) = shown.striking.as_mut() {
            *striking += dt;
        }
        let coil = shown.coiling.filter(|_| !shown.coil_struck).map(|c| c / windup);
        let (stance, struck_out) = strike.map_or((Default::default(), true), |strike| crate::fight_fx::strike_stance(&strike, coil, shown.striking, recovery));
        if let (Some(strike), Some(c)) = (strike, coil) {
            shown.coil_deepest = shown.coil_deepest.max(crate::fight_fx::coil_share(&strike, c));
        }
        if let (Some(strike), Some(s)) = (strike, shown.striking) {
            // As drawn this frame, and at its deepest if that came between this frame and
            // the last (a slow frame steps over it).
            let peak = crate::fight_fx::recover_peak(&strike, recovery);
            let passed = struck_before.unwrap_or(0.0) < peak && s >= peak;
            let at = if passed { peak } else { s };
            shown.recover_most = shown.recover_most.max(crate::fight_fx::recover_share(&strike, at, recovery));
        }
        // Its wind-up is written down as the blow strikes, whatever comes after (it may die
        // or wind up again before its recovery is out); its recovery once it has stood it out.
        if let Some(strike) = strike.filter(|_| shown.read_due) {
            shown.coil_deepest = shown.coil_deepest.max(crate::fight_fx::coil_share(&strike, (shown.coil_reached / windup).min(1.0)));
            let style = attack.map_or("", |a| a.style.key());
            reads.push(shown.coil_read(&def.name, style, windup));
            shown.read_due = false;
        }
        if struck_out && strike.is_some_and(|s| shown.striking.is_some() && s.recover != Default::default()) {
            recoveries.push(shown.recover_most);
        }
        if struck_out {
            shown.striking = None;
        }
        // Staggered, it reels where it stands (`combat_fx.ron`'s `stagger`): eased into its
        // stagger stance by game time, swaying, whatever of its strike still shows giving way
        // to it; eased back out once the stagger is over. The most of it shown is kept for
        // the gate.
        let reeling = shown.view.tell == voxmar_protocol::STAGGERED && shown.dying.is_none();
        if let Some(t) = shown.staggered.as_mut() {
            *t += dt;
        }
        let stance = match (fx, shown.staggered) {
            (Some(fx), Some(t)) => {
                shown.stagger_depth = crate::fight_fx::stagger_depth(&fx.stagger, shown.stagger_depth, reeling, dt);
                let share = crate::fight_fx::stagger_share(shown.stagger_depth);
                if let Some(read) = shown.stagger_read.and_then(|i| creatures.stagger_reads.get_mut(i)) {
                    read.deepest = read.deepest.max(share);
                    read.frames += u32::from(reeling);
                }
                stance.scaled(1.0 - share).plus(crate::fight_fx::stagger_stance(&fx.stagger, shown.stagger_depth, t))
            }
            _ => stance,
        };
        if !reeling && shown.stagger_depth <= 0.0 {
            (shown.staggered, shown.stagger_read, shown.stagger_depth) = (None, None, 0.0);
        }
        // Its flinch from our blow: pushed back and tipped away, squashed, then easing back.
        if let Some(flinch) = shown.flinch.as_mut() {
            *flinch += dt;
        }
        let flinch = match (shown.flinch, fx) {
            (Some(t), Some(fx)) if t < fx.hit.flinch && shown.dying.is_none() => crate::fight_fx::flinch_weight(t, fx.hit.flinch),
            _ => {
                shown.flinch = None;
                0.0
            }
        };

        let (nod, crouch) = match shown.view.pose {
            Pose::Grazing => (0.8, 0.0),
            Pose::Alert => (-0.25, 0.0),
            Pose::Resting => (0.35, 0.18),
            Pose::Crouched => (0.1, 0.25),
            Pose::Attacking => (0.3, 0.05),
            Pose::Idle => (0.0, 0.0),
        };
        // A boss's move shows its own tell, so each reads apart; rearing into a new phase,
        // it rears right up. Staggered, its stagger stance (above) is what shows.
        let (nod, crouch, rear) = match shown.view.tell {
            voxmar_protocol::REARING => (-0.4, 0.0, 1.0),
            voxmar_protocol::STAGGERED => (nod, crouch, 0.0),
            0 => (nod, crouch, 0.0),
            index => {
                let tell = registry.get(shown.view.species).tell_at(index);
                (tell.nod, tell.crouch * 0.3, tell.rear)
            }
        };
        // Standing about, now and then it sniffs at the ground; running away, its head
        // goes out low.
        let idle = shown.view.pose == Pose::Idle && speed < 0.2 && shown.coiling.is_none() && shown.staggered.is_none() && shown.dying.is_none();
        let sniff = motion::now_and_then(&moves.idle.sniff, now, shown.view.seed, 2).filter(|_| idle).map_or(0.0, |t| (t * std::f32::consts::TAU * 2.0).sin().abs());
        let nod = nod + sniff * moves.idle.sniff.size + moves.flee.head * shown.flee;
        shown.nod += (nod - shown.nod) * (6.0 * dt).min(1.0);
        shown.crouch += (crouch - shown.crouch) * (6.0 * dt).min(1.0);
        shown.rear += (rear - shown.rear) * (5.0 * dt).min(1.0);
        // Watching the player who comes near, unless grazing or running.
        let to_eye = eye - shown.position;
        let watching = to_eye.length() < WATCHES_WITHIN && shown.view.pose != Pose::Grazing && speed < def.movement.walk * 1.2 && shown.dying.is_none();
        let look = if watching {
            let facing = (-to_eye.x).atan2(-to_eye.z);
            ((facing - shown.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).clamp(-WATCH_TURN, WATCH_TURN)
        } else if let Some(t) = motion::now_and_then(&moves.idle.glance, now, shown.view.seed, 1).filter(|_| idle) {
            // Standing about, it glances aside now and then, one way or the other.
            let side = if creature::unit_from(creature::mix(shown.view.seed as u64, motion::slot(&moves.idle.glance, now, shown.view.seed, 1))) < 0.5 { -1.0 } else { 1.0 };
            side * moves.idle.glance.size * (t * std::f32::consts::PI).sin().min(0.6) / 0.6
        } else {
            0.0
        };
        // Its head goes ahead of a turn.
        let look = (look + turn_left * moves.turn.head_lead).clamp(-WATCH_TURN, WATCH_TURN);
        shown.look += (look - shown.look) * (4.0 * dt).min(1.0);

        // A flier spreads its wings off the ground, and falls when it dies in the air.
        let flier = def.movement.style == Style::Flier;
        let aloft = flier && !world.is_solid((shown.position - Vec3::Y * 0.1).floor().as_ivec3());
        shown.spread += (f32::from(u8::from(aloft && shown.dying.is_none())) - shown.spread) * (8.0 * dt).min(1.0);
        let falling = aloft && shown.dying.is_some() && shown.fell < FALLS_MOST;
        if falling {
            shown.fell += FALLS * dt;
            shown.position.y -= FALLS * dt;
            shown.view.position = shown.position;
            shown.view.velocity = Vec3::ZERO;
        }
        let moving = (going / (def.movement.walk.max(0.5) * 1.5)).min(1.0) * (1.0 - shown.spread);
        let running = speed > def.movement.walk * 1.6;
        // The body rides high as each foot passes under it, or in the air of a hop or a
        // bound, and a running body rocks nose to tail.
        let lifts = motion::bob(shown.stride, &gait, paired);
        let bob = gait.bob * lifts * moving;
        let pitch = gait.pitch * (shown.stride * std::f32::consts::TAU).sin() * moving * (1.0 - shown.spread);
        let bob = match def.movement.style {
            Style::Drifter => (now * 1.7 + id as f32).sin() * 0.06,
            // Small birds rise and dip as they fly, a burst of wingbeats and a glide.
            Style::Flier => bob * (1.0 - shown.spread) + shown.spread * flight_bound(now, id).0 * UNDULATE / height.max(0.05),
            _ => bob,
        };

        let rear = (shown.rear + stance.rear).clamp(0.0, 1.0);
        // Leaning into a turn, and standing about, its weight shifting side to side.
        let lean = (shown.spin * speed * moves.turn.lean).clamp(-moves.turn.lean_most, moves.turn.lean_most);
        let still = 1.0 - moving.max(shown.flee);
        let shift = (now * 0.55 + id as f32 * 1.3).sin() * moves.idle.shift * still * f32::from(u8::from(shown.dying.is_none()));
        let mut root = Transform::from_translation(shown.position + Vec3::Y * height * (bob - shown.crouch - stance.crouch))
            .with_rotation(Quat::from_rotation_y(shown.yaw + stance.twist) * Quat::from_rotation_x(rear * REAR + pitch) * Quat::from_rotation_z(lean + shift))
            .with_scale(Vec3::splat(scale) * Vec3::new(1.0, 1.0, 1.0 + moves.flee.stretch * shown.flee));
        // A running body rocks about its hips, not its feet: the hips stay over the ground
        // the feet are planted on.
        if pitch != 0.0 {
            let hips = root.scale * Vec3::Y * legs_length(parts);
            let level = Quat::from_rotation_y(shown.yaw + stance.twist) * Quat::from_rotation_x(rear * REAR) * Quat::from_rotation_z(lean + shift);
            root.translation += (level * hips - root.rotation * hips).with_y(0.0);
        }
        // A heavy blow's wind-up trembles, so it reads as one to get out of the way of.
        if attack.is_some_and(|a| a.heavy) && coil.is_some() {
            let shiver = Vec3::new((now * 61.0 + id as f32).sin(), 0.0, (now * 47.0).cos());
            root.translation += shiver * moves.tells.heavy_tremble * height * coil.unwrap_or(0.0);
        }
        let forward = Quat::from_rotation_y(shown.yaw) * Vec3::NEG_Z;
        root.translation += forward * stance.lean * def.body.size.0.max(def.body.size.1) * scale;
        if let Some(fx) = fx.filter(|_| flinch > 0.0) {
            let away = shown.flinch_away;
            let flinch = flinch * shown.flinch_size;
            root.translation += away * fx.hit.recoil * height * flinch;
            root.rotation = Quat::from_axis_angle(Vec3::Y.cross(away).normalize_or(Vec3::X), fx.hit.tilt.to_radians() * flinch) * root.rotation;
            let squash = (fx.hit.squash * flinch).min(0.9);
            root.scale *= Vec3::new(1.0 + squash * 0.5, 1.0 - squash, 1.0 + squash * 0.5);
        }
        if shake > 0.0 && let Some(fx) = fx {
            let wobble = Vec3::new((now * 157.0 + id as f32).sin(), 0.0, (now * 131.0 + id as f32 * 1.7).cos());
            root.translation += wobble * fx.hit.shake * shake;
        }
        if let Some(dying) = shown.dying.as_mut() {
            // It topples once the killing blow's freeze is over (by `dying`), and goes up in
            // smoke as its loot comes out (by `since_death`, which the freeze does not hold).
            // One that died in the air rolls over as it falls, and lands rolled over.
            let death = &def.death;
            shown.since_death += frame;
            if *dying < death.fall {
                shown.death_slowest = shown.death_slowest.max(dt);
            }
            *dying = if falling { (*dying + dt).min(death.fall) } else { *dying + dt };
            // It falls away from the eye: rolled toward the eye at melee range, its back
            // came through the camera and showed the inside of the body.
            if shown.fall_side == 0.0 {
                let local = Quat::from_rotation_y(shown.yaw).inverse() * (eye - shown.position);
                shown.fall_side = if local.x >= 0.0 { 1.0 } else { -1.0 };
            }
            // It topples, quicker as it goes, and lies there.
            let t = (shown.since_death / death.drops_after().max(0.05)).min(1.0);
            let over = (*dying / death.fall.max(0.01)).min(1.0);
            root.rotation *= Quat::from_rotation_z(shown.fall_side * over * over * death.roll.to_radians());
            // How far it strays from where it stood as it falls, in heights, for the gate
            // (before it sinks away, which is meant to take it out of sight).
            let middle = root.translation + root.rotation * Vec3::Y * height * 0.5;
            let strayed = middle.distance(shown.position + Vec3::Y * height * 0.5) / height.max(0.1);
            shown.death_strayed = shown.death_strayed.max(strayed);
            // Over the last of its death it sinks and shrinks away, and as it goes it is a
            // puff of smoke where it lay, its loot bursting out (the server's `drops_after`).
            let shrink = fx.map_or(0.0, |fx| fx.death.shrink);
            let going = if shrink > 0.0 && t > 1.0 - shrink { ((t - (1.0 - shrink)) / shrink).min(1.0) } else { 0.0 };
            root.translation.y -= going * death.sink * height;
            root.scale *= (1.0 - going).clamp(0.02, 1.0);
            if creatures.watch == Some(id) {
                let w = &mut creatures.watched;
                (w.death_roll, w.death_scale, w.death_sink) = (over * over * death.roll, 1.0 - going, going * death.sink * height);
            }
            if t >= 1.0 && !shown.poofed {
                shown.poofed = true;
                let coat = def.body.colours.first().copied().unwrap_or([200, 200, 200]);
                poofs.push((shown.position + Vec3::Y * height * 0.4, height, coat));
            }
            if t >= 1.0 {
                finished.push(id);
            }
        }
        // The gate's watch: where last frame drew its feet and body, against the frame
        // before, a foot that was down in both counting how far it slid.
        let watched = creatures.watch == Some(id);
        if watched {
            let w = &mut creatures.watched;
            if let Ok(body) = drawn.globals.get(shown.entity) {
                let at = body.translation();
                if let Some(before) = shown.drawn_at {
                    w.went += (at - before).with_y(0.0).length();
                    w.frames += 1;
                }
                shown.drawn_at = Some(at);
            }
            shown.feet.resize(shown.joints.len(), None);
            for (i, (joint, part)) in shown.joints.iter().enumerate() {
                let Ok(global) = drawn.globals.get(*joint) else { continue };
                let foot = global.transform_point(Vec3::new(0.0, -part.pivot.y, 0.0));
                let down = |planted: &[Option<i64>]| planted.get(i).copied().flatten();
                if let Some(before) = shown.feet[i]
                    && let Some(stride) = down(&shown.planted)
                    && down(&shown.planted_before) == Some(stride)
                {
                    w.slid += (foot - before).with_y(0.0).length();
                    w.planted += 1;
                }
                shown.feet[i] = Some(foot);
            }
            w.fastest_turn = w.fastest_turn.max(shown.spin.abs());
            w.yaw = shown.yaw;
            w.flee = shown.flee;
            shown.planted_before = std::mem::replace(&mut shown.planted, vec![None; shown.joints.len()]);
        }
        if let Ok(mut transform) = transforms.get_mut(shown.entity) {
            *transform = root;
        }
        // What the last frame drew of a falling body, joint by joint, for the gate: while it
        // tips over (not the killing blow's freeze before), before it lies there.
        let fallen = shown.dying.is_some_and(|d| d >= def.death.fall);
        if shown.dying.is_some() && !fallen && !frozen {
            let middle = shown.position + Vec3::Y * height * 0.5;
            for (joint, _) in &shown.joints {
                if let Ok(global) = drawn.globals.get(*joint) {
                    let (s, _, t) = global.to_scale_rotation_translation();
                    shown.death_spread = shown.death_spread.max(t.distance(middle) / height.max(0.1));
                    shown.death_scale = shown.death_scale.max((s / scale.max(1e-3) - Vec3::ONE).abs().max_element());
                    shown.death_samples += 1;
                }
            }
        }
        let far = shown.position.distance(eye) > DRAWN_WITHIN;
        // Hearts: trust filling while a wild one is being tamed, all full while a kept one
        // is ready to breed, bobbing over it and turned to face the eye.
        let filled = if shown.view.ready { HEARTS } else { (shown.view.trust as usize * HEARTS).div_ceil(100) };
        let showing = (shown.view.ready || (!shown.view.kept && shown.view.trust > 0)) && shown.dying.is_none() && !far;
        if showing && shown.hearts.is_empty() {
            shown.hearts = (0..HEARTS)
                .map(|_| commands.spawn((Mesh3d(heart_mesh.clone()), MeshMaterial3d(heart_empty.clone()), Transform::default(), NotShadowCaster, DespawnOnExit(Screen::InGame))).id())
                .collect();
        }
        let above = shown.position + Vec3::Y * (height + 0.3 + (now * 3.0 + id as f32).sin() * 0.04);
        let toward = (eye - above).with_y(0.0).normalize_or(Vec3::Z);
        let facing = Quat::from_rotation_y(toward.x.atan2(toward.z));
        let across = facing * Vec3::X;
        for (i, heart) in shown.hearts.iter().enumerate() {
            if let Ok(mut transform) = transforms.get_mut(*heart) {
                let offset = (i as f32 - (HEARTS as f32 - 1.0) / 2.0) * 0.27;
                let size = if showing { Vec3::new(0.25, 0.2, 0.01) } else { Vec3::ZERO };
                *transform = Transform::from_translation(above + across * offset).with_rotation(facing).with_scale(size);
            }
            let colour = if i < filled { &heart_full } else { &heart_empty };
            if let Ok(mut look) = looks.get_mut(*heart)
                && look.0 != *colour
            {
                look.0 = colour.clone();
            }
        }

        // A kept creature wags faster when its keeper is near.
        let keeper_near = shown.view.kept && shown.view.owner.is_some() && shown.view.owner == me && to_eye.length() < 6.0;
        let breath = 1.0 + (now * 2.2 + id as f32 * 0.7).sin() * 0.012 * (1.0 - moving);
        let twitch = |side: f32| {
            // Now and then, one ear flicks back.
            let slot = (now * 0.8 + id as f32 * 0.37 + side * 0.21).floor();
            let chance = creature::unit_from(creature::mix(id as u64 ^ slot as i64 as u64, side.to_bits() as u64));
            if chance < 0.18 { ((now * 0.8 + id as f32 * 0.37 + side * 0.21).fract() * std::f32::consts::TAU).sin().max(0.0) * 0.6 } else { 0.0 }
        };
        // Its skin, once painted.
        if let Some(task) = shown.painting.as_mut()
            && let Some(image) = block_on(future::poll_once(task))
        {
            shown.painting = None;
            shown.skin = images.add(image);
            if let Some(mut material) = skins.get_mut(&shown.material) {
                material.extension.skin = shown.skin.clone();
            }
        }
        // Never drawn around the eye: inside a body only its inside shows, so a body the eye
        // is inside is hidden.
        let around_eye = eye_inside(&registry.get(shown.view.species).parts, eye, shown.position, shown.yaw, scale, height, shown.dying.is_some());
        if around_eye && shown.dying.is_some() {
            shown.death_around_eye = true;
        }
        if let Ok(mut shows) = drawn.visible.get_mut(shown.body) {
            let want = if far || around_eye || shown.painting.is_some() { Visibility::Hidden } else { Visibility::Inherited };
            if *shows != want {
                *shows = want;
            }
        }
        // Each foot travels the same ground in a stride, however long its leg: half of it
        // either way from under the hip.
        // (Drawn stretched long when it flees, its legs' reach is stretched with it.)
        let travel = travel * (1.0 - shown.spread) / stretched;
        let front = parts.iter().filter(|p| matches!(p.swing, Swing::Scuttle { .. })).map(|p| p.pivot.z).fold(f32::INFINITY, f32::min);
        let crawler_lift = stance.paw.abs() * moves.tells.crawler_front;
        let dead = shown.dying.is_some();
        shown.holds.resize(shown.joints.len(), Hold::default());
        for (index, (child, part)) in shown.joints.iter().enumerate() {
            let (rotation, stretch) = match part.swing {
                Swing::Leg { phase, hind } => {
                    let phase = motion::leg_phase(phase, hind, paired);
                    if watched && part.role != creature::Role::Arm {
                        let at = shown.stride + phase;
                        shown.planted[index] = (at.rem_euclid(1.0) < gait.duty && moving > 0.0).then_some(at.floor() as i64);
                    }
                    // A leg's own length sets how far it turns for the same ground; an arm
                    // swings as the legs do.
                    let length = if part.role == creature::Role::Arm { hip } else { part.pivot.y * scale };
                    let at = shown.stride + phase;
                    let (mut angle, lift) = motion::leg(at, &gait, travel / length.max(0.01));
                    // A foot that is down holds the ground it was put on, whatever the body
                    // does meanwhile (speeding up, slowing, a flinch), and swings off from
                    // where it was lifted.
                    if part.role != creature::Role::Arm && !dead && gait.swing > 0.05 && moving > 0.05 {
                        let stride = at.floor() as i64;
                        let hip_at = root.transform_point(part.pivot);
                        let forward = (root.rotation * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
                        let reaches = length * stretched;
                        let hold = &mut shown.holds[index];
                        let into = at.rem_euclid(1.0);
                        if into < gait.duty {
                            let ground = match hold.down {
                                Some((ground, at)) if at == stride => ground,
                                _ => {
                                    let ground = hip_at + forward * reaches * angle.sin();
                                    hold.down = Some((ground, stride));
                                    ground
                                }
                            };
                            // (Less the body's own tip nose to tail, which turns the legs with it.)
                            angle = ((ground - hip_at).dot(forward) / reaches.max(0.01)).clamp(-0.99, 0.99).asin() - (rear * REAR + pitch);
                            hold.lifted = Some((angle, stride));
                        } else if let Some((from, _)) = hold.lifted.filter(|&(_, at)| at == stride) {
                            let w = ((into - gait.duty) / (1.0 - gait.duty).max(1e-3) * 3.0).min(1.0);
                            angle = from + (angle - from) * w * w * (3.0 - 2.0 * w);
                        }
                    }
                    // Front limbs reach with a strike (claws swipe, a spell's caster raises them).
                    let paw = if hind { 0.0 } else { stance.paw };
                    let lifted = if part.role == creature::Role::Arm || dead { 0.0 } else { lift * moving };
                    (Quat::from_rotation_x(angle + paw), Vec3::new(1.0, 1.0 - lifted, 1.0))
                }
                Swing::Scuttle { phase } => {
                    // A crawler's legs sweep fore and aft about the hip, its front pair
                    // raised as it winds up and strikes.
                    let (angle, lift) = motion::leg(shown.stride + phase, &gait, travel / (part.pivot.y * scale * 1.2).max(0.01));
                    let side = (part.centre.x - part.pivot.x).signum();
                    let raised = if part.pivot.z <= front + 1e-3 { crawler_lift } else { 0.0 };
                    (Quat::from_rotation_y(angle * side) * Quat::from_rotation_z(side * (lift * 0.6 + raised)), Vec3::ONE)
                }
                Swing::Head => (Quat::from_rotation_y(shown.look) * Quat::from_rotation_x(-(shown.nod + stance.nod)), Vec3::ONE),
                Swing::Ear { side } => (Quat::from_rotation_x(twitch(side) + moves.flee.ears * shown.flee) * Quat::from_rotation_y(shown.look), Vec3::ONE),
                Swing::Tail => {
                    let (rate, sway) = if keeper_near { (11.0, 0.45) } else { (2.5, 0.3) };
                    // Now and then a flick, standing about; up and stiff running away.
                    let flick = motion::now_and_then(&moves.idle.flick, now, shown.view.seed, 3).filter(|_| idle).map_or(0.0, |t| (t * std::f32::consts::TAU).sin());
                    let sway = sway * (1.0 - shown.flee * 0.7);
                    let turn = (now * rate + id as f32).sin() * sway + flick * moves.idle.flick.size - shown.spin * 0.08;
                    (Quat::from_rotation_y(turn) * Quat::from_rotation_x(moves.flee.tail * shown.flee), Vec3::ONE)
                }
                Swing::Wing { side } if flier => {
                    // Spread out to the sides and lengthened in flight, beating in bursts.
                    let (_, beating) = flight_bound(now, id);
                    let beat = (now * WINGBEATS).sin() * 0.75 * beating;
                    let angle = shown.spread * (WING_SPREAD + beat);
                    (Quat::from_rotation_z(side * angle), Vec3::new(1.0, 1.0 + WING_REACH * shown.spread, 1.0))
                }
                Swing::Wing { side } => {
                    let beat = if running || speed > def.movement.walk * 1.2 { (now * 18.0).sin() * 0.6 } else { 0.0 };
                    (Quat::from_rotation_z(side * beat), Vec3::ONE)
                }
                Swing::Orbit { phase } => (Quat::from_rotation_y((now * 1.3 + phase) * std::f32::consts::TAU), Vec3::ONE),
                Swing::Still if part.role == creature::Role::Torso => (Quat::IDENTITY, Vec3::new(1.0, breath, 1.0)),
                Swing::Still => (Quat::IDENTITY, Vec3::ONE),
            };
            // The joint turns its part about the pivot (its rest is folded into the mesh).
            if let Ok(mut transform) = transforms.get_mut(*child) {
                *transform = Transform { translation: part.pivot, rotation, scale: stretch };
            }
        }
        // Lit exactly as the terrain around it is (`SceneLight`: at night as dark as the
        // ground, which a separate daylight curve left far brighter), a glowing creature by
        // itself; red when struck.
        let cell = (shown.position + Vec3::Y * 0.5).floor().as_ivec3();
        let light = if def.body.glows {
            LinearRgba::WHITE
        } else {
            scene_light.at(sky_light(&world, cell), block_light(&world, cell))
        };
        // In steps too fine to see, and written only when it changes: a changed material
        // is uploaded again and every part drawn with it specialised again for the camera
        // and each shadow cascade: writing it every frame took a herd of 40 from 4 to 8 ms
        // a frame on an NVIDIA L4.
        let step = |c: f32| (c * LIGHT_STEPS).round() / LIGHT_STEPS;
        let red = shown.flash / red_seconds;
        let lit = LinearRgba::rgb(step(light.red), step(light.green), step(light.blue));
        let mut colour = lit.mix(&LinearRgba::rgb(0.9, 0.08, 0.05), red.min(1.0) * 0.7);
        // The moment of a blow: a white flash, through the freeze and a moment after.
        if shown.white > 0.0 {
            colour = colour.mix(&LinearRgba::rgb(2.2, 2.2, 2.2), 0.85);
        }
        shown.white = (shown.white - dt).max(0.0);
        let colour = Color::LinearRgba(colour);
        if skins.get(&shown.material).is_some_and(|material| material.base.base_color != colour)
            && let Some(mut material) = skins.get_mut(&shown.material)
        {
            material.base.base_color = colour;
        }
    }
    creatures.poofs.extend(poofs);
    creatures.strikes += strikes;
    creatures.reads.extend(reads);
    creatures.recoveries.extend(recoveries);
    for id in finished {
        if let Some(shown) = creatures.shown.remove(&id) {
            let tip = registry.get(shown.view.species).def.death.fall;
            creatures.falls.push(Fall { strayed: shown.death_strayed, around_eye: shown.death_around_eye, spread: shown.death_spread, scale: shown.death_scale, samples: shown.death_samples, joints: shown.joints.len() as u32, slowest: shown.death_slowest, tip });
            despawn(&mut commands, &shown);
            if shown.painting.is_none() {
                images.remove(&shown.skin);
            }
            skins.remove(&shown.material);
        }
    }
}

/// Finds the creature under the crosshair, nearer than any block, and strikes it on a
/// left click. Runs between finding the block target and breaking, so aiming at a
/// creature never mines the ground behind it.
#[allow(clippy::too_many_arguments)]
pub fn aim(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    ui: Res<UiState>,
    launch: Res<Launch>,
    net: Res<Net>,
    actions: Res<Actions>,
    player: Res<LocalPlayer>,
    creatures: Res<Creatures>,
    mut target: ResMut<Target>,
    mut fighting: ResMut<crate::fight::Fighting>,
    mut sounds: ResMut<CreatureSounds>,
    mut tending: ResMut<Tending>,
) {
    // What the right button did this frame, if anything, is said afresh: here, then in
    // `interact::break_and_place`; `fight::guard` raises no guard if it did something.
    target.used_right = false;
    // An animal tended answers once the server has done it (its product in the pack, the
    // food gone from hand), never on the click alone.
    let now = time.elapsed_secs();
    let Tending { waiting, harmless } = &mut *tending;
    let meanwhile = |t: &Tended| t.something_else_since(net.proposals(), *harmless);
    for answered in answer_tended(waiting, now, meanwhile, |item| player.inventory.count(item)) {
        if let Some(shown) = creatures.shown.get(&answered.id) {
            sounds.push(CreatureSound { answering: true, ..sound(answered.answer, answered.id, &shown.view, shown.position, 0.0) });
        }
    }
    let eye = player.eye();
    let look = player.look_direction().normalize_or_zero();
    let block = target.hit.map_or(f32::MAX, |hit| hit.distance);
    // What a blow would be struck with: its reach and rhythm, and whether it guards.
    let held = player.inventory.get(player.selected).map(|s| s.item);
    let combat = &launch.content.combat;
    let weapon = combat.wielded(held, |skill| launch.content.skills.level(player.skills.xp(skill)));
    // Aimed at as far as it can be touched (tended, looked at) or struck, whichever is
    // further; struck only within the weapon's reach.
    let nearest = creatures
        .shown
        .keys()
        .filter_map(|&id| Some((id, ray_box(eye, look, &creatures.aabb(&launch, id)?)?)))
        .filter(|&(_, d)| d <= weapon.reach.max(combat.feel.touch) && d < block)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    target.creature = nearest.map(|(id, _)| id);
    let in_reach = nearest.is_some_and(|(_, d)| d <= weapon.reach);
    if target.creature.is_some() {
        target.hit = None;
    }
    // A right click tends it with what is in hand: food that tames it or readies a kept one
    // to breed, or a tool for its product (the server judges; this only asks when it may).
    // At a creature fighting the player (it struck or was struck lately, or is winding up)
    // with a weapon or shield that guards, the button guards (`fight`); otherwise it looks
    // closely at it. With Shift held, on one of your own, it asks to let it go wild (twice
    // to do it).
    if let Some(id) = target.creature
        && ui.cursor_locked
        && !player.dead
        && mouse.just_pressed(MouseButton::Right)
        && let Some(view) = creatures.shown.get(&id).map(|s| s.view)
    {
        let species = launch.content.creatures.get(view.species);
        let mine = view.kept && view.owner.is_some() && view.owner == creatures.me;
        let accepts = held.is_some_and(|item| {
            let tags = &launch.content.blocks.def(item).tags;
            let tool = product_taken_by(species, item, &launch.content.blocks).is_some();
            // One of yours takes its food as a meal or to breed (the server judges which).
            let food = species.tame_food.contains(&item) || species.breed_food.contains(&item) || tags.iter().any(|t| t == "breeding_feed");
            (!view.kept && species.tame_food.contains(&item)) || (mine && (food || tool))
        });
        let guards = combat.guard_of(held, &player.inventory.worn, |skill| launch.content.skills.level(player.skills.xp(skill))).is_some();
        let engaged = !view.kept && (view.pose == Pose::Attacking || fighting.engaged(id, time.elapsed_secs_f64(), &combat.feel));
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let action = if shift && mine {
            Some("base:release")
        } else if accepts {
            Some("base:tend")
        } else if !(guards && engaged) {
            Some("base:inspect")
        } else {
            None
        };
        if let Some(action) = action.and_then(|a| actions.0.id(a)) {
            // Tending is heard by what is in hand: shears, a pail, food (`sounds.ron`'s ways
            // and what it brings).
            net.propose_from_with(&player, action, ActionTarget::Creature(id), held.into_iter().collect());
            target.used_right = true;
            // Tending, looking closely or letting go change nothing in the pack but what a
            // tend gives or takes.
            tending.harmless += 1;
            // The animal answers the hand once the server does it: it munches what it is
            // fed, and bleats or lows as it is shorn or milked.
            if Some(action) == actions.0.id("base:tend")
                && let Some(item) = held
            {
                let (proposals, harmless) = (net.proposals(), tending.harmless);
                tending.waiting.push(Tended { proposals, harmless, ..Tended::new(id, species, item, &launch.content.blocks, |item| player.inventory.count(item), now) });
            }
        }
    }
    // A left click swings: at once, or held for the beat (the server keeps the same
    // rhythm and says what lands).
    if let Some(id) = target.creature
        && in_reach
        && ui.cursor_locked
        && !player.dead
        && mouse.just_pressed(MouseButton::Left)
        && let Some(attack) = actions.0.id("core:attack")
        && fighting.click(time.elapsed_secs_f64(), &combat.timing(&weapon), held, &combat.feel)
    {
        net.propose_from(&player, attack, ActionTarget::Creature(id));
        fighting.strikes += 1;
    }
    // A left click at one out of reach swings at the air (`Fighting::whiff`): the hand and
    // a whoosh say the click was heard, and nothing is struck.
    if target.creature.is_some() && !in_reach && ui.cursor_locked && !player.dead && mouse.just_pressed(MouseButton::Left) {
        fighting.whiff(time.elapsed_secs_f64(), &combat.timing(&weapon));
    }
}

/// A wind-up begun: the coil starts (its end is the blow), and it is told by sound and,
/// for a boss's far reach, a ring on the ground (`fight_fx`).
fn wind_up(shown: &mut Shown, view: &voxmar_protocol::CreatureView, registry: &creature::CreatureRegistry, creature_windup: f32, late: bool, told: &mut Vec<Told>) {
    (shown.coiling, shown.striking, shown.coil_struck) = (Some(0.0), None, false);
    (shown.coil_from, shown.coil_far, shown.coil_kept, shown.coil_slid, shown.coil_deepest, shown.recover_most) = (shown.position, 0.0, 1.0, 0.0, 0.0, 0.0);
    shown.move_shown = if matches!(view.tell, voxmar_protocol::REARING | voxmar_protocol::STAGGERED) { 0 } else { view.tell };
    let species = registry.get(view.species);
    let name = species.def.name.rsplit(':').next().unwrap_or(&species.def.name);
    let (key, attack) = match shown.move_shown {
        0 => (name.to_owned(), species.def.attack.as_ref()),
        index => match species.def.moves.get(index as usize - 1) {
            Some(m) => (format!("{name}.{}", m.name), Some(&m.attack)),
            None => (name.to_owned(), species.def.attack.as_ref()),
        },
    };
    if let Some(attack) = attack {
        let windup = attack.windup.unwrap_or(creature_windup);
        told.push(Told { id: view.id, key, windup, reach: species.reach * view.scale + attack.reach, style: attack.style, late });
    }
}

/// Where a ray from `origin` along unit `direction` first enters `aabb`, if it does.
pub(crate) fn ray_box(origin: Vec3, direction: Vec3, aabb: &Aabb) -> Option<f32> {
    let inverse = direction.recip();
    let t1 = (aabb.min - origin) * inverse;
    let t2 = (aabb.max - origin) * inverse;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    (far >= near.max(0.0)).then_some(near.max(0.0))
}

/// Whether `view` shows a creature in a fight, after the view `before` it: attacking,
/// winding up a move (its tell), or hurt since.
fn shows_fight(before: Option<&CreatureView>, view: &CreatureView) -> bool {
    view.pose == Pose::Attacking || view.tell != 0 || before.is_some_and(|before| view.health < before.health)
}

/// Whether a creature shown as `view`, last in a fight at `fought_at`, is in one at `now`:
/// attacking now, or within `calm_after` seconds of its last fight.
fn in_fight(view: &CreatureView, fought_at: Option<f32>, now: f32, calm_after: f32) -> bool {
    view.pose == Pose::Attacking || fought_at.is_some_and(|at| now - at < calm_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ewe shorn and a cow milked call out once their wool or milk is in the pack; fed,
    /// they munch once the food has left the hand. Refused, nothing changes: silent.
    #[test]
    fn a_tended_animal_answers_what_the_server_did() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let (blocks, creatures) = (&content.blocks, &content.creatures);
        let species = |name: &str| creatures.get(creatures.id(name).unwrap_or_else(|| panic!("{name}")));
        let item = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("{name}"));
        let tend = |who: &str, held: &str| Tended::new(7, species(who), item(held), blocks, |_| 4, 10.0);
        let shorn = tend("base:mouflon", "base:shears");
        assert_eq!((shorn.watched, shorn.answer, shorn.had), (item("base:wool"), Sound::Idle, 4));
        assert!(shorn.done(5) && !shorn.done(4), "the wool came, or not yet");
        let milked = tend("base:aurochs", "base:pail");
        assert_eq!((milked.watched, milked.answer), (item("base:milk"), Sound::Idle));
        let fed = tend("base:mouflon", "base:wheat");
        assert_eq!((fed.watched, fed.answer), (item("base:wheat"), Sound::Graze));
        assert!(fed.done(3) && !fed.done(4), "eaten, or refused and still in hand");
        assert_eq!(tend("base:aurochs", "base:shears").answer, Sound::Graze, "an aurochs gives no wool");
        assert_eq!(product_taken_by(species("base:mouflon"), item("base:pail"), blocks), None);
    }

    /// Rejoining (a new connection) forgets what was waiting from the last session, whose
    /// counts no longer compare; within a session what waits stays.
    #[test]
    fn a_rejoin_forgets_the_animals_waiting() {
        let content = voxmar_server::load_content(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")).unwrap();
        let net = || {
            let (connection, _server_side) = voxmar_protocol::local_pair();
            Net::new(connection, None, "tester".into(), content.fingerprint(), None)
        };
        let stale = Tended { id: 1, watched: voxmar_engine::block::BlockId(5), had: 0, answer: Sound::Idle, until: 9.0, proposals: 40, harmless: 12 };
        let mut app = App::new();
        app.insert_resource(Tending { waiting: vec![stale], harmless: 12 }).insert_resource(net()).add_systems(Update, forget_tending_on_rejoin);
        app.update();
        assert!(app.world().resource::<Tending>().waiting.is_empty() && app.world().resource::<Tending>().harmless == 0, "the first session starts clean");
        app.world_mut().resource_mut::<Tending>().waiting.push(stale);
        app.update();
        assert_eq!(app.world().resource::<Tending>().waiting, [stale], "within a session it waits on");
        app.world_mut().remove_resource::<Net>();
        app.update();
        assert_eq!(app.world().resource::<Tending>().waiting, [stale], "left, as the session's end does");
        app.insert_resource(net());
        app.update();
        assert!(app.world().resource::<Tending>().waiting.is_empty(), "rejoined");
    }

    /// Two ewes shorn in turn answer in turn, each as its own wool comes; one waiting past
    /// its time never does.
    #[test]
    fn a_flock_tended_answers_in_turn() {
        let wool = voxmar_engine::block::BlockId(5);
        let ewe = |id: u32, had: u32| Tended { id, watched: wool, had, answer: Sound::Idle, until: 3.0, proposals: 0, harmless: 0 };
        let answer_tended = |waiting: &mut Vec<Tended>, now: f32, count: fn(voxmar_engine::block::BlockId) -> u32| answer_tended(waiting, now, |_| false, count);
        let mut waiting = vec![ewe(1, 0), ewe(2, 0)];
        assert_eq!(answer_tended(&mut waiting, 0.5, |_| 0), []);
        let first = answer_tended(&mut waiting, 0.6, |_| 2);
        assert_eq!(first.iter().map(|t| t.id).collect::<Vec<_>>(), [1], "the first ewe's wool");
        assert_eq!(waiting, [ewe(2, 2)], "the second waits from the new count");
        assert_eq!(answer_tended(&mut waiting, 0.7, |_| 2), []);
        assert_eq!(answer_tended(&mut waiting, 0.8, |_| 3).iter().map(|t| t.id).collect::<Vec<_>>(), [2]);
        let mut late = vec![ewe(3, 0)];
        assert_eq!(answer_tended(&mut late, 3.5, |_| 1), [], "past its time");
        assert!(late.is_empty());
        // Fed, then seeds of the same sown before the server's word: the food's count may
        // fall by the sowing, so it answers nothing.
        let fed = Tended { id: 4, watched: wool, had: 5, answer: Sound::Graze, until: 3.0, proposals: 1, harmless: 1 };
        let mut sown = vec![fed];
        assert_eq!(super::answer_tended(&mut sown, 0.5, |t| t.proposals < 2, |_| 4), [], "something else done meanwhile");
        assert!(sown.is_empty());
        // Two ewes fed in one of the server's ticks: the food falls by two, both munch.
        let ewe_fed = |id: u32| Tended { id, answer: Sound::Graze, ..ewe(id, 6) };
        let mut flock = vec![ewe_fed(5), ewe_fed(6), ewe_fed(7)];
        assert_eq!(answer_tended(&mut flock, 0.5, |_| 4).iter().map(|t| t.id).collect::<Vec<_>>(), [5, 6]);
        assert_eq!(flock, [Tended { had: 4, ..ewe_fed(7) }], "the third waits from the new count");
        // Counted since: two more harmless (tends, a look) is nothing else; a sowing among them is. Counts from
        // before a rejoin (larger than now) never underflow.
        assert!(!fed.something_else_since(3, 3) && fed.something_else_since(4, 3));
        assert!(!fed.something_else_since(0, 0), "a new session's counts");
    }

    /// A roaming boss nicked once is in a fight, and out of it once it has gone back to
    /// grazing for `calm_after`, though it never mends: its fight music ends.
    #[test]
    fn a_nicked_boss_that_calms_down_is_no_longer_in_a_fight() {
        let calm_after = 12.0;
        let (grazing, full) = (Pose::Grazing, 60);
        let at = |pose: Pose, tell: u8, health: u16| CreatureView { health, ..view_of(pose, tell) };
        let mut fought = None;
        let mut before = at(grazing, 0, full);
        let mut step = |view: CreatureView, now: f32, fought: &mut Option<f32>| {
            if shows_fight(Some(&before), &view) {
                *fought = Some(now);
            }
            before = view;
            in_fight(&before, *fought, now, calm_after)
        };
        assert!(!step(at(grazing, 0, full), 0.0, &mut fought), "grazing unhurt: no fight");
        assert!(step(at(Pose::Alert, 0, full - 1), 1.0, &mut fought), "nicked: a fight");
        assert!(step(at(grazing, 0, full - 1), 1.0 + calm_after - 0.5, &mut fought), "still in it a while after");
        assert!(!step(at(grazing, 0, full - 1), 1.0 + calm_after + 0.5, &mut fought), "calmed and grazing, still hurt: the fight is over");
        assert!(step(at(Pose::Idle, 3, full - 1), 20.0, &mut fought), "a move wound up: a fight again");
        assert!(in_fight(&at(Pose::Attacking, 0, full), None, 100.0, calm_after), "attacking is always a fight");
    }

    fn bear() -> Vec<Part> {
        creature::Model::Quadruped {
            body: (1.0, 0.92, 1.5),
            head: (0.54, 0.46, 0.46),
            legs: 0.55,
            ears: None,
            tail: None,
            horns: None,
            horn_shape: creature::HornShape::Curved,
            snout: None,
            neck: None,
            haunches: false,
            hump: None,
            shell: None,
            tail_droop: 0.0,
            ear_lean: 0.0,
        }
        .parts(1.0)
    }

    fn view_of(pose: Pose, tell: u8) -> CreatureView {
        CreatureView {
            id: 1,
            species: CreatureId(0),
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pose,
            tell,
            health: 10,
            scale: 1.0,
            seed: 1,
            kept: false,
            owner: None,
            young: false,
            ready: false,
            trust: 0,
            genes: [0; voxmar_engine::genes::MOST],
        }
    }

    /// A daze cuts a wind-up short mid-coil: it reels with no blow shown. A perfect block
    /// staggers it once its blow has come, shown by the wind-up's own time: it reels after
    /// the blow, and the stagger says the blow showed.
    #[test]
    fn a_daze_shows_no_blow_and_a_parried_blow_still_shows() {
        let winding = view_of(Pose::Attacking, 0);
        let staggered = view_of(Pose::Idle, voxmar_protocol::STAGGERED);
        assert_eq!(turn(&winding, &staggered, true, false, false, false), Turn::Staggered { struck: false });
        assert_eq!(turn(&winding, &staggered, true, true, true, false), Turn::Staggered { struck: true });
        // Its blow at the server's word, and its wind-up's start.
        let idle = view_of(Pose::Idle, 0);
        assert_eq!(turn(&winding, &idle, true, false, false, false), Turn::Strikes);
        assert_eq!(turn(&winding, &idle, true, true, true, false), Turn::Ends);
        assert_eq!(turn(&idle, &winding, false, false, false, false), Turn::WindsUp);
        assert_eq!(turn(&staggered, &staggered, false, false, false, false), Turn::Same);
    }

    /// A 0.6 s wind-up the player's blows land on twice, each freezing it 0.09 s (a heavy
    /// blow's hit-stop), frame by frame at 60 frames a second and at 2, 3 and 4 times as long
    /// a frame: how far into it the blow struck, its own time or the server's word (which
    /// comes the wind-up after it was seen begin) first. `frozen_clock` as the coil once ran,
    /// standing still through a freeze and counting nothing of the frame the word came on.
    fn wind_up_reached(dt: f32, frozen_clock: bool) -> f32 {
        let (windup, freeze, blows) = (0.6, 0.09, [0.1, 0.4]);
        let (mut coil, mut stopped, mut struck) = (0.0f32, 0.0f32, 0);
        for frame in 0.. {
            let t = frame as f32 * dt;
            if t >= windup {
                return if frozen_clock { coil } else { word_reached(coil, dt) };
            }
            if struck < blows.len() && t >= blows[struck] {
                (stopped, struck) = (stopped.max(freeze), struck + 1);
            }
            if frozen_clock {
                let moved = frame_clocks(&mut stopped, None, dt);
                coil += moved;
            } else {
                frame_clocks(&mut stopped, Some(&mut coil), dt);
            }
            if coil >= windup {
                return coil;
            }
        }
        unreachable!()
    }

    #[test]
    fn a_wind_up_coils_whole_by_the_game_clock_at_any_frame_rate() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content/core/combat_fx.ron")).unwrap();
        let file = voxmar_engine::content::ContentFile { package: "core".into(), name: crate::fight_fx::style::FILE.into(), source };
        let strike = crate::fight_fx::FightLook::load(&[file]).unwrap().strike(voxmar_engine::combat::Style::Crush);
        let share = |reached: f32| crate::fight_fx::coil_share(&strike, (reached / 0.6).min(1.0));
        for slower in [1.0, 2.0, 3.0, 4.0] {
            let dt = slower / 60.0;
            let coiled = share(wind_up_reached(dt, false));
            assert!(coiled >= 0.99, "{slower}x frames: coiled {coiled:.2} deep as the blow struck");
        }
        // As it was: deep enough at 60 frames a second, and barely coiled at 15.
        assert!(share(wind_up_reached(1.0 / 60.0, true)) >= 0.6);
        assert!(share(wind_up_reached(4.0 / 60.0, true)) < 0.6);
    }

    #[test]
    fn bodies_hide_only_with_the_eye_inside() {
        let parts = bear();
        // Beside the flank, 0.2 out: seen.
        assert!(!eye_inside(&parts, Vec3::new(0.7, 1.0, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, false));
        // Inside the torso: hidden.
        assert!(eye_inside(&parts, Vec3::new(0.2, 1.0, 0.1), Vec3::ZERO, 0.0, 1.0, 1.4, false));
        // Turned a quarter, the flank faces along z: still seen from 0.2 out.
        assert!(!eye_inside(&parts, Vec3::new(0.0, 1.0, 0.7), Vec3::ZERO, std::f32::consts::FRAC_PI_2, 1.0, 1.4, false));
        // A dying body hides within its height, where it may lie.
        assert!(eye_inside(&parts, Vec3::new(1.2, 0.5, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, true));
        assert!(!eye_inside(&parts, Vec3::new(2.0, 0.5, 0.0), Vec3::ZERO, 0.0, 1.0, 1.4, true));
    }

    #[test]
    fn a_creature_gone_is_not_brought_back_by_a_view_told_before_it() {
        let view = |id| CreatureView {
            id,
            species: creature::CreatureId(0),
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pose: creature::brain::Pose::Idle,
            tell: 0,
            health: 4,
            scale: 1.0,
            seed: 0,
            kept: false,
            owner: None,
            young: false,
            ready: false,
            trust: 0,
            genes: [0; voxmar_engine::genes::MOST],
        };
        let mut creatures = Creatures::default();
        // Moved, then dispersed, both reaching the client in one frame.
        creatures.receive(vec![view(1), view(2)], vec![]);
        creatures.receive(vec![], vec![CreatureGone { id: 1, died: false }]);
        assert_eq!(creatures.inbox.iter().map(|v| v.id).collect::<Vec<_>>(), [2]);
        // Gone, then back in sight: the later view is shown.
        creatures.receive(vec![], vec![CreatureGone { id: 2, died: false }]);
        creatures.receive(vec![view(2)], vec![]);
        assert_eq!(creatures.inbox.iter().map(|v| v.id).collect::<Vec<_>>(), [2]);
        // A bark from one told of but not yet built waits for it; one from a stranger goes.
        creatures.bark(2, voxmar_engine::creature::Bark::Greet);
        creatures.bark(9, voxmar_engine::creature::Bark::Alarm);
        assert_eq!(creatures.barks_heard(), []);
        assert_eq!(creatures.barks, [(2, voxmar_engine::creature::Bark::Greet)]);
        creatures.inbox.clear();
        assert_eq!(creatures.barks_heard(), []);
        assert!(creatures.barks.is_empty(), "a bark kept for one never coming");
    }

    #[test]
    fn rays_find_boxes_in_front_only() {
        let aabb = Aabb { min: Vec3::new(2.0, -0.5, -0.5), max: Vec3::new(3.0, 0.5, 0.5) };
        assert_eq!(ray_box(Vec3::ZERO, Vec3::X, &aabb), Some(2.0));
        assert_eq!(ray_box(Vec3::ZERO, Vec3::NEG_X, &aabb), None);
        assert_eq!(ray_box(Vec3::ZERO, Vec3::Y, &aabb), None);
    }
}
