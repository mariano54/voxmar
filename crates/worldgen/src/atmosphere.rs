//! The air of a place: haze, a nudge to the sky and the colour grading, and ambient
//! particles. All of it follows the climate by default (dusty haze over dry land, a damp
//! green one over rainforest, cold blue air near the poles, a little more colour where
//! aether is high), so most biomes say nothing about it; a biome can set any part, and
//! fantastical ones set more. Particle sets are a shared pool (`ambient.ron`) chosen like
//! plants: by selector, weighed by how well the climate suits them, times their rarity.
//!
//! The sample is pure and cheap, for the client to take around the player every so often,
//! and blends across biome borders like everything else. Time of day and weather are the
//! client's to apply, from each particle set's conditions.

use crate::biome::BiomeId;
use crate::content::{AtmosphereDef, ParticleDef, Select, WorldContent};
use crate::ecology::{fit, selected};
use crate::geology::smoothstep;
use crate::rarity::{Featured, Rarity, Tier};
use crate::Site;

/// A particle set's index in the pool ([`crate::Overworld::particle_sets`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParticleId(pub u16);

/// The air at a position.
#[derive(Clone, Debug, PartialEq)]
pub struct Atmosphere {
    /// Haze colour, sRGB 0..=1.
    pub haze: [f32; 3],
    /// Haze density relative to the usual, around 1.
    pub haze_density: f32,
    /// Tint multiplied into the sky, sRGB 0..=1; white leaves it.
    pub sky: [f32; 3],
    /// Colour grading, −1 cooler ..= 1 warmer; slight, around 0.
    pub warmth: f32,
    /// Colour grading saturation, around 1.
    pub saturation: f32,
    /// Particle sets and how many of each, 0 ..= 1 of the most the client shows, before
    /// their time of day and weather conditions.
    pub particles: Vec<(ParticleId, f32)>,
}

/// A biome's particle candidates and the parts of its air it sets.
struct Air {
    def: AtmosphereDef,
    particles: Vec<(ParticleId, f32)>,
}

pub struct Atmospheres {
    sets: Vec<ParticleDef>,
    /// Per biome, in biome order.
    air: Vec<Air>,
}

fn rgb((r, g, b): (u8, u8, u8)) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

impl Atmospheres {
    pub fn new(content: &WorldContent, featured: &Featured) -> Self {
        let names: Vec<(&str, Rarity)> = content.particles.iter().map(|p| (p.name.as_str(), p.rarity)).collect();
        let rarity: Vec<f32> = names.iter().zip(featured.multipliers("particles", &names)).map(|((_, r), b)| r.multiplier() * b).collect();
        let every = vec![Select::Tag(String::new(), 1.0)];
        let air = content
            .biomes
            .iter()
            .map(|biome| {
                let def = &biome.atmosphere;
                let selects = def.particles.as_ref().unwrap_or(&every);
                let particles = content
                    .particles
                    .iter()
                    .enumerate()
                    .filter_map(|(i, set)| {
                        // An empty tag (the default) selects every set.
                        let tags = set.tags.iter().map(String::as_str).chain([""]);
                        let weight = selected(selects, &def.exclude, &set.name, tags) * rarity[i];
                        (weight > 0.0).then_some((ParticleId(i as u16), weight))
                    })
                    .collect();
                Air { def: def.clone(), particles }
            })
            .collect();
        Self { sets: content.particles.clone(), air }
    }

    pub fn sets(&self) -> &[ParticleDef] {
        &self.sets
    }

    /// The air at a site, its home biome's blended with the neighbour's `share`.
    pub fn at(&self, site: &Site, share: f32) -> Atmosphere {
        let home = self.of(site.home, site);
        if share <= 0.0 || site.neighbour == site.home {
            return home;
        }
        let other = self.of(site.neighbour, site);
        let lerp = |a: f32, b: f32| a + (b - a) * share;
        let mut particles = home.particles.iter().map(|&(p, d)| (p, d * (1.0 - share))).collect::<Vec<_>>();
        for &(p, d) in &other.particles {
            match particles.iter_mut().find(|e| e.0 == p) {
                Some(entry) => entry.1 += d * share,
                None => particles.push((p, d * share)),
            }
        }
        Atmosphere {
            haze: mix(home.haze, other.haze, share),
            haze_density: lerp(home.haze_density, other.haze_density),
            sky: mix(home.sky, other.sky, share),
            warmth: lerp(home.warmth, other.warmth),
            saturation: lerp(home.saturation, other.saturation),
            particles,
        }
    }

    /// One biome's air at a site: what it sets, the climate's for the rest.
    fn of(&self, biome: BiomeId, site: &Site) -> Atmosphere {
        let air = &self.air[biome.0 as usize];
        let (t, m, aether) = (site.temperature, site.soil_moisture(), site.aether);
        // The air follows the regional climate, not the damp ground of a riverbank.
        let air_m = site.moisture;
        let (dry, humid, cold, hot) = (1.0 - smoothstep(0.2, 0.6, air_m), smoothstep(1.1, 2.0, air_m), 1.0 - smoothstep(-6.0, 6.0, t), smoothstep(20.0, 30.0, t));
        let fantasy = smoothstep(0.6, 1.0, aether);

        // Pale blue-grey air, dusty over dry land, greenish over steaming forests, blue in
        // the cold, with a touch of violet where aether runs high.
        let mut haze = [0.76, 0.83, 0.92];
        haze = mix(haze, [0.86, 0.8, 0.68], dry * (0.7 + 0.3 * hot));
        haze = mix(haze, [0.78, 0.86, 0.8], humid * hot);
        haze = mix(haze, [0.8, 0.86, 0.97], cold);
        haze = mix(haze, [0.82, 0.78, 0.96], 0.5 * fantasy);
        let def = &air.def;
        let particles = air
            .particles
            .iter()
            .filter_map(|&(id, weight)| {
                let set = &self.sets[id.0 as usize];
                let aethereal = 1.0 - set.aether + set.aether * smoothstep(0.5, 0.95, aether);
                let density = (set.density * weight * fit(&set.comfort, t, m) * aethereal).min(1.0);
                (density > 0.002).then_some((id, density))
            })
            .collect();
        Atmosphere {
            haze: def.haze.map_or(haze, rgb),
            haze_density: def.haze_density.unwrap_or(1.0 + 0.4 * humid + 0.3 * dry * hot - 0.15 * cold),
            sky: def.sky.map_or(mix([1.0; 3], [0.94, 0.92, 1.0], fantasy), rgb),
            warmth: def.warmth.unwrap_or(0.15 * (hot - cold) + 0.08 * dry),
            saturation: def.saturation.unwrap_or(1.0 + 0.06 * humid - 0.08 * dry - 0.05 * cold + 0.12 * fantasy),
            particles,
        }
    }
}
