//! Canonical serialization — the hashing layer owned by Trellis (spec §19).
//!
//! Hashes are meaningless if equivalent values serialize differently, so
//! every content identity is computed over bytes produced here:
//!
//! - integers as unsigned LEB128 varints;
//! - byte/string payloads as varint length prefix + raw bytes;
//! - lists/maps written in sorted order (helpers sort defensively);
//! - every structured encoding starts with a domain-separation tag string
//!   (e.g. `"trellis.manifest.v1"`) so identities of different domain
//!   objects can never collide.
//!
//! This module defines bytes, not semantics. The CAS identity must never
//! depend on third-party serialization (spec §19).

/// Length prefix + payload encoding for a single item.
pub mod primitives {
    /// Append an unsigned LEB128 varint.
    pub fn write_u64(buf: &mut Vec<u8>, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                buf.push(byte);
                break;
            }
            buf.push(byte | 0x80);
        }
    }

    /// Append a length-prefixed byte payload.
    pub fn write_bytes(buf: &mut Vec<u8>, data: &[u8]) {
        write_u64(buf, data.len() as u64);
        buf.extend_from_slice(data);
    }

    /// Append a length-prefixed UTF-8 string.
    pub fn write_str(buf: &mut Vec<u8>, s: &str) {
        write_bytes(buf, s.as_bytes());
    }

    /// Append a varint item count followed by the strings in sorted order.
    /// Sorts defensively: canonical order is enforced here, not trusted
    /// from the caller (spec §19: lexicographically ordered sets/maps).
    pub fn write_sorted_strs(buf: &mut Vec<u8>, items: &[String]) {
        let mut sorted: Vec<&String> = items.iter().collect();
        sorted.sort();
        write_u64(buf, sorted.len() as u64);
        for item in sorted {
            write_str(buf, item);
        }
    }

    /// Append a varint item count followed by `(key, value)` string pairs
    /// sorted by key then value. Duplicate keys produce an ambiguous
    /// encoding and are a caller-contract violation: map builders must
    /// reject duplicates before encoding (see
    /// `EnvironmentFingerprint::from_declared`).
    pub fn write_sorted_str_pairs(buf: &mut Vec<u8>, pairs: &[(String, String)]) {
        let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
        sorted.sort();
        write_u64(buf, sorted.len() as u64);
        for (k, v) in sorted {
            write_str(buf, k);
            write_str(buf, v);
        }
    }
}

pub use primitives::{
    write_bytes, write_sorted_str_pairs, write_sorted_strs, write_str, write_u64,
};

#[cfg(test)]
mod tests {
    use super::primitives::*;

    #[test]
    fn uvarint_is_compact_and_unambiguous() {
        let mut a = Vec::new();
        write_u64(&mut a, 0);
        assert_eq!(a, vec![0x00]);

        let mut b = Vec::new();
        write_u64(&mut b, 127);
        assert_eq!(b, vec![0x7f]);

        let mut c = Vec::new();
        write_u64(&mut c, 128);
        assert_eq!(c, vec![0x80, 0x01]);
    }

    #[test]
    fn strings_are_length_prefixed() {
        let mut buf = Vec::new();
        write_str(&mut buf, "abc");
        assert_eq!(buf, vec![3, b'a', b'b', b'c']);
    }

    #[test]
    fn sorted_helpers_sort_defensively() {
        let mut buf = Vec::new();
        write_sorted_strs(&mut buf, &["b".into(), "a".into(), "c".into()]);
        assert_eq!(buf, vec![3, 1, b'a', 1, b'b', 1, b'c']);
    }

    #[test]
    fn pair_sorting_is_key_then_value() {
        let mut buf = Vec::new();
        write_sorted_str_pairs(
            &mut buf,
            &[("a".into(), "z".into()), ("a".into(), "b".into())],
        );
        assert_eq!(buf[0], 2);
        // first pair must be ("a","b")
        assert_eq!(&buf[1..], &[1, b'a', 1, b'b', 1, b'a', 1, b'z']);
    }
}
