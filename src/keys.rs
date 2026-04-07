//! Key combo types, parsing, and default bindings.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer};

/// A key combination: modifiers + key code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    /// Does this combo match a crossterm key event?
    ///
    /// For character keys, SHIFT is implicit in the case (e.g. 'G' vs 'g'),
    /// so we only check CONTROL and ALT. For non-character keys we also check SHIFT.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        if self.code != event.code {
            return false;
        }
        let mask = match self.code {
            KeyCode::Char(_) => KeyModifiers::CONTROL | KeyModifiers::ALT,
            _ => KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
        };
        (self.modifiers & mask) == (event.modifiers & mask)
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            write!(f, "Ctrl-")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            write!(f, "Alt-")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            write!(f, "S-")?;
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "Space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => write!(f, "Enter"),
            KeyCode::Esc => write!(f, "Esc"),
            KeyCode::Tab => write!(f, "Tab"),
            KeyCode::Up => write!(f, "Up"),
            KeyCode::Down => write!(f, "Down"),
            KeyCode::Left => write!(f, "Left"),
            KeyCode::Right => write!(f, "Right"),
            KeyCode::Backspace => write!(f, "Backspace"),
            KeyCode::Delete => write!(f, "Del"),
            _ => write!(f, "?"),
        }
    }
}

/// Parse a key string like "ctrl-r", "shift-enter", "esc", "G".
impl std::str::FromStr for KeyCombo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key string".into());
        }

        let mut modifiers = KeyModifiers::NONE;
        let mut remaining = s;

        loop {
            let lower = remaining.to_lowercase();
            if let Some(rest) = lower
                .strip_prefix("ctrl-")
                .or_else(|| lower.strip_prefix("c-"))
            {
                modifiers |= KeyModifiers::CONTROL;
                remaining = &remaining[remaining.len() - rest.len()..];
            } else if let Some(rest) = lower
                .strip_prefix("shift-")
                .or_else(|| lower.strip_prefix("s-"))
            {
                modifiers |= KeyModifiers::SHIFT;
                remaining = &remaining[remaining.len() - rest.len()..];
            } else if let Some(rest) = lower
                .strip_prefix("alt-")
                .or_else(|| lower.strip_prefix("a-"))
            {
                modifiers |= KeyModifiers::ALT;
                remaining = &remaining[remaining.len() - rest.len()..];
            } else {
                break;
            }
        }

        let code = match remaining.to_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "space" => KeyCode::Char(' '),
            _ => {
                let chars: Vec<char> = remaining.chars().collect();
                if chars.len() == 1 {
                    let mut c = chars[0];
                    // "shift-g" normalizes to 'G' without SHIFT modifier, since for
                    // character keys we match on the char value directly.
                    if modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_alphabetic() {
                        c = c.to_ascii_uppercase();
                        modifiers -= KeyModifiers::SHIFT;
                    }
                    KeyCode::Char(c)
                } else {
                    return Err(format!("unknown key: {remaining}"));
                }
            }
        };

        Ok(KeyCombo { code, modifiers })
    }
}

impl<'de> Deserialize<'de> for KeyCombo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// One or more key combos that trigger the same action.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Binding {
    Single(KeyCombo),
    Multiple(Vec<KeyCombo>),
}

impl Binding {
    pub fn combos(&self) -> &[KeyCombo] {
        match self {
            Binding::Single(k) => std::slice::from_ref(k),
            Binding::Multiple(v) => v,
        }
    }

    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.combos().iter().any(|k| k.matches(event))
    }

    pub fn display(&self) -> String {
        self.combos()
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn bind(s: &str) -> Binding {
    Binding::Single(s.parse().expect("invalid default key binding"))
}

fn bind_multi(keys: &[&str]) -> Binding {
    Binding::Multiple(
        keys.iter()
            .map(|s| s.parse().expect("invalid default key binding"))
            .collect(),
    )
}

/// Key bindings for Normal mode (search input focused).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NormalKeys {
    pub quit: Binding,
    pub clear_input: Binding,
    pub select_next: Binding,
    pub select_prev: Binding,
    pub scroll_half_down: Binding,
    pub scroll_half_up: Binding,
    pub resume_session: Binding,
    pub fork_session: Binding,
    pub open_detail: Binding,
    pub toggle_help: Binding,
    pub copy_session_id: Binding,
}

impl Default for NormalKeys {
    fn default() -> Self {
        Self {
            quit: bind("ctrl-c"),
            clear_input: bind("ctrl-u"),
            select_next: bind_multi(&["down", "ctrl-n", "ctrl-j"]),
            select_prev: bind_multi(&["up", "ctrl-p", "ctrl-k"]),
            scroll_half_down: bind("ctrl-d"),
            scroll_half_up: bind("ctrl-b"),
            resume_session: bind("enter"),
            fork_session: bind("shift-enter"),
            open_detail: bind("tab"),
            toggle_help: bind("ctrl-/"),
            copy_session_id: bind("ctrl-y"),
        }
    }
}

/// Key bindings for Detail mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DetailKeys {
    pub back: Binding,
    pub quit: Binding,
    pub focus_search: Binding,
    pub scroll_down: Binding,
    pub scroll_up: Binding,
    pub scroll_half_down: Binding,
    pub scroll_half_up: Binding,
    pub top: Binding,
    pub bottom: Binding,
    pub next_match: Binding,
    pub prev_match: Binding,
    pub copy_session_id: Binding,
    pub toggle_help: Binding,
}

impl Default for DetailKeys {
    fn default() -> Self {
        Self {
            back: bind_multi(&["esc", "q"]),
            quit: bind("ctrl-c"),
            focus_search: bind("/"),
            scroll_down: bind_multi(&["down", "j"]),
            scroll_up: bind_multi(&["up", "k"]),
            scroll_half_down: bind("ctrl-d"),
            scroll_half_up: bind("ctrl-b"),
            top: bind("g"),
            bottom: bind("G"),
            next_match: bind("n"),
            prev_match: bind("N"),
            copy_session_id: bind("y"),
            toggle_help: bind("ctrl-/"),
        }
    }
}

/// Key bindings for Help mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HelpKeys {
    pub close: Binding,
}

impl Default for HelpKeys {
    fn default() -> Self {
        Self {
            close: bind_multi(&["esc", "q", "ctrl-/"]),
        }
    }
}

/// All key bindings, organized by mode.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KeyBindings {
    pub normal: NormalKeys,
    pub detail: DetailKeys,
    pub help: HelpKeys,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_shift(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn parse_simple_char() {
        let k: KeyCombo = "r".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('r'));
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_uppercase_char() {
        let k: KeyCombo = "G".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('G'));
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_shift_letter_normalizes() {
        let k: KeyCombo = "shift-g".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('G'));
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_ctrl_char() {
        let k: KeyCombo = "ctrl-r".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('r'));
        assert_eq!(k.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_enter() {
        let k: KeyCombo = "shift-enter".parse().unwrap();
        assert_eq!(k.code, KeyCode::Enter);
        assert_eq!(k.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_esc() {
        let k: KeyCombo = "esc".parse().unwrap();
        assert_eq!(k.code, KeyCode::Esc);
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_ctrl_slash() {
        let k: KeyCombo = "ctrl-/".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('/'));
        assert_eq!(k.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn matches_ctrl_r() {
        let k: KeyCombo = "ctrl-r".parse().unwrap();
        assert!(k.matches(&key_ctrl(KeyCode::Char('r'))));
        assert!(!k.matches(&key(KeyCode::Char('r'))));
    }

    #[test]
    fn matches_bare_char() {
        let k: KeyCombo = "r".parse().unwrap();
        assert!(k.matches(&key(KeyCode::Char('r'))));
        assert!(!k.matches(&key_ctrl(KeyCode::Char('r'))));
    }

    #[test]
    fn matches_shift_enter() {
        let k: KeyCombo = "shift-enter".parse().unwrap();
        assert!(k.matches(&key_shift(KeyCode::Enter)));
        assert!(!k.matches(&key(KeyCode::Enter)));
    }

    #[test]
    fn matches_uppercase_ignores_shift_modifier() {
        let k: KeyCombo = "G".parse().unwrap();
        // Terminals may report 'G' with or without SHIFT
        assert!(k.matches(&key(KeyCode::Char('G'))));
        assert!(k.matches(&key_shift(KeyCode::Char('G'))));
    }

    #[test]
    fn binding_single_matches() {
        let b = bind("ctrl-r");
        assert!(b.matches(&key_ctrl(KeyCode::Char('r'))));
        assert!(!b.matches(&key(KeyCode::Char('r'))));
    }

    #[test]
    fn binding_multi_matches_any() {
        let b = bind_multi(&["down", "ctrl-n"]);
        assert!(b.matches(&key(KeyCode::Down)));
        assert!(b.matches(&key_ctrl(KeyCode::Char('n'))));
        assert!(!b.matches(&key(KeyCode::Up)));
    }

    #[test]
    fn display_ctrl_r() {
        let k: KeyCombo = "ctrl-r".parse().unwrap();
        assert_eq!(k.to_string(), "Ctrl-r");
    }

    #[test]
    fn display_shift_enter() {
        let k: KeyCombo = "shift-enter".parse().unwrap();
        assert_eq!(k.to_string(), "S-Enter");
    }

    #[test]
    fn display_esc() {
        let k: KeyCombo = "esc".parse().unwrap();
        assert_eq!(k.to_string(), "Esc");
    }

    #[test]
    fn deserialize_toml_single() {
        #[derive(Deserialize)]
        struct T {
            key: Binding,
        }
        let t: T = toml::from_str(r#"key = "ctrl-r""#).unwrap();
        assert!(t.key.matches(&key_ctrl(KeyCode::Char('r'))));
    }

    #[test]
    fn deserialize_toml_array() {
        #[derive(Deserialize)]
        struct T {
            key: Binding,
        }
        let t: T = toml::from_str(r#"key = ["down", "ctrl-n"]"#).unwrap();
        assert!(t.key.matches(&key(KeyCode::Down)));
        assert!(t.key.matches(&key_ctrl(KeyCode::Char('n'))));
    }

    #[test]
    fn deserialize_keybindings_defaults() {
        let keys: KeyBindings = toml::from_str("").unwrap();
        assert!(keys
            .normal
            .copy_session_id
            .matches(&key_ctrl(KeyCode::Char('y'))));
        assert!(keys.detail.back.matches(&key(KeyCode::Esc)));
    }

    #[test]
    fn deserialize_keybindings_override() {
        let keys: KeyBindings = toml::from_str(
            r#"
            [normal]
            copy_session_id = "ctrl-x"
            "#,
        )
        .unwrap();
        // Override took effect
        assert!(keys
            .normal
            .copy_session_id
            .matches(&key_ctrl(KeyCode::Char('x'))));
        // Other defaults preserved
        assert!(keys.normal.quit.matches(&key_ctrl(KeyCode::Char('c'))));
    }
}
