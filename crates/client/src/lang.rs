//! The player's language: [`Locale`] holds its strings (see `voxmar_engine::lang`), text
//! spawned with a [`Localized`] key keeps showing that key's string, and the setting
//! switches all of it at once.
//!
//! Words a screen writes once go on screen as `(Localized("menu.play"), text(…))`, and are
//! rewritten whenever the language changes. Words built as the game runs (counts, names,
//! messages) come from `Res<Locale>`, and the system writing them redraws them when the
//! locale changes too. A definition's name comes from [`Locale::block`] and its kin, which
//! fall back to a name made from its id, so a mod's blocks read well before anyone
//! translates them.

use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::text::LineHeight;
use voxmar_engine::block::{BlockDef, BlockRegistry};
use voxmar_engine::lang::{ENGLISH, Lang, Language, def_key, fill, languages};
use voxmar_engine::skill::SkillDef;

use crate::settings::Settings;

pub struct LangPlugin;

impl Plugin for LangPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, switch.run_if(resource_changed::<Settings>))
            .add_systems(PostUpdate, localize.before(bevy::ui::UiSystems::Prepare));
    }
}

/// The player's language. Cheap to clone.
#[derive(Resource, Clone)]
pub struct Locale {
    lang: Arc<Lang>,
    /// Every language some package has strings for.
    pub languages: Arc<Vec<Language>>,
}

impl Locale {
    /// Language `code` from the content packages, or the system's language for an empty
    /// code (English when the game has no strings for it).
    pub fn load(content_dir: &Path, code: &str) -> Self {
        let languages = languages(content_dir);
        let code = resolve(code, &languages);
        Locale { lang: Arc::new(Lang::load(content_dir, &code)), languages: Arc::new(languages) }
    }

    /// What a block or item is called: its own name in the language (`block.base.oak_log`),
    /// or, for a block made from a template, the template member's name with the family's
    /// filled in (`block.base.{name}_log` with `family.base.oak`), so a language names a
    /// whole family of woods or rocks in two lines; another look of a plant goes by the
    /// plant's name. Without any of those, a name made from its id: "Oak Log".
    pub fn block(&self, def: &BlockDef) -> String {
        if let Some(name) = self.lang.find(&def_key("block", &def.name)) {
            return name.to_owned();
        }
        let (namespace, local) = def.name.split_once(':').unwrap_or(("", &def.name));
        if let Some(family) = &def.family {
            let family_local = family.rsplit(':').next().unwrap_or(family);
            let pattern = local.replacen(family_local, "{name}", 1);
            if let (Some(member), Some(family)) = (self.lang.find(&format!("block.{namespace}.{pattern}")), self.lang.find(&def_key("family", family))) {
                return fill(member, &[("name", &family)]);
            }
        }
        if let Some(plant) = def.variant_of.as_deref().and_then(|plant| self.lang.find(&def_key("block", plant))) {
            return plant.to_owned();
        }
        title_from_id(&def.name)
    }

    /// What the block called `name` is called (see [`Locale::block`]).
    pub fn block_named(&self, blocks: &BlockRegistry, name: &str) -> String {
        blocks.id(name).map_or_else(|| title_from_id(name), |id| self.block(blocks.def(id)))
    }

    /// What a skill is called.
    pub fn skill(&self, def: &SkillDef) -> String {
        self.get_or(&def_key("skill", &def.name), &def.title).to_owned()
    }

    /// One line on what a skill covers.
    pub fn skill_description(&self, def: &SkillDef) -> String {
        self.get_or(&format!("{}.description", def_key("skill", &def.name)), &def.description).to_owned()
    }

    /// Line `index` of a skill's guide.
    pub fn skill_guide(&self, def: &SkillDef, index: usize) -> String {
        let key = format!("{}.guide.{index}", def_key("skill", &def.name));
        self.get_or(&key, def.guide.get(index).map_or("", |step| step.text.as_str())).to_owned()
    }

    /// How tall a line of wrapped text is, in font sizes: kanji fill their whole box, so
    /// Japanese needs more room between lines than English (`language.line_height`).
    pub fn line_height(&self) -> f32 {
        self.find("language.line_height").and_then(|value| value.parse().ok()).filter(|&h: &f32| (1.0..3.0).contains(&h)).unwrap_or(1.2)
    }

    /// The language's name in itself, for the language setting.
    pub fn language_name<'a>(&'a self, code: &'a str) -> &'a str {
        self.languages.iter().find(|language| language.code == code).map_or(code, |language| language.name.as_str())
    }
}

impl Deref for Locale {
    type Target = Lang;

    fn deref(&self) -> &Lang {
        &self.lang
    }
}

/// The code `wanted` means: itself when some package has it, the system's language for
/// an empty one, and English otherwise.
fn resolve(wanted: &str, languages: &[Language]) -> String {
    let has = |code: &str| languages.iter().any(|language| language.code == code);
    if !wanted.is_empty() {
        return if has(wanted) { wanted.to_owned() } else { ENGLISH.to_owned() };
    }
    // "ja-JP" and "ja_JP.UTF-8" are Japanese.
    let system = sys_locale::get_locale().unwrap_or_default();
    let primary = system.split(['-', '_', '.']).next().unwrap_or("").to_ascii_lowercase();
    if has(&primary) { primary } else { ENGLISH.to_owned() }
}

/// A name for people made from an id: `base:oak_log` is "Oak Log".
pub fn title_from_id(name: &str) -> String {
    let local = name.rsplit(':').next().unwrap_or(name);
    local
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text that shows a key's string in the player's language, rewritten when it changes.
#[derive(Component, Clone, Copy, Debug)]
pub struct Localized(pub &'static str);

/// Loads the language the settings ask for when they change to another.
fn switch(settings: Res<Settings>, mut locale: ResMut<Locale>) {
    let wanted = resolve(&settings.language, &locale.languages);
    if wanted != locale.code() {
        let content = voxmar_server::assets_dir().join("content");
        locale.lang = Arc::new(Lang::load(&content, &wanted));
    }
}

/// Writes each new [`Localized`] text, and every one when the language changes, before
/// text is laid out so no frame shows a key; and spaces the lines of every text as the
/// language wants.
fn localize(
    locale: Res<Locale>,
    mut texts: Query<(Ref<Localized>, &mut Text)>,
    mut lines: Query<(Ref<Text>, &mut LineHeight)>,
) {
    let all = locale.is_changed();
    let height = LineHeight::RelativeToFont(locale.line_height());
    for (text, mut line) in &mut lines {
        if all || text.is_added() {
            line.set_if_neq(height);
        }
    }
    for (localized, mut text) in &mut texts {
        if all || localized.is_added() || localized.is_changed() {
            let words = locale.get(localized.0);
            if text.0 != words {
                words.clone_into(&mut text.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use voxmar_engine::block::BlockId;
    use voxmar_engine::lang::arguments;

    use super::*;

    fn content() -> std::path::PathBuf {
        voxmar_server::assets_dir().join("content")
    }

    /// The first-party packages, which must be complete in every language the game ships.
    const FIRST_PARTY: [&str; 2] = ["base", "core"];

    /// Every name a player can come across in first-party content, in `locale`, by what
    /// it names: blocks and items (a fluid's flowing states show their source's), and
    /// skills with their descriptions and guides.
    fn definition_names(content: &voxmar_engine::content::Content, locale: &Locale) -> Vec<(String, String)> {
        let first_party = |name: &str| FIRST_PARTY.iter().any(|package| name.starts_with(&format!("{package}:")));
        let blocks = &content.blocks;
        let mut names = Vec::new();
        for id in (0..blocks.len()).map(|i| BlockId(i as u16)) {
            let def = blocks.def(id);
            let flowing = blocks.fluid(id).is_some_and(|state| blocks.fluid_def(state.fluid).source != id);
            if first_party(&def.name) && !flowing && def.name != "core:air" {
                names.push((def.name.clone(), locale.block(def)));
            }
        }
        for id in content.skills.ids() {
            let def = content.skills.get(id);
            names.push((def.name.clone(), locale.skill(def)));
            names.push((format!("{} description", def.name), locale.skill_description(def)));
            for i in 0..def.guide.len() {
                names.push((format!("{} guide {i}", def.name), locale.skill_guide(def, i)));
            }
        }
        names
    }

    /// Japanese has every word English has, with the same arguments, and every
    /// first-party block and skill has a Japanese name.
    #[test]
    fn japanese_is_complete() {
        let japanese = Lang::load(&content(), "ja");
        let english = Lang::load(&content(), ENGLISH);
        let untranslated: BTreeSet<&str> = japanese.untranslated().collect();
        let mut missing: Vec<String> = Vec::new();
        for key in english.keys() {
            // Japanese counts need no form for one.
            if key.ends_with(".one") {
                continue;
            }
            if untranslated.contains(key) {
                missing.push(key.to_owned());
            } else {
                assert_eq!(arguments(japanese.get(key)), arguments(english.get(key)), "`{key}` changes its arguments");
            }
        }
        let loaded = voxmar_server::load_content(&voxmar_server::assets_dir()).unwrap();
        let in_english = definition_names(&loaded, &Locale::load(&content(), ENGLISH));
        for ((what, japanese), (_, english)) in definition_names(&loaded, &Locale::load(&content(), "ja")).into_iter().zip(in_english) {
            if japanese == english {
                missing.push(format!("{what} ({english})"));
            }
        }
        missing.sort();
        assert!(missing.is_empty(), "{} words have no Japanese:\n{}", missing.len(), missing.join("\n"));
    }

    /// Every key the interface's code names is in English. Keys the code builds (a
    /// setting's `.hint`, `key.<code>`, `alchemy.<property>`) are checked where they are
    /// built.
    #[test]
    fn every_key_in_the_code_exists() {
        let english = Lang::load(&content(), ENGLISH);
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![source];
        let mut unknown = BTreeSet::new();
        let namespaces = [
            "menu", "worlds", "time", "create", "mode", "join", "loading", "net", "settings", "screen", "hud", "inventory",
            "rarity", "item", "skills", "character", "language",
        ];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for literal in text.split('"').skip(1).step_by(2) {
                let namespace = literal.split('.').next().unwrap_or("");
                let key_like = literal.contains('.') && literal.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
                if !key_like || !namespaces.contains(&namespace) || literal.ends_with(".ron") {
                    continue;
                }
                let counted = english.find(&format!("{literal}.other")).is_some();
                if english.find(literal).is_none() && !counted {
                    unknown.insert(format!("{}: {literal}", path.file_name().unwrap().to_string_lossy()));
                }
            }
        }
        assert!(unknown.is_empty(), "keys missing from en.ron:\n{}", unknown.into_iter().collect::<Vec<_>>().join("\n"));
    }

    /// Every character any language writes is in the interface's fonts, with the
    /// fallback fonts merged in: no tofu boxes.
    #[test]
    fn every_character_is_drawn() {
        let fonts = voxmar_server::assets_dir().join("ui/fonts");
        let read = |name: &str| std::fs::read_to_string(fonts.join(name)).ok();
        let fallback = crate::ui::skin::fallback();
        let loaded = voxmar_server::load_content(&voxmar_server::assets_dir()).unwrap();
        let mut texts: Vec<String> = Vec::new();
        for language in languages(&content()) {
            let locale = Locale::load(&content(), &language.code);
            texts.extend(locale.keys().map(|key| locale.get(key).to_owned()));
            texts.extend(definition_names(&loaded, &locale).into_iter().map(|(_, name)| name));
        }
        for file in ["ledger.txt", "block.txt", "voxmar.txt"] {
            let mut font = voxmar_texgen::font::PixelFont::parse_with(&read(file).unwrap(), read).unwrap();
            font.fill_from(fallback);
            let missing: BTreeSet<char> =
                texts.iter().flat_map(|text| text.chars()).filter(|c| !c.is_whitespace() && !font.glyphs.contains_key(c)).collect();
            assert!(missing.is_empty(), "{file} lacks {}", missing.into_iter().collect::<String>());
        }
    }

    #[test]
    fn names_come_from_ids_until_a_language_names_them() {
        assert_eq!(title_from_id("base:oak_log"), "Oak Log");
        assert_eq!(title_from_id("tall__grass"), "Tall Grass");
    }

    #[test]
    fn unknown_languages_fall_back_to_english() {
        let languages = vec![Language { code: "en".into(), name: "English".into() }, Language { code: "ja".into(), name: "日本語".into() }];
        assert_eq!(resolve("ja", &languages), "ja");
        assert_eq!(resolve("xx", &languages), "en");
        let locale = Locale::load(&content(), "xx");
        assert_eq!(locale.code(), "en");
    }
}
