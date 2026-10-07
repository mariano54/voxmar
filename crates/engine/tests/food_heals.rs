//! Every food heals by the rule (design drafts 55 and 63): cooked food 0.4 × the lowest
//! level that cooks it, at least 2; wonder fruit the same by the Farming level that picks
//! it; other fruit and berries 2; anything else edible, raw produce and meat among it, 1.

use voxmar_engine::block::BlockId;
use voxmar_engine::content::Content;
use voxmar_engine::vitals::cooked_heal;

#[test]
fn every_food_heals_by_its_level() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
    let content = Content::load(&dir).unwrap();
    let cooking = content.skills.id("core:cooking");
    // The lowest Cooking level that makes each item, at a station or by hand.
    let mut cooked: Vec<(BlockId, u8)> = Vec::new();
    let mut made = |item: BlockId, level: u8| match cooked.iter_mut().find(|(i, _)| *i == item) {
        Some((_, least)) => *least = (*least).min(level),
        None => cooked.push((item, level)),
    };
    for (_, recipe) in content.cooking.iter() {
        if recipe.skill.is_none() || recipe.skill == cooking {
            made(recipe.output, recipe.def.level);
        }
    }
    for recipe in content.recipes.iter().filter(|r| r.skill.is_some() && r.skill == cooking) {
        made(recipe.output, recipe.level);
    }
    let picked_at = |item: BlockId| content.crops.iter().find(|(_, c)| c.def.fruit && c.harvest.iter().any(|(i, _)| *i == item)).map(|(_, c)| c.def.level);
    let mut wrong = Vec::new();
    for id in content.blocks.ids() {
        let def = content.blocks.def(id);
        let heal = content.blocks.info(id).food;
        if heal == 0 {
            continue;
        }
        let expected = if let Some(&(_, level)) = cooked.iter().find(|(i, _)| *i == id) {
            cooked_heal(level)
        } else if def.has_tag("wonder") && def.has_tag("fruit") {
            cooked_heal(picked_at(id).unwrap_or(1))
        } else if def.has_tag("fruit") || def.has_tag("berry") {
            2
        } else {
            1
        };
        if heal != expected {
            wrong.push(format!("{} heals {heal}, not {expected}", def.name));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
