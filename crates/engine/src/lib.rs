//! Engine core: world data, lighting, meshing, physics and game rules.
//!
//! This crate has no dependency on a renderer or windowing library. The server uses it
//! headless and the client turns its output into GPU resources.

pub mod block;
pub mod chunk;
pub mod coords;
pub mod inventory;
pub mod light;
pub mod mesh;
pub mod physics;
pub mod visibility;
pub mod world;

pub use glam;
