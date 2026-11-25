//! Random passwords from the OS random source (no external word list).
//! Owner's rule (25.11.2026): every generated password contains a lowercase
//! letter, an uppercase letter, a digit and a symbol. Characters are picked by
//! rejection sampling, so every character of the alphabet is equally likely.
use crate::{random_bytes, CryptoError, Result};
use zeroize::Zeroizing;

const LOWER: &str = "abcdefghijkmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ";
const DIGITS: &str = "23456789";
const SYMBOLS: &str = "!#$%&*+-=?@_~";

fn uniform(below: usize) -> Result<usize> {
    let limit = 256 - 256 % below;
    loop {
        let mut raw = [0u8; 1];
        random_bytes(&mut raw)?;
        if (raw[0] as usize) < limit { return Ok(raw[0] as usize % below); }
    }
}

/// A `length`-character password (12..=64) with all four kinds of characters.
/// Look-alike characters (l, I, O, 0, 1) are left out for readability.
pub fn generate_password(length: usize) -> Result<Zeroizing<String>> {
    if !(12..=64).contains(&length) { return Err(CryptoError::InvalidInput); }
    let groups = [LOWER, UPPER, DIGITS, SYMBOLS];
    let all: Vec<char> = groups.concat().chars().collect();
    loop {
        let mut picked = Zeroizing::new(Vec::with_capacity(length));
        for _ in 0..length { picked.push(all[uniform(all.len())?]); }
        if groups.iter().all(|g| picked.iter().any(|c| g.contains(*c))) {
            return Ok(Zeroizing::new(picked.iter().collect()));
        }
    }
}

/// Kept for the bridge contract: `words` (4..=10) selects the length, 3 characters per word.
pub fn generate_passphrase(words: usize, _separator: char) -> Result<Zeroizing<String>> {
    if !(4..=10).contains(&words) { return Err(CryptoError::InvalidInput); }
    generate_password((words * 3).max(12))
}

/// Entropy of a generated password of `length` characters, in bits.
pub fn passphrase_bits(length: usize) -> f64 {
    length as f64 * ((LOWER.len() + UPPER.len() + DIGITS.len() + SYMBOLS.len()) as f64).log2()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn generated_passwords_have_all_kinds_and_differ() {
        for _ in 0..200 {
            let p = generate_password(16).unwrap();
            assert_eq!(p.chars().count(), 16);
            assert!(p.chars().any(|c| c.is_ascii_lowercase()) && p.chars().any(|c| c.is_ascii_uppercase())
                && p.chars().any(|c| c.is_ascii_digit()) && p.chars().any(|c| !c.is_alphanumeric()));
        }
        assert_ne!(*generate_password(16).unwrap(), *generate_password(16).unwrap());
        assert!(generate_password(8).is_err() && generate_password(65).is_err());
        assert_eq!(generate_passphrase(6, ' ').unwrap().chars().count(), 18);
        assert!(generate_passphrase(3, ' ').is_err());
    }
}
