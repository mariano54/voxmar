//! Content packages: everything the game knows about, loaded as data.
//!
//! Each directory under `content/` is a package whose name is its namespace. First-party
//! content (`core`, `base`) loads exactly like a mod would.

use std::path::Path;
use std::sync::Arc;

use crate::action::ActionRegistry;
use crate::block::{BlockRegistry, RegistryError};
use crate::command::CommandRegistry;
use crate::weather::WeatherTable;

#[derive(Clone)]
pub struct Content {
    pub blocks: Arc<BlockRegistry>,
    pub actions: Arc<ActionRegistry>,
    pub commands: Arc<CommandRegistry>,
    pub weather: Arc<WeatherTable>,
}

impl Content {
    /// Loads every package in `dir`, in name order so ids are the same everywhere.
    pub fn load(dir: &Path) -> Result<Self, RegistryError> {
        let read_error = |path: &Path, e: std::io::Error| RegistryError(format!("{}: {e}", path.display()));
        let mut packages: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| read_error(dir, e))?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        packages.sort();

        let mut blocks = Vec::new();
        let mut actions = ActionRegistry::default();
        let mut commands = CommandRegistry::default();
        let mut weather = WeatherTable::default();
        for package in &packages {
            let path = dir.join(package);
            if let Ok(source) = std::fs::read_to_string(path.join("blocks.ron")) {
                blocks.push((package.clone(), BlockRegistry::read(package, &source)?));
            }
            if let Ok(source) = std::fs::read_to_string(path.join("actions.ron")) {
                actions.extend(package, &source)?;
            }
            if let Ok(source) = std::fs::read_to_string(path.join("commands.ron")) {
                commands.extend(package, &source)?;
            }
            if let Ok(source) = std::fs::read_to_string(path.join("weather.ron")) {
                weather.extend(package, &source)?;
            }
        }
        let blocks = BlockRegistry::expand(blocks)?;
        Ok(Self {
            blocks: Arc::new(BlockRegistry::new(blocks)?),
            actions: Arc::new(actions),
            commands: Arc::new(commands),
            weather: Arc::new(weather),
        })
    }

    /// Fingerprint used to check that client and server load the same content.
    pub fn fingerprint(&self) -> u64 {
        self.blocks.fingerprint() ^ self.actions.fingerprint().rotate_left(17)
    }
}
