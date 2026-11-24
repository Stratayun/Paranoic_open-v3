//! Password strength for the notes password: a conservative estimate, not a
//! promise. Weak passwords are refused; Fair is allowed with a warning.

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum PasswordStrength { Weak, Fair, Strong }

const COMMON: &[&str] = &["password", "passw0rd", "qwerty", "123456", "111111", "letmein", "admin", "welcome",
    "iloveyou", "monkey", "dragon", "пароль", "йцукен", "qwertyuiop", "abc123", "000000"];

/// Weak: fewer than 10 characters, a common password inside, or under ~50 bits
/// of estimated entropy. Strong: at least ~72 bits.
pub fn password_strength(password: &str) -> PasswordStrength {
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < 10 { return PasswordStrength::Weak; }
    let lower = password.to_lowercase();
    if COMMON.iter().any(|c| lower.contains(c)) { return PasswordStrength::Weak; }
    let mut pool = 0f64;
    if chars.iter().any(|c| c.is_ascii_lowercase()) { pool += 26.0; }
    if chars.iter().any(|c| c.is_ascii_uppercase()) { pool += 26.0; }
    if chars.iter().any(|c| c.is_ascii_digit()) { pool += 10.0; }
    if chars.iter().any(|c| c.is_ascii_punctuation() || *c == ' ') { pool += 33.0; }
    if chars.iter().any(|c| !c.is_ascii()) { pool += 66.0; }
    // Repetition and runs (aaaa, 1234, abcd) add little: count only "surprising" steps.
    let mut effective = 1f64;
    for pair in chars.windows(2) {
        let step = pair[1] as i64 - pair[0] as i64;
        effective += if step == 0 || step.abs() == 1 { 0.2 } else { 1.0 };
    }
    let distinct = { let mut d = chars.clone(); d.sort_unstable(); d.dedup(); d.len() as f64 };
    let effective = effective.min(distinct * 2.0);
    let bits = effective * pool.max(1.0).log2();
    if bits < 50.0 { PasswordStrength::Weak } else if bits < 72.0 { PasswordStrength::Fair } else { PasswordStrength::Strong }
}

/// Plan 6.1: what to improve, for a hint next to the password field. The
/// strength itself is `password_strength`; hints only explain it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordHint {
    /// Under 10 characters.
    TooShort,
    /// Contains a well-known password ("qwerty", "пароль"…).
    Common,
    /// The same character over and over, or very few different ones.
    Repeats,
    /// A run like "1234" or "abcd".
    Sequence,
    /// Only one kind of character (only letters of one case, only digits).
    OneKind,
    /// Not strong yet: another word helps most.
    AddWord,
}

pub fn password_feedback(password: &str) -> (PasswordStrength, Vec<PasswordHint>) {
    let strength = password_strength(password);
    let chars: Vec<char> = password.chars().collect();
    let mut hints = Vec::new();
    if chars.len() < 10 { hints.push(PasswordHint::TooShort); }
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
    let kinds = [chars.iter().any(|c| c.is_lowercase()), chars.iter().any(|c| c.is_uppercase()),
        chars.iter().any(|c| c.is_ascii_digit()), chars.iter().any(|c| !c.is_alphanumeric())];
    if !chars.is_empty() && kinds.iter().filter(|k| **k).count() == 1 { hints.push(PasswordHint::OneKind); }
    if strength != PasswordStrength::Strong { hints.push(PasswordHint::AddWord); }
    (strength, hints)
}
