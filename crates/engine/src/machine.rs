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
        /// How many stacks it holds of what goes in, and of what it made.
        #[serde(default = "three")]
        input_slots: u8,
        #[serde(default = "three")]
        output_slots: u8,
    },
    /// Carries items between the machines beside a run of chutes, `rate` a second at most;
    /// a run moves as fast as its slowest chute.
    Chute { rate: f32 },
    /// Holds items in `slots` stacks. One that `feeds` (a hopper) gives what players put in
    /// it to the machines on its chutes; one that does not (a chest) collects what they make.
    Bin {
        slots: u8,
        #[serde(default)]
        feeds: bool,
    },
}

fn one() -> f32 {
    1.0
}

fn three() -> u8 {
    3
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
    /// The creatures that, kept, walk a treadwheel (`Walked`), by species name; and those an
    /// earlier package named that no longer do.
    #[serde(default)]
    pub walkers: Vec<String>,
    #[serde(default)]
    pub not_walkers: Vec<String>,
    /// How the world drives wind and walked machines; a later package's replaces it.
    #[serde(default)]
    pub sources: Option<SourceRules>,
}

/// How the world drives the machines it powers (`sources` in `machines.ron`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SourceRules {
    /// Wind (blocks a second) under which a windmill stalls, and at which it gives all it can.
    pub wind_stall: f32,
    pub wind_full: f32,
    /// Heights between which a windmill catches more wind, from `wind_low_share` of it to all.
    pub wind_low: f32,
    pub wind_high: f32,
    pub wind_low_share: f32,
    /// How near (in blocks, any way, diagonals too) another windmill's sails becalm one.
    pub wind_spacing: i32,
    /// The wind (blocks a second) a windmill is reckoned to catch while its land is not
    /// loaded: an average day's, not the gale it was left in.
    pub wind_mean: f32,
    /// Seconds between looks at what the world gives machines where their land is loaded.
    pub survey_seconds: f32,
    /// Fluids with this tag (lava) neither turn wheels nor wash ore.
    pub hot_tag: String,
    /// How near a kept animal must stand to walk a treadwheel: across, from its centre, and
    /// up or down.
    pub walk_reach: f32,
    pub walk_height: f32,
    /// What a treadwheel gives, as a share of its power, with a walker in the worst and in
    /// the best condition (between, in proportion).
    pub walk_share: (f32, f32),
    /// In-game days a kept animal walks a treadwheel after a player last fed it.
    pub walk_fed_days: f32,
}

impl Default for SourceRules {
    fn default() -> Self {
        Self {
            wind_stall: 1.0,
            wind_full: 10.0,
            wind_low: 96.0,
            wind_high: 176.0,
            wind_low_share: 1.0 / 3.0,
            wind_spacing: 4,
            wind_mean: 5.0,
            survey_seconds: 1.0,
            hot_tag: "hot".into(),
            walk_reach: 3.0,
            walk_height: 2.0,
            walk_share: (0.5, 1.2),
            walk_fed_days: 2.0,
        }
    }
}

impl SourceRules {
    /// The share of its power a windmill gives in `wind` (blocks a second) at height `y`:
    /// nothing in calm air, all of it in a gale up high.
    pub fn wind_share(&self, wind: f32, y: f32) -> f32 {
        let strength = ((wind - self.wind_stall) / (self.wind_full - self.wind_stall).max(0.01)).clamp(0.0, 1.0);
        let height = ((y - self.wind_low) / (self.wind_high - self.wind_low).max(1.0)).clamp(self.wind_low_share, 1.0);
        strength * height
    }

    /// The share of its power a treadwheel gives with a walker in `condition` (0 to 1); an
    /// animal whose condition is not known walks it at its full power.
    pub fn walk_share(&self, condition: Option<f32>) -> f32 {
        let (worst, best) = self.walk_share;
        condition.map_or(1.0, |c| worst + (best - worst) * c.clamp(0.0, 1.0))
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

    /// The stacks it holds of what goes in and of what it made, if it is a processor.
    pub fn slots(&self) -> (usize, usize) {
        match self.role {
            Role::Processor { input_slots, output_slots, .. } => (input_slots.max(1) as usize, output_slots.max(1) as usize),
            _ => (0, 0),
        }
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
        for walker in file.walkers.iter_mut().chain(&mut file.not_walkers) {
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
            registry.walkers.retain(|w| !file.not_walkers.contains(w));
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

    /// How long one of `item` burns at full output, if it burns: the last fuel that takes it,
    /// so a later package may change what an earlier one said.
    pub fn burn_seconds(&self, item: BlockId) -> Option<f32> {
        self.fuels.iter().rev().find(|(accepts, _)| accepts.binary_search(&item).is_ok()).map(|&(_, s)| s)
    }

    /// How many of its `slots` input stacks `item` may fill: enough to leave one for each
    /// other thing a recipe it goes into takes, so two stacks of ore never shut out the coal.
    pub fn input_room(&self, machine: &Machine, item: BlockId, slots: usize) -> usize {
        let others = machine
            .processes
            .iter()
            .map(|&p| self.process(p))
            .filter(|p| p.inputs.iter().any(|(accepts, _)| accepts.binary_search(&item).is_ok()))
            .map(|p| p.inputs.len() - 1)
            .max()
            .unwrap_or(0);
        slots.saturating_sub(others).max(1)
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

    /// The last fuel that takes an item wins, so the game's broad tags must not shorten coal.
    #[test]
    fn coal_burns_longer_than_a_log_and_a_log_longer_than_planks() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let burns = |name: &str| content.machines.burn_seconds(content.blocks.expect(name));
        assert_eq!(burns("base:coal_ore"), Some(80.0));
        assert_eq!(burns("base:charcoal"), Some(80.0));
        assert_eq!(burns("base:oak_log"), Some(15.0));
        assert_eq!(burns("base:oak_planks"), Some(4.0));
    }

    /// The most ingots one ore can become, by any chain of the game's machines (each kind at
    /// the best yield any machine of it has), crafting recipes and cooking: the iron ladder
    /// (1 by hand, 1.5 bloomed and hammered, 2 crushed, 3 washed) must hold for every ore,
    /// and no loop may give back more than went in. What a recipe makes is shared among the
    /// units that go in, fuels aside (they burn, they add no metal): three copper and a tin
    /// make four bronze, a fourth each; two ore and a coal make a bloom of three, half each.
    #[test]
    fn no_chain_gets_more_than_three_ingots_from_an_ore_and_no_loop_gains() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (blocks, machines) = (&content.blocks, &content.machines);
        let best_yield = |kind: &str| {
            machines.iter().filter_map(|m| match &m.role {
                Role::Processor { kinds, extra, .. } if kinds.iter().any(|k| k == kind) => Some(*extra),
                _ => None,
            }).fold(0.0f32, f32::max)
        };
        // Every conversion: what goes in (any of, how many) and what comes out (how many).
        let mut conversions: Vec<(Vec<(Vec<BlockId>, f32)>, Vec<(BlockId, f32)>)> = Vec::new();
        for (_, process) in machines.processes() {
            let extra = 1.0 + best_yield(&process.kind);
            conversions.push((process.inputs.iter().map(|(a, n)| (a.clone(), *n as f32)).collect(), process.outputs.iter().map(|&(o, n)| (o, n as f32 * extra)).collect()));
        }
        for recipe in content.recipes.iter() {
            let mut inputs: Vec<(Vec<BlockId>, f32)> = Vec::new();
            for accepts in recipe.alternatives().into_iter().flatten() {
                match inputs.iter_mut().find(|(a, _)| a.as_slice() == accepts) {
                    Some((_, n)) => *n += 1.0,
                    None => inputs.push((accepts.to_vec(), 1.0)),
                }
            }
            conversions.push((inputs, vec![(recipe.output, recipe.count as f32)]));
        }
        for (input, recipe) in content.cooking.iter() {
            conversions.push((vec![(vec![input], 1.0)], vec![(recipe.output, 1.0)]));
        }
        // Worth in ingots, raised until nothing changes: a loop that gains never settles.
        let ingot = |b: BlockId| blocks.def(b).name.ends_with("_ingot");
        let mut worth: Vec<f32> = (0..blocks.len()).map(|i| if ingot(BlockId(i as u16)) { 1.0 } else { 0.0 }).collect();
        let mut settled = false;
        for _ in 0..blocks.len() + 2 {
            let mut changed = false;
            for (inputs, outputs) in &conversions {
                let made: f32 = outputs.iter().map(|&(o, n)| n * worth[o.0 as usize]).sum();
                let fuel = |accepts: &Vec<BlockId>| accepts.iter().any(|&a| machines.burn_seconds(a).is_some());
                let units: f32 = inputs.iter().filter(|(a, _)| !fuel(a)).map(|(_, n)| n).sum();
                let units = if units > 0.0 { units } else { inputs.iter().map(|(_, n)| n).sum() };
                for (accepts, _) in inputs {
                    for &item in accepts {
                        let each = made / units;
                        if each > worth[item.0 as usize] * (1.0 + 1e-5) + 1e-6 {
                            worth[item.0 as usize] = each;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                settled = true;
                break;
            }
        }
        let rising: Vec<&str> = (0..blocks.len()).filter(|&i| worth[i] > 1e3).map(|i| blocks.def(BlockId(i as u16)).name.as_str()).collect();
        assert!(settled && worth.iter().all(|w| w.is_finite()), "a loop of recipes gives back more than went in: {rising:?}");
        let mut top = 0.0f32;
        for i in 0..blocks.len() {
            let ore = BlockId(i as u16);
            // An ore that burns (coal) is fuel, not metal: it rides along free.
            if !blocks.def(ore).name.ends_with("_ore") || machines.burn_seconds(ore).is_some() {
                continue;
            }
            assert!(worth[i] <= 3.0 + 1e-4, "{} can become {} ingots", blocks.def(ore).name, worth[i]);
            top = top.max(worth[i]);
        }
        assert!((top - 3.0).abs() < 1e-4, "the ladder's top, washing, gives {top}");
    }

    #[test]
    fn a_treadwheel_gives_half_to_six_fifths_by_its_walkers_condition_and_all_when_unknown() {
        let rules = SourceRules::default();
        assert_eq!(rules.walk_share(None), 1.0);
        assert_eq!(rules.walk_share(Some(0.0)), 0.5);
        assert!((rules.walk_share(Some(1.0)) - 1.2).abs() < 1e-6);
        assert!((rules.walk_share(Some(5.0 / 7.0)) - 1.0).abs() < 1e-6, "an ordinary animal gives its power");
    }

    #[test]
    fn windmills_stall_in_calm_air_and_give_all_in_a_gale_up_high() {
        let rules = SourceRules::default();
        assert_eq!(rules.wind_share(0.5, 180.0), 0.0, "calm air");
        assert_eq!(rules.wind_share(12.0, 180.0), 1.0, "a gale up high");
        assert!((rules.wind_share(12.0, 64.0) - 1.0 / 3.0).abs() < 1e-5, "a gale low down gives a third");
        let breeze = rules.wind_share(5.5, 180.0);
        assert!(breeze > 0.4 && breeze < 0.6, "a breeze gives about half: {breeze}");
    }
}
