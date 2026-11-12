use std::collections::{HashMap, HashSet};

use crate::Item;

/// How one operating system judges file names.
#[derive(Clone, Copy)]
pub struct NameRules {
    /// True for a character this OS cannot hold in a file name.
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

/// Replaces characters this OS forbids, and the names `.` and `..`, so that [`unescape`] gives the remote name back.
pub fn escape(name: &str, rules: &NameRules) -> String {
    if name == "." || name == ".." {
        return name.replace('.', "%2E");
    }
    let mut out = String::with_capacity(name.len());
    let mut rest = name.chars();
    while let Some(character) = rest.next() {
        let hex_digits_next = rest.as_str().bytes().take(2);
        if (rules.forbidden)(character) {
            for byte in character.encode_utf8(&mut [0; 4]).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else if character == '%' && hex_digits_next.filter(u8::is_ascii_hexdigit).count() == 2 {
            // A literal `%41` in a remote name must not come back from `unescape` as `A`.
            out.push_str("%25");
        } else {
            out.push(character);
        }
    }
    out
}

/// Turns each `%XX` back into its byte. If the bytes are not valid UTF-8, the input is returned unchanged.
pub fn unescape(name: &str) -> String {
    let bytes = name.as_bytes();
    let hex = |at: usize| bytes.get(at).and_then(|byte| (*byte as char).to_digit(16));
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if let (b'%', Some(high), Some(low)) = (bytes[index], hex(index + 1), hex(index + 2)) {
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| name.to_string())
}

/// Rewrites `item.name` so one folder can be shown on this OS. The result depends only on the set of ids and names. Nothing is stored, so adding or removing a duplicate can change another item's suffix.
pub fn unique_names(items: &mut [Item], rules: &NameRules) {
    for item in items.iter_mut() {
        item.name = escape(&item.name, rules);
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        *counts.entry((rules.key)(&item.name)).or_default() += 1;
    }
    let mut taken = HashSet::new();
    let mut pending = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = (rules.key)(&item.name);
        if counts[&key] == 1 {
            taken.insert(key);
        } else {
            pending.push(index);
        }
    }
    pending.sort_by(|&left, &right| items[left].id.cmp(&items[right].id));
    for index in pending {
        let original = items[index].name.clone();
        let (stem, extension) = match original.rfind('.') {
            Some(dot) if dot > 0 => original.split_at(dot),
            _ => (original.as_str(), ""),
        };
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
                },
            })
            .collect()
    }

    fn mapped(pairs: &[(&str, &str)], rules: &NameRules) -> Vec<(String, String)> {
        let mut items = sample(pairs);
        unique_names(&mut items, rules);
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
        assert_eq!(escape("/", &POSIX), "%2F");
        assert_eq!(escape("%2F", &POSIX), "%252F");
        assert_eq!(escape("100% done.txt", &POSIX), "100% done.txt");
        assert_eq!(escape(".", &POSIX), "%2E");
        assert_eq!(escape("..", &POSIX), "%2E%2E");
        assert_eq!(escape("a\u{2603}b", &SNOWMAN), "a%E2%98%83b");
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
            assert_eq!(unescape(&escape(name, &SNOWMAN)), name, "{name:?}");
        }
        assert_eq!(unescape("%FF"), "%FF");
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
