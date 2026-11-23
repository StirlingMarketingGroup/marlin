use std::collections::{HashMap, HashSet};

use crate::Item;

/// How one operating system judges file names.
#[derive(Clone, Copy)]
pub struct NameRules {
    /// True for a character this OS cannot hold in a file name. `%`, `.`, and the hex digits must never be forbidden: the mapping relies on it.
    pub forbidden: fn(char) -> bool,

    /// Two names with the same key are one name to this OS.
    pub key: fn(&str) -> String,
}

fn posix_forbidden(character: char) -> bool {
    character == '/' || character == '\0'
}

fn posix_key(name: &str) -> String {
    name.to_string()
}

/// Linux name rules: only `/` and NUL are forbidden, and names differ by their exact bytes.
pub const POSIX: NameRules = NameRules {
    forbidden: posix_forbidden,
    key: posix_key,
};

/// Whether a `%` followed by `after` reads as an escape: any number of `25`, then the escape of a character the OS forbids. At the start of a name, `2E` or `2E%2E` that ends the name counts too, so that the names `%2E` and `%2E%2E` stay free for `.` and `..`.
fn reads_as_escape(after: &str, at_start: bool, rules: &NameRules) -> bool {
    let mut rest = after;
    while let Some(next) = rest.strip_prefix("25") {
        rest = next;
    }
    forbidden_escape(rest, rules).is_some() || (at_start && (rest == "2E" || rest == "2E%2E"))
}

/// The forbidden character whose escape `after` starts with, and how many bytes of `after` the escape takes. `after` is the text after a `%`: `2F` for `/`, or `E2%98%83` for a character of three bytes.
fn forbidden_escape(after: &str, rules: &NameRules) -> Option<(char, usize)> {
    // Upper case only: `escape` writes upper case, and a lower-case `%2f` the OS sends must come back as itself.
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let text = after.as_bytes();
    let mut bytes = Vec::with_capacity(4);
    let mut used = 0;
    while bytes.len() < 4 {
        if !bytes.is_empty() {
            if text.get(used) != Some(&b'%') {
                return None;
            }
            used += 1;
        }
        let pair = text.get(used..used + 2)?;
        bytes.push(digit(pair[0])? * 16 + digit(pair[1])?);
        used += 2;
        // A first byte that starts a longer character does not decode alone, so read the next `%XX`.
        if let Ok(decoded) = std::str::from_utf8(&bytes) {
            let character = decoded.chars().next()?;
            return (rules.forbidden)(character).then_some((character, used));
        }
    }
    None
}

/// Turns a remote name into a name this OS can hold. A forbidden character becomes `%XX`, one per byte, and the names `.` and `..` become `%2E` and `%2E%2E`. A `%` becomes `%25` only where the text after it would read as an escape, and only upper-case hex is an escape, so [`unescape`] gives the remote name back.
pub fn escape(name: &str, rules: &NameRules) -> String {
    if name == "." || name == ".." {
        return name.replace('.', "%2E");
    }
    let mut out = String::with_capacity(name.len());
    for (at, character) in name.char_indices() {
        if (rules.forbidden)(character) {
            for byte in character.encode_utf8(&mut [0; 4]).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else if character == '%' && reads_as_escape(&name[at + 1..], at == 0, rules) {
            out.push_str("%25");
        } else {
            out.push(character);
        }
    }
    out
}

/// Turns a name this OS sent back into the remote name. `%2E` and `%2E%2E` become `.` and `..`, and a `%` followed by the escape of a forbidden character becomes that character. A `%25` that stands in front of an escape becomes a literal `%`, and every other `%` is kept as it is.
pub fn unescape(name: &str, rules: &NameRules) -> String {
    match name {
        "%2E" => return ".".to_string(),
        "%2E%2E" => return "..".to_string(),
        _ => {}
    }
    let mut out = String::with_capacity(name.len());
    let mut at = 0;
    while at < name.len() {
        if name.as_bytes()[at] == b'%' {
            let after = &name[at + 1..];
            if let Some((character, length)) = forbidden_escape(after, rules) {
                out.push(character);
                at += 1 + length;
            } else {
                out.push('%');
                at += if after.starts_with("25") && reads_as_escape(&after[2..], at == 0, rules) {
                    3
                } else {
                    1
                };
            }
        } else {
            let character = name[at..].chars().next().unwrap();
            out.push(character);
            at += character.len_utf8();
        }
    }
    out
}

/// Finder's own files, which a macOS mount keeps on this computer: `.DS_Store`, and the `._name` files that hold extended attributes on a filesystem that has none.
pub fn finder_files(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with("._")
}

/// The stem and the extension of `name`. The extension starts at the last `.` that is not the first character, and it includes the dot. A name with no such dot has an empty extension.
pub(crate) fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// Rewrites `item.name` so one folder can be shown on this OS. `held` are names, as the OS shows them, that other entries in the folder hold: an item with one of them gets a suffix. The result depends only on the set of ids and names, and on `held`. Nothing is stored, so adding or removing a duplicate can change another item's suffix.
pub fn unique_names(items: &mut [Item], rules: &NameRules, held: &[&str]) {
    for item in items.iter_mut() {
        item.name = escape(&item.name, rules);
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        *counts.entry((rules.key)(&item.name)).or_default() += 1;
    }
    let held: HashSet<String> = held.iter().map(|name| (rules.key)(name)).collect();
    let mut taken = held.clone();
    let mut pending = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = (rules.key)(&item.name);
        if counts[&key] == 1 && !held.contains(&key) {
            taken.insert(key);
        } else {
            pending.push(index);
        }
    }
    pending.sort_by(|&left, &right| items[left].id.cmp(&items[right].id));
    for index in pending {
        let original = items[index].name.clone();
        let (stem, extension) = split_extension(&original);
        let mut candidate = original.clone();
        let mut number = 2u64;
        while !taken.insert((rules.key)(&candidate)) {
            candidate = format!("{stem} ({number}){extension}");
            number += 1;
        }
        items[index].name = candidate;
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, unescape, unique_names, NameRules, POSIX};
    use crate::{Attributes, Item, ItemId, Kind};
    use std::time::UNIX_EPOCH;

    fn forbid_snowman(character: char) -> bool {
        character == '/' || character == '\0' || character == '\u{2603}'
    }

    const SNOWMAN: NameRules = NameRules {
        forbidden: forbid_snowman,
        key: super::posix_key,
    };

    fn lower_key(name: &str) -> String {
        name.to_lowercase()
    }

    const LOWER: NameRules = NameRules {
        forbidden: super::posix_forbidden,
        key: lower_key,
    };

    fn sample(pairs: &[(&str, &str)]) -> Vec<Item> {
        pairs
            .iter()
            .map(|(id, name)| Item {
                id: ItemId((*id).to_string()),
                name: (*name).to_string(),
                attributes: Attributes {
                    kind: Kind::File,
                    size: 0,
                    modified: UNIX_EPOCH,
                    revision: None,
                    read_only: false,
                },
            })
            .collect()
    }

    fn mapped(pairs: &[(&str, &str)], rules: &NameRules) -> Vec<(String, String)> {
        let mut items = sample(pairs);
        unique_names(&mut items, rules, &[]);
        let mut names: Vec<_> = items
            .into_iter()
            .map(|item| (item.id.0, item.name))
            .collect();
        names.sort();
        names
    }

    fn expect(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, name)| ((*id).to_string(), (*name).to_string()))
            .collect()
    }

    #[test]
    fn escape_and_round_trip() {
        assert_eq!(escape("My%20File.pdf", &POSIX), "My%20File.pdf");
        assert_eq!(escape("100%.txt", &POSIX), "100%.txt");
        assert_eq!(escape("%41", &POSIX), "%41");
        assert_eq!(escape("%25", &POSIX), "%25");
        assert_eq!(escape("/", &POSIX), "%2F");
        assert_eq!(escape("%2F", &POSIX), "%252F");
        assert_eq!(escape("%252F", &POSIX), "%25252F");
        assert_eq!(escape("%2f", &POSIX), "%2f");
        assert_eq!(escape(".", &POSIX), "%2E");
        assert_eq!(escape("..", &POSIX), "%2E%2E");
        assert_eq!(escape("%2E", &POSIX), "%252E");
        assert_eq!(escape("%2E%2E", &POSIX), "%252E%2E");
        assert_eq!(escape("a%2E", &POSIX), "a%2E");
        assert_eq!(unescape("%2F", &POSIX), "/");
        assert_eq!(unescape("%2f", &POSIX), "%2f");
        assert_eq!(unescape("%252F", &POSIX), "%2F");
        assert_eq!(unescape("%25", &POSIX), "%25");
        assert_eq!(unescape("%2E", &POSIX), ".");
        assert_eq!(unescape("%252E", &POSIX), "%2E");
        assert_eq!(unescape("a%2Fb", &POSIX), "a/b");
        assert_eq!(unescape("%FF", &POSIX), "%FF");
        assert_eq!(escape("a\u{2603}b", &SNOWMAN), "a%E2%98%83b");
        assert_eq!(escape("%E2%98%83", &SNOWMAN), "%25E2%98%83");
        assert_eq!(unescape("%E2%98%83", &SNOWMAN), "\u{2603}");
        assert_eq!(unescape("%E2%98", &SNOWMAN), "%E2%98");
        for name in [
            "/",
            "a/b",
            "%2F",
            "100% done.txt",
            "100%",
            ".",
            "..",
            "a\u{2603}b",
            "a\0b",
        ] {
            assert_eq!(
                unescape(&escape(name, &SNOWMAN), &SNOWMAN),
                name,
                "{name:?}"
            );
        }
    }

    #[test]
    fn round_trips_hold_for_every_short_name() {
        // Every name of up to seven characters made of these, so `%2E`, `%2F`, `%25` and chains of them all occur.
        let alphabet = ['%', '2', '5', 'E', 'F', '/', '.', 'a'];
        let mut pending = vec![String::new()];
        while let Some(name) = pending.pop() {
            let escaped = escape(&name, &POSIX);
            assert_eq!(unescape(&escaped, &POSIX), name, "{name:?}");
            assert!(
                escaped != "." && escaped != ".." && !escaped.chars().any(POSIX.forbidden),
                "{name:?} -> {escaped:?}"
            );
            // The OS never sends `.`, `..`, or a forbidden character.
            if name != "." && name != ".." && !name.chars().any(POSIX.forbidden) {
                assert_eq!(escape(&unescape(&name, &POSIX), &POSIX), name, "{name:?}");
            }
            if name.len() < 7 {
                pending.extend(
                    alphabet
                        .iter()
                        .map(|character| format!("{name}{character}")),
                );
            }
        }
    }

    #[test]
    fn duplicates_take_suffixes_in_id_order() {
        let pairs = [
            ("item-2", "a.txt"),
            ("item-1", "a.txt"),
            ("item-4", ".bashrc"),
            ("item-3", ".bashrc"),
        ];
        let expected = expect(&[
            ("item-1", "a.txt"),
            ("item-2", "a (2).txt"),
            ("item-3", ".bashrc"),
            ("item-4", ".bashrc (2)"),
        ]);
        assert_eq!(mapped(&pairs, &POSIX), expected);
        let mut reversed = pairs;
        reversed.reverse();
        assert_eq!(mapped(&reversed, &POSIX), expected);
    }

    #[test]
    fn existing_suffix_is_kept_and_the_duplicate_skips_it() {
        let pairs = [
            ("item-2", "a.txt"),
            ("item-3", "a (2).txt"),
            ("item-1", "a.txt"),
        ];
        let expected = expect(&[
            ("item-1", "a.txt"),
            ("item-2", "a (3).txt"),
            ("item-3", "a (2).txt"),
        ]);
        let mut order = pairs.to_vec();
        for _ in 0..order.len() {
            assert_eq!(mapped(&order, &POSIX), expected);
            order.rotate_left(1);
        }
    }

    #[test]
    fn competing_groups_keep_the_same_names_in_every_order() {
        let pairs = [
            ("item-4", "a (2).txt"),
            ("item-2", "a.txt"),
            ("item-3", "a (2).txt"),
            ("item-1", "a.txt"),
        ];
        let expected = expect(&[
            ("item-1", "a.txt"),
            ("item-2", "a (2).txt"),
            ("item-3", "a (2) (2).txt"),
            ("item-4", "a (2) (3).txt"),
        ]);
        let mut order = pairs.to_vec();
        for _ in 0..order.len() {
            assert_eq!(mapped(&order, &POSIX), expected);
            order.rotate_left(1);
        }
    }

    #[test]
    fn an_item_with_a_held_name_gets_a_suffix() {
        let mut items = sample(&[
            ("item-1", "a.txt"),
            ("item-2", "b.txt"),
            ("item-3", "b.txt"),
        ]);
        unique_names(&mut items, &POSIX, &["a.txt", "b (2).txt"]);
        let mut names: Vec<_> = items
            .into_iter()
            .map(|item| (item.id.0, item.name))
            .collect();
        names.sort();
        assert_eq!(
            names,
            expect(&[
                ("item-1", "a (2).txt"),
                ("item-2", "b.txt"),
                ("item-3", "b (3).txt")
            ])
        );
    }

    #[test]
    fn case_insensitive_key_makes_report_collide() {
        let pairs = [("item-2", "report.txt"), ("item-1", "Report.txt")];
        assert_eq!(
            mapped(&pairs, &LOWER),
            expect(&[("item-1", "Report.txt"), ("item-2", "report (2).txt")])
        );
        assert_eq!(
            mapped(&pairs, &POSIX),
            expect(&[("item-1", "Report.txt"), ("item-2", "report.txt")])
        );
    }
}
