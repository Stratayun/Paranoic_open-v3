//! Password strength for the notes password: a conservative estimate, not a
//! promise. Weak passwords are refused; Fair is allowed with a warning.

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum PasswordStrength { Weak, Fair, Strong }

const COMMON: &[&str] = &["password", "passw0rd", "qwerty", "123456", "111111", "letmein", "admin", "welcome",
    "iloveyou", "monkey", "dragon", "пароль", "йцукен", "qwertyuiop", "abc123", "000000"];

/// Owner's rule (25.11.2026): at least 8 characters with a lowercase letter,
/// an uppercase letter, a digit and a symbol (space counts), and no
/// well-known password inside; otherwise Weak. Strong from 12 characters.
/// Unicode letters count (Cyrillic upper/lower case included).
pub fn password_strength(password: &str) -> PasswordStrength {
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < MIN_LEN { return PasswordStrength::Weak; }
    let lower = password.to_lowercase();
    if COMMON.iter().any(|c| lower.contains(c)) { return PasswordStrength::Weak; }
    if !all_kinds(&chars) { return PasswordStrength::Weak; }
    if chars.len() >= STRONG_LEN { PasswordStrength::Strong } else { PasswordStrength::Fair }
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
    let lower = password.to_lowercase();
    if COMMON.iter().any(|c| lower.contains(c)) { hints.push(PasswordHint::Common); }
    let distinct = { let mut d = chars.clone(); d.sort_unstable(); d.dedup(); d.len() };
    if chars.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) || (chars.len() >= 4 && distinct * 2 < chars.len()) {
        hints.push(PasswordHint::Repeats);
    }
    if chars.windows(4).any(|w| (0..3).all(|i| (w[i + 1] as i64 - w[i] as i64) == 1)
        || (0..3).all(|i| (w[i + 1] as i64 - w[i] as i64) == -1)) {
        hints.push(PasswordHint::Sequence);
    }
    if !chars.is_empty() && !all_kinds(&chars) { hints.push(PasswordHint::OneKind); }
    if strength != PasswordStrength::Strong { hints.push(PasswordHint::AddWord); }
    (strength, hints)
}
