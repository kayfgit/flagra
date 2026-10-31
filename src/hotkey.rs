use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};

/// One press can arrive twice (see Keys): anything this soon after is the same press.
const SAME_PRESS: Duration = Duration::from_millis(400);

/// Finds hotkey presses in the raw keyboard feed. RegisterHotKey alone isn't
/// enough: games can switch it off while they're focused (RIDEV_NOHOTKEYS), and
/// then the keys never arrive. Raw input still does, so a press normally arrives
/// both ways, and whichever is first counts.
pub struct Keys<const N: usize> {
    combos: [Option<(HOT_KEY_MODIFIERS, u32)>; N],
    last: [Option<Instant>; N],
    /// The combo key being held, whose auto-repeat isn't a new press.
    held: Option<u32>,
}

impl<const N: usize> Keys<N> {
    pub fn new() -> Self {
        Keys { combos: [None; N], last: [None; N], held: None }
    }

    pub fn set(&mut self, i: usize, combo: Option<(HOT_KEY_MODIFIERS, u32)>) {
        self.combos[i] = combo;
    }

    /// A key event from the raw feed, with the modifiers down at the time.
    /// Returns which combo it pressed.
    pub fn key(&mut self, vk: u32, up: bool, mods: HOT_KEY_MODIFIERS) -> Option<usize> {
        if up {
            if self.held == Some(vk) {
                self.held = None;
            }
            return None;
        }
        if self.held == Some(vk) {
            return None;
        }
        let i = self.combos.iter().position(|c| c.is_some_and(|(m, k)| k == vk && plain(m) == plain(mods)))?;
        self.held = Some(vk);
        Some(i)
    }

    /// Whether this press of combo `i` hasn't been acted on yet.
    pub fn first_report(&mut self, i: usize, now: Instant) -> bool {
        let fresh = self.last[i].map_or(true, |t| now.duration_since(t) >= SAME_PRESS);
        if fresh {
            self.last[i] = Some(now);
        }
        fresh
    }
}

fn plain(mods: HOT_KEY_MODIFIERS) -> u32 {
    mods.0 & !MOD_NOREPEAT.0
}

/// The modifier keys down right now, wherever the keyboard focus is.
pub fn mods_down() -> HOT_KEY_MODIFIERS {
    let mut mods = HOT_KEY_MODIFIERS(0);
    for (vk, m) in [(VK_CONTROL, MOD_CONTROL), (VK_MENU, MOD_ALT), (VK_SHIFT, MOD_SHIFT), (VK_LWIN, MOD_WIN), (VK_RWIN, MOD_WIN)] {
        if unsafe { GetAsyncKeyState(vk.0 as i32) } < 0 {
            mods |= m;
        }
    }
    mods
}

/// Parses e.g. "Ctrl+Shift+F9" into RegisterHotKey modifiers and a virtual-key code.
pub fn parse(s: &str) -> Result<(HOT_KEY_MODIFIERS, u32), String> {
    let mut mods = MOD_NOREPEAT;
    let mut key = None;
    for part in s.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "windows" => mods |= MOD_WIN,
            k => {
                if key.is_some() {
                    return Err("only one non-modifier key is allowed".into());
                }
                key = Some(vk(k).ok_or_else(|| format!("unknown key {:?}", part))?);
            }
        }
    }
    key.map(|k| (mods, k)).ok_or_else(|| "no key given".into())
}

fn vk(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| 0x70 + n - 1);
    }
    if let Some(n) = k.strip_prefix("numpad").and_then(|n| n.parse::<u32>().ok()) {
        return (n <= 9).then(|| 0x60 + n);
    }
    Some(match k {
        "home" => 0x24,
        "end" => 0x23,
        "insert" | "ins" => 0x2D,
        "delete" | "del" => 0x2E,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "printscreen" | "prtsc" | "print" => 0x2C,
        "pause" | "break" => 0x13,
        "scrolllock" => 0x91,
        "space" => 0x20,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "`" | "backquote" | "tilde" => 0xC0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combos() {
        assert_eq!(parse("Alt+F10").unwrap(), (MOD_NOREPEAT | MOD_ALT, 0x79));
        assert_eq!(parse("ctrl + shift + c").unwrap(), (MOD_NOREPEAT | MOD_CONTROL | MOD_SHIFT, b'C' as u32));
        assert_eq!(parse("Numpad5").unwrap().1, 0x65);
        assert!(parse("Alt").is_err());
        assert!(parse("Alt+F25").is_err());
        assert!(parse("A+B").is_err());
    }

    fn keys() -> Keys<2> {
        let mut k = Keys::new();
        k.set(0, Some(parse("Alt+Q").unwrap()));
        k.set(1, Some(parse("Alt+F").unwrap()));
        k
    }

    #[test]
    fn a_held_combo_is_one_press() {
        let mut k = keys();
        assert_eq!(k.key(b'Q' as u32, false, MOD_ALT), Some(0));
        assert_eq!(k.key(b'Q' as u32, false, MOD_ALT), None);
        k.key(b'Q' as u32, true, MOD_ALT);
        assert_eq!(k.key(b'Q' as u32, false, MOD_ALT), Some(0));
        assert_eq!(k.key(b'F' as u32, false, MOD_ALT), Some(1));
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let mut k = keys();
        assert_eq!(k.key(b'Q' as u32, false, HOT_KEY_MODIFIERS(0)), None);
        assert_eq!(k.key(b'Q' as u32, false, MOD_ALT | MOD_SHIFT), None);
        assert_eq!(k.key(b'W' as u32, false, MOD_ALT), None);
    }

    #[test]
    fn the_two_reports_of_a_press_count_once() {
        let mut k = keys();
        let t = Instant::now();
        assert!(k.first_report(0, t));
        assert!(!k.first_report(0, t + Duration::from_millis(50)));
        assert!(k.first_report(1, t + Duration::from_millis(50)));
        assert!(k.first_report(0, t + SAME_PRESS));
    }
}
