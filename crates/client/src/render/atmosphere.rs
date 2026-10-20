//! The atmosphere uniform: everything the terrain, sky and volumetric light shaders need
//! about the air and the light besides Bevy's sun and ambient light. Shadow filtering and
//! how surfaces take light (sky light falloff, occlusion, leaves glowing against the sun),
//! clear-air haze, ground fog (valley fog, dawn mist, sea fog), the weather's fog, rain and
//! dust, and the volumetric light's reach and strength.
//!
//! It is computed once a frame on the main thread from the time of day, the weather, the
//! air of the place, the shader pack and the quality settings ([`Atmosphere::new`], pure,
//! so it is tested without a GPU), extracted, and written to one small uniform buffer that
//! every terrain bind group, the sky material and the volumetric pass bind. See
//! atmosphere.wgsl for how the shaders use it.

use std::sync::{Arc, OnceLock};

use bevy::color::ColorToComponents;
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages, ShaderType, encase};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::load_shader_library;
use voxmar_protocol::Weather;

use super::cloud_map;
use super::pack::ShaderPack;
use super::Quality;

pub struct AtmospherePlugin;

impl Plugin for AtmospherePlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "clouds.wgsl");
        load_shader_library!(app, "atmosphere.wgsl");
        load_shader_library!(app, "shadows.wgsl");
        let shared = SharedAtmosphere::default();
        app.init_resource::<Atmosphere>().insert_resource(shared.clone()).add_plugins(ExtractResourcePlugin::<Atmosphere>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .insert_resource(shared)
            .add_systems(RenderStartup, create_buffer)
            .add_systems(Render, write_buffer.in_set(RenderSystems::PrepareResources));
    }
}

/// Clear-air haze, per block at its base, before the shader pack scales it.
const HAZE_DENSITY: f32 = 0.0002;
/// Clear air is densest at sea level (`voxmar_worldgen::SEA_LEVEL`) and thins by e every
/// this many blocks above it.
const HAZE_BASE: f32 = 128.0;
const HAZE_HEIGHT: f32 = 90.0;
/// Distant terrain fades completely into the sky over its last 40%.
const EDGE_FADE: f32 = 0.6;
/// Ground fog at full density takes this much light out per block (about 65 blocks of
/// visibility inside it), and thins by e every `GROUND_FOG_THINNING` blocks above its top,
/// so it has no hard top and reaches thinly up the slopes. The volumetric pass draws it as
/// far as `GROUND_FOG_REACH`, uneven and drifting; beyond, it is smooth.
const GROUND_FOG: f32 = 0.06;
const GROUND_FOG_THINNING: f32 = 9.0;
const GROUND_FOG_REACH: f32 = 160.0;
/// The weather's fog is thick up to this far below the eye, whose weather it is, and thins
/// by e every `FOG_HEIGHT` blocks above that.
const FOG_BELOW_EYE: f32 = 16.0;
const FOG_HEIGHT: f32 = 120.0;
/// How far one sees in clear air, in blocks (as `Weather::visibility`).
const CLEAR_VISIBILITY: f32 = 3030.0;
/// Volumetric light: how far its rays reach, in blocks, how many steps they take, how
/// much light scatters evenly in every direction besides the glow around the sun, and how
/// much more clear air scatters sunlight into shafts than it dims the view.
const RAY_REACH: f32 = 96.0;
const RAY_STEPS: f32 = 12.0;
const RAY_ISOTROPIC: f32 = 0.015;
const RAY_CLEAR_AIR: f32 = 10.0;
/// The low cloud: its base over sea level in fair and in wet weather, its depth as flat
/// layers and as heaped cloud, steps through it in the volumetric pass, and how far it is
/// drawn. High peaks stand in it; in rain it comes down onto the hills. Its clouds float
/// above and below these, by up to 0.6 of its depth (`cloud_lift` in atmosphere.wgsl).
const CLOUD_BASE: [f32; 2] = [135.0, 85.0];
const CLOUD_DEPTH: [f32; 2] = [32.0, 150.0];
const CLOUD_STEPS: f32 = 16.0;
const CLOUD_FAR: f32 = 6000.0;
/// The widest shadow penumbra, in blocks, and how far shadow lookups step off a surface,
/// in shadow texels.
const MAX_PENUMBRA: f32 = 0.6;
const NORMAL_OFFSET: f32 = 1.0;
/// How much brighter than its lit texture a block giving full light glows (enough to bloom).
const GLOW: f32 = 3.0;

/// See atmosphere.wgsl for each field.
#[derive(Resource, ShaderType, ExtractResource, Clone, Copy, Debug, PartialEq)]
pub struct Atmosphere {
    pub shadow: Vec4,
    pub surface: Vec4,
    pub haze: Vec4,
    pub haze_shape: Vec4,
    pub glow: Vec4,
    pub fog: Vec4,
    pub fog_shape: Vec4,
    pub mist: Vec4,
    pub rays: Vec4,
    pub clouds: Vec4,
    pub cloud_shape: Vec4,
    pub cloud_look: Vec4,
    pub cloud_map: Vec4,
    pub cloud_shade: Vec4,
    pub ground: Vec4,
    pub water: Vec4,
    pub water_light: Vec4,
    pub water_scales: Vec4,
    pub dusk: Vec4,
    pub cloud_flash: Vec4,
    pub bow: Vec4,
    /// A light held in the hand: where, and its level over 15 (`HeldLight`).
    pub held: Vec4,
}

impl Default for Atmosphere {
    fn default() -> Self {
        Self::new(&Light::default(), &Weather::NONE, &Air::default(), &ShaderPack::default(), Quality::default(), View::new(4096.0))
    }
}

/// Where the eye is and how far it sees.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// The edge of the distant terrain, in blocks.
    pub distance: f32,
    pub eye_height: f32,
    /// The water the eye is in, if it is under water.
    pub underwater: Option<Submerged>,
    /// How much of the mist shows, 0..1: the ground fog and the weather's fog belong to the
    /// open air, so a camera dug down, in a cave or in a room has none (`ShaderPack::mist_share`).
    pub mist_share: f32,
}

impl View {
    pub fn new(distance: f32) -> Self {
        Self { distance, eye_height: 130.0, underwater: None, mist_share: 1.0 }
    }
}

/// The water around an eye under water.
#[derive(Clone, Copy, Debug)]
pub struct Submerged {
    /// Its colour as the biome tints it, linear RGB.
    pub tint: Vec3,
    /// The height of its surface.
    pub surface: f32,
    /// Sun and sky light falling on the surface.
    pub light: Vec3,
}

/// The light of the moment, from the day cycle (see `Environment` in render/mod.rs).
#[derive(Clone, Copy, Debug)]
pub struct Light {
    /// Colour of the horizon, which clear-air haze takes.
    pub horizon: Vec3,
    /// Sky light reaching surfaces in shade.
    pub ambient: Vec3,
    /// Direct sun or moon.
    pub sun: Vec3,
    /// Colour of the glow around the sun, as the sky draws it.
    pub glow: Vec3,
    /// Height of the sun or moon in the sky, the sine of its elevation.
    pub elevation: f32,
    /// Dusk and dawn: xy the way to the sun across the land (a unit vector along x and z),
    /// z how strongly the side of the sky towards it is warmer than the side away (0..1).
    pub dusk: Vec3,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            horizon: Vec3::new(0.62, 0.78, 0.98),
            ambient: Vec3::new(0.4, 0.45, 0.55),
            sun: Vec3::splat(0.7),
            glow: Vec3::new(1.0, 0.8, 0.55),
            elevation: 0.7,
            dusk: Vec3::ZERO,
        }
    }
}

/// The air of the place: haze colour and density as the land and climate have them (the
/// colour parts of `weather::atmosphere::ClientAtmosphere`), applied within the pack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    /// Haze colour, linear RGB, or `None` for the horizon's own.
    pub haze: Option<Vec3>,
    /// Haze density relative to the pack's, around 1.
    pub density: f32,
}

impl Default for Air {
    fn default() -> Self {
        Self { haze: None, density: 1.0 }
    }
}

impl Atmosphere {
    /// The air and light for this moment, seen from `view`.
    pub fn new(light: &Light, weather: &Weather, air: &Air, pack: &ShaderPack, quality: Quality, view: View) -> Self {
        let View { distance: view_distance, eye_height, underwater, mist_share } = view;
        let shadow = Vec4::new(quality.shadows.taps() as f32, pack.shadow_softness, NORMAL_OFFSET, MAX_PENUMBRA);
        let surface = Vec4::new(pack.sky_falloff, pack.ambient_occlusion, pack.subsurface, pack.glow * GLOW);
        if let Some(water) = underwater {
            // No air: the water between the eye and everything takes its place.
            return Self {
                shadow,
                surface,
                haze: light.horizon.extend(0.0),
                haze_shape: Vec4::new(HAZE_BASE, HAZE_HEIGHT, view_distance * EDGE_FADE, view_distance),
                glow: Vec4::ZERO,
                fog: light.horizon.extend(0.0),
                fog_shape: Vec4::new(0.0, FOG_HEIGHT, 0.0, 0.0),
                mist: Vec4::new(0.0, 0.0, 1.0, 0.0),
                rays: Vec4::ZERO,
                clouds: Vec4::ZERO,
                cloud_shape: Vec4::new(1.0, 2.0, 0.0, 0.0),
                cloud_look: Vec4::ZERO,
                cloud_map: Vec4::ZERO,
                cloud_shade: Vec4::ZERO,
                ground: Vec4::new(0.0, 0.0, 0.0, pack.bounce),
                water: water.tint.extend(water.surface),
                water_light: water.light.extend(1.0),
                water_scales: Vec4::new(pack.water.absorption, pack.water.scattering, 0.0, 0.0),
                dusk: Vec4::ZERO,
                cloud_flash: Vec4::ZERO,
                bow: Vec4::ZERO,
                held: Vec4::ZERO,
            };
        }

        // The place's haze colour nudges the horizon's, as much as the pack lets it; its
        // brightness stays the sky's.
        let mut haze_color = light.horizon;
        if let Some(tint) = air.haze {
            let tinted = tint * luminance(light.horizon) / luminance(tint).max(1e-4);
            haze_color = haze_color.lerp(tinted, pack.local_air.clamp(0.0, 1.0));
        }
        let haze_density = HAZE_DENSITY * pack.haze * (1.0 + (air.density - 1.0) * pack.local_air);

        // Fog is lit by the sky and the sun, paler than the horizon behind it, and takes the
        // colour of blowing dust.
        let grey = Vec3::splat(luminance(light.horizon));
        let mut fog_color = light.horizon.lerp(grey, 0.6) * 0.8 + light.ambient * 0.25 + light.sun * 0.15;
        let dust = weather.dust.clamp(0.0, 1.0);
        if dust > 0.0 {
            let [r, g, b] = weather.dust_color;
            let sand = Color::srgb_u8(r, g, b).to_linear().to_vec3();
            fog_color = fog_color.lerp(sand * luminance(fog_color) / luminance(sand).max(1e-4), dust.sqrt());
        }
        let weather_fog = weather_extinction(weather) * pack.fog * mist_share;
        let ground_fog = weather.ground_fog.clamp(0.0, 1.0) * GROUND_FOG * pack.mist * mist_share;

        let rays = quality.volumetric_light && pack.rays > 0.0;
        // Shafts show most when the sun is low, as in the mornings and evenings of shader
        // packs, where its light crosses the most air.
        let clear_air = RAY_CLEAR_AIR * (1.0 + 2.0 * (1.0 - smoothstep(0.15, 0.6, light.elevation)));
        Self {
            shadow,
            surface,
            haze: haze_color.extend(haze_density),
            haze_shape: Vec4::new(HAZE_BASE, HAZE_HEIGHT, view_distance * EDGE_FADE, view_distance),
            glow: light.glow.extend(if rays { RAY_REACH } else { 0.0 }),
            fog: fog_color.extend(weather_fog),
            fog_shape: Vec4::new(eye_height - FOG_BELOW_EYE, FOG_HEIGHT, SEA_LEVEL, 0.0),
            mist: Vec4::new(ground_fog, weather.fog_top, GROUND_FOG_THINNING, if ground_fog > 0.0 { GROUND_FOG_REACH } else { 0.0 }),
            rays: Vec4::new(if rays { pack.rays } else { 0.0 }, RAY_STEPS, RAY_ISOTROPIC, clear_air),
            clouds: Vec4::ZERO,
            cloud_shape: Vec4::new(1.0, 2.0, 0.0, 0.0),
            cloud_look: Vec4::ZERO,
            cloud_map: Vec4::ZERO,
            cloud_shade: Vec4::ZERO,
            ground: Vec4::new(
                weather.wetness.clamp(0.0, 1.0) * pack.wet_ground,
                weather.frost.clamp(0.0, 1.0) * pack.frost,
                weather.snow_cover.clamp(0.0, 1.0) * pack.frost,
                pack.bounce,
            ),
            water: Vec4::ZERO,
            water_light: Vec4::ZERO,
            water_scales: Vec4::ZERO,
            dusk: light.dusk.extend(0.0),
            cloud_flash: Vec4::ZERO,
            bow: Vec4::ZERO,
            held: Vec4::ZERO,
        }
    }

    /// Adds the low cloud: a layer in the world whose base comes down in wet weather and
    /// which heaps up with convection, its noise coordinates over the eye (`layer`, as
    /// `SkyParams::weather.xy`), where the eye is and the direction to the light (for the
    /// cloud maps' places), how long it has been forming and a lightning flash in it.
    /// Its shadows on the land fade as the sky closes over, when the cloud's dimming of the
    /// whole sky (in the sun's colour) takes over.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn with_clouds(
        mut self,
        weather: &Weather,
        layer: Vec2,
        eye: DVec3,
        to_light: Vec3,
        seconds: f32,
        flash: f32,
        cloud_flash: Option<(Vec3, f32)>,
        pack: &ShaderPack,
    ) -> Self {
        // Rain only falls from a well covered sky.
        let cover = (weather.cover + 0.25 * weather.precipitation).min(1.0);
        let strength = pack.cloud_shadows * (1.0 - smoothstep(0.6, 0.95, cover));
        let wet = smoothstep(0.5, 1.0, cover) * (0.4 + 0.6 * weather.precipitation.min(1.0));
        let base = SEA_LEVEL + CLOUD_BASE[0] + (CLOUD_BASE[1] - CLOUD_BASE[0]) * wet;
        let depth = (CLOUD_DEPTH[0] + (CLOUD_DEPTH[1] - CLOUD_DEPTH[0]) * weather.convection) * (0.7 + 0.3 * cover);
        let storm = weather.precipitation.min(1.0) * (0.4 + 0.6 * weather.convection);
        self.clouds = Vec4::new(layer.x, layer.y, cover, weather.convection);
        self.cloud_shape = Vec4::new(base, base + depth, seconds, storm);
        self.cloud_look = Vec4::new(strength, flash, if cover > 0.005 { CLOUD_STEPS } else { 0.0 }, CLOUD_FAR);
        let drawn = if cover > 0.005 { 1.0 } else { 0.0 };
        let map = cloud_map::corner(eye.xz());
        self.cloud_map = Vec4::new(map.x, map.y, cloud_map::EXTENT, drawn);
        // Ground around the eye takes its shadow from the cloud towards the light above it,
        // up to a quarter of the shade map away (a low sun's shadows come from far off).
        let middle = (base + depth / 2.0) as f64;
        let mut towards = DVec2::ZERO;
        if to_light.y > 0.02 {
            towards = (to_light.xz() / to_light.y).as_dvec2() * (middle - eye.y).max(0.0);
            towards = towards.clamp_length_max(cloud_map::SHADE_EXTENT as f64 / 4.0);
        }
        let shade = cloud_map::shade_corner(eye.xz(), eye.xz() + towards);
        self.cloud_shade = Vec4::new(shade.x, shade.y, cloud_map::SHADE_EXTENT, drawn);
        self.cloud_flash = cloud_flash.map_or(Vec4::ZERO, |(at, brightness)| at.extend(brightness));
        self
    }
}

/// Sea level (`voxmar_worldgen::SEA_LEVEL`), which the cloud base is measured from.
const SEA_LEVEL: f32 = voxmar_worldgen::SEA_LEVEL as f32;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Light the weather takes out of the view per block, beyond clear air's: fog and mist,
/// dust, and falling rain and snow, so that at `Weather::visibility` (ground fog aside,
/// which lies in its own layer) about 2% of what lies there still shows. Rain murks the air
/// more than that function counts it for gameplay, as in the hiking video: a downpour
/// leaves about 150 blocks of view, steady rain about a kilometre, drizzle most of it.
pub fn weather_extinction(weather: &Weather) -> f32 {
    let rain = 0.8 * smoothstep(0.0, 1.0, weather.precipitation) * (1.0 - weather.snow);
    let falling = 0.6 * weather.precipitation * (0.35 + 0.65 * weather.snow);
    let murk = (weather.fog + 0.8 * weather.dust + falling.max(rain)).clamp(0.0, 1.0);
    let visibility = 3000.0 * (1.0 - murk) * (1.0 - murk) + 30.0;
    (3.912 / visibility - 3.912 / CLEAR_VISIBILITY).max(0.0)
}

fn luminance(color: Vec3) -> f32 {
    color.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// The atmosphere uniform on the GPU: one buffer for the whole session, so bind groups
/// that hold it never need rebuilding.
#[derive(Resource)]
pub struct AtmosphereBuffer(pub Buffer);

/// The same buffer for the main world, where materials are made: a material that wants the
/// air (fog over a creature, say) binds it as a read-only storage buffer and calls
/// `fogged` from `voxmar::atmosphere` (the terrain's own shaders bind it as a uniform at
/// group 2, which a material cannot reach). Empty until the renderer has started.
#[derive(Resource, Clone, Default)]
pub struct SharedAtmosphere(Arc<OnceLock<Buffer>>);

impl SharedAtmosphere {
    pub fn get(&self) -> Option<Buffer> {
        self.0.get().cloned()
    }
}

fn create_buffer(mut commands: Commands, device: Res<RenderDevice>, shared: Res<SharedAtmosphere>) {
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("atmosphere"),
        size: Atmosphere::min_size().get(),
        usage: BufferUsages::UNIFORM | BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let _ = shared.0.set(buffer.clone());
    commands.insert_resource(AtmosphereBuffer(buffer));
}

fn write_buffer(atmosphere: Res<Atmosphere>, buffer: Res<AtmosphereBuffer>, queue: Res<RenderQueue>) {
    let mut uniform = encase::UniformBuffer::new(Vec::<u8>::new());
    uniform.write(atmosphere.as_ref()).expect("the atmosphere fits a uniform");
    queue.write_buffer(&buffer.0, 0, uniform.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_fog_leaves_two_percent_at_the_visibility() {
        assert_eq!(weather_extinction(&Weather::NONE), 0.0);
        let fog = Weather { fog: 0.6, ..Weather::NONE };
        let visibility = fog.visibility();
        let clear = 3.912 / CLEAR_VISIBILITY;
        let through = (-(weather_extinction(&fog) + clear) * visibility).exp();
        assert!((through - 0.02).abs() < 0.002, "{through}");
        // Ground fog lies in its own layer and does not thicken the air everywhere.
        let ground = Weather { ground_fog: 1.0, fog_top: 80.0, ..Weather::NONE };
        assert_eq!(weather_extinction(&ground), 0.0);
        // A downpour closes the view to about 150 blocks.
        let downpour = Weather { precipitation: 1.0, ..Weather::NONE };
        let through = (-(weather_extinction(&downpour) + clear) * 150.0).exp();
        assert!((0.01..0.05).contains(&through), "{through}");
    }

    #[test]
    fn ground_fog_follows_the_weather_and_the_pack() {
        let pack = ShaderPack::default();
        let quality = Quality::default();
        let light = Light::default();
        let clear = Atmosphere::new(&light, &Weather::NONE, &Air::default(), &pack, quality, View::new(2000.0));
        assert_eq!(clear.mist.x, 0.0);
        let valley = Weather { ground_fog: 0.9, fog_top: 82.0, ..Weather::NONE };
        let foggy = Atmosphere::new(&light, &valley, &Air::default(), &pack, quality, View::new(2000.0));
        assert!(foggy.mist.x > 0.04 && foggy.mist.y == 82.0);
        // The volumetric pass shapes it only when there is some.
        assert_eq!(clear.mist.w, 0.0);
        assert!(foggy.mist.w > 0.0);
        let none = ShaderPack { mist: 0.0, ..ShaderPack::default() };
        assert_eq!(Atmosphere::new(&light, &valley, &Air::default(), &none, quality, View::new(2000.0)).mist.x, 0.0);
    }

    #[test]
    fn mist_is_gone_where_the_sky_is_not_in_view() {
        let pack = ShaderPack::default();
        let quality = Quality::default();
        let light = Light::default();
        let weather = Weather { ground_fog: 0.9, fog_top: 82.0, fog: 0.6, ..Weather::NONE };
        let at = |open: f32| {
            let view = View { mist_share: pack.mist_share(open), ..View::new(2000.0) };
            Atmosphere::new(&light, &weather, &Air::default(), &pack, quality, view)
        };
        let outside = at(1.0);
        assert!(outside.mist.x > 0.04 && outside.mist.w > 0.0 && outside.fog.w > 0.0);
        // In a shaft dug down, ground fog and the weather's fog are both gone, and the
        // volumetric pass does not shape any.
        let shaft = at(0.0);
        assert_eq!((shaft.mist.x, shaft.mist.w, shaft.fog.w), (0.0, 0.0, 0.0));
        // At a cave mouth they thin with the sky in view.
        let mouth = at(0.25);
        assert!(mouth.mist.x > 0.0 && mouth.mist.x < outside.mist.x && mouth.fog.w < outside.fog.w);
        // The haze of clear air is not mist and stays.
        assert_eq!(shaft.haze, outside.haze);
    }

    #[test]
    fn the_low_cloud_comes_down_in_rain() {
        let pack = ShaderPack::default();
        let air = Atmosphere::new(&Light::default(), &Weather::NONE, &Air::default(), &pack, Quality::default(), View::new(2000.0));
        let cloud = |weather: Weather| air.with_clouds(&weather, Vec2::ZERO, DVec3::ZERO, Vec3::Y, 0.0, 0.0, None, &pack);
        assert_eq!(cloud(Weather::NONE).cloud_look.z, 0.0, "a clear sky has no cloud to march");
        let fair = cloud(Weather { cover: 0.4, ..Weather::NONE });
        let rain = cloud(Weather { cover: 1.0, precipitation: 1.0, ..Weather::NONE });
        assert!(fair.cloud_look.z > 0.0);
        assert!(fair.cloud_shape.x > SEA_LEVEL + 120.0, "fair-weather cloud lies above most hills: {}", fair.cloud_shape.x);
        assert!(rain.cloud_shape.x < fair.cloud_shape.x - 40.0, "rain cloud comes down onto them");
        assert!(rain.cloud_shape.y > rain.cloud_shape.x && rain.cloud_shape.w > fair.cloud_shape.w);
    }

    #[test]
    fn local_air_tints_the_haze_within_the_pack() {
        let light = Light::default();
        let air = Air { haze: Some(Vec3::new(0.9, 0.6, 0.3)), density: 2.0 };
        let quality = Quality::default();
        let follows = ShaderPack { local_air: 1.0, ..ShaderPack::default() };
        let ignores = ShaderPack { local_air: 0.0, ..ShaderPack::default() };
        let tinted = Atmosphere::new(&light, &Weather::NONE, &air, &follows, quality, View::new(2000.0));
        let plain = Atmosphere::new(&light, &Weather::NONE, &air, &ignores, quality, View::new(2000.0));
        assert_eq!(plain.haze.truncate(), light.horizon);
        assert!(tinted.haze.x > tinted.haze.z, "dusty air should warm the haze");
        // The tint keeps the horizon's brightness; density doubles.
        assert!((luminance(tinted.haze.truncate()) - luminance(light.horizon)).abs() < 1e-4);
        assert!((tinted.haze.w / plain.haze.w - 2.0).abs() < 1e-4);
    }

    #[test]
    fn volumetric_light_follows_the_setting() {
        let pack = ShaderPack { rays: 1.0, ..ShaderPack::default() };
        let on = Quality { volumetric_light: true, ..Quality::default() };
        let off = Quality { volumetric_light: false, ..Quality::default() };
        let light = Light::default();
        // Forward fog scatters all sunlight when the pass is off (reach 0).
        assert_eq!(Atmosphere::new(&light, &Weather::NONE, &Air::default(), &pack, off, View::new(2000.0)).glow.w, 0.0);
        assert!(Atmosphere::new(&light, &Weather::NONE, &Air::default(), &pack, on, View::new(2000.0)).glow.w > 0.0);
    }
}
