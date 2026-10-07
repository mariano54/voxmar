//! Machines: blocks that work by themselves, as content (each package's `machines.ron`),
//! and what they make (`processing.ron`). [`crate::factory`] runs them.
//!
//! A machine block plays one role:
//!
//! - a **conduit** carries power to its neighbours: shafts carry rotation, pipes steam,
//!   cables electricity (each cable up to a tier: LV 1, MV 2, HV 3);
//! - a **generator** makes power from a source: fuel it burns, flowing water beside it, the
//!   wind (more up high), or power of another kind it converts (a beam engine turns steam
//!   into rotation, a dynamo rotation into electricity);
//! - a **store** holds power (a battery bank);
//! - a **transformer** takes electricity from above and gives it a tier lower to its sides
//!   and below;
//! - a **processor** works recipes of its kinds (`crush`, `mill`, `smelt`...), driven by
//!   power, by fuel it burns itself, or by nothing at all;
//! - a **chute** carries items between the machines beside a run of chutes (a conveyor is
//!   a faster chute), at the rate of its slowest;
//! - a **bin** holds items: a hopper feeds what players put in it to the machines on its
//!   chutes, a chest collects what they made.
//!
//! Machines of the same kind differ in how fast they work (`speed`) and how much more they
//! get from what goes in (`yield`), never in what they make: every crusher turns copper
//! ore into copper dust, a better one sooner and more of it. Machines teach nothing: they
//! work for you; making them (recipes) is what takes Engineering.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, BlockRegistry, RegistryError};

/// What power travels as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Carrier {
    /// Turning shafts: water wheels, windmills, treadwheels (the Bronze age).
    Rotation,
    /// Steam in pipes, from boilers (the Iron and Steam ages).
    Steam,
    /// Electricity in cables, at a tier (LV, MV, HV...).
    Electric,
}

impl Carrier {
    pub const ALL: [Carrier; 3] = [Carrier::Rotation, Carrier::Steam, Carrier::Electric];

    /// Whether power of this kind comes at tiers that machines must match.
    pub fn tiered(self) -> bool {
        self == Carrier::Electric
    }
}

/// Where a generator's power comes from.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub enum Source {
    /// Fuel put into it (`fuels` in `machines.ron`), burnt only as fast as the power is used.
    Fuel,
    /// Flowing water touching it: a river, a stream, a waterfall.
    FlowingWater,
    /// The wind where it stands, stronger in a gale and higher up; nothing in calm air.
    Wind,
    /// A kept animal of a kind that walks it (`walkers` in `machines.ron`) beside it: a
    /// treadwheel.
    Walked,
    /// Power of another kind it draws from its other neighbours, converting this much of
    /// it to each unit it gives (a dynamo's 1.25: four fifths come out).
    Converts { from: Carrier, ratio: f32 },
}

/// A machine block's role, as `machines.ron` gives it.
#[derive(Clone, Debug, Deserialize)]
pub enum Role {
    Conduit {
        carrier: Carrier,
        /// The highest tier it carries (electricity); a network fed above it carries nothing.
        #[serde(default)]
        tier: u8,
    },
    Generator {
        carrier: Carrier,
        #[serde(default)]
        tier: u8,
        /// Power at full output, in units a tick.
        power: f32,
        source: Source,
    },
    Store {
        carrier: Carrier,
        #[serde(default)]
        tier: u8,
        /// Power it holds full.
        capacity: f32,
        /// Power it takes or gives at most, in units a tick.
        rate: f32,
    },
    /// Takes electricity at `tier` from above and gives it at `tier - 1` around the rest.
    Transformer {
        tier: u8,
        /// Power it passes at most, in units a tick.
        rate: f32,
    },
    Processor {
        /// The kinds of recipe it works (`processing.ron`).
        kinds: Vec<String>,
        /// How fast it works: a recipe's seconds are divided by it.
        #[serde(default = "one")]
        speed: f32,
        /// How much more it makes: each output's count times one more than this, the
        /// fraction a chance of one more.
        #[serde(default, rename = "yield")]
        extra: f32,
        /// The power it draws while working, of a kind, in units a tick; none for a
        /// machine that needs none, or burns fuel itself.
        #[serde(default)]
        power: Option<(Carrier, f32)>,
        /// Its tier (electricity): it refuses a network above it and works recipes up to it.
        #[serde(default)]
        tier: u8,
        /// It burns fuel itself, as a furnace does, rather than drawing power.
        #[serde(default)]
        burns: bool,
        /// It works only with water beside it (a washer); the world says whether there is.
        #[serde(default)]
        water: bool,
    },
    /// Carries items between the machines beside a run of chutes, `rate` a second at most;
    /// a run moves as fast as its slowest chute.
    Chute { rate: f32 },
    /// Holds items in `slots` stacks. One that `feeds` (a hopper) gives what players put in
    /// it to the machines on its chutes; one that does not (a crate) collects what they make.
    Bin {
        slots: u8,
        #[serde(default)]
        feeds: bool,
    },
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineDef {
    /// The block that is this machine.
    pub block: String,
    pub role: Role,
}

/// Something that burns, and for how long at full output.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuelDef {
    /// An item, `tag:TAG` or `a|b`.
    pub item: String,
    pub seconds: f32,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MachineFile {
    #[serde(default)]
    pub machines: Vec<MachineDef>,
    #[serde(default)]
    pub fuels: Vec<FuelDef>,
    /// The creatures that, kept, walk a treadwheel (`Walked`), by species name.
    #[serde(default)]
    pub walkers: Vec<String>,
    /// How the world drives wind and walked machines; a later package's replaces it.
    #[serde(default)]
    pub sources: Option<SourceRules>,
}

/// How the world drives the machines it powers (`sources` in `machines.ron`).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SourceRules {
    /// Wind (blocks a second) under which a windmill stalls, and at which it gives all it can.
    pub wind_stall: f32,
    pub wind_full: f32,
    /// Heights between which a windmill catches more wind, from `wind_low_share` of it to all.
    pub wind_low: f32,
    pub wind_high: f32,
    pub wind_low_share: f32,
    /// How near a kept animal must stand to walk a treadwheel: across, from its centre, and
    /// up or down.
    pub walk_reach: f32,
    pub walk_height: f32,
}

impl Default for SourceRules {
    fn default() -> Self {
        Self { wind_stall: 1.0, wind_full: 10.0, wind_low: 96.0, wind_high: 176.0, wind_low_share: 1.0 / 3.0, walk_reach: 3.0, walk_height: 2.0 }
    }
}

/// A recipe a processor works, as `processing.ron` gives it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessDef {
    pub kind: String,
    /// What goes in: an item (`tag:TAG` or `a|b` for any of several) and how many.
    pub inputs: Vec<(String, u8)>,
    /// What comes out, and how many, before a machine's yield.
    pub outputs: Vec<(String, u8)>,
    /// Now and then something more: an item, how many and the chance (yield leaves these be).
    #[serde(default)]
    pub byproducts: Vec<(String, u8, f32)>,
    /// Seconds at speed 1.
    pub seconds: f32,
    /// The least machine tier that works it (electricity); 0 for any.
    #[serde(default)]
    pub tier: u8,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProcessFile {
    #[serde(default)]
    pub recipes: Vec<ProcessDef>,
}

/// What an input takes: one item or any of several, sorted.
pub type Accepts = Vec<BlockId>;

#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub kind: String,
    pub inputs: Vec<(Accepts, u8)>,
    pub outputs: Vec<(BlockId, u8)>,
    pub byproducts: Vec<(BlockId, u8, f32)>,
    pub seconds: f32,
    pub tier: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessId(pub u16);

/// Which neighbours a port joins: the faces it opens on, a bit per [`crate::coords::Face`].
pub type Faces = u8;
pub const ALL_FACES: Faces = 0b11_1111;
/// The top face alone, and every face but it (a transformer's two sides).
pub const UP: Faces = 1 << crate::coords::Face::PosY as u8;
pub const NOT_UP: Faces = ALL_FACES & !UP;

/// What a port does on its network.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PortRole {
    /// Passes power on.
    Carries,
    /// Gives up to this much a tick.
    Gives(f32),
    /// Takes up to this much a tick.
    Takes(f32),
    /// Holds this much, taking or giving up to `rate` a tick.
    Holds { capacity: f32, rate: f32 },
}

/// One connection of a machine to a power network: what it carries, at which tier, on
/// which faces, and what it does there. Most machines have one; converters and
/// transformers have two, each on a network of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Port {
    pub carrier: Carrier,
    pub tier: u8,
    pub faces: Faces,
    pub role: PortRole,
}

/// A machine with its names resolved.
#[derive(Clone, Debug)]
pub struct Machine {
    pub block: BlockId,
    pub role: Role,
    /// The recipes it works, in the order packages list them.
    pub processes: Vec<ProcessId>,
}

impl Machine {
    /// Its connections to power networks. A machine that gives power gives on its first;
    /// one that converts takes on its second.
    pub fn ports(&self) -> Vec<Port> {
        let port = |carrier, tier, faces, role| Port { carrier, tier, faces, role };
        match self.role {
            Role::Conduit { carrier, tier } => vec![port(carrier, tier, ALL_FACES, PortRole::Carries)],
            Role::Generator { carrier, tier, power, source } => {
                let mut ports = vec![port(carrier, tier, ALL_FACES, PortRole::Gives(power))];
                if let Source::Converts { from, ratio } = source {
                    // At the tier it gives, when it draws electricity.
                    ports.push(port(from, if from.tiered() { tier.max(1) } else { 0 }, ALL_FACES, PortRole::Takes(power * ratio)));
                }
                ports
            }
            Role::Store { carrier, tier, capacity, rate } => vec![port(carrier, tier, ALL_FACES, PortRole::Holds { capacity, rate })],
            // What it gives first, as for every machine that gives.
            Role::Transformer { tier, rate } => vec![
                port(Carrier::Electric, tier.saturating_sub(1).max(1), NOT_UP, PortRole::Gives(rate)),
                port(Carrier::Electric, tier, UP, PortRole::Takes(rate)),
            ],
            Role::Processor { power: Some((carrier, power)), tier, .. } => vec![port(carrier, tier, ALL_FACES, PortRole::Takes(power))],
            Role::Processor { .. } | Role::Chute { .. } | Role::Bin { .. } => Vec::new(),
        }
    }

    /// Whether it takes fuel: a generator that burns it, or a processor that does.
    pub fn burns(&self) -> bool {
        matches!(self.role, Role::Generator { source: Source::Fuel, .. } | Role::Processor { burns: true, .. })
    }

    pub fn is_processor(&self) -> bool {
        matches!(self.role, Role::Processor { .. })
    }

    /// The stacks it holds as a bin, if it is one.
    pub fn bin_slots(&self) -> usize {
        match self.role {
            Role::Bin { slots, .. } => slots as usize,
            _ => 0,
        }
    }
}

/// Every machine, fuel and processing recipe.
#[derive(Debug, Default)]
pub struct MachineRegistry {
    machines: Vec<Machine>,
    by_block: FxHashMap<BlockId, usize>,
    fuels: Vec<(Accepts, f32)>,
    processes: Vec<Process>,
    walkers: Vec<String>,
    /// How the world drives wind and walked machines.
    pub sources: SourceRules,
}

impl MachineRegistry {
    /// Reads a package's `machines.ron`; unqualified names go into `namespace`.
    pub fn read(namespace: &str, source: &str) -> Result<MachineFile, RegistryError> {
        let mut file: MachineFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/machines.ron: {e}")))?;
        for def in &mut file.machines {
            qualify(namespace, &mut def.block);
        }
        for fuel in &mut file.fuels {
            qualify(namespace, &mut fuel.item);
        }
        for walker in &mut file.walkers {
            qualify(namespace, walker);
        }
        Ok(file)
    }

    /// Reads a package's `processing.ron`.
    pub fn read_processes(namespace: &str, source: &str) -> Result<ProcessFile, RegistryError> {
        let mut file: ProcessFile = ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/processing.ron: {e}")))?;
        for def in &mut file.recipes {
            def.inputs.iter_mut().for_each(|(name, _)| qualify(namespace, name));
            def.outputs.iter_mut().for_each(|(name, _)| qualify(namespace, name));
            def.byproducts.iter_mut().for_each(|(name, _, _)| qualify(namespace, name));
        }
        Ok(file)
    }

    pub fn new(files: Vec<MachineFile>, processes: Vec<ProcessFile>, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut registry = Self::default();
        for def in processes.into_iter().flat_map(|f| f.recipes) {
            let bad = |what: String| RegistryError(format!("processing recipe {} for {:?}: {what}", def.kind, def.outputs.first().map(|o| &o.0)));
            if def.seconds <= 0.0 || def.inputs.is_empty() || def.outputs.is_empty() {
                return Err(bad("needs inputs, outputs and seconds".into()));
            }
            let item = |name: &str| blocks.id(name).ok_or_else(|| bad(format!("no block `{name}`")));
            let inputs = def.inputs.iter().map(|(name, n)| Ok((accepts(name, blocks).map_err(|e| bad(e.0))?, (*n).max(1)))).collect::<Result<_, RegistryError>>()?;
            let outputs = def.outputs.iter().map(|(name, n)| Ok((item(name)?, (*n).max(1)))).collect::<Result<_, RegistryError>>()?;
            let byproducts = def.byproducts.iter().map(|(name, n, chance)| Ok((item(name)?, (*n).max(1), chance.clamp(0.0, 1.0)))).collect::<Result<_, RegistryError>>()?;
            registry.processes.push(Process { kind: def.kind.clone(), inputs, outputs, byproducts, seconds: def.seconds, tier: def.tier });
        }
        for file in files {
            for def in file.machines {
                let block = blocks.id(&def.block).ok_or_else(|| RegistryError(format!("machine: no block `{}`", def.block)))?;
                let processes = match &def.role {
                    Role::Processor { kinds, .. } => (0..registry.processes.len())
                        .filter(|&i| kinds.contains(&registry.processes[i].kind))
                        .map(|i| ProcessId(i as u16))
                        .collect(),
                    _ => Vec::new(),
                };
                if let Role::Processor { kinds, .. } = &def.role
                    && processes.is_empty()
                {
                    return Err(RegistryError(format!("machine {}: no recipes of {kinds:?}", def.block)));
                }
                if let Role::Chute { rate } = def.role
                    && rate <= 0.0
                {
                    return Err(RegistryError(format!("machine {}: a chute needs a rate above 0", def.block)));
                }
                if registry.by_block.insert(block, registry.machines.len()).is_some() {
                    return Err(RegistryError(format!("machine {} is defined twice", def.block)));
                }
                registry.machines.push(Machine { block, role: def.role, processes });
            }
            for fuel in file.fuels {
                registry.fuels.push((accepts(&fuel.item, blocks)?, fuel.seconds.max(0.0)));
            }
            registry.walkers.extend(file.walkers);
            if let Some(sources) = file.sources {
                registry.sources = sources;
            }
        }
        Ok(registry)
    }

    /// The machine a block is, if it is one.
    pub fn get(&self, block: BlockId) -> Option<&Machine> {
        self.by_block.get(&block).map(|&i| &self.machines[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Machine> {
        self.machines.iter()
    }

    pub fn process(&self, id: ProcessId) -> &Process {
        &self.processes[id.0 as usize]
    }

    pub fn processes(&self) -> impl Iterator<Item = (ProcessId, &Process)> {
        self.processes.iter().enumerate().map(|(i, p)| (ProcessId(i as u16), p))
    }

    /// The species that walk treadwheels, by name.
    pub fn walkers(&self) -> &[String] {
        &self.walkers
    }

    /// Whether a kept creature of this species (by name) walks a treadwheel.
    pub fn walks(&self, species: &str) -> bool {
        self.walkers.iter().any(|w| w == species)
    }

    /// How long one of `item` burns at full output, if it burns: the first fuel that takes it.
    pub fn burn_seconds(&self, item: BlockId) -> Option<f32> {
        self.fuels.iter().find(|(accepts, _)| accepts.binary_search(&item).is_ok()).map(|&(_, s)| s)
    }

    /// Whether `machine` works any recipe `item` goes into.
    pub fn takes(&self, machine: &Machine, item: BlockId) -> bool {
        machine.processes.iter().any(|&p| self.process(p).inputs.iter().any(|(accepts, _)| accepts.binary_search(&item).is_ok()))
    }
}

fn qualify(namespace: &str, name: &mut String) {
    if !name.starts_with("tag:") {
        *name = name.split('|').map(|n| if n.contains(':') { n.to_owned() } else { format!("{namespace}:{n}") }).collect::<Vec<_>>().join("|");
    }
}

/// What `name` takes: an item, `tag:TAG` or `a|b`.
fn accepts(name: &str, blocks: &BlockRegistry) -> Result<Accepts, RegistryError> {
    let mut accepts: Accepts = match name.strip_prefix("tag:") {
        Some(tag) => blocks.ids().filter(|&id| blocks.def(id).has_tag(tag)).collect(),
        None => name.split('|').map(|n| blocks.id(n).ok_or_else(|| RegistryError(format!("no block `{n}`")))).collect::<Result<_, _>>()?,
    };
    if accepts.is_empty() {
        return Err(RegistryError(format!("nothing has the tag in `{name}`")));
    }
    accepts.sort();
    accepts.dedup();
    Ok(accepts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    pub(crate) fn blocks() -> BlockRegistry {
        let block = |name: &str, tags: &[&str]| BlockDef { name: format!("t:{name}"), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        BlockRegistry::new(vec![block("ore", &[]), block("dust", &[]), block("log", &["log"]), block("crusher", &[]), block("cable", &[])]).unwrap()
    }

    #[test]
    fn processors_know_their_recipes_and_fuels_their_items() {
        let blocks = blocks();
        let machines = MachineRegistry::read(
            "t",
            r#"(machines: [
                (block: "crusher", role: Processor(kinds: ["crush"], speed: 2.0, yield: 1.0, power: Some((Electric, 8.0)), tier: 1)),
                (block: "cable", role: Conduit(carrier: Electric, tier: 1)),
            ], fuels: [(item: "tag:log", seconds: 15.0)])"#,
        )
        .unwrap();
        let processes = MachineRegistry::read_processes("t", r#"(recipes: [(kind: "crush", inputs: [("ore", 1)], outputs: [("dust", 1)], seconds: 10.0)])"#).unwrap();
        let registry = MachineRegistry::new(vec![machines], vec![processes], &blocks).unwrap();
        let crusher = registry.get(blocks.id("t:crusher").unwrap()).unwrap();
        assert_eq!(crusher.processes, vec![ProcessId(0)]);
        assert!(registry.takes(crusher, blocks.id("t:ore").unwrap()));
        assert!(!registry.takes(crusher, blocks.id("t:dust").unwrap()));
        assert_eq!(registry.burn_seconds(blocks.id("t:log").unwrap()), Some(15.0));
        assert_eq!(registry.burn_seconds(blocks.id("t:ore").unwrap()), None);
        assert_eq!(crusher.ports(), vec![Port { carrier: Carrier::Electric, tier: 1, faces: ALL_FACES, role: PortRole::Takes(8.0) }]);
    }

    #[test]
    fn a_processor_with_nothing_to_work_is_an_error() {
        let blocks = blocks();
        let machines = MachineRegistry::read("t", r#"(machines: [(block: "crusher", role: Processor(kinds: ["mill"]))])"#).unwrap();
        assert!(MachineRegistry::new(vec![machines], Vec::new(), &blocks).is_err());
    }

    /// The most ingots one ore can become through the game's machines, each kind at the best
    /// yield any machine of it has: the iron ladder (1 by hand, 1.5 bloomed and hammered, 2
    /// crushed, 3 washed) must hold for every ore, so no chain of machines beats it.
    #[test]
    fn no_chain_of_machines_gets_more_than_three_ingots_from_an_ore() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let best_yield = |kind: &str| {
            machines.iter().filter_map(|m| match &m.role {
                Role::Processor { kinds, extra, .. } if kinds.iter().any(|k| k == kind) => Some(*extra),
                _ => None,
            }).fold(0.0f32, f32::max)
        };
        // Ingots an item is worth at most, following recipes from it; fuels and other
        // inputs beside it are free, which only makes the bound looser.
        fn worth(item: BlockId, blocks: &BlockRegistry, machines: &MachineRegistry, best_yield: &dyn Fn(&str) -> f32, seen: &mut Vec<BlockId>) -> f32 {
            if blocks.def(item).name.ends_with("_ingot") {
                return 1.0;
            }
            if seen.contains(&item) {
                return 0.0;
            }
            seen.push(item);
            let mut best = 0.0f32;
            for (_, process) in machines.processes() {
                let Some(&(_, count)) = process.inputs.iter().find(|(accepts, _)| accepts.binary_search(&item).is_ok()) else { continue };
                let made: f32 = process.outputs.iter().map(|&(out, n)| n as f32 * (1.0 + best_yield(&process.kind)) * worth(out, blocks, machines, best_yield, seen)).sum();
                best = best.max(made / count as f32);
            }
            seen.pop();
            best
        }
        let ores: Vec<BlockId> = (0..blocks.len()).map(|i| BlockId(i as u16)).filter(|&b| blocks.def(b).name.ends_with("_ore")).collect();
        let mut top = 0.0f32;
        for ore in ores {
            let most = worth(ore, blocks, machines, &best_yield, &mut Vec::new());
            assert!(most <= 3.0 + 1e-4, "{} can become {most} ingots", blocks.def(ore).name);
            top = top.max(most);
        }
        assert!((top - 3.0).abs() < 1e-4, "the ladder's top, washing, gives {top}");
    }
}
