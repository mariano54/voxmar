//! The player's language: [`Locale`] holds its strings (see `voxmar_engine::lang`), text
//! spawned with a [`Localized`] key keeps showing that key's string, and the setting
//! switches all of it at once.
//!
//! Words a screen writes once go on screen as `(Localized("menu.settings"), text(…))`, and are
//! rewritten whenever the language changes. Words built as the game runs (counts, names,
//! messages) come from `Res<Locale>`, and the system writing them redraws them when the
//! locale changes too. A definition's name comes from [`Locale::block`] and its kin, which
//! fall back to a name made from its id, so a mod's blocks read well before anyone
//! translates them.

use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::text::{FontCx, FontSmoothing, LineHeight};
use voxmar_engine::block::{BlockDef, BlockRegistry};
use voxmar_engine::creature::CreatureDef;
use voxmar_engine::lang::{ENGLISH, Lang, Language, def_key, languages, title_from_id};
use voxmar_engine::skill::SkillDef;
use voxmar_engine::effect::EffectDef;

use crate::settings::Settings;

pub struct LangPlugin;

impl Plugin for LangPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SmoothFonts>()
            .add_systems(Startup, load_smooth_fonts)
            .add_systems(Update, switch.run_if(resource_changed::<Settings>))
            .add_systems(
                PostUpdate,
                (
                    localize.before(bevy::ui::UiSystems::Prepare),
                    (register_rebuilt_fonts, fall_back_to_smooth_fonts.run_if(resource_changed::<FontCx>))
                        .chain()
                        .after(bevy::text::load_font_assets_into_font_collection)
                        .before(bevy::ui::UiSystems::Prepare),
                ),
            );
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

    /// What a block or item is called (see `Lang::block`).
    pub fn block(&self, def: &BlockDef) -> String {
        self.lang.block(def)
    }

    /// What the block called `name` is called (see [`Locale::block`]).
    pub fn block_named(&self, blocks: &BlockRegistry, name: &str) -> String {
        blocks.id(name).map_or_else(|| title_from_id(name), |id| self.block(blocks.def(id)))
    }

    /// How a block or item's name reads, when it holds kanji: kanji in hiragana, the rest as
    /// written (オークのげんぼく for オークの原木). None for a name with no kanji, and in a
    /// language that has none. The same for the others below.
    #[allow(dead_code, reason = "the interface shows readings with names (Shift over one)")]
    pub fn block_reading(&self, def: &BlockDef) -> Option<String> {
        self.lang.block_reading(def)
    }

    /// How a creature's name reads (see [`Locale::block_reading`]).
    #[allow(dead_code, reason = "the interface shows readings with names (Shift over one)")]
    pub fn creature_reading(&self, def: &CreatureDef) -> Option<String> {
        self.reading(&def_key("creature", &def.name))
    }

    /// How a skill's name reads (see [`Locale::block_reading`]).
    #[allow(dead_code, reason = "the interface shows readings with names (Shift over one)")]
    pub fn skill_reading(&self, def: &SkillDef) -> Option<String> {
        self.reading(&def_key("skill", &def.name))
    }

    /// How a condition's name reads (see [`Locale::block_reading`]).
    #[allow(dead_code, reason = "the interface shows readings with names (Shift over one)")]
    pub fn effect_reading(&self, def: &EffectDef) -> Option<String> {
        self.reading(&def_key("effect", &def.name))
    }

    /// What a creature is called.
    pub fn creature(&self, def: &CreatureDef) -> String {
        self.get_or(&def_key("creature", &def.name), &def.title).to_owned()
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

    /// How tall the one line of a [`TightLine`] text is, in font sizes: the letters' own
    /// height in the language's font (`language.tight_line`), 1 unless given.
    pub fn tight_line_height(&self) -> f32 {
        self.find("language.tight_line").and_then(|value| value.parse().ok()).filter(|&h: &f32| (1.0..2.0).contains(&h)).unwrap_or(1.0)
    }

    /// What an effect (well fed, soaked, hearty) is called.
    pub fn effect(&self, def: &EffectDef) -> String {
        self.get_or(&def_key("effect", &def.name), &def.title).to_owned()
    }

    /// What an effect does, in a line.
    pub fn effect_description(&self, def: &EffectDef) -> String {
        self.get_or(&format!("{}.description", def_key("effect", &def.name)), &def.description).to_owned()
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

/// A one-line text whose line is just its font size tall in every language, so it sits
/// centred in a box made to fit it: the number in a meter, the name on a chip.
#[derive(Component, Clone, Copy, Default)]
pub struct TightLine;

/// Writes each new [`Localized`] text, and every one when the language changes, before
/// text is laid out so no frame shows a key; and spaces the lines of every text as the
/// language wants.
fn localize(
    locale: Res<Locale>,
    smooth: Res<SmoothFonts>,
    mut texts: Query<(Option<Ref<Localized>>, Mut<Text>, &mut LineHeight, &mut TextFont, Has<TightLine>)>,
) {
    let all = locale.is_changed();
    let height = LineHeight::RelativeToFont(locale.line_height());
    let tight_height = LineHeight::RelativeToFont(locale.tight_line_height());
    for (localized, mut text, mut line, mut font, tight) in &mut texts {
        if all || text.is_added() {
            line.set_if_neq(if tight { tight_height } else { height });
        }
        if let Some(localized) = localized
            && (all || localized.is_added() || localized.is_changed())
        {
            let words = locale.get(localized.0);
            if text.0 != words {
                words.clone_into(&mut text.0);
            }
        }
        if text.is_changed() && !smooth.0.is_empty() {
            // A smooth font's glyphs drawn without smoothing come out ragged.
            let smoothing = if text.0.chars().any(drawn_smooth) { FontSmoothing::AntiAliased } else { FontSmoothing::None };
            if font.font_smoothing != smoothing {
                font.font_smoothing = smoothing;
            }
        }
    }
}

/// Whether a character comes from a smooth fallback font rather than our pixel fonts: the
/// CJK scripts, from radicals and punctuation on.
fn drawn_smooth(c: char) -> bool {
    c as u32 >= 0x2E80 && !(0xE000..0xF900).contains(&(c as u32))
}

/// Puts a font rebuilt into a handle that already had one (the interface font, when the
/// setting changes) into the font collection under a new name, and points text at it. Bevy
/// only adds fonts with new handles, so text would otherwise ask for a name nothing has:
/// every glyph a missing-glyph box.
fn register_rebuilt_fonts(mut fonts: ResMut<Assets<Font>>, mut font_cx: ResMut<FontCx>, mut texts: Query<&mut TextFont>, mut built: Local<u32>) {
    let rebuilt: Vec<_> = fonts.iter().filter(|(_, font)| font.alias.is_empty()).map(|(id, _)| id).collect();
    if rebuilt.is_empty() {
        return;
    }
    for id in rebuilt {
        let Some(font) = fonts.get_mut_untracked(id) else { continue };
        *built += 1;
        font.alias = format!("asset_id:{id:?} build {}", *built);
        let name = parley::fontique::FontInfoOverride { family_name: Some(font.alias.as_str()), ..default() };
        font_cx.collection.register_fonts(font.data.clone(), Some(name));
    }
    for mut font in &mut texts {
        font.set_changed();
    }
}

/// The texts whose font isn't in the font collection, so that every character draws a
/// missing-glyph box: how many, and what the first reads. None when every text has one.
pub fn texts_without_a_font(world: &mut World) -> Option<String> {
    let mut texts = world.query::<(&Text, &TextFont)>();
    let names: Vec<(String, String)> = texts
        .iter(world)
        .filter_map(|(text, font)| match &font.font {
            bevy::text::FontSource::Handle(handle) => Some((text.0.clone(), world.resource::<Assets<Font>>().get(handle.id()).map_or(String::new(), |f| f.alias.clone()))),
            _ => None,
        })
        .collect();
    let collection = &mut world.resource_mut::<FontCx>().into_inner().collection;
    let lost: Vec<String> = names.into_iter().filter(|(_, name)| name.is_empty() || collection.family_id(name).is_none()).map(|(text, _)| text).collect();
    lost.first().map(|first| format!("{} texts, the first reading {first:?}", lost.len()))
}

/// The smooth fallback fonts (see `assets/ui/fonts/fallback.ron`), in order.
#[derive(Resource, Default)]
struct SmoothFonts(Vec<Handle<Font>>);

fn load_smooth_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let dir = voxmar_server::assets_dir().join("ui/fonts");
    let handles = crate::ui::skin::smooth_fallbacks(&dir).into_iter().map(|data| fonts.add(Font::from_bytes(data))).collect();
    commands.insert_resource(SmoothFonts(handles));
}

/// Whether text in the CJK scripts falls back to the smooth fonts, as it should once they
/// have loaded (true when `fallback.ron` lists none).
pub fn smooth_fonts_ready(world: &mut World) -> bool {
    let fonts: Vec<_> = world.resource::<SmoothFonts>().0.iter().map(|font| format!("asset_id:{:?}", font.id())).collect();
    let collection = &mut world.resource_mut::<FontCx>().into_inner().collection;
    let Some(first) = fonts.first().and_then(|name| collection.family_id(name)) else { return fonts.is_empty() };
    SMOOTH_SCRIPTS.iter().all(|script| {
        let key = parley::fontique::FallbackKey::new(parley::fontique::Script::from_bytes(*script), None);
        collection.fallback_families(key).next() == Some(first)
    })
}

/// The scripts the smooth fallback fonts are for: Japanese.
const SMOOTH_SCRIPTS: [[u8; 4]; 4] = [*b"Hira", *b"Kana", *b"Hani", *b"Zyyy"];

/// Makes the smooth fonts what Japanese text falls back to, once Bevy has them in its
/// font collection, and again whenever it rebuilds that (when a font is removed). Runs
/// only when the collection may have changed.
fn fall_back_to_smooth_fonts(smooth: Res<SmoothFonts>, mut font_cx: ResMut<FontCx>, mut texts: Query<&mut TextFont>) {
    if smooth.0.is_empty() {
        return;
    }
    let collection = &mut font_cx.bypass_change_detection().collection;
    let families: Vec<_> = smooth.0.iter().filter_map(|font| collection.family_id(&format!("asset_id:{:?}", font.id()))).collect();
    if families.is_empty() {
        return;
    }
    let mut changed = false;
    for script in SMOOTH_SCRIPTS {
        let key = parley::fontique::FallbackKey::new(parley::fontique::Script::from_bytes(script), None);
        if !collection.fallback_families(key).eq(families.iter().copied()) {
            collection.set_fallbacks(key, families.iter().copied());
            changed = true;
        }
    }
    // Text laid out before could not fall back.
    if changed {
        for mut font in &mut texts {
            font.set_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use voxmar_engine::block::BlockId;
    use voxmar_engine::lang::arguments;

    use super::*;

    fn assets() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    fn content() -> std::path::PathBuf {
        assets().join("content")
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
        // Biomes, and the kinds of weather, effects and winds, as the console names them.
        let world = voxmar_worldgen::WorldContent::load(content).unwrap();
        let mut own: Vec<(String, String)> =
            world.biomes.iter().filter(|biome| first_party(&biome.name)).map(|biome| (def_key("biome", &biome.name), biome.name.clone())).collect();
        let weather = &content.weather;
        let named = weather.presets().iter().map(|p| ("kind", p.short_name())).chain(weather.winds().iter().map(|w| ("wind", w.short_name())));
        own.extend(named.chain(weather.effects().iter().map(|e| ("effect", e.short_name()))).map(|(what, name)| (format!("weather.{what}.{name}"), name.replace('_', " "))));
        for (key, name) in own {
            let words = voxmar_engine::lang::Words::Own(key.clone(), name);
            names.push((key, locale.render(&words, blocks)));
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
        let loaded = voxmar_server::load_content(&assets()).unwrap();
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
            "rarity", "item", "skills", "character", "language", "effect",
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

    /// Every character anything on screen writes: every language's strings and names, and
    /// the characters the client's own string literals hold (symbols, markers), from the
    /// lines that aren't comments.
    fn every_text_written() -> Vec<String> {
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let mut texts: Vec<String> = Vec::new();
        for language in languages(&content()) {
            let locale = Locale::load(&content(), &language.code);
            texts.extend(locale.keys().map(|key| locale.get(key).to_owned()));
            texts.extend(definition_names(&loaded, &locale).into_iter().map(|(_, name)| name));
        }
        let mut sources = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(path) = sources.pop() {
            if path.is_dir() {
                sources.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
            } else if path.extension().is_some_and(|e| e == "rs") {
                for line in std::fs::read_to_string(&path).unwrap().lines().filter(|line| !line.trim_start().starts_with("//")) {
                    // The text between each pair of quotes, string or character.
                    for quote in ['"', '\''] {
                        let parts: Vec<&str> = line.split(quote).collect();
                        texts.extend(parts.iter().skip(1).step_by(2).filter(|part| !part.is_ascii()).map(|part| part.to_string()));
                    }
                }
            }
        }
        texts
    }

    /// Every text the game writes, shaped as the game shapes it (in each interface font,
    /// falling back to the smooth fonts for the CJK scripts, see `fall_back_to_smooth_fonts`),
    /// has a glyph for every character: no missing-glyph boxes in any language or font.
    #[test]
    fn no_text_shows_a_missing_glyph_box() {
        use parley::fontique::{Blob, FallbackKey, FontInfoOverride, Script};
        use parley::{FontFamily, PositionedLayoutItem, StyleProperty};
        let fonts = assets().join("ui/fonts");
        let read = |name: &str| std::fs::read_to_string(fonts.join(name)).ok();
        let fallback = crate::ui::skin::read_fallback(&fonts);
        let mut cx = parley::FontContext::default();
        let smooth: Vec<_> = crate::ui::skin::smooth_fallbacks(&fonts)
            .into_iter()
            .enumerate()
            .map(|(i, data)| {
                let name = format!("smooth {i}");
                cx.collection.register_fonts(Blob::from(data), Some(FontInfoOverride { family_name: Some(&name), ..Default::default() }))[0].0
            })
            .collect();
        for script in SMOOTH_SCRIPTS {
            cx.collection.set_fallbacks(FallbackKey::new(Script::from_bytes(script), None), smooth.iter().copied());
        }
        let texts = every_text_written();
        assert!(texts.iter().any(|text| text.contains('▴')) && texts.iter().any(|text| text.contains("設定")), "the texts are not all there");
        let mut layouts = parley::LayoutContext::<()>::new();
        let mut boxes = |cx: &mut parley::FontContext, family: &str, text: &str| {
            let mut builder = layouts.ranged_builder(cx, text, 1.0, true);
            builder.push_default(StyleProperty::FontFamily(FontFamily::named(family)));
            builder.push_default(StyleProperty::FontSize(16.0));
            let mut layout = builder.build(text);
            layout.break_all_lines(None);
            layout
                .lines()
                .flat_map(|line| line.items().collect::<Vec<_>>())
                .any(|item| matches!(item, PositionedLayoutItem::GlyphRun(run) if run.glyphs().any(|glyph| glyph.id == 0)))
        };
        for file in ["ledger.txt", "block.txt", "voxmar.txt"] {
            let mut font = voxmar_texgen::font::PixelFont::parse_with(&read(file).unwrap(), read).unwrap();
            font.fill_from(&fallback);
            let family = format!("test {file}");
            let name = FontInfoOverride { family_name: Some(&family), ..Default::default() };
            cx.collection.register_fonts(Blob::from(font.to_ttf(&family)), Some(name));
            // The check itself sees boxes: a private-use character no font draws makes one.
            assert!(boxes(&mut cx, &family, "a\u{E000}") && !boxes(&mut cx, &family, "a 経験値"), "{file}: the box check is broken");
            // Each text with a box, then the characters in it that draw one on their own.
            let mut missing = BTreeSet::new();
            let boxed: Vec<&String> = texts.iter().filter(|text| boxes(&mut cx, &family, text)).collect();
            for text in boxed {
                let alone: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && boxes(&mut cx, &family, &c.to_string())).collect();
                missing.insert(if alone.is_empty() { format!("{text:?}") } else { alone.into_iter().map(|c| format!("{c} (U+{:04X})", c as u32)).collect::<Vec<_>>().join(", ") });
            }
            assert!(missing.is_empty(), "{file}: missing glyphs: {}", missing.into_iter().collect::<Vec<_>>().join("; "));
        }
    }

    /// Every Japanese name with kanji (blocks and items, creatures, skills, conditions) has
    /// a reading, in kana alone, for the interface to show with it.
    #[test]
    fn every_japanese_name_with_kanji_reads() {
        use voxmar_engine::lang::has_kanji;
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let locale = Locale::load(&content(), "ja");
        let mut unread = BTreeSet::new();
        let mut check = |name: String, reading: Option<String>| match reading {
            Some(reading) if !has_kanji(&reading) => {}
            _ if !has_kanji(&name) => {}
            other => {
                unread.insert(format!("{name} ({other:?})"));
            }
        };
        let blocks = &loaded.blocks;
        for id in (0..blocks.len()).map(|i| BlockId(i as u16)) {
            let def = blocks.def(id);
            check(locale.block(def), locale.block_reading(def));
        }
        for (_, creature) in loaded.creatures.iter() {
            check(locale.creature(&creature.def), locale.creature_reading(&creature.def));
        }
        for id in loaded.skills.ids() {
            let def = loaded.skills.get(id);
            check(locale.skill(def), locale.skill_reading(def));
        }
        for key in locale.keys().filter(|key| key.split_once('.').is_some_and(|(kind, rest)| kind == "effect" && !rest.contains('.'))) {
            check(locale.get(key).to_owned(), locale.reading(key));
        }
        assert!(unread.is_empty(), "Japanese names without a reading (add them to lang/readings/ja.ron): {}", unread.into_iter().collect::<Vec<_>>().join(", "));
        let oak = blocks.id("base:oak_log").map(|id| blocks.def(id)).unwrap();
        assert_eq!((locale.block(oak).as_str(), locale.block_reading(oak).as_deref()), ("オークの原木", Some("オークのげんぼく")));
        assert_eq!(Locale::load(&content(), "en").block_reading(oak), None);
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
