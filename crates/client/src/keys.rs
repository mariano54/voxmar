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

    /// What the menu calls it.
    pub fn label(self) -> &'static str {
        KEYS.iter().find(|&&(k, _)| k == self.0).map_or("?", |&(_, label)| label)
    }

    /// What the menu calls it in the player's language: a language may rename a key
    /// (`key.Space`), and the others keep the names below.
    /// A key named differently on each system (Option on a Mac, Win on Windows) has
    /// `key.AltLeft.macos` and the like.
    pub fn name(self, lang: &voxmar_engine::lang::Lang) -> String {
        let key = format!("key.{}", self.code());
        lang.find(&format!("{key}.{}", std::env::consts::OS)).or_else(|| lang.find(&key)).unwrap_or(self.label()).to_owned()
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


/// Every bindable key and its name in the menu.
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
    (KeyCode::Space, "Space"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::CapsLock, "Caps Lock"),
    (KeyCode::Enter, "Enter"),
    (KeyCode::Backspace, "Backspace"),
    (KeyCode::ShiftLeft, "Left Shift"),
    (KeyCode::ShiftRight, "Right Shift"),
    (KeyCode::ControlLeft, "Left Ctrl"),
    (KeyCode::ControlRight, "Right Ctrl"),
    (KeyCode::AltLeft, "Left Alt"),
    (KeyCode::AltRight, "Right Alt"),
    (KeyCode::SuperLeft, "Left Super"),
    (KeyCode::SuperRight, "Right Super"),
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
    (KeyCode::ArrowUp, "Up"),
    (KeyCode::ArrowDown, "Down"),
    (KeyCode::ArrowLeft, "Left"),
    (KeyCode::ArrowRight, "Right"),
    (KeyCode::Insert, "Insert"),
    (KeyCode::Delete, "Delete"),
    (KeyCode::Home, "Home"),
    (KeyCode::End, "End"),
    (KeyCode::PageUp, "Page Up"),
    (KeyCode::PageDown, "Page Down"),
    (KeyCode::Numpad0, "Numpad 0"),
    (KeyCode::Numpad1, "Numpad 1"),
    (KeyCode::Numpad2, "Numpad 2"),
    (KeyCode::Numpad3, "Numpad 3"),
    (KeyCode::Numpad4, "Numpad 4"),
    (KeyCode::Numpad5, "Numpad 5"),
    (KeyCode::Numpad6, "Numpad 6"),
    (KeyCode::Numpad7, "Numpad 7"),
    (KeyCode::Numpad8, "Numpad 8"),
    (KeyCode::Numpad9, "Numpad 9"),
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
        assert_eq!(Key(KeyCode::KeyC).label(), "C");
    }
}
