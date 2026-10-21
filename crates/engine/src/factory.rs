//! The factory: every machine in the world, the power networks between them and the work
//! they do (definitions in [`crate::machine`]).
//!
//! Machines live here rather than in the world's columns, so a factory keeps working while
//! its owner is away and its land unloaded (docs/design-philosophy.md: factories and power
//! keep running far from players). The server tells the factory when a machine block is
//! placed or broken, and what the world lets each generator give (water flowing past a
//! wheel, the wind at a windmill); the factory does the rest.
//!
//! **Networks** are solved per network, not per block. Machines join a network through
//! their ports: neighbours whose ports carry the same kind of power on the faces between
//! them are joined, conduits pass it on, and each network is a connected group. Every step
//! a network adds up what its generators can give and its machines ask for: generators
//! feed the machines first, stores make up a shortfall and take a surplus, and every
//! machine gets the same share, so a weak network slows its machines rather than stopping
//! one of them. Fuel burns only as fast as the power is used.
//!
//! **Tiers** (electricity): a network runs at the tier of the highest generator or store on
//! it. A machine below that tier refuses it (it needs a transformer); a cable below it
//! carries nothing at all (the cable is too thin). Nothing ever explodes.
//!
//! A machine that converts power (a dynamo, a beam engine) or steps it down (a
//! transformer) sits on two networks; what it gives on one follows what it got on the
//! other a step before.
//!
//! **Items** move along runs of chutes (and conveyors, faster chutes) between the machines
//! beside a run. Blocks have no facing, so what goes where follows from what each machine
//! is: a hopper gives what players put in it, a processor what it made; each item goes to
//! a machine on the run that works or burns it, in turn, and what a processor made that no
//! machine takes goes to a chest. Bins never feed bins, and a run moves as many items a
//! second as its slowest chute.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::coords::Face;
use crate::glam::IVec3;
use crate::inventory::{ItemStack, max_stack};
use crate::machine::{Carrier, Machine, MachineRegistry, Port, PortRole, ProcessId, Role, Source};


/// What a machine is doing, as its players are told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Nothing to work, or nothing asks for its power.
    #[default]
    Idle,
    Working,
    /// Giving power.
    Generating,
    /// It has work but its network gives no power.
    NoPower,
    /// It has work (or power is asked of it) but nothing to burn.
    NoFuel,
    /// The world gives it nothing: no flowing water, no wind.
    NoSource,
    /// Its outputs are full.
    Full,
    /// Its network is a tier above it: it needs a transformer.
    TooHigh,
    /// A cable on its network is a tier below the power on it.
    Overloaded,
    /// The structure it is the heart of does not stand (a multiblock's core).
    Incomplete,
}

/// A machine's state: what it holds and how far its work has come.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineState {
    pub inputs: Vec<Option<ItemStack>>,
    pub outputs: Vec<Option<ItemStack>>,
    pub fuel: Option<ItemStack>,
    /// Seconds of full output left of what it burns now.
    pub burning: f32,
    /// The recipe it works now, and seconds of work done on it.
    pub process: Option<ProcessId>,
    pub progress: f32,
    /// Power it holds (a store).
    pub energy: f32,
    /// How much of its output the world lets it give (water, wind), 0 to 1; for a
    /// processor, whether it may work at all (the structure it is the core of stands).
    pub condition: f32,
    /// How many things it has made, for its rolls.
    pub made: u32,
    pub status: Status,
    /// The share of what it asked that its network gave last step, 0 to 1.
    pub supplied: f32,
    /// The share of what it could give that was used last step, 0 to 1.
    pub load: f32,
    /// What it holds as a bin (a hopper, a chest).
    #[serde(default)]
    pub bin: Vec<Option<ItemStack>>,
    /// Who placed it, by name, if a player did.
    #[serde(default)]
    pub owner: Option<String>,
    /// Whether its standing structure was counted for quests already (a multiblock's core).
    #[serde(default)]
    pub counted: bool,
}

#[derive(Clone, Debug)]
struct Node {
    block: BlockId,
    ports: Vec<Port>,
    /// Items a second it carries, if it is a chute.
    chute: Option<f32>,
    state: MachineState,
}

/// A run of chutes and the machines beside it.
#[derive(Clone, Debug)]
struct Run {
    ends: Vec<IVec3>,
    /// Items a second: its slowest chute's.
    rate: f32,
    /// Items it may move now, built up a step at a time.
    credit: f32,
    /// The end the next item tries first, so items go round them in turn.
    next: usize,
}

/// One port of one machine on a network.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Member {
    pos: IVec3,
    port: usize,
}

#[derive(Clone, Debug)]
struct Network {
    carrier: Carrier,
    members: Vec<Member>,
    /// Whether it asked for more than it got last step.
    short: bool,
}

/// Every machine and the networks between them.
#[derive(Default)]
pub struct Factory {
    nodes: FxHashMap<IVec3, Node>,
    /// Machine positions in order, so a step always runs them alike.
    order: Vec<IVec3>,
    networks: Vec<Network>,
    /// Network of each port, by position then port.
    joined: FxHashMap<IVec3, Vec<usize>>,
    runs: Vec<Run>,
    /// Run of each chute.
    run_of: FxHashMap<IVec3, usize>,
    /// Seconds each animal (by creature id) has walked a wheel giving power, for its
    /// keeping: the creatures' side reads it and takes it back when the animal rests.
    walked: FxHashMap<u32, f32>,
    dirty: bool,
}

/// A network as a machine on it sees it, for its players.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub carrier: Option<Carrier>,
    pub tier: u8,
    /// What its generators and stores could give, and what its machines asked, a tick.
    pub supply: f32,
    pub demand: f32,
}

/// What a player sees of a machine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineView {
    pub block: BlockId,
    pub status: Status,
    pub inputs: Vec<Option<ItemStack>>,
    pub outputs: Vec<Option<ItemStack>>,
    pub fuel: Option<ItemStack>,
    /// How far the work is, in percent, and what it makes first.
    pub progress: u8,
    pub making: Option<BlockId>,
    /// Percent of the power it asks that it gets, or of what it could give that is used.
    pub power: u8,
    /// Power held and held full (a store).
    pub energy: u32,
    pub capacity: u32,
    pub grid: Grid,
    /// Items it holds (a bin), and items a second its run moves (a chute).
    pub held: u32,
    pub moves: f32,
}

impl Factory {
    /// Adds a machine at `pos` if `block` is one; returns whether it was. A processor or a
    /// converter starts able to work, a generator that needs the world (water, wind) unable
    /// until told otherwise ([`Factory::set_condition`]).
    pub fn insert(&mut self, pos: IVec3, block: BlockId, machines: &MachineRegistry) -> bool {
        let condition = if machines.get(block).is_some_and(|m| m.is_processor() || m.converts()) { 1.0 } else { 0.0 };
        self.insert_state(pos, block, MachineState { condition, ..MachineState::default() }, machines)
    }

    /// Adds a machine with what it held (from a save).
    pub fn insert_state(&mut self, pos: IVec3, block: BlockId, mut state: MachineState, machines: &MachineRegistry) -> bool {
        let Some(machine) = machines.get(block) else { return false };
        let (inputs, outputs) = machine.slots();
        state.inputs.resize(inputs, None);
        state.outputs.resize(outputs, None);
        // A store a home grew (`Machine::grown_bin`) keeps its size; nothing in it is lost.
        state.bin.resize(machine.bin_slots().max(state.bin.len().min(crate::inventory::STORE_SLOTS)), None);
        let chute = match machine.role {
            Role::Chute { rate } => Some(rate),
            _ => None,
        };
        self.nodes.insert(pos, Node { block, ports: machine.ports(), chute, state });
        self.dirty = true;
        true
    }

    /// Takes away the machine at `pos`, returning its state (what it held drops).
    pub fn remove(&mut self, pos: IVec3) -> Option<(BlockId, MachineState)> {
        let node = self.nodes.remove(&pos)?;
        self.dirty = true;
        Some((node.block, node.state))
    }

    pub fn get(&self, pos: IVec3) -> Option<(BlockId, &MachineState)> {
        self.nodes.get(&pos).map(|n| (n.block, &n.state))
    }

    pub fn state_mut(&mut self, pos: IVec3) -> Option<&mut MachineState> {
        self.nodes.get_mut(&pos).map(|n| &mut n.state)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Counts `seconds` of work for the animal `creature` walking a wheel.
    pub fn add_walked(&mut self, creature: u32, seconds: f32) {
        *self.walked.entry(creature).or_default() += seconds;
    }

    /// Seconds the animal has walked a wheel since it last rested.
    pub fn walked(&self, creature: u32) -> f32 {
        self.walked.get(&creature).copied().unwrap_or(0.0)
    }

    /// The animal rested (or is gone): its seconds of work, which start again from nothing.
    pub fn rested(&mut self, creature: u32) -> f32 {
        self.walked.remove(&creature).unwrap_or(0.0)
    }

    /// Lets go of the work kept for animals `keep` says are gone.
    pub fn retain_walked(&mut self, keep: impl Fn(u32) -> bool) {
        self.walked.retain(|&id, _| keep(id));
    }

    /// Every machine, by position.
    pub fn iter(&self) -> impl Iterator<Item = (IVec3, BlockId, &MachineState)> {
        self.nodes.iter().map(|(&pos, n)| (pos, n.block, &n.state))
    }

    /// Sets what the world lets the generator at `pos` give, as a share of its power (above
    /// 1 for one driven harder than usual: a fine animal on a treadwheel), or whether the
    /// processor there may work (0 or 1).
    pub fn set_condition(&mut self, pos: IVec3, condition: f32) {
        if let Some(node) = self.nodes.get_mut(&pos) {
            node.state.condition = condition.max(0.0);
        }
    }

    /// Puts what it can of `stack` into the machine at `pos`: fuel where it burns, an input
    /// where a recipe takes it, into a bin anything it holds. Returns what is left.
    pub fn put(&mut self, pos: IVec3, stack: ItemStack, machines: &MachineRegistry) -> Option<ItemStack> {
        let Some(node) = self.nodes.get_mut(&pos) else { return Some(stack) };
        let Some(machine) = machines.get(node.block) else { return Some(stack) };
        let mut left = stack.count;
        if machine.burns() && machines.burns_in(machine, stack.item).is_some() {
            left = fill(std::slice::from_mut(&mut node.state.fuel), stack.item, stack.wear, left);
        }
        if left > 0 && machine.is_processor() && machines.takes(machine, stack.item) {
            let room = machines.input_room(machine, stack.item, node.state.inputs.len());
            left = fill_within(&mut node.state.inputs, stack.item, stack.wear, left, room, |other| machines.same_input(machine, stack.item, other));
        }
        if left > 0 && !node.state.bin.is_empty() && machine.bin_takes(stack.item) {
            let left = fill(&mut node.state.bin, stack.item, stack.wear, left);
            return (left > 0).then_some(ItemStack { count: left, ..stack });
        }
        if left < stack.count {
            // Picks up the work (or lights the fuel) now, so whoever put it in sees it start;
            // the next step says whether it has the power.
            prepare(node, machine, machines);
            let state = &mut node.state;
            if matches!(state.status, Status::Idle | Status::NoFuel) {
                if state.process.is_some() && state.condition <= 0.0 {
                    // What the world does not give it yet: water, or the structure round it.
                    let water = matches!(machine.role, Role::Processor { water: true, .. });
                    state.status = if water { Status::NoSource } else { Status::Incomplete };
                } else if state.process.is_some() {
                    state.status = Status::Working;
                } else if state.burning > 0.0 {
                    state.status = Status::Generating;
                }
            }
        }
        (left > 0).then_some(ItemStack { count: left, ..stack })
    }

    /// Whether the machine at `pos` would take `item` at all, room or not: something it
    /// burns or works, or for a bin, anything it holds.
    pub fn accepts(&self, pos: IVec3, item: BlockId, machines: &MachineRegistry) -> bool {
        let Some(machine) = self.nodes.get(&pos).and_then(|node| machines.get(node.block)) else { return false };
        (machine.burns() && machines.burns_in(machine, item).is_some()) || (machine.is_processor() && machines.takes(machine, item)) || machine.bin_takes(item)
    }

    /// Takes everything out of the machine's outputs, or out of a bin; and what went in,
    /// while it has nothing it can work (half a recipe, or the wrong things).
    pub fn take_outputs(&mut self, pos: IVec3) -> Vec<ItemStack> {
        let mut taken = Vec::new();
        self.take_outputs_into(pos, |stack| {
            taken.push(stack);
            0
        });
        taken
    }

    /// As [`Self::take_outputs`], a stack at a time to `take`, which says how many of it it
    /// could not carry: those stay in the slot they came from.
    pub fn take_outputs_into(&mut self, pos: IVec3, mut take: impl FnMut(ItemStack) -> u8) {
        let Some(node) = self.nodes.get_mut(&pos) else { return };
        let state = &mut node.state;
        // Inputs come out only of a machine with nothing to do: a full one keeps them for
        // when its outputs are taken.
        let idle = state.process.is_none() && state.status != Status::Full;
        let inputs = state.inputs.iter_mut().filter(|_| idle);
        for slot in state.outputs.iter_mut().chain(state.bin.iter_mut()).chain(inputs) {
            if let Some(stack) = slot.take() {
                let left = take(stack);
                if left > 0 {
                    *slot = Some(ItemStack { count: left, ..stack });
                }
            }
        }
    }

    /// The network a port of the machine at `pos` is on, as it looks.
    pub fn grid(&self, pos: IVec3, port: usize) -> Grid {
        let Some(&network) = self.joined.get(&pos).and_then(|ports| ports.get(port)) else { return Grid::default() };
        let network = &self.networks[network];
        let mut grid = Grid { carrier: Some(network.carrier), ..Grid::default() };
        // A machine broken since the networks were last joined is skipped until they are.
        for member in &network.members {
            let Some(node) = self.nodes.get(&member.pos) else { continue };
            let Some(&port) = node.ports.get(member.port) else { continue };
            match port.role {
                PortRole::Gives(power) => {
                    grid.tier = grid.tier.max(port.tier);
                    grid.supply += power * offer_share(node);
                }
                PortRole::Holds { .. } => grid.tier = grid.tier.max(port.tier),
                PortRole::Takes(power) => grid.demand += power * ask_share(node, port),
                PortRole::Carries => {}
            }
        }
        grid
    }

    /// What a player sees of the machine at `pos`.
    pub fn view(&self, pos: IVec3, machines: &MachineRegistry) -> Option<MachineView> {
        let node = self.nodes.get(&pos)?;
        let machine = machines.get(node.block)?;
        let state = &node.state;
        let process = state.process.map(|p| machines.process(p));
        let progress = process.map_or(0, |p| (100.0 * state.progress / p.seconds).clamp(0.0, 100.0) as u8);
        let power = match machine.role {
            Role::Generator { .. } | Role::Transformer { .. } => state.load,
            _ => state.supplied,
        };
        let (capacity, energy) = match machine.role {
            Role::Store { capacity, .. } => (capacity as u32, state.energy as u32),
            _ => (0, 0),
        };
        // The network it gives to, or takes from: its first port's.
        let grid = if node.ports.is_empty() { Grid::default() } else { self.grid(pos, 0) };
        let held = state.bin.iter().flatten().map(|s| s.count as u32).sum();
        // Its run's rate, or its own until the runs are next joined.
        let run = self.run_of.get(&pos).filter(|_| !self.dirty).map(|&r| self.runs[r].rate);
        let moves = run.or(node.chute).unwrap_or(0.0);
        Some(MachineView {
            block: node.block,
            status: state.status,
            inputs: state.inputs.clone(),
            outputs: state.outputs.clone(),
            fuel: state.fuel,
            progress,
            making: process.map(|p| p.outputs[0].0),
            power: (power.clamp(0.0, 1.0) * 100.0).round() as u8,
            energy,
            capacity,
            grid,
            held,
            moves,
        })
    }

    /// Runs every machine for `ticks` server ticks, `per_second` of them a second (power is
    /// counted in units a tick, work in seconds).
    pub fn step(&mut self, ticks: u32, per_second: f32, machines: &MachineRegistry) {
        if self.dirty {
            self.connect();
        }
        let seconds = ticks as f32 / per_second;
        let ticks = ticks as f32;
        // What each machine asks of its networks, or offers them.
        for pos in &self.order {
            let node = self.nodes.get_mut(pos).expect("ordered machines exist");
            let Some(machine) = machines.get(node.block) else { continue };
            prepare(node, machine, machines);
        }
        // Each network shares out what it has.
        let mut given: FxHashMap<(IVec3, usize), Result<f32, Status>> = FxHashMap::default();
        for network in &mut self.networks {
            let nodes = &self.nodes;
            let port = |m: &Member| (&nodes[&m.pos], nodes[&m.pos].ports[m.port]);
            // A network's tier: its highest generator's or store's.
            let tier = network.members.iter().map(port).filter(|(_, p)| matches!(p.role, PortRole::Gives(_) | PortRole::Holds { .. })).map(|(_, p)| p.tier).max().unwrap_or(0);
            let thinnest = network.members.iter().map(port).filter(|(_, p)| p.role == PortRole::Carries).map(|(_, p)| p.tier).min();
            let overloaded = network.carrier.tiered() && thinnest.is_some_and(|thin| thin < tier);
            let refuses = |p: &Port| network.carrier.tiered() && p.tier < tier;
            let (mut supply, mut stored, mut demand, mut room) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for member in &network.members {
                let (node, p) = port(member);
                match p.role {
                    PortRole::Gives(power) if !overloaded => supply += power * offer_share(node),
                    PortRole::Takes(power) if !overloaded && !refuses(&p) => demand += power * ask_share(node, p),
                    PortRole::Holds { capacity, rate } if !overloaded => {
                        stored += (node.state.energy / ticks).min(rate);
                        room += ((capacity - node.state.energy) / ticks).clamp(0.0, rate);
                    }
                    _ => {}
                }
            }
            let fed = supply.min(demand);
            let drawn = stored.min(demand - fed);
            let share = if demand > 0.0 { (fed + drawn) / demand } else { 1.0 };
            let charged = (supply - fed).min(room);
            let load = if supply > 0.0 { (fed + charged) / supply } else { 0.0 };
            // Wanting more: machines short of power, or stores with room left.
            network.short = demand - fed - drawn + room - charged > 1e-4;
            for member in &network.members {
                let (node, p) = port(member);
                let amount = match p.role {
                    _ if overloaded => Err(Status::Overloaded),
                    PortRole::Gives(_) => Ok(load),
                    PortRole::Takes(_) if refuses(&p) => Err(Status::TooHigh),
                    // The share of its rated draw it got.
                    PortRole::Takes(_) => Ok(share * ask_share(node, p)),
                    PortRole::Holds { capacity, rate } => {
                        let out = if stored > 0.0 { (node.state.energy / ticks).min(rate) * drawn / stored } else { 0.0 };
                        let into = if room > 0.0 { ((capacity - node.state.energy) / ticks).clamp(0.0, rate) * charged / room } else { 0.0 };
                        Ok((into - out) * ticks)
                    }
                    PortRole::Carries => continue,
                };
                given.insert((member.pos, member.port), amount);
            }
        }
        // Each machine works with what it got.
        for &pos in &self.order {
            let short = |port: usize| self.joined.get(&pos).and_then(|n| n.get(port)).is_some_and(|&n| self.networks[n].short);
            let shorts: Vec<bool> = (0..self.nodes[&pos].ports.len()).map(short).collect();
            let node = self.nodes.get_mut(&pos).expect("ordered machines exist");
            let Some(machine) = machines.get(node.block) else { continue };
            let got = |port: usize| given.get(&(pos, port)).copied();
            work(pos, node, machine, machines, seconds, got, &shorts);
        }
        self.carry(seconds, machines);
    }

    /// Moves items along every run of chutes: what a hopper holds or a processor made, to
    /// the machines on the run that work or burn it, in turn; what a processor made that
    /// none takes, to a chest.
    fn carry(&mut self, seconds: f32, machines: &MachineRegistry) {
        for r in 0..self.runs.len() {
            let run = &mut self.runs[r];
            // No more than `chute_burst` seconds' worth built up, so an idle run does not burst.
            run.credit = (run.credit + run.rate * seconds).min((run.rate * machines.rules.chute_burst).max(1.0));
            let mut budget = run.credit.floor() as u32;
            if budget == 0 {
                continue;
            }
            let (ends, mut next) = (std::mem::take(&mut run.ends), run.next);
            let role = |nodes: &FxHashMap<IVec3, Node>, pos: &IVec3| nodes.get(pos).and_then(|n| machines.get(n.block));
            let mut moved = 0;
            for &from in &ends {
                let Some(giver) = role(&self.nodes, &from) else { continue };
                let hopper = matches!(giver.role, Role::Bin { feeds: true, .. });
                if !hopper && !giver.is_processor() {
                    continue;
                }
                let slots = if hopper { self.nodes[&from].state.bin.len() } else { self.nodes[&from].state.outputs.len() };
                for slot in 0..slots {
                    let state = &self.nodes[&from].state;
                    let Some(stack) = (if hopper { state.bin[slot] } else { state.outputs[slot] }) else { continue };
                    if budget == 0 {
                        break;
                    }
                    let count = stack.count.min(budget.min(u8::MAX as u32) as u8);
                    let mut left = count;
                    // First to machines that work or burn it, then (from a processor) to chests.
                    for chests in [false, true] {
                        if chests && hopper {
                            break;
                        }
                        for k in 0..ends.len() {
                            let to = ends[(next + k) % ends.len()];
                            let takes = role(&self.nodes, &to).is_some_and(|m| match m.role {
                                Role::Bin { feeds, .. } => chests && !feeds,
                                _ => !chests && (m.is_processor() || m.burns()),
                            });
                            if left == 0 || to == from || !takes {
                                continue;
                            }
                            let before = left;
                            left = self.put(to, ItemStack { count: left, ..stack }, machines).map_or(0, |s| s.count);
                            if left < before {
                                next = (next + k + 1) % ends.len();
                            }
                        }
                    }
                    let gone = count - left;
                    if gone > 0 {
                        let state = &mut self.nodes.get_mut(&from).expect("ends are machines").state;
                        let slot = if hopper { &mut state.bin[slot] } else { &mut state.outputs[slot] };
                        *slot = (stack.count > gone).then_some(ItemStack { count: stack.count - gone, ..stack });
                        budget -= gone as u32;
                        moved += gone as u32;
                    }
                }
            }
            let run = &mut self.runs[r];
            run.ends = ends;
            run.next = next;
            run.credit -= moved as f32;
        }
    }

    /// Joins machines into networks.
    fn connect(&mut self) {
        self.dirty = false;
        self.order = self.nodes.keys().copied().collect();
        self.order.sort_by_key(|p| (p.x, p.y, p.z));
        self.networks.clear();
        self.joined = self.nodes.iter().map(|(&pos, n)| (pos, vec![usize::MAX; n.ports.len()])).collect();
        for &start in &self.order {
            for start_port in 0..self.nodes[&start].ports.len() {
                if self.joined[&start][start_port] != usize::MAX {
                    continue;
                }
                let id = self.networks.len();
                let carrier = self.nodes[&start].ports[start_port].carrier;
                let mut members = Vec::new();
                let mut stack = vec![Member { pos: start, port: start_port }];
                self.joined.get_mut(&start).unwrap()[start_port] = id;
                while let Some(member) = stack.pop() {
                    members.push(member);
                    let here = self.nodes[&member.pos].ports[member.port];
                    for face in Face::ALL {
                        if here.faces & (1 << face as u8) == 0 {
                            continue;
                        }
                        let next = member.pos + face.normal();
                        let Some(node) = self.nodes.get(&next) else { continue };
                        let back = 1 << face.opposite() as u8;
                        for (i, there) in node.ports.iter().enumerate() {
                            // Two ports that both only give or both only take have nothing
                            // to say to each other, but still join: power passes through
                            // neither, so it changes nothing, and keeps networks simple.
                            if there.carrier == carrier && there.faces & back != 0 && self.joined[&next][i] == usize::MAX {
                                self.joined.get_mut(&next).unwrap()[i] = id;
                                stack.push(Member { pos: next, port: i });
                            }
                        }
                    }
                }
                members.sort_by_key(|m| (m.pos.x, m.pos.y, m.pos.z, m.port));
                self.networks.push(Network { carrier, members, short: false });
            }
        }
        // Runs of chutes, and the machines beside them.
        self.runs.clear();
        self.run_of.clear();
        for &start in &self.order {
            let Some(rate) = self.nodes[&start].chute else { continue };
            if self.run_of.contains_key(&start) {
                continue;
            }
            let id = self.runs.len();
            let mut run = Run { ends: Vec::new(), rate, credit: 0.0, next: 0 };
            let mut stack = vec![start];
            self.run_of.insert(start, id);
            while let Some(pos) = stack.pop() {
                for face in Face::ALL {
                    let next = pos + face.normal();
                    let Some(node) = self.nodes.get(&next) else { continue };
                    match node.chute {
                        Some(rate) if !self.run_of.contains_key(&next) => {
                            run.rate = run.rate.min(rate);
                            self.run_of.insert(next, id);
                            stack.push(next);
                        }
                        Some(_) => {}
                        None if !run.ends.contains(&next) => run.ends.push(next),
                        None => {}
                    }
                }
            }
            run.ends.sort_by_key(|p| (p.x, p.y, p.z));
            self.runs.push(run);
        }
    }
}

/// The share of its rated output a machine offers this step: what the world lets it give,
/// or for a converter or transformer (two ports), the share of its input it got a step
/// before.
fn offers(ports: &[Port], state: &MachineState) -> f32 {
    match ports.first().map(|p| p.role) {
        Some(PortRole::Gives(_)) if ports.len() > 1 => state.supplied,
        Some(PortRole::Gives(_)) => state.condition,
        _ => 0.0,
    }
}

fn offer_share(node: &Node) -> f32 {
    offers(&node.ports, &node.state)
}

/// The share of its rated draw a taking port asks this step.
fn ask_share(node: &Node, port: Port) -> f32 {
    let state = &node.state;
    let gives = node.ports.iter().any(|p| matches!(p.role, PortRole::Gives(_)));
    match port.role {
        // A converter asks for what was used of it a step before.
        PortRole::Takes(_) if gives => state.load,
        // A processor asks only while it has work.
        PortRole::Takes(_) if state.process.is_some() && state.condition > 0.0 && state.status != Status::Full => 1.0,
        _ => 0.0,
    }
}

/// Before the networks share out: a processor picks its work, a generator lights its fuel
/// and says how much it can give (`condition`).
fn prepare(node: &mut Node, machine: &Machine, machines: &MachineRegistry) {
    let state = &mut node.state;
    match &machine.role {
        Role::Processor { tier, extra, .. } => {
            if state.process.is_none() {
                state.progress = 0.0;
                let possible = |p: &ProcessId| {
                    let process = machines.process(*p);
                    process.tier <= *tier && inputs_hold(&state.inputs, process)
                };
                state.process = machine.processes.iter().copied().filter(possible).find(|&p| outputs_fit(&state.outputs, machines.process(p), *extra));
                // Work it could do but has no room for: full, not idle, until outputs are taken.
                if state.process.is_none() {
                    let full = machine.processes.iter().any(possible);
                    if full {
                        state.status = Status::Full;
                    } else if state.status == Status::Full {
                        state.status = Status::Idle;
                    }
                }
            }
            // What it holds may have changed: the work it picked must still be possible.
            if let Some(p) = state.process {
                let process = machines.process(p);
                if !inputs_hold(&state.inputs, process) {
                    state.process = None;
                    state.progress = 0.0;
                } else if !outputs_fit(&state.outputs, process, *extra) {
                    state.status = Status::Full;
                } else if state.status == Status::Full {
                    state.status = Status::Idle;
                }
            }
        }
        Role::Generator { source: Source::Fuel, .. } => {
            if state.burning <= 0.0 {
                light(state, machines);
            }
            state.condition = if state.burning > 0.0 { 1.0 } else { 0.0 };
        }
        // A converter's output follows its input; the world sets the rest.
        _ => {}
    }
}

/// Burns the next fuel, if there is any.
fn light(state: &mut MachineState, machines: &MachineRegistry) -> bool {
    let Some(stack) = state.fuel else { return false };
    let Some(seconds) = machines.burn_seconds(stack.item) else { return false };
    state.fuel = (stack.count > 1).then_some(ItemStack { count: stack.count - 1, ..stack });
    state.burning += seconds;
    true
}

/// After the networks share out: what each machine does with what it got. `got(port)` is
/// the share of its rated draw a taking port got, the share of a giving port's offer that
/// was used, or the power a store took (negative: gave) over the step; or why its network
/// refused it. `short[port]` says whether the port's network wanted more.
fn work(
    pos: IVec3,
    node: &mut Node,
    machine: &Machine,
    machines: &MachineRegistry,
    seconds: f32,
    got: impl Fn(usize) -> Option<Result<f32, Status>>,
    short: &[bool],
) {
    let ports = &node.ports;
    let state = &mut node.state;
    let value = |port: usize| got(port).and_then(Result::ok).unwrap_or(0.0);
    let refused = |port: usize| got(port).and_then(Result::err);
    match &machine.role {
        Role::Conduit { .. } => state.status = refused(0).unwrap_or(Status::Idle),
        Role::Chute { .. } | Role::Bin { .. } => state.status = Status::Idle,
        Role::Store { .. } => match refused(0) {
            Some(status) => state.status = status,
            None => {
                let change = value(0);
                state.energy = (state.energy + change).max(0.0);
                state.status = if change < 0.0 { Status::Generating } else if change > 0.0 { Status::Working } else { Status::Idle };
            }
        },
        Role::Generator { .. } | Role::Transformer { .. } => {
            if let Some(status) = refused(0) {
                state.status = status;
                state.load = 0.0;
                state.supplied = 0.0;
                return;
            }
            let load = value(0);
            // How much its network used of what it offered; with nothing offered but the
            // network wanting more, it asks for its input in full.
            let offered = offers(ports, state);
            state.load = if offered > 0.0 { load } else if short.first().copied().unwrap_or(false) { 1.0 } else { 0.0 };
            let used = load * offered;
            match source_of(machine) {
                Some(Source::Fuel) => {
                    state.burning = (state.burning - seconds * used).max(0.0);
                    state.status = match () {
                        _ if state.condition <= 0.0 && state.load > 0.0 => Status::NoFuel,
                        _ if used > 0.0 => Status::Generating,
                        _ => Status::Idle,
                    };
                }
                Some(Source::FlowingWater | Source::Wind | Source::Walked) => {
                    state.status = if state.condition <= 0.0 { Status::NoSource } else if used > 0.0 { Status::Generating } else { Status::Idle };
                }
                // A converter whose structure is not standing (a turbine hall's) neither takes
                // nor gives.
                _ if state.condition <= 0.0 => {
                    state.status = Status::Incomplete;
                    state.load = 0.0;
                    state.supplied = 0.0;
                }
                // A converter or transformer: the share of its input it got, it gives next step.
                _ => {
                    state.supplied = value(1).max(0.0);
                    state.status = match refused(1) {
                        Some(status) => status,
                        None if used > 0.0 => Status::Generating,
                        None if state.load > 0.0 => Status::NoPower,
                        None => Status::Idle,
                    };
                }
            }
        }
        Role::Processor { speed, extra, burns, water, .. } => {
            let Some(process) = state.process.map(|p| machines.process(p)) else {
                if state.status != Status::Full {
                    state.status = Status::Idle;
                }
                state.supplied = 0.0;
                return;
            };
            if state.status == Status::Full && !outputs_fit(&state.outputs, process, *extra) {
                return;
            }
            if state.condition <= 0.0 {
                // What the world did not give it: water, or the structure round it.
                state.status = if *water { Status::NoSource } else { Status::Incomplete };
                state.supplied = 0.0;
                return;
            }
            let rate = if *burns {
                if state.burning <= 0.0 && !light(state, machines) {
                    state.status = Status::NoFuel;
                    return;
                }
                state.burning = (state.burning - seconds).max(0.0);
                1.0
            } else if ports.is_empty() {
                1.0
            } else {
                if let Some(status) = refused(0) {
                    state.status = status;
                    state.supplied = 0.0;
                    return;
                }
                state.supplied = value(0);
                state.supplied
            };
            if rate <= 0.0 {
                state.status = Status::NoPower;
                return;
            }
            state.status = Status::Working;
            state.progress += seconds * speed * rate;
            // A fast machine may finish the same work more than once a step: what is left
            // over goes on to the next while it has the inputs, and waits, kept, while its
            // outputs have no room for it.
            let id = state.process;
            while state.progress >= process.seconds {
                let left = state.progress - process.seconds;
                finish(pos, state, process, *extra);
                if left <= 0.0 || !inputs_hold(&state.inputs, process) {
                    break;
                }
                state.process = id;
                state.progress = left;
                if !outputs_fit(&state.outputs, process, *extra) {
                    state.status = Status::Full;
                    break;
                }
            }
        }
    }
}

fn source_of(machine: &Machine) -> Option<Source> {
    match machine.role {
        Role::Generator { source, .. } => Some(source),
        _ => None,
    }
}

/// Uses up the inputs of a finished recipe and adds its outputs, with the machine's yield.
fn finish(pos: IVec3, state: &mut MachineState, process: &crate::machine::Process, extra: f32) {
    for (accepts, count) in &process.inputs {
        let mut left = *count;
        for slot in state.inputs.iter_mut() {
            if let Some(stack) = slot.filter(|s| accepts.binary_search(&s.item).is_ok()) {
                let taken = left.min(stack.count);
                left -= taken;
                *slot = (stack.count > taken).then_some(ItemStack { count: stack.count - taken, ..stack });
            }
        }
    }
    let mut roll = roll(pos, state.made);
    for &(item, count) in &process.outputs {
        let made = count as f32 * (1.0 + extra);
        let more = if roll.next() < made.fract() { 1 } else { 0 };
        fill(&mut state.outputs, item, 0, made.floor() as u8 + more);
    }
    for &(item, count, chance) in &process.byproducts {
        if roll.next() < chance {
            fill(&mut state.outputs, item, 0, count);
        }
    }
    state.made = state.made.wrapping_add(1);
    state.process = None;
    state.progress = 0.0;
}

/// Puts `count` of `item` worn by `wear` into `slots`, topping up its stacks of the same
/// wear first, while no more than `room` stacks hold it or what goes in `same` as it;
/// returns how many did not fit. Wear goes with it: a worn tool stays worn.
fn fill_within(slots: &mut [Option<ItemStack>], item: BlockId, wear: u16, mut count: u8, room: usize, same: impl Fn(BlockId) -> bool) -> u8 {
    for stack in slots.iter_mut().flatten().filter(|s| s.item == item && s.wear == wear) {
        let moved = count.min(max_stack(item).saturating_sub(stack.count));
        stack.count += moved;
        count -= moved;
    }
    let held = slots.iter().flatten().filter(|s| same(s.item)).count();
    for slot in slots.iter_mut().filter(|s| s.is_none()).take(room.saturating_sub(held)) {
        if count == 0 {
            break;
        }
        let moved = count.min(max_stack(item));
        *slot = Some(ItemStack { wear, ..ItemStack::new(item, moved) });
        count -= moved;
    }
    count
}

/// Puts `count` of `item` worn by `wear` into `slots`, topping up stacks of the same wear
/// first; returns how many did not fit.
fn fill(slots: &mut [Option<ItemStack>], item: BlockId, wear: u16, mut count: u8) -> u8 {
    for stack in slots.iter_mut().flatten().filter(|s| s.item == item && s.wear == wear) {
        let moved = count.min(max_stack(item).saturating_sub(stack.count));
        stack.count += moved;
        count -= moved;
    }
    for slot in slots.iter_mut().filter(|s| s.is_none()) {
        if count == 0 {
            break;
        }
        let moved = count.min(max_stack(item));
        *slot = Some(ItemStack { wear, ..ItemStack::new(item, moved) });
        count -= moved;
    }
    count
}

/// Whether the inputs hold all a recipe takes, each item counted once.
pub fn inputs_hold(inputs: &[Option<ItemStack>], process: &crate::machine::Process) -> bool {
    let mut left: Vec<Option<ItemStack>> = inputs.to_vec();
    process.inputs.iter().all(|(accepts, count)| {
        let mut need = *count;
        for slot in left.iter_mut() {
            if let Some(stack) = slot.as_mut().filter(|s| accepts.binary_search(&s.item).is_ok()) {
                let taken = need.min(stack.count);
                need -= taken;
                stack.count -= taken;
            }
        }
        need == 0
    })
}

/// Whether the outputs have room for the most a recipe could make.
fn outputs_fit(outputs: &[Option<ItemStack>], process: &crate::machine::Process, extra: f32) -> bool {
    let mut slots = outputs.to_vec();
    let most = |count: u8| (count as f32 * (1.0 + extra)).ceil() as u8;
    process.outputs.iter().map(|&(item, count)| (item, most(count))).chain(process.byproducts.iter().map(|&(item, count, _)| (item, count))).all(|(item, count)| fill(&mut slots, item, 0, count) == 0)
}

/// A machine's rolls: the same for the same machine and the same thing made, so a world
/// plays out alike.
struct Roll(u64);

fn roll(pos: IVec3, made: u32) -> Roll {
    let seed = (pos.x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (pos.y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f) ^ (pos.z as u64).wrapping_mul(0x1656_67b1_9e37_79f9) ^ made as u64;
    Roll(seed)
}

impl Roll {
    /// The next roll, from 0 to 1 (splitmix64).
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockDef, BlockRegistry};

    struct Test {
        blocks: BlockRegistry,
        machines: MachineRegistry,
    }

    impl Test {
        fn id(&self, name: &str) -> BlockId {
            self.blocks.id(&format!("t:{name}")).unwrap()
        }
    }

    fn test() -> Test {
        let names = [
            "ore", "dust", "log", "wheel", "shaft", "mill", "boiler", "pipe", "engine", "dynamo", "lv_cable", "mv_cable", "crusher",
            "battery", "mv_battery", "transformer", "furnace", "ingot", "washer", "chute", "conveyor", "hopper", "chest", "kiln",
        ];
        let blocks = BlockRegistry::new(names.iter().map(|n| BlockDef { name: format!("t:{n}"), tags: if *n == "log" { vec!["log".into()] } else { vec![] }, ..Default::default() }).collect()).unwrap();
        let machines = MachineRegistry::read(
            "t",
            r#"(machines: [
                (block: "wheel", role: Generator(carrier: Rotation, power: 8.0, source: FlowingWater)),
                (block: "shaft", role: Conduit(carrier: Rotation)),
                (block: "mill", role: Processor(kinds: ["crush"], power: Some((Rotation, 4.0)))),
                (block: "boiler", role: Generator(carrier: Steam, power: 20.0, source: Fuel)),
                (block: "pipe", role: Conduit(carrier: Steam)),
                (block: "engine", role: Generator(carrier: Rotation, power: 10.0, source: Converts(from: Steam, ratio: 2.0))),
                (block: "dynamo", role: Generator(carrier: Electric, tier: 1, power: 8.0, source: Converts(from: Rotation, ratio: 1.25))),
                (block: "lv_cable", role: Conduit(carrier: Electric, tier: 1)),
                (block: "mv_cable", role: Conduit(carrier: Electric, tier: 2)),
                (block: "crusher", role: Processor(kinds: ["crush"], speed: 2.0, yield: 1.0, power: Some((Electric, 8.0)), tier: 1)),
                (block: "battery", role: Store(carrier: Electric, tier: 1, capacity: 1000.0, rate: 32.0)),
                (block: "mv_battery", role: Store(carrier: Electric, tier: 2, capacity: 100000.0, rate: 512.0)),
                (block: "transformer", role: Transformer(tier: 2, rate: 128.0)),
                (block: "furnace", role: Processor(kinds: ["smelt"], burns: true)),
                (block: "washer", role: Processor(kinds: ["crush"], speed: 0.5, yield: 2.0, power: Some((Rotation, 4.0)), water: true)),
                (block: "chute", role: Chute(rate: 1.0)),
                (block: "conveyor", role: Chute(rate: 4.0)),
                (block: "hopper", role: Bin(slots: 3, feeds: true)),
                (block: "chest", role: Bin(slots: 3)),
                (block: "kiln", role: Processor(kinds: ["fire"])),
            ], fuels: [(item: "tag:log", seconds: 10.0)])"#,
        )
        .unwrap();
        let processes = MachineRegistry::read_processes(
            "t",
            r#"(recipes: [
                (kind: "crush", inputs: [("ore", 1)], outputs: [("dust", 1)], seconds: 4.0),
                (kind: "smelt", inputs: [("dust", 2)], outputs: [("ingot", 1)], seconds: 2.0),
                (kind: "fire", inputs: [("ore", 2), ("log", 1)], outputs: [("ingot", 1)], seconds: 2.0),
            ])"#,
        )
        .unwrap();
        let machines = MachineRegistry::new(vec![machines], vec![processes], &blocks).unwrap();
        Test { blocks, machines }
    }

    fn build(t: &Test, factory: &mut Factory, row: &[&str]) {
        for (x, name) in row.iter().enumerate() {
            assert!(factory.insert(IVec3::new(x as i32, 0, 0), t.id(name), &t.machines), "{name} is a machine");
        }
    }

    fn feed(t: &Test, factory: &mut Factory, at: i32, item: &str, count: u8) {
        assert_eq!(factory.put(IVec3::new(at, 0, 0), ItemStack::new(t.id(item), count), &t.machines), None);
    }

    fn run(t: &Test, factory: &mut Factory, seconds: f32) {
        for _ in 0..(seconds * 5.0) as u32 {
            factory.step(4, 20.0, &t.machines);
        }
    }

    fn made(t: &Test, factory: &Factory, at: i32, item: &str) -> u32 {
        let (_, state) = factory.get(IVec3::new(at, 0, 0)).unwrap();
        state.outputs.iter().flatten().filter(|s| s.item == t.id(item)).map(|s| s.count as u32).sum()
    }

    #[test]
    fn a_washer_needs_water_and_gives_three_dust_an_ore() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["wheel", "washer"]);
        factory.set_condition(IVec3::ZERO, 1.0);
        factory.set_condition(IVec3::X, 0.0);
        feed(&t, &mut factory, 1, "ore", 2);
        // Dry, it says so as the ore goes in, not that it works.
        assert_eq!(factory.get(IVec3::X).unwrap().1.status, Status::NoSource);
        run(&t, &mut factory, 10.0);
        assert_eq!(factory.get(IVec3::X).unwrap().1.status, Status::NoSource);
        assert_eq!(made(&t, &factory, 1, "dust"), 0);
        // Water beside it: half as fast as the recipe, three dust for each ore.
        factory.set_condition(IVec3::X, 1.0);
        run(&t, &mut factory, 9.0);
        assert_eq!(made(&t, &factory, 1, "dust"), 3);
        run(&t, &mut factory, 9.0);
        assert_eq!(made(&t, &factory, 1, "dust"), 6);
    }

    #[test]
    fn a_machine_beside_a_broken_shaft_can_be_looked_at_before_the_networks_are_joined_again() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["wheel", "shaft", "shaft", "mill"]);
        factory.set_condition(IVec3::ZERO, 1.0);
        feed(&t, &mut factory, 3, "ore", 1);
        run(&t, &mut factory, 1.0);
        factory.remove(IVec3::X);
        for x in [0, 2, 3] {
            assert!(factory.view(IVec3::new(x, 0, 0), &t.machines).is_some());
        }
        run(&t, &mut factory, 1.0);
        assert_eq!(factory.view(IVec3::new(2, 0, 0), &t.machines).unwrap().grid.supply, 0.0, "the wheel no longer reaches it");
    }

    #[test]
    fn an_animals_work_at_a_wheel_adds_up_until_it_rests_or_is_gone() {
        let mut factory = Factory::default();
        factory.add_walked(7, 1.0);
        factory.add_walked(7, 2.5);
        factory.add_walked(9, 1.0);
        assert_eq!(factory.walked(7), 3.5);
        assert_eq!(factory.rested(7), 3.5);
        assert_eq!(factory.walked(7), 0.0);
        factory.retain_walked(|id| id != 9);
        assert_eq!(factory.walked(9), 0.0);
    }

    #[test]
    fn a_water_wheel_turns_a_mill_through_shafts_only_with_water_flowing() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["wheel", "shaft", "shaft", "mill"]);
        feed(&t, &mut factory, 3, "ore", 4);
        run(&t, &mut factory, 10.0);
        assert_eq!(made(&t, &factory, 3, "dust"), 0, "no water, no work");
        assert_eq!(factory.get(IVec3::ZERO).unwrap().1.status, Status::NoSource);
        assert_eq!(factory.get(IVec3::new(3, 0, 0)).unwrap().1.status, Status::NoPower);
        factory.set_condition(IVec3::ZERO, 1.0);
        run(&t, &mut factory, 8.2);
        assert_eq!(made(&t, &factory, 3, "dust"), 2, "two ores in eight seconds at speed 1");
        let view = factory.view(IVec3::ZERO, &t.machines).unwrap();
        assert_eq!((view.status, view.power), (Status::Generating, 50), "the mill takes half the wheel's 8");
    }

    #[test]
    fn a_gap_in_the_shaft_breaks_the_network() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["wheel", "shaft", "shaft", "mill"]);
        factory.set_condition(IVec3::ZERO, 1.0);
        factory.remove(IVec3::new(1, 0, 0));
        feed(&t, &mut factory, 3, "ore", 1);
        run(&t, &mut factory, 6.0);
        assert_eq!(made(&t, &factory, 3, "dust"), 0);
    }

    #[test]
    fn a_weak_network_slows_its_machines_rather_than_stopping_one() {
        let t = test();
        let mut factory = Factory::default();
        // A wheel at half flow gives 4: two mills that each want 4 share it, at half speed.
        build(&t, &mut factory, &["mill", "shaft", "wheel", "shaft", "mill"]);
        factory.set_condition(IVec3::new(2, 0, 0), 0.5);
        feed(&t, &mut factory, 0, "ore", 4);
        feed(&t, &mut factory, 4, "ore", 4);
        run(&t, &mut factory, 8.2);
        assert_eq!((made(&t, &factory, 0, "dust"), made(&t, &factory, 4, "dust")), (1, 1));
    }

    #[test]
    fn steam_turns_an_engine_that_turns_a_dynamo_that_runs_a_crusher_and_fuel_burns_as_used() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["boiler", "pipe", "engine", "shaft", "dynamo", "lv_cable", "crusher"]);
        factory.put(IVec3::ZERO, ItemStack::new(t.id("log"), 4), &t.machines);
        feed(&t, &mut factory, 6, "ore", 2);
        run(&t, &mut factory, 6.0);
        // Speed 2 crushes an ore in 2 seconds, and yield 1 doubles the dust.
        assert_eq!(made(&t, &factory, 6, "dust"), 4);
        // The crusher asks 8, the dynamo 10 rotation for that, the engine 20 steam: the
        // boiler at full output, a log every 10 seconds. One lit, three left.
        let (_, boiler) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(boiler.fuel.map(|f| f.count), Some(3));
        run(&t, &mut factory, 10.0);
        let (_, boiler) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(boiler.fuel.map(|f| f.count), Some(3), "with nothing to crush, nothing burns");
        assert_eq!(boiler.status, Status::Idle);
    }

    #[test]
    fn a_converter_whose_structure_is_down_neither_takes_nor_gives() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["boiler", "pipe", "engine", "shaft", "mill"]);
        factory.put(IVec3::ZERO, ItemStack::new(t.id("log"), 4), &t.machines);
        feed(&t, &mut factory, 4, "ore", 1);
        // An engine stands able, as a converter needs nothing of the world.
        assert_eq!(factory.get(IVec3::new(2, 0, 0)).unwrap().1.condition, 1.0);
        // Told its structure is down (a turbine hall's), it gives nothing and burns nothing.
        factory.set_condition(IVec3::new(2, 0, 0), 0.0);
        run(&t, &mut factory, 3.0);
        assert_eq!(factory.get(IVec3::new(2, 0, 0)).unwrap().1.status, Status::Incomplete);
        assert_eq!(made(&t, &factory, 4, "dust"), 0);
        assert_eq!(factory.get(IVec3::ZERO).unwrap().1.fuel.map(|f| f.count), Some(3), "one log lit, none burnt for nothing");
        // Standing again, it turns the mill.
        factory.set_condition(IVec3::new(2, 0, 0), 1.0);
        run(&t, &mut factory, 6.0);
        assert_eq!(made(&t, &factory, 4, "dust"), 1);
    }

    #[test]
    fn high_voltage_needs_a_transformer_and_a_thick_cable() {
        let t = test();
        let mut factory = Factory::default();
        // An MV battery straight onto an LV crusher: the crusher refuses it.
        build(&t, &mut factory, &["mv_battery", "mv_cable", "crusher"]);
        factory.state_mut(IVec3::ZERO).unwrap().energy = 50000.0;
        feed(&t, &mut factory, 2, "ore", 1);
        run(&t, &mut factory, 3.0);
        assert_eq!(factory.get(IVec3::new(2, 0, 0)).unwrap().1.status, Status::TooHigh);
        assert_eq!(made(&t, &factory, 2, "dust"), 0);
        // Through an LV cable, nothing at all: the cable is too thin.
        factory.remove(IVec3::new(1, 0, 0));
        factory.insert(IVec3::new(1, 0, 0), t.id("lv_cable"), &t.machines);
        run(&t, &mut factory, 1.0);
        assert_eq!(factory.get(IVec3::new(2, 0, 0)).unwrap().1.status, Status::Overloaded);
        // A transformer under the MV cable gives LV to the crusher beside it.
        let mut factory = Factory::default();
        build(&t, &mut factory, &["mv_battery", "mv_cable"]);
        factory.state_mut(IVec3::ZERO).unwrap().energy = 50000.0;
        factory.insert(IVec3::new(1, -1, 0), t.id("transformer"), &t.machines);
        factory.insert(IVec3::new(2, -1, 0), t.id("crusher"), &t.machines);
        factory.put(IVec3::new(2, -1, 0), ItemStack::new(t.id("ore"), 1), &t.machines);
        run(&t, &mut factory, 3.0);
        let (_, crusher) = factory.get(IVec3::new(2, -1, 0)).unwrap();
        assert_eq!(crusher.outputs[0].map(|s| s.count), Some(2));
        assert!(factory.get(IVec3::ZERO).unwrap().1.energy < 50000.0);
    }

    #[test]
    fn batteries_store_a_surplus_and_give_it_back() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["wheel", "dynamo", "lv_cable", "battery"]);
        factory.set_condition(IVec3::ZERO, 1.0);
        run(&t, &mut factory, 5.0);
        // The battery asks; the dynamo, once turning, gives 6.4 for the wheel's 8 rotation
        // (it wants 10 for its full 8).
        let stored = factory.get(IVec3::new(3, 0, 0)).unwrap().1.energy;
        assert!(stored > 500.0 && stored <= 800.0, "{stored}");
        // The wheel stops; a crusher now runs from the battery.
        factory.set_condition(IVec3::ZERO, 0.0);
        factory.insert(IVec3::new(4, 0, 0), t.id("crusher"), &t.machines);
        feed(&t, &mut factory, 4, "ore", 1);
        run(&t, &mut factory, 2.2);
        assert_eq!(made(&t, &factory, 4, "dust"), 2);
        assert!(factory.get(IVec3::new(3, 0, 0)).unwrap().1.energy < stored);
    }

    #[test]
    fn a_kiln_with_no_room_says_full_and_keeps_its_inputs_when_its_outputs_are_taken() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["kiln"]);
        let ingot = t.id("ingot");
        let stack = ItemStack::new(ingot, max_stack(ingot));
        factory.state_mut(IVec3::ZERO).unwrap().outputs.fill(Some(stack));
        feed(&t, &mut factory, 0, "ore", 2);
        feed(&t, &mut factory, 0, "log", 1);
        run(&t, &mut factory, 1.0);
        let (_, kiln) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(kiln.status, Status::Full, "work it could do but has no room for");
        // An empty hand takes what it made, not what it has yet to work.
        let taken = factory.take_outputs(IVec3::ZERO);
        assert!(taken.iter().all(|s| s.item == ingot), "{taken:?}");
        assert_eq!(taken.len(), 3);
        run(&t, &mut factory, 2.4);
        assert_eq!(made(&t, &factory, 0, "ingot"), 1, "with room again it fires what it kept");
        assert_eq!(factory.get(IVec3::ZERO).unwrap().1.status, Status::Idle);
    }

    #[test]
    fn a_furnace_burns_its_own_fuel_and_stops_when_full() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["furnace"]);
        feed(&t, &mut factory, 0, "log", 1);
        // Dust goes in as an input; two make an ingot in two seconds.
        assert_eq!(factory.put(IVec3::ZERO, ItemStack::new(t.id("dust"), 6), &t.machines), None);
        run(&t, &mut factory, 6.4);
        assert_eq!(made(&t, &factory, 0, "ingot"), 3);
        let (_, furnace) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(furnace.status, Status::Idle);
        assert!(furnace.burning > 3.0, "a log lasts ten seconds of work");
        assert_eq!(factory.take_outputs(IVec3::ZERO), vec![ItemStack::new(t.id("ingot"), 3)]);
    }

    #[test]
    fn a_machine_takes_only_what_it_uses() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["mill"]);
        let ingot = ItemStack::new(t.id("ingot"), 5);
        assert_eq!(factory.put(IVec3::ZERO, ingot, &t.machines), Some(ingot));
        let left = factory.put(IVec3::ZERO, ItemStack::new(t.id("ore"), 64), &t.machines);
        assert_eq!(left, None);
    }

    fn held(t: &Test, factory: &Factory, at: i32, item: &str) -> u32 {
        let (_, state) = factory.get(IVec3::new(at, 0, 0)).unwrap();
        state.bin.iter().chain(&state.inputs).flatten().filter(|s| s.item == t.id(item)).map(|s| s.count as u32).sum()
    }

    #[test]
    fn a_hopper_feeds_a_furnace_by_chute_and_a_crate_collects_what_it_made() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["hopper", "conveyor", "furnace", "chute", "chest"]);
        feed(&t, &mut factory, 0, "dust", 4);
        feed(&t, &mut factory, 0, "log", 1);
        assert_eq!(held(&t, &factory, 0, "dust"), 4, "a hopper takes anything");
        run(&t, &mut factory, 8.0);
        assert_eq!(held(&t, &factory, 0, "dust"), 0, "the furnace took the dust");
        let (_, furnace) = factory.get(IVec3::new(2, 0, 0)).unwrap();
        assert!(furnace.burning > 0.0 && furnace.fuel.is_none(), "and burns the log");
        assert_eq!(held(&t, &factory, 4, "ingot"), 2, "the chest on the far run holds what it made");
        assert_eq!(made(&t, &factory, 2, "ingot"), 0);
        // A chest's stacks come out like a machine's outputs.
        assert_eq!(factory.take_outputs(IVec3::new(4, 0, 0)), vec![ItemStack::new(t.id("ingot"), 2)]);
    }

    #[test]
    fn a_run_moves_as_fast_as_its_slowest_chute() {
        let t = test();
        let moved = |row: &[&str]| {
            let mut factory = Factory::default();
            build(&t, &mut factory, row);
            feed(&t, &mut factory, 0, "dust", 64);
            run(&t, &mut factory, 3.0);
            held(&t, &factory, row.len() as i32 - 1, "dust")
        };
        // No fuel: the furnace only gathers the dust.
        assert_eq!(moved(&["hopper", "chute", "furnace"]), 3);
        assert_eq!(moved(&["hopper", "conveyor", "furnace"]), 12);
        assert_eq!(moved(&["hopper", "conveyor", "chute", "furnace"]), 3);
    }

    /// What wears keeps its wear wherever it goes: into a bin beside a fresh one, into a
    /// machine's inputs, and down a chute; it never merges with a stack worn otherwise.
    #[test]
    fn wear_goes_with_a_stack_through_machines() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["hopper", "conveyor", "furnace", "conveyor", "chest"]);
        let (worn, fresh) = (ItemStack { wear: 7, ..ItemStack::new(t.id("dust"), 1) }, ItemStack::new(t.id("dust"), 1));
        let hopper = IVec3::ZERO;
        assert_eq!(factory.put(hopper, worn, &t.machines), None);
        assert_eq!(factory.put(hopper, fresh, &t.machines), None);
        assert_eq!(factory.get(hopper).unwrap().1.bin.iter().flatten().copied().collect::<Vec<_>>(), [worn, fresh], "kept apart");
        run(&t, &mut factory, 3.0);
        let (_, furnace) = factory.get(IVec3::new(2, 0, 0)).unwrap();
        let mut inputs: Vec<ItemStack> = furnace.inputs.iter().flatten().copied().collect();
        inputs.sort_by_key(|s| s.wear);
        assert_eq!(inputs, [fresh, worn], "down the chute as worn as it went in");
    }

    #[test]
    fn bins_never_feed_bins() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["hopper", "conveyor", "chest"]);
        feed(&t, &mut factory, 0, "ore", 10);
        run(&t, &mut factory, 4.0);
        assert_eq!(held(&t, &factory, 0, "ore"), 10);
        assert_eq!(held(&t, &factory, 2, "ore"), 0);
    }

    #[test]
    fn a_run_moves_items_without_making_or_losing_any() {
        let t = test();
        let mut factory = Factory::default();
        // Two hoppers and two unfired furnaces on one run, a chest beside: dust goes round
        // into the furnaces, and nothing else moves.
        build(&t, &mut factory, &["hopper", "conveyor", "furnace", "conveyor", "furnace", "conveyor", "hopper", "conveyor", "chest"]);
        feed(&t, &mut factory, 0, "dust", 50);
        feed(&t, &mut factory, 6, "dust", 30);
        feed(&t, &mut factory, 6, "ore", 7);
        let total = |factory: &Factory, item: &str| (0..9).map(|x| held(&t, factory, x, item) + made(&t, factory, x, item)).sum::<u32>();
        for _ in 0..40 {
            run(&t, &mut factory, 0.5);
            assert_eq!(total(&factory, "dust"), 80);
            assert_eq!(total(&factory, "ore"), 7);
        }
        assert_eq!(held(&t, &factory, 0, "dust") + held(&t, &factory, 6, "dust"), 0, "the furnaces took all the dust");
        assert_eq!(held(&t, &factory, 6, "ore"), 7, "nothing takes ore: it stays in its hopper");
        assert_eq!(held(&t, &factory, 8, "dust"), 0, "hoppers never feed the chest");
    }

    #[test]
    fn stacks_of_one_input_leave_room_for_the_other_and_an_idle_machine_gives_them_back() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["kiln"]);
        let ore = |count| ItemStack::new(t.id("ore"), count);
        // Three stacks of ore: the third finds no room, kept for the log.
        assert_eq!(factory.put(IVec3::ZERO, ore(64), &t.machines), None);
        assert_eq!(factory.put(IVec3::ZERO, ore(64), &t.machines), None);
        assert_eq!(factory.put(IVec3::ZERO, ore(64), &t.machines), Some(ore(64)));
        // Half a recipe: an empty hand takes the ore back out (what it cannot carry stays an
        // input, not a product), and it goes in again.
        factory.take_outputs_into(IVec3::ZERO, |stack| stack.count);
        let (_, state) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(state.inputs.iter().flatten().count(), 2);
        assert!(state.outputs.iter().all(Option::is_none));
        assert_eq!(factory.take_outputs(IVec3::ZERO), vec![ore(64), ore(64)]);
        assert_eq!(factory.put(IVec3::ZERO, ore(64), &t.machines), None);
        assert_eq!(factory.put(IVec3::ZERO, ore(64), &t.machines), None);
        feed(&t, &mut factory, 0, "log", 2);
        run(&t, &mut factory, 2.4);
        assert_eq!(made(&t, &factory, 0, "ingot"), 1);
        // Working on the next, it keeps what went in.
        assert_eq!(factory.take_outputs(IVec3::ZERO), vec![ItemStack::new(t.id("ingot"), 1)]);
    }

    /// The game's own bloom: a trip hammer beats it into three ingots (1.5 an ore, the
    /// ladder's second rung), the crafting grid into two.
    #[test]
    fn a_trip_hammer_beats_a_bloom_into_three_ingots_and_the_hand_into_two() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let mut factory = Factory::default();
        let (wheel, hammer) = (IVec3::ZERO, IVec3::X);
        assert!(factory.insert(wheel, id("base:water_wheel"), machines) && factory.insert(hammer, id("base:trip_hammer"), machines));
        factory.set_condition(wheel, 1.0);
        assert_eq!(factory.put(hammer, ItemStack::new(id("base:iron_bloom"), 1), machines), None);
        for _ in 0..100 {
            factory.step(4, 20.0, machines);
        }
        assert_eq!(factory.take_outputs(hammer), vec![ItemStack::new(id("base:iron_ingot"), 3)]);
        let by_hand: Vec<_> = content.recipes.iter().filter(|r| r.output == id("base:iron_ingot") && r.alternatives().into_iter().flatten().any(|a| a.contains(&id("base:iron_bloom")))).collect();
        assert_eq!(by_hand.len(), 1);
        assert_eq!(by_hand[0].count, 2);
        assert_eq!(by_hand[0].alternatives().into_iter().flatten().count(), 1, "one bloom");
    }

    /// The Iron age's machines from the game's content: a coke oven bakes coal by its own
    /// heat, a blast furnace blows two ore and a coke into four ingots (two an ore) and
    /// takes no dust, and a sawmill cuts a log into six planks of its wood on average.
    #[test]
    fn a_blast_furnace_makes_two_ingots_an_ore_from_ore_alone_and_a_sawmill_six_planks_a_log() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let stack = |name: &str, count| ItemStack::new(id(name), count);
        let mut factory = Factory::default();
        // The coke oven, standing (its structure's condition), with no power at all.
        let oven = IVec3::new(0, 0, 10);
        assert!(factory.insert(oven, id("base:coke_oven"), machines));
        factory.set_condition(oven, 1.0);
        assert_eq!(factory.put(oven, stack("base:coal_ore", 2), machines), None);
        for _ in 0..(65 * 5) {
            factory.step(4, 20.0, machines);
        }
        assert_eq!(factory.take_outputs(oven), vec![stack("base:coke", 2)]);
        // The blast furnace on a water wheel: two ore and a coke, four ingots.
        let (wheel, furnace) = (IVec3::ZERO, IVec3::X);
        assert!(factory.insert(wheel, id("base:water_wheel"), machines) && factory.insert(furnace, id("base:blast_furnace"), machines));
        factory.set_condition(wheel, 1.0);
        factory.set_condition(furnace, 1.0);
        let furnace_def = machines.get(id("base:blast_furnace")).unwrap();
        assert!(!machines.takes(furnace_def, id("base:iron_dust")), "a blast furnace takes no dust");
        assert_eq!(factory.put(furnace, stack("base:iron_ore", 4), machines), None);
        assert_eq!(factory.put(furnace, stack("base:coke", 2), machines), None);
        // A water wheel gives 8 of the 12 it draws: two blasts of 16 s take 48 s.
        for _ in 0..(55 * 5) {
            factory.step(4, 20.0, machines);
        }
        assert_eq!(factory.take_outputs(furnace), vec![stack("base:iron_ingot", 8)]);
        // The sawmill beside the wheel: each wood's own planks, half as many again as by hand.
        let saw = IVec3::NEG_X;
        assert!(factory.insert(saw, id("base:sawmill"), machines));
        let mut planks = 0;
        for _ in 0..10 {
            assert_eq!(factory.put(saw, stack("base:oak_log", 10), machines), None);
            for _ in 0..(25 * 5) {
                factory.step(4, 20.0, machines);
            }
            for s in factory.take_outputs(saw) {
                assert_eq!(s.item, id("base:oak_planks"), "a log makes planks of its own wood");
                planks += s.count as u32;
            }
        }
        assert!((560..=640).contains(&planks), "a hundred logs made {planks} planks, not about 600");
        let birch = machines.processes().filter(|(_, p)| p.kind == "saw").any(|(_, p)| p.outputs[0].0 == id("base:birch_planks"));
        assert!(birch, "every wood has its sawing recipe");
    }

    /// The HV processors from the game's content: a processing array crushes as sixteen
    /// macerators and an induction smelter smelts as eight electric furnaces, each for the
    /// same power a dust or an ingot as those, and the array only while its hall stands.
    #[test]
    fn hv_processors_are_many_machines_in_one_at_their_power_and_yield() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let stack = |name: &str, count| ItemStack::new(id(name), count);
        const FULL: f32 = 1_000_000.0;
        // A bank and cable of the machine's tier feeding it; what it made after `seconds`,
        // and the energy it took.
        let run_on_bank = |machine: &str, condition: f32, input: &str, count: u8, seconds: f32| {
            let (bank_block, cable_block) = match machines.get(id(machine)).map(|m| &m.role) {
                Some(Role::Processor { tier: 3, .. }) => ("base:hv_battery_bank", "base:hv_cable"),
                _ => ("base:battery_bank", "base:lv_cable"),
            };
            let mut factory = Factory::default();
            let (bank, cable, at) = (IVec3::ZERO, IVec3::X, IVec3::new(2, 0, 0));
            assert!(factory.insert(bank, id(bank_block), machines) && factory.insert(cable, id(cable_block), machines));
            assert!(factory.insert(at, id(machine), machines));
            factory.state_mut(bank).unwrap().energy = FULL;
            factory.set_condition(at, condition);
            assert_eq!(factory.put(at, stack(input, count), machines), None, "{machine} takes {count} {input}");
            for _ in 0..(seconds * 5.0).round() as u32 {
                factory.step(4, 20.0, machines);
            }
            let made: u32 = factory.take_outputs(at).iter().map(|s| s.count as u32).sum();
            (made, FULL - factory.get(bank).unwrap().1.energy, factory.get(at).unwrap().1.progress)
        };
        // The energy a recipe took: what was spent over the recipes done, counting the one
        // under way by how far it got.
        let per = |(made, energy, progress): (u32, f32, f32), each: u32, seconds: f32| energy / (made as f32 / each as f32 + progress / seconds);
        // Copper ore takes 6 s at speed 1: a macerator crushes one in 3 s, the array 32 in.
        let array = run_on_bank("base:processing_array", 1.0, "base:copper_ore", 64, 13.0);
        assert_eq!(array.0, 128, "64 ore in 12 s, two dust each");
        let macerator = run_on_bank("base:macerator", 1.0, "base:copper_ore", 64, 13.0);
        assert_eq!(macerator.0, 8, "a macerator crushes four ore in 13 s");
        // Each ore costs the array what it costs a macerator (timed while it still has ore,
        // since the step it runs out in is drawn in full).
        let (array_ore, macerator_ore) = (per(run_on_bank("base:processing_array", 1.0, "base:copper_ore", 64, 6.0), 2, 6.0), per(macerator, 2, 6.0));
        assert!((array_ore / macerator_ore - 1.0).abs() < 0.01, "an ore costs the array {array_ore}, a macerator {macerator_ore}");
        // Its hall not standing, the array does nothing.
        assert_eq!(run_on_bank("base:processing_array", 0.0, "base:copper_ore", 64, 13.0), (0, 0.0, 0.0));
        // Copper dust takes 4 s at speed 1: an electric furnace smelts one in 2 s, the
        // smelter 32 in 8 s.
        let smelter = run_on_bank("base:induction_smelter", 1.0, "base:copper_dust", 32, 9.0);
        assert_eq!(smelter.0, 32);
        let furnace = run_on_bank("base:electric_furnace", 1.0, "base:copper_dust", 32, 9.0);
        assert_eq!(furnace.0, 4, "an electric furnace smelts four in 9 s");
        let (smelter_ingot, furnace_ingot) = (per(smelter, 1, 4.0), per(furnace, 1, 4.0));
        assert!((smelter_ingot / furnace_ingot - 1.0).abs() < 0.01, "an ingot costs the smelter {smelter_ingot}, a furnace {furnace_ingot}");
    }

    /// Work done past a finished recipe in a step is kept while the outputs have no room
    /// for the next, and goes on once they are taken: a processing array (copper ore 6 s
    /// at speed 16, 3.2 s of work a fifth of a second) with room for one ore's dust.
    #[test]
    fn leftover_work_waits_while_the_outputs_are_full() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let id = |name: &str| blocks.id(name).unwrap_or_else(|| panic!("no {name}"));
        let mut factory = Factory::default();
        let (bank, cable, at) = (IVec3::ZERO, IVec3::X, IVec3::new(2, 0, 0));
        assert!(factory.insert(bank, id("base:hv_battery_bank"), machines) && factory.insert(cable, id("base:hv_cable"), machines));
        assert!(factory.insert(at, id("base:processing_array"), machines));
        factory.state_mut(bank).unwrap().energy = 1_000_000.0;
        factory.set_condition(at, 1.0);
        let dust = id("base:copper_dust");
        let outputs = &mut factory.state_mut(at).unwrap().outputs;
        outputs.fill(Some(ItemStack::new(dust, max_stack(dust))));
        outputs[0] = Some(ItemStack::new(dust, max_stack(dust) - 2));
        assert_eq!(factory.put(at, ItemStack::new(id("base:copper_ore"), 8), machines), None);
        // 3.2 s, then 6.4: one ore crushed and 0.4 s on the next, which has no room.
        factory.step(4, 20.0, machines);
        factory.step(4, 20.0, machines);
        let (_, array) = factory.get(at).unwrap();
        assert_eq!((array.status, array.process.is_some()), (Status::Full, true));
        assert!((array.progress - 0.4).abs() < 1e-3, "kept {}", array.progress);
        // Full, it waits with what it kept and takes no power.
        let energy = factory.get(bank).unwrap().1.energy;
        factory.step(4, 20.0, machines);
        assert!((factory.get(at).unwrap().1.progress - 0.4).abs() < 1e-3);
        assert_eq!(factory.get(bank).unwrap().1.energy, energy);
        // Its dust taken, it goes on from there (its network's power coming back up).
        factory.take_outputs(at);
        factory.step(4, 20.0, machines);
        let (_, array) = factory.get(at).unwrap();
        assert!(array.status == Status::Working && array.progress > 0.4 + 1e-3, "went on to {}", array.progress);
    }

    /// Coal and charcoal are one input of the bloomery's recipe: together they fill no more
    /// than its room, and the ore always gets in.
    #[test]
    fn coal_and_charcoal_share_the_fuels_room_so_ore_still_goes_in() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let stack = |name: &str, count| ItemStack::new(blocks.id(name).unwrap_or_else(|| panic!("no {name}")), count);
        let mut factory = Factory::default();
        assert!(factory.insert(IVec3::ZERO, blocks.id("base:bloomery").unwrap(), machines));
        assert_eq!(factory.put(IVec3::ZERO, stack("base:coal_ore", 64), machines), None);
        assert_eq!(factory.put(IVec3::ZERO, stack("base:charcoal", 64), machines), None);
        assert_eq!(factory.put(IVec3::ZERO, stack("base:coal_ore", 64), machines), Some(stack("base:coal_ore", 64)), "a third stack of fuel finds no room");
        assert_eq!(factory.put(IVec3::ZERO, stack("base:iron_ore", 64), machines), None, "the ore gets in");
    }
}
