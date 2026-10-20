//! Water and lava flowing: runs the engine's fluid kernel once a tick, for a set number of
//! updates, and sends each player what changed in the columns they have, in one message.

use voxmar_engine::coords::ColumnPos;

use crate::Server;

/// Fluid updates a tick runs, at most. A set number, not a share of the tick's time, so
/// water spreads the same number of ticks on any machine however busy it is (a budget by
/// the clock let a loaded server's floods run slower than an idle one's). Only a flood fills it:
/// at a couple of microseconds an update, that is a third of a fifty-millisecond tick on a
/// slow machine and a twentieth on a fast one.
const UPDATES_PER_TICK: usize = 8192;

impl Server {
    pub(crate) fn flow(&mut self) {
        let _span = tracing::info_span!("fluids").entered();
        let mut changes = Vec::new();
        self.fluids.step(&self.content.blocks, &mut self.world, self.tick, UPDATES_PER_TICK, &mut changes);
        if changes.is_empty() {
            return;
        }
        // One pass over what changed, every per-cell hook in it.
        for &(pos, block) in &changes {
            self.dirty.insert(ColumnPos::of_block(pos));
            // Water washes a snare (or a planted flower) away as surely as a hand breaks
            // it: what grows there after is the world's, not the player's.
            self.forget_placed(pos);
            self.unplaced(pos, block);
            // A flood can wash a bag away: no ghost home where it lay.
            self.home_changed(pos, block);
            // Water over burning thatch puts it out; over a flame, ends what it lit.
            self.blaze_changed(pos, block);
        }
        self.send_changes(changes);
    }
}
