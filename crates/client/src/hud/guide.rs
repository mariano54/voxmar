//! What a skill's guide lists, sorted into kinds (planting, breeding, cooking), and the page
//! each line opens: what it needs, roughly how to go about it, what it gives and what is
//! near it. The pages hint rather than tell: where to look and what helps, not the numbers
//! (books and study will tell the rest).

use bevy::prelude::*;
use voxmar_engine::block::BlockId;
use voxmar_engine::crop::{Crop, CropId};
use voxmar_engine::skill::SkillId;

use crate::lang::Locale;
use crate::session::Launch;

/// Where a guide line comes from, for the page it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The skill's own guide in skills.ron, by its place there.
    Step(usize),
    /// A crop sown at its level.
    Crop(CropId),
    /// A dish cooked at its level.
    Cook(BlockId),
}

/// A line of a skill's guide.
pub struct Entry {
    pub level: u8,
    pub words: String,
    pub icon: Option<BlockId>,
    /// The kind it is sorted under, by name (`planting`); `general` when its data names none.
    pub category: String,
    pub source: Source,
}

pub const GENERAL: &str = "general";

/// What a skill opens up, by level: its own guide in skills.ron, every crop it sows, and
/// every dish it cooks.
pub fn entries(locale: &Locale, launch: &Launch, skill: SkillId) -> Vec<Entry> {
    let content = &launch.content;
    let def = content.skills.get(skill);
    let item = |name: &str| content.blocks.id(name).or_else(|| content.blocks.id(&format!("base:{name}")));
    let mut entries: Vec<Entry> = def
        .guide
        .iter()
        .enumerate()
        .map(|(i, step)| Entry {
            level: step.level,
            words: locale.skill_guide(def, i),
            icon: step.icon.as_deref().and_then(item),
            category: step.category.clone().unwrap_or_else(|| GENERAL.into()),
            source: Source::Step(i),
        })
        .collect();
    for (id, crop) in content.crops.iter().filter(|(_, crop)| crop.skill == skill && crop.def.sown) {
        let words = locale.format("character.sow", &[("crop", &locale.block_named(&content.blocks, &crop.def.name))]);
        entries.push(Entry { level: crop.def.level, words, icon: item(&crop.def.name), category: "planting".into(), source: Source::Crop(id) });
    }
    // Everything cooked (or fired, or smelted) in this skill, at the level it starts at,
    // once each.
    {
        let mut cooked: Vec<BlockId> = Vec::new();
        for (_, recipe) in content.cooking.iter().filter(|(_, r)| r.skill == Some(skill)) {
            if !cooked.contains(&recipe.output) {
                cooked.push(recipe.output);
                let words = locale.format("character.cook", &[("item", &locale.block(content.blocks.def(recipe.output)))]);
                entries.push(Entry { level: recipe.def.level, words, icon: Some(recipe.output), category: "cooking".into(), source: Source::Cook(recipe.output) });
            }
        }
    }
    entries.sort_by_key(|entry| entry.level);
    entries
}

/// The kinds a guide's lines are sorted under, in the order they first come; none when
/// all are of one kind, which needs no sorting.
pub fn categories(entries: &[Entry]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for entry in entries {
        if !found.contains(&entry.category) {
            found.push(entry.category.clone());
        }
    }
    // The uncategorised last, under General.
    if let Some(i) = found.iter().position(|c| c == GENERAL) {
        let general = found.remove(i);
        found.push(general);
    }
    if found.len() < 2 { Vec::new() } else { found }
}

/// A kind's name, in the player's language.
pub fn category_name(locale: &Locale, category: &str) -> String {
    let key = format!("guide.category.{category}");
    locale.get_or(&key, category).to_owned()
}

/// One part of a guide line's page.
pub enum Part {
    /// A heading over the lines that follow.
    Heading(String),
    /// A line of words.
    Line(String),
    /// Items, by their icons and names.
    Items(Vec<BlockId>),
}

/// The page a guide line opens, as parts to lay out top to bottom, and the lines near it.
pub fn page(locale: &Locale, launch: &Launch, skill: SkillId, entries: &[Entry], index: usize) -> (Vec<Part>, Vec<usize>) {
    let content = &launch.content;
    let def = content.skills.get(skill);
    let entry = &entries[index];
    let skill_name = locale.skill(def);
    let mut parts = vec![Part::Heading(locale.get("guide.needs").to_owned())];
    parts.push(Part::Line(locale.format("guide.level", &[("skill", &skill_name), ("level", &entry.level)])));
    let mut how: Vec<String> = Vec::new();
    let mut gives: Vec<BlockId> = Vec::new();
    match entry.source {
        Source::Crop(id) => {
            let crop = content.crops.get(id);
            if let Some(seed) = crop.seed {
                parts.push(Part::Items(vec![seed]));
            }
            how.extend(crop_hints(locale, launch, crop));
            gives.extend(crop.harvest.iter().map(|(item, _)| *item));
        }
        Source::Cook(output) => {
            // One way to make it, the first defined at this entry's level: an item made two
            // ways (copper ingots from ore or from crushed ore) shows what one of them takes,
            // not both mixed, and the plain way before the anvil's, as cooking.ron lists them.
            let made: Vec<_> = content.cooking.iter().filter(|(_, recipe)| recipe.output == output).collect();
            let ways: Vec<_> = content.cooking.recipes().filter(|recipe| recipe.output == output).collect();
            let recipe = ways.iter().find(|recipe| recipe.def.level == entry.level).or(ways.first()).copied();
            let inputs: Vec<BlockId> = made.iter().filter(|(_, other)| recipe.is_some_and(|r| std::ptr::eq(*other, r))).map(|(input, _)| *input).collect();
            if !inputs.is_empty() {
                parts.push(Part::Items(inputs));
            }
            // What it takes besides what is held (a tin ore with the copper).
            parts.extend(recipe.into_iter().flat_map(|recipe| recipe.with.iter().map(|(items, _)| Part::Items(items.clone()))));
            if let Some(recipe) = recipe {
                how.push(locale.get(if recipe.def.station.ends_with("campfire") { "guide.station.campfire" } else { "guide.station.other" }).to_owned());
                if recipe.def.stop_burning.is_some() {
                    how.push(locale.get("guide.burns").to_owned());
                }
            }
            gives.push(output);
        }
        Source::Step(i) => {
            let key = format!("{}.guide.{i}.how", voxmar_engine::lang::def_key("skill", &def.name));
            let own = def.guide.get(i).and_then(|step| step.how.as_deref()).unwrap_or("");
            let words = locale.get_or(&key, own);
            if !words.is_empty() {
                how.push(words.to_owned());
            } else if let Some(hint) = locale.find(&format!("guide.hint.{}", entry.category)) {
                how.push(hint.to_owned());
            }
            gives.extend(entry.icon);
        }
    }
    if !how.is_empty() {
        parts.push(Part::Heading(locale.get("guide.how").to_owned()));
        parts.extend(how.into_iter().map(Part::Line));
    }
    parts.push(Part::Heading(locale.get("guide.gives").to_owned()));
    if !gives.is_empty() {
        parts.push(Part::Items(gives));
    }
    parts.push(Part::Line(locale.format("guide.xp", &[("skill", &skill_name)])));
    // Near it: the other lines of its kind closest in level, up to four.
    let mut near: Vec<usize> = (0..entries.len()).filter(|&i| i != index && entries[i].category == entry.category).collect();
    near.sort_by_key(|&i| (entries[i].level as i32 - entry.level as i32).abs());
    near.truncate(4);
    near.sort_by_key(|&i| entries[i].level);
    (parts, near)
}

/// Roughly where and how a crop grows, without its numbers.
fn crop_hints(locale: &Locale, launch: &Launch, crop: &Crop) -> Vec<String> {
    let mut hints = Vec::new();
    if let Some(wild) = crop.def.wild.first() {
        hints.push(locale.format("guide.wild", &[("plant", &locale.block_named(&launch.content.blocks, wild))]));
    }
    if crop.def.ground.iter().any(|tag| tag == "tilled") {
        hints.push(locale.get("guide.ground.tilled").to_owned());
    }
    let (low, high) = crop.def.comfort.temperature;
    if high - low < 100.0 {
        let middle = (low + high) / 2.0;
        let climate = if middle < 8.0 {
            "guide.climate.cold"
        } else if middle < 20.0 {
            "guide.climate.mild"
        } else if middle < 28.0 {
            "guide.climate.warm"
        } else {
            "guide.climate.hot"
        };
        hints.push(locale.get(climate).to_owned());
    }
    let (dry, wet) = crop.def.comfort.moisture;
    if wet <= 1.0 {
        hints.push(locale.get("guide.climate.dry").to_owned());
    } else if dry >= 0.9 {
        hints.push(locale.get("guide.climate.wet").to_owned());
    }
    if crop.def.water {
        hints.push(locale.get("guide.water").to_owned());
    }
    if crop.def.light >= 9 {
        hints.push(locale.get("guide.light").to_owned());
    }
    hints.push(
        locale
            .get(if crop.def.minutes < 10.0 {
                "guide.time.quick"
            } else if crop.def.minutes < 40.0 {
                "guide.time.while"
            } else {
                "guide.time.long"
            })
            .to_owned(),
    );
    if crop.def.regrow.is_some() {
        hints.push(locale.get("guide.regrow").to_owned());
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An item cooked two ways shows the way defined first at its entry's level, whole: the
    /// bronze page holds copper ingots with tin (any of it), not crushed tin with copper.
    #[test]
    fn a_dish_made_two_ways_shows_the_first_way_whole() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).unwrap();
        let locale = Locale::load(&assets.join("content"), "en");
        let id = |name: &str| content.blocks.id(name).unwrap();
        let launch = Launch { name: String::new(), host: false, content: content.clone() };
        let engineering = content.skills.id("core:engineering").unwrap();
        let entries = entries(&locale, &launch, engineering);
        let bronze = entries.iter().position(|e| e.source == Source::Cook(id("base:bronze_ingot"))).expect("bronze is in the guide");
        let (parts, _) = page(&locale, &launch, engineering, &entries, bronze);
        let items: Vec<&Vec<BlockId>> = parts.iter().filter_map(|p| if let Part::Items(items) = p { Some(items) } else { None }).collect();
        assert_eq!(items[0], &vec![id("base:copper_ingot")], "{items:?}");
        assert!(items[1].contains(&id("base:tin_ore")), "{items:?}");
        // Held, with, gives: no second way's input or with.
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(items[2], &vec![id("base:bronze_ingot")], "{items:?}");
    }
}
