//! Looking closely at a creature (right click on one with nothing it wants in hand): what
//! the player learns grows with their Farming level, as docs/breeding.md, "Inspecting",
//! lays out. Anyone sees its kind, health, age and temper, and a kept one's condition in a
//! word and what it wants (the number at Farming 30); Farming 10 reads what its genes
//! show, 20 what it carries hidden, 30 the numbers, 40 the odds of a mutation where it
//! stands. Every line travels as words, in the player's language.

use voxmar_engine::creature::brain::Personality;
use voxmar_engine::creature::condition;
use voxmar_engine::genes::{self, Gene, GeneKind, NumberEffect};
use voxmar_engine::lang::{Words, def_key};
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage};

use crate::Server;

/// The Farming levels that show each further line.
pub const SHOWS: u8 = 10;
pub const CARRIES: u8 = 20;
pub const NUMBERS: u8 = 30;
pub const ODDS: u8 = 40;

/// How far a number gene must be from its species' mean, in spreads, to be worth a word.
const NOTABLE: f32 = 0.8;

impl Server {
    pub(crate) fn inspect(&mut self, player: PlayerId, id: u32) {
        // A wandering party's speaker answers instead (`wandering::meet`).
        if self.within_speech(player, id) && self.meet(player, id) {
            return;
        }
        // A kept one's needs as they are now, not as last reckoned.
        if self.creatures.live.get(&id).is_some_and(|c| c.owner.is_some()) {
            let needs = self.needs_of(id);
            self.creatures.live.get_mut(&id).expect("checked").needs = needs;
        }
        let Some(creature) = self.creatures.live.get(&id) else { return };
        let species = self.content.creatures.get(creature.species);
        let def = &species.def;
        let level = self.skill_level(player, "core:farming");
        let name = Words::Own(def_key("creature", &def.name), def.title.clone());
        let keeper = match &creature.owner {
            Some(owner) => Words::key("inspect.kept_by").with("name", owner.as_str()),
            None => Words::key("inspect.wild"),
        };
        let age = Words::key(if creature.grows_up.is_some() { "inspect.young" } else { "inspect.adult" });
        let mut lines = vec![
            Words::key("inspect.title").with("creature", name).with("level", Words::text(def.level)).with("keeper", keeper),
            Words::key("inspect.condition")
                .with("health", Words::text(creature.health))
                .with("most", Words::text(def.health))
                .with("age", age)
                .with("temper", temper(&creature.mind.personality)),
        ];
        // A kept one's condition in a word for anyone, exactly at Farming 30, which way it
        // is heading, and what it wants.
        if creature.owner.is_some() {
            let rules = self.content.creatures.condition();
            let state = Words::key(&format!("condition.{}", condition::word(creature.condition, rules.breed)));
            let target = rules.target(&creature.needs, (creature.health as u32) * 2 < def.health as u32);
            let (kept, exactly) = match condition::trend(creature.condition, target) {
                Some(true) => ("inspect.kept_improving", "inspect.kept_exactly_improving"),
                Some(false) => ("inspect.kept_worsening", "inspect.kept_exactly_worsening"),
                None => ("inspect.kept", "inspect.kept_exactly"),
            };
            lines.push(if level >= NUMBERS {
                Words::key(exactly).with("state", state).with("value", Words::text(format!("{:.2}", creature.condition)))
            } else {
                Words::key(kept).with("state", state)
            });
            if let Some(wants) = self.wants(id) {
                lines.push(Words::key("inspect.wants").with("needs", wants));
            }
        }
        // A person's looks are their own, not a breeder's to read.
        let defs: &[_] = if def.kind == voxmar_engine::creature::Kind::Villager { &[] } else { &def.genes };
        let genome = &creature.genome;
        if !defs.is_empty() {
            if level >= SHOWS {
                if let Some(breed) = genes::breed_of(defs, &def.breeds, genome) {
                    let title = if breed.title.is_empty() { voxmar_engine::lang::title_from_id(&breed.name) } else { breed.title.clone() };
                    lines.push(Words::key("inspect.breed").with("breed", Words::Own(def_key("breed", &format!("{}.{}", def.name, breed.name)), title)));
                }
                lines.push(Words::key("inspect.shows").with("traits", Words::List(shown(&def.name, defs, genome))));
            }
            if level >= CARRIES {
                let hidden = carried(&def.name, defs, genome);
                lines.push(if hidden.is_empty() { Words::key("inspect.carries_nothing") } else { Words::key("inspect.carries").with("alleles", Words::List(hidden)) });
            }
            if level >= NUMBERS {
                let numbers: Vec<Words> = defs
                    .iter()
                    .zip(&genome.0)
                    .filter_map(|(gene, value)| match value {
                        Gene::Number(n) => Some(Words::key("inspect.number").with("gene", gene_name(&def.name, &gene.name)).with("value", Words::text(format!("{n:.2}")))),
                        Gene::Pair(..) => None,
                    })
                    .collect();
                if !numbers.is_empty() {
                    lines.push(Words::key("inspect.numbers").with("numbers", Words::List(numbers)));
                }
            }
            if level >= ODDS {
                let at = creature.body.position;
                let aether = self.overworld.site(at.x.floor() as i32, at.z.floor() as i32).aether;
                let chance = self.genetics().mutation_at(aether) * 100.0;
                lines.push(Words::key("inspect.odds").with("chance", Words::text(format!("{chance:.1}"))));
            }
            let next = [SHOWS, CARRIES, NUMBERS, ODDS].into_iter().find(|&l| l > level);
            if let Some(next) = next {
                lines.push(Words::key("inspect.more_at").with("level", Words::text(next)));
            }
        }
        // One keeping a market stall, at it: today's trade.
        if let Some(trade) = self.trade_today(id) {
            let item = |&(item, count): &(voxmar_engine::block::BlockId, u8)| Words::key("inspect.trade_item").with("count", Words::text(count)).with("item", Words::Block(self.content.blocks.def(item).name.clone()));
            lines.push(Words::key("inspect.trade").with("wants", item(&trade.wants)).with("gives", Words::List(trade.gives.iter().map(item).collect())));
        }
        if let Some(p) = self.players.get(&player) {
            p.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::Lines(lines) });
        }
    }
}

/// A gene's name for players: `gene.base.mouflon.wool`, falling back to how data wrote it.
fn gene_name(species: &str, gene: &str) -> Words {
    Words::Own(def_key("gene", &format!("{species}.{gene}")), gene.replace('_', " "))
}

/// An allele's name for players: `gene.base.rabbit.coat.white`.
fn allele_name(species: &str, gene: &str, allele: &str) -> Words {
    Words::Own(def_key("gene", &format!("{species}.{gene}.{allele}")), format!("{} {}", allele.replace('_', " "), gene.replace('_', " ")))
}

/// Its temper in a few words: the far ends of its personality.
fn temper(personality: &Personality) -> Words {
    let words = personality.words();
    if words.is_empty() {
        return Words::key("personality.even");
    }
    Words::List(words.into_iter().map(|w| Words::key(&format!("personality.{w}"))).collect())
}

/// What shows: each allele gene's showing allele, and each number far enough from the mean
/// to notice.
fn shown(species: &str, defs: &[genes::GeneDef], genome: &genes::Genome) -> Vec<Words> {
    let mut out = Vec::new();
    for (def, gene) in defs.iter().zip(&genome.0) {
        match (&def.kind, gene) {
            (GeneKind::Alleles(alleles), Gene::Pair(a, b)) => {
                if let Some(allele) = alleles.get((*a).min(*b) as usize) {
                    out.push(allele_name(species, &def.name, &allele.name));
                }
            }
            (GeneKind::Number { mean, spread, effect, .. }, Gene::Number(n)) => {
                let off = (n - mean) / spread.max(1e-3);
                if off.abs() < NOTABLE {
                    continue;
                }
                let high = off > 0.0;
                let key = match effect {
                    NumberEffect::Scale => if high { "inspect.large" } else { "inspect.small" },
                    NumberEffect::Yield => if high { "inspect.rich" } else { "inspect.poor" },
                    NumberEffect::Temper => if high { "inspect.fierce" } else { "inspect.calm" },
                    NumberEffect::Hardiness => if high { "inspect.hardy" } else { "inspect.tender" },
                };
                out.push(Words::key(key).with("gene", gene_name(species, &def.name)));
            }
            _ => {}
        }
    }
    out
}

/// What it carries hidden: the recessive allele of each pair whose other allele shows.
fn carried(species: &str, defs: &[genes::GeneDef], genome: &genes::Genome) -> Vec<Words> {
    defs.iter()
        .zip(&genome.0)
        .filter_map(|(def, gene)| match (&def.kind, gene) {
            (GeneKind::Alleles(alleles), Gene::Pair(a, b)) if a != b => alleles.get((*a).max(*b) as usize).map(|al| allele_name(species, &def.name, &al.name)),
            _ => None,
        })
        .collect()
}
