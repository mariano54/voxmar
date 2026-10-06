//! Wild plants growing back where they were picked (see `voxmar_engine::regrowth`).

use voxmar_engine::block::{BlockId, Support};
use voxmar_engine::glam::IVec3;
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::save::SavedRegrowing;

/// A picked plant waiting to come back: what it was, the block it grew from (it comes back
/// only if that is still there), and the tick it is due.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Regrowing {
    pub plant: BlockId,
    pub ground: BlockId,
    pub due: u64,
}

/// Seconds between looks at what is due.
const EVERY: u64 = TICKS_PER_SECOND as u64;

impl Server {
    /// Where `plant` at `pos` grows from.
    fn ground_of(&self, plant: BlockId, pos: IVec3) -> IVec3 {
        match self.content.blocks.info(plant).support {
            Support::Above => pos + IVec3::Y,
            _ => pos - IVec3::Y,
        }
    }

    /// `agent` picked (broke) the plant at `pos`: if it is wild, or in a herb planter and
    /// their Farming level is enough, it will come back. Call before the plant is gone.
    pub(crate) fn picked(&mut self, agent: PlayerId, pos: IVec3) {
        let Some(plant) = self.block(pos) else { return };
        let regrowth = self.content.regrowth.clone();
        let def = self.content.blocks.def(plant);
        if !regrowth.regrows(def, self.content.blocks.info(plant).solid) {
            return;
        }
        let rarity = def.rarity;
        let ground_at = self.ground_of(plant, pos);
        let Some(ground) = self.block(ground_at) else { return };
        let placed = self.placed.remove(&pos);
        if placed {
            if Some(ground) != regrowth.planter {
                return;
            }
            let farming = self.content.skills.id("core:farming");
            let level = farming.map_or(1, |skill| self.check_level(agent, skill));
            let needs = regrowth.rules.planter_level.of(rarity) as u8;
            if level < needs {
                let words = Words::key("farming.planter_needs").with("plant", Words::Block(self.content.blocks.def(plant).name.clone())).with("level", Words::text(needs));
                if let Some(player) = self.players.get(&agent) {
                    player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
                }
                return;
            }
            if let Some(skill) = farming {
                self.grant_xp(agent, skill, regrowth.rules.planter_xp.of(rarity));
            }
        }
        let due = self.tick + (regrowth.seconds(rarity) * TICKS_PER_SECOND as f32) as u64;
        self.regrowing.insert(pos, Regrowing { plant, ground, due });
    }

    /// A player put `block` at `pos`: a plant placed by hand does not regrow by itself.
    pub(crate) fn placed_plant(&mut self, block: BlockId, pos: IVec3) {
        if self.content.regrowth.regrows(self.content.blocks.def(block), self.content.blocks.info(block).solid) {
            self.placed.insert(pos);
            self.regrowing.remove(&pos);
        }
    }

    /// Puts back the plants that are due, where their ground is still there and nothing
    /// took their place. Those in columns not loaded wait.
    pub(crate) fn regrow(&mut self) {
        if self.tick % EVERY != 0 || self.regrowing.is_empty() {
            return;
        }
        let tick = self.tick;
        let due: Vec<(IVec3, Regrowing)> = self.regrowing.iter().filter(|(_, r)| r.due <= tick).map(|(&p, &r)| (p, r)).collect();
        for (pos, regrowing) in due {
            let (Some(here), Some(ground)) = (self.block(pos), self.block(self.ground_of(regrowing.plant, pos))) else { continue };
            self.regrowing.remove(&pos);
            if here.is_air() && ground == regrowing.ground {
                self.set_block(pos, regrowing.plant);
            }
        }
    }

    /// Makes the picked plants within `radius` of `at` due now (`/grow`), and says how many.
    pub(crate) fn hurry_regrowth(&mut self, at: voxmar_engine::glam::Vec3, radius: f32) -> usize {
        let tick = self.tick;
        let mut hurried = 0;
        for (pos, regrowing) in &mut self.regrowing {
            if (pos.as_vec3() + 0.5).distance(at) <= radius {
                regrowing.due = regrowing.due.min(tick);
                hurried += 1;
            }
        }
        hurried
    }

    pub(crate) fn saved_regrowing(&self) -> Vec<SavedRegrowing> {
        let name = |id: BlockId| self.content.blocks.def(id).name.clone();
        let mut saved: Vec<SavedRegrowing> = self
            .regrowing
            .iter()
            .map(|(pos, r)| SavedRegrowing { pos: pos.to_array(), plant: name(r.plant), ground: name(r.ground), due: r.due })
            .collect();
        saved.sort_by_key(|s| s.pos);
        saved
    }
}

/// Saved regrowing plants back by id, dropping those whose blocks no longer exist.
pub(crate) fn restore(saved: &[SavedRegrowing], id: impl Fn(&str) -> Option<BlockId>) -> rustc_hash::FxHashMap<IVec3, Regrowing> {
    saved
        .iter()
        .filter_map(|s| Some((IVec3::from_array(s.pos), Regrowing { plant: id(&s.plant)?, ground: id(&s.ground)?, due: s.due })))
        .collect()
}
