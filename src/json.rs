//! A minimal JSON emitter for the decisions this guard prints.
//!
//! Vendored from `amont-runtime`, where it existed because that crate must
//! stay dependency-free. Here the reason is different and narrower: this
//! crate DOES depend on `serde_json`, but only for READING — Claude Code's
//! hook payload and the user's `settings.json`, two shapes defined by
//! somebody else. What we WRITE is a flat, known schema of our own, and a
//! hand-rolled escaper for it keeps the reading and the writing from sharing
//! a representation. `preserve_order` is load-bearing for the settings
//! rewrite; nothing about it should reach the hook's answer.
//!
//! Trimmed to what this crate calls. The emitter in `amont-runtime` also
//! carries `bool_field`, `opt_int_field` and `opt_string_field`; they are
//! omitted here because an unused `pub fn` in a binary crate is a `dead_code`
//! warning, and this crate builds under `-D warnings`.

/// Escape `s` per the JSON spec: `"`, `\`, and control characters.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn string_field(key: &str, value: &str) -> String {
    format!("\"{}\":\"{}\"", escape(key), escape(value))
}

/// A number, unquoted — a limit a reader will compare against is worth
/// emitting as one rather than as a string they have to parse back.
pub fn int_field(key: &str, value: i64) -> String {
    format!("\"{}\":{value}", escape(key))
}

pub fn string_array_field(key: &str, values: &[String]) -> String {
    let items: Vec<String> = values
        .iter()
        .map(|v| format!("\"{}\"", escape(v)))
        .collect();
    format!("\"{}\":[{}]", escape(key), items.join(","))
}

/// Comma-join already-built `"key":value` fragments into `{...}`.
pub fn object(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}

/// Comma-join already-built objects into `[...]`.
pub fn array(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_quote_backslash_and_control_chars() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("a\rb"), "a\\rb");
        assert_eq!(escape("a\tb"), "a\\tb");
        assert_eq!(escape("a\u{1}b"), "a\\u0001b");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(escape("pipe-to-tail"), "pipe-to-tail");
        assert_eq!(escape(""), "");
    }

    #[test]
    fn fields_quote_both_key_and_string_value() {
        assert_eq!(
            string_field("rule", "pipe-to-tail"),
            "\"rule\":\"pipe-to-tail\""
        );
        assert_eq!(int_field("per_1000", 42), "\"per_1000\":42");
    }

    #[test]
    fn string_array_field_joins_and_escapes_each_element() {
        assert_eq!(string_array_field("samples", &[]), "\"samples\":[]");
        assert_eq!(
            string_array_field("samples", &["git push".to_string(), "a\"b".to_string()]),
            "\"samples\":[\"git push\",\"a\\\"b\"]"
        );
    }

    #[test]
    fn object_and_array_join_with_commas() {
        assert_eq!(
            object(&["\"a\":1".into(), "\"b\":2".into()]),
            "{\"a\":1,\"b\":2}"
        );
        assert_eq!(array(&["1".into(), "2".into()]), "[1,2]");
        assert_eq!(object(&[]), "{}");
        assert_eq!(array(&[]), "[]");
    }
}
