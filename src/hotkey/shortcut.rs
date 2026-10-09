//! Keyboard shortcuts written as text, e.g. `"control+option+Y"`, and their
//! Carbon key codes and modifier masks.

use std::fmt;

/// Carbon modifier masks (`Events.h`).
const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

impl Modifiers {
    pub fn carbon_mask(self) -> u32 {
        [
            (self.control, CONTROL_KEY),
            (self.option, OPTION_KEY),
            (self.shift, SHIFT_KEY),
            (self.command, CMD_KEY),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |mask, (_, bit)| mask | bit)
    }

    fn any(self) -> bool {
        self.control || self.option || self.shift || self.command
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    /// Canonical key name: an uppercase letter, a digit, a symbol, `Space`
    /// or `F1` to `F12`.
    pub key: String,
    /// Carbon virtual key code (ANSI layout).
    pub key_code: u32,
}

impl Shortcut {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut modifiers = Modifiers::default();
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "control" | "ctrl" | "⌃" => modifiers.control = true,
                "option" | "opt" | "alt" | "⌥" => modifiers.option = true,
                "shift" | "⇧" => modifiers.shift = true,
                "command" | "cmd" | "⌘" => modifiers.command = true,
                "" => return Err(format!("empty part in shortcut {text:?}")),
                _ if key.is_some() => return Err(format!("more than one key in {text:?}")),
                _ => key = Some(part),
            }
        }
        let key = key.ok_or_else(|| format!("no key in shortcut {text:?}"))?;
        let (name, key_code) =
            key_code(key).ok_or_else(|| format!("unknown key {key:?} in {text:?}"))?;
        let is_function_key = name.starts_with('F') && name.len() > 1;
        if !modifiers.any() && !is_function_key {
            return Err(format!(
                "shortcut {text:?} needs a modifier (control, option, shift or command)"
            ));
        }
        Ok(Self {
            modifiers,
            key: name,
            key_code,
        })
    }

    /// The key as a menu key equivalent, if it is a character key.
    pub fn menu_key(&self) -> Option<String> {
        match self.key.as_str() {
            "Space" => Some(" ".to_owned()),
            key if key.chars().count() == 1 => Some(key.to_lowercase()),
            _ => None,
        }
    }
}

impl fmt::Display for Shortcut {
    /// Canonical form, e.g. `control+option+Y`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.control, "control"),
            (m.option, "option"),
            (m.shift, "shift"),
            (m.command, "command"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

#[rustfmt::skip]
const LETTERS: [(char, u32); 26] = [
    ('A', 0), ('B', 11), ('C', 8), ('D', 2), ('E', 14), ('F', 3), ('G', 5),
    ('H', 4), ('I', 34), ('J', 38), ('K', 40), ('L', 37), ('M', 46), ('N', 45),
    ('O', 31), ('P', 35), ('Q', 12), ('R', 15), ('S', 1), ('T', 17), ('U', 32),
    ('V', 9), ('W', 13), ('X', 7), ('Y', 16), ('Z', 6),
];
#[rustfmt::skip]
const OTHERS: [(&str, u32); 22] = [
    ("0", 29), ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("5", 23), ("6", 22),
    ("7", 26), ("8", 28), ("9", 25), ("-", 27), ("=", 24), ("[", 33), ("]", 30),
    ("\\", 42), (";", 41), ("'", 39), (",", 43), (".", 47), ("/", 44), ("`", 50),
    ("Space", 49),
];
/// F1 to F12.
const FUNCTION: [u32; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];

/// Canonical name and ANSI virtual key code (`kVK_*` in `Events.h`).
fn key_code(key: &str) -> Option<(String, u32)> {
    let upper = key.to_ascii_uppercase();
    let mut chars = upper.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && let Some((name, code)) = LETTERS.iter().find(|(letter, _)| *letter == c)
    {
        return Some((name.to_string(), *code));
    }
    if let Some((name, code)) = OTHERS
        .iter()
        .find(|(name, _)| *name == key || name.eq_ignore_ascii_case(key))
    {
        return Some((name.to_string(), *code));
    }
    let number: usize = upper.strip_prefix('F')?.parse().ok()?;
    let code = FUNCTION.get(number.checked_sub(1)?)?;
    Some((format!("F{number}"), *code))
}

/// The canonical name for a virtual key code, if it is a supported key.
fn key_name(code: u32) -> Option<String> {
    LETTERS
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(letter, _)| letter.to_string())
        .or_else(|| {
            OTHERS
                .iter()
                .find(|(_, c)| *c == code)
                .map(|(name, _)| name.to_string())
        })
        .or_else(|| {
            FUNCTION
                .iter()
                .position(|c| *c == code)
                .map(|i| format!("F{}", i + 1))
        })
}

impl Shortcut {
    /// A shortcut from a recorded key press. Fails for unsupported keys and
    /// for keys without a modifier (except function keys).
    pub fn from_key(key_code: u32, modifiers: Modifiers) -> Result<Self, String> {
        let key = key_name(key_code).ok_or_else(|| "That key can't be used".to_owned())?;
        // Round-trip through the text form so the same rules apply as for
        // shortcuts typed into settings.json.
        Self::parse(
            &Self {
                modifiers,
                key,
                key_code,
            }
            .to_string(),
        )
    }

    /// Symbols as shown in menus, e.g. `⌃⌥Y`.
    pub fn symbols(&self) -> String {
        let m = self.modifiers;
        let mut text = String::new();
        for (on, symbol) in [
            (m.control, '⌃'),
            (m.option, '⌥'),
            (m.shift, '⇧'),
            (m.command, '⌘'),
        ] {
            if on {
                text.push(symbol);
            }
        }
        text.push_str(&self.key);
        text
    }

    /// A built-in macOS (or near-universal app) shortcut this one would
    /// clash with, if any.
    pub fn known_conflict(&self) -> Option<&'static str> {
        const COMMAND: Modifiers = Modifiers {
            control: false,
            option: false,
            shift: false,
            command: true,
        };
        const COMMAND_SHIFT: Modifiers = Modifiers {
            control: false,
            option: false,
            shift: true,
            command: true,
        };
        const CONTROL: Modifiers = Modifiers {
            control: true,
            option: false,
            shift: false,
            command: false,
        };
        const COMMAND_OPTION: Modifiers = Modifiers {
            control: false,
            option: true,
            shift: false,
            command: true,
        };
        const COMMAND_CONTROL: Modifiers = Modifiers {
            control: true,
            option: false,
            shift: false,
            command: true,
        };
        let table: &[(Modifiers, &str, &str)] = &[
            (COMMAND, "Space", "Spotlight"),
            (COMMAND_OPTION, "Space", "Finder search"),
            (CONTROL, "Space", "switching input sources"),
            (COMMAND, "Q", "Quit in every app"),
            (COMMAND, "W", "Close Window in every app"),
            (COMMAND, "C", "Copy"),
            (COMMAND, "V", "Paste"),
            (COMMAND, "X", "Cut"),
            (COMMAND, "Z", "Undo"),
            (COMMAND, "A", "Select All"),
            (COMMAND, "S", "Save"),
            (COMMAND, "N", "New"),
            (COMMAND, "O", "Open"),
            (COMMAND, "P", "Print"),
            (COMMAND, "F", "Find"),
            (COMMAND, "T", "New Tab"),
            (COMMAND, "H", "Hide"),
            (COMMAND, "M", "Minimize"),
            (COMMAND, ",", "Settings in every app"),
            (COMMAND, "`", "switching windows"),
            (COMMAND_SHIFT, "3", "screenshots"),
            (COMMAND_SHIFT, "4", "screenshots"),
            (COMMAND_SHIFT, "5", "screenshots"),
            (COMMAND_SHIFT, "Z", "Redo"),
            (COMMAND_OPTION, "Escape", "Force Quit"),
            (COMMAND_CONTROL, "Q", "Lock Screen"),
            (COMMAND_CONTROL, "F", "full screen"),
            (COMMAND_CONTROL, "Space", "Emoji & Symbols"),
        ];
        table
            .iter()
            .find(|(modifiers, key, _)| *modifiers == self.modifiers && *key == self.key)
            .map(|(_, _, what)| *what)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_shortcut() {
        let shortcut = Shortcut::parse("control+option+Y").unwrap();
        assert_eq!(shortcut.key_code, 16);
        assert_eq!(shortcut.modifiers.carbon_mask(), CONTROL_KEY | OPTION_KEY);
        assert_eq!(shortcut.to_string(), "control+option+Y");
        assert_eq!(shortcut.menu_key().as_deref(), Some("y"));
    }

    #[test]
    fn accepts_aliases_symbols_and_any_order() {
        let a = Shortcut::parse("cmd + Shift + k").unwrap();
        let b = Shortcut::parse("⇧+⌘+K").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "shift+command+K");
        assert_eq!(Shortcut::parse("ctrl+alt+space").unwrap().key, "Space");
    }

    #[test]
    fn function_keys_may_stand_alone() {
        let f5 = Shortcut::parse("F5").unwrap();
        assert_eq!(f5.key_code, 96);
        assert_eq!(f5.menu_key(), None);
        assert_eq!(Shortcut::parse("option+F12").unwrap().key_code, 111);
    }

    #[test]
    fn rejects_bad_shortcuts() {
        assert!(Shortcut::parse("Y").is_err(), "needs a modifier");
        assert!(Shortcut::parse("control+").is_err());
        assert!(Shortcut::parse("control+option").is_err(), "no key");
        assert!(Shortcut::parse("control+A+B").is_err());
        assert!(Shortcut::parse("control+F13").is_err());
        assert!(Shortcut::parse("control+Enter").is_err());
    }

    #[test]
    fn from_recorded_keys() {
        let ctrl_opt = Modifiers {
            control: true,
            option: true,
            ..Modifiers::default()
        };
        let shortcut = Shortcut::from_key(16, ctrl_opt).unwrap();
        assert_eq!(shortcut.to_string(), "control+option+Y");
        assert_eq!(shortcut.symbols(), "⌃⌥Y");
        assert_eq!(Shortcut::from_key(49, ctrl_opt).unwrap().key, "Space");
        assert!(
            Shortcut::from_key(16, Modifiers::default()).is_err(),
            "needs a modifier"
        );
        assert!(
            Shortcut::from_key(36, ctrl_opt).is_err(),
            "Return is not supported"
        );
        assert_eq!(
            Shortcut::from_key(96, Modifiers::default()).unwrap().key,
            "F5"
        );
    }

    #[test]
    fn known_conflicts() {
        assert_eq!(
            Shortcut::parse("command+Space").unwrap().known_conflict(),
            Some("Spotlight")
        );
        assert_eq!(
            Shortcut::parse("shift+command+4").unwrap().known_conflict(),
            Some("screenshots")
        );
        assert_eq!(
            Shortcut::parse("control+option+Y")
                .unwrap()
                .known_conflict(),
            None
        );
        assert_eq!(
            Shortcut::parse("command+shift+Y").unwrap().known_conflict(),
            None
        );
    }
}
