//! A pack's `pack.ron`: what a texture pack the player can pick says about itself.
//!
//! Packs without one are add-ons (how mods ship art): they fill in textures but never
//! replace ours. A pack with a manifest is a whole look the player chooses in Settings →
//! Textures, such as Voxmar HD. It names the packs it builds on, whose recipes and images
//! it takes for everything it does not redraw itself, and the resolution it is drawn at,
//! which every texture it takes from them is drawn at too (see `Library::render_at_least`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What the settings call it.
    pub name: String,
    /// Packs (folder names beside this one) whose books and images this pack builds on,
    /// lowest first; its own replace theirs by name.
    #[serde(default)]
    pub extends: Vec<String>,
    /// Texels across that every block and item texture is drawn at, at least.
    #[serde(default)]
    pub resolution: u32,
}

impl Manifest {
    pub const FILE: &str = "pack.ron";

    pub fn parse(source: &str) -> Result<Self, Error> {
        ron::from_str(source).map_err(|e| Error(format!("{}: {e}", Self::FILE)))
    }

    /// The manifest in a pack folder, if it has one.
    pub fn read(pack: &Path) -> Result<Option<Self>, Error> {
        match std::fs::read_to_string(pack.join(Self::FILE)) {
            Ok(source) => Self::parse(&source).map(Some).map_err(|Error(e)| Error(format!("{}: {e}", pack.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error(format!("{}: {e}", pack.join(Self::FILE).display()))),
        }
    }

    /// The pack folder and every folder it builds on, lowest first, itself last.
    pub fn chain(pack: &Path) -> Result<Vec<PathBuf>, Error> {
        let mut chain = Vec::new();
        Self::walk(pack, &mut chain, 0)?;
        Ok(chain)
    }

    fn walk(pack: &Path, chain: &mut Vec<PathBuf>, depth: usize) -> Result<(), Error> {
        if depth > 8 {
            return Err(Error(format!("{}: packs extend each other in a cycle", pack.display())));
        }
        if let Some(manifest) = Self::read(pack)? {
            for base in &manifest.extends {
                let folder = pack.parent().unwrap_or(Path::new(".")).join(base);
                if !chain.contains(&folder) {
                    Self::walk(&folder, chain, depth + 1)?;
                }
            }
        }
        chain.push(pack.to_owned());
        Ok(())
    }
}
