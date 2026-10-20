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
use voxmar_engine::block::{BlockId, BlockRegistry};
use voxmar_engine::coords::{CHUNK_SIZE, ColumnPos, local_index};
use voxmar_engine::dropped;
use voxmar_engine::factory::{MachineState, MachineView, Status, inputs_hold};
use voxmar_engine::glam::{IVec3, Vec2, Vec3};
use voxmar_engine::inventory::ItemStack;
use voxmar_engine::lang::{Words, def_key};
use voxmar_engine::machine::{Machine, MachineRegistry, ProcessId, Role, Source};
use voxmar_engine::schedule::Tier;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;
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
        let every = (server.content.machines.sources.survey_seconds * TICKS_PER_SECOND as f32).round().max(1.0) as u64;
        if server.tick.is_multiple_of(every) {
            server.survey_machines(every as f32 / TICKS_PER_SECOND as f32);
        }
        let machines = server.content.machines.clone();
        server.factory.step(Tier::Medium.period() as u32, TICKS_PER_SECOND as f32, &machines);
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
    /// How worn the tools in the bin are, by their place in `bin` (only those worn at all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bin_wear: Vec<(u16, u16)>,
    /// Who placed it, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Whether its standing structure was counted for quests already (a multiblock's core).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub counted: bool,
    /// The work it was doing, by what it makes first, and the seconds done of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<(String, f32)>,
}

/// Where the machines that look at their neighbours stand, gathered once a survey.
struct Sites {
    /// Treadwheels and who placed them.
    wheels: Vec<(IVec3, Option<String>)>,
    windmills: Vec<IVec3>,
    water_wheels: Vec<IVec3>,
}

/// The recipe of `machine` that makes `made` (by name) to go on with, preferring one whose
/// inputs `inputs` hold.
fn resumed(machine: &Machine, machines: &MachineRegistry, blocks: &BlockRegistry, made: &str, inputs: &[Option<ItemStack>]) -> Option<ProcessId> {
    let makes = |p: &ProcessId| machines.process(*p).outputs.first().is_some_and(|&(item, _)| blocks.def(item).name == made);
    let mut making = machine.processes.iter().copied().filter(makes);
    making.clone().find(|&p| inputs_hold(inputs, machines.process(p))).or_else(|| making.next())
}

/// Whether the water wheel at `pos` is the one the water in `cell` turns: of the wheels it
/// runs past (by `faces`, where wheels take water from), the first in order, so a fall
/// ringed by wheels turns one of them.
fn turns_first(wheels: &[IVec3], faces: &[IVec3], pos: IVec3, cell: IVec3) -> bool {
    !wheels.iter().any(|&other| (other.x, other.y, other.z) < (pos.x, pos.y, pos.z) && faces.iter().any(|&d| other + d == cell))
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
                let random = self.meta.next_pop(pos, 0x3ac4 ^ i as u32);
                self.items.spawn(stack, dropped::popped(pos, random), dropped::PICKUP_DELAY);
            }
        }
        if self.factory.insert(pos, new, &machines) {
            // One that needs water or a structure round it is unable until a survey has seen
            // them, which waits while the land beside it is not loaded.
            let looks = matches!(machines.get(new).map(|m| &m.role), Some(Role::Processor { water: true, .. })) || self.content.multiblocks.with_core(new).next().is_some();
            if looks {
                self.factory.set_condition(pos, 0.0);
            }
            let sites = self.machine_sites();
            self.survey_machine(pos, &sites, 0.0);
        }
    }

    /// `agent` placed the block at `pos`: if it is a machine, it is theirs (only their kept
    /// animals walk their treadwheels).
    pub(crate) fn placed_machine(&mut self, agent: PlayerId, pos: IVec3) {
        let name = self.players.get(&agent).map(|p| p.name.clone());
        if let Some(state) = self.factory.state_mut(pos) {
            state.owner = name;
        }
    }

    /// Where the treadwheels and windmills stand.
    fn machine_sites(&self) -> Sites {
        let machines = &self.content.machines;
        let source = |block: BlockId| match machines.get(block).map(|m| &m.role) {
            Some(Role::Generator { source, .. }) => Some(*source),
            _ => None,
        };
        let mut sites = Sites { wheels: Vec::new(), windmills: Vec::new(), water_wheels: Vec::new() };
        for (pos, block, state) in self.factory.iter() {
            match source(block) {
                Some(Source::Walked) => sites.wheels.push((pos, state.owner.clone())),
                Some(Source::Wind) => sites.windmills.push(pos),
                Some(Source::FlowingWater) => sites.water_wheels.push(pos),
                _ => {}
            }
        }
        sites
    }

    /// Looks at what the world gives every machine.
    fn survey_machines(&mut self, seconds: f32) {
        let sites = self.machine_sites();
        let positions: Vec<IVec3> = self.factory.iter().map(|(pos, ..)| pos).collect();
        for pos in positions {
            self.survey_machine(pos, &sites, seconds);
        }
        // Work kept for animals no longer about is let go.
        let live = &self.creatures.live;
        self.factory.retain_walked(|id| live.contains_key(&id));
    }

    /// What the world lets the machine at `pos` do: flowing water beside a water wheel, the
    /// wind at a windmill (more up high), a kept animal at a treadwheel, the structure round
    /// a multiblock's core. Where the land is not loaded, a windmill is reckoned to catch an
    /// average wind and the rest stay as they were last seen. `seconds` since the last look
    /// count as work for an animal walking a wheel that gives power.
    fn survey_machine(&mut self, pos: IVec3, sites: &Sites, seconds: f32) {
        let Some((block, state)) = self.factory.get(pos) else { return };
        let owner = state.owner.clone();
        let giving = state.status == Status::Generating;
        let machines = self.content.machines.clone();
        let Some(machine) = machines.get(block) else { return };
        let rules = &machines.sources;
        // Another windmill's sails near this one's becalm it.
        let becalmed = || sites.windmills.iter().any(|&other| other != pos && (other - pos).abs().max_element() <= rules.wind_spacing);
        // Another water wheel near this one shares its water and stills it.
        let crowded = || sites.water_wheels.iter().any(|&other| other != pos && (other - pos).abs().max_element() <= rules.water_spacing);
        // What it looks at around it must be loaded too, or it stays as it was last seen
        // (a windmill takes the mean wind, an animal walks no wheel).
        let core_reach = self.content.multiblocks.with_core(block).map(|m| m.reach()).max();
        let reach = match machine.role {
            // Animals are not about where the land is not loaded, so none walk it there.
            Role::Generator { source: Source::Walked, .. } => 0,
            _ if core_reach.is_some() => core_reach.unwrap_or_default(),
            Role::Processor { water: false, .. } => 0,
            _ => 1,
        };
        let loaded = self.land_loaded(pos, reach);
        if !loaded {
            match machine.role {
                Role::Generator { source: Source::Wind, .. } => {
                    let share = if becalmed() { 0.0 } else { rules.wind_share(rules.wind_mean, pos.y as f32) };
                    self.factory.set_condition(pos, share);
                }
                // No animal walks where the land is not loaded.
                Role::Generator { source: Source::Walked, .. } => self.factory.set_condition(pos, 0.0),
                _ => {}
            }
            return;
        }
        let condition = match machine.role {
            Role::Generator { source: Source::FlowingWater, .. } => {
                let blocks = &self.content.blocks;
                // A river's current or a fall of water: not water spreading from a pail, which
                // would turn a wheel for nothing. Not lava.
                // Each fall or current turns one wheel: the first in order beside it.
                let hot = &rules.hot_tag;
                let faces: Vec<IVec3> = rules.water_faces.iter().map(|&f| IVec3::from_array(f)).collect();
                let flowing = faces.iter().any(|&d| {
                    let cell = pos + d;
                    turns_first(&sites.water_wheels, &faces, pos, cell) && self.block(cell).filter(|&b| !blocks.def(b).has_tag(hot)).and_then(|b| blocks.fluid(b)).is_some_and(|f| f.falling || f.current.is_some())
                });
                if flowing && !crowded() { 1.0 } else { 0.0 }
            }
            Role::Generator { source: Source::Wind, .. } => {
                let at = Vec2::new(pos.x as f32 + 0.5, pos.z as f32 + 0.5);
                let (tick, day) = (self.tick, self.day);
                let wind = self.weather.local(&mut self.temperature, at, tick, day).weather.wind.length();
                // Open air around the sails, and no other sails near, or nothing turns them.
                let open = rules.wind_faces.iter().all(|&d| !self.is_solid(pos + IVec3::from_array(d)));
                if open && !becalmed() { rules.wind_share(wind, pos.y as f32) } else { 0.0 }
            }
            Role::Generator { source: Source::Walked, .. } => {
                // A kept animal of a walking kind within reach of the wheel, kept by whoever
                // placed it (by anyone, for a wheel no player placed), and walking this one:
                // an animal walks only the nearest wheel it may, so one aurochs among many
                // wheels turns one of them.
                let creatures = &self.content.creatures;
                let reach = |wheel: IVec3, at: Vec3| {
                    let centre = wheel.as_vec3() + Vec3::splat(0.5);
                    let across = Vec2::new(at.x - centre.x, at.z - centre.z).length();
                    (across <= rules.walk_reach && (at.y - centre.y).abs() <= rules.walk_height).then(|| at.distance(centre))
                };
                let keeps = |wheel_owner: &Option<String>, animal_owner: &Option<String>| {
                    let key = |owner: &Option<String>| owner.as_deref().map(crate::player_key);
                    animal_owner.is_some() && (wheel_owner.is_none() || key(wheel_owner) == key(animal_owner))
                };
                // Only one that would put work in (kept, grown and fed, by the condition rules
                // in creatures.ron: `Server::working_share`).
                let walker = self.creatures.live.iter().find(|&(&id, c)| {
                    let at = c.body.position;
                    let Some(here) = reach(pos, at) else { return false };
                    // Nearer wheels it may walk, or as near and first in order, take the animal.
                    let taken = sites.wheels.iter().any(|(other, other_owner)| {
                        *other != pos && self.world.contains(ColumnPos::of_block(*other)) && keeps(other_owner, &c.owner) && reach(*other, at).is_some_and(|d| d < here || (d == here && (other.x, other.y, other.z) < (pos.x, pos.y, pos.z)))
                    });
                    keeps(&owner, &c.owner) && self.working_share(id) > 0.0 && !taken && machines.walks(&creatures.get(c.species).def.name)
                });
                let walker = walker.map(|(&id, _)| id);
                // Its time at work is kept for its keeping, and it gives by its condition.
                if let Some(id) = walker.filter(|_| giving && seconds > 0.0) {
                    self.factory.add_walked(id, seconds);
                }
                walker.map_or(0.0, |id| self.working_share(id))
            }
            Role::Processor { water: true, .. } => {
                let blocks = &self.content.blocks;
                // Any water beside or below, still or flowing. Not lava.
                let wet = rules
                    .water_faces
                    .iter()
                    .map(|&d| IVec3::from_array(d))
                    .any(|d| self.block(pos + d).is_some_and(|b| !blocks.def(b).has_tag(&rules.hot_tag) && blocks.fluid(b).is_some()));
                if wet { 1.0 } else { 0.0 }
            }
            // A multiblock's core, whatever it does, works while its structure stands.
            _ if core_reach.is_some() => {
                let stands = self.station_stands(pos);
                // Fallen, it counts again for quests once rebuilt.
                if !stands && let Some(state) = self.factory.state_mut(pos) {
                    state.counted = false;
                }
                if stands { 1.0 } else { 0.0 }
            }
            _ if machine.converts() => 1.0,
            _ => return,
        };
        self.factory.set_condition(pos, condition);
    }

    /// `agent` uses the machine at `pos`: what they hold goes in if it takes it, otherwise
    /// what it made comes out; then they are told how it is doing.
    pub(crate) fn use_machine(&mut self, agent: PlayerId, pos: IVec3) {
        let machines = self.content.machines.clone();
        // What the world gives it now, so what it says as things go in is so.
        let sites = self.machine_sites();
        self.survey_machine(pos, &sites, 0.0);
        let Some(player) = self.players.get_mut(&agent) else { return };
        let selected = player.selected;
        let mut moved = None;
        // Something it takes but has no room for stays in hand, and nothing comes out.
        let mut refused = false;
        if let Some(stack) = player.inventory.get(selected) {
            refused = self.factory.accepts(pos, stack.item, &machines);
            let left = self.factory.put(pos, stack, &machines);
            let put = stack.count - left.map_or(0, |l| l.count);
            if put > 0 {
                player.inventory.take(selected, put);
                player.changed(traits::INVENTORY);
                moved = Some(Words::key("machine.put").with("count", Words::text(put)).with("item", Words::Block(self.content.blocks.def(stack.item).name.clone())));
            }
        }
        if moved.is_none() && !refused {
            let mut took: Vec<ItemStack> = Vec::new();
            // What does not fit stays in the machine, in the slot it was in.
            self.factory.take_outputs_into(pos, |stack| {
                let left = player.inventory.add_stack(stack);
                if stack.count > left {
                    took.push(ItemStack { count: stack.count - left, ..stack });
                }
                left
            });
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
            // Not standing as far as can be seen: a piece in land still loading may be there.
            Some(_) if !self.station_stands(pos) && !self.structure_loaded(pos) => lines.push(Words::key("station.unloaded")),
            Some(structure) if !self.station_stands(pos) => {
                lines.push(Words::Own(format!("{}.hint", def_key("multiblock", &structure.name)), structure.hint.clone()));
            }
            Some(structure) => {
                // Counted once a build, not on every use, for whoever placed the core (whoever
                // uses it, if nobody did), and only while they are about.
                let owner = self.factory.get(pos).filter(|(_, s)| !s.counted).map(|(_, s)| s.owner.clone());
                let builder = match owner {
                    Some(Some(name)) => self.players.iter().find(|(_, p)| p.joined && p.name == name).map(|(&id, _)| id),
                    Some(None) => Some(agent),
                    None => None,
                };
                if let Some(builder) = builder {
                    if let Some(state) = self.factory.state_mut(pos) {
                        state.counted = true;
                    }
                    self.tally(builder, "built", &structure.name, 1);
                }
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
            (Status::NoSource, _) if source == Some(Source::Wind) => key("machine.no_wind").with("spacing", Words::text(self.content.machines.sources.wind_spacing)),
            (Status::NoSource, _) if washer => key("machine.needs_water"),
            (Status::NoSource, _) if source == Some(Source::Walked) => key("machine.no_walker").with("days", Words::text(self.content.creatures.condition().fed.1)),
            (Status::NoSource, _) => key("machine.no_water").with("spacing", Words::text(self.content.machines.sources.water_spacing)),
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
        let machines = &self.content.machines;
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
                bin_wear: state.bin.iter().flatten().enumerate().filter(|(_, s)| s.wear > 0).map(|(at, s)| (at as u16, s.wear)).collect(),
                owner: state.owner.clone(),
                counted: state.counted,
                work: state.process.and_then(|p| machines.process(p).outputs.first()).map(|&(item, _)| (blocks.def(item).name.clone(), state.progress)),
            })
            .collect();
        saved.sort_by_key(|m| m.position);
        saved
    }

    /// The machines in a column just loaded and its machine blocks must agree. One whose
    /// block is gone (the world was edited, or the land was saved after it was broken)
    /// leaves the factory, dropping what it held where it stood: the save it came from is
    /// the one the dropped items and players came from, so nothing is doubled. A machine
    /// block the factory does not hold (its file could not be read) joins it, empty, if the
    /// column was saved (`adopt`): only players build machines.
    pub(crate) fn reconcile_machines(&mut self, column: ColumnPos, adopt: bool) {
        let gone: Vec<(IVec3, BlockId)> = self
            .factory
            .iter()
            .filter(|(pos, ..)| ColumnPos::of_block(*pos) == column)
            .filter_map(|(pos, block, _)| self.block(pos).filter(|&here| here != block).map(|_| (pos, block)))
            .collect();
        for (pos, block) in gone {
            tracing::warn!("a machine at {pos} is no longer in the world; it leaves the factory");
            self.machine_changed(pos, block, BlockId::AIR);
        }
        let machines = &self.content.machines;
        let Some(loaded) = self.world.column(column).filter(|_| adopt) else { return };
        let origin = column.origin();
        let mut found = Vec::new();
        for (index, chunk) in loaded.chunks.iter().enumerate() {
            if !chunk.palette().iter().any(|&b| machines.get(b).is_some()) {
                continue;
            }
            let base = origin + IVec3::new(0, index as i32 * CHUNK_SIZE, 0);
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let block = chunk.get(local_index(IVec3::new(x, y, z)));
                        let pos = base + IVec3::new(x, y, z);
                        if machines.get(block).is_some() && self.factory.get(pos).is_none() {
                            found.push((pos, block));
                        }
                    }
                }
            }
        }
        for (pos, block) in found {
            tracing::warn!("a machine block at {pos} was not in the factory; it joins it, empty");
            self.machine_changed(pos, BlockId::AIR, block);
        }
    }

    /// Machines from a save, minus those whose block is no longer a machine; items no
    /// longer known are lost.
    pub(crate) fn restore_machines(&mut self, saved: Vec<SavedMachine>) {
        let (blocks, machines) = (self.content.blocks.clone(), self.content.machines.clone());
        let stack = |(name, count): &(String, u8)| blocks.item_named(name).map(|item| ItemStack::new(item, *count));
        for machine in saved {
            let Some(block) = blocks.id(&machine.block) else { continue };
            let mut state = MachineState {
                burning: machine.burning,
                energy: machine.energy,
                condition: machine.condition,
                made: machine.made,
                owner: machine.owner.clone(),
                counted: machine.counted,
                ..MachineState::default()
            };
            // A converter needs nothing of the world but a standing structure, if it is a
            // multiblock's core (saves before converters had a condition held none).
            if machines.get(block).is_some_and(|m| m.converts()) && self.content.multiblocks.with_core(block).next().is_none() {
                state.condition = 1.0;
            }
            // Slots as the machine has them now (insert_state sizes them).
            state.inputs = machine.inputs.iter().map(stack).collect();
            state.outputs = machine.outputs.iter().map(stack).collect();
            state.fuel = machine.fuel.as_ref().and_then(stack);
            state.bin = machine.bin.iter().map(stack).collect();
            for &(at, wear) in &machine.bin_wear {
                if let Some(Some(stack)) = state.bin.get_mut(at as usize).filter(|s| s.is_some_and(|s| s.count == 1)) {
                    stack.wear = wear;
                }
            }
            // Tools stacked before they wore come apart, one to a slot (what has no room
            // falls out below).
            for slots in [&mut state.inputs, &mut state.outputs, &mut state.bin] {
                *slots = slots.drain(..).flatten().flat_map(voxmar_engine::inventory::split_stack).map(Some).collect();
            }
            // The work it was doing goes on where it was: the recipe of this machine that makes
            // the same thing from what it holds (a furnace makes an ingot from ore or from dust;
            // the next step drops it if what it holds no longer does).
            if let (Some((made, progress)), Some(def)) = (&machine.work, machines.get(block)) {
                state.process = resumed(def, &machines, &blocks, made, &state.inputs);
                state.progress = if state.process.is_some() { *progress } else { 0.0 };
            }
            // Stacks a machine no longer has room for (a package gave it fewer slots) fall out
            // where it stands.
            let pos = IVec3::from_array(machine.position);
            if let Some(def) = machines.get(block) {
                let (inputs, outputs) = def.slots();
                let mut spilled = Vec::new();
                for (slots, room) in [(&mut state.inputs, inputs), (&mut state.outputs, outputs), (&mut state.bin, def.bin_slots())] {
                    slots.retain(Option::is_some);
                    if slots.len() > room {
                        spilled.extend(slots.split_off(room).into_iter().flatten());
                    }
                }
                for (i, stack) in spilled.into_iter().enumerate() {
                    tracing::warn!("a machine at {pos} has no room for {} {}; it falls out", stack.count, blocks.def(stack.item).name);
                    let random = self.meta.next_pop(pos, 0x5b11 ^ i as u32);
                    self.items.spawn(stack, dropped::popped(pos, random), dropped::PICKUP_DELAY);
                }
            }
            self.factory.insert_state(pos, block, state, &machines);
        }
        // The land already loaded (round the spawn) is checked now, the rest as it loads.
        let mut loaded: Vec<ColumnPos> = self.world.columns().map(|(&c, _)| c).collect();
        loaded.sort_by_key(|c| (c.0.x, c.0.y));
        loaded.dedup();
        for column in loaded {
            let saved = self.save.as_ref().is_some_and(|save| save.has_column(column));
            self.reconcile_machines(column, saved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_furnace_restored_mid_ingot_goes_on_with_the_recipe_its_inputs_make() {
        let content = voxmar_engine::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let furnace = machines.get(blocks.expect("base:electric_furnace")).unwrap();
        for input in ["base:iron_ore", "base:iron_dust"] {
            let inputs = [Some(ItemStack::new(blocks.expect(input), 1)), None, None];
            let process = resumed(furnace, machines, blocks, "base:iron_ingot", &inputs).map(|p| machines.process(p));
            assert!(process.is_some_and(|p| p.inputs[0].0.contains(&blocks.expect(input))), "an ingot from {input}");
        }
    }

    #[test]
    fn a_kept_aurochs_walks_a_treadwheel_by_its_condition_and_a_young_one_not_at_all() {
        let mut server = crate::test_server();
        let wheel = server.spawn.floor().as_ivec3() + IVec3::new(3, 0, 0);
        server.load_area(wheel - IVec3::new(4, 0, 4), wheel + IVec3::new(4, 0, 4));
        let treadwheel = server.content.blocks.expect("base:treadwheel");
        server.set_block(wheel, treadwheel);
        let species = server.content.creatures.id("base:aurochs").expect("aurochs");
        let id = server.spawn_creature(species, wheel.as_vec3() + Vec3::new(0.5, 0.0, 2.5), 1).expect("room");
        let day = server.day;
        // What the wheel gives, as a share of its power, with the aurochs kept, just fed, in
        // `condition`, and young or grown.
        let mut share = |condition: f32, young: bool| {
            let c = server.creatures.live.get_mut(&id).unwrap();
            (c.owner, c.fed, c.condition, c.grows_up) = (Some("test".into()), day, condition, young.then_some(u64::MAX));
            let sites = server.machine_sites();
            server.survey_machine(wheel, &sites, 0.0);
            server.factory.get(wheel).expect("a machine").1.condition
        };
        assert!((share(0.0, false) - 0.5).abs() < 1e-3, "the poorest kept walker gives half");
        assert!((share(1.0, false) - 1.2).abs() < 1e-3, "the best kept gives six fifths");
        assert_eq!(share(1.0, true), 0.0, "a young one walks nothing");
    }

    /// A structure's core whose land is still loading says so when used, not how to build
    /// it: a piece in the next column could be standing there.
    #[test]
    fn a_core_on_land_still_loading_says_so_not_how_to_build_it() {
        let mut server = crate::test_server();
        let (client, connection) = voxmar_protocol::local_pair();
        server.players.insert(1, crate::player::Player::new(1, connection, server.spawn, 2, server.tick, server.survival.max_health, &server.config.limits));
        let column = ColumnPos::of_block(server.spawn.floor().as_ivec3());
        let core = column.origin() + IVec3::new(31, server.spawn.y as i32 + 4, 8);
        server.load_area(core - IVec3::splat(2), core + IVec3::splat(2));
        server.set_block(core, server.content.blocks.expect("base:bloomery"));
        let said = |server: &mut Server| {
            server.use_machine(1, core);
            std::iter::from_fn(|| client.try_recv().ok().flatten()).find_map(|m| match m { ServerMessage::Chat { text: Words::Lines(lines), .. } => lines.first().cloned(), _ => None })
        };
        let east = column.offset(1, 0);
        let held = server.world.remove(east).expect("loaded");
        assert_eq!(said(&mut server), Some(Words::key("station.unloaded")));
        server.world.insert(east, held);
        assert!(matches!(said(&mut server), Some(Words::Own(key, _)) if key.ends_with(".hint")), "loaded, it says how to build it");
    }

    #[test]
    fn a_fall_ringed_by_wheels_turns_one_of_them() {
        let cell = IVec3::new(0, 10, 0);
        let wheels = [IVec3::new(1, 10, 0), IVec3::new(-1, 10, 0), IVec3::new(0, 10, 1), IVec3::new(0, 10, -1), IVec3::new(0, 11, 0)];
        let faces: Vec<IVec3> = voxmar_engine::machine::SourceRules::default().water_faces.iter().map(|&f| IVec3::from_array(f)).collect();
        let turning: Vec<IVec3> = wheels.iter().copied().filter(|&w| turns_first(&wheels, &faces, w, cell)).collect();
        assert_eq!(turning, [IVec3::new(-1, 10, 0)]);
        // Water elsewhere is no other wheel's.
        assert!(turns_first(&wheels, &faces, IVec3::new(1, 10, 0), IVec3::new(2, 10, 0)));
    }
}
