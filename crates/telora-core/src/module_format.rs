//! Resource format classification for Host inventories; not module identity.
use std::{fmt, path::Path};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleFormat {
    Telora,
    Json,
    Toml,
    Yaml,
}

impl ModuleFormat {
    pub fn from_path(path: &Path) -> Result<Self, ModuleFormatError> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or(ModuleFormatError::MissingExtension)?;
        match extension {
            "telora" => Ok(Self::Telora),
            "json" => Ok(Self::Json),
            "toml" => Ok(Self::Toml),
            "yaml" | "yml" => Ok(Self::Yaml),
            _ => Err(ModuleFormatError::UnknownExtension(extension.into())),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Telora => "telora",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleFormatError {
    MissingExtension,
    UnknownExtension(String),
}

impl fmt::Display for ModuleFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingExtension => f.write_str("module path has no extension"),
            Self::UnknownExtension(extension) => write!(f, "unknown module extension .{extension}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_exact_and_case_sensitive() {
        for (path, expected) in [
            ("a.telora", ModuleFormat::Telora),
            ("a.json", ModuleFormat::Json),
            ("a.toml", ModuleFormat::Toml),
            ("a.yaml", ModuleFormat::Yaml),
            ("a.yml", ModuleFormat::Yaml),
        ] {
            assert_eq!(ModuleFormat::from_path(Path::new(path)), Ok(expected));
        }
        for path in ["a", "a.JSON", "a.txt"] {
            assert!(ModuleFormat::from_path(Path::new(path)).is_err());
        }
    }
}
