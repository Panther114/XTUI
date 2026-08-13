use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Every user-facing command that can be bound to a key in Normal mode.
/// Search-mode editing and sidebar-focus keys stay fixed on purpose: they are
/// modal, discoverable, and re-binding them would trade muscle memory for
/// configuration surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Open,
    Back,
    Search,
    Refresh,
    Profile,
    Likes,
    MediaPreview,
    OpenMedia,
    OpenPost,
    Help,
    Sidebar,
    Home,
    Explore,
    Mentions,
    Bookmarks,
    Lists,
    ToggleFeed,
    ExpandThread,
}

impl Action {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_lowercase().as_str() {
            "quit" => Self::Quit,
            "move_up" | "up" => Self::MoveUp,
            "move_down" | "down" => Self::MoveDown,
            "page_up" | "pageup" => Self::PageUp,
            "page_down" | "pagedown" => Self::PageDown,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "open" => Self::Open,
            "back" => Self::Back,
            "search" => Self::Search,
            "refresh" => Self::Refresh,
            "profile" => Self::Profile,
            "likes" => Self::Likes,
            "media_preview" | "media" => Self::MediaPreview,
            "open_media" => Self::OpenMedia,
            "open_post" => Self::OpenPost,
            "help" => Self::Help,
            "sidebar" => Self::Sidebar,
            "home" => Self::Home,
            "explore" => Self::Explore,
            "mentions" => Self::Mentions,
            "bookmarks" => Self::Bookmarks,
            "lists" => Self::Lists,
            "toggle_feed" | "feed" => Self::ToggleFeed,
            "expand_thread" | "expand" => Self::ExpandThread,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::MoveUp => "move_up",
            Self::MoveDown => "move_down",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Open => "open",
            Self::Back => "back",
            Self::Search => "search",
            Self::Refresh => "refresh",
            Self::Profile => "profile",
            Self::Likes => "likes",
            Self::MediaPreview => "media_preview",
            Self::OpenMedia => "open_media",
            Self::OpenPost => "open_post",
            Self::Help => "help",
            Self::Sidebar => "sidebar",
            Self::Home => "home",
            Self::Explore => "explore",
            Self::Mentions => "mentions",
            Self::Bookmarks => "bookmarks",
            Self::Lists => "lists",
            Self::ToggleFeed => "toggle_feed",
            Self::ExpandThread => "expand_thread",
        }
    }
}

/// A single key description. `"j"`, `"ctrl-u"`, `"shift+l"`, `"enter"`,
/// `"space"`, `"pagedown"` and friends are all valid. The SHIFT modifier is
/// normalized away at match time so terminals that report it for uppercase
/// letters behave identically to those that do not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySpec {
    pub fn parse(spec: &str) -> Option<Self> {
        let mut modifiers = KeyModifiers::empty();
        let mut name = spec;
        for prefix in ["ctrl-", "ctrl+", "shift-", "shift+", "alt-", "alt+"] {
            if let Some(rest) = spec.strip_prefix(prefix) {
                modifiers |= match prefix.trim_end_matches(['-', '+']) {
                    "ctrl" => KeyModifiers::CONTROL,
                    "shift" => KeyModifiers::SHIFT,
                    _ => KeyModifiers::ALT,
                };
                name = rest;
                break;
            }
        }
        let code = match name.to_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            "?" => KeyCode::Char('?'),
            "/" => KeyCode::Char('/'),
            _ => {
                let mut chars = name.chars();
                let first = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                // "shift+x" on a letter means the uppercase key.
                if modifiers.contains(KeyModifiers::SHIFT) && first.is_ascii_lowercase() {
                    KeyCode::Char(first.to_ascii_uppercase())
                } else {
                    KeyCode::Char(first)
                }
            }
        };
        Some(Self { code, modifiers })
    }

    /// A display label for the help and hints ("Enter", "Space", "G", "j").
    pub fn label(&self) -> String {
        match self.code {
            KeyCode::Char(c) if self.modifiers.contains(KeyModifiers::CONTROL) => {
                format!("Ctrl+{}", c.to_ascii_uppercase())
            }
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            other => format!("{other:?}"),
        }
    }

    fn matches(&self, key: &KeyEvent) -> bool {
        if self.code != key.code {
            return false;
        }
        // SHIFT is reported inconsistently across terminals; treat it as noise.
        let actual = key.modifiers & !KeyModifiers::SHIFT;
        let expected = self.modifiers & !KeyModifiers::SHIFT;
        actual == expected
    }
}

/// Action → key bindings. Defaults reproduce XTUI's original keys; a user
/// config layer overrides individual actions.
#[derive(Clone, Debug)]
pub struct KeyBindings {
    map: HashMap<Action, Vec<KeySpec>>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self::from_specs(&[
            (Action::Quit, vec!["q"]),
            (Action::MoveUp, vec!["k", "up"]),
            (Action::MoveDown, vec!["j", "down"]),
            (Action::PageUp, vec!["pageup"]),
            (Action::PageDown, vec!["pagedown"]),
            (Action::Top, vec!["g", "home"]),
            (Action::Bottom, vec!["shift+g", "end"]),
            (Action::Open, vec!["enter", "right"]),
            (Action::Back, vec!["left", "esc", "backspace"]),
            (Action::Search, vec!["/"]),
            (Action::Refresh, vec!["r"]),
            (Action::Profile, vec!["p"]),
            (Action::Likes, vec!["shift+l"]),
            (Action::MediaPreview, vec!["m"]),
            (Action::OpenMedia, vec!["v"]),
            (Action::OpenPost, vec!["o"]),
            (Action::Help, vec!["?"]),
            (Action::Sidebar, vec!["tab"]),
            (Action::Home, vec!["1"]),
            (Action::Explore, vec!["2"]),
            (Action::Mentions, vec!["3"]),
            (Action::Bookmarks, vec!["4"]),
            (Action::Lists, vec!["5"]),
            (Action::ToggleFeed, vec!["f"]),
            (Action::ExpandThread, vec!["space"]),
        ])
    }
}

impl KeyBindings {
    fn from_specs(specs: &[(Action, Vec<&str>)]) -> Self {
        let mut map = HashMap::new();
        for (action, keys) in specs {
            let parsed: Vec<KeySpec> = keys.iter().filter_map(|k| KeySpec::parse(k)).collect();
            map.insert(*action, parsed);
        }
        Self { map }
    }

    /// Merge JSON config overrides over the defaults. Unknown action names or
    /// unparsable keys are ignored so a typo never bricks the interface.
    pub fn from_config(overrides: Option<&HashMap<String, Vec<String>>>) -> Self {
        let mut bindings = Self::default();
        if let Some(map) = overrides {
            for (name, keys) in map {
                let Some(action) = Action::parse(name) else {
                    continue;
                };
                let parsed: Vec<KeySpec> = keys.iter().filter_map(|k| KeySpec::parse(k)).collect();
                if !parsed.is_empty() {
                    bindings.map.insert(action, parsed);
                }
            }
        }
        bindings
    }

    pub fn action_for(&self, key: &KeyEvent) -> Option<Action> {
        self.map
            .iter()
            .find(|(_, specs)| specs.iter().any(|spec| spec.matches(key)))
            .map(|(action, _)| *action)
    }

    pub fn key_label(&self, action: Action) -> String {
        self.map
            .get(&action)
            .and_then(|specs| specs.first())
            .map(|spec| spec.label())
            .unwrap_or_else(|| action.name().into())
    }

    pub fn bound_keys(&self, action: Action) -> Vec<String> {
        self.map
            .get(&action)
            .map(|specs| specs.iter().map(|s| s.label()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn defaults_bind_the_classic_keys() {
        let b = KeyBindings::default();
        assert_eq!(
            b.action_for(&key(KeyCode::Char('j'))),
            Some(Action::MoveDown)
        );
        assert_eq!(b.action_for(&key(KeyCode::Down)), Some(Action::MoveDown));
        assert_eq!(b.action_for(&key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(b.action_for(&key(KeyCode::Esc)), Some(Action::Back));
        assert_eq!(b.action_for(&key(KeyCode::Enter)), Some(Action::Open));
        assert_eq!(b.action_for(&key(KeyCode::Char('L'))), Some(Action::Likes));
        assert_eq!(
            b.action_for(&key(KeyCode::Char('l'))),
            None,
            "likes is L, not l"
        );
        assert_eq!(b.action_for(&key(KeyCode::Char('G'))), Some(Action::Bottom));
        assert_eq!(b.action_for(&key(KeyCode::Char('g'))), Some(Action::Top));
        assert_eq!(
            b.action_for(&key(KeyCode::Char(' '))),
            Some(Action::ExpandThread)
        );
        assert_eq!(b.action_for(&key(KeyCode::Char('1'))), Some(Action::Home));
        assert_eq!(b.action_for(&key(KeyCode::Char('?'))), Some(Action::Help));
    }

    #[test]
    fn config_overrides_win_and_typos_are_ignored() {
        let mut overrides = HashMap::new();
        overrides.insert("move_down".into(), vec!["s".into(), "down".into()]);
        overrides.insert("totally_bogus".into(), vec!["z".into()]);
        overrides.insert("quit".into(), vec!["ctrl-x".into()]);
        let b = KeyBindings::from_config(Some(&overrides));
        assert_eq!(
            b.action_for(&key(KeyCode::Char('s'))),
            Some(Action::MoveDown)
        );
        assert_eq!(b.action_for(&key(KeyCode::Down)), Some(Action::MoveDown));
        assert_eq!(
            b.action_for(&key(KeyCode::Char('j'))),
            None,
            "j was unbound"
        );
        assert_eq!(
            b.action_for(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(b.action_for(&key(KeyCode::Char('z'))), None);
    }

    #[test]
    fn shift_is_normalized_away() {
        let with_shift = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        let without_shift = KeyEvent::from(KeyCode::Char('L'));
        let b = KeyBindings::default();
        assert_eq!(b.action_for(&with_shift), Some(Action::Likes));
        assert_eq!(b.action_for(&without_shift), Some(Action::Likes));
    }

    #[test]
    fn specs_parse_and_label() {
        assert_eq!(KeySpec::parse("ctrl-u").unwrap().label(), "Ctrl+U");
        assert_eq!(KeySpec::parse("space").unwrap().label(), "Space");
        assert_eq!(KeySpec::parse("shift+g").unwrap().label(), "G");
        assert_eq!(KeySpec::parse("enter").unwrap().label(), "Enter");
        assert_eq!(KeySpec::parse("pageup").unwrap().label(), "PgUp");
        assert_eq!(KeySpec::parse("2chars").map(|_| ()), None);
        assert_eq!(KeySpec::parse("ctrl-").map(|_| ()), None);
    }
}
