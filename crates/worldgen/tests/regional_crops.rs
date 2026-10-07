//! The regional crops (crops.ron: rye, chickpea, rice, taro) each belong to a part of the
//! world: a census of where their climate lets them grow, over the land around three
//! spawns (Review's rulings on d506b51). Each grows on some land but not most of it, has a
//! biome it thrives in, is not at its best at most spawns, shares little of its land with
//! the other regional crops and has some none of them grows on. Wheat grows almost
//! everywhere and is held to none of that; rye out-grows it where the cold holds wheat
//! back. `CENSUS=1` prints the shares.

use rustc_hash::FxHashMap;
use voxmar_engine::content::Content;
use voxmar_engine::crop::{self, Conditions};
use voxmar_engine::glam::IVec3;
use voxmar_worldgen::{Generator, Overworld, SEA_LEVEL, WorldContent};

const SEEDS: [u64; 3] = [7, 1, 42];
/// Samples on a side of the square around each spawn, and blocks between them: 12.8 km
/// across, to take in more than the spawn's own biomes.
const SIDE: i32 = 200;
const STEP: i32 = 64;
const REGIONAL: [&str; 4] = ["base:rye", "base:chickpea", "base:rice", "base:taro"];

#[test]
fn regional_crops_are_regional() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let world_content = WorldContent::load(&content).unwrap();
    let crops: Vec<crop::CropDef> = REGIONAL.iter().chain(["base:wheat"].iter()).map(|name| content.crops.iter().find(|(_, c)| c.def.name == *name).unwrap().1.def.clone()).collect();
    // Where each crop's climate lets it grow (watered, fertile, in daylight): per land
    // sample, a bit per crop.
    let mut land: Vec<(String, u8)> = Vec::new();
    let mut below_best = [0; 4];
    let mut sampled = 0usize;
    // Land where the cold holds wheat back, and where rye grows faster there.
    let (mut cold, mut rye_wins) = (0usize, 0usize);
    for seed in SEEDS {
        let world = Overworld::with_content(seed, content.blocks.clone(), &world_content);
        let spawn = world.spawn_point();
        let grows = |site: &voxmar_worldgen::Site| {
            let at = Conditions { temperature: site.temperature, moisture: site.soil_moisture(), light: 15, fertile: true, watered: true };
            crops.iter().map(|def| crop::rate(def, at)).collect::<Vec<f32>>()
        };
        let here = grows(&world.site(spawn.x, spawn.z));
        for (i, rate) in here.iter().take(4).enumerate() {
            if *rate < 1.0 {
                below_best[i] += 1;
            }
        }
        for dz in 0..SIDE {
            for dx in 0..SIDE {
                let at = IVec3::new(spawn.x + (dx - SIDE / 2) * STEP, 0, spawn.z + (dz - SIDE / 2) * STEP);
                let site = world.site(at.x, at.z);
                sampled += 1;
                if site.height <= SEA_LEVEL {
                    continue;
                }
                let rates = grows(&site);
                if site.temperature < crops[4].comfort.temperature.0 {
                    cold += 1;
                    rye_wins += (rates[0] > rates[4]) as usize;
                }
                let bits = rates.iter().enumerate().filter(|(_, r)| **r > 0.0).fold(0u8, |bits, (i, _)| bits | 1 << i);
                land.push((world.biomes().get(site.biome).name.clone(), bits));
            }
        }
    }
    let total = land.len() as f32;
    if std::env::var("CENSUS").is_ok() {
        for (i, name) in REGIONAL.iter().chain(["base:wheat"].iter()).enumerate() {
            let mine = land.iter().filter(|(_, bits)| bits & 1 << i != 0).count() as f32;
            eprintln!("{name}: {:.1}% of {} land samples, {:.1}% of all {sampled}", mine / total * 100.0, land.len(), mine / sampled as f32 * 100.0);
        }
        eprintln!("below best at spawns: {below_best:?}; rye out-grows wheat on {rye_wins} of {cold} cold-limited samples");
    }
    let mut wrong = Vec::new();
    for (i, name) in REGIONAL.iter().enumerate() {
        let mine = land.iter().filter(|(_, bits)| bits & 1 << i != 0).count() as f32;
        let share = mine / total;
        if !(0.05..=0.5).contains(&share) {
            wrong.push(format!("{name} grows on {:.1}% of land", share * 100.0));
        }
        // A biome (with enough land to judge) where it grows almost everywhere.
        let mut biomes: FxHashMap<&str, (u32, u32)> = FxHashMap::default();
        for (biome, bits) in &land {
            let entry = biomes.entry(biome.as_str()).or_default();
            entry.0 += 1;
            entry.1 += (bits & 1 << i != 0) as u32;
        }
        let best = biomes.iter().filter(|(_, (all, _))| *all >= 200).map(|(b, (all, grows))| (*grows as f32 / *all as f32, *b)).max_by(|a, b| a.0.total_cmp(&b.0));
        // Rice keeps to wet heat, which no biome is all of (Review accepts 60%).
        let thrives = if *name == "base:rice" { 0.6 } else { 0.8 };
        if !best.is_some_and(|(share, _)| share >= thrives) {
            wrong.push(format!("{name} thrives in no biome; best {best:?}"));
        }
        if below_best[i] < 2 {
            wrong.push(format!("{name} is at its best at {} of 3 spawns", 3 - below_best[i]));
        }
        let others: u8 = (0..REGIONAL.len()).filter(|&j| j != i).fold(0, |bits, j| bits | 1 << j);
        if !land.iter().any(|(_, bits)| bits & 1 << i != 0 && bits & others == 0) {
            wrong.push(format!("{name} grows nowhere the other regional crops do not"));
        }
        for (j, other) in REGIONAL.iter().enumerate().filter(|&(j, _)| j != i) {
            let shared = land.iter().filter(|(_, bits)| bits & 1 << i != 0 && bits & 1 << j != 0).count() as f32;
            if shared / mine >= 0.8 {
                wrong.push(format!("{name} shares {:.0}% of its land with {other}", shared / mine * 100.0));
            }
        }
    }
    if cold == 0 || (rye_wins as f32) < 0.8 * cold as f32 {
        wrong.push(format!("rye out-grows wheat on only {rye_wins} of {cold} samples where the cold holds wheat back"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
