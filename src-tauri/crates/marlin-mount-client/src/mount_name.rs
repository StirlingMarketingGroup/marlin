//! The folder name an account mounts at.

/// The longest folder name every OS takes. Linux and macOS count bytes. Windows counts UTF-16 units, and a name never has more of those than bytes.
const MAX_BYTES: usize = 255;

/// Builds the folder name an account mounts at.
///
/// The caller stores the result and never builds it again. `taken` holds the stored names of the
/// caller's other accounts. The provider prefix keeps providers apart. The result is a valid
/// folder name on Linux, macOS, and Windows, because the accounts file can move between machines.
pub fn build(provider: &str, account: &str, taken: &[String]) -> String {
    // Windows device names such as CON and NUL need no check: the "<provider>-" prefix rules them out.
    let mut base: String = format!("{provider}-{account}")
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    base = cut(&base, MAX_BYTES);
    // Windows drops a dot or a space at the end of a name.
    if base.ends_with(['.', ' ']) {
        base.pop();
        base.push('_');
    }

    let mut number = 2;
    let mut candidate = base.clone();
    // The helper compares names the same way (`Mounts::mount`, error -1002).
    while taken
        .iter()
        .any(|name| name.to_lowercase() == candidate.to_lowercase())
    {
        let suffix = format!("-{number}");
        number += 1;
        candidate = format!("{}{suffix}", cut(&base, MAX_BYTES - suffix.len()));
    }
    candidate
}

/// Keeps `name` to at most `max_bytes` bytes, ending on a character boundary.
fn cut(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_string();
    }
    let mut end = max_bytes;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::build;

    #[test]
    fn an_email_keeps_its_characters() {
        assert_eq!(
            build("gdrive", "ana@example.com", &[]),
            "gdrive-ana@example.com"
        );
    }

    #[test]
    fn a_forbidden_character_or_a_control_character_becomes_an_underscore() {
        for ch in ['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\u{1}'] {
            let account = format!("a{ch}b");
            assert_eq!(build("gdrive", &account, &[]), "gdrive-a_b", "{ch:?}");
        }
    }

    #[test]
    fn a_trailing_dot_or_space_becomes_an_underscore() {
        assert_eq!(build("gdrive", "ana.", &[]), "gdrive-ana_");
        assert_eq!(build("gdrive", "ana ", &[]), "gdrive-ana_");
    }

    #[test]
    fn a_name_taken_ignoring_case_gets_a_number() {
        let first = "gdrive-Ana@example.com".to_string();
        assert_eq!(
            build("gdrive", "ana@example.com", &[first.clone()]),
            "gdrive-ana@example.com-2"
        );
        let second = "gdrive-ana@example.com-2".to_string();
        assert_eq!(
            build("gdrive", "ANA@example.com", &[first, second]),
            "gdrive-ANA@example.com-3"
        );
    }

    #[test]
    fn a_long_name_stays_within_255_bytes() {
        let account = "a".repeat(300);
        let name = build("gdrive", &account, &[]);
        assert!(name.len() <= 255, "{}", name.len());
        let numbered = build("gdrive", &account, &[name]);
        assert!(numbered.ends_with("-2"), "{numbered}");
        assert!(numbered.len() <= 255, "{}", numbered.len());

        let wide = "你".repeat(300);
        let wide_name = build("gdrive", &wide, &[]);
        assert!(wide_name.len() <= 255, "{}", wide_name.len());
        assert_eq!(wide_name.chars().next_back(), Some('你'));
        let wide_numbered = build("gdrive", &wide, &[wide_name]);
        assert!(wide_numbered.ends_with("-2"), "{wide_numbered}");
        assert!(wide_numbered.len() <= 255, "{}", wide_numbered.len());
    }
}
