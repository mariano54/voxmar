//! Engine core: world data, lighting, meshing, physics and game rules.
//!
//! The game is built from seven primitives: definitions (`block`, `action`), traits
//! (`traits`), fields (`field`), actions (`action`), processes and agents (run by the
//! server on the `schedule`), and generators (the worldgen crate).
//!
//! This crate has no dependency on a renderer or windowing library. The server uses it
//! headless and the client turns its output into GPU resources.

pub mod action;
pub mod bees;
pub mod block;
pub mod calendar;
pub mod chunk;
pub mod combat;
pub mod command;
pub mod content;
pub mod cooking;
pub mod coords;
pub mod creature;
pub mod craft;
pub mod crop;
pub mod effect;
pub mod factory;
pub mod dropped;
pub mod field;
pub mod fishing;
pub mod fluid;
pub mod genes;
pub mod inventory;
pub mod journal;
pub mod lang;
pub mod level;
pub mod light;
pub mod loot;
pub mod machine;
pub mod mesh;
pub mod movement;
pub mod multiblock;
pub mod physics;
pub mod prayer;
pub mod quest;
pub mod quest_path;
pub mod recipe_book;
pub mod refusal;
pub mod fire;
pub mod regrowth;
pub mod schedule;
pub mod shape;
pub mod skill;
pub mod station;
pub mod trap;
pub mod tool;
pub mod traits;
pub mod treasure;
pub mod value;
pub mod vitals;
pub mod visibility;
pub mod wander;
pub mod weather;
pub mod world;

pub use glam;
