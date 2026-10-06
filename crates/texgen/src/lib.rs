//! Texture programs and text sprites: pixel art written as text.
//!
//! A texture pack (see `assets/packs/default`) holds PNGs under `textures/` and recipe
//! books under `recipes/*.ron`. A recipe describes a texture instead of storing it: a
//! palette, noise layers, patterns such as bricks, boards, cells and rings, edge shading
//! and highlights, or a text grid where each character is a colour (see [`recipe`]).
//! Recipes derive from each other (`base`), so an ore is the stone recipe plus flecks and
//! a maple plank is the oak plank recipe with maple's palette, and templates make a whole
//! family of textures (a wood's log, planks, leaves and sapling) from one entry.
//!
//! **The game renders recipes when it loads**, the same way for every pack. Rendering
//! every texture of the default pack takes a few milliseconds, and it means a pack or a
//! mod can ship its art as text alone, with no build step and nothing generated to keep in
//! sync: what is in the book is what the game shows. A PNG and a recipe of the same name
//! in one pack: the PNG wins, so a hand-painted texture can replace a generated one.
//!
//! The `texgen` tool renders a pack for review (`texgen sheet`), checks that every recipe
//! renders (`texgen check`), and can write recipes out as PNGs (`texgen export`).
//!
//! Everything is deterministic: a recipe renders to the same pixels on every machine.

mod colour;
mod field;
pub mod font;
pub mod icon;
mod image;
mod library;
mod manifest;
pub mod recipe;
mod render;
mod scale;
pub mod sheet;

pub use colour::parse as parse_colour;
pub use image::Image;
pub use library::Library;
pub use manifest::Manifest;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
