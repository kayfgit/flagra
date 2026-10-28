use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

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
}
