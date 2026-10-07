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

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::BlockId;
use crate::coords::Face;
use crate::glam::IVec3;
use crate::inventory::{ItemStack, max_stack};
use crate::machine::{Carrier, Machine, MachineRegistry, Port, PortRole, ProcessId, Role, Source};

/// A processor's input and output slots.
pub const INPUTS: usize = 3;
pub const OUTPUTS: usize = 3;
/// Server ticks a second (the factory counts power in units a tick and work in seconds).
pub const TICKS_PER_SECOND: f32 = 20.0;

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
    pub inputs: [Option<ItemStack>; INPUTS],
    pub outputs: [Option<ItemStack>; OUTPUTS],
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
}

#[derive(Clone, Debug)]
struct Node {
    block: BlockId,
    ports: Vec<Port>,
    state: MachineState,
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
    pub inputs: [Option<ItemStack>; INPUTS],
    pub outputs: [Option<ItemStack>; OUTPUTS],
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
}

impl Factory {
    /// Adds a machine at `pos` if `block` is one; returns whether it was. A processor
    /// starts able to work, a generator that needs the world (water, wind) unable until
    /// told otherwise ([`Factory::set_condition`]).
    pub fn insert(&mut self, pos: IVec3, block: BlockId, machines: &MachineRegistry) -> bool {
        let condition = if machines.get(block).is_some_and(Machine::is_processor) { 1.0 } else { 0.0 };
        self.insert_state(pos, block, MachineState { condition, ..MachineState::default() }, machines)
    }

    /// Adds a machine with what it held (from a save).
    pub fn insert_state(&mut self, pos: IVec3, block: BlockId, state: MachineState, machines: &MachineRegistry) -> bool {
        let Some(machine) = machines.get(block) else { return false };
        self.nodes.insert(pos, Node { block, ports: machine.ports(), state });
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

    /// Every machine, by position.
    pub fn iter(&self) -> impl Iterator<Item = (IVec3, BlockId, &MachineState)> {
        self.nodes.iter().map(|(&pos, n)| (pos, n.block, &n.state))
    }

    /// Sets what the world lets the generator at `pos` give, 0 to 1, or whether the
    /// processor there may work.
    pub fn set_condition(&mut self, pos: IVec3, condition: f32) {
        if let Some(node) = self.nodes.get_mut(&pos) {
            node.state.condition = condition.clamp(0.0, 1.0);
        }
    }

    /// Puts what it can of `stack` into the machine at `pos`: fuel where it burns, an input
    /// where a recipe takes it. Returns what is left.
    pub fn put(&mut self, pos: IVec3, stack: ItemStack, machines: &MachineRegistry) -> Option<ItemStack> {
        let Some(node) = self.nodes.get_mut(&pos) else { return Some(stack) };
        let machine = machines.get(node.block)?;
        let mut left = stack.count;
        if machine.burns() && machines.burn_seconds(stack.item).is_some() {
            left = fill(std::slice::from_mut(&mut node.state.fuel), stack.item, left);
        }
        if left > 0 && machine.is_processor() && machines.takes(machine, stack.item) {
            left = fill(&mut node.state.inputs, stack.item, left);
        }
        if left < stack.count {
            // Picks up the work (or lights the fuel) now, so whoever put it in sees it start;
            // the next step says whether it has the power.
            prepare(node, machine, machines);
            let state = &mut node.state;
            if matches!(state.status, Status::Idle | Status::NoFuel) {
                if state.process.is_some() {
                    state.status = Status::Working;
                } else if state.burning > 0.0 {
                    state.status = Status::Generating;
                }
            }
        }
        (left > 0).then_some(ItemStack { count: left, ..stack })
    }

    /// Puts a stack back into the machine's outputs (what a player could not carry);
    /// returns what did not fit.
    pub fn put_output(&mut self, pos: IVec3, stack: ItemStack) -> Option<ItemStack> {
        let Some(node) = self.nodes.get_mut(&pos) else { return Some(stack) };
        let left = fill(&mut node.state.outputs, stack.item, stack.count);
        (left > 0).then_some(ItemStack { count: left, ..stack })
    }

    /// Takes everything out of the machine's outputs.
    pub fn take_outputs(&mut self, pos: IVec3) -> Vec<ItemStack> {
        let Some(node) = self.nodes.get_mut(&pos) else { return Vec::new() };
        node.state.outputs.iter_mut().filter_map(Option::take).collect()
    }

    /// The network a port of the machine at `pos` is on, as it looks.
    pub fn grid(&self, pos: IVec3, port: usize) -> Grid {
        let Some(&network) = self.joined.get(&pos).and_then(|ports| ports.get(port)) else { return Grid::default() };
        let network = &self.networks[network];
        let mut grid = Grid { carrier: Some(network.carrier), ..Grid::default() };
        for member in &network.members {
            let node = &self.nodes[&member.pos];
            let port = node.ports[member.port];
            match port.role {
                PortRole::Gives(power) => {
                    grid.tier = grid.tier.max(port.tier);
                    grid.supply += power * offer_share(node);
                }
                PortRole::Holds { .. } => grid.tier = grid.tier.max(port.tier),
                PortRole::Takes(power) => grid.demand += power * node.state.supplied.max(0.0),
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
        Some(MachineView {
            block: node.block,
            status: state.status,
            inputs: state.inputs,
            outputs: state.outputs,
            fuel: state.fuel,
            progress,
            making: process.map(|p| p.outputs[0].0),
            power: (power.clamp(0.0, 1.0) * 100.0).round() as u8,
            energy,
            capacity,
            grid,
        })
    }

    /// Runs every machine for `ticks` server ticks.
    pub fn step(&mut self, ticks: u32, machines: &MachineRegistry) {
        if self.dirty {
            self.connect();
        }
        let seconds = ticks as f32 / TICKS_PER_SECOND;
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
                state.process = machine.processes.iter().copied().find(|&p| {
                    let process = machines.process(p);
                    process.tier <= *tier && inputs_hold(&state.inputs, process) && outputs_fit(&state.outputs, process, *extra)
                });
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
                Some(Source::FlowingWater | Source::Wind) => {
                    state.status = if state.condition <= 0.0 { Status::NoSource } else if used > 0.0 { Status::Generating } else { Status::Idle };
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
                state.status = Status::Idle;
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
            if state.progress >= process.seconds {
                finish(pos, state, process, *extra);
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
        fill(&mut state.outputs, item, made.floor() as u8 + more);
    }
    for &(item, count, chance) in &process.byproducts {
        if roll.next() < chance {
            fill(&mut state.outputs, item, count);
        }
    }
    state.made = state.made.wrapping_add(1);
    state.process = None;
    state.progress = 0.0;
}

/// Puts `count` of `item` into `slots`, topping up stacks first; returns how many did not fit.
fn fill(slots: &mut [Option<ItemStack>], item: BlockId, mut count: u8) -> u8 {
    for stack in slots.iter_mut().flatten().filter(|s| s.item == item) {
        let moved = count.min(max_stack(item).saturating_sub(stack.count));
        stack.count += moved;
        count -= moved;
    }
    for slot in slots.iter_mut().filter(|s| s.is_none()) {
        if count == 0 {
            break;
        }
        let moved = count.min(max_stack(item));
        *slot = Some(ItemStack { item, count: moved });
        count -= moved;
    }
    count
}

/// Whether the inputs hold all a recipe takes, each item counted once.
fn inputs_hold(inputs: &[Option<ItemStack>; INPUTS], process: &crate::machine::Process) -> bool {
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
fn outputs_fit(outputs: &[Option<ItemStack>; OUTPUTS], process: &crate::machine::Process, extra: f32) -> bool {
    let mut slots = *outputs;
    let most = |count: u8| (count as f32 * (1.0 + extra)).ceil() as u8;
    process.outputs.iter().map(|&(item, count)| (item, most(count))).chain(process.byproducts.iter().map(|&(item, count, _)| (item, count))).all(|(item, count)| fill(&mut slots, item, count) == 0)
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
            "battery", "mv_battery", "transformer", "furnace", "ingot", "washer",
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
            ], fuels: [(item: "tag:log", seconds: 10.0)])"#,
        )
        .unwrap();
        let processes = MachineRegistry::read_processes(
            "t",
            r#"(recipes: [
                (kind: "crush", inputs: [("ore", 1)], outputs: [("dust", 1)], seconds: 4.0),
                (kind: "smelt", inputs: [("dust", 2)], outputs: [("ingot", 1)], seconds: 2.0),
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
        assert_eq!(factory.put(IVec3::new(at, 0, 0), ItemStack { item: t.id(item), count }, &t.machines), None);
    }

    fn run(t: &Test, factory: &mut Factory, seconds: f32) {
        for _ in 0..(seconds * 5.0) as u32 {
            factory.step(4, &t.machines);
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
        factory.put(IVec3::ZERO, ItemStack { item: t.id("log"), count: 4 }, &t.machines);
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
        factory.put(IVec3::new(2, -1, 0), ItemStack { item: t.id("ore"), count: 1 }, &t.machines);
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
    fn a_furnace_burns_its_own_fuel_and_stops_when_full() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["furnace"]);
        feed(&t, &mut factory, 0, "log", 1);
        // Dust goes in as an input; two make an ingot in two seconds.
        assert_eq!(factory.put(IVec3::ZERO, ItemStack { item: t.id("dust"), count: 6 }, &t.machines), None);
        run(&t, &mut factory, 6.4);
        assert_eq!(made(&t, &factory, 0, "ingot"), 3);
        let (_, furnace) = factory.get(IVec3::ZERO).unwrap();
        assert_eq!(furnace.status, Status::Idle);
        assert!(furnace.burning > 3.0, "a log lasts ten seconds of work");
        assert_eq!(factory.take_outputs(IVec3::ZERO), vec![ItemStack { item: t.id("ingot"), count: 3 }]);
    }

    #[test]
    fn a_machine_takes_only_what_it_uses() {
        let t = test();
        let mut factory = Factory::default();
        build(&t, &mut factory, &["mill"]);
        let ingot = ItemStack { item: t.id("ingot"), count: 5 };
        assert_eq!(factory.put(IVec3::ZERO, ingot, &t.machines), Some(ingot));
        let left = factory.put(IVec3::ZERO, ItemStack { item: t.id("ore"), count: 64 }, &t.machines);
        assert_eq!(left, None);
    }
}
