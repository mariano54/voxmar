//! Procedural world generation.
//!
//! The overworld generator runs in stages, each a pure function of the world seed and
//! position: geology (continents, mountains), climate (temperature, precipitation and the
//! biome they select), resources (ore veins) and terrain (blocks, caves, surface, trees).
//! Purity means any column can be generated alone, in any order, on any thread, and
//! features that cross column borders (trees, veins) come out identical on both sides.

mod climate;
mod features;
mod geology;
mod resources;
mod rng;
mod terrain;

use std::sync::Arc;

use voxmar_engine::block::BlockRegistry;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::glam::IVec3;
use voxmar_engine::world::Column;

pub use climate::Biome;

/// Something that produces world columns. Generators are identified by a stable id that
/// is mixed into their random streams.
pub trait Generator: Send + Sync {
    fn id(&self) -> &'static str;
    fn generate(&self, pos: ColumnPos) -> Column;
    /// A safe place to stand, near the world origin.
    fn spawn_point(&self) -> IVec3;
}

pub const SEA_LEVEL: i32 = 64;

/// The default overworld.
pub struct Overworld {
    geology: geology::Geology,
    climate: climate::Climate,
    terrain: terrain::Terrain,
    seed: u64,
}

impl Overworld {
    pub const ID: &'static str = "base:overworld";
    /// Bump when generation output changes.
    pub const VERSION: u32 = 1;

    pub fn new(seed: u64, registry: Arc<BlockRegistry>) -> Self {
        let stream = rng::stream_seed(seed, Self::ID, Self::VERSION);
        Self {
            geology: geology::Geology::new(stream),
            climate: climate::Climate::new(stream),
            terrain: terrain::Terrain::new(stream, &registry),
            seed: stream,
        }
    }

    /// Surface height and biome at a world position, from the first two stages only.
    fn surface(&self, x: i32, z: i32) -> (i32, climate::ClimateSample) {
        let geo = self.geology.sample(x, z);
        (geo.height, self.climate.sample(x, z, &geo))
    }
}

impl Generator for Overworld {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn generate(&self, pos: ColumnPos) -> Column {
        let _span = tracing::info_span!("generate_column").entered();
        terrain::generate(self, pos)
    }

    fn spawn_point(&self) -> IVec3 {
        // Walk outwards on a spiral until we find grassland that is not a mountain top.
        for ring in 0..256 {
            for step in 0..(ring * 8).max(1) {
                let (x, z) = spiral(ring, step);
                let (height, climate) = self.surface(x * 16, z * 16);
                let grassy = matches!(climate.biome, Biome::Plains | Biome::Forest | Biome::BirchForest | Biome::Taiga);
                if grassy && height > SEA_LEVEL + 3 && height < SEA_LEVEL + 40 {
                    return IVec3::new(x * 16, height + 1, z * 16);
                }
            }
        }
        IVec3::new(0, voxmar_engine::coords::WORLD_HEIGHT - 2, 0)
    }
}

/// The `step`-th position on the square ring at Chebyshev distance `ring`.
fn spiral(ring: i32, step: i32) -> (i32, i32) {
    if ring == 0 {
        return (0, 0);
    }
    let side = step / (ring * 2);
    let t = step % (ring * 2) - ring;
    match side {
        0 => (t, -ring),
        1 => (ring, t),
        2 => (-t, ring),
        _ => (-ring, -t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxmar_engine::light::LitRegion;
    use voxmar_engine::mesh::mesh_chunk;

    fn registry() -> Arc<BlockRegistry> {
        let source = include_str!("../../../assets/content/base/blocks.ron");
        Arc::new(BlockRegistry::from_ron("base", source).unwrap())
    }

    #[test]
    fn generation_is_deterministic_and_meshable() {
        let registry = registry();
        let world = Overworld::new(42, registry.clone());
        let spawn = world.spawn_point();
        let center = ColumnPos::of_block(spawn);
        assert_eq!(world.generate(center), world.generate(center));

        let start = std::time::Instant::now();
        let columns: [Arc<Column>; 9] =
            std::array::from_fn(|i| Arc::new(world.generate(center.offset(i as i32 % 3 - 1, i as i32 / 3 - 1))));
        let generated = start.elapsed();
        let start = std::time::Instant::now();
        let region = LitRegion::compute(&registry, &columns);
        let lit = start.elapsed();
        let start = std::time::Instant::now();
        let quads: usize = (0..8).map(|cy| mesh_chunk(&registry, &region, &columns[4], cy).opaque.vertices.len() / 4).sum();
        println!("9 columns {generated:?}, light {lit:?}, mesh {:?}, {quads} quads, spawn {spawn}", start.elapsed());
        assert!(quads > 0);
    }
}
