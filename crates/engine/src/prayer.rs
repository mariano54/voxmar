//! Prayers (the design wiki's Combat feel tab, draft 87): Magic spells that draw on prayer
//! points instead of runes. A player has 10 points and one more a Magic level; each prayer
//! on drains its points by the minute, and at none left every prayer goes out. Points come
//! back in full at an altar (a block tagged `altar`), and slowly out of a fight with no
//! prayer on. One ward is on at a time, and one boost of each kind: the one turned on last
//! puts out any it shares a kind with. Turning prayers on or off costs no beat.
//!
//! Every number and the book itself are content: each package's `prayers.ron`. Its `rules`
//! are laid field by field over the last package's; a prayer a later package names again
//! replaces the earlier one in its place.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::combat::Style;
use crate::skill::{SkillId, SkillRegistry};

/// A prayer, by its place in the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, Deserialize)]
pub struct PrayerId(pub u16);

/// What a boost raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum Stat {
    /// The Defence roll against blows.
    Defence,
    /// Melee max hit.
    Strength,
    /// Melee accuracy.
    Accuracy,
}

impl Stat {
    /// Its lang key's last part (`prayer.effect.boost.<key>`).
    pub fn key(self) -> &'static str {
        match self {
            Stat::Defence => "defence",
            Stat::Strength => "strength",
            Stat::Accuracy => "accuracy",
        }
    }
}

/// What a prayer does while it is on.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Effect {
    /// Raises a stat by a share.
    Boost { stat: Stat, share: f32 },
    /// Stops the share of the rules' `wards` of blows in these styles.
    Ward { styles: Vec<Style> },
    /// Death keeps this many more (to `value.ron`'s `death.most`).
    Keep(u8),
    /// Health comes back this many times as fast.
    Mending(f32),
    /// Under `below` of the most health, a blow is followed by a heal of `heal` of it, and
    /// every prayer point goes.
    LastBreath { below: f32, heal: f32 },
}

impl Effect {
    /// The kind it fills: two prayers that share one are never on together.
    fn kinds(&self) -> Vec<Kind> {
        match self {
            Effect::Boost { stat, .. } => vec![Kind::Boost(*stat)],
            Effect::Ward { .. } => vec![Kind::Ward],
            Effect::Keep(_) => vec![Kind::Keep],
            Effect::Mending(_) => vec![Kind::Mending],
            Effect::LastBreath { .. } => vec![Kind::LastBreath],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Boost(Stat),
    Ward,
    Keep,
    Mending,
    LastBreath,
}

/// One prayer in the book.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrayerDef {
    /// Namespaced on load (`base:ward_stab`); its words are `prayer.<package>.<name>`.
    pub name: String,
    /// The Magic level it opens at.
    pub level: u8,
    /// Points it drains a minute while on.
    pub drain: f32,
    pub effects: Vec<Effect>,
}

/// Declares [`Rules`] with its defaults, and [`RulesPatch`], what one package sets.
macro_rules! rules {
    ($($(#[$doc:meta])* $field:ident: $ty:ty = $default:expr,)*) => {
        /// Prayer points and what wards stop, as content sets them.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Rules {
            $($(#[$doc])* pub $field: $ty,)*
        }

        impl Default for Rules {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        /// One package's `rules`: the fields it sets.
        #[derive(Clone, Copy, Debug, Default, Deserialize)]
        #[serde(deny_unknown_fields, default)]
        pub struct RulesPatch {
            $(pub $field: Option<$ty>,)*
        }

        impl Rules {
            fn patch(&mut self, patch: &RulesPatch) {
                $(if let Some(value) = patch.$field {
                    self.$field = value;
                })*
            }
        }
    };
}

rules! {
    /// Points at Magic level 0, and how many more each level brings (10 + level).
    points: f32 = 10.0,
    per_level: f32 = 1.0,
    /// With no prayer on and no blow either way for `calm` seconds, a point comes back
    /// every `regain` seconds.
    calm: f32 = 10.0,
    regain: f32 = 20.0,
    /// The share of a creature's blow in its style a ward stops, and of a boss's (a boss
    /// can't be fought untouched behind a prayer; the wiki's 50%). Players' blows get
    /// their own share when there are player fights to ward off.
    ward: f32 = 0.8,
    boss_ward: f32 = 0.5,
    /// Magic experience for each bone offered at an altar, times the offering's worth.
    offer_xp: f32 = 16.0,
}

/// One package's `prayers.ron`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PrayerFile {
    pub rules: Option<RulesPatch>,
    pub prayers: Vec<PrayerDef>,
    /// What an altar takes for Magic experience, and how many bones each is worth.
    pub offerings: Vec<Offering>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offering {
    pub item: String,
    pub worth: f32,
}

/// The prayer book, the rules, and what an altar takes.
#[derive(Clone, Debug, Default)]
pub struct PrayerRegistry {
    pub rules: Rules,
    prayers: Vec<PrayerDef>,
    offerings: Vec<(BlockId, f32)>,
    /// The skill prayers belong to (`core:magic`), if the content has it.
    pub skill: Option<SkillId>,
}

impl PrayerRegistry {
    pub fn read(namespace: &str, source: &str) -> Result<PrayerFile, RegistryError> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let mut file: PrayerFile = options.from_str(source).map_err(|e| RegistryError(format!("{namespace}/prayers.ron: {e}")))?;
        for prayer in &mut file.prayers {
            if !prayer.name.contains(':') {
                prayer.name = format!("{namespace}:{}", prayer.name);
            }
        }
        for offering in &mut file.offerings {
            if !offering.item.contains(':') {
                offering.item = format!("{namespace}:{}", offering.item);
            }
        }
        Ok(file)
    }

    pub fn new(files: Vec<PrayerFile>, blocks: &BlockRegistry, skills: &SkillRegistry) -> Result<Self, RegistryError> {
        let mut rules = Rules::default();
        let mut prayers: Vec<PrayerDef> = Vec::new();
        let mut offerings = Vec::new();
        for file in files {
            if let Some(patch) = &file.rules {
                rules.patch(patch);
            }
            for prayer in file.prayers {
                match prayers.iter_mut().find(|p| p.name == prayer.name) {
                    Some(earlier) => *earlier = prayer,
                    None => prayers.push(prayer),
                }
            }
            offerings.extend(file.offerings);
        }
        let wrong = |what: String| RegistryError(format!("prayers.ron: {what}"));
        if !(0.0..=1.0).contains(&rules.ward) || !(0.0..=1.0).contains(&rules.boss_ward) || rules.points < 1.0 || rules.per_level < 0.0 || rules.calm < 0.0 || rules.regain <= 0.0 || rules.offer_xp < 0.0 {
            return Err(wrong("a ward stops a share from 0 to 1, points start at 1 or more, and no time or experience is negative".into()));
        }
        if prayers.len() > u16::MAX as usize {
            return Err(wrong("too many prayers".into()));
        }
        for prayer in &prayers {
            if prayer.drain < 0.0 || prayer.effects.is_empty() {
                return Err(wrong(format!("`{}` drains nothing negative and does something", prayer.name)));
            }
            for effect in &prayer.effects {
                let fine = match effect {
                    Effect::Boost { share, .. } => (0.0..=1.0).contains(share),
                    Effect::Ward { styles } => !styles.is_empty(),
                    Effect::Keep(more) => *more > 0,
                    Effect::Mending(times) => *times >= 1.0,
                    Effect::LastBreath { below, heal } => (0.0..1.0).contains(below) && (0.0..=1.0).contains(heal) && *heal > 0.0,
                };
                if !fine {
                    return Err(wrong(format!("`{}`: {effect:?} is out of bounds (shares 0 to 1, a ward names a style, mending at least once as fast)", prayer.name)));
                }
            }
            let kinds: Vec<Kind> = prayer.effects.iter().flat_map(Effect::kinds).collect();
            if kinds.iter().enumerate().any(|(i, k)| kinds[..i].contains(k)) {
                return Err(wrong(format!("`{}` does one kind of thing twice", prayer.name)));
            }
        }
        let offerings = offerings
            .into_iter()
            .map(|o| match blocks.id(&o.item) {
                Some(item) if o.worth > 0.0 => Ok((item, o.worth)),
                Some(_) => Err(wrong(format!("the offering `{}` is worth nothing", o.item))),
                None => Err(wrong(format!("the offering `{}` is no item", o.item))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { rules, prayers, offerings, skill: skills.id("core:magic") })
    }

    pub fn len(&self) -> usize {
        self.prayers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prayers.is_empty()
    }

    pub fn get(&self, id: PrayerId) -> Option<&PrayerDef> {
        self.prayers.get(id.0 as usize)
    }

    pub fn id(&self, name: &str) -> Option<PrayerId> {
        self.prayers.iter().position(|p| p.name == name).map(|i| PrayerId(i as u16))
    }

    /// Every prayer, in the book's order (by level, as content lists them).
    pub fn all(&self) -> impl Iterator<Item = (PrayerId, &PrayerDef)> {
        self.prayers.iter().enumerate().map(|(i, p)| (PrayerId(i as u16), p))
    }

    /// The most points at a Magic level.
    pub fn most(&self, level: u8) -> f32 {
        self.rules.points + self.rules.per_level * level as f32
    }

    /// How many bones an item is worth at an altar, if it takes it.
    pub fn offering(&self, item: BlockId) -> Option<f32> {
        self.offerings.iter().find(|(i, _)| *i == item).map(|&(_, worth)| worth)
    }

    /// The prayers on after turning `id` on among `on`: it, and those it shares no kind with.
    pub fn turn_on(&self, on: &[PrayerId], id: PrayerId) -> Vec<PrayerId> {
        let Some(new) = self.get(id) else { return on.to_vec() };
        let kinds: Vec<Kind> = new.effects.iter().flat_map(Effect::kinds).collect();
        let mut after: Vec<PrayerId> = on.iter().copied().filter(|&other| other != id && self.get(other).is_some_and(|p| !p.effects.iter().flat_map(Effect::kinds).any(|k| kinds.contains(&k)))).collect();
        after.push(id);
        after.sort();
        after
    }

    /// The effects of the prayers on.
    fn effects<'a>(&'a self, on: &'a [PrayerId]) -> impl Iterator<Item = &'a Effect> {
        on.iter().filter_map(|&id| self.get(id)).flat_map(|p| p.effects.iter())
    }

    /// What the prayers on raise `stat` by, as a share.
    pub fn boost(&self, on: &[PrayerId], stat: Stat) -> f32 {
        self.effects(on).map(|e| if let Effect::Boost { stat: s, share } = e { if *s == stat { *share } else { 0.0 } } else { 0.0 }).sum()
    }

    /// Whether a ward on stops blows in `style`.
    pub fn warded(&self, on: &[PrayerId], style: Style) -> bool {
        self.effects(on).any(|e| matches!(e, Effect::Ward { styles } if styles.contains(&style)))
    }

    /// How many more death keeps.
    pub fn keeps(&self, on: &[PrayerId]) -> u8 {
        self.effects(on).map(|e| if let Effect::Keep(more) = e { *more } else { 0 }).sum()
    }

    /// How many times as fast health comes back.
    pub fn mending(&self, on: &[PrayerId]) -> f32 {
        self.effects(on).map(|e| if let Effect::Mending(times) = e { *times } else { 1.0 }).fold(1.0, f32::max)
    }

    /// The last breath on, if any: below what share of health, and the share it heals.
    pub fn last_breath(&self, on: &[PrayerId]) -> Option<(f32, f32)> {
        self.effects(on).find_map(|e| if let Effect::LastBreath { below, heal } = e { Some((*below, *heal)) } else { None })
    }

    /// Points the prayers on drain a second.
    pub fn drain(&self, on: &[PrayerId]) -> f32 {
        on.iter().filter_map(|&id| self.get(id)).map(|p| p.drain / 60.0).sum()
    }
}

/// A player's prayer points and the prayers they have on.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
#[serde(default)]
pub struct Praying {
    pub points: f32,
    pub on: Vec<PrayerId>,
    /// Seconds towards the next point back, out of a fight with nothing on.
    #[serde(skip)]
    pub regain_clock: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    fn blocks() -> BlockRegistry {
        BlockRegistry::new(vec![BlockDef { name: "t:air".into(), ..Default::default() }]).unwrap()
    }

    fn book() -> PrayerRegistry {
        let source = r#"(
            rules: (calm: 8.0),
            prayers: [
                (name: "bark_skin", level: 1, drain: 5.0, effects: [Boost(stat: Defence, share: 0.05)]),
                (name: "stone_skin", level: 20, drain: 10.0, effects: [Boost(stat: Defence, share: 0.1)]),
                (name: "forge_arm", level: 22, drain: 10.0, effects: [Boost(stat: Strength, share: 0.1)]),
                (name: "ward_stab", level: 12, drain: 20.0, effects: [Ward(styles: [Stab])]),
                (name: "ward_melee", level: 46, drain: 24.0, effects: [Ward(styles: [Stab, Slash, Crush])]),
                (name: "keepsake", level: 25, drain: 2.0, effects: [Keep(1)]),
            ],
        )"#;
        let later = r#"(rules: (ward: 0.75), prayers: [(name: "base:bark_skin", level: 1, drain: 4.0, effects: [Boost(stat: Defence, share: 0.05)])])"#;
        let files = vec![PrayerRegistry::read("base", source).unwrap(), PrayerRegistry::read("more", later).unwrap()];
        PrayerRegistry::new(files, &blocks(), &SkillRegistry::default()).unwrap()
    }

    #[test]
    fn one_ward_and_one_boost_of_each_kind_the_last_turned_on_winning() {
        let book = book();
        let id = |name: &str| book.id(&format!("base:{name}")).unwrap();
        let on = book.turn_on(&[], id("bark_skin"));
        let on = book.turn_on(&on, id("forge_arm"));
        let on = book.turn_on(&on, id("ward_stab"));
        let on = book.turn_on(&on, id("keepsake"));
        // A higher defence boost puts out the lower; the melee ward puts out the stab ward.
        let on = book.turn_on(&on, id("stone_skin"));
        let on = book.turn_on(&on, id("ward_melee"));
        let mut want = vec![id("stone_skin"), id("forge_arm"), id("ward_melee"), id("keepsake")];
        want.sort();
        assert_eq!(on, want);
        assert_eq!((book.boost(&on, Stat::Defence), book.boost(&on, Stat::Accuracy)), (0.1, 0.0));
        assert!(book.warded(&on, Style::Crush) && !book.warded(&on, Style::Magic));
        assert_eq!(book.keeps(&on), 1);
        assert!((book.drain(&on) - (10.0 + 10.0 + 24.0 + 2.0) / 60.0).abs() < 1e-6);
    }

    #[test]
    fn later_packages_patch_the_rules_and_replace_a_prayer_in_its_place() {
        let book = book();
        assert_eq!((book.rules.calm, book.rules.ward), (8.0, 0.75));
        assert_eq!(book.id("base:bark_skin"), Some(PrayerId(0)));
        assert_eq!(book.get(PrayerId(0)).unwrap().drain, 4.0);
        assert_eq!(book.most(80), 90.0, "10 + Magic");
    }

    #[test]
    fn a_book_out_of_bounds_is_refused_on_load() {
        let refused = |source: &str| PrayerRegistry::new(vec![PrayerRegistry::read("t", source).unwrap()], &blocks(), &SkillRegistry::default()).is_err();
        assert!(refused("(rules: (ward: 1.5))"));
        assert!(refused("(rules: (regain: 0.0))"));
        assert!(refused(r#"(prayers: [(name: "x", level: 1, drain: -1.0, effects: [Keep(1)])])"#));
        assert!(refused(r#"(prayers: [(name: "x", level: 1, drain: 1.0, effects: [])])"#));
        assert!(refused(r#"(prayers: [(name: "x", level: 1, drain: 1.0, effects: [Ward(styles: [])])])"#));
        assert!(refused(r#"(prayers: [(name: "x", level: 1, drain: 1.0, effects: [Boost(stat: Defence, share: 0.1), Boost(stat: Defence, share: 0.1)])])"#));
        assert!(refused(r#"(offerings: [(item: "nothing", worth: 1.0)])"#));
        assert!(!refused(r#"(prayers: [(name: "x", level: 1, drain: 1.0, effects: [LastBreath(below: 0.1, heal: 0.25)])])"#));
    }
}
