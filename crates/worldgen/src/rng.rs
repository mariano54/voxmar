//! Deterministic random streams keyed by seed, generator, purpose and position.

/// Mixes a world seed with a generator id and version into the generator's base seed, so
/// a new generator version yields new land without disturbing other generators.
pub fn stream_seed(seed: u64, generator: &str, version: u32) -> u64 {
    let mut h = mix(seed ^ version as u64);
    for b in generator.bytes() {
        h = mix(h ^ b as u64);
    }
    h
}

/// SplitMix64 finaliser.
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Hash of a 3D position within a stream, for per-block decisions.
pub fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    mix(seed ^ mix((x as u32 as u64) | (z as u32 as u64) << 32) ^ (y as u32 as u64).wrapping_mul(0xff51_afd7_ed55_8ccd))
}

/// A small, fast generator (SplitMix64) for placing features.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64, purpose: u64, x: i32, z: i32) -> Self {
        Self(hash3(seed ^ purpose.wrapping_mul(0xd6e8_feb8_6659_fd93), x, 0, z))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: u32) -> i32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as i32
    }

    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo) as u32)
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}
