//! Placed lights that burn out and are lit again (progression piece 8b), all told by block
//! tags and checked at load:
//!
//! - `burns_days:D` and `burns_out_to:BLOCK`: placed, it burns `D` game days, then turns
//!   into `BLOCK` (a torch into its stub), in its own facing.
//! - `rain_douses`: rain on it, with open sky over it, puts it out at once.
//! - `relights_to:BLOCK`: a right click on it with something that `kindles` in hand (a
//!   firebrand, a lit torch) lights it again, as `BLOCK` in its facing, for nothing.
//! - `kindles`: an item that lights what `relights_to:` something.
//! - `relight_with:TAG`: beside `relights_to:`, it is lit again by an item tagged `TAG`
//!   instead, one used up (an emptied oil lamp is filled with fat).
//!
//! - `gutters_to:BLOCK`: in the last share of its burn (`gutter_share` in survival.ron) it
//!   turns into `BLOCK`, in its facing, burning on to the same end: a torch guttering, its
//!   flame low and sputtering, a cue it wants relighting. `BLOCK` burns as long, out to the
//!   same block.
//! - `tended`: set down inside a settlement (near its height), its people tend it and it
//!   never burns out there.
//!
//! What burns out and is lit again is lit into itself: a torch's stub relights to a torch.
//! What burns out to something never lit again is used up (a candle to its end).
//!
//! Picked up while it burns, a light keeps what it has burnt as its item's wear, counted in
//! seconds of the world's clock ([`WEAR_PER_DAY`] a game day), and set down again it burns
//! on from there: nothing is relit by carrying it.

/// A placed light's burn kept as its item's wear: seconds of the world's clock, 1200 a game
/// day (24 000 ticks at 20 a second).
pub const WEAR_PER_DAY: u16 = 1200;

use crate::block::{BlockId, BlockRegistry, RegistryError};

/// What each block does as a flame.
#[derive(Debug, Default)]
pub struct Flames {
    /// Each block: game days it burns placed, and what it leaves.
    out: Vec<Option<(f32, BlockId)>>,
    relights: Vec<Option<BlockId>>,
    /// Each block lit again: the tag of what is used up lighting it, if not a kindling.
    relight_with: Vec<Option<String>>,
    rain: Vec<bool>,
    kindles: Vec<bool>,
    /// Each block: what it gutters into near its end.
    gutters: Vec<Option<BlockId>>,
    tended: Vec<bool>,
}

impl Flames {
    /// Reads every block's flame tags, refusing what does not hold together.
    pub fn new(blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut flames = Self::default();
        for id in blocks.ids() {
            let def = blocks.def(id);
            let tags = &def.tags;
            let bad = |why: String| RegistryError(format!("`{}`: {why}", def.name));
            for once in ["burns_days:", "burns_out_to:", "relights_to:", "relight_with:", "gutters_to:"] {
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
                Some(d) => Some(
                    d.parse::<f32>()
                        .ok()
                        .filter(|d| d.is_finite() && *d > 0.0 && d * WEAR_PER_DAY as f32 <= u16::MAX as f32)
                        .ok_or_else(|| bad(format!("`burns_days:{d}` is not a number of days above 0 and at most {}", u16::MAX as f32 / WEAR_PER_DAY as f32)))?,
                ),
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
            let with = tags.iter().find_map(|t| t.strip_prefix("relight_with:")).map(str::to_owned);
            if let Some(tag) = &with {
                if relights.is_none() {
                    return Err(bad(format!("`relight_with:{tag}` but nothing it lights into (`relights_to:`)")));
                }
                if !blocks.ids().any(|b| blocks.def(b).has_tag(tag)) {
                    return Err(bad(format!("`relight_with:{tag}`, which nothing is tagged")));
                }
            }
            let gutters = tags.iter().find_map(|t| t.strip_prefix("gutters_to:")).map(block).transpose()?;
            if gutters.is_some() && out.is_none() {
                return Err(bad("`gutters_to:` but it never burns out (`burns_days:`)".into()));
            }
            let tended = tags.iter().any(|t| t == "tended");
            if tended && out.is_none() {
                return Err(bad("`tended` but it never burns out (`burns_days:`)".into()));
            }
            flames.tended.push(tended);
            flames.gutters.push(gutters);
            flames.out.push(out);
            flames.relight_with.push(with);
            flames.relights.push(relights);
            flames.rain.push(rain);
            flames.kindles.push(tags.iter().any(|t| t == "kindles"));
        }
        // What burns out, if it is lit again, is lit into itself (or into what gutters into
        // it).
        for id in blocks.ids() {
            let Some(stub) = flames.burns_out(blocks, id).map(|(_, stub)| stub) else { continue };
            // (A guttering light burns out to the stub of what guttered into it.)
            if flames.relit(blocks, stub).is_some_and(|lit| lit != id && flames.gutters(blocks, lit) != Some(id)) {
                return Err(RegistryError(format!(
                    "`{}`: burns out to `{}`, which does not relight to it (`relights_to:`)",
                    blocks.def(id).name,
                    blocks.def(stub).name
                )));
            }
        }
        // What gutters burns on as long as it did, out to the same block, and never gutters
        // again.
        for id in blocks.ids() {
            let Some(gutter) = flames.gutters[id.0 as usize] else { continue };
            let bad = |why: &str| RegistryError(format!("`{}`: gutters to `{}`, {why}", blocks.def(id).name, blocks.def(gutter).name));
            if blocks.info(id).facing.is_some() != blocks.info(gutter).facing.is_some() {
                return Err(bad("which does not turn as it does"));
            }
            if flames.out[gutter.0 as usize] != flames.out[id.0 as usize] {
                return Err(bad("which does not burn as long, out to the same block (`burns_days:`, `burns_out_to:`)"));
            }
            if flames.gutters[gutter.0 as usize].is_some() {
                return Err(bad("which gutters again"));
            }
        }
        Ok(flames)
    }

    /// Whether a settlement's people tend `block` set down there, so it never burns out.
    pub fn tended(&self, block: BlockId) -> bool {
        self.tended.get(block.0 as usize).copied().unwrap_or(false)
    }

    /// What `block` gutters into near its end, in its facing, if anything.
    pub fn gutters(&self, blocks: &BlockRegistry, block: BlockId) -> Option<BlockId> {
        let to = self.gutters.get(block.0 as usize).copied().flatten()?;
        Some(blocks.info(block).facing.map_or(to, |facing| blocks.facing(to, facing)))
    }

    /// The wear a light set down as `item` lasts ([`WEAR_PER_DAY`] a game day); 0 for what
    /// never burns out.
    pub fn burn_life(&self, item: BlockId) -> u16 {
        self.out.get(item.0 as usize).copied().flatten().map_or(0, |(days, _)| (days * WEAR_PER_DAY as f32).round() as u16)
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

    /// Whether `item` in hand lights `block` again: what its `relight_with:` asks, else a
    /// kindling.
    pub fn lights(&self, blocks: &BlockRegistry, block: BlockId, item: BlockId) -> bool {
        self.relit(blocks, block).is_some()
            && match self.relight_with.get(block.0 as usize).and_then(Option::as_deref) {
                Some(tag) => blocks.def(item).has_tag(tag),
                None => self.kindles(item),
            }
    }

    /// The tag of what lights `block` again and is used up doing it (`relight_with:`), if
    /// anything is.
    pub fn relit_with(&self, block: BlockId) -> Option<&str> {
        self.relight_with.get(block.0 as usize).and_then(Option::as_deref)
    }

    /// Whether lighting `block` again uses up the item it is lit with.
    pub fn uses_up(&self, block: BlockId) -> bool {
        self.relight_with.get(block.0 as usize).is_some_and(Option::is_some)
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
            ("lamp", &["burns_days:5", "burns_out_to:empty"], false),
            ("empty", &["relights_to:lamp", "relight_with:fat"], false),
            ("lard", &["fat"], false),
            ("candle", &["burns_days:3", "burns_out_to:end"], false),
            ("end", &[], false),
            ("lit", &["burns_days:1", "burns_out_to:wick", "gutters_to:low", "tended"], true),
            ("low", &["burns_days:1", "burns_out_to:wick", "relights_to:lit"], true),
            ("wick", &["relights_to:lit"], true),
        ])
        .unwrap();
        let id = |name: &str| blocks.expect(&format!("t:{name}"));
        assert_eq!(flames.burns_out(&blocks, id("torch")), Some((1.0, id("stub"))));
        assert_eq!(flames.relit(&blocks, id("stub")), Some(id("torch")));
        assert_eq!(flames.burns_out(&blocks, id("wall_east")), Some((0.5, id("wall_stub_east"))), "in its facing");
        assert_eq!(flames.relit(&blocks, id("wall_stub_west")), Some(id("wall_west")));
        assert!(flames.douses(id("torch")) && !flames.douses(id("wall")));
        assert!(flames.kindles(id("brand")) && !flames.kindles(id("torch")));
        assert!(flames.lights(&blocks, id("stub"), id("brand")) && !flames.lights(&blocks, id("stub"), id("lard")) && !flames.uses_up(id("stub")));
        assert!(flames.lights(&blocks, id("empty"), id("lard")) && !flames.lights(&blocks, id("empty"), id("brand")) && flames.uses_up(id("empty")), "filled with fat, used up");
        assert_eq!((flames.burns_out(&blocks, id("candle")), flames.relit(&blocks, id("end"))), (Some((3.0, id("end"))), None), "a candle burns to its end, never lit again");
        assert_eq!(flames.gutters(&blocks, id("lit_east")), Some(id("low_east")), "gutters in its facing");
        assert_eq!(flames.gutters(&blocks, id("low")), None);
        assert_eq!(flames.relit(&blocks, id("low_west")), Some(id("lit_west")), "guttering, lit again");
        assert!(flames.tended(id("lit")) && !flames.tended(id("torch")));
        assert_eq!((flames.burn_life(id("torch")), flames.burn_life(id("candle")), flames.burn_life(id("stub"))), (1200, 3600, 0), "seconds of the clock");
        for bad in [
            &[("torch", &["burns_days:1"][..], false)][..],
            &[("torch", &["burns_days:60", "burns_out_to:torch"][..], false)],
            &[("torch", &["tended"][..], false)],
            &[("torch", &["gutters_to:low"][..], false), ("low", &["burns_days:1", "burns_out_to:low"][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:end", "gutters_to:low"][..], false), ("low", &["burns_days:2", "burns_out_to:end"][..], false), ("end", &[][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:end", "gutters_to:low"][..], false), ("low", &["burns_days:1", "burns_out_to:low"][..], false), ("end", &[][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:end", "gutters_to:torch"][..], false), ("end", &[][..], false)],
            &[("torch", &["burns_out_to:torch"][..], false)],
            &[("torch", &["burns_days:0", "burns_out_to:torch"][..], false)],
            &[("torch", &["rain_douses"][..], false)],
            &[("torch", &["relights_to:nothing"][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:stub"][..], false), ("stub", &["relights_to:other"][..], false), ("other", &[][..], false)],
            &[("stub", &["relight_with:fat"][..], false)],
            &[("lamp", &[][..], false), ("stub", &["relights_to:lamp", "relight_with:nothing_has_it"][..], false)],
            &[("torch", &["burns_days:1", "burns_out_to:stub"][..], true), ("stub", &["relights_to:torch"][..], false)],
            &[("torch", &["burns_days:1", "burns_days:2", "burns_out_to:torch"][..], false)],
        ] {
            assert!(load(bad).is_err(), "{bad:?} loaded");
        }
    }
}
