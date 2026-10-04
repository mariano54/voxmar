//! World dials: a few physically meaningful global parameters sampled from the seed.
//!
//! Every stage reads them, so worlds differ in character before any detail is generated
//! (a cold, wet world of islands; a hot, dry continent with young mountains), and
//! gameplay can ask why a world is the way it is. They are drawn together with sensible
//! correlations: more ocean and warmer air mean a more humid world, humid worlds have
//! more rivers and richer plant life, and old land is both worn smooth and quieter.
//!
//! Some dials are read by stages still to come (caves, volcanoes); they are drawn now so
//! that every world already has them and gameplay can show them.

use crate::rarity::Featured;
use crate::rng::Rng;

const PURPOSE: u64 = 0xd1a1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldDials {
    /// Mean sea-level air temperature halfway between equator and pole, °C.
    pub mean_temperature: f32,
    /// Sea-level temperature difference between the equator and a pole, °C: large for a
    /// strongly tilted world, small for a mild one.
    pub latitude_gradient: f32,
    /// Distance from the equator to a pole, in blocks. Latitude repeats along z with
    /// colder land to the north (−z), so this is the size of the climate zones.
    pub pole_distance: f32,
    /// Roughly the fraction of the surface below sea level.
    pub ocean_fraction: f32,
    /// Typical size of a landmass, in blocks: small for a world of islands, large for one
    /// of continents with dry interiors.
    pub continent_size: f32,
    /// How much water the air carries, 0 (arid world) ..= 1 (humid world).
    pub humidity: f32,
    /// Unit vector (x, z) of the direction the prevailing wind blows towards.
    pub wind: [f32; 2],
    /// Mountain building, 0 (few, low ranges) ..= 1 (young, high ranges).
    pub tectonic_activity: f32,
    /// 0 (young, rugged land) ..= 1 (old land worn smooth).
    pub erosion_age: f32,
    /// How many rivers cross the land, 0 ..= 1.
    pub river_density: f32,
    /// How richly plants grow wherever the climate allows, 0 ..= 1.
    pub vegetation: f32,
    /// How fantastical the world is: the median of the aether field, 0 ..= 1.
    pub fantasy: f32,
    /// Volcanic activity, 0 (none) ..= 1 (many volcanoes and lava fields); follows
    /// tectonic activity.
    pub volcanism: f32,
    /// How riddled with caves the ground is, 0 ..= 1.
    pub cave_density: f32,
    /// Height of the highest mountain peaks above the land around them, in blocks: active
    /// worlds raise high ranges, old ones have worn theirs down.
    pub mountain_height: f32,
    /// How many kinds of things the world grows and how many rare ones it features,
    /// 0 ..= 1.
    pub biodiversity: f32,
    /// The rare things this world boosts and the common ones it suppresses.
    pub featured: Featured,
}

impl WorldDials {
    /// Samples the dials from a generator's stream seed.
    pub fn sample(seed: u64) -> Self {
        let mut rng = Rng::new(seed, PURPOSE, 0, 0);
        // The mean of two draws leans towards the middle: extremes stay possible but rare.
        let centered = |rng: &mut Rng| (rng.unit() + rng.unit()) / 2.0;
        let tectonic_activity = centered(&mut rng);
        let warmth = centered(&mut rng);
        let mut unit = || rng.unit();
        let erosion_age = (0.75 * (1.0 - tectonic_activity) + 0.25 * unit()).clamp(0.0, 1.0);
        let ocean_fraction = 0.3 + 0.25 * unit();
        let wetness = unit();
        let humidity = (0.15 + 0.4 * wetness + 0.8 * (ocean_fraction - 0.3) + 0.25 * warmth).clamp(0.0, 1.0);
        let river_density = (0.25 + 0.5 * humidity + 0.4 * (unit() - 0.5)).clamp(0.0, 1.0);
        let vegetation = (0.4 + 0.35 * humidity + 0.15 * warmth + 0.2 * (unit() - 0.5)).clamp(0.0, 1.0);
        let angle = unit() * std::f32::consts::TAU;
        let fantasy = 0.2 + 0.3 * unit();
        let volcanism = (0.7 * tectonic_activity + 0.3 * unit()).clamp(0.0, 1.0);
        let cave_density = 0.3 + 0.5 * unit();
        let biodiversity = (0.25 + 0.5 * vegetation + 0.25 * unit()).clamp(0.0, 1.0);
        Self {
            mean_temperature: 8.0 + 7.0 * warmth,
            latitude_gradient: 26.0 + 12.0 * unit(),
            pole_distance: 3500.0 + 2000.0 * unit(),
            ocean_fraction,
            continent_size: 1400.0 + 1800.0 * unit(),
            humidity,
            wind: [angle.cos(), angle.sin()],
            tectonic_activity,
            erosion_age,
            river_density,
            vegetation,
            fantasy,
            volcanism,
            cave_density,
            mountain_height: (100.0 + 80.0 * tectonic_activity) * (1.0 - 0.25 * erosion_age),
            biodiversity,
            featured: Featured::new(seed, biodiversity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dials_are_deterministic_and_in_range() {
        assert_eq!(WorldDials::sample(7), WorldDials::sample(7));
        assert_ne!(WorldDials::sample(7), WorldDials::sample(8));
        for seed in 0..200 {
            let d = WorldDials::sample(seed);
            let units = [d.ocean_fraction, d.humidity, d.tectonic_activity, d.erosion_age, d.river_density, d.vegetation];
            for unit in units.into_iter().chain([d.fantasy, d.volcanism, d.cave_density, d.biodiversity]) {
                assert!((0.0..=1.0).contains(&unit), "{d:?}");
            }
            // Every world has temperate land somewhere between its equator and poles.
            let (equator, pole) = (d.mean_temperature + d.latitude_gradient / 2.0, d.mean_temperature - d.latitude_gradient / 2.0);
            assert!(equator > 18.0 && pole < 3.0, "{d:?}");
            assert!((d.wind[0].hypot(d.wind[1]) - 1.0).abs() < 1e-4);
        }
    }
}
