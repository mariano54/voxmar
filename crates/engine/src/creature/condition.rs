//! A kept animal's condition (docs/breeding.md, "Keeping animals"): how well it is kept,
//! from 0 to 1. Its target is the product of its needs met (food, water, shelter, space,
//! climate, company), each counting 1 met, a little less partly met and half unmet; below
//! half health it is held to half. Condition walks toward the target slowly, a little each
//! in-game day, faster down than up, so one rainy night or a missed meal shows only over
//! days. What a kept animal gives scales with it: products, litters and the power it walks
//! into a treadwheel, by `output` (0.5 to 1.2 by default).
//!
//! The rates are data: a package's `creatures.ron` may set any of them (`condition`), as it
//! sets the creature limits, a later package's replacing an earlier one's field by field
//! (Design draft 91); each species may say what it needs (`needs`).

use serde::Deserialize;

use crate::block::RegistryError;

/// How condition is reckoned. Rates are per in-game day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConditionRules {
    /// How fast it rises toward its target, and falls.
    pub up: f32,
    pub down: f32,
    /// What a need counts for, met, partly met and unmet.
    pub met: f32,
    pub partly: f32,
    pub unmet: f32,
    /// Where a newly tamed animal starts (a newborn starts at its parents' average).
    pub tamed: f32,
    /// The most it reaches below half health.
    pub hurt: f32,
    /// Outputs scale by `output.0 + output.1 × condition`.
    pub output: (f32, f32),
    /// Breeding takes condition above this.
    pub breed: f32,
    /// Days since it last ate: within the first fed, within the second partly.
    pub fed: (f32, f32),
    /// How far around it others count against its room, in blocks.
    pub space_radius: f32,
    /// Within how many blocks one of its kind keeps a social animal company.
    pub company: f32,
    /// How well its home's climate suits it (its comfort fit there, 0 to 1): at least the
    /// first is met, at least the second partly.
    pub climate: (f32, f32),
    /// Rain or snow heavier than this, falling on it under open sky, wants a roof.
    pub wet: f32,
    /// How near a roof, in blocks, it can shelter under.
    pub roof_within: f32,
    /// Snow counts as rain (partly met) for a kind whose comfort reaches this cold, in °C,
    /// and as unmet for the rest.
    pub snow_hardy: f32,
}

impl Default for ConditionRules {
    fn default() -> Self {
        Self {
            up: 0.15,
            down: 0.25,
            met: 1.0,
            partly: 0.8,
            unmet: 0.5,
            tamed: 0.6,
            hurt: 0.5,
            output: (0.5, 0.7),
            breed: 0.5,
            fed: (1.0, 2.0),
            space_radius: 5.0,
            company: 14.0,
            climate: (0.75, 0.25),
            wet: 0.05,
            roof_within: 3.0,
            snow_hardy: -10.0,
        }
    }
}

crate::content::settable! {
    /// What one package sets of the condition rules.
    SetConditionRules for ConditionRules {
        up: f32, down: f32, met: f32, partly: f32, unmet: f32, tamed: f32, hurt: f32,
        output: (f32, f32), breed: f32, fed: (f32, f32), space_radius: f32, company: f32,
        climate: (f32, f32), wet: f32, roof_within: f32, snow_hardy: f32,
    }
}

impl ConditionRules {
    /// Whether these rules make sense: shares within 0..1 in order, rates and reaches above
    /// zero, outputs never below nothing.
    pub fn check(&self) -> Result<(), RegistryError> {
        let error = |why: &str| Err(RegistryError(format!("condition: {why}")));
        let share = |v: f32| (0.0..=1.0).contains(&v);
        if ![self.met, self.partly, self.unmet, self.tamed, self.hurt, self.breed].into_iter().all(share) {
            return error("met, partly, unmet, tamed, hurt and breed must be within 0 and 1");
        }
        if !(self.unmet <= self.partly && self.partly <= self.met) {
            return error("a need met must count at least as much as partly met, and that as unmet");
        }
        if !(self.up > 0.0 && self.down > 0.0 && self.space_radius > 0.0) {
            return error("up, down and space_radius must be above 0");
        }
        if !(self.output.0 >= 0.0 && self.output.0 + self.output.1 >= 0.0) {
            return error("output must not fall below 0");
        }
        if !(0.0 <= self.fed.0 && self.fed.0 <= self.fed.1) {
            return error("fed must be two day counts, the first no more than the second");
        }
        if !(share(self.climate.0) && share(self.climate.1) && self.climate.1 <= self.climate.0) {
            return error("climate must be two fits within 0 and 1, the first no less than the second");
        }
        if !(self.company > 0.0 && self.wet >= 0.0 && self.roof_within >= 0.0 && self.snow_hardy.is_finite()) {
            return error("company must be above 0, wet and roof_within 0 or more, snow_hardy a temperature");
        }
        Ok(())
    }

    /// What one need counts for.
    pub fn share(&self, need: Met) -> f32 {
        match need {
            Met::Yes => self.met,
            Met::Partly => self.partly,
            Met::No => self.unmet,
        }
    }

    /// The condition its needs aim it at, held to `hurt` below half health.
    pub fn target(&self, needs: &Needs, hurt: bool) -> f32 {
        let target = needs.all().into_iter().map(|(_, met)| self.share(met)).product::<f32>();
        if hurt { target.min(self.hurt) } else { target }
    }

    /// Condition after `days` in-game days aiming at `target`.
    pub fn step(&self, condition: f32, target: f32, days: f32) -> f32 {
        let days = days.max(0.0);
        (condition + (target - condition).clamp(-self.down * days, self.up * days)).clamp(0.0, 1.0)
    }

    /// How much of its usual output an animal in `condition` gives: 0.5 to 1.2 by default.
    pub fn output(&self, condition: f32) -> f32 {
        (self.output.0 + self.output.1 * condition.clamp(0.0, 1.0)).max(0.0)
    }

    /// How well its home's climate suits it, from its comfort fit there.
    pub fn climate(&self, fit: f32) -> Met {
        if fit >= self.climate.0 {
            Met::Yes
        } else if fit >= self.climate.1 {
            Met::Partly
        } else {
            Met::No
        }
    }

    /// How well it has eaten, `days` after its last meal.
    pub fn fed(&self, days: f32) -> Met {
        if days <= self.fed.0 {
            Met::Yes
        } else if days <= self.fed.1 {
            Met::Partly
        } else {
            Met::No
        }
    }
}

/// How well one need is met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Met {
    Yes,
    Partly,
    No,
}

/// Each need, as last looked at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Needs {
    pub food: Met,
    pub water: Met,
    pub shelter: Met,
    pub space: Met,
    pub climate: Met,
    pub company: Met,
}

impl Default for Needs {
    fn default() -> Self {
        Self { food: Met::Yes, water: Met::Yes, shelter: Met::Yes, space: Met::Yes, climate: Met::Yes, company: Met::Yes }
    }
}

impl Needs {
    /// Every need by name (`food`, `water`, `shelter`, `space`, `climate`, `company`).
    pub fn all(&self) -> [(&'static str, Met); 6] {
        [
            ("food", self.food),
            ("water", self.water),
            ("shelter", self.shelter),
            ("space", self.space),
            ("climate", self.climate),
            ("company", self.company),
        ]
    }
}

/// What a species needs when kept, beyond food, water and its climate, which every one
/// needs. Unset, its room comes from its size and its want of company from how social it is.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NeedsDef {
    /// Blocks of ground each one wants to itself (chickens 2, sheep 6, cattle 12).
    pub space: Option<f32>,
    /// Whether it minds rain and snow without a roof (ducks do not).
    pub shelter: Option<bool>,
    /// Whether it wants one of its kind nearby.
    pub company: Option<bool>,
}

impl NeedsDef {
    /// Its room: given, or about eight blocks for each block of its width, between 2 and 16.
    pub fn space(&self, width: f32) -> f32 {
        self.space.unwrap_or_else(|| (8.0 * width).round().clamp(2.0, 16.0))
    }

    pub fn shelter(&self) -> bool {
        self.shelter.unwrap_or(true)
    }

    /// Whether it wants company: given, or if it is social (sociability 0.5 or more).
    pub fn company(&self, sociability: f32) -> bool {
        self.company.unwrap_or(sociability >= 0.5)
    }
}

/// How well its room is met: the ground within reach (`area` blocks) against what every
/// kept animal on it wants (`wanted`, its own included). Up to twice crowded is partly met.
pub fn room(area: f32, wanted: f32) -> Met {
    if wanted <= area {
        Met::Yes
    } else if wanted <= 2.0 * area {
        Met::Partly
    } else {
        Met::No
    }
}

/// How well its want of shelter is met: `wet` is what falls on it under open sky
/// (`Some(true)` snow, `Some(false)` rain, `None` nothing), `roof` whether one is near enough
/// to shelter under, `hardy` whether its kind is made for the cold (snow is then no worse
/// than rain).
pub fn shelter(wants_roof: bool, wet: Option<bool>, roof: bool, hardy: bool) -> Met {
    match wet {
        None => Met::Yes,
        Some(_) if !wants_roof || roof => Met::Yes,
        Some(true) if !hardy => Met::No,
        Some(_) => Met::Partly,
    }
}

/// Its condition in a word, for players: `poor` (too poorly kept to breed, at or below
/// `breed`), `fair`, `good` or `excellent`.
pub fn word(condition: f32, breed: f32) -> &'static str {
    match condition {
        c if c <= breed => "poor",
        c if c < 0.6 => "fair",
        c if c < 0.85 => "good",
        _ => "excellent",
    }
}

/// Which way its condition is heading, for players: toward `target` by more than a hair
/// (`Some(true)` improving, `Some(false)` getting worse), or holding.
pub fn trend(condition: f32, target: f32) -> Option<bool> {
    const HAIR: f32 = 0.01;
    if target > condition + HAIR {
        Some(true)
    } else if target < condition - HAIR {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_is_the_product_of_needs_met() {
        let rules = ConditionRules::default();
        assert_eq!(rules.target(&Needs::default(), false), 1.0);
        let thirsty = Needs { water: Met::No, ..Default::default() };
        assert_eq!(rules.target(&thirsty, false), 0.5);
        let crowded = Needs { space: Met::Partly, shelter: Met::Partly, ..Default::default() };
        assert!((rules.target(&crowded, false) - 0.64).abs() < 1e-6);
        // Below half health it is held to half, however well kept.
        assert_eq!(rules.target(&Needs::default(), true), 0.5);
    }

    #[test]
    fn condition_moves_slowly_and_falls_faster_than_it_rises() {
        let rules = ConditionRules::default();
        // A tamed animal kept well reaches its best in under three days.
        let mut c = rules.tamed;
        let mut days = 0;
        while c < 1.0 - 1e-5 {
            c = rules.step(c, 1.0, 1.0);
            days += 1;
        }
        assert_eq!(days, 3);
        // One day without water costs a quarter, not all of it.
        assert!((rules.step(1.0, 0.5, 1.0) - 0.75).abs() < 1e-6);
        // Never past its target.
        assert_eq!(rules.step(0.9, 1.0, 5.0), 1.0);
        assert_eq!(rules.step(0.6, 0.5, 5.0), 0.5);
        assert_eq!(rules.step(0.6, 1.0, -1.0), 0.6);
    }

    #[test]
    fn output_runs_from_half_to_six_fifths() {
        let rules = ConditionRules::default();
        assert_eq!(rules.output(0.0), 0.5);
        assert!((rules.output(1.0) - 1.2).abs() < 1e-6);
        // A treadwheel's 8 power, as Horse power has it: 4 to 10 (about 9.6).
        assert_eq!(8.0 * rules.output(0.0), 4.0);
        assert!((8.0 * rules.output(1.0) - 9.6).abs() < 1e-5);
    }

    #[test]
    fn meals_rooms_and_species_needs_read_as_written() {
        let rules = ConditionRules::default();
        assert_eq!([0.5, 1.5, 2.5].map(|d| rules.fed(d)), [Met::Yes, Met::Partly, Met::No]);
        // 78 blocks within 5 of it: room for 13 sheep at 6 each, half again partly.
        let area = std::f32::consts::PI * 25.0;
        assert_eq!([13.0, 26.0, 27.0].map(|sheep| room(area, sheep * 6.0)), [Met::Yes, Met::Partly, Met::No]);
        assert_eq!([0.9, 0.5, 0.1].map(|fit| rules.climate(fit)), [Met::Yes, Met::Partly, Met::No]);
        let needs = NeedsDef::default();
        assert_eq!((needs.space(0.4), needs.space(0.9), needs.space(1.4), needs.space(3.0)), (3.0, 7.0, 11.0, 16.0));
        assert!(needs.shelter() && needs.company(0.6) && !needs.company(0.3));
        let duck = NeedsDef { shelter: Some(false), space: Some(2.0), company: Some(true) };
        assert!(!duck.shelter() && duck.company(0.0) && duck.space(5.0) == 2.0);
        // Poor is too poorly kept to breed: at or below the breeding line.
        assert_eq!([0.2, 0.5, 0.55, 0.7, 0.9].map(|c| word(c, rules.breed)), ["poor", "poor", "fair", "good", "excellent"]);
        // Dry, under a roof or minding no rain: met. Rain in the open: partly. Snow in the
        // open: unmet, but partly for a kind made for the cold.
        assert_eq!(shelter(true, None, false, false), Met::Yes);
        assert_eq!(shelter(true, Some(true), true, false), Met::Yes);
        assert_eq!(shelter(false, Some(true), false, false), Met::Yes);
        assert_eq!(shelter(true, Some(false), false, false), Met::Partly);
        assert_eq!(shelter(true, Some(true), false, false), Met::No);
        assert_eq!(shelter(true, Some(true), false, true), Met::Partly);
        assert_eq!([(0.6, 0.8), (0.6, 0.5), (0.6, 0.605)].map(|(c, t)| trend(c, t)), [Some(true), Some(false), None]);
    }

    #[test]
    fn rules_that_cannot_hold_fail_to_load() {
        assert!(ConditionRules::default().check().is_ok());
        let bad = [
            ConditionRules { met: 1.5, ..Default::default() },
            ConditionRules { partly: 0.4, ..Default::default() },
            ConditionRules { up: 0.0, ..Default::default() },
            ConditionRules { output: (0.5, -0.7), ..Default::default() },
            ConditionRules { fed: (3.0, 2.0), ..Default::default() },
            ConditionRules { climate: (0.25, 0.75), ..Default::default() },
            ConditionRules { company: 0.0, ..Default::default() },
            ConditionRules { snow_hardy: f32::NAN, ..Default::default() },
        ];
        for rules in bad {
            assert!(rules.check().is_err(), "{rules:?} loaded");
        }
    }
}
