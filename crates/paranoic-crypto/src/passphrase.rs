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

/// Well-known passwords: a password containing one is weak (shared by the
/// generator and the notes policy, audit 15).
pub const COMMON_PASSWORDS: &[&str] = &["password", "passw0rd", "qwerty", "123456", "111111", "letmein", "admin", "welcome",
    "iloveyou", "monkey", "dragon", "пароль", "йцукен", "qwertyuiop", "abc123", "000000"];

pub fn contains_common(password: &str) -> bool {
    let lower = password.to_lowercase();
    COMMON_PASSWORDS.iter().any(|c| lower.contains(c))
}
/// The same character three times in a row, or fewer than half distinct characters.
pub fn has_repeats(chars: &[char]) -> bool {
    let distinct = { let mut d = chars.to_vec(); d.sort_unstable(); d.dedup(); d.len() };
    chars.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) || (chars.len() >= 4 && distinct * 2 < chars.len())
}
/// A run of four like "1234", "abcd" or "dcba".
pub fn has_sequence(chars: &[char]) -> bool {
    chars.windows(4).any(|w| (0..3).all(|i| (w[i + 1] as i64 - w[i] as i64) == 1)
        || (0..3).all(|i| (w[i + 1] as i64 - w[i] as i64) == -1))
}

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
        // The same acceptance as the notes policy: every kind, nothing well
        // known, no repeats or runs, so a generated password is always Strong.
        let candidate = Zeroizing::new(picked.iter().collect::<String>());
        if groups.iter().all(|g| picked.iter().any(|c| g.contains(*c))) && !contains_common(&candidate)
            && !has_repeats(&picked) && !has_sequence(&picked) {
            return Ok(candidate);
        }
    }
}

/// Kept for the bridge contract: `words` (4..=10) selects the length, 3 characters per word.
pub fn generate_passphrase(words: usize, _separator: char) -> Result<Zeroizing<String>> {
    if !(4..=10).contains(&words) { return Err(CryptoError::InvalidInput); }
    generate_password((words * 3).max(12))
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
