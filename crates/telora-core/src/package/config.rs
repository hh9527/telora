use super::*;

impl WorkspaceConfig {
    pub(super) fn read(path: &Path) -> Result<Self, PackageError> {
        let config: Self = read_json(path)?;
        let invalid = |message| PackageError::new(format!("{}: {message}", path.display()));
        config.compiler.validate().map_err(invalid)?;
        config.runtime.limits().map_err(invalid)?;
        if config.version != 1 {
            return Err(invalid(format!(
                "unsupported version {}; expected 1",
                config.version
            )));
        }
        Ok(config)
    }

    /// Builtin-only commands need workspace options but not a package lock or
    /// dependency acquisition. Absence is allowed; malformed config is not.
    pub fn discover_optional(start: &Path) -> Result<Option<Self>, PackageError> {
        let start = absolute(start)?;
        let start = fs::canonicalize(&start).map_err(|error| {
            PackageError::new(format!(
                "cannot resolve config context {}: {error}",
                start.display()
            ))
        })?;
        let directory = if start.is_file() {
            start.parent().unwrap_or(&start)
        } else {
            &start
        };
        directory
            .ancestors()
            .map(|path| path.join(CONFIG_FILE))
            .find(|path| path.is_file())
            .map(|path| Self::read(&path))
            .transpose()
    }
}
