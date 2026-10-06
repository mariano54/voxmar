//! The Homestead Hearth: a multiblock (`multiblocks.ron`) that, lit, becomes a player's home
//! (the Nomad age's gate in the progression plan). They wake beside it after dying.

use voxmar_engine::glam::{IVec3, Vec3};
use voxmar_engine::lang::Words;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;

/// The multiblock a hearth's core must stand in.
const HEARTH: &str = "base:homestead_hearth";

impl Server {
    /// `agent` lights the hearthstone at `core`: their home if the whole hearth stands.
    pub(crate) fn claim_home(&mut self, agent: PlayerId, core: IVec3) {
        let stands = self.content.multiblocks.get(HEARTH).is_some_and(|hearth| hearth.stands(core, |p| self.block(p)));
        let Some(player) = self.players.get_mut(&agent) else { return };
        let words = if stands {
            // On the hearthstone, in the middle of its ring.
            player.home = Some(core.as_vec3() + Vec3::new(0.5, 1.0, 0.5));
            Words::key("hearth.claimed")
        } else {
            Words::key("hearth.incomplete")
        };
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: words });
    }
}
