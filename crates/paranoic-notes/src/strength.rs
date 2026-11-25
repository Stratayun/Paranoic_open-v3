//! Password strength for the notes password: a conservative estimate, not a
//! promise. Weak passwords are refused; Fair is allowed with a warning.

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum PasswordStrength { Weak, Fair, Strong }

use paranoic_crypto::{contains_common, has_repeats, has_sequence};

/// Owner's rule (25.11.2026): at least 8 characters with a lowercase letter,
/// an uppercase letter, a digit and a symbol (space counts), and no
/// well-known password inside; otherwise Weak. Strong from 12 characters,
/// unless it repeats characters or contains a run (then Fair, audit 16).
/// Length counts Unicode scalar values; the UI counts the same way (audit 13).
/// Unicode letters count (Cyrillic upper/lower case included).
pub fn password_strength(password: &str) -> PasswordStrength {
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < MIN_LEN { return PasswordStrength::Weak; }
    if contains_common(password) { return PasswordStrength::Weak; }
    if !all_kinds(&chars) { return PasswordStrength::Weak; }
    if chars.len() >= STRONG_LEN && !has_repeats(&chars) && !has_sequence(&chars) { PasswordStrength::Strong } else { PasswordStrength::Fair }
}

const MIN_LEN: usize = 8;
const STRONG_LEN: usize = 12;

fn kinds(chars: &[char]) -> [bool; 4] {
    [chars.iter().any(|c| c.is_lowercase()), chars.iter().any(|c| c.is_uppercase()),
        chars.iter().any(|c| c.is_ascii_digit()), chars.iter().any(|c| !c.is_alphanumeric())]
}
fn all_kinds(chars: &[char]) -> bool { kinds(chars).iter().all(|k| *k) }

/// Plan 6.1: what to improve, for a hint next to the password field. The
/// strength itself is `password_strength`; hints only explain it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordHint {
    /// Under 8 characters.
    TooShort,
    /// Contains a well-known password ("qwerty", "пароль"…).
    Common,
    /// The same character over and over, or very few different ones.
    Repeats,
    /// A run like "1234" or "abcd".
    Sequence,
    /// Missing a lowercase letter, an uppercase letter, a digit or a symbol.
    OneKind,
    /// Not strong yet (under 12 characters): make it longer.
    AddWord,
}

pub fn password_feedback(password: &str) -> (PasswordStrength, Vec<PasswordHint>) {
    let strength = password_strength(password);
    let chars: Vec<char> = password.chars().collect();
    let mut hints = Vec::new();
    if chars.len() < MIN_LEN { hints.push(PasswordHint::TooShort); }
    if contains_common(password) { hints.push(PasswordHint::Common); }
    if has_repeats(&chars) { hints.push(PasswordHint::Repeats); }
    if has_sequence(&chars) { hints.push(PasswordHint::Sequence); }
    if !chars.is_empty() && !all_kinds(&chars) { hints.push(PasswordHint::OneKind); }
    if strength != PasswordStrength::Strong { hints.push(PasswordHint::AddWord); }
    (strength, hints)
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    /// Audit 16: repeats/runs keep the admission rule but are never "Strong".
    #[test] fn repeats_and_runs_are_acceptable_but_not_strong() {
        assert_eq!(password_strength("Aa1!Aa1!Aa1!"), PasswordStrength::Fair);
        assert_eq!(password_strength("Xq7#abcd-Mzt9"), PasswordStrength::Fair);
        assert_eq!(password_strength("Xq7#vRt9-Mzw2"), PasswordStrength::Strong);
    }
    /// Audit 13: length is Unicode scalar values (the UI counts code points too).
    #[test] fn length_counts_scalar_values() {
        assert_eq!(password_strength("Aa729583𝔸"), PasswordStrength::Weak, "a supplementary Unicode letter is not a symbol");
        assert_eq!(password_strength("Aa1!😀😀"), PasswordStrength::Weak, "6 characters");
        assert_eq!(password_strength("Aa1!😀😀😺x"), PasswordStrength::Fair, "8 characters");
    }
}
