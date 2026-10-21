//! The level a check sees (Mariano's progression plan, 2026-10-06): three ways past any
//! level requirement. Effective level = level + boost + schematic. Boosts come from effects
//! (a boost potion, a dish) and add at most +6 in all; a schematic carried for what is
//! being made takes 5 to 10 off what it asks. Boosts and schematics help with making and
//! gathering, never with wielding, and a firm recipe (the Singularity) takes neither.

use crate::block::BlockRegistry;
use crate::effect::{ActiveEffect, EffectRegistry};
use crate::inventory::Inventory;
use crate::skill::{SkillId, SkillRegistry, Skills};

/// The most boosts add, or take, in all.
pub const MAX_BOOST: i8 = 6;

/// A level with boosts (already capped) and a schematic's discount.
pub fn effective(level: u8, boost: i8, discount: u8) -> u8 {
    (level as i16 + boost.clamp(-MAX_BOOST, MAX_BOOST) as i16 + discount as i16).clamp(1, u8::MAX as i16) as u8
}

/// A player's level in `skill` for a check: their own, with what their effects boost.
pub fn check_level(skills: &SkillRegistry, effects: &EffectRegistry, xp: &Skills, active: &[ActiveEffect], skill: SkillId) -> u8 {
    effective(skills.level(xp.xp(skill)), effects.boost(active, &skills.get(skill).name), 0)
}

/// The discount schematics carried give for making `output`: the best of their
/// `schematic:<item>:<levels>` tags naming it (by full name or name alone).
pub fn schematic_discount(inventory: &Inventory, blocks: &BlockRegistry, output: &str) -> u8 {
    let short = output.rsplit(':').next().unwrap_or(output);
    inventory
        .slots
        .iter()
        .flatten()
        .flat_map(|stack| blocks.def(stack.item).tags.iter())
        .filter_map(|tag| {
            let (item, levels) = tag.strip_prefix("schematic:")?.rsplit_once(':')?;
            (item == output || item == short).then(|| levels.parse::<u8>().ok()).flatten()
        })
        .max()
        .unwrap_or(0)
}

/// Whether a player whose own level (with boosts) in a recipe's skill is `level` may make
/// it, with what their schematics take off.
pub fn meets_recipe(recipe: &crate::craft::Recipe, own: u8, boosted: u8, inventory: &Inventory, blocks: &BlockRegistry) -> bool {
    meets(recipe.level, recipe.firm, recipe.output, own, boosted, inventory, blocks)
}

/// The skill and level mending `item` asks that a player lacks, if any: its recipe's, met
/// as making one is (`level` gives their own level in a skill and that with boosts).
pub fn lacks_for_mend(tools: &crate::tool::ToolRegistry, item: crate::block::BlockId, level: impl Fn(SkillId) -> (u8, u8), inventory: &Inventory, blocks: &BlockRegistry) -> Option<(SkillId, u8)> {
    let (skill, asks) = tools.mend_skill(item)?;
    let (own, boosted) = level(skill);
    (!meets(asks, tools.mend_firm(item), item, own, boosted, inventory, blocks)).then_some((skill, asks))
}

fn meets(level: u8, firm: bool, output: crate::block::BlockId, own: u8, boosted: u8, inventory: &Inventory, blocks: &BlockRegistry) -> bool {
    if firm {
        return own >= level;
    }
    let discount = schematic_discount(inventory, blocks, &blocks.def(output).name);
    effective(boosted, 0, discount) >= level
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDef;

    #[test]
    fn boosts_are_capped_and_schematics_take_levels_off() {
        assert_eq!(effective(54, 3, 0), 57);
        assert_eq!(effective(54, 12, 0), 60, "boosts stop at +6");
        assert_eq!(effective(54, 3, 10), 67);
        assert_eq!(effective(2, -6, 0), 1);
        let blocks = BlockRegistry::new(vec![
            BlockDef { name: "t:drill".into(), ..Default::default() },
            BlockDef { name: "t:plan".into(), tags: vec!["schematic:drill:8".into(), "schematic:t:pump:5".into()], ..Default::default() },
        ])
        .unwrap();
        let mut inventory = Inventory::default();
        assert_eq!(schematic_discount(&inventory, &blocks, "t:drill"), 0);
        inventory.add(blocks.id("t:plan").unwrap(), 1);
        assert_eq!(schematic_discount(&inventory, &blocks, "t:drill"), 8);
        assert_eq!(schematic_discount(&inventory, &blocks, "t:pump"), 5);
        assert_eq!(schematic_discount(&inventory, &blocks, "t:saw"), 0);
    }
}
