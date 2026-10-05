//! The night sky of a world, drawn from its seed: its constellations, planets and their
//! orbits, the Milky Way's path, faint galaxies and nebulae, and the comets that visit it.
//! sky.wgsl draws them; this lays them out once per world.
//!
//! Nothing shows every night. The stars turn against the sun over a year of `YEAR_DAYS`
//! days, so each season has its own constellations at midnight. Outer planets creep along
//! the sun's path and spend part of their cycle lost in its glare; an inner planet never
//! strays far from the sun and shows only at dusk or dawn. The moon goes through its phases
//! every `MONTH_DAYS` days (see `moon`), so most nights are darker than a full moon's. A
//! comet comes a few times in the 1000 days the sky's clock runs before it wraps, and stays
//! for days.

use bevy::prelude::*;
use bevy::render::render_resource::ShaderType;

/// Days for the stars to turn once against the sun. Like every period here it divides the
/// 1000 days after which the sky's day count wraps, so the wrap never shows.
pub const YEAR_DAYS: f64 = 100.0;
/// Days from one full moon to the next.
pub const MONTH_DAYS: f64 = 10.0;
/// The days the sky's clock runs before it wraps.
pub const SKY_DAYS: f64 = 1000.0;

pub const CONSTELLATIONS: usize = 10;
pub const CONSTELLATION_STARS: usize = 96;
pub const PLANETS: usize = 3;
pub const DEEP_SKY: usize = 8;
pub const COMETS: usize = 3;

/// Least angle between two constellations' centres, in degrees.
const APART: f32 = 28.0;

#[derive(ShaderType, Clone, Copy, Debug, PartialEq)]
pub struct SkyLayout {
    /// Per constellation: centre longitude and latitude in degrees, the radius enclosing its
    /// stars, and its number of stars (0: none).
    pub constellations: [Vec4; CONSTELLATIONS],
    /// The constellations' stars, in order: offset east and north of the centre in degrees,
    /// brightness, and colour from blue-white (0) through white and yellow to orange (1).
    pub stars: [Vec4; CONSTELLATION_STARS],
    /// Per planet: latitude in degrees, radius in degrees, look (0 rusty with a polar cap,
    /// 1 pale blue, 2 banded and ringed), and 1 if it exists.
    pub planets: [Vec4; PLANETS],
    /// Per planet, its motion. An outer planet: x its longitude on day 0, y degrees a day
    /// it moves against the stars, w 0. An inner one: x its phase, y its period in days, z
    /// how far it strays from the sun in degrees, w 1.
    pub orbits: [Vec4; PLANETS],
    /// Faint objects: longitude and latitude in degrees, size in degrees, and kind (0 a
    /// spiral galaxy, 1 a glowing red nebula, 2 a blue one, 3 a grey dwarf galaxy; -1 none).
    pub deep_sky: [Vec4; DEEP_SKY],
    /// Their looks: a galaxy's tilt in degrees and how flattened it is; a nebula's
    /// stretch and how ragged it is.
    pub deep_look: [Vec4; DEEP_SKY],
    /// Per comet: the day it arrives, the days it stays, the longitude and latitude it
    /// appears at (it drifts a few degrees over its stay).
    pub comets: [Vec4; COMETS],
    /// Per comet: degrees a day it drifts east and north, its tail's length in degrees,
    /// and how bright it gets.
    pub comet_looks: [Vec4; COMETS],
    /// The Milky Way runs along the great circle facing this pole (xyz).
    pub milky_pole: Vec4,
    /// And is brightest towards this point on it (xyz).
    pub milky_core: Vec4,
    /// Seeds of the two layers of background stars.
    pub seeds: UVec4,
}

impl Default for SkyLayout {
    fn default() -> Self {
        Self::of_world(0)
    }
}

impl SkyLayout {
    /// The sky of the world with `seed`.
    pub fn of_world(seed: u64) -> Self {
        let mut random = Random(seed ^ 0x6e69_6768_7473_6b79);
        let mut layout = Self {
            constellations: [Vec4::ZERO; CONSTELLATIONS],
            stars: [Vec4::ZERO; CONSTELLATION_STARS],
            planets: [Vec4::ZERO; PLANETS],
            orbits: [Vec4::ZERO; PLANETS],
            deep_sky: [Vec4::new(0.0, 0.0, 0.0, -1.0); DEEP_SKY],
            deep_look: [Vec4::ZERO; DEEP_SKY],
            comets: [Vec4::ZERO; COMETS],
            comet_looks: [Vec4::ZERO; COMETS],
            milky_pole: Vec4::Z,
            milky_core: Vec4::X,
            seeds: UVec4::new(random.next() as u32, random.next() as u32, 0, 0),
        };
        layout.place_constellations(&mut random);
        layout.place_planets(&mut random);
        layout.place_deep_sky(&mut random);
        layout.place_comets(&mut random);
        let pole = random.direction();
        let core = random.direction().reject_from_normalized(pole).try_normalize().unwrap_or(pole.any_orthonormal_vector());
        layout.milky_pole = pole.extend(0.0);
        layout.milky_core = core.extend(0.0);
        layout
    }

    fn place_constellations(&mut self, random: &mut Random) {
        let mut centres: Vec<Vec3> = Vec::new();
        let mut next_star = 0;
        for i in 0..CONSTELLATIONS {
            // Spread over the sphere, never crowding another.
            let mut centre = random.direction();
            for _ in 0..64 {
                if centres.iter().all(|c| c.angle_between(centre).to_degrees() > APART) {
                    break;
                }
                centre = random.direction();
            }
            centres.push(centre);
            // One in ten is a tight cluster of faint stars; the rest are figures of a few
            // bright ones, a walk that wanders and now and then branches.
            let cluster = random.unit() < 0.1;
            let wanted = if cluster { 6 + random.below(3) } else { 5 + random.below(7) };
            let count = wanted.min(CONSTELLATION_STARS - next_star);
            if count < 3 {
                break;
            }
            let mut points = vec![Vec2::ZERO];
            let mut heading = random.unit() * std::f32::consts::TAU;
            while points.len() < count {
                let from = if points.len() > 2 && random.unit() < 0.25 { points[random.below(points.len() - 1)] } else { *points.last().unwrap() };
                heading += (random.unit() - 0.5) * 2.4;
                let step = if cluster { 0.35 + 0.6 * random.unit() } else { 2.5 + 3.5 * random.unit() };
                points.push(from + Vec2::from_angle(heading) * step);
            }
            let middle = points.iter().sum::<Vec2>() / points.len() as f32;
            let mut radius: f32 = 0.0;
            let brightest = random.below(points.len());
            for (k, point) in points.iter().enumerate() {
                let offset = *point - middle;
                radius = radius.max(offset.length());
                let brightness = match (cluster, k == brightest) {
                    (true, _) => 0.5 + 0.4 * random.unit(),
                    (false, true) => 1.5 + 0.7 * random.unit(),
                    (false, false) => 0.5 + 0.8 * random.unit().powi(2),
                };
                // Most stars look white; strongly coloured ones are rarer.
                let spread = random.unit() * 2.0 - 1.0;
                let colour = 0.5 + 0.5 * spread * spread * spread;
                self.stars[next_star + k] = Vec4::new(offset.x, offset.y, brightness, colour);
            }
            let (lon, lat) = lon_lat(centre);
            self.constellations[i] = Vec4::new(lon, lat, radius + 1.5, count as f32);
            next_star += count;
        }
    }

    fn place_planets(&mut self, random: &mut Random) {
        // One to three, each look at most once; the pale blue one, if any, an inner planet.
        let count = [1, 2, 2, 3, 3, 3][random.below(6)];
        let mut looks = [0.0, 1.0, 2.0];
        for i in (1..3).rev() {
            looks.swap(i, random.below(i + 1));
        }
        for (i, &look) in looks.iter().take(count).enumerate() {
            let latitude = (random.unit() - 0.5) * 12.0;
            let radius = 0.38 + 0.08 * random.unit();
            self.planets[i] = Vec4::new(latitude, radius, look, 1.0);
            self.orbits[i] = if look == 1.0 {
                let period = [20.0, 25.0, 40.0, 50.0][random.below(4)];
                Vec4::new(random.unit() * std::f32::consts::TAU, period, 24.0 + 22.0 * random.unit(), 1.0)
            } else {
                // Once round the stars in 200 to 1000 days: against the turning sky, it
                // meets the sun every 110 to 200 days.
                let period = [200.0, 250.0, 500.0, 1000.0][random.below(4)];
                Vec4::new(random.unit() * 360.0, 360.0 / period, 0.0, 0.0)
            };
        }
    }

    fn place_deep_sky(&mut self, random: &mut Random) {
        // Three galaxies, two nebulae and two dwarf galaxies, anywhere.
        let kinds = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 3.0];
        for (i, &kind) in kinds.iter().enumerate() {
            let (lon, lat) = lon_lat(random.direction());
            let (size, look) = match kind as u32 {
                0 => (1.0 + 2.2 * random.unit().powi(2), Vec4::new(random.unit() * 180.0 - 90.0, 0.3 + 0.6 * random.unit(), 0.0, 0.0)),
                1 | 2 => (0.6 + 0.6 * random.unit(), Vec4::new(0.0, 1.0, 0.5 + 0.2 * random.unit(), 0.0)),
                _ => (2.0 + 1.6 * random.unit(), Vec4::new(0.0, 1.4 + 0.3 * random.unit(), 0.4, 0.0)),
            };
            self.deep_sky[i] = Vec4::new(lon, lat, size, kind);
            self.deep_look[i] = look;
        }
    }

    fn place_comets(&mut self, random: &mut Random) {
        // Two or three visits in the sky's 1000 days, a week or so each, well apart.
        let count = 2 + random.below(2);
        let span = SKY_DAYS as f32 / count as f32;
        for i in 0..count {
            let arrives = i as f32 * span + random.unit() * (span - 12.0);
            let stays = 5.0 + 4.0 * random.unit();
            let (lon, lat) = lon_lat(random.direction());
            self.comets[i] = Vec4::new(arrives, stays, lon, lat.clamp(-50.0, 50.0));
            let drift = Vec2::from_angle(random.unit() * std::f32::consts::TAU) * (0.8 + 1.5 * random.unit());
            self.comet_looks[i] = Vec4::new(drift.x, drift.y, 6.0 + 14.0 * random.unit(), 0.6 + 0.4 * random.unit());
        }
    }
}

/// The moon on `days` (days elapsed): its direction from the eye, by the sun's direction
/// `sun_path` at a fraction of a day, and how much of it is lit, 0 (new) to 1 (full). It is
/// full on day 0 and on every `MONTH_DAYS`-th day after, opposite the sun; from there it
/// falls behind it a little more each day, rising later each night, until it rises and
/// sets with the sun, new, and grows back.
pub fn moon(days: f64, sun_path: impl Fn(f32) -> Vec3) -> (Vec3, f32) {
    let month = (days / MONTH_DAYS).fract() as f32;
    let direction = sun_path((days.fract() as f32 + 0.5 - month).rem_euclid(1.0));
    let lit = 0.5 + 0.5 * (month * std::f32::consts::TAU).cos();
    (direction, lit)
}

/// Longitude and latitude in degrees of a direction in the stars' frame (z their pole).
fn lon_lat(direction: Vec3) -> (f32, f32) {
    (direction.y.atan2(direction.x).to_degrees(), direction.z.clamp(-1.0, 1.0).asin().to_degrees())
}

/// SplitMix64: the same sky from the same seed, on every machine.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// 0..1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// 0..n.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A direction spread evenly over the sphere.
    fn direction(&mut self) -> Vec3 {
        let z = self.unit() * 2.0 - 1.0;
        let angle = self.unit() * std::f32::consts::TAU;
        let r = (1.0 - z * z).sqrt();
        Vec3::new(r * angle.cos(), r * angle.sin(), z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_keeps_its_sky_and_another_has_its_own() {
        assert_eq!(SkyLayout::of_world(7), SkyLayout::of_world(7));
        let (a, b) = (SkyLayout::of_world(7), SkyLayout::of_world(8));
        assert_ne!(a.constellations[0], b.constellations[0]);
        assert_ne!(a.milky_pole, b.milky_pole);
    }

    #[test]
    fn constellations_hold_their_stars_and_keep_apart() {
        for seed in 0..50 {
            let sky = SkyLayout::of_world(seed);
            let mut first = 0;
            let mut centres = Vec::new();
            for c in sky.constellations.iter().filter(|c| c.w > 0.0) {
                assert!((3.0..=11.0).contains(&c.w), "{c}");
                for star in &sky.stars[first..first + c.w as usize] {
                    assert!(star.truncate().truncate().length() + 1.5 <= c.z + 1e-3, "a star outside its constellation");
                    assert!(star.z > 0.0 && star.z < 2.5);
                }
                first += c.w as usize;
                let (lon, lat) = (c.x.to_radians(), c.y.to_radians());
                centres.push(Vec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()));
            }
            assert!(first <= CONSTELLATION_STARS && centres.len() >= 8, "seed {seed}: {} constellations", centres.len());
            let crowded = centres.iter().enumerate().flat_map(|(i, a)| centres[i + 1..].iter().map(move |b| a.angle_between(*b).to_degrees())).filter(|&d| d < APART).count();
            assert!(crowded <= 1, "seed {seed}: {crowded} constellations crowd another");
        }
    }

    #[test]
    fn the_milky_way_core_lies_on_its_band() {
        for seed in 0..20 {
            let sky = SkyLayout::of_world(seed);
            assert!(sky.milky_pole.truncate().dot(sky.milky_core.truncate()).abs() < 1e-4);
            assert!((sky.milky_core.truncate().length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn planets_differ_and_only_the_pale_one_stays_near_the_sun() {
        for seed in 0..50 {
            let sky = SkyLayout::of_world(seed);
            let present: Vec<_> = sky.planets.iter().zip(&sky.orbits).filter(|(p, _)| p.w > 0.0).collect();
            assert!(!present.is_empty());
            for (i, (planet, orbit)) in present.iter().enumerate() {
                assert_eq!(orbit.w == 1.0, planet.z == 1.0);
                assert!(present[i + 1..].iter().all(|(other, _)| other.z != planet.z), "seed {seed}: two planets look alike");
                if orbit.w == 0.0 {
                    // Whole turns in the sky's 1000 days, so the wrap never shows.
                    let turns = orbit.y as f64 * SKY_DAYS / 360.0;
                    assert!((turns - turns.round()).abs() < 1e-3, "{turns}");
                } else {
                    assert!((SKY_DAYS / orbit.y as f64).fract() == 0.0);
                }
            }
        }
    }

    #[test]
    fn comets_are_rare_and_stay_for_days() {
        for seed in 0..50 {
            let sky = SkyLayout::of_world(seed);
            let visits: Vec<_> = sky.comets.iter().filter(|c| c.y > 0.0).collect();
            assert!((2..=3).contains(&visits.len()));
            let days: f32 = visits.iter().map(|c| c.y).sum();
            assert!(days < 30.0, "comets show {days} of 1000 days");
            for c in &visits {
                assert!(c.y >= 5.0 && c.x + c.y <= SKY_DAYS as f32);
            }
        }
    }

    #[test]
    fn the_moon_is_full_opposite_the_sun_and_new_beside_it() {
        let path = |f: f32| {
            let angle = f * std::f32::consts::TAU;
            Vec3::new(angle.cos(), angle.sin(), 0.25).normalize()
        };
        // At sunrise (a day fraction of 0).
        let (full, lit) = moon(3.0 * MONTH_DAYS, path);
        assert!((lit - 1.0).abs() < 1e-4 && full.dot(path(0.0)) < -0.8);
        let (new, lit) = moon(3.5 * MONTH_DAYS, path);
        assert!(lit < 1e-4 && new.dot(path(0.0)) > 0.99);
        // A day after full it rises later: lower in the sky at the same hour of the night.
        let midnight = 0.75;
        assert!(moon(1.0 + midnight, path).0.y < moon(midnight as f64, path).0.y);
    }
}
