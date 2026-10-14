//! A boss's fight, as data: the moves it chooses among by its phase and how far away its
//! target stands, the phases its health passes through, and how it mends between blows
//! (docs/bosses.md). Any creature may have them; a boss is what usually does.

use serde::Deserialize;

use super::Attack;

/// One of a creature's attacks besides its plain `attack`, with when it uses it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    /// Its name, for tests and the docs.
    pub name: String,
    pub attack: Attack,
    /// The phases it is used in (0, the first, while the creature is above its first
    /// phase's `below`); none for every phase.
    #[serde(default)]
    pub phases: Vec<u8>,
    /// Blocks from its body's edge the target must be at least: a volley for a player who
    /// keeps away, never for one at its feet. Its most is its attack's `reach`.
    #[serde(default)]
    pub least: f32,
    /// How it shows the wind-up, so each move reads apart.
    #[serde(default)]
    pub tell: Tell,
}

/// A charge (`Attack::charge`): once its wind-up is done the creature runs straight the
/// way it faces, its aim locked, at `speed` blocks a second for up to `distance` blocks. It
/// does not turn or stop for what lies ahead, so it is a thing to step aside from, and the
/// land answers it as data, for any charger (docs/bosses.md):
/// - it strikes the first player it comes to, judged as any blow is (a dodge passes it);
/// - a guard that braces (`combat::GuardDef::brace`, a spear's) raised facing it stops
///   it on the point: the bracer's max hit, sure to land, and it stands dazed `stun` s;
/// - a wall two blocks high stops it dazed for `stun` seconds as well (a step it hops);
/// - it runs off edges, and falls past survival's `safe_fall` cost it
///   `combat::Feel::charge_fall` of its most health a block: a cliff, or a pit it cannot
///   climb out of.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Charge {
    pub speed: f32,
    pub distance: f32,
    #[serde(default)]
    pub stun: f32,
}

impl Charge {
    /// Seconds the run takes at most.
    pub fn seconds(&self) -> f32 {
        self.distance / self.speed
    }
}

/// How a creature's body shows a wind-up: it nods its head down, crouches and rears up
/// (each 0 ..= 1, as the client draws a pose).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tell {
    pub nod: f32,
    pub crouch: f32,
    pub rear: f32,
}

/// A stage of the fight, from when the creature's health falls to `below` (a share of its
/// most). Crossing into it, it rears for `hold` seconds, its tell (`Tell::rear` full),
/// and nearby players are told (`boss.NS.NAME.phase.N` in every language).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub below: f32,
    #[serde(default = "hold")]
    pub hold: f32,
}

fn hold() -> f32 {
    2.0
}

/// How a creature mends its wounds: `health` every `every` seconds once `after` seconds
/// have passed without a blow landing on it, unless something stops it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mend {
    pub health: u16,
    #[serde(default = "every")]
    pub every: f32,
    #[serde(default)]
    pub after: f32,
    /// Blocks with any of these tags within `fire_reach` stop it: cut and burned.
    #[serde(default)]
    pub fire: Vec<String>,
    #[serde(default)]
    pub fire_reach: f32,
    /// Daylight (0 ..= 1) at which it stops under open sky: a thing of the dark withers
    /// in the sun. Unset, the sun does nothing.
    #[serde(default)]
    pub daylight: Option<f32>,
}

fn every() -> f32 {
    1.0
}

/// What stops a creature mending now.
#[derive(Clone, Copy, Debug, Default)]
pub struct MendStoppers {
    /// A block with one of its `fire` tags is within its `fire_reach`.
    pub fire: bool,
    /// The daylight now, and whether open sky is over it.
    pub daylight: f32,
    pub open_sky: bool,
}

impl Mend {
    /// Whether it mends at all, things being as `now` says.
    pub fn mends(&self, now: MendStoppers) -> bool {
        let sun = self.daylight.is_some_and(|least| now.open_sky && now.daylight >= least);
        !(now.fire || sun)
    }
}

impl super::Creature {
    /// The phase a creature at `health` (a share of its most) is in: how many of its
    /// phases' `below` it has fallen to.
    pub fn phase(&self, health: f32) -> u8 {
        self.def.phases.iter().filter(|p| health <= p.below).count() as u8
    }

    /// Its attack by index: 0 its plain `attack`, then its moves in order.
    pub fn attack_at(&self, index: u8) -> Option<Attack> {
        match index {
            0 => self.def.attack,
            i => self.def.moves.get(i as usize - 1).map(|m| m.attack),
        }
    }

    /// How it shows the wind-up of the attack at `index`.
    pub fn tell_at(&self, index: u8) -> Tell {
        match index {
            0 => Tell::default(),
            i => self.def.moves.get(i as usize - 1).map(|m| m.tell).unwrap_or_default(),
        }
    }

    /// The moves usable in `phase`, with their indices (for `attack_at`).
    fn moves_in(&self, phase: u8) -> impl Iterator<Item = (u8, &Move)> {
        self.def.moves.iter().enumerate().map(|(i, m)| (i as u8 + 1, m)).filter(move |(_, m)| m.phases.is_empty() || m.phases.contains(&phase))
    }

    /// The furthest it strikes from in `phase`, from its body's edge.
    pub fn longest_reach(&self, phase: u8) -> f32 {
        self.moves_in(phase).map(|(_, m)| m.attack.reach).fold(self.def.attack.map_or(1.0, |a| a.reach), f32::max)
    }

    /// How far a target `flat` blocks from its centre (across the ground) is from its body's
    /// edge, at its individual `scale`: what its reach is measured from, by its brain when
    /// it judges it close enough to strike and by the fight when it winds up.
    pub fn edge(&self, flat: f32, scale: f32) -> f32 {
        flat - self.def.body.size.0 * scale * 0.5 - 0.3
    }

    /// Whether it has an attack for a target `edge` blocks from its body's edge in `phase`:
    /// one of its moves fits (`choose`), so a boss whose moves leave a gap between their
    /// reaches (a lash up close, a volley from afar) closes in across it rather than stand
    /// where nothing fits; one with no moves, within its plain attack's reach.
    pub fn reaches(&self, phase: u8, edge: f32) -> bool {
        if self.def.moves.is_empty() { edge <= self.longest_reach(phase) } else { self.choose(phase, edge, 0).is_some() }
    }

    /// The attack it winds up at a target `edge` blocks from its body's edge, in `phase`,
    /// having used `last` before: the next of its moves that fits, in order, so a fight
    /// has a pattern to learn; its plain attack if none does and the target is in its
    /// reach; none if the target is out of reach of everything. A creature with no moves
    /// always uses its plain attack, as it did before moves: its brain judged the reach.
    pub fn choose(&self, phase: u8, edge: f32, last: u8) -> Option<u8> {
        if self.def.moves.is_empty() {
            return self.def.attack.map(|_| 0);
        }
        // A target pressed against its body, or inside its edge, is as near as can be: its
        // close moves fit, rather than a plain blow with no tell (`moves` are what a boss
        // tells).
        let edge = edge.max(0.0);
        let fits: Vec<u8> = self.moves_in(phase).filter(|(_, m)| edge >= m.least && edge <= m.attack.reach).map(|(i, _)| i).collect();
        if let Some(&next) = fits.iter().find(|&&i| i > last).or(fits.first()) {
            return Some(next);
        }
        self.def.attack.filter(|a| edge <= a.reach).map(|_| 0)
    }
}

/// Checks a creature's moves, phases and mending as read.
pub(super) fn check(def: &super::CreatureDef) -> Result<(), String> {
    if def.moves.len() >= u8::MAX as usize {
        return Err("has too many moves".into());
    }
    if !def.moves.is_empty() && def.attack.is_none() {
        return Err("has moves but no plain `attack`".into());
    }
    let mut last = 1.0f32;
    for phase in &def.phases {
        if !(phase.below > 0.0 && phase.below < last) || !(phase.hold.is_finite() && phase.hold >= 0.0) {
            return Err("phases' `below` must fall from under 1 toward 0, each with a `hold` of 0 or more seconds".into());
        }
        last = phase.below;
    }
    for (i, m) in def.moves.iter().enumerate() {
        if def.moves[..i].iter().any(|o| o.name == m.name) {
            return Err(format!("has two moves named `{}`", m.name));
        }
        if let Some(&p) = m.phases.iter().find(|&&p| p as usize > def.phases.len()) {
            return Err(format!("move `{}` is used in phase {p}, which it does not have", m.name));
        }
        if !(m.least >= 0.0 && m.least < m.attack.reach) {
            return Err(format!("move `{}` needs its `least` from 0 to under its reach", m.name));
        }
        if let Some(charge) = m.attack.charge {
            check_charge(&m.name, charge)?;
        }
        let t = m.tell;
        if ![t.nod, t.crouch, t.rear].iter().all(|v| (0.0..=1.0).contains(v)) {
            return Err(format!("move `{}`'s tell must be from 0 to 1", m.name));
        }
    }
    if let Some(charge) = def.attack.and_then(|a| a.charge) {
        check_charge("attack", charge)?;
    }
    if let Some(mend) = &def.mend {
        if mend.health == 0 || !(mend.every > 0.0) || !(mend.after >= 0.0) || !(mend.fire_reach >= 0.0) || mend.fire.is_empty() != (mend.fire_reach == 0.0) {
            return Err("`mend` needs health, a positive `every`, an `after` of 0 or more, and `fire` tags with a `fire_reach` (or neither)".into());
        }
        if mend.daylight.is_some_and(|d| !(0.0..=1.0).contains(&d)) {
            return Err("`mend.daylight` must be from 0 to 1".into());
        }
    }
    Ok(())
}

fn check_charge(name: &str, charge: Charge) -> Result<(), String> {
    if !(charge.speed > 0.0 && charge.speed <= 32.0 && charge.distance > 0.0 && charge.distance <= 64.0 && charge.stun >= 0.0 && charge.stun <= 30.0) {
        return Err(format!("`{name}`'s charge needs a speed from above 0 to 32, a distance from above 0 to 64 and a stun from 0 to 30 seconds"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use glam::Vec3;

    fn content() -> Content {
        Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads")
    }

    fn rootmother(content: &Content) -> &crate::creature::Creature {
        content.creatures.get(content.creatures.id("base:rootmother").expect("the Rootmother"))
    }

    fn index(creature: &crate::creature::Creature, name: &str) -> u8 {
        creature.def.moves.iter().position(|m| m.name == name).expect("the move") as u8 + 1
    }

    /// Her lash and her grasp take turns at a player beside her; below half she thrashes
    /// in place of the lash, and a player who keeps away (a sling or a bow) draws her
    /// volley, which a player at her feet never does.
    #[test]
    fn the_rootmother_takes_turns_by_phase_and_distance() {
        let content = content();
        let her = rootmother(&content);
        let [lash, thrash, grasp, volley] = ["lash", "thrash", "grasp", "volley"].map(|m| index(her, m));
        assert_eq!((her.phase(1.0), her.phase(0.51), her.phase(0.5), her.phase(0.1)), (0, 0, 1, 1));
        // Beside her, the pattern to learn: lash, grasp, lash.
        assert_eq!(her.choose(0, 1.0, 0), Some(lash));
        assert_eq!(her.choose(0, 1.0, lash), Some(grasp));
        assert_eq!(her.choose(0, 1.0, grasp), Some(lash));
        // Pressed against her, or inside her edge, her close moves still fit, never a plain
        // blow with no tell.
        assert_eq!(her.choose(0, 0.0, 0), Some(lash));
        assert_eq!(her.choose(0, -0.4, lash), Some(grasp));
        assert!(her.reaches(0, -0.4));
        // Past her reach in the first phase, nothing reaches; in the second, the volley.
        assert_eq!(her.choose(0, 6.0, lash), None);
        assert_eq!(her.choose(1, 6.0, lash), Some(volley));
        assert_eq!(her.choose(1, 1.0, grasp), Some(thrash));
        assert_eq!(her.choose(1, 1.0, thrash), Some(grasp));
        assert!(her.longest_reach(1) > 10.0 && her.longest_reach(0) < 3.0, "she strikes from afar only once awake");
        // The grasp goes through any guard: dodge it.
        assert!(her.attack_at(grasp).is_some_and(|a| a.heavy) && her.attack_at(lash).is_some_and(|a| !a.heavy));
        assert_eq!(her.attack_at(volley).map(|a| a.style), Some(crate::combat::Style::Ranged));
        assert!(her.tell_at(grasp) != her.tell_at(lash) && her.tell_at(volley) != her.tell_at(lash), "each move reads apart");
    }

    /// Old Tusk gores a player beside him and charges one who keeps away; below half his
    /// charge is the quicker frenzy. Each charge is heavy (a guard does not take it), its
    /// crouch reads apart from the gore's nod, and a spear's guard braces against it where a
    /// sword's does not. He roams rather than keeps a room, never starts a fight, and a
    /// truffle tempts him.
    #[test]
    fn old_tusk_gores_up_close_and_charges_from_afar() {
        let content = content();
        let tusk = content.creatures.get(content.creatures.id("base:old_tusk").expect("Old Tusk"));
        let [gore, charge, frenzy] = ["gore", "charge", "frenzy"].map(|m| index(tusk, m));
        assert_eq!(tusk.choose(0, 0.5, 0), Some(gore));
        assert_eq!(tusk.choose(0, 6.0, 0), Some(charge));
        assert_eq!(tusk.choose(1, 6.0, 0), Some(frenzy));
        assert_eq!(tusk.choose(1, 0.5, frenzy), Some(gore));
        for i in [charge, frenzy] {
            let attack = tusk.attack_at(i).unwrap();
            assert!(attack.heavy && attack.charge.is_some(), "a charge goes through any guard");
        }
        assert!(tusk.attack_at(frenzy).unwrap().charge.unwrap().speed > tusk.attack_at(charge).unwrap().charge.unwrap().speed);
        assert!(tusk.tell_at(charge) != tusk.tell_at(gore));
        let guard = |item: &str| content.combat.guard_of(content.blocks.id(item), &[], |_| 1).expect("it guards");
        assert!(guard("base:flint_spear").braces(Vec3::Z, Vec3::Z) && !guard("base:flint_spear").braces(Vec3::Z, -Vec3::Z));
        assert!(!guard("base:stone_sword").braces(Vec3::Z, Vec3::Z));
        let def = &tusk.def;
        assert!(def.boss && !def.habitat.party_only && def.temperament.aggression == 0.0);
        assert!(tusk.favourite.contains(&content.blocks.id("base:truffle").unwrap()));
    }

    /// Cut her and she grows back, unless a fire burns near her or the sun is on her.
    #[test]
    fn fire_and_the_sun_stop_the_rootmother_mending() {
        let content = content();
        let mend = rootmother(&content).def.mend.clone().expect("she mends");
        let dark = MendStoppers { fire: false, daylight: 0.0, open_sky: false };
        assert!(mend.mends(dark));
        assert!(!mend.mends(MendStoppers { fire: true, ..dark }), "a fire near her stops it");
        assert!(!mend.mends(MendStoppers { daylight: 1.0, open_sky: true, ..dark }), "so does the sun on her");
        assert!(mend.mends(MendStoppers { daylight: 1.0, open_sky: false, ..dark }), "not the day over her hollow's roof");
        assert!(mend.mends(MendStoppers { daylight: 0.1, open_sky: true, ..dark }), "nor the night sky");
        assert!(mend.fire.iter().any(|t| t == "fire"));
    }

    /// A boss never flees, however hurt.
    #[test]
    fn a_boss_fights_to_the_end() {
        let content = content();
        let her = rootmother(&content);
        let mut mind = crate::creature::brain::Mind::new(&her.def.temperament, 7);
        mind.hurt(her, Some(1), 0.05);
        assert!(mind.anger > 0.0 && mind.fear == 0.0);
    }

    /// Each phase a creature comes to is told to players near in every language the game
    /// has (`boss.NS.NAME.phase.N`).
    #[test]
    fn every_phase_is_told_in_every_language() {
        let content = content();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base/lang");
        for code in ["en", "ja"] {
            let lang: std::collections::HashMap<String, String> = ron::from_str(&std::fs::read_to_string(dir.join(format!("{code}.ron"))).unwrap()).unwrap();
            for (_, creature) in content.creatures.iter() {
                let (ns, short) = creature.def.name.split_once(':').unwrap();
                for phase in 1..=creature.def.phases.len() {
                    let key = format!("boss.{ns}.{short}.phase.{phase}");
                    assert!(lang.contains_key(&key), "{code}: `{key}` is missing");
                }
            }
        }
    }

    #[test]
    fn moves_phases_and_mending_are_checked() {
        let content = content();
        let mut def = rootmother(&content).def.clone();
        assert_eq!(check(&def), Ok(()));
        let mut bad = def.clone();
        bad.attack = None;
        assert!(check(&bad).unwrap_err().contains("no plain `attack`"));
        let mut bad = def.clone();
        bad.phases.push(Phase { below: 0.7, hold: 1.0 });
        assert!(check(&bad).unwrap_err().contains("phases"));
        let mut bad = def.clone();
        bad.moves[0].phases = vec![3];
        assert!(check(&bad).unwrap_err().contains("phase 3"));
        let mut bad = def.clone();
        bad.moves[1].name = bad.moves[0].name.clone();
        assert!(check(&bad).unwrap_err().contains("two moves"));
        def.mend.as_mut().unwrap().fire.clear();
        assert!(check(&def).unwrap_err().contains("mend"));
    }
}
