//! World dials: a few physically meaningful global parameters sampled from the seed.
//!
//! Every stage reads them, so worlds differ in character before any detail is generated
//! (a cold, wet world of islands; a hot, dry continent with young mountains), and
//! gameplay can ask why a world is the way it is. They are drawn together with sensible
//! correlations: more ocean and warmer air mean a more humid world, humid worlds have
//! more rivers and richer plant life, and old land is both worn smooth and quieter.

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
    /// How cavernous the underground is, 0 (solid rock with few tunnels) ..= 1 (riddled
    /// with caves). Each kind of cave varies around it from the seed too.
    pub caves: f32,
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
        let latitude_gradient = 26.0 + 12.0 * unit();
        let pole_distance = 3500.0 + 2000.0 * unit();
        let continent_size = 1400.0 + 1800.0 * unit();
        let caves = (unit() + unit()) / 2.0;
        Self {
            mean_temperature: 8.0 + 7.0 * warmth,
            latitude_gradient,
            pole_distance,
            ocean_fraction,
            continent_size,
            humidity,
            wind: [angle.cos(), angle.sin()],
            tectonic_activity,
            erosion_age,
            river_density,
            vegetation,
            caves,
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
            for unit in [d.ocean_fraction, d.humidity, d.tectonic_activity, d.erosion_age, d.river_density, d.vegetation, d.caves] {
                assert!((0.0..=1.0).contains(&unit), "{d:?}");
            }
            // Every world has temperate land somewhere between its equator and poles.
            let (equator, pole) = (d.mean_temperature + d.latitude_gradient / 2.0, d.mean_temperature - d.latitude_gradient / 2.0);
            assert!(equator > 18.0 && pole < 3.0, "{d:?}");
            assert!((d.wind[0].hypot(d.wind[1]) - 1.0).abs() < 1e-4);
        }
    }
}
