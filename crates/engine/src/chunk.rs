//! Palette-compressed chunk storage.
//!
//! Each chunk stores a palette of distinct blocks and, per block, an index into that
//! palette packed at 0, 1, 2, 4, 8 or 16 bits. Power-of-two widths keep every entry inside
//! one `u64` word so reads are a shift and a mask. A uniform chunk (all air, all stone)
//! stores no index data at all.

use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::coords::CHUNK_VOLUME;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    palette: Vec<BlockId>,
    bits: u8,
    data: Vec<u64>,
}

impl Default for Chunk {
    fn default() -> Self {
        Self::uniform(BlockId::AIR)
    }
}

fn bits_for(palette_len: usize) -> u8 {
    match palette_len {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        17..=256 => 8,
        _ => 16,
    }
}

fn words_for(bits: u8) -> usize {
    (CHUNK_VOLUME * bits as usize).div_ceil(64)
}

impl Chunk {
    pub fn uniform(block: BlockId) -> Self {
        Self { palette: vec![block], bits: 0, data: Vec::new() }
    }

    pub fn from_dense(blocks: &[BlockId]) -> Self {
        assert_eq!(blocks.len(), CHUNK_VOLUME);
        // Many chunks are all air or all stone. Compared in runs, which vectorise.
        let first = blocks[0];
        if blocks.as_chunks::<64>().0.iter().all(|run| run.iter().fold(0, |any, b| any | (b.0 ^ first.0)) == 0) {
            return Self::uniform(first);
        }
        let mut palette: Vec<BlockId> = Vec::new();
        // Indices are resolved through a small direct-mapped cache because neighbouring
        // blocks are usually identical.
        let mut last = (BlockId(u16::MAX), 0u16);
        let mut indices = vec![0u16; CHUNK_VOLUME];
        for (dst, &block) in indices.iter_mut().zip(blocks) {
            if block != last.0 {
                let index = match palette.iter().position(|&p| p == block) {
                    Some(i) => i,
                    None => {
                        palette.push(block);
                        palette.len() - 1
                    }
                };
                last = (block, index as u16);
            }
            *dst = last.1;
        }
        let bits = bits_for(palette.len());
        // Whole words at a time: each holds `64 / bits` indices, lowest first.
        let data = indices
            .chunks(64 / bits as usize)
            .map(|word| word.iter().rev().fold(0u64, |packed, &index| packed << bits | index as u64))
            .collect();
        Self { palette, bits, data }
    }

    /// Returns the block if the chunk contains only one kind of block.
    pub fn as_uniform(&self) -> Option<BlockId> {
        (self.bits == 0).then(|| self.palette[0])
    }

    pub fn palette(&self) -> &[BlockId] {
        &self.palette
    }

    #[inline]
    pub fn get(&self, index: usize) -> BlockId {
        if self.bits == 0 {
            return self.palette[0];
        }
        self.palette[self.read_index(index) as usize]
    }

    pub fn set(&mut self, index: usize, block: BlockId) {
        let palette_index = match self.palette.iter().position(|&p| p == block) {
            Some(i) => i,
            None => {
                self.palette.push(block);
                let bits = bits_for(self.palette.len());
                if bits != self.bits {
                    self.repack(bits);
                }
                self.palette.len() - 1
            }
        };
        if self.bits > 0 {
            self.write_index(index, palette_index as u16);
        }
    }

    /// Writes every block into `out` (length `CHUNK_VOLUME`).
    pub fn copy_to_dense(&self, out: &mut [BlockId]) {
        assert_eq!(out.len(), CHUNK_VOLUME);
        if self.bits == 0 {
            out.fill(self.palette[0]);
            return;
        }
        let bits = self.bits as usize;
        let per_word = 64 / bits;
        let mask = (1u64 << bits) - 1;
        for (w, &word) in self.data.iter().enumerate() {
            let start = w * per_word;
            for (j, slot) in out[start..(start + per_word).min(CHUNK_VOLUME)].iter_mut().enumerate() {
                *slot = self.palette[((word >> (j * bits)) & mask) as usize];
            }
        }
    }

    /// Whether the internal data is consistent, for chunks read from storage. Other methods
    /// may panic on an inconsistent chunk.
    pub fn is_valid(&self) -> bool {
        let len = self.palette.len();
        if len == 0 || self.bits != bits_for(len) || self.data.len() != words_for(self.bits) {
            return false;
        }
        self.bits == 0 || len == 1 << self.bits || (0..CHUNK_VOLUME).all(|i| (self.read_index(i) as usize) < len)
    }

    /// Drops palette entries that are no longer used and shrinks the index width.
    pub fn compact(&mut self) {
        if self.bits == 0 {
            return;
        }
        let mut dense = vec![BlockId::AIR; CHUNK_VOLUME];
        self.copy_to_dense(&mut dense);
        *self = Self::from_dense(&dense);
    }

    #[inline]
    fn read_index(&self, index: usize) -> u16 {
        let bits = self.bits as usize;
        let bit = index * bits;
        ((self.data[bit / 64] >> (bit % 64)) & ((1 << bits) - 1)) as u16
    }

    #[inline]
    fn write_index(&mut self, index: usize, value: u16) {
        let bits = self.bits as usize;
        let bit = index * bits;
        let mask = ((1u64 << bits) - 1) << (bit % 64);
        let word = &mut self.data[bit / 64];
        *word = (*word & !mask) | ((value as u64) << (bit % 64));
    }

    fn repack(&mut self, bits: u8) {
        // A previously uniform chunk has index 0 everywhere, which zeroed data already encodes.
        let mut repacked = Self { palette: Vec::new(), bits, data: vec![0; words_for(bits)] };
        if self.bits > 0 {
            for i in 0..CHUNK_VOLUME {
                repacked.write_index(i, self.read_index(i));
            }
        }
        repacked.palette = std::mem::take(&mut self.palette);
        *self = repacked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip_through_palette_growth() {
        let mut chunk = Chunk::default();
        for i in 0..300u16 {
            chunk.set(i as usize * 7, BlockId(i + 1));
        }
        for i in 0..300u16 {
            assert_eq!(chunk.get(i as usize * 7), BlockId(i + 1));
        }
        assert_eq!(chunk.get(1), BlockId::AIR);

        let mut dense = vec![BlockId::AIR; CHUNK_VOLUME];
        chunk.copy_to_dense(&mut dense);
        assert_eq!(Chunk::from_dense(&dense), {
            let mut c = chunk.clone();
            c.compact();
            c
        });
    }

    #[test]
    fn from_dense_packs_like_set() {
        for kinds in [1u16, 2, 3, 5, 17, 300] {
            let dense: Vec<BlockId> = (0..CHUNK_VOLUME).map(|i| BlockId((i * 7919 % 1009) as u16 % kinds)).collect();
            let mut chunk = Chunk::uniform(dense[0]);
            for (i, &block) in dense.iter().enumerate() {
                chunk.set(i, block);
            }
            let packed = Chunk::from_dense(&dense);
            assert_eq!(packed.palette.len(), chunk.palette.len());
            assert!((0..CHUNK_VOLUME).all(|i| packed.get(i) == dense[i]), "{kinds} kinds");
            assert!(packed.is_valid());
        }
        assert_eq!(Chunk::from_dense(&vec![BlockId(4); CHUNK_VOLUME]), Chunk::uniform(BlockId(4)));
    }

    #[test]
    fn compact_returns_to_uniform() {
        let mut chunk = Chunk::uniform(BlockId(3));
        chunk.set(10, BlockId(4));
        chunk.set(10, BlockId(3));
        chunk.compact();
        assert_eq!(chunk.as_uniform(), Some(BlockId(3)));
    }

    #[test]
    fn validity_catches_inconsistent_data() {
        let mut chunk = Chunk::default();
        for i in 0..3u16 {
            chunk.set(i as usize, BlockId(i + 1));
        }
        assert!(chunk.is_valid());
        // Four palette slots at 2 bits; index 3 points past the three real entries.
        let mut broken = chunk.clone();
        broken.palette.pop();
        broken.bits = bits_for(broken.palette.len());
        assert!(!broken.is_valid());
        let mut short = chunk;
        short.data.pop();
        assert!(!short.is_valid());
    }
}
