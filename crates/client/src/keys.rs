//! Keys the player can bind in the settings: a name to show in the menu for each, and the
//! key's standard code name (`KeyC`, `AltLeft`) to keep in the settings file, which stays
//! the same on every platform and keyboard layout.

use bevy::prelude::KeyCode;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A key a control is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key(pub KeyCode);

impl Key {
    /// Whether `key` can be bound. Esc cannot: it always opens the menu and cancels.
    pub fn bindable(key: KeyCode) -> bool {
        KEYS.iter().any(|&(k, _)| k == key)
    }

    /// What the menu calls it in the player's language. A key with a word for a name
    /// (Space, Left Shift) is named by the language, `key.<code>`, and on this system by
    /// `key.<code>.<system>` where that differs (Option on macOS for Alt, Win on Windows
    /// for Super); the others are their own symbol (C, F3, `[`).
    pub fn name(self, lang: &voxmar_engine::lang::Lang) -> String {
        name_of(lang, &self.code(), self.symbol())
    }

    /// The symbol on the key, or "" for a key named in words.
    fn symbol(self) -> &'static str {
        KEYS.iter().find(|&&(k, _)| k == self.0).map_or("?", |&(_, symbol)| symbol)
    }

    /// The standard code name, as kept in the settings file.
    fn code(self) -> String {
        format!("{:?}", self.0)
    }

    fn from_code(code: &str) -> Option<Self> {
        KEYS.iter().map(|&(k, _)| Key(k)).find(|k| k.code() == code)
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.code())
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Key::from_code(&code).ok_or_else(|| D::Error::custom(format!("no bindable key called {code}")))
    }
}

/// The name of the key with standard code name `code`: `key.<code>.<system>`, then
/// `key.<code>`, then its symbol (and its code, for a word key no language names).
fn name_of(lang: &voxmar_engine::lang::Lang, code: &str, symbol: &str) -> String {
    let key = format!("key.{code}");
    if let Some(name) = lang.find(&format!("{key}.{SYSTEM}")).or_else(|| lang.find(&key)) {
        return name.to_owned();
    }
    if symbol.is_empty() { code.to_owned() } else { symbol.to_owned() }
}

/// The system, as the per-system names of keys know it (`key.AltLeft.macos`).
const SYSTEM: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(target_os = "windows") {
    "windows"
} else {
    "linux"
};

/// What Esc is called: it is not bindable, but the interface shows it.
pub fn escape_name(lang: &voxmar_engine::lang::Lang) -> String {
    name_of(lang, "Escape", "")
}

/// Every bindable key and the symbol on it; "" for a key named in words, by the
/// language (`key.<code>` in `lang/<code>.ron`).
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::Space, ""),
    (KeyCode::Tab, ""),
    (KeyCode::CapsLock, ""),
    (KeyCode::Enter, ""),
    (KeyCode::Backspace, ""),
    (KeyCode::ShiftLeft, ""),
    (KeyCode::ShiftRight, ""),
    (KeyCode::ControlLeft, ""),
    (KeyCode::ControlRight, ""),
    (KeyCode::AltLeft, ""),
    (KeyCode::AltRight, ""),
    (KeyCode::SuperLeft, ""),
    (KeyCode::SuperRight, ""),
    (KeyCode::Backquote, "`"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Slash, "/"),
    (KeyCode::ArrowUp, ""),
    (KeyCode::ArrowDown, ""),
    (KeyCode::ArrowLeft, ""),
    (KeyCode::ArrowRight, ""),
    (KeyCode::Insert, ""),
    (KeyCode::Delete, ""),
    (KeyCode::Home, ""),
    (KeyCode::End, ""),
    (KeyCode::PageUp, ""),
    (KeyCode::PageDown, ""),
    (KeyCode::Numpad0, ""),
    (KeyCode::Numpad1, ""),
    (KeyCode::Numpad2, ""),
    (KeyCode::Numpad3, ""),
    (KeyCode::Numpad4, ""),
    (KeyCode::Numpad5, ""),
    (KeyCode::Numpad6, ""),
    (KeyCode::Numpad7, ""),
    (KeyCode::Numpad8, ""),
    (KeyCode::Numpad9, ""),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_by_code_name() {
        for &(code, _) in KEYS {
            let text = ron::to_string(&Key(code)).unwrap();
            assert_eq!(ron::from_str::<Key>(&text).unwrap(), Key(code), "{text}");
        }
        assert_eq!(ron::to_string(&Key(KeyCode::KeyC)).unwrap(), "\"KeyC\"");
        assert!(ron::from_str::<Key>("\"Escape\"").is_err(), "Esc is not bindable");
        assert_eq!(Key(KeyCode::KeyC).symbol(), "C");
    }

    /// Every key named in words has a name in English and Japanese, with the modifiers
    /// named as each system names them (Option and Cmd on macOS, Win on Windows).
    #[test]
    fn word_keys_are_named_in_every_language() {
        use voxmar_engine::lang::Lang;
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let mut unnamed = Vec::new();
        for code in ["en", "ja"] {
            let lang = Lang::load(&content, code);
            // Its own name, not English's in its place.
            let named = |key: &str| lang.find(key).is_some() && lang.untranslated().all(|k| k != key);
            let words = KEYS.iter().filter(|(_, symbol)| symbol.is_empty()).map(|&(key, _)| Key(key).code()).chain(["Escape".to_owned()]);
            for code_name in words {
                if !named(&format!("key.{code_name}")) {
                    unnamed.push(format!("{code}: key.{code_name}"));
                }
            }
            let by_system = [("AltLeft", "macos"), ("AltRight", "macos"), ("SuperLeft", "macos"), ("SuperRight", "macos"), ("SuperLeft", "windows"), ("SuperRight", "windows")];
            for (modifier, system) in by_system {
                if !named(&format!("key.{modifier}.{system}")) {
                    unnamed.push(format!("{code}: key.{modifier}.{system}"));
                }
            }
        }
        assert!(unnamed.is_empty(), "keys without a name: {}", unnamed.join(", "));
        let en = Lang::load(&content, "en");
        assert_eq!(name_of(&en, "AltLeft", ""), if cfg!(target_os = "macos") { "Left Option" } else { "Left Alt" });
        assert_eq!(Key(KeyCode::KeyC).name(&en), "C");
        assert_eq!(Key(KeyCode::ShiftLeft).name(&en), "Left Shift");
        let ja = Lang::load(&content, "ja");
        assert_eq!(name_of(&ja, "SuperLeft", ""), if cfg!(target_os = "macos") { "左Cmd" } else if cfg!(target_os = "windows") { "左Win" } else { "左Super" });
    }
}
