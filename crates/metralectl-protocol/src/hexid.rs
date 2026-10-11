// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fixed-width ids written as hex.
//!
//! The platform's documents carry every id and digest as hex of a fixed width:
//! writers emit lower case, readers accept either. One decoder and one macro
//! here, so every id type reads and writes that form the same way.

/// Decode exactly `N` bytes of hex, either case.
///
/// # Errors
/// When `s` is not `2 * N` hexadecimal characters.
pub fn decode<const N: usize>(s: &str) -> Result<[u8; N], String> {
    if s.len() != 2 * N {
        return Err(format!(
            "expected {} hex characters, got {}",
            2 * N,
            s.len()
        ));
    }
    let mut out = [0u8; N];
    for (i, pair) in s.as_bytes().chunks_exact(2).enumerate() {
        let digit = |b: u8| char::from(b).to_digit(16);
        match (digit(pair[0]), digit(pair[1])) {
            // Each digit is below 16, so the byte cannot overflow.
            (Some(hi), Some(lo)) => out[i] = u8::try_from(hi * 16 + lo).unwrap_or(0),
            _ => return Err(format!("character {} is not hexadecimal", i * 2)),
        }
    }
    Ok(out)
}

/// Write bytes as lower-case hex.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Refuse all-zero `bytes` when the id `name` may not be zero.
///
/// # Errors
/// When `nonzero` and every byte is zero.
pub fn refuse_zero(nonzero: bool, bytes: &[u8], name: &str) -> Result<(), String> {
    if nonzero && bytes.iter().all(|b| *b == 0) {
        return Err(format!("{name} is never all zero"));
    }
    Ok(())
}

/// Define a fixed-width hex id: parsed from hex of either case, written in
/// lower case, and (with `nonzero: true`) never all zero.
#[macro_export]
macro_rules! hex_id {
    ($(#[$meta:meta])* $vis:vis struct $name:ident([u8; $n:literal]); nonzero: $nz:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        $vis struct $name([u8; $n]);

        impl $name {
            /// Wrap raw bytes.
            ///
            /// # Errors
            /// When this id may not be all zero and is.
            pub fn from_bytes(bytes: [u8; $n]) -> Result<Self, String> {
                $crate::hexid::refuse_zero($nz, &bytes, stringify!($name))?;
                Ok(Self(bytes))
            }

            /// Parse hex of either case.
            ///
            /// # Errors
            /// When `s` is not the id's width of hex, or is all zero where
            /// that is not allowed.
            pub fn parse(s: &str) -> Result<Self, String> {
                let bytes = $crate::hexid::decode::<$n>(s)
                    .map_err(|e| format!("{}: {e}", stringify!($name)))?;
                Self::from_bytes(bytes)
            }

            /// The raw bytes.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $n] {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&$crate::hexid::encode(&self.0))
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.to_string()
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> Result<Self, String> {
                Self::parse(&s)
            }
        }
    };
}

hex_id! {
    /// A W3C trace id: 16 bytes, never all zero.
    pub struct TraceId([u8; 16]); nonzero: true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn either_case_reads_and_lower_case_writes() {
        let id = TraceId::parse("4BF92F3577B34DA6A3CE929D0E0E4736").expect("hex");
        assert_eq!(id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(TraceId::parse(&id.to_string()), Ok(id));
    }

    #[test]
    fn a_wrong_width_a_non_hex_digit_and_all_zero_are_refused() {
        assert!(TraceId::parse("abc").is_err());
        assert!(TraceId::parse("4bf92f3577b34da6a3ce929d0e0e473g").is_err());
        assert!(TraceId::parse(&"0".repeat(32)).is_err());
        assert!(TraceId::from_bytes([0; 16]).is_err());
    }
}
