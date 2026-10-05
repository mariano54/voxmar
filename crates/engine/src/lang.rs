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
        let files = read_files(content_dir, |file_code| file_code == code || file_code == ENGLISH);
        let (lang, problems) = Self::from_files(code, files.iter().map(|(package, code, source)| (package.as_str(), code.as_str(), source.as_str())));
        for problem in problems {
            tracing::warn!("{problem}");
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
                Ok(strings) => {
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

/// The languages any package in `content_dir` has strings for, English first, then by
/// code.
pub fn languages(content_dir: &Path) -> Vec<Language> {
    let mut names: FxHashMap<String, Option<String>> = FxHashMap::default();
    for (_, code, source) in read_files(content_dir, |_| true) {
        let name = parse(&source).ok().and_then(|mut strings| strings.remove(NAME_KEY));
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

fn parse(source: &str) -> Result<FxHashMap<String, String>, String> {
    ron::from_str(source).map_err(|e| e.to_string())
}

/// `(package, language, source)` of every language file whose language `wanted` accepts,
/// in package order.
fn read_files(content_dir: &Path, wanted: impl Fn(&str) -> bool) -> Vec<(String, String, String)> {
    let Ok(entries) = std::fs::read_dir(content_dir) else { return Vec::new() };
    let mut packages: Vec<String> = entries.flatten().filter_map(|entry| entry.file_name().into_string().ok()).collect();
    packages.sort();
    let mut files = Vec::new();
    for package in packages {
        let dir = content_dir.join(&package).join("lang");
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
                Err(e) => tracing::warn!("{package}/lang/{name}: {e}"),
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn definition_keys_come_from_names() {
        assert_eq!(def_key("block", "base:oak_log"), "block.base.oak_log");
        assert_eq!(def_key("skill", "core:farming"), "skill.core.farming");
    }
}
