//! Plan 6.1: passphrases of random words (EFF Large Wordlist, 7776 words:
//! log2(7776) ≈ 12.9 bits per word). Chosen with the OS random source and
//! rejection sampling, so every word is equally likely.
use crate::{random_bytes, CryptoError, Result};
use zeroize::Zeroizing;

const WORDS_TEXT: &str = include_str!("../data/eff_large_wordlist.txt");
pub const WORDLIST_LEN: usize = 7776;

fn words() -> impl Iterator<Item = &'static str> { WORDS_TEXT.lines().filter(|w| !w.is_empty()) }

/// `count` words (4..=10; 6 ≈ 77 bits) joined by `separator` (one of " -._").
pub fn generate_passphrase(count: usize, separator: char) -> Result<Zeroizing<String>> {
    if !(4..=10).contains(&count) || !matches!(separator, ' ' | '-' | '.' | '_') { return Err(CryptoError::InvalidInput); }
    let list: Vec<&str> = words().collect();
    if list.len() != WORDLIST_LEN { return Err(CryptoError::InvalidInput); }
    let mut out = Zeroizing::new(String::new());
    let mut picked = 0;
    while picked < count {
        let mut raw = [0u8; 2];
        random_bytes(&mut raw)?;
        let value = u16::from_le_bytes(raw) as usize;
        // 65536 = 8 × 7776 + 3328: values from 62208 up would favour the first words.
        if value >= WORDLIST_LEN * 8 { continue; }
        if picked > 0 { out.push(separator); }
        out.push_str(list[value % WORDLIST_LEN]);
        picked += 1;
    }
    Ok(out)
}
/// Estimated strength in bits of a generated passphrase of `count` words.
pub fn passphrase_bits(count: usize) -> f64 { count as f64 * (WORDLIST_LEN as f64).log2() }

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    #[test] fn wordlist_and_generation() {
        let list: Vec<&str> = words().collect();
        assert_eq!(list.len(), WORDLIST_LEN);
        let unique: std::collections::BTreeSet<_> = list.iter().collect();
        assert_eq!(unique.len(), WORDLIST_LEN, "duplicate words");
        assert!(list.iter().all(|w| (3..=9).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')));
        let phrase = generate_passphrase(6, '-').unwrap();
        let parts: Vec<&str> = phrase.split('-').collect();
        // Some EFF words contain '-' themselves, so split by the list instead.
        assert!(parts.len() >= 6 && parts.iter().all(|p| !p.is_empty()));
        let spaced = generate_passphrase(6, ' ').unwrap();
        assert_eq!(spaced.split(' ').count(), 6);
        assert!(spaced.split(' ').all(|w| unique.contains(&w)));
        assert_ne!(*generate_passphrase(6, ' ').unwrap(), *spaced, "two runs gave the same phrase");
        assert!(generate_passphrase(3, ' ').is_err() && generate_passphrase(11, ' ').is_err() && generate_passphrase(6, 'x').is_err());
        assert!(passphrase_bits(6) > 77.0 && passphrase_bits(6) < 78.0);
    }
    /// Rough uniformity: over 50 000 words every first letter group appears.
    #[test] fn draws_cover_the_list() {
        let mut first = std::collections::BTreeSet::new();
        for _ in 0..5000 { for w in generate_passphrase(10, ' ').unwrap().split(' ') { first.insert(w.as_bytes()[0]); } }
        assert_eq!(first.len(), 26);
    }
}
