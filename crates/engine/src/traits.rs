//! Traits: typed data attached to things. Each trait declares how it replicates and
//! whether it persists, so nothing is synchronised or saved by accident.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replication {
    /// Never leaves the server.
    ServerOnly,
    /// Sent reliably to the owning player only.
    Owner,
    /// Sent reliably to everyone who can see the thing.
    Everyone,
    /// Sent often and unreliably to everyone nearby, and interpolated.
    Interpolated,
}

#[derive(Clone, Copy, Debug)]
pub struct TraitDecl {
    pub name: &'static str,
    pub replication: Replication,
    pub persistent: bool,
}

pub const HEALTH: TraitDecl = TraitDecl { name: "core:health", replication: Replication::Owner, persistent: true };
pub const INVENTORY: TraitDecl = TraitDecl { name: "core:inventory", replication: Replication::Owner, persistent: true };
pub const POSITION: TraitDecl =
    TraitDecl { name: "core:position", replication: Replication::Interpolated, persistent: true };

/// `core:health`: hit points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub current: u8,
    pub max: u8,
}

impl Health {
    pub const fn full(max: u8) -> Self {
        Self { current: max, max }
    }

    pub fn alive(self) -> bool {
        self.current > 0
    }

    /// Returns true if the change killed it.
    pub fn damage(&mut self, amount: u8) -> bool {
        let was_alive = self.alive();
        self.current = self.current.saturating_sub(amount);
        was_alive && !self.alive()
    }

    pub fn heal(&mut self, amount: u8) {
        self.current = (self.current + amount).min(self.max);
    }
}
