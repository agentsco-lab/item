//! Text typed into a window as key presses from the compositor's own
//! keyboard (state.rs sends them as go_back does Alt+Left): each character
//! of ASCII as its key on the US layout, shifted where it needs to be, each
//! Cyrillic letter as its key in the keymap's second group (ЙЦУКЕН on the
//! same keys: kbd.rs); a newline is Enter, a tab Tab. Anything else is left
//! out.

/// evdev's codes (xkb's are these plus 8).
pub const ENTER: u32 = 28;
pub const ESC: u32 = 1;
pub const TAB: u32 = 15;
pub const BACKSPACE: u32 = 14;
pub const SPACE: u32 = 57;
pub const UP: u32 = 103;
pub const DOWN: u32 = 108;
pub const PAGE_UP: u32 = 104;
pub const PAGE_DOWN: u32 = 109;
pub const LEFT_SHIFT: u32 = 42;
pub const LEFT_CTRL: u32 = 29;
pub const LEFT_ALT: u32 = 56;
pub const LEFT: u32 = 105;
pub const RIGHT: u32 = 106;
pub const HOME: u32 = 102;
pub const END: u32 = 107;
const KEY_C: u32 = 46;
const KEY_W: u32 = 17;

/// One key press: its code, the modifiers held for it, and the keymap's
/// group it is read in (0 the US layout, 1 the Russian).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub code: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub group: u8,
}

impl Key {
    pub const fn plain(code: u32) -> Key {
        Key { code, shift: false, ctrl: false, alt: false, group: 0 }
    }
    pub const fn shifted(code: u32) -> Key {
        Key { code, shift: true, ctrl: false, alt: false, group: 0 }
    }
    pub const fn ctrl(code: u32) -> Key {
        Key { code, shift: false, ctrl: true, alt: false, group: 0 }
    }
    const fn ru(code: u32, shift: bool) -> Key {
        Key { code, shift, ctrl: false, alt: false, group: 1 }
    }
}

pub const ENTER_KEY: Key = Key::plain(ENTER);
pub const ESC_KEY: Key = Key::plain(ESC);
pub const CTRL_C: Key = Key::ctrl(KEY_C);
/// A word back: a shell's, and an editor's or a field's.
pub const CTRL_W: Key = Key::ctrl(KEY_W);
pub const CTRL_BACKSPACE: Key = Key::ctrl(BACKSPACE);
pub const TAB_KEY: Key = Key::plain(TAB);
pub const SHIFT_TAB: Key = Key::shifted(TAB);
pub const BACKSPACE_KEY: Key = Key::plain(BACKSPACE);
pub const UP_KEY: Key = Key::plain(UP);
pub const DOWN_KEY: Key = Key::plain(DOWN);
pub const SPACE_KEY: Key = Key::plain(SPACE);
pub const PAGE_UP_KEY: Key = Key::plain(PAGE_UP);
pub const PAGE_DOWN_KEY: Key = Key::plain(PAGE_DOWN);
pub const LEFT_KEY: Key = Key::plain(LEFT);
pub const RIGHT_KEY: Key = Key::plain(RIGHT);
pub const HOME_KEY: Key = Key::plain(HOME);
pub const END_KEY: Key = Key::plain(END);

/// A Cyrillic letter's key: ЙЦУКЕН lies on the same keys as QWERTY, and
/// the keymap's second group reads them so.
fn key_of_ru(c: char) -> Option<Key> {
    const ROWS: [(&str, &[u32]); 3] = [
        ("йцукенгшщзхъ", &[16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27]),
        ("фывапролджэ", &[30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40]),
        ("ячсмитьбю", &[44, 45, 46, 47, 48, 49, 50, 51, 52]),
    ];
    let lower: String = c.to_lowercase().collect();
    let lower = lower.chars().next()?;
    if lower == 'ё' {
        return Some(Key::ru(41, c.is_uppercase()));
    }
    ROWS.iter().find_map(|(row, codes)| row.chars().position(|r| r == lower).map(|i| Key::ru(codes[i], c.is_uppercase())))
}

/// A character's key: on the US layout, shifted where it is; a Cyrillic
/// letter's in the second group.
pub fn key_of(c: char) -> Option<Key> {
    let letter = |c: char| -> Option<u32> {
        Some(match c {
            'q' => 16, 'w' => 17, 'e' => 18, 'r' => 19, 't' => 20, 'y' => 21, 'u' => 22, 'i' => 23, 'o' => 24, 'p' => 25,
            'a' => 30, 's' => 31, 'd' => 32, 'f' => 33, 'g' => 34, 'h' => 35, 'j' => 36, 'k' => 37, 'l' => 38,
            'z' => 44, 'x' => 45, 'c' => 46, 'v' => 47, 'b' => 48, 'n' => 49, 'm' => 50,
            _ => return None,
        })
    };
    // The unshifted and the shifted sign on each key of the top row and
    // the punctuation.
    const SIGNS: [(u32, char, char); 21] = [
        (41, '`', '~'), (2, '1', '!'), (3, '2', '@'), (4, '3', '#'), (5, '4', '$'), (6, '5', '%'), (7, '6', '^'), (8, '7', '&'), (9, '8', '*'), (10, '9', '('), (11, '0', ')'),
        (12, '-', '_'), (13, '=', '+'), (26, '[', '{'), (27, ']', '}'), (43, '\\', '|'), (39, ';', ':'), (40, '\'', '"'), (51, ',', '<'), (52, '.', '>'), (53, '/', '?'),
    ];
    if let Some(code) = letter(c.to_ascii_lowercase()) {
        return Some(if c.is_ascii_uppercase() { Key::shifted(code) } else { Key::plain(code) });
    }
    if let Some(k) = key_of_ru(c) {
        return Some(k);
    }
    match c {
        ' ' => Some(Key::plain(SPACE)),
        '\n' => Some(Key::plain(ENTER)),
        '\t' => Some(Key::plain(TAB)),
        _ => SIGNS.iter().find_map(|&(code, plain, shifted)| {
            if c == plain {
                Some(Key::plain(code))
            } else if c == shifted {
                Some(Key::shifted(code))
            } else {
                None
            }
        }),
    }
}

/// The keys that type `text`, in order; what has no key is left out.
pub fn keys_for(text: &str) -> Vec<Key> {
    text.chars().filter_map(key_of).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_signs_and_shift() {
        let keys = keys_for("Hi, 2+2?\n");
        assert_eq!(keys[0], Key::shifted(35));
        assert_eq!(keys[1], Key::plain(23));
        assert_eq!(keys[2], Key::plain(51));
        assert_eq!(keys[3], Key::plain(SPACE));
        assert_eq!(keys[4], Key::plain(3));
        assert_eq!(keys[5], Key::shifted(13));
        assert_eq!(keys[7], Key::shifted(53));
        assert_eq!(keys[8], Key::plain(ENTER));
        assert_eq!(keys.len(), 9);
    }

    #[test]
    fn what_has_no_key_is_left_out() {
        assert_eq!(keys_for("é—ok").len(), 2);
    }

    #[test]
    fn cyrillic_is_read_in_the_second_group() {
        let keys = keys_for("Йа ok");
        assert_eq!(keys[0], Key { code: 16, shift: true, ctrl: false, alt: false, group: 1 });
        assert_eq!(keys[1], Key { code: 33, shift: false, ctrl: false, alt: false, group: 1 });
        assert_eq!(keys[3].group, 0);
    }
}
