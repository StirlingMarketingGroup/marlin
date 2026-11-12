//! JSON log lines on stderr. Free text goes through [`redact`] first.

use std::io::Write;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

static BEARER_OR_BASIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\b(?:Bearer|Basic) )(\S+)").expect("bearer regex"));

static SECRET_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(["']?(?:access_token|refresh_token|id_token|client_secret|password|passwd|api_key|apikey|secret|token)["']?\s*[=:]\s*)("[^"]*"|'[^']*'|[^"'\s,}]+)"#,
    )
    .expect("secret field regex")
});

static URL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(://)([^/@\s]+:[^/@\s]+)@").expect("url userinfo regex"));

/// Masks credentials and tokens in free text. `MountError`'s `Display` calls it, so it covers every log line, status, and response.
pub fn redact(text: &str) -> String {
    let text = URL_USERINFO.replace_all(text, "$1[redacted]@");
    let text = BEARER_OR_BASIC.replace_all(&text, "$1[redacted]");
    SECRET_FIELD.replace_all(&text, "$1[redacted]").into_owned()
}

/// Writes one JSON object as one line on stderr: the time, the level, the event, then `fields`, which is a JSON object or null.
pub fn write(level: &str, event: &str, fields: serde_json::Value) {
    let time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    let mut line = serde_json::json!({"timeMs": time_ms, "level": level, "event": event});
    if let (Some(line), serde_json::Value::Object(fields)) = (line.as_object_mut(), fields) {
        line.extend(fields);
    }
    // One write per line, so lines from two tasks never mix. A write that fails is dropped: a closed stderr must never stop the helper.
    let _ = std::io::stderr().write_all(format!("{line}\n").as_bytes());
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redact_patterns() {
        let sentence = "the mount named token-drive is online";
        for (input, want) in [
            ("Bearer abc123", "Bearer [redacted]"),
            ("Basic QWxhZGRpbjpvcGVu", "Basic [redacted]"),
            ("bEaReR abc123", "bEaReR [redacted]"),
            ("access_token=abc123", "access_token=[redacted]"),
            ("refresh_token: abc123", "refresh_token: [redacted]"),
            (r#""token": "abc123""#, r#""token": [redacted]"#),
            (
                r#"password="correct horse battery staple" rejected"#,
                "password=[redacted] rejected",
            ),
            (r#"{"password":"one,two"}"#, r#"{"password":[redacted]}"#),
            ("secret='one two'", "secret=[redacted]"),
            ("id_token=a", "id_token=[redacted]"),
            ("client_secret=b", "client_secret=[redacted]"),
            ("password=c", "password=[redacted]"),
            ("passwd=d", "passwd=[redacted]"),
            ("secret=e", "secret=[redacted]"),
            ("api_key=f", "api_key=[redacted]"),
            ("apikey=g", "apikey=[redacted]"),
            ("TOKEN=h", "TOKEN=[redacted]"),
            (
                "https://user:password@host.example/path",
                "https://[redacted]@host.example/path",
            ),
            (sentence, sentence),
        ] {
            assert_eq!(redact(input), want, "{input}");
        }
    }
}
