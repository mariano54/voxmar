//! Machines on the server: the world's factory (`voxmar_engine::factory`, defined by
//! `machines.ron` and `processing.ron`), told when machine blocks are placed and broken and
//! what the world lets each one do, run by its own process, saved with the world, and used
//! by players.
//!
//! The factory runs whether or not a machine's land is loaded, so a mill keeps grinding
//! while its owner is away. What the world gives a machine (flowing water past a wheel, the
//! wind at a windmill, the structure round a multiblock's core) is looked at once a second
//! where the land is loaded, and kept as it was where it is not.
//!
//! A player right clicks a machine (`base:use_machine`): with something in hand it takes,
//! fuel or what it works, the whole stack goes in; otherwise what it made comes out into
//! their inventory. Either way they are told how it is doing.

use serde::{Deserialize, Serialize};
use voxmar_engine::block::BlockId;
use voxmar_engine::coords::ColumnPos;
use voxmar_engine::dropped;
use voxmar_engine::factory::{MachineState, MachineView, Status};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::machine::{Role, Source};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
use crate::items::random3;
use crate::process::Process;

/// Runs the factory.
pub(crate) struct Machines;

impl Process for Machines {
    fn tier(&self) -> Tier {
        Tier::Medium
    }

    fn run(&mut self, server: &mut Server) {
        if server.factory.is_empty() {
            return;
        }
        if server.tick.is_multiple_of(TICKS_PER_SECOND as u64) {
            server.survey_machines();
        }
        let machines = server.content.machines.clone();
        server.factory.step(Tier::Medium.period() as u32, &machines);
    }
}

/// A machine as a save keeps it: its block and what it holds, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedMachine {
    pub position: [i32; 3],
    pub block: String,
    #[serde(default)]
    pub inputs: Vec<(String, u8)>,
    #[serde(default)]
    pub outputs: Vec<(String, u8)>,
    #[serde(default)]
    pub fuel: Option<(String, u8)>,
    #[serde(default)]
    pub burning: f32,
    #[serde(default)]
    pub energy: f32,
    #[serde(default)]
    pub condition: f32,
    #[serde(default)]
    pub made: u32,
    /// What it holds as a bin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin: Vec<(String, u8)>,
}

impl Server {
    /// The block at `pos` changed from `old` to `new`: a machine broken leaves the factory
    /// and drops what it held; one placed joins it.
    pub(crate) fn machine_changed(&mut self, pos: IVec3, old: BlockId, new: BlockId) {
        if old == new {
            return;
        }
        let machines = self.content.machines.clone();
        if machines.get(old).is_some()
            && let Some((_, state)) = self.factory.remove(pos)
        {
            let held = state.inputs.into_iter().chain(state.outputs).chain([state.fuel]).chain(state.bin).flatten();
            for (i, stack) in held.enumerate() {
                let random = random3(self.tick, pos, 0x3ac4 ^ i as u32);
                self.items.spawn(stack, dropped::popped(pos, random), dropped::PICKUP_DELAY);
            }
        }
        if self.factory.insert(pos, new, &machines) {
            self.survey_machine(pos, 0.0);
        }
    }

    /// Looks at what the world gives every machine whose land is loaded.
    fn survey_machines(&mut self) {
        let positions: Vec<IVec3> = self.factory.iter().map(|(pos, ..)| pos).collect();
        for pos in positions {
            self.survey_machine(pos, 1.0);
        }
    }

    /// What the world lets the machine at `pos` do, if its land is loaded: flowing water
    /// beside a water wheel, the wind at a windmill (more up high), the structure round a
    /// multiblock's core.
    /// `seconds` is how long since the last look, counted as work for an animal walking a
    /// wheel that is giving power.
    fn survey_machine(&mut self, pos: IVec3, seconds: f32) {
        if !self.world.contains(ColumnPos::of_block(pos)) {
            return;
        }
        let Some((block, state)) = self.factory.get(pos) else { return };
        let giving = state.status == Status::Generating;
        let Some(machine) = self.content.machines.get(block) else { return };
        let condition = match machine.role {
            Role::Generator { source: Source::FlowingWater, .. } => {
                let blocks = &self.content.blocks;
                // Any water but still water: a flow, a fall, a river's current. Not lava.
                let flowing = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::NEG_Y].into_iter().any(|d| {
                    self.block(pos + d).filter(|&b| !blocks.def(b).has_tag("hot")).and_then(|b| blocks.fluid(b)).is_some_and(|f| !f.source || f.falling || f.current.is_some())
                });
                if flowing { 1.0 } else { 0.0 }
            }
            Role::Generator { source: Source::Wind, .. } => {
                let at = Vec2::new(pos.x as f32 + 0.5, pos.z as f32 + 0.5);
                let (tick, day) = (self.tick, self.day);
                let rules = self.content.machines.sources;
                let wind = self.weather.local(&mut self.temperature, at, tick, day).weather.wind.length();
                // Open air around the sails, or nothing turns them.
                let open = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::Y].into_iter().all(|d| !self.is_solid(pos + d));
                if open { rules.wind_share(wind, pos.y as f32) } else { 0.0 }
            }
            Role::Generator { source: Source::Walked, .. } => {
                // A kept animal of a walking kind within reach of the wheel, and walking this
                // one: an animal walks only the wheel nearest it, so one aurochs among many
                // wheels turns one of them.
                let (machines, creatures) = (&self.content.machines, &self.content.creatures);
                let rules = machines.sources;
                let reach = |wheel: IVec3, at: Vec3| {
                    let centre = wheel.as_vec3() + Vec3::splat(0.5);
                    let across = Vec2::new(at.x - centre.x, at.z - centre.z).length();
                    (across <= rules.walk_reach && (at.y - centre.y).abs() <= rules.walk_height).then(|| at.distance(centre))
                };
                let wheels: Vec<IVec3> = self
                    .factory
                    .iter()
                    .filter(|(_, b, _)| machines.get(*b).is_some_and(|m| matches!(m.role, Role::Generator { source: Source::Walked, .. })))
                    .map(|(p, ..)| p)
                    .collect();
                let walker = self.creatures.live.values().find(|c| {
                    let at = c.body.position;
                    let Some(here) = reach(pos, at) else { return false };
                    // Nearer wheels, or as near and first in order, take the animal.
                    let taken = wheels.iter().any(|&other| other != pos && reach(other, at).is_some_and(|d| d < here || (d == here && (other.x, other.y, other.z) < (pos.x, pos.y, pos.z))));
                    c.owner.is_some() && !taken && machines.walks(&creatures.get(c.species).def.name)
                });
                // How well kept the walker is: every animal counts as ordinary (1) until kept
                // creatures have a condition of their own. Its time at work is kept for its
                // keeping (a long day at the wheel wears it).
                let walker = walker.and_then(|c| self.creatures.live.iter().find(|(_, l)| std::ptr::eq(*l, c)).map(|(&id, _)| id));
                if let Some(id) = walker.filter(|_| giving && seconds > 0.0) {
                    self.factory.add_walked(id, seconds);
                }
                walker.map_or(0.0, |_| rules.walk_share(1.0))
            }
            Role::Processor { water: true, .. } => {
                let blocks = &self.content.blocks;
                // Any water beside or below, still or flowing. Not lava.
                let wet = [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::NEG_Y]
                    .into_iter()
                    .any(|d| self.block(pos + d).is_some_and(|b| !blocks.def(b).has_tag("hot") && blocks.fluid(b).is_some()));
                if wet { 1.0 } else { 0.0 }
            }
            Role::Processor { .. } if self.content.blocks.def(block).has_tag("multiblock_core") => {
                if self.station_stands(pos) { 1.0 } else { 0.0 }
            }
            _ => return,
        };
        self.factory.set_condition(pos, condition);
    }

    /// `agent` uses the machine at `pos`: what they hold goes in if it takes it, otherwise
    /// what it made comes out; then they are told how it is doing.
    pub(crate) fn use_machine(&mut self, agent: PlayerId, pos: IVec3) {
        let machines = self.content.machines.clone();
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let mut moved = None;
        if let Some(stack) = player.inventory.get(selected) {
            let left = self.factory.put(pos, stack, &machines);
            let put = stack.count - left.map_or(0, |l| l.count);
            if put > 0 {
                player.inventory.take(selected, put);
                player.changed(traits::INVENTORY);
                moved = Some(Words::key("machine.put").with("count", Words::text(put)).with("item", Words::Block(self.content.blocks.def(stack.item).name.clone())));
            }
        }
        if moved.is_none() {
            let outputs = self.factory.take_outputs(pos);
            let mut took: Vec<ItemStack> = Vec::new();
            for stack in outputs {
                let left = player.inventory.add(stack.item, stack.count);
                if left > 0 {
                    // What does not fit stays in the machine.
                    self.factory.put_output(pos, ItemStack { count: left, ..stack });
                }
                if stack.count > left {
                    took.push(ItemStack { count: stack.count - left, ..stack });
                }
            }
            if !took.is_empty() {
                player.changed(traits::INVENTORY);
                let blocks = &self.content.blocks;
                let items = took.iter().map(|s| Words::key("machine.count").with("count", Words::text(s.count)).with("item", Words::Block(blocks.def(s.item).name.clone()))).collect();
                moved = Some(Words::key("machine.took").with("items", Words::List(items)));
            }
        }
        let Some(view) = self.factory.view(pos, &machines) else { return };
        let mut lines: Vec<Words> = moved.into_iter().collect();
        // A multiblock's core says what its structure still needs, or counts it built.
        let multiblocks = self.content.multiblocks.clone();
        let structure = multiblocks.with_core(view.block).next();
        match structure {
            Some(structure) if !self.station_stands(pos) => {
                lines.push(Words::Own(format!("{}.hint", def_key("multiblock", &structure.name)), structure.hint.clone()));
            }
            Some(structure) => {
                self.tally(agent, "built", &structure.name, 1);
                lines.push(self.machine_words(&view));
            }
            None => lines.push(self.machine_words(&view)),
        }
        let player = &self.players[&agent];
        player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::Lines(lines) });
    }

    /// How the machine at `pos` is doing, in a line, if there is one (`/machine`).
    pub(crate) fn machine_status(&self, pos: IVec3) -> Option<Words> {
        let view = self.factory.view(pos, &self.content.machines)?;
        Some(self.machine_words(&view))
    }

    /// How a machine is doing, in a line.
    fn machine_words(&self, view: &MachineView) -> Words {
        let def = self.content.blocks.def(view.block);
        let name = Words::Block(def.name.clone());
        let machine = self.content.machines.get(view.block);
        let source = machine.and_then(|m| match m.role {
            Role::Generator { source, .. } => Some(source),
            _ => None,
        });
        let washer = machine.is_some_and(|m| matches!(m.role, Role::Processor { water: true, .. }));
        let key = |key: &str| Words::key(key).with("machine", name.clone());
        match (view.status, machine.map(|m| &m.role)) {
            (_, Some(Role::Chute { .. })) => key("machine.chute").with("rate", Words::text(view.moves)),
            (_, Some(Role::Bin { .. })) => key("machine.bin").with("count", Words::text(view.held)),
            (_, Some(Role::Store { .. })) => key("machine.holds").with("energy", Words::text(view.energy)).with("capacity", Words::text(view.capacity)),
            (Status::Working, Some(Role::Processor { power: Some(_), .. })) => key("machine.working_powered")
                .with("item", view.making.map_or(Words::text("?"), |item| Words::Block(self.content.blocks.def(item).name.clone())))
                .with("progress", Words::text(view.progress))
                .with("power", Words::text(view.power)),
            (Status::Working, _) => key("machine.working")
                .with("item", view.making.map_or(Words::text("?"), |item| Words::Block(self.content.blocks.def(item).name.clone())))
                .with("progress", Words::text(view.progress)),
            (Status::Generating, _) => key("machine.generating").with("power", Words::text(view.power)),
            (Status::NoPower, _) => key("machine.no_power"),
            (Status::NoFuel, _) => key("machine.no_fuel"),
            (Status::NoSource, _) if source == Some(Source::Wind) => key("machine.no_wind"),
            (Status::NoSource, _) if washer => key("machine.needs_water"),
            (Status::NoSource, _) if source == Some(Source::Walked) => key("machine.no_walker"),
            (Status::NoSource, _) => key("machine.no_water"),
            (Status::Full, _) => key("machine.full"),
            (Status::TooHigh, _) => key("machine.too_high"),
            (Status::Overloaded, _) => key("machine.overloaded"),
            (Status::Incomplete, _) => key("machine.incomplete"),
            (Status::Idle, Some(Role::Conduit { .. })) => {
                key("machine.network").with("supply", Words::text(view.grid.supply.round())).with("demand", Words::text(view.grid.demand.round()))
            }
            (Status::Idle, _) => key("machine.idle"),
        }
    }

    /// The machines, as a save keeps them.
    pub(crate) fn saved_machines(&self) -> Vec<SavedMachine> {
        let blocks = &self.content.blocks;
        let stack = |s: &ItemStack| (blocks.def(s.item).name.clone(), s.count);
        let mut saved: Vec<SavedMachine> = self
            .factory
            .iter()
            .map(|(pos, block, state)| SavedMachine {
                position: pos.to_array(),
                block: blocks.def(block).name.clone(),
                inputs: state.inputs.iter().flatten().map(stack).collect(),
                outputs: state.outputs.iter().flatten().map(stack).collect(),
                fuel: state.fuel.as_ref().map(stack),
                burning: state.burning,
                energy: state.energy,
                condition: state.condition,
                made: state.made,
                bin: state.bin.iter().flatten().map(stack).collect(),
            })
            .collect();
        saved.sort_by_key(|m| m.position);
        saved
    }

    /// Machines from a save, minus those whose block is no longer a machine; items no
    /// longer known are lost.
    pub(crate) fn restore_machines(&mut self, saved: Vec<SavedMachine>) {
        let (blocks, machines) = (self.content.blocks.clone(), self.content.machines.clone());
        let stack = |(name, count): &(String, u8)| blocks.id(name).map(|item| ItemStack { item, count: *count });
        for machine in saved {
            let Some(block) = blocks.id(&machine.block) else { continue };
            let mut state = MachineState { burning: machine.burning, energy: machine.energy, condition: machine.condition, made: machine.made, ..MachineState::default() };
            for (slot, saved) in state.inputs.iter_mut().zip(&machine.inputs) {
                *slot = stack(saved);
            }
            for (slot, saved) in state.outputs.iter_mut().zip(&machine.outputs) {
                *slot = stack(saved);
            }
            state.fuel = machine.fuel.as_ref().and_then(stack);
            state.bin = machine.bin.iter().map(stack).collect();
            self.factory.insert_state(IVec3::from_array(machine.position), block, state, &machines);
        }
    }
}
