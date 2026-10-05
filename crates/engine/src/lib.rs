//! Engine core: world data, lighting, meshing, physics and game rules.
//!
//! The game is built from seven primitives: definitions (`block`, `action`), traits
//! (`traits`), fields (`field`), actions (`action`), processes and agents (run by the
//! server on the `schedule`), and generators (the worldgen crate).
//!
//! This crate has no dependency on a renderer or windowing library. The server uses it
//! headless and the client turns its output into GPU resources.

pub mod action;
pub mod block;
pub mod chunk;
pub mod command;
pub mod content;
pub mod coords;
pub mod craft;
pub mod crop;
pub mod dropped;
pub mod field;
pub mod fluid;
pub mod inventory;
pub mod light;
pub mod mesh;
pub mod movement;
pub mod physics;
pub mod schedule;
pub mod skill;
pub mod traits;
pub mod vitals;
pub mod visibility;
pub mod weather;
pub mod world;

pub use glam;
