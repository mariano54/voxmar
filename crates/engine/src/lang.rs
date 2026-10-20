//! Languages: every word the game shows a player, looked up by key in the player's
//! language.
//!
//! Each content package may ship `lang/<code>.ron` files (`lang/en.ron`, `lang/ja.ron`),
//! each a map from keys to text, so a mod translates its own blocks and skills the way it
//! defines them, and a translation is a package that adds one file per package it covers.
//! The interface's own words are `core`'s.
//!
//! Keys are dotted paths: `menu.title.play`. Definitions get theirs from their names, so
//! content needs no keys of its own: block `base:oak_log` is `block.base.oak_log` (see
//! [`def_key`]). Text names its arguments in braces, `"Plants {crop}"`, so a language puts
//! them wherever its grammar wants; `{{` and `}}` write braces.
//!
//! A lookup falls back from the player's language to English, then to a default the
//! caller has (a definition's own title, or a name made from its id), and finally to the
//! key itself, so a missing string shows up on screen as something to grep for, never as
//! nothing.

use std::fmt::{Display, Write};
use std::path::Path;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::block::{BlockDef, BlockRegistry};

/// The language every other falls back to, and the one every key must exist in.
pub const ENGLISH: &str = "en";

/// The key every language file names its language with, in that language: "日本語".
pub const NAME_KEY: &str = "language.name";

/// The strings of one language, and English's to fall back to.
#[derive(Clone, Debug, Default)]
pub struct Lang {
    code: String,
    strings: FxHashMap<String, String>,
    english: FxHashMap<String, String>,
    /// How this language's names read, by their keys (`lang/readings/<code>.ron`).
    readings: FxHashMap<String, String>,
}

/// A language some package has strings for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    /// Its name in itself, or its code when no file names it.
    pub name: String,
}

impl Lang {
    /// Reads language `code` (and English) from every package in `content_dir`. Files that
    /// cannot be read are reported and skipped: a broken translation never stops the game.
    pub fn load(content_dir: &Path, code: &str) -> Self {
        let files = read_files(content_dir, "lang", |file_code| file_code == code || file_code == ENGLISH);
        let (mut lang, problems) = Self::from_files(code, files.iter().map(|(package, code, source)| (package.as_str(), code.as_str(), source.as_str())));
        for problem in problems {
            tracing::warn!("{problem}");
        }
        for (package, _, source) in read_files(content_dir, "lang/readings", |file_code| file_code == code) {
            match parse(&source) {
                // As with strings, the first package to give a reading keeps it.
                Ok((readings, again)) => {
                    for key in again {
                        tracing::warn!("{package}/lang/readings/{code}.ron: `{key}` is given twice; the first is kept");
                    }
                    readings.into_iter().for_each(|(key, reading)| {
                        lang.readings.entry(key).or_insert(reading);
                    })
                }
                Err(e) => tracing::warn!("{package}/lang/readings/{code}.ron: {e}"),
            }
        }
        lang
    }

    /// Builds language `code` from `(package, language, source)` files, in package order,
    /// returning what was wrong with them as well.
    pub fn from_files<'a>(code: &str, files: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) -> (Self, Vec<String>) {
        let mut lang = Lang { code: code.to_owned(), ..Default::default() };
        let mut problems = Vec::new();
        for (package, file_code, source) in files {
            let table = if file_code == code {
                &mut lang.strings
            } else if file_code == ENGLISH {
                &mut lang.english
            } else {
                continue;
            };
            match parse(source) {
                Ok((strings, again)) => {
                    problems.extend(again.into_iter().map(|key| format!("{package}/lang/{file_code}.ron: `{key}` is given twice; the first is kept")));
                    for (key, text) in strings {
                        // The first package (in name order) to define a key keeps it, so
                        // which wins never depends on how files happen to load.
                        if table.contains_key(&key) {
                            problems.push(format!("{package}/lang/{file_code}.ron: `{key}` is already defined by another package"));
                        } else {
                            table.insert(key, text);
                        }
                    }
                }
                Err(e) => problems.push(format!("{package}/lang/{file_code}.ron: {e}")),
            }
        }
        if code == ENGLISH {
            lang.english = std::mem::take(&mut lang.strings);
        }
        (lang, problems)
    }

    /// The language's code, such as `ja`.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The text for `key`, or the key itself when no language has it.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        self.find(key).unwrap_or(key)
    }

    /// The text for `key`, or `default` when no language has it.
    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.find(key).unwrap_or(default)
    }

    /// Whether this language itself has words for `key`, without falling back to English:
    /// the key's own, or its counted form (`key.other`, which every language has).
    pub fn defines(&self, key: &str) -> bool {
        let table = if self.code == ENGLISH { &self.english } else { &self.strings };
        table.contains_key(key) || table.contains_key(&format!("{key}.other"))
    }

    /// The text for `key` in this language or English, if either has it.
    pub fn find(&self, key: &str) -> Option<&str> {
        self.strings.get(key).or_else(|| self.english.get(key)).map(String::as_str)
    }

    /// The text for `key` with its `{name}`s replaced by `args`.
    pub fn format(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        fill(self.get(key), args)
    }

    /// The text for a count of something, `{count}` and `args` filled in: `key.one` for
    /// one where the language has it ("1 minute ago"), `key.other` otherwise ("5 minutes
    /// ago"; Japanese needs only this one).
    pub fn count(&self, key: &str, count: u64, args: &[(&str, &dyn Display)]) -> String {
        let (one, other) = (format!("{key}.one"), format!("{key}.other"));
        let own = if self.code == ENGLISH { &self.english } else { &self.strings };
        fn pick<'a>(table: &'a FxHashMap<String, String>, one: Option<&str>, other: &str) -> Option<&'a String> {
            one.and_then(|one| table.get(one)).or_else(|| table.get(other))
        }
        let one = (count == 1).then_some(one.as_str());
        let text = pick(own, one, &other).or_else(|| pick(&self.english, one, &other)).map_or(other.as_str(), String::as_str);
        let mut all: Vec<(&str, &dyn Display)> = vec![("count", &count)];
        all.extend_from_slice(args);
        fill(text, &all)
    }

    /// What a block or item is called: its own name in the language (`block.base.oak_log`),
    /// or, for a block made from a template, the template member's name with the family's
    /// filled in (`block.base.{name}_log` with `family.base.oak`), so a language names a
    /// whole family of woods or rocks in two lines; another look of a plant goes by the
    /// plant's name, and a block turned to face another way goes by the block's (a wall
    /// torch facing east is a wall torch). Without any of those, a name made from its id:
    /// "Oak Log".
    pub fn block(&self, def: &BlockDef) -> String {
        let name = def.form_of().unwrap_or(&def.name);
        if let Some(name) = self.find(&def_key("block", name)) {
            return name.to_owned();
        }
        let (namespace, local) = name.split_once(':').unwrap_or(("", name));
        if let Some(family) = &def.family {
            let family_local = family.rsplit(':').next().unwrap_or(family);
            let pattern = local.replacen(family_local, "{name}", 1);
            if let (Some(member), Some(family)) = (self.find(&format!("block.{namespace}.{pattern}")), self.find(&def_key("family", family))) {
                return fill(member, &[("name", &family)]);
            }
        }
        if let Some(plant) = def.variant_of.as_deref().and_then(|plant| self.find(&def_key("block", plant))) {
            return plant.to_owned();
        }
        title_from_id(name)
    }

    /// How the name at `key` reads, for a name written with kanji (`lang/readings/<code>.ron`):
    /// kanji in hiragana, the rest as written. None for a name that needs no reading, or
    /// that the language gives none.
    pub fn reading(&self, key: &str) -> Option<String> {
        self.find(key).filter(|name| has_kanji(name)).and_then(|_| self.readings.get(key).cloned())
    }

    /// How a block or item's name reads (see [`Lang::reading`]), made as its name is (see
    /// [`Lang::block`]): a template's reading is its member's with the family's filled in.
    pub fn block_reading(&self, def: &BlockDef) -> Option<String> {
        let name = def.form_of().unwrap_or(&def.name);
        let key = def_key("block", name);
        if self.find(&key).is_some() {
            return self.reading(&key);
        }
        let (namespace, local) = name.split_once(':').unwrap_or(("", name));
        if let Some(family) = &def.family {
            let family_local = family.rsplit(':').next().unwrap_or(family);
            let (member, family) = (format!("block.{namespace}.{}", local.replacen(family_local, "{name}", 1)), def_key("family", family));
            if let (Some(member_name), Some(family_name)) = (self.find(&member), self.find(&family)) {
                if !has_kanji(member_name) && !has_kanji(family_name) {
                    return None;
                }
                // Each part reads as written unless it holds kanji.
                let read = |key: &str, name: &str| if has_kanji(name) { self.readings.get(key).cloned() } else { Some(name.to_owned()) };
                return Some(fill(&read(&member, member_name)?, &[("name", &read(&family, family_name)?)]));
            }
        }
        def.variant_of.as_deref().map(|plant| def_key("block", plant)).filter(|plant| self.find(plant).is_some()).and_then(|plant| self.reading(&plant))
    }

    /// `words` in this language. Blocks are looked up in `blocks`.
    pub fn render(&self, words: &Words, blocks: &BlockRegistry) -> String {
        match words {
            Words::Text(text) => text.clone(),
            Words::Own(key, own) => self.get_or(key, own).to_owned(),
            Words::Block(name) => blocks.id(name).map_or_else(|| title_from_id(name), |id| self.block(blocks.def(id))),
            Words::Name(names) => names.iter().find(|(code, _)| *code == self.code).or(names.first()).map(|(_, name)| name.clone()).unwrap_or_default(),
            Words::Lines(lines) => lines.iter().map(|line| self.render(line, blocks)).collect::<Vec<_>>().join("\n"),
            Words::List(items) => {
                let separator = self.find("language.list_separator").unwrap_or(", ");
                items.iter().map(|item| self.render(item, blocks)).collect::<Vec<_>>().join(separator)
            }
            Words::Key(key, args) => {
                let rendered: Vec<(&str, String)> = args.iter().map(|(name, value)| (name.as_str(), self.render(value, blocks))).collect();
                let display: Vec<(&str, &dyn Display)> = rendered.iter().map(|(name, value)| (*name, value as &dyn Display)).collect();
                // A count picks its form: `key.one` or `key.other`.
                let count = rendered.iter().find(|(name, _)| *name == "count").and_then(|(_, value)| value.parse::<u64>().ok());
                match count {
                    Some(count) if self.find(key).is_none() => {
                        let rest: Vec<(&str, &dyn Display)> = display.into_iter().filter(|(name, _)| *name != "count").collect();
                        self.count(key, count, &rest)
                    }
                    _ => fill(self.get(key), &display),
                }
            }
        }
    }

    /// Every key this language or English defines.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.english.keys().chain(self.strings.keys().filter(|key| !self.english.contains_key(*key))).map(String::as_str)
    }

    /// The keys English has and this language lacks.
    pub fn untranslated(&self) -> impl Iterator<Item = &str> {
        let own = if self.code == ENGLISH { &self.english } else { &self.strings };
        self.english.keys().filter(|key| !own.contains_key(*key)).map(String::as_str)
    }
}

/// Where a language lets a line of wrapped text break, beyond what Unicode's line breaking
/// already does: its own rules, from its language file, so a mod or another language
/// replaces them. Japanese (kinsoku) keeps closing punctuation and small kana off the start
/// of a line, opening brackets off its end, and katakana words whole; between any two other
/// kanji or kana a line may break, and a Latin word stays whole as Unicode keeps it.
///
/// Text is laid out with a word joiner (U+2060) wherever a break is forbidden (see
/// [`LineRules::join`]); everything else (what is stored, compared, searched or copied)
/// sees the text as written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineRules {
    /// Characters no line may start with (`language.line_start_forbidden`).
    no_start: Vec<char>,
    /// Characters no line may end with (`language.line_end_forbidden`).
    no_end: Vec<char>,
    /// Characters whose runs stay on one line (`language.unbroken`): a run longer than
    /// `longest` (`language.unbroken_longest`) may still break, rather than spill out of
    /// its box.
    unbroken: Vec<char>,
    longest: usize,
}

/// The character that keeps the two either side of it on one line.
pub const WORD_JOINER: char = '\u{2060}';

impl LineRules {
    /// The rules language `lang` gives; none for a language that gives none (English).
    pub fn of(lang: &Lang) -> Self {
        let chars = |key: &str| -> Vec<char> { lang.find(key).map(|text| text.chars().filter(|c| !c.is_whitespace()).collect()).unwrap_or_default() };
        let longest = lang.find("language.unbroken_longest").and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
        LineRules { no_start: chars("language.line_start_forbidden"), no_end: chars("language.line_end_forbidden"), unbroken: chars("language.unbroken"), longest }
    }

    /// Whether the rules forbid nothing, so text lays out as written.
    pub fn is_empty(&self) -> bool {
        self.no_start.is_empty() && self.no_end.is_empty() && self.unbroken.is_empty()
    }

    /// For each character of `chars`, whether a line may not break just before it (false
    /// for the first). Whitespace and word joiners are left to Unicode.
    pub fn held(&self, chars: &[char]) -> Vec<bool> {
        let mut held = vec![false; chars.len()];
        if self.is_empty() {
            return held;
        }
        // The length of the unbroken run each character is in, 0 outside one.
        let mut run = vec![0; chars.len()];
        let mut start = 0;
        for i in 0..=chars.len() {
            if i == chars.len() || !self.unbroken.contains(&chars[i]) {
                run[start..i].fill(i - start);
                start = i + 1;
            }
        }
        let apart = |c: char| c.is_whitespace() || c == WORD_JOINER;
        for i in 1..chars.len() {
            let (before, c) = (chars[i - 1], chars[i]);
            let whole = run[i] > 1 && run[i] <= self.longest && run[i - 1] > 0;
            held[i] = !apart(before) && !apart(c) && (self.no_start.contains(&c) || self.no_end.contains(&before) || whole);
        }
        held
    }

    /// `text` as it is laid out: a word joiner before each character a line may not break
    /// before. None when it needs none.
    pub fn join(&self, text: &str) -> Option<String> {
        if self.is_empty() || text.is_ascii() {
            return None;
        }
        let chars: Vec<char> = text.chars().collect();
        let held = self.held(&chars);
        if !held.contains(&true) {
            return None;
        }
        let mut out = String::with_capacity(text.len() + 3 * held.iter().filter(|&&h| h).count());
        for (&c, &held) in chars.iter().zip(&held) {
            if held {
                out.push(WORD_JOINER);
            }
            out.push(c);
        }
        Some(out)
    }
}

/// Whether `text` holds kanji, which a reading spells out.
pub fn has_kanji(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々'))
}

/// The languages any package in `content_dir` has strings for, English first, then by
/// code.
pub fn languages(content_dir: &Path) -> Vec<Language> {
    let mut names: FxHashMap<String, Option<String>> = FxHashMap::default();
    for (_, code, source) in read_files(content_dir, "lang", |_| true) {
        let name = parse(&source).ok().and_then(|(mut strings, _)| strings.remove(NAME_KEY));
        let entry = names.entry(code).or_default();
        if entry.is_none() {
            *entry = name;
        }
    }
    let mut languages: Vec<Language> =
        names.into_iter().map(|(code, name)| Language { name: name.unwrap_or_else(|| code.clone()), code }).collect();
    languages.sort_by(|a, b| (a.code != ENGLISH, &a.code).cmp(&(b.code != ENGLISH, &b.code)));
    languages
}

/// Words for a player, which their client puts into their language: what the server
/// says (a command's answer, news, why it turned someone away) travels as keys and
/// arguments, never as English.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Words {
    /// As it was written: a player's chat, a name, a number.
    Text(String),
    /// A key's text, its `{name}`s filled with the arguments.
    Key(String, Vec<(String, Words)>),
    /// A block or item's name, by the block's id.
    Block(String),
    /// A definition's own text (a command's help), unless the language has `key`.
    Own(String, String),
    /// Several, a line each.
    Lines(Vec<Words>),
    /// Several, as a list: "a, b, c".
    List(Vec<Words>),
    /// A generated name (a settlement's), as it is written in each language by code,
    /// English first: the reader's language's, else English.
    Name(Vec<(String, String)>),
}

impl Words {
    /// A key's text, with arguments to add with [`Words::with`].
    pub fn key(key: &str) -> Self {
        Words::Key(key.to_owned(), Vec::new())
    }

    /// Adds argument `name` to a key's text.
    pub fn with(mut self, name: &str, value: impl Into<Words>) -> Self {
        if let Words::Key(_, args) = &mut self {
            args.push((name.to_owned(), value.into()));
        }
        self
    }

    /// Text as written: a number, a name, a coordinate.
    pub fn text(value: impl Display) -> Self {
        Words::Text(value.to_string())
    }
}

impl From<String> for Words {
    fn from(text: String) -> Self {
        Words::Text(text)
    }
}

impl From<&str> for Words {
    fn from(text: &str) -> Self {
        Words::Text(text.to_owned())
    }
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

/// The key a definition's name is looked up by: `kind` `block` and name `base:oak_log`
/// make `block.base.oak_log`.
pub fn def_key(kind: &str, name: &str) -> String {
    let mut key = String::with_capacity(kind.len() + 1 + name.len());
    key.push_str(kind);
    key.push('.');
    key.extend(name.chars().map(|c| if c == ':' { '.' } else { c }));
    key
}

/// `text` with each `{name}` replaced by its argument. A name with no argument is left as
/// written, so the mistake shows.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let brace = rest.as_bytes()[i];
        rest = &rest[i + 1..];
        if rest.as_bytes().first() == Some(&brace) {
            out.push(brace as char);
            rest = &rest[1..];
            continue;
        }
        if brace == b'}' {
            out.push('}');
            continue;
        }
        let Some(end) = rest.find('}') else {
            out.push('{');
            continue;
        };
        let name = &rest[..end];
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => {
                let _ = write!(out, "{value}");
            }
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `{name}`s a text uses, for checking that translations keep their arguments.
pub fn arguments(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        rest = &rest[i + 1..];
        if let Some(stripped) = rest.strip_prefix('{') {
            rest = stripped;
            continue;
        }
        if let Some(end) = rest.find('}') {
            names.push(&rest[..end]);
            rest = &rest[end + 1..];
        }
    }
    names.sort_unstable();
    names.dedup();
    names
}

fn parse(source: &str) -> Result<(FxHashMap<String, String>, Vec<String>), String> {
    parse_map(source)
}

/// A language file (its strings, readings or pins) as key to value, with the keys given
/// again after their first: the first keeps the key, and the repeats are for the caller to
/// report, so one slip never drops the whole file nor lets the later text quietly win.
pub fn parse_map<V: serde::de::DeserializeOwned>(source: &str) -> Result<(FxHashMap<String, V>, Vec<String>), String> {
    struct Map<V>(FxHashMap<String, V>, Vec<String>);
    impl<'de, V: Deserialize<'de>> Deserialize<'de> for Map<V> {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visit<V>(std::marker::PhantomData<V>);
            impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Visit<V> {
                type Value = Map<V>;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a map of keys")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Map<V>, A::Error> {
                    let (mut entries, mut again) = (FxHashMap::default(), Vec::new());
                    while let Some((key, value)) = map.next_entry::<String, V>()? {
                        if entries.contains_key(&key) {
                            again.push(key);
                        } else {
                            entries.insert(key, value);
                        }
                    }
                    Ok(Map(entries, again))
                }
            }
            deserializer.deserialize_map(Visit(std::marker::PhantomData))
        }
    }
    ron::from_str::<Map<V>>(source).map(|m| (m.0, m.1)).map_err(|e| e.to_string())
}

/// `(package, language, source)` of every language file whose language `wanted` accepts,
/// in package order.
fn read_files(content_dir: &Path, sub: &str, wanted: impl Fn(&str) -> bool) -> Vec<(String, String, String)> {
    let Ok(entries) = std::fs::read_dir(content_dir) else { return Vec::new() };
    let mut packages: Vec<String> = entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    packages.sort();
    let mut files = Vec::new();
    for package in packages {
        let dir = content_dir.join(&package).join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut names: Vec<String> = entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            let Some(code) = name.strip_suffix(".ron") else { continue };
            if !wanted(code) {
                continue;
            }
            match std::fs::read_to_string(dir.join(&name)) {
                Ok(source) => files.push((package.clone(), code.to_owned(), source)),
                Err(e) => tracing::warn!("{package}/{sub}/{name}: {e}"),
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kinsoku: no line starts with 。 or a small kana or ends with 「, katakana words hold
    /// together up to the longest, and anything else (spaces, Latin) is left to Unicode.
    #[test]
    fn line_rules_join_what_may_not_break() {
        let rules = LineRules { no_start: "。」ょ".chars().collect(), no_end: "「".chars().collect(), unbroken: "クリッ".chars().collect(), longest: 4 };
        let j = WORD_JOINER;
        assert_eq!(rules.join("見た。「きょう」"), Some(format!("見た{j}。「{j}き{j}ょう{j}」")));
        assert_eq!(rules.join("クリック"), Some(format!("ク{j}リ{j}ッ{j}ク")));
        // Longer than the longest, a run may break (rather than spill out of its box).
        assert_eq!(rules.join("クリッククリック"), None);
        assert_eq!(rules.join("ク リ"), None);
        assert_eq!(rules.join("Shift click"), None);
        assert_eq!(LineRules::default().join("見た。"), None);
    }

    const EN: &str = r#"{ "language.name": "English", "menu.play": "Play", "hud.plants": "Plants {crop} at level {level}", "only.en": "Only" }"#;
    const JA: &str = r#"{ "language.name": "日本語", "menu.play": "プレイ", "hud.plants": "{crop}を植える(レベル{level})" }"#;

    fn japanese() -> Lang {
        let (lang, problems) = Lang::from_files("ja", [("core", "en", EN), ("core", "ja", JA)]);
        assert!(problems.is_empty(), "{problems:?}");
        lang
    }

    #[test]
    fn looks_up_then_falls_back_to_english_then_the_default_then_the_key() {
        let lang = japanese();
        assert_eq!(lang.get("menu.play"), "プレイ");
        assert_eq!(lang.get("only.en"), "Only");
        assert_eq!(lang.get_or("block.base.oak_log", "Oak Log"), "Oak Log");
        assert_eq!(lang.get("missing.key"), "missing.key");
        assert_eq!(lang.untranslated().collect::<Vec<_>>(), ["only.en"]);
    }

    #[test]
    fn english_is_its_own_fallback() {
        let (lang, _) = Lang::from_files("en", [("core", "en", EN), ("core", "ja", JA)]);
        assert_eq!(lang.get("menu.play"), "Play");
        assert_eq!(lang.untranslated().count(), 0);
    }

    #[test]
    fn arguments_go_where_each_language_puts_them() {
        let lang = japanese();
        assert_eq!(lang.format("hud.plants", &[("crop", &"小麦"), ("level", &3)]), "小麦を植える(レベル3)");
        assert_eq!(fill("{a} and {b}", &[("b", &2)]), "{a} and 2");
        assert_eq!(fill("{{literal}} {x}}", &[("x", &1)]), "{literal} 1}");
        assert_eq!(fill("unclosed {x", &[("x", &1)]), "unclosed {x");
        assert_eq!(arguments("{b} {a} {{c}} {a}"), ["a", "b"]);
    }

    #[test]
    fn counts_take_the_form_their_language_needs() {
        let en = r#"{ "ago.one": "{count} minute ago", "ago.other": "{count} minutes ago" }"#;
        let (english, _) = Lang::from_files("en", [("core", "en", en)]);
        assert_eq!(english.count("ago", 1, &[]), "1 minute ago");
        assert_eq!(english.count("ago", 5, &[]), "5 minutes ago");
        let (japanese, _) = Lang::from_files("ja", [("core", "en", en), ("core", "ja", r#"{ "ago.other": "{count}分前" }"#)]);
        assert_eq!(japanese.count("ago", 1, &[]), "1分前");
        assert_eq!(japanese.count("missing", 2, &[]), "missing.other");
    }

    #[test]
    fn the_first_package_to_define_a_key_keeps_it() {
        let (lang, problems) = Lang::from_files("en", [("base", "en", r#"{ "k": "base" }"#), ("mod", "en", r#"{ "k": "mod" }"#)]);
        assert_eq!(lang.get("k"), "base");
        assert_eq!(problems.len(), 1);
        let (_, problems) = Lang::from_files("en", [("base", "en", "not ron")]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn a_key_given_twice_in_one_file_keeps_the_first_and_is_reported() {
        let (strings, again) = parse(r#"{ "k": "first", "j": "other", "k": "second" }"#).unwrap();
        assert_eq!((strings["k"].as_str(), strings.len()), ("first", 2));
        assert_eq!(again, ["k"]);
        let (lang, problems) = Lang::from_files("en", [("base", "en", r#"{ "k": "first", "j": "other", "k": "second" }"#)]);
        assert_eq!((lang.get("k"), lang.get("j")), ("first", "other"));
        assert!(matches!(problems.as_slice(), [p] if p.contains("`k` is given twice")), "{problems:?}");
    }

    /// A family's block placed in another form (stairs turned east, upside down) is named
    /// and read as the block itself, through the family's template.
    #[test]
    fn a_family_block_in_another_form_goes_by_the_block() {
        let ja = r#"{ "block.base.{name}_stairs": "{name}の階段", "family.base.oak": "オーク" }"#;
        let (mut lang, _) = Lang::from_files("ja", [("base", "ja", ja)]);
        lang.readings.insert("block.base.{name}_stairs".into(), "{name}のかいだん".into());
        let stairs = |name: &str, faces, flipped| BlockDef { name: name.into(), family: Some("base:oak".into()), faces, flipped, ..Default::default() };
        for def in [
            stairs("base:oak_stairs", Some(crate::block::Facing::North), false),
            stairs("base:oak_stairs_east", Some(crate::block::Facing::East), false),
            stairs("base:oak_stairs_top_west", Some(crate::block::Facing::West), true),
        ] {
            assert_eq!(lang.block(&def), "オークの階段", "{}", def.name);
            assert_eq!(lang.block_reading(&def).as_deref(), Some("オークのかいだん"), "{}", def.name);
        }
    }

    #[test]
    fn definition_keys_come_from_names() {
        assert_eq!(def_key("block", "base:oak_log"), "block.base.oak_log");
        assert_eq!(def_key("skill", "core:farming"), "skill.core.farming");
    }
}
