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
use voxmar_engine::lang::{ENGLISH, Lang, Language, LineRules, def_key, languages, title_from_id};
use voxmar_engine::skill::SkillDef;
use voxmar_engine::effect::EffectDef;

use crate::settings::Settings;

pub struct LangPlugin;

impl Plugin for LangPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SmoothFonts>()
            .init_resource::<Joined>()
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
                    // Text is laid out joined where the language keeps characters together,
                    // and is as written again right after. Joined only for measuring: this
                    // depends on bevy_ui's `text_system` (PostLayout) breaking lines from
                    // `ComputedTextBlock`, never reading `Text` (bevy_ui widget/text.rs,
                    // 0.19.1). Re-check that when upgrading Bevy.
                    join_for_layout
                        .in_set(bevy::ui::UiSystems::Content)
                        .after(bevy::ui::UiSystems::Prepare)
                        .before(bevy::ui::widget::measure_text_system),
                    unjoin_after_layout.in_set(bevy::ui::UiSystems::Content).after(bevy::ui::widget::measure_text_system),
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

    /// What a prayer is called (`base:bark_skin`).
    pub fn prayer(&self, name: &str) -> String {
        self.get_or(&def_key("prayer", name), &title_from_id(name)).to_owned()
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

/// The texts the language keeps characters of together, as they are laid out (see
/// [`LineRules`]): each swapped into its `Text` just before text is measured and out again
/// just after, so all that reads text between frames (what writes it, compares it, copies
/// or searches it) sees it as written. One place for every wrapped text in the game.
#[derive(Resource, Default)]
struct Joined {
    rules: LineRules,
    /// By text, its other form: the joined one between frames, as written while measured.
    texts: bevy::platform::collections::HashMap<Entity, String>,
}

fn join_for_layout(locale: Res<Locale>, mut joined: ResMut<Joined>, mut texts: Query<(Entity, Mut<Text>)>, mut gone: RemovedComponents<Text>) {
    let joined = &mut *joined;
    for entity in gone.read() {
        joined.texts.remove(&entity);
    }
    let rules_changed = locale.is_changed() && {
        let rules = LineRules::of(&locale);
        let changed = rules != joined.rules;
        joined.rules = rules;
        changed
    };
    for (entity, mut text) in &mut texts {
        if text.is_changed() || rules_changed {
            match joined.rules.join(&text.0) {
                Some(laid_out) => {
                    let written = std::mem::replace(&mut text.bypass_change_detection().0, laid_out);
                    joined.texts.insert(entity, written);
                    // Measured again, joined, though the words are the same.
                    if rules_changed {
                        text.set_changed();
                    }
                }
                None => {
                    if joined.texts.remove(&entity).is_some() && rules_changed {
                        text.set_changed();
                    }
                }
            }
        } else if let Some(other) = joined.texts.get_mut(&entity) {
            std::mem::swap(&mut text.bypass_change_detection().0, other);
        }
    }
}

fn unjoin_after_layout(mut joined: ResMut<Joined>, mut texts: Query<&mut Text>) {
    let joined = &mut *joined;
    for (&entity, other) in joined.texts.iter_mut() {
        if let Ok(mut text) = texts.get_mut(entity) {
            // Nothing writes text while it is joined: a write here would be lost.
            debug_assert_eq!(Some(&text.0), joined.rules.join(other).as_ref(), "text written while it was laid out joined");
            std::mem::swap(&mut text.bypass_change_detection().0, other);
        }
    }
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
    /// it names: blocks and items (a fluid's flowing states show their source's), biomes
    /// and weather, commands' help, the names of alchemy properties and biome tags, and
    /// skills with their descriptions and guides.
    fn definition_names(content: &voxmar_engine::content::Content, locale: &Locale) -> Vec<(String, String)> {
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
        for def in content.commands.iter().filter(|def| first_party(&def.name)) {
            names.push((format!("/{} help", def.short_name()), locale.render(&def.help_words(), blocks)));
            names.push((format!("/{} summary", def.short_name()), locale.render(&def.summary_words(), blocks)));
        }
        for key in built_keys(content) {
            let own = key.rsplit('.').next().unwrap_or(&key).replace('_', " ");
            names.push((key.clone(), locale.get_or(&key, &own).to_owned()));
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

    /// The keys the code builds from first-party content rather than names: what each
    /// `alchemy:<property>` tag is called (`alchemy.<property>`, in item tooltips), each
    /// biome tag (`biome.tag.<tag>`, in `/biome`), what creatures are doing
    /// (`creature.doing.<module>`), animals' genes and alleles (`gene.<species>.<gene>`, in
    /// `/inspect`; people's are never read out), the weather's channels
    /// (`weather.channel.<name>`), the quest book's chapters (`quest.age.<age>`), and the
    /// fonts and shader packs in the settings (`settings.font.<font>`, `shader_pack.<pack>`).
    fn built_keys(content: &voxmar_engine::content::Content) -> BTreeSet<String> {
        let blocks = &content.blocks;
        let mut keys = BTreeSet::new();
        for id in (0..blocks.len()).map(|i| BlockId(i as u16)) {
            let def = blocks.def(id);
            if first_party(&def.name) {
                keys.extend(def.tags.iter().filter_map(|tag| tag.strip_prefix("alchemy:")).map(|property| format!("alchemy.{property}")));
            }
        }
        let world = voxmar_worldgen::WorldContent::load(content).unwrap();
        for biome in world.biomes.iter().filter(|biome| first_party(&biome.name)) {
            keys.extend(biome.tags.iter().map(|tag| format!("biome.tag.{tag}")));
        }
        // What `/creatures` says each one is doing: its brain's module, or resting.
        keys.insert("creature.doing.resting".into());
        for (_, species) in content.creatures.iter().filter(|(_, species)| first_party(&species.def.name)) {
            keys.extend(species.modules.iter().map(|(module, _)| format!("creature.doing.{}", module.name())));
        }
        // What inspecting an animal names: its genes and their alleles (never a person's).
        let animals = content.creatures.iter().filter(|(_, species)| species.def.kind != voxmar_engine::creature::Kind::Villager);
        for (_, species) in animals.filter(|(_, species)| first_party(&species.def.name)) {
            let name = &species.def.name;
            for gene in &species.def.genes {
                keys.insert(def_key("gene", &format!("{name}.{}", gene.name)));
                if let voxmar_engine::genes::GeneKind::Alleles(alleles) = &gene.kind {
                    keys.extend(alleles.iter().map(|allele| def_key("gene", &format!("{name}.{}.{}", gene.name, allele.name))));
                }
            }
        }
        // The weather's channels in `/weather` and the F3 screen, and what falls.
        let channels = voxmar_engine::weather::Weather::default().channels().map(|(name, _)| name);
        keys.extend(channels.chain(["rain", "snow"]).map(|name| format!("weather.channel.{name}")));
        // Armour sets by name, and what each kind of pouch holds, in item tooltips.
        for id in (0..blocks.len()).map(|i| BlockId(i as u16)).filter(|&id| first_party(&blocks.def(id).name)) {
            if let Some(piece) = content.combat.piece(id) {
                keys.insert(format!("armour_set.{}", content.combat.set(piece.set).name));
            }
            keys.extend(blocks.def(id).tags.iter().filter_map(|tag| tag.strip_prefix("holds:")).map(|kind| format!("pouch.fills.{kind}")));
        }
        // The prayer book's names (the UI draws them).
        keys.extend(content.prayers.all().filter(|(_, prayer)| first_party(&prayer.name)).map(|(_, prayer)| def_key("prayer", &prayer.name)));
        // What each prayer's effects say, built from the effect's kind and stat.
        keys.extend(["keep", "mending", "last_breath", "ward"].map(|effect| format!("prayer.effect.{effect}")));
        keys.extend(["defence", "strength", "accuracy"].map(|stat| format!("prayer.effect.boost.{stat}")));
        // What each first-party package is called, at the foot of every item's tooltip.
        keys.extend(FIRST_PARTY.iter().map(|package| format!("package.{package}")));
        // The quest book's chapters, one per age the first-party quests name.
        keys.extend(content.quests.iter().filter(|quest| first_party(&quest.name)).map(|quest| format!("quest.age.{}", quest.age)));
        // And each age a rite opens, which a recipe greyed for it names.
        keys.extend(content.quests.iter().filter(|quest| first_party(&quest.name)).filter_map(|quest| quest.opens.as_ref()).map(|age| format!("quest.age.{age}")));
        // The interface fonts and the shader packs the game ships, as the settings name them.
        keys.extend(crate::ui::skin::InterfaceFont::ALL.map(|font| font.key().to_owned()));
        keys.extend(crate::render::pack::ShaderPacks::load(&assets()).0.iter().filter(|pack| pack.shipped).map(crate::render::pack::ShaderPack::key));
        keys
    }

    /// Whether a definition's name is first-party's.
    fn first_party(name: &str) -> bool {
        FIRST_PARTY.iter().any(|package| name.strip_prefix(package).is_some_and(|rest| rest.starts_with(':')))
    }

    /// The English first-party definitions carry themselves, by the key that names it:
    /// titles, descriptions, guide lines and command help. The language files say the
    /// same, so a translator works from a complete English file.
    fn english_in_definitions(content: &voxmar_engine::content::Content) -> Vec<(String, String)> {
        let mut own = Vec::new();
        for id in content.effects.ids() {
            let def = content.effects.get(id);
            if first_party(&def.name) {
                let key = def_key("effect", &def.name);
                own.push((format!("{key}.description"), def.description.clone()));
                if !def.next.is_empty() {
                    own.push((format!("{key}.next"), def.next.clone()));
                }
                own.push((key, def.title.clone()));
            }
        }
        for (_, creature) in content.creatures.iter().filter(|(_, creature)| first_party(&creature.def.name)) {
            let def = &creature.def;
            own.push((def_key("creature", &def.name), def.title.clone()));
            for breed in &def.breeds {
                let title = if breed.title.is_empty() { title_from_id(&breed.name) } else { breed.title.clone() };
                own.push((def_key("breed", &format!("{}.{}", def.name, breed.name)), title));
            }
        }
        for id in content.skills.ids() {
            let def = content.skills.get(id);
            let key = def_key("skill", &def.name);
            own.push((format!("{key}.description"), def.description.clone()));
            for (i, step) in def.guide.iter().enumerate() {
                own.push((format!("{key}.guide.{i}"), step.text.clone()));
                if let Some(how) = &step.how {
                    own.push((format!("{key}.guide.{i}.how"), how.clone()));
                }
            }
            own.push((key, def.title.clone()));
        }
        for def in content.commands.iter().filter(|def| first_party(&def.name)) {
            for words in [def.help_words(), def.summary_words()] {
                if let voxmar_engine::lang::Words::Own(key, english) = words {
                    own.push((key, english));
                }
            }
        }
        for quest in content.quests.iter().filter(|quest| first_party(&quest.name)) {
            let key = def_key("quest", &quest.name);
            own.push((format!("{key}.why"), quest.why.clone()));
            own.push((key, quest.title.clone()));
        }
        // The quest book's paths, on the first-party quests' ages.
        let ages: Vec<&str> = content.quests.iter().filter(|quest| first_party(&quest.name)).map(|quest| quest.age.as_str()).collect();
        own.extend(content.quest_paths.iter().filter(|path| ages.contains(&path.age.as_str())).map(|path| (path.key(), path.title.clone())));
        for structure in content.multiblocks.iter().filter(|structure| first_party(&structure.name)) {
            let key = def_key("multiblock", &structure.name);
            own.push((format!("{key}.hint"), structure.hint.clone()));
            own.push((key, structure.title.clone()));
        }
        own
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

    /// Every key the interface's code names is in English, and so is every key it builds
    /// from content (see [`built_keys`]). A setting's `.hint` is checked in `settings`, and
    /// `key.<code>` in `keys`.
    #[test]
    fn every_key_in_the_code_exists() {
        let english = Lang::load(&content(), ENGLISH);
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let mut unknown: BTreeSet<String> = built_keys(&loaded).into_iter().filter(|key| english.find(key).is_none()).map(|key| format!("content: {key}")).collect();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![source];
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

    /// Each first-party package's languages have the same keys (Japanese needs no `.one`
    /// form of a count), so a translator works from a complete English file and no
    /// translation keeps a string for a key that is gone; and where a definition writes its
    /// own English (see [`english_in_definitions`]), English says the same, so the two
    /// never drift apart.
    #[test]
    fn every_language_has_the_same_keys() {
        use std::collections::BTreeMap;
        let read = |package: &str, code: &str| -> BTreeMap<String, String> {
            let path = content().join(package).join(format!("lang/{code}.ron"));
            let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            ron::from_str(&source).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        };
        let mut wrong = Vec::new();
        for package in FIRST_PARTY {
            let english = read(package, ENGLISH);
            // The languages first-party ships; a mod's own language falls back to English.
            let shipped = |code: &str| FIRST_PARTY.iter().any(|package| content().join(package).join(format!("lang/{code}.ron")).exists());
            for language in languages(&content()).into_iter().filter(|language| language.code != ENGLISH && shipped(&language.code)) {
                let other = read(package, &language.code);
                let code = &language.code;
                wrong.extend(english.keys().filter(|key| !key.ends_with(".one") && !other.contains_key(*key)).map(|key| format!("{package}/lang/{code}.ron lacks `{key}`")));
                wrong.extend(other.keys().filter(|key| !english.contains_key(*key)).map(|key| format!("{package}/lang/en.ron lacks `{key}` (or {code}.ron keeps a key that is gone)")));
            }
        }
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let english = Lang::load(&content(), ENGLISH);
        for (key, own) in english_in_definitions(&loaded) {
            match english.find(&key) {
                Some(text) if text == own => {}
                Some(text) => wrong.push(format!("`{key}` is {own:?} in its definition but {text:?} in en.ron: change both (and the other languages)")),
                None => wrong.push(format!("en.ron lacks `{key}`: {own:?}")),
            }
        }
        assert!(wrong.is_empty(), "{} language keys out of step:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// Every English name of a block, item or multiblock reads in Title Case ("HV Battery
    /// Bank"): each word begins with a capital, but for the small words (of, and, the, a,
    /// with) after the first.
    #[test]
    fn every_english_name_is_in_title_case() {
        use std::collections::BTreeMap;
        const SMALL: [&str; 5] = ["of", "and", "the", "a", "with"];
        let mut wrong = Vec::new();
        for package in FIRST_PARTY {
            let path = content().join(package).join(format!("lang/{ENGLISH}.ron"));
            let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let english: BTreeMap<String, String> = ron::from_str(&source).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            // A name's key is `kind.package.id`; longer keys (a multiblock's hint) are sentences,
            // and `item.style.*` are words used in them.
            let names = english.iter().filter(|(key, _)| {
                let parts: Vec<&str> = key.split('.').collect();
                parts.len() == 3 && ["block", "item", "multiblock"].contains(&parts[0]) && parts[1] == package
            });
            for (key, name) in names {
                // A family's `{name}` is filled with a name that is itself checked.
                let lower = |word: &str| !word.starts_with('{') && word.chars().find(|c| c.is_alphabetic()).is_some_and(char::is_lowercase);
                if let Some((_, word)) = name.split(' ').enumerate().find(|&(i, word)| lower(word) && (i == 0 || !SMALL.contains(&word))) {
                    wrong.push(format!("{package}/lang/{ENGLISH}.ron: `{key}` is {name:?}, but `{word}` wants a capital"));
                }
            }
        }
        assert!(wrong.is_empty(), "{} names not in Title Case:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// No first-party language file (strings, readings or pins) gives a key twice: the
    /// game keeps the first and reports the repeat, and a key added again by mistake would
    /// otherwise say the wrong words where the second was meant.
    #[test]
    fn no_language_file_gives_a_key_twice() {
        let mut files = Vec::new();
        for package in FIRST_PARTY {
            for sub in ["lang", "lang/readings", "lang/pins"] {
                let dir = content().join(package).join(sub);
                for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).flatten() {
                    if entry.path().extension().is_some_and(|ext| ext == "ron") {
                        files.push(entry.path());
                    }
                }
            }
        }
        assert!(files.len() >= 8, "{files:?}");
        let wrong: Vec<String> = files
            .iter()
            .filter_map(|path| {
                let source = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                match voxmar_engine::lang::parse_map::<ron::Value>(&source) {
                    Ok((_, again)) if again.is_empty() => None,
                    Ok((_, again)) => Some(format!("{}: given twice: {again:?}", path.display())),
                    Err(e) => Some(format!("{}: {e}", path.display())),
                }
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Every translation of English that lives in a definition (guide lines, help, titles)
    /// was made from the English there now: each package's `lang/pins/<code>.ron` keeps,
    /// by key, the English a translation followed and the translation written for it. So a
    /// line inserted into a skill's guide (which shifts every later line's key), English
    /// reworded, or a translation changed on its own fails here until someone checks the
    /// two agree and updates the pin, instead of showing the wrong line.
    #[test]
    fn every_translation_follows_its_english() {
        use std::collections::BTreeMap;
        fn read<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<BTreeMap<String, T>> {
            let source = std::fs::read_to_string(path).ok()?;
            Some(ron::from_str(&source).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        }
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let own: BTreeMap<String, String> = english_in_definitions(&loaded).into_iter().collect();
        let mut wrong = Vec::new();
        for language in languages(&content()).into_iter().filter(|language| language.code != ENGLISH) {
            let code = &language.code;
            for package in FIRST_PARTY {
                let Some(strings) = read::<String>(&content().join(package).join(format!("lang/{code}.ron"))) else { continue };
                let pins = read::<(String, String)>(&content().join(package).join(format!("lang/pins/{code}.ron"))).unwrap_or_default();
                let file = format!("{package}/lang/pins/{code}.ron");
                for (key, english) in own.iter() {
                    let Some(translation) = strings.get(key) else { continue };
                    match pins.get(key) {
                        Some((pinned, written)) if pinned == english && written == translation => {}
                        Some((pinned, _)) if pinned != english => {
                            wrong.push(format!("`{key}` in {package}/lang/{code}.ron follows {pinned:?}, but English now says {english:?}: change it, then its pin in {file}"))
                        }
                        Some((_, written)) => wrong.push(format!("`{key}` in {package}/lang/{code}.ron was {written:?} and is now {translation:?}: if it still follows {english:?}, update its pin in {file}")),
                        None => wrong.push(format!("{file} lacks `{key}`: {key:?}: ({english:?}, {translation:?}),")),
                    }
                }
                wrong.extend(pins.keys().filter(|key| !own.contains_key(*key) || !strings.contains_key(*key)).map(|key| format!("{file} pins `{key}`, which {code} no longer translates")));
            }
        }
        assert!(wrong.is_empty(), "{} translations out of step with their English:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// The string literals in Rust or RON source, as written (escapes left in), past line
    /// and block comments, character literals and raw strings.
    fn string_literals(source: &str) -> Vec<String> {
        let chars: Vec<char> = source.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '/' if chars.get(i + 1) == Some(&'/') => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                // Block comments, nested as Rust nests them.
                '/' if chars.get(i + 1) == Some(&'*') => {
                    let mut depth = 0;
                    while i < chars.len() {
                        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                            depth += 1;
                            i += 1;
                        } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                            depth -= 1;
                            i += 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        i += 1;
                    }
                }
                // A character literal ('"', '\''), not a lifetime ('a).
                '\'' if chars.get(i + 1) == Some(&'\\') => {
                    i += 2;
                    while i < chars.len() && chars[i] != '\'' {
                        i += 1;
                    }
                }
                '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                'r' if matches!(chars.get(i + 1), Some('#' | '"')) && (i == 0 || !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_') => {
                    let hashes = chars[i + 1..].iter().take_while(|&&c| c == '#').count();
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        let start = i + 2 + hashes;
                        let mut end = start;
                        while end < chars.len() && !(chars[end] == '"' && chars[end + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes) {
                            end += 1;
                        }
                        out.push(chars[start..end.min(chars.len())].iter().collect());
                        i = end + hashes;
                    }
                }
                '"' => {
                    let start = i + 1;
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        i += if chars[i] == '\\' { 2 } else { 1 };
                    }
                    out.push(chars[start..i.min(chars.len())].iter().collect());
                }
                _ => {}
            }
            i += 1;
        }
        out
    }

    #[test]
    fn string_literals_skip_comments_and_characters() {
        let source = "let a = \"menu.done\"; // \"menu.gone\"\n/* \"menu.gone\" /* \"menu.gone\" */ */ let c = '\"'; let d: &'static str = r#\"x \"y\" z\"#; \"esc \\\" q\"";
        assert_eq!(string_literals(source), ["menu.done", "x \"y\" z", "esc \\\" q"]);
    }

    /// No language file keeps a key nothing uses: each first-party English key is a block's
    /// or family's name as [`Lang::block`] looks it up, English a definition carries or a key
    /// built from content (see [`english_in_definitions`], [`built_keys`]), or a key some
    /// code or data names, whole or in a pattern (`"tip.{name}"`, `"{key}.hint"`). So
    /// renaming or dropping something drops its words too, instead of leaving them behind.
    #[test]
    fn no_language_keeps_a_key_nothing_uses() {
        use std::collections::BTreeMap;
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let english = Lang::load(&content(), ENGLISH);
        let mut used: BTreeSet<String> = built_keys(&loaded);
        used.extend(english_in_definitions(&loaded).into_iter().map(|(key, _)| key));
        used.extend(definition_names(&loaded, &Locale::load(&content(), ENGLISH)).into_iter().map(|(key, _)| key));
        // Blocks by the keys their names come from (see `Lang::block`).
        let blocks = &loaded.blocks;
        for def in (0..blocks.len()).map(|i| blocks.def(BlockId(i as u16))) {
            let own = def_key("block", &def.name);
            if english.find(&own).is_some() {
                used.insert(own);
            } else if let Some(family) = &def.family {
                let (namespace, local) = def.name.split_once(':').unwrap_or(("", &def.name));
                used.insert(format!("block.{namespace}.{}", local.replacen(family.rsplit(':').next().unwrap_or(family), "{name}", 1)));
                used.insert(def_key("family", family));
            }
        }
        // Every string in the code and the data, whole, or as a pattern where it fills in
        // `{...}` (with at least a word of its own, so `"{}.{}"` matches nothing). The lang
        // modules' own patterns (`"block.{namespace}.{pattern}"`) are what is modelled above.
        let mut patterns: Vec<Vec<String>> = Vec::new();
        let mut files = vec![assets().join("../crates"), content()];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "lang" || name == "target") {
                    continue;
                }
                files.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs" && e != "ron") {
                continue;
            }
            for literal in string_literals(&std::fs::read_to_string(&path).unwrap()) {
                let literal = literal.as_str();
                if !literal.contains('{') {
                    used.insert(literal.to_owned());
                    continue;
                }
                let parts: Vec<String> = literal.split('{').enumerate().map(|(i, part)| if i == 0 { part } else { part.split_once('}').map_or("", |(_, rest)| rest) }.to_owned()).collect();
                if !path.ends_with("lang.rs") && parts.iter().any(|part| part.trim_matches('.').len() > 1) && parts.iter().all(|part| part.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')) {
                    patterns.push(parts);
                }
            }
        }
        // Whether `key` is `parts` with each gap filled by a word or more; one that only adds
        // to another key (`"{key}.hint"`) is used where that key is.
        let fits = |key: &str, parts: &[String]| {
            if let [stem, suffix] = parts
                && stem.is_empty()
            {
                return key.strip_suffix(suffix.as_str()).is_some_and(|stem| used.contains(stem));
            }
            let Some(mut rest) = key.strip_prefix(parts[0].as_str()) else { return false };
            for (i, part) in parts.iter().enumerate().skip(1) {
                let last = i == parts.len() - 1;
                let found = if last { rest.strip_suffix(part.as_str()).map(|before| before.len()).filter(|_| part.len() <= rest.len()) } else { rest.get(1..).and_then(|after| after.find(part.as_str())).map(|at| at + 1) };
                let Some(at) = found.filter(|&at| at > 0) else { return false };
                rest = &rest[at + if last { rest.len() - at } else { part.len() }..];
            }
            parts.len() > 1 && rest.is_empty()
        };
        let mut unused = Vec::new();
        for package in FIRST_PARTY {
            let path = content().join(package).join("lang/en.ron");
            let strings: BTreeMap<String, String> = ron::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for key in strings.keys() {
                // A count's forms are used with it.
                let count = key.strip_suffix(".one").or_else(|| key.strip_suffix(".other"));
                // What content names is only what it names (`"block.{kind}.{event}"` is a sound).
                let modelled = ["block", "family", "biome", "creature", "breed", "gene", "effect", "skill", "quest", "multiblock", "alchemy"].contains(&key.split('.').next().unwrap_or(""));
                if ![Some(key.as_str()), count].into_iter().flatten().any(|key| used.contains(key) || !modelled && patterns.iter().any(|parts| fits(key, parts))) {
                    unused.push(format!("{package}/lang/en.ron: {key}"));
                }
            }
        }
        assert!(unused.is_empty(), "{} keys nothing uses (drop them from every language, or use them):\n{}", unused.len(), unused.join("\n"));
    }

    /// Every character anything on screen writes: every language's strings and names (as
    /// they are laid out, joined, too), and the characters the client's own string literals
    /// hold (symbols, markers), from the lines that aren't comments.
    fn every_text_written() -> Vec<String> {
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let mut texts: Vec<String> = Vec::new();
        for language in languages(&content()) {
            let locale = Locale::load(&content(), &language.code);
            let start = texts.len();
            texts.extend(locale.keys().map(|key| locale.get(key).to_owned()));
            texts.extend(definition_names(&loaded, &locale).into_iter().map(|(_, name)| name));
            let rules = LineRules::of(&locale);
            let joined: Vec<String> = texts[start..].iter().filter_map(|text| rules.join(text)).collect();
            texts.extend(joined);
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
    /// has a glyph for every character: no missing-glyph boxes in any language or font. One
    /// test a font, so the three shape side by side.
    #[test]
    fn no_text_shows_a_missing_glyph_box_in_the_ledger_font() {
        no_text_shows_a_missing_glyph_box("ledger.txt");
    }

    #[test]
    fn no_text_shows_a_missing_glyph_box_in_the_block_font() {
        no_text_shows_a_missing_glyph_box("block.txt");
    }

    #[test]
    fn no_text_shows_a_missing_glyph_box_in_the_voxmar_font() {
        no_text_shows_a_missing_glyph_box("voxmar.txt");
    }

    fn no_text_shows_a_missing_glyph_box(file: &str) {
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
        {
            let mut font = voxmar_texgen::font::PixelFont::parse_with(&read(file).unwrap(), read).unwrap();
            font.fill_from(&fallback);
            let family = format!("test {file}");
            let name = FontInfoOverride { family_name: Some(&family), ..Default::default() };
            cx.collection.register_fonts(Blob::from(font.to_ttf(&family)), Some(name));
            // The check itself sees boxes: a private-use character no font draws makes one
            // (outside the Basic Multilingual Plane: Fusion Pixel draws some of its own).
            assert!(boxes(&mut cx, &family, "a\u{10FFFD}") && !boxes(&mut cx, &family, "a 経験値"), "{file}: the box check is broken");
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

    /// Text is joined only while it is laid out: before and after, and to anything that
    /// writes or reads it, it is as written, and it is joined again when it changes or the
    /// language does.
    #[test]
    fn text_is_joined_only_while_laid_out() {
        #[derive(Resource, Default)]
        struct Seen(Vec<String>);
        let mut app = App::new();
        app.insert_resource(Locale::load(&content(), "ja")).init_resource::<Joined>().init_resource::<Seen>();
        app.add_systems(Update, (join_for_layout, |texts: Query<&Text>, mut seen: ResMut<Seen>| seen.0.extend(texts.iter().map(|text| text.0.clone())), unjoin_after_layout).chain());
        let text = app.world_mut().spawn(Text::new("クリック。")).id();
        let j = voxmar_engine::lang::WORD_JOINER;
        let frame = |app: &mut App| {
            app.update();
            let seen = std::mem::take(&mut app.world_mut().resource_mut::<Seen>().0);
            (seen, app.world().get::<Text>(text).unwrap().0.clone())
        };
        assert_eq!(frame(&mut app), (vec![format!("ク{j}リ{j}ッ{j}ク{j}。")], "クリック。".to_owned()));
        assert_eq!(frame(&mut app), (vec![format!("ク{j}リ{j}ッ{j}ク{j}。")], "クリック。".to_owned()), "joined again on a frame it does not change");
        app.world_mut().get_mut::<Text>(text).unwrap().0 = "ボタン".into();
        assert_eq!(frame(&mut app).0, vec![format!("ボ{j}タ{j}ン")]);
        // English has no rules: as written throughout.
        *app.world_mut().resource_mut::<Locale>() = Locale::load(&content(), "en");
        assert_eq!(frame(&mut app), (vec!["ボタン".to_owned()], "ボタン".to_owned()));
    }

    /// A text written while it is joined (between being joined and measured, and back)
    /// would be lost: debug runs say so.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "text written while it was laid out joined")]
    fn writing_joined_text_is_caught() {
        let mut app = App::new();
        app.insert_resource(Locale::load(&content(), "ja")).init_resource::<Joined>();
        app.add_systems(Update, (join_for_layout, |mut texts: Query<&mut Text>| texts.iter_mut().for_each(|mut text| text.0 = "ボタン".into()), unjoin_after_layout).chain());
        app.world_mut().spawn(Text::new("クリック"));
        app.update();
    }

    /// Japanese text wraps as kinsoku wants at every width: no line starts with closing
    /// punctuation or a small kana, none ends with an opening bracket, and no katakana word
    /// splits, while a line still breaks between any two other kanji or kana. Laid out as
    /// the game lays it out (joined, see [`Joined`]) in the CJK fonts, from Japanese's
    /// own strings and names.
    #[test]
    fn japanese_wraps_where_kinsoku_allows() {
        use parley::fontique::{Blob, FontInfoOverride};
        use parley::{FontFamily, StyleProperty};
        let fonts = assets().join("ui/fonts");
        let mut cx = parley::FontContext::default();
        let data = crate::ui::skin::smooth_fallbacks(&fonts).into_iter().next().expect("a CJK font");
        cx.collection.register_fonts(Blob::from(data), Some(FontInfoOverride { family_name: Some("cjk"), ..Default::default() }));
        let locale = Locale::load(&content(), "ja");
        let rules = LineRules::of(&locale);
        assert!(!rules.is_empty(), "Japanese gives no line rules");
        let loaded = voxmar_server::load_content(&assets()).unwrap();
        let mut texts: Vec<String> = locale.keys().filter(|key| !key.starts_with("language.")).map(|key| locale.get(key).to_owned()).collect();
        texts.extend(definition_names(&loaded, &locale).into_iter().map(|(_, name)| name));
        texts.retain(|text| text.chars().any(drawn_smooth) && rules.join(text).is_some());
        texts.sort();
        texts.dedup();
        let mut layouts = parley::LayoutContext::<()>::new();
        // Where `text` breaks into lines `width` wide: the index in it, as written, of each
        // character starting a line after the first.
        let mut breaks = |text: &str, width: f32| -> Vec<usize> {
            let mut builder = layouts.ranged_builder(&mut cx, text, 1.0, true);
            builder.push_default(StyleProperty::FontFamily(FontFamily::named("cjk")));
            builder.push_default(StyleProperty::FontSize(16.0));
            let mut layout = builder.build(text);
            layout.break_all_lines(Some(width));
            let written = |end: usize| text[..end].chars().filter(|&c| c != voxmar_engine::lang::WORD_JOINER).count();
            layout.lines().skip(1).map(|line| written(line.text_range().start)).collect()
        };
        let widths = [48.0, 80.0, 120.0, 176.0, 260.0];
        let mut wrong = BTreeSet::new();
        let mut unjoined = 0;
        for text in &texts {
            let (joined, chars) = (rules.join(text).unwrap(), text.chars().collect::<Vec<_>>());
            let held = rules.held(&chars);
            let shown = |at: usize| format!("{}|{}", chars[..at].iter().collect::<String>(), chars[at..].iter().collect::<String>());
            for width in widths {
                wrong.extend(breaks(&joined, width).into_iter().filter(|&at| held.get(at) == Some(&true)).map(|at| format!("{} ({width} wide)", shown(at))));
                unjoined += breaks(text, width).into_iter().filter(|&at| held.get(at) == Some(&true)).count();
            }
        }
        // The check sees the breaks joining keeps out.
        assert!(texts.len() > 200 && unjoined > 0, "the check sees no breaks to keep ({} texts)", texts.len());
        assert!(wrong.is_empty(), "{} Japanese lines break where kinsoku forbids:\n{}", wrong.len(), wrong.into_iter().take(40).collect::<Vec<_>>().join("\n"));
        // And a line still breaks between kanji and kana mid-sentence, not only at punctuation.
        let sentence: Vec<char> = "経験値は行動ごとに入り、レベルが上がるほど手に入るものが増えていきます。".chars().collect();
        let joined = rules.join(&sentence.iter().collect::<String>()).unwrap();
        let at = breaks(&joined, 120.0);
        assert!(at.iter().any(|&at| sentence[at - 1].is_alphabetic() && sentence[at].is_alphabetic()), "no break between kanji or kana: {at:?}");
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
        for key in locale.keys().filter(|key| key.split('.').count() == 3 && key.split('.').next() == Some("effect")) {
            check(locale.get(key).to_owned(), locale.reading(key));
        }
        assert!(unread.is_empty(), "Japanese names without a reading (add them to lang/readings/ja.ron): {}", unread.into_iter().collect::<Vec<_>>().join(", "));
        let oak = blocks.id("base:oak_log").map(|id| blocks.def(id)).unwrap();
        assert_eq!((locale.block(oak).as_str(), locale.block_reading(oak).as_deref()), ("オークの原木", Some("オークのげんぼく")));
        assert_eq!(Locale::load(&content(), "en").block_reading(oak), None);
    }

    /// A command's reply counting creatures says "1 creature" in English, and Japanese
    /// counts with its one form.
    #[test]
    fn a_reply_counts_its_creatures() {
        let blocks = voxmar_server::load_content(&assets()).unwrap().blocks;
        let done = |code: &str, count: usize| {
            Lang::load(&content(), code).render(&voxmar_engine::lang::Words::key("command.creatures.done").with("count", voxmar_engine::lang::Words::text(count)), &blocks)
        };
        assert_eq!(done(ENGLISH, 1), "Done, for 1 creature within 8 blocks.");
        assert_eq!(done(ENGLISH, 3), "Done, for 3 creatures within 8 blocks.");
        assert_eq!(done("ja", 1), "8ブロック以内の1体に行いました。");
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
