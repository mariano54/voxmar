//! Why an action was refused, in words a player can act on (`refusals.ron`). Every
//! requirement an action can ask for ([`Requirement`]) either names the lang key that says
//! why it failed or is `silent`: a requirement that only decides which action a click
//! meant (holding a block, aiming at a bed) has nothing to teach. Every key the server
//! refuses with itself (`Server::refuse`) is listed under `sent`. A requirement that says
//! why may also name a `hint`: the key the client says instead when the data shows what
//! would work (the soil a seed takes, the station that cooks the held food), with that
//! thing filled in. Loading refuses a file
//! that leaves a requirement out, names one twice or one that does not exist, or a key
//! some language lacks, so no refusal ever reaches a player as silence or a raw key.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::action::Requirement;
use crate::block::RegistryError;
use crate::content::ContentFile;

pub const FILE: &str = "refusals.ron";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RefusalFile {
    #[serde(default)]
    unmet: BTreeMap<String, Say>,
    #[serde(default)]
    sent: Vec<String>,
}

/// What a requirement that failed says: a lang key, or nothing; and what it says instead
/// when the client can name what would work.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Say {
    #[serde(default)]
    say: Option<String>,
    #[serde(default)]
    hint: Option<String>,
    #[serde(default)]
    silent: bool,
}

/// Why each requirement failed, and the keys the server refuses with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refusals {
    /// By [`Requirement::index`]: the key that says why, or none for a silent one.
    unmet: Vec<Option<String>>,
    /// By [`Requirement::index`]: the key naming what would work, if there is one.
    hints: Vec<Option<String>>,
    sent: BTreeSet<String>,
}

impl Refusals {
    /// Reads every package's file in order: a later package changes what a requirement
    /// says and adds keys the server sends. Refuses a requirement left out or unknown, one
    /// both said and silent (or neither), and a key no file gives.
    pub fn load(files: &[ContentFile]) -> Result<Self, RegistryError> {
        let mut said: Vec<Option<Option<String>>> = vec![None; Requirement::ALL.len()];
        let mut hints: Vec<Option<String>> = vec![None; Requirement::ALL.len()];
        let mut sent = BTreeSet::new();
        let mut any = false;
        for file in files.iter().filter(|f| f.name == FILE) {
            any = true;
            let at = |e: String| RegistryError(format!("{}/{FILE}: {e}", file.package));
            let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
            let given: RefusalFile = options.from_str(&file.source).map_err(|e| at(e.to_string()))?;
            for (name, say) in given.unmet {
                let need = Requirement::named(&name).ok_or_else(|| at(format!("`{name}` is no requirement")))?;
                let key = match (say.say, say.silent) {
                    (Some(key), false) if !key.is_empty() => Some(key),
                    (None, true) => None,
                    _ => return Err(at(format!("`{name}` must either say a key or be silent"))),
                };
                hints[need.index()] = match say.hint {
                    Some(hint) if hint.is_empty() || key.is_none() => return Err(at(format!("`{name}`: a hint goes with a key it says"))),
                    hint => hint,
                };
                said[need.index()] = Some(key);
            }
            for key in given.sent {
                if key.is_empty() || !sent.insert(key.clone()) {
                    return Err(at(format!("`sent` lists `{key}` twice or empty")));
                }
            }
        }
        if !any {
            return Err(RegistryError(format!("no package gives {FILE}")));
        }
        let missing: Vec<String> = Requirement::ALL.iter().filter(|need| said[need.index()].is_none()).map(|need| need.name()).collect();
        if !missing.is_empty() {
            return Err(RegistryError(format!("{FILE}: say why, or `silent: true`, for {}", missing.join(", "))));
        }
        Ok(Self { unmet: said.into_iter().map(Option::unwrap_or_default).collect(), hints, sent })
    }

    /// The key that says why `need` failed, if it says anything.
    pub fn says(&self, need: Requirement) -> Option<&str> {
        self.unmet.get(need.index()).and_then(|key| key.as_deref())
    }

    /// The key that says what would have worked when `need` failed, if it has one.
    pub fn hint(&self, need: Requirement) -> Option<&str> {
        self.hints.get(need.index()).and_then(|key| key.as_deref())
    }

    /// Whether the server may refuse with `key`.
    pub fn sent(&self, key: &str) -> bool {
        self.sent.contains(key)
    }

    /// Every key a refusal may say, its hints too.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        let said: BTreeSet<&str> = self.unmet.iter().chain(&self.hints).flatten().map(String::as_str).collect();
        let sent: Vec<&str> = self.sent.iter().map(String::as_str).filter(|k| !said.contains(k)).collect();
        said.into_iter().chain(sent)
    }

    /// Refuses a key that `defines` (a language's code, a key) is false for in any of
    /// `languages`: each refusal is said in every language the game has.
    pub fn check_words(&self, languages: &[&str], defines: impl Fn(&str, &str) -> bool) -> Result<(), RegistryError> {
        for key in self.keys() {
            if let Some(code) = languages.iter().find(|code| !defines(code, key)) {
                return Err(RegistryError(format!("{FILE}: `{key}` has no words in lang/{code}.ron")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(package: &str, source: &str) -> ContentFile {
        ContentFile { package: package.into(), name: FILE.into(), source: source.into() }
    }

    fn every(but: Requirement) -> String {
        Requirement::ALL.iter().filter(|&&n| n != but).map(|n| format!("\"{}\": (silent: true)", n.name())).collect::<Vec<_>>().join(", ")
    }

    #[test]
    fn every_requirement_stands_once_in_order() {
        for (i, need) in Requirement::ALL.iter().enumerate() {
            assert_eq!(need.index(), i, "{need:?}");
            assert_eq!(Requirement::named(&need.name()), Some(*need));
        }
    }

    #[test]
    fn every_requirement_says_why_or_is_silent() {
        let full = format!("(unmet: {{ {}, \"ToolGood\": (say: \"tools.needs\") }}, sent: [\"a.b\"])", every(Requirement::ToolGood));
        let refusals = Refusals::load(&[file("core", &full)]).unwrap();
        assert_eq!(refusals.says(Requirement::ToolGood), Some("tools.needs"));
        assert_eq!(refusals.says(Requirement::Alive), None);
        assert!(refusals.sent("a.b") && !refusals.sent("c.d"));
        assert_eq!(refusals.keys().collect::<Vec<_>>(), ["tools.needs", "a.b"]);
        // A later package changes what one says.
        let later = Refusals::load(&[file("core", &full), file("base", "(unmet: { \"Alive\": (say: \"refusal.dead\") })")]).unwrap();
        assert_eq!(later.says(Requirement::Alive), Some("refusal.dead"));
        // Left out, unknown, both or neither, a key twice, no file at all.
        assert!(Refusals::load(&[file("core", "(unmet: { \"Alive\": (silent: true) })")]).unwrap_err().0.contains("ToolGood"));
        assert!(Refusals::load(&[file("core", &full), file("base", "(unmet: { \"Flying\": (silent: true) })")]).unwrap_err().0.contains("no requirement"));
        assert!(Refusals::load(&[file("core", &full), file("base", "(unmet: { \"Alive\": (say: \"x\", silent: true) })")]).unwrap_err().0.contains("either"));
        assert!(Refusals::load(&[file("core", &full), file("base", "(unmet: { \"Alive\": () })")]).unwrap_err().0.contains("either"));
        assert!(Refusals::load(&[file("core", &full), file("base", "(sent: [\"a.b\"])")]).unwrap_err().0.contains("twice"));
        assert!(Refusals::load(&[]).unwrap_err().0.contains("no package"));
        // A hint, only beside a key it says.
        let hinted = Refusals::load(&[file("core", &full), file("base", "(unmet: { \"ToolGood\": (say: \"tools.needs\", hint: \"tools.try\") })")]).unwrap();
        assert_eq!(hinted.hint(Requirement::ToolGood), Some("tools.try"));
        assert_eq!(refusals.hint(Requirement::ToolGood), None);
        assert!(Refusals::load(&[file("core", &full), file("base", "(unmet: { \"Alive\": (silent: true, hint: \"x\") })")]).unwrap_err().0.contains("hint"));
        // Words in every language, a hint's too.
        assert!(refusals.check_words(&["en", "ja"], |_, _| true).is_ok());
        assert!(refusals.check_words(&["en", "ja"], |code, key| code == "en" || key != "a.b").unwrap_err().0.contains("lang/ja.ron"));
        assert_eq!(hinted.keys().collect::<Vec<_>>(), ["tools.needs", "tools.try", "a.b"]);
        assert!(hinted.check_words(&["en", "ja"], |code, key| code == "en" || key != "tools.try").unwrap_err().0.contains("`tools.try` has no words in lang/ja.ron"));
    }
}
