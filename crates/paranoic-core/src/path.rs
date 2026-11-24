use serde::{Deserialize, Serialize};

use crate::{CoreError, ErrorCode, Result};

/// A portable, validated path inside a storage grant. Empty means its root.
/// The same validation runs when recovering requests from the journal.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ResourcePath(String);

impl ResourcePath {
    pub fn root() -> Self { Self(String::new()) }

    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() { return Ok(Self::root()); }
        if value.len() > 4096 {
            return Err(invalid());
        }
        for part in value.split('/') {
            validate_component(part)?;
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str { &self.0 }
    pub fn is_root(&self) -> bool { self.0.is_empty() }

    pub fn join(&self, name: &str) -> Result<Self> {
        validate_component(name)?;
        Self::parse(if self.is_root() { name.to_owned() } else { format!("{}/{name}", self.0) })
    }

    pub fn parent_and_name(&self) -> Result<(Self, &str)> {
        if self.is_root() { return Err(invalid()); }
        match self.0.rsplit_once('/') {
            Some((parent, name)) => Ok((Self(parent.to_owned()), name)),
            None => Ok((Self::root(), &self.0)),
        }
    }
}

fn validate_component(part: &str) -> Result<()> {
    if part.is_empty() || part.len() > 255 || part == "." || part == ".."
        || part.ends_with(['.', ' '])
        || part.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(invalid());
    }
    let upper = part.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    if matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            })
        })
        || upper.starts_with(".PARANOIC-")
    {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> CoreError { CoreError::new(ErrorCode::InvalidPath, "invalid or reserved resource path") }

impl TryFrom<String> for ResourcePath {
    type Error = CoreError;
    fn try_from(value: String) -> Result<Self> { Self::parse(value) }
}

impl From<ResourcePath> for String {
    fn from(value: ResourcePath) -> Self { value.0 }
}
