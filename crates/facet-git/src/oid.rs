//! Compact object ids for the long lists (history, search results, reflog).
//!
//! A history of 100k commits kept every id as a 40-character `String` (plus one per parent):
//! ~150 bytes of heap per commit for ids alone. [`Oid`] stores the raw bytes inline (SHA-1 and
//! SHA-256 both fit) and is `Copy`, so lists, maps and the graph builder never allocate for
//! ids. Hex text is produced on demand into a stack buffer ([`Hex`]).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

/// The longest object id git produces (SHA-256).
const MAX_BYTES: usize = 32;

/// A git object id: the raw bytes of a SHA-1 (20) or SHA-256 (32) hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Oid {
    len: u8,
    bytes: [u8; MAX_BYTES],
}

impl Hash for Oid {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.as_bytes());
    }
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

const DIGITS: &[u8; 16] = b"0123456789abcdef";

impl Oid {
    /// Parse a full hex object id (40 or 64 digits; any even length from 2 to 64 is accepted
    /// so tests can use short ids). Surrounding whitespace is not.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let digits = hex.as_bytes();
        if digits.is_empty() || digits.len() % 2 != 0 || digits.len() > MAX_BYTES * 2 {
            return None;
        }
        let mut bytes = [0u8; MAX_BYTES];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            *byte = nibble(pair[0])? << 4 | nibble(pair[1])?;
        }
        Some(Self {
            len: (digits.len() / 2) as u8,
            bytes,
        })
    }

    /// A stand-in id made from a short name (its bytes, zero-padded to 20), for tests that
    /// name commits "a", "b", "topic". Names longer than 20 bytes are cut.
    #[doc(hidden)]
    pub fn fake(name: &str) -> Self {
        let mut bytes = [0u8; MAX_BYTES];
        for (byte, from) in bytes.iter_mut().zip(name.bytes().take(20)) {
            *byte = from;
        }
        Self { len: 20, bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The full id in lower-case hex, without allocating.
    pub fn hex(&self) -> Hex {
        self.hex_prefix(usize::MAX)
    }

    /// The usual 7-digit abbreviation (not guaranteed unique; for display only).
    pub fn short(&self) -> Hex {
        self.hex_prefix(7)
    }

    fn hex_prefix(&self, digits: usize) -> Hex {
        let mut out = Hex {
            len: 0,
            buf: [0; MAX_BYTES * 2],
        };
        let wanted = digits.min(self.len as usize * 2);
        for (index, byte) in self.as_bytes().iter().enumerate() {
            for (offset, nib) in [byte >> 4, byte & 0xf].into_iter().enumerate() {
                let at = index * 2 + offset;
                if at < wanted {
                    out.buf[at] = DIGITS[nib as usize];
                }
            }
        }
        out.len = wanted as u8;
        out
    }

    /// Whether this id's hex form starts with `prefix` (case-insensitive).
    pub fn starts_with(&self, prefix: &str) -> bool {
        let hex = self.hex();
        hex.len() >= prefix.len() && hex[..prefix.len()].eq_ignore_ascii_case(prefix)
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", &*self.hex())
    }
}

impl PartialEq<str> for Oid {
    fn eq(&self, other: &str) -> bool {
        Oid::from_hex(other).is_some_and(|other| other == *self)
    }
}

impl PartialEq<&str> for Oid {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<String> for Oid {
    fn eq(&self, other: &String) -> bool {
        self == other.as_str()
    }
}

/// Hex text of an [`Oid`] on the stack; derefs to `str`.
#[derive(Clone, Copy)]
pub struct Hex {
    len: u8,
    buf: [u8; MAX_BYTES * 2],
}

impl Deref for Hex {
    type Target = str;

    fn deref(&self) -> &str {
        // Only ASCII hex digits are ever written.
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or_default()
    }
}

impl AsRef<str> for Hex {
    fn as_ref(&self) -> &str {
        self
    }
}

impl fmt::Display for Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self)
    }
}

/// A commit's parents. Almost every commit has one, so a single parent is stored inline and
/// only merges allocate.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum Parents {
    #[default]
    None,
    One(Oid),
    Many(Box<[Oid]>),
}

impl Parents {
    pub fn from_vec(mut parents: Vec<Oid>) -> Self {
        match parents.len() {
            0 => Self::None,
            1 => Self::One(parents.remove(0)),
            _ => Self::Many(parents.into_boxed_slice()),
        }
    }
}

impl Deref for Parents {
    type Target = [Oid];

    fn deref(&self) -> &[Oid] {
        match self {
            Self::None => &[],
            Self::One(oid) => std::slice::from_ref(oid),
            Self::Many(oids) => oids,
        }
    }
}

impl fmt::Debug for Parents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<const N: usize> PartialEq<[&str; N]> for Parents {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.len() == N && self.iter().zip(other).all(|(oid, hex)| oid == hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn hex_round_trips_and_compares_with_text() {
        let oid = Oid::from_hex(SHA1).unwrap();
        assert_eq!(&*oid.hex(), SHA1);
        assert_eq!(oid.to_string(), SHA1);
        assert_eq!(&*oid.short(), "0123456");
        assert_eq!(oid, *SHA1);
        assert_eq!(oid, SHA1.to_uppercase());
        assert!(oid.starts_with("0123456789AB"));
        assert!(!oid.starts_with("1"));
        assert_eq!(oid.as_bytes().len(), 20);

        let sha256 = "ab".repeat(32);
        let long = Oid::from_hex(&sha256).unwrap();
        assert_eq!(&*long.hex(), sha256);
        assert_ne!(long, oid);
    }

    #[test]
    fn bad_hex_is_rejected() {
        for bad in ["", "abc", "zz", &"a".repeat(66), " 0123", "01 3"] {
            assert!(Oid::from_hex(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn parents_store_one_inline() {
        let a = Oid::from_hex("aa").unwrap();
        let b = Oid::from_hex("bb").unwrap();
        assert!(Parents::from_vec(Vec::new()).is_empty());
        assert!(matches!(Parents::from_vec(vec![a]), Parents::One(_)));
        let merge = Parents::from_vec(vec![a, b]);
        assert_eq!(merge.len(), 2);
        assert_eq!(merge.first(), Some(&a));
        assert_eq!(merge, ["aa", "bb"]);
        assert!(std::mem::size_of::<Oid>() <= 33);
    }
}
