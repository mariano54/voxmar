//! Placed lights that burn out and are lit again (progression piece 8b), all told by block
//! tags and checked at load:
//!
//! - `burns_days:D` and `burns_out_to:BLOCK`: placed, it burns `D` game days, then turns
//!   into `BLOCK` (a torch into its stub), in its own facing.
//! - `rain_douses`: rain on it, with open sky over it, puts it out at once.
//! - `relights_to:BLOCK`: a right click on it with something that `kindles` in hand (a
//!   firebrand, a lit torch) lights it again, as `BLOCK` in its facing, for nothing.
//! - `kindles`: an item that lights what `relights_to:` something.
//!
//! What burns out must be lit again into itself: a torch's stub relights to a torch.

use crate::block::{BlockId, BlockRegistry, RegistryError};

/// What each block does as a flame.
#[derive(Debug, Default)]
pub struct Flames {
    /// Each block: game days it burns placed, and what it leaves.
    out: Vec<Option<(f32, BlockId)>>,
    relights: Vec<Option<BlockId>>,
    rain: Vec<bool>,
    kindles: Vec<bool>,
}

impl Flames {
    /// Reads every block's flame tags, refusing what does not hold together.
    pub fn new(blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut flames = Self::default();
        for id in blocks.ids() {
            let def = blocks.def(id);
            let tags = &def.tags;
            let bad = |why: String| RegistryError(format!("`{}`: {why}", def.name));
            for once in ["burns_days:", "burns_out_to:", "relights_to:"] {
                if tags.iter().filter(|t| t.starts_with(once)).count() > 1 {
                    return Err(bad(format!("more than one `{once}` tag")));
                }
            }
            // A name without a package is in the block's own.
            let package = def.name.split_once(':').map_or("base", |(package, _)| package);
            let block = |name: &str| {
                let name = if name.contains(':') { name.to_owned() } else { format!("{package}:{name}") };
                blocks.id(&name).ok_or_else(|| bad(format!("no block `{name}`")))
            };
            let days = match tags.iter().find_map(|t| t.strip_prefix("burns_days:")) {
                None => None,
                Some(d) => Some(d.parse::<f32>().ok().filter(|d| d.is_finite() && *d > 0.0).ok_or_else(|| bad(format!("`burns_days:{d}` is not a number of days above 0")))?),
            };
            let to = tags.iter().find_map(|t| t.strip_prefix("burns_out_to:")).map(block).transpose()?;
            let out = match (days, to) {
                (Some(days), Some(to)) => Some((days, to)),
                (None, None) => None,
                _ => return Err(bad("`burns_days:` and `burns_out_to:` go together".into())),
            };
            let rain = tags.iter().any(|t| t == "rain_douses");
            if rain && out.is_none() {
                return Err(bad("`rain_douses` but nothing it goes out to (`burns_out_to:`)".into()));
            }
            let relights = tags.iter().find_map(|t| t.strip_prefix("relights_to:")).map(block).transpose()?;
            for (what, to) in [("burns out to", out.map(|(_, to)| to)), ("relights to", relights)] {
                if let Some(to) = to
                    && blocks.info(id).facing.is_some() != blocks.info(to).facing.is_some()
                {
                    return Err(bad(format!("{what} `{}`, which does not turn as it does", blocks.def(to).name)));
                }
            }
            flames.out.push(out);
            flames.relights.push(relights);
            flames.rain.push(rain);
            flames.kindles.push(tags.iter().any(|t| t == "kindles"));
        }
        // What burns out is lit again into itself.
        for id in blocks.ids() {
            let Some(stub) = flames.burns_out(blocks, id).map(|(_, stub)| stub) else { continue };
            if flames.relit(blocks, stub) != Some(id) {
                return Err(RegistryError(format!(
                    "`{}`: burns out to `{}`, which does not relight to it (`relights_to:`)",
                    blocks.def(id).name,
                    blocks.def(stub).name
                )));
            }
        }
        Ok(flames)
    }

    /// Game days `block` burns placed, and what it leaves then, in its facing.
    pub fn burns_out(&self, blocks: &BlockRegistry, block: BlockId) -> Option<(f32, BlockId)> {
        let (days, to) = self.out.get(block.0 as usize).copied().flatten()?;
        Some((days, blocks.info(block).facing.map_or(to, |facing| blocks.facing(to, facing))))
    }

    /// What `block` is lit again into, in its facing, if it can be.
    pub fn relit(&self, blocks: &BlockRegistry, block: BlockId) -> Option<BlockId> {
        let to = self.relights.get(block.0 as usize).copied().flatten()?;
        Some(blocks.info(block).facing.map_or(to, |facing| blocks.facing(to, facing)))
    }

    /// Whether rain under open sky puts `block` out.
    pub fn douses(&self, block: BlockId) -> bool {
        self.rain.get(block.0 as usize).copied().unwrap_or(false)
    }

    /// Whether `item` in hand lights what can be lit again.
    pub fn kindles(&self, item: BlockId) -> bool {
        self.kindles.get(item.0 as usize).copied().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn load(defs: &[(&str, &[&str], bool)]) -> Result<(BlockRegistry, Flames), RegistryError> {
        let mut all = vec![BlockDef { name: "t:air".into(), ..Default::default() }];
        all.extend(defs.iter().map(|&(name, tags, facing)| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), facing, ..Default::default() }));
        let blocks = BlockRegistry::new(all)?;
        let flames = Flames::new(&blocks)?;
        Ok((blocks, flames))
    }

    /// A torch burns out to its stub in its own facing, the stub lights again into it,
    /// and load refuses tags that do not hold together.
    #[test]
    fn what_burns_out_is_lit_again_into_itself() {
        let (blocks, flames) = load(&[
            ("torch", &["burns_days:1", "burns_out_to:stub", "rain_douses"], false),
            ("stub", &["relights_to:torch"], false),
            ("wall", &["burns_days:0.5", "burns_out_to:wall_stub"], true),
            ("wall_stub", &["relights_to:wall"], true),
            ("brand", &["kindles"], false),
        ])
        .unwrap();
        let id = |name: &str| blocks.expect(&format!("t:{name}"));
        assert_eq!(flames.burns_out(&blocks, id("torch")), Some((1.0, id("stub"))));
        assert_eq!(flames.relit(&blocks, id("stub")), Some(id("torch")));
        assert_eq!(flames.burns_out(&blocks, id("wall_east")), Some((0.5, id("wall_stub_east"))), "in its facing");
        assert_eq!(flames.relit(&blocks, id("wall_stub_west")), Some(id("wall_west")));
        assert!(flames.douses(id("torch")) && !flames.douses(id("wall")));
        assert!(flames.kindles(id("brand")) && !flames.kindles(id("torch")));
        for bad in [
            &[("torch", &["burns_days:1"][..], false)][..],
            &[("torch", &["burns_out_to:torch"][..], false)],
            &[("torch", &["burns_days:0", "burns_out_to:torch"][..], false)],
            &[("torch", &["rain_douses"][..], false)],
            &[("torch", &["relights_to:nothing"][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:stub"][..], false), ("stub", &[][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:stub"][..], true), ("stub", &["relights_to:torch"][..], false)],
            &[("torch", &["burns_days:1", "burns_days:2", "burns_out_to:torch"][..], false)],
        ] {
            assert!(load(bad).is_err(), "{bad:?} loaded");
        }
    }
}
