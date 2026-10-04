//! Water and lava flowing: runs the engine's fluid kernel once a tick, within a budget
//! that adapts to how long updates take and how busy the server is, and sends each player
//! what changed in the columns they have, in one message.

use std::time::Instant;

use voxmar_engine::coords::ColumnPos;
use voxmar_protocol::TICKS_PER_SECOND;

use crate::Server;

impl Server {
    pub(crate) fn flow(&mut self) {
        let _span = tracing::info_span!("fluids").entered();
        let started = Instant::now();
        let mut changes = Vec::new();
        let budget = self.fluid_budget.updates();
        let stats = self.fluids.step(&self.content.blocks, &mut self.world, self.tick, budget, &mut changes);
        let busy = self.tick_seconds * TICKS_PER_SECOND as f64;
        self.fluid_budget.record(stats.updates, started.elapsed().as_secs_f64(), busy);
        if changes.is_empty() {
            return;
        }
        for &(pos, _) in &changes {
            self.dirty.insert(ColumnPos::of_block(pos));
        }
        self.send_changes(changes);
    }
}
