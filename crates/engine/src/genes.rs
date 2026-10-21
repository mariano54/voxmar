//! Genes: what an animal (and, later, a crop) inherits, and what shows of it
//! (docs/breeding.md, "Genes: the deep layer").
//!
//! A species lists its genes in data. An **allele gene** is Mendelian: every individual
//! carries two alleles, one from each parent, and the more dominant one shows (a recessive
//! white shows only in a pair, so two brown rabbits may have a white kit). A **number
//! gene** is a quantity with many small causes (size, yield, temper): a child gets the
//! middle of its parents plus a little noise, so selective breeding moves it slowly, over
//! generations. Each allele and number says what it does to the animal ([`Effect`]): a coat
//! colour, a marking, its size, its temper, its yield, the climate it is comfortable in.
//!
//! **Regional populations.** Wild animals are not all alike: every region of the world
//! (a few hundred blocks across) has its own mix of alleles and its own means, drawn from
//! the world seed and pushed by climate, so wolves in the snowy north are paler and larger
//! than wolves in southern woods, and one valley's rabbits are mostly black. A breeder who
//! wants a trait travels for it, and taming wild stock from far away brings new blood.
//!
//! **Mutation.** Each birth, each gene may mutate (a new allele, or a jump in a number),
//! more where aether is high. Everything here is deterministic from its seeds.

use serde::{Deserialize, Serialize};

use crate::creature::skin::Marking;
use crate::creature::{mix, unit_from};

/// A gene as a species' data declares it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneDef {
    pub name: String,
    pub kind: GeneKind,
}

#[derive(Clone, Debug, Deserialize)]
pub enum GeneKind {
    /// Two of these alleles per animal; earlier in the list is more dominant.
    Alleles(Vec<AlleleDef>),
    /// A number about `mean`, varying by `spread` (one standard deviation) in the wild,
    /// clamped to `range`; `effect` says what it does, scaled by the value.
    Number { mean: f32, spread: f32, range: (f32, f32), effect: NumberEffect },
}

/// One allele: its name, how common it is in the wild, how climate shifts that, and what
/// it does when it shows.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlleleDef {
    pub name: String,
    /// How common it is in a wild population, before region and climate (any scale).
    #[serde(default = "one")]
    pub frequency: f32,
    /// How much more common it is in the cold (positive) or the heat (negative), per
    /// 10 °C away from 10 °C.
    #[serde(default)]
    pub cold: f32,
    /// How much more common where aether is high.
    #[serde(default)]
    pub aether: f32,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

fn one() -> f32 {
    1.0
}

/// What an allele does when it shows.
#[derive(Clone, Debug, Deserialize)]
pub enum Effect {
    /// Replaces one of the body's colours (by index).
    Colour(u8, [u8; 3]),
    /// Adds a marking to the skin.
    Marking(Marking),
    /// Removes the markings of the species (a solid coat).
    Plain,
    /// Multiplies its size.
    Scale(f32),
    /// Multiplies what it yields (wool, milk, eggs).
    Yield(f32),
    /// Shifts its temper toward fierce (positive) or calm (negative).
    Temper(f32),
    /// Shifts the temperatures it is comfortable in, in °C.
    Hardiness(f32),
}

/// What a number gene does, per unit of its value away from 1 (or 0 for shifts).
#[derive(Clone, Copy, Debug, Deserialize)]
pub enum NumberEffect {
    /// The value multiplies its size.
    Scale,
    /// The value multiplies its yield.
    Yield,
    /// The value shifts its temper toward fierce.
    Temper,
    /// The value shifts its comfortable temperatures, in °C.
    Hardiness,
}

/// One gene's value in one animal.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Gene {
    /// Indices into the gene's alleles, from each parent.
    Pair(u8, u8),
    Number(f32),
}

/// Every gene of one animal, in its species' order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Genome(pub Vec<Gene>);

/// What shows of a genome: what the client paints and the server uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Traits {
    /// Colours replaced, by index.
    pub colours: Vec<(u8, [u8; 3])>,
    pub markings: Vec<Marking>,
    /// The species' own markings are dropped (a solid coat).
    pub plain: bool,
    pub scale: f32,
    pub yield_: f32,
    pub temper: f32,
    pub hardiness: f32,
}

impl Default for Traits {
    fn default() -> Self {
        Self { colours: Vec::new(), markings: Vec::new(), plain: false, scale: 1.0, yield_: 1.0, temper: 0.0, hardiness: 0.0 }
    }
}

/// A named breed: a species' animals that meet every condition, as breeders know them
/// (docs/breeding.md, "Named breeds"). Mods add breeds as data.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreedDef {
    pub name: String,
    /// What players call it; from the name unless given.
    #[serde(default)]
    pub title: String,
    pub when: Vec<Condition>,
}

/// A condition on genes.
#[derive(Clone, Debug, Deserialize)]
pub enum Condition {
    /// This allele shows for this gene.
    Shows(String, String),
    /// This number gene is at least this.
    Above(String, f32),
    /// This number gene is at most this.
    Below(String, f32),
}

/// The first breed `genome` meets, if any.
pub fn breed_of<'a>(defs: &[GeneDef], breeds: &'a [BreedDef], genome: &Genome) -> Option<&'a BreedDef> {
    let gene = |name: &str| defs.iter().position(|d| d.name == name).and_then(|i| Some((&defs[i], genome.0.get(i)?)));
    breeds.iter().find(|breed| {
        breed.when.iter().all(|condition| match condition {
            Condition::Shows(name, allele) => match gene(name) {
                Some((GeneDef { kind: GeneKind::Alleles(alleles), .. }, Gene::Pair(a, b))) => alleles.get((*a).min(*b) as usize).is_some_and(|al| &al.name == allele),
                _ => false,
            },
            Condition::Above(name, value) => matches!(gene(name), Some((_, Gene::Number(n))) if n >= value),
            Condition::Below(name, value) => matches!(gene(name), Some((_, Gene::Number(n))) if n <= value),
        })
    })
}

/// The most genes a species may have (what the wire carries).
pub const MOST: usize = 8;

/// Blocks across a region of one wild population.
pub const REGION: f32 = 384.0;

/// The chance a gene mutates at a birth, before aether.
pub const MUTATION: f32 = 0.005;

/// The region a place belongs to.
pub fn region_of(x: f32, z: f32) -> (i32, i32) {
    ((x / REGION).floor() as i32, (z / REGION).floor() as i32)
}

/// What a wild population's genes look like in one region: each allele gene's
/// frequencies, and each number gene's mean.
#[derive(Clone, Debug, PartialEq)]
pub struct Population {
    pub genes: Vec<PopulationGene>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PopulationGene {
    Frequencies(Vec<f32>),
    Mean(f32),
}

impl Population {
    /// The population of a species (`defs`) in `region`, where it is `temperature` °C and
    /// the aether is `aether` (0..1), for a world `seed`.
    pub fn of(defs: &[GeneDef], seed: u64, species: u64, region: (i32, i32), temperature: f32, aether: f32) -> Self {
        let key = mix(mix(mix(seed ^ 0x9e3, species), region.0 as i64 as u64), region.1 as i64 as u64);
        let cold = (10.0 - temperature) / 10.0;
        let genes = defs
            .iter()
            .enumerate()
            .map(|(g, def)| {
                let key = mix(key, g as u64);
                match &def.kind {
                    GeneKind::Alleles(alleles) => {
                        // Data frequencies, pushed by climate and aether, then by the region's
                        // own luck (each between a third and three times as common).
                        let raw: Vec<f32> = alleles
                            .iter()
                            .enumerate()
                            .map(|(a, allele)| {
                                let climate = (allele.cold * cold + allele.aether * aether * 4.0).exp();
                                let luck = 3f32.powf(unit_from(mix(key, a as u64)) * 2.0 - 1.0);
                                (allele.frequency * climate * luck).max(0.0)
                            })
                            .collect();
                        let total: f32 = raw.iter().sum::<f32>().max(1e-6);
                        PopulationGene::Frequencies(raw.into_iter().map(|f| f / total).collect())
                    }
                    GeneKind::Number { mean, spread, range, effect } => {
                        // The region's mean wanders a spread or so from the species', and
                        // bigger in the cold for size (Bergmann's rule).
                        let wander = (unit_from(mix(key, 99)) * 2.0 - 1.0) * spread * 1.2;
                        let climate = match effect {
                            NumberEffect::Scale => cold.clamp(-2.0, 3.0) * spread * 0.5,
                            NumberEffect::Hardiness => cold.clamp(-3.0, 3.0) * spread,
                            _ => 0.0,
                        };
                        PopulationGene::Mean((mean + wander + climate).clamp(range.0, range.1))
                    }
                }
            })
            .collect();
        Self { genes }
    }

    /// A wild animal of this population.
    pub fn individual(&self, defs: &[GeneDef], seed: u64) -> Genome {
        Genome(
            defs.iter()
                .zip(&self.genes)
                .enumerate()
                .map(|(g, (def, population))| {
                    let key = mix(seed ^ 0xa11e1e, g as u64);
                    match (&def.kind, population) {
                        (GeneKind::Alleles(_), PopulationGene::Frequencies(freq)) => Gene::Pair(pick(freq, unit_from(mix(key, 1))), pick(freq, unit_from(mix(key, 2)))),
                        (GeneKind::Number { spread, range, .. }, PopulationGene::Mean(mean)) => {
                            Gene::Number((mean + normal(key) * spread).clamp(range.0, range.1))
                        }
                        _ => unreachable!("a population is made from its genes"),
                    }
                })
                .collect(),
        )
    }
}

/// The allele at `roll` (0..1) through `frequencies`.
fn pick(frequencies: &[f32], roll: f32) -> u8 {
    let mut sum = 0.0;
    for (i, f) in frequencies.iter().enumerate() {
        sum += f;
        if roll < sum {
            return i as u8;
        }
    }
    frequencies.len().saturating_sub(1) as u8
}

/// A standard normal number from a key (the sum of four uniforms, rescaled).
fn normal(key: u64) -> f32 {
    let sum: f32 = (0..4).map(|i| unit_from(mix(key, 10 + i))).sum();
    (sum - 2.0) * (3.0f32).sqrt()
}

/// A child of `mother` and `father`: one allele of each pair from each, numbers at the
/// middle of the parents plus noise a third of the wild spread, and now and then a
/// mutation, `mutation` the chance per gene.
pub fn inherit(defs: &[GeneDef], mother: &Genome, father: &Genome, seed: u64, mutation: f32) -> Genome {
    Genome(
        defs.iter()
            .enumerate()
            .map(|(g, def)| {
                let key = mix(seed ^ 0xb1e7, g as u64);
                let mutates = unit_from(mix(key, 7)) < mutation;
                match (&def.kind, mother.0.get(g), father.0.get(g)) {
                    (GeneKind::Alleles(alleles), Some(&Gene::Pair(m0, m1)), Some(&Gene::Pair(f0, f1))) => {
                        let from_mother = if unit_from(mix(key, 1)) < 0.5 { m0 } else { m1 };
                        let mut from_father = if unit_from(mix(key, 2)) < 0.5 { f0 } else { f1 };
                        if mutates {
                            from_father = (unit_from(mix(key, 3)) * alleles.len() as f32) as u8 % alleles.len().max(1) as u8;
                        }
                        Gene::Pair(from_mother, from_father)
                    }
                    (GeneKind::Number { spread, range, .. }, Some(&Gene::Number(m)), Some(&Gene::Number(f))) => {
                        let jump = if mutates { normal(mix(key, 5)) * spread * 2.0 } else { 0.0 };
                        Gene::Number(((m + f) / 2.0 + normal(key) * spread / 3.0 + jump).clamp(range.0, range.1))
                    }
                    // Parents from before the species had this gene: the species' mean.
                    (GeneKind::Alleles(_), ..) => Gene::Pair(0, 0),
                    (GeneKind::Number { mean, .. }, ..) => Gene::Number(*mean),
                }
            })
            .collect(),
    )
}

/// What shows of `genome`.
pub fn express(defs: &[GeneDef], genome: &Genome) -> Traits {
    let mut traits = Traits::default();
    for (def, gene) in defs.iter().zip(&genome.0) {
        match (&def.kind, *gene) {
            (GeneKind::Alleles(alleles), Gene::Pair(a, b)) => {
                // The more dominant allele shows: the one earlier in the list.
                let Some(shown) = alleles.get(a.min(b) as usize) else { continue };
                for effect in &shown.effects {
                    match effect {
                        Effect::Colour(index, rgb) => traits.colours.push((*index, *rgb)),
                        Effect::Marking(marking) => traits.markings.push(marking.clone()),
                        Effect::Plain => traits.plain = true,
                        Effect::Scale(s) => traits.scale *= s,
                        Effect::Yield(y) => traits.yield_ *= y,
                        Effect::Temper(t) => traits.temper += t,
                        Effect::Hardiness(h) => traits.hardiness += h,
                    }
                }
            }
            (GeneKind::Number { effect, .. }, Gene::Number(value)) => match effect {
                NumberEffect::Scale => traits.scale *= value,
                NumberEffect::Yield => traits.yield_ *= value,
                NumberEffect::Temper => traits.temper += value,
                NumberEffect::Hardiness => traits.hardiness += value,
            },
            _ => {}
        }
    }
    traits
}

/// The names of the alleles that show, for an inspecting player: "black, long wool".
pub fn shown_alleles<'a>(defs: &'a [GeneDef], genome: &Genome) -> Vec<&'a str> {
    defs.iter()
        .zip(&genome.0)
        .filter_map(|(def, gene)| match (&def.kind, gene) {
            (GeneKind::Alleles(alleles), Gene::Pair(a, b)) => alleles.get((*a).min(*b) as usize).map(|a| a.name.as_str()),
            _ => None,
        })
        .collect()
}

/// A genome packed small for the wire: a pair as two bytes, a number in hundredths
/// offset to stay positive.
pub fn pack(genome: &Genome) -> [u16; MOST] {
    let mut packed = [0; MOST];
    for (slot, gene) in packed.iter_mut().zip(&genome.0) {
        *slot = match *gene {
            Gene::Pair(a, b) => (a as u16) << 8 | b as u16,
            Gene::Number(n) => ((n + 100.0) * 100.0).round().clamp(0.0, u16::MAX as f32) as u16,
        };
    }
    packed
}

/// The genome `pack` packed, read against the species' genes.
pub fn unpack(defs: &[GeneDef], packed: &[u16]) -> Genome {
    Genome(
        defs.iter()
            .zip(packed)
            .map(|(def, &value)| match def.kind {
                GeneKind::Alleles(_) => Gene::Pair((value >> 8) as u8, value as u8),
                GeneKind::Number { .. } => Gene::Number(value as f32 / 100.0 - 100.0),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> Vec<GeneDef> {
        let source = r#"[
            (name: "coat", kind: Alleles([
                (name: "agouti", frequency: 6.0),
                (name: "black", frequency: 2.0, effects: [Colour(0, (40, 36, 34))]),
                (name: "white", frequency: 0.5, cold: 2.0, effects: [Colour(0, (240, 240, 236)), Plain]),
            ])),
            (name: "size", kind: Number(mean: 1.0, spread: 0.06, range: (0.7, 1.4), effect: Scale)),
        ]"#;
        ron::from_str(source).unwrap()
    }

    #[test]
    fn regions_differ_and_the_cold_whitens() {
        let defs = defs();
        let white = |temperature: f32| -> f32 {
            (0..40)
                .map(|r| match &Population::of(&defs, 1, 5, (r, 0), temperature, 0.0).genes[0] {
                    PopulationGene::Frequencies(f) => f[2],
                    _ => 0.0,
                })
                .sum::<f32>()
                / 40.0
        };
        assert!(white(-20.0) > white(20.0) * 4.0, "white is commoner in the cold");
        assert_ne!(Population::of(&defs, 1, 5, (0, 0), 10.0, 0.0), Population::of(&defs, 1, 5, (1, 0), 10.0, 0.0));
        assert_eq!(Population::of(&defs, 1, 5, (0, 0), 10.0, 0.0), Population::of(&defs, 1, 5, (0, 0), 10.0, 0.0));
    }

    #[test]
    fn recessives_hide_and_children_take_one_of_each() {
        let defs = defs();
        let carrier = Genome(vec![Gene::Pair(0, 2), Gene::Number(1.0)]);
        assert!(express(&defs, &carrier).colours.is_empty(), "agouti hides white");
        let mut whites = 0;
        for seed in 0..400 {
            let child = inherit(&defs, &carrier, &carrier, seed, 0.0);
            let Gene::Pair(a, b) = child.0[0] else { panic!() };
            assert!([0, 2].contains(&a) && [0, 2].contains(&b));
            if a == 2 && b == 2 {
                whites += 1;
                assert!(express(&defs, &child).plain);
            }
        }
        // A quarter, as Mendel found.
        assert!((70..130).contains(&whites), "{whites} white of 400");
    }

    /// At a mutation rate of 1 every gene mutates: the father's allele is any of the gene's,
    /// the mother's still one of hers, and numbers jump well past their parents' spread;
    /// at 0 none does.
    #[test]
    fn mutation_draws_a_new_allele_and_a_jump() {
        let defs = defs();
        let plain = Genome(vec![Gene::Pair(0, 0), Gene::Number(1.0)]);
        let (mut fathers, mut far) = ([0u32; 3], 0);
        for seed in 0..600 {
            let child = inherit(&defs, &plain, &plain, seed, 1.0);
            let Gene::Pair(m, f) = child.0[0] else { panic!() };
            assert_eq!(m, 0, "the mother's allele is hers");
            fathers[f as usize] += 1;
            let Gene::Number(n) = child.0[1] else { panic!() };
            far += usize::from((n - 1.0).abs() > 0.06);
            assert_eq!(inherit(&defs, &plain, &plain, seed, 0.0).0[0], Gene::Pair(0, 0), "no mutation at 0");
        }
        // Each allele a third of the time, as the draw is even over them.
        assert!(fathers.iter().all(|&n| (140..260).contains(&n)), "the father's alleles drawn {fathers:?}");
        // Beyond one spread in about half (a jump of two spreads' sd); a third at most without.
        assert!(far > 250, "only {far} of 600 sizes jumped past a spread");
    }

    #[test]
    fn breeds_are_known_by_their_genes() {
        let defs = defs();
        let breeds: Vec<BreedDef> = ron::from_str(r#"[(name: "giant_white", when: [Shows("coat", "white"), Above("size", 1.15)]), (name: "black", when: [Shows("coat", "black")])]"#).unwrap();
        let giant = Genome(vec![Gene::Pair(2, 2), Gene::Number(1.2)]);
        let small_white = Genome(vec![Gene::Pair(2, 2), Gene::Number(1.0)]);
        let black = Genome(vec![Gene::Pair(1, 2), Gene::Number(1.0)]);
        assert_eq!(breed_of(&defs, &breeds, &giant).map(|b| b.name.as_str()), Some("giant_white"));
        assert!(breed_of(&defs, &breeds, &small_white).is_none());
        assert_eq!(breed_of(&defs, &breeds, &black).map(|b| b.name.as_str()), Some("black"));
    }

    #[test]
    fn numbers_breed_toward_the_middle_and_pack_small() {
        let defs = defs();
        let big = Genome(vec![Gene::Pair(0, 2), Gene::Number(1.3)]);
        let small = Genome(vec![Gene::Pair(1, 1), Gene::Number(0.9)]);
        let children: Vec<f32> = (0..200)
            .map(|s| match inherit(&defs, &big, &small, s, 0.0).0[1] {
                Gene::Number(n) => n,
                _ => 0.0,
            })
            .collect();
        let mean = children.iter().sum::<f32>() / children.len() as f32;
        assert!((mean - 1.1).abs() < 0.02, "children average {mean}");
        let same = |a: &Genome, b: &Genome| {
            a.0.iter().zip(&b.0).all(|(x, y)| match (x, y) {
                (Gene::Number(x), Gene::Number(y)) => (x - y).abs() < 0.006,
                _ => x == y,
            })
        };
        assert!(same(&unpack(&defs, &pack(&big)), &big));
        let calm = Genome(vec![Gene::Pair(1, 0), Gene::Number(-0.25)]);
        assert!(same(&unpack(&defs, &pack(&calm)), &calm));
    }
}
