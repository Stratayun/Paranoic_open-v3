//! Password strength for the notes password: a conservative estimate, not a
//! promise. Weak passwords are refused; Fair is allowed with a warning.

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum PasswordStrength { Weak, Fair, Strong }

use paranoic_crypto::{contains_common, has_repeats, has_sequence};

/// Rule (owner 26.11.2026, replaces the 25.11 composition rule): length wins,
/// no mandatory character kinds (NIST SP 800-63B). Weak: under 12 characters,
/// a well-known password inside, fewer than 6 different characters, or under
/// 10 characters once runs ("abcd") and repeats ("aaa") are squeezed out.
/// Strong: 16+ such characters, or 12+ with three kinds; otherwise Fair.
/// Length counts Unicode scalar values; the UI counts the same way (audit 13).
pub fn password_strength(password: &str) -> PasswordStrength {
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < MIN_LEN || contains_common(password) { return PasswordStrength::Weak; }
    let distinct = { let mut d = chars.clone(); d.sort_unstable(); d.dedup(); d.len() };
    let effective = effective_len(&chars);
    if distinct < 6 || effective < 10 { return PasswordStrength::Weak; }
    let kinds = kinds(&chars).iter().filter(|k| **k).count();
    let clean = !has_repeats(&chars) && !has_sequence(&chars);
    if clean && (effective >= 16 || (effective >= MIN_LEN && kinds >= 3)) { PasswordStrength::Strong } else { PasswordStrength::Fair }
}

const MIN_LEN: usize = 12;

/// Length after squeezing each run of 4+ consecutive code points ("1234", "dcba")
/// and each repeat of 3+ ("aaa") down to one character.
fn effective_len(chars: &[char]) -> usize {
    let mut kept = 0;
    let mut i = 0;
    while i < chars.len() {
        let mut j = i + 1;
        while j < chars.len() && chars[j] == chars[i] { j += 1; }
        if j - i >= 3 { kept += 1; i = j; continue; }
        for step in [1i64, -1] {
            let mut k = i + 1;
            while k < chars.len() && chars[k] as i64 - chars[k - 1] as i64 == step { k += 1; }
            if k - i >= 4 { j = k; break; }
        }
        if j - i >= 4 { kept += 1; i = j; } else { kept += 1; i += 1; }
    }
    kept
}

fn kinds(chars: &[char]) -> [bool; 4] {
    [chars.iter().any(|c| c.is_lowercase()), chars.iter().any(|c| c.is_uppercase()),
        chars.iter().any(|c| c.is_ascii_digit()), chars.iter().any(|c| !c.is_alphanumeric())]
}

/// Plan 6.1: what to improve, for a hint next to the password field. The
/// strength itself is `password_strength`; hints only explain it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordHint {
    /// Under 12 characters.
    TooShort,
    /// Contains a well-known password ("qwerty", "пароль"…).
    Common,
    /// The same character over and over, or very few different ones.
    Repeats,
    /// A run like "1234" or "abcd".
    Sequence,
    /// Only one kind of character: a longer phrase or another kind helps (advice, not required).
    OneKind,
    /// Not strong yet: make it longer.
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
    if !chars.is_empty() && kinds(&chars).iter().filter(|k| **k).count() == 1 && chars.len() < 20 { hints.push(PasswordHint::OneKind); }
    if strength != PasswordStrength::Strong { hints.push(PasswordHint::AddWord); }
    (strength, hints)
}

#[cfg(test)]
mod audit_tests {
    use super::*;
    /// Audit 16: repeats/runs keep the admission rule but are never "Strong".
    #[test] fn repeats_and_runs_are_acceptable_but_not_strong() {
        assert_eq!(password_strength("Aa1!Aa1!Aa1!"), PasswordStrength::Weak, "4 different characters");
        assert_eq!(password_strength("Xq7#abcd-Mzt9"), PasswordStrength::Fair);
        assert_eq!(password_strength("Xq7#vRt9-Mzw2"), PasswordStrength::Strong);
    }
    /// Audit 13: length is Unicode scalar values (the UI counts code points too).
    #[test] fn length_counts_scalar_values() {
        assert_eq!(password_strength("Aa1!😀😀😺xQz7"), PasswordStrength::Weak, "11 characters");
        assert_eq!(password_strength("Aa1!😀😀😺xQz7#"), PasswordStrength::Strong, "12 characters, emoji count once each");
    }
}
