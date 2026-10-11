// SPDX-License-Identifier: MIT OR Apache-2.0

//! Canonical JSON (RFC 8785) and the content digest built on it.
//!
//! A document's identity is the SHA-256 of its canonical JSON: a deployment
//! names its release that way, and a release names the hardware fingerprints
//! it was qualified on that way. Because no bundle document holds a
//! floating-point number, canonical JSON here is: object keys sorted by UTF-16
//! code units, no whitespace, integers in plain decimal, and strings escaping
//! only `"`, `\` and control characters. The keys are sorted here rather than
//! left to `serde_json`'s map order, which a feature elsewhere in the graph
//! could change.

use super::ids::Sha256Digest;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

/// The SHA-256 of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> Sha256Digest {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    // A digest has no zero restriction, so this cannot refuse.
    Sha256Digest::from_bytes(out).unwrap_or_else(|_| unreachable!("digests may be zero"))
}

/// The canonical JSON of `value`.
///
/// # Errors
/// When it holds a number that is not an integer: the bundle has none, and
/// rounding one would give two writers two digests.
pub fn canonical(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write(value, &mut out)?;
    Ok(out)
}

/// The content digest of `doc`: the SHA-256 of its canonical JSON.
///
/// # Errors
/// When `doc` does not serialize, or holds a non-integer number.
pub fn digest<T: Serialize>(doc: &T) -> Result<Sha256Digest, String> {
    let value = serde_json::to_value(doc).map_err(|e| e.to_string())?;
    Ok(sha256(canonical(&value)?.as_bytes()))
}

fn write(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => {
                let _ = write!(out, "{u}");
            }
            (None, Some(i)) => {
                let _ = write!(out, "{i}");
            }
            (None, None) => return Err(format!("{n} is not an integer")),
        },
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                write(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_sort_by_utf16_and_nothing_else_is_written() {
        // U+FF61 sorts before U+1F600 by UTF-8 bytes (`serde_json`'s map
        // order) and after it by UTF-16 code units, the order RFC 8785 asks for.
        let v = json!({"b": [1, -2, true, null], "a": {"\u{1F600}": 1, "\u{FF61}": 2}});
        assert_eq!(
            canonical(&v).expect("integers"),
            "{\"a\":{\"\u{1F600}\":1,\"\u{FF61}\":2},\"b\":[1,-2,true,null]}"
        );
    }

    #[test]
    fn strings_escape_only_quote_backslash_and_controls() {
        let v = json!("q\"b\\n\n\u{1}/é");
        assert_eq!(canonical(&v).expect("text"), "\"q\\\"b\\\\n\\n\\u0001/é\"");
    }

    #[test]
    fn a_fraction_is_refused_rather_than_rounded() {
        assert!(canonical(&json!({"x": 0.5})).is_err());
    }
}
