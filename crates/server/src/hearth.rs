//! Lighting a multiblock (`multiblocks.ron`) by its core: the Homestead Hearth becomes a
//! player's home (the Nomad age's gate in the progression plan), where they wake after
//! dying; the Crucible Kiln (the Farm age's) starts smelting. Quests count what was lit.

use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::lang::{Words, def_key};
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;

impl Server {
    /// `agent` lights the core block at `core`: each multiblock it is the core of that
    /// stands comes to life; if none does, they are told what the first needs.
    pub(crate) fn light(&mut self, agent: PlayerId, core: IVec3) {
        let Some(block) = self.block(core) else { return };
        let multiblocks = self.content.multiblocks.clone();
        let candidates: Vec<_> = multiblocks.with_core(block).collect();
        let standing = candidates.iter().find(|m| m.stands(core, |p| self.block(p)));
        tracing::debug!("player {agent} lights {core}: {}", standing.map_or("nothing stands", |m| m.name.as_str()));
        let words = match (standing, candidates.first()) {
            (Some(lit), _) => {
                let name = lit.name.clone();
                self.tally(agent, "built", &name, 1);
                if lit.home
                    && let Some(player) = self.players.get_mut(&agent)
                {
                    // On the core, in the middle of its ring.
                    player.home = Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
                }
                let key = if lit.home { "multiblock.home" } else { "multiblock.lit" };
                Words::key(key).with("structure", Words::Own(def_key("multiblock", &lit.name), lit.title.clone()))
            }
            (None, Some(first)) => Words::Own(format!("{}.hint", def_key("multiblock", &first.name)), first.hint.clone()),
            (None, None) => return,
        };
        if let Some(player) = self.players.get(&agent) {
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
        }
    }

    /// Whether a station block that is a multiblock's core (a crucible) stands in its
    /// structure; any other station always works.
    pub(crate) fn station_stands(&self, at: IVec3) -> bool {
        let Some(block) = self.block(at) else { return false };
        if !self.content.blocks.def(block).has_tag("multiblock_core") {
            return true;
        }
        self.content.multiblocks.with_core(block).any(|m| m.stands(at, |p| self.block(p)))
    }
}
