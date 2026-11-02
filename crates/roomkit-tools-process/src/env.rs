//! The environment commands receive, chosen by the host.

use std::collections::BTreeMap;

use roomkit_tools_core::ToolError;

/// Host variables passed to commands by default, when the host has them set.
pub const DEFAULT_INHERITED_ENV: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR",
];

/// Host configuration of the environment a command starts with.
///
/// A command receives only the variables named in `inherit_env` that are set
/// in the host when it starts, then the explicit `env` values, which replace
/// inherited ones of the same name. No other host variable reaches it, so
/// credentials in the host's environment stay out of commands unless listed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandEnv {
    /// Names of host variables to pass through when set.
    /// Default: [`DEFAULT_INHERITED_ENV`].
    pub inherit_env: Vec<String>,
    /// Explicit values, applied after inheritance. Default: none.
    pub env: BTreeMap<String, String>,
}

impl Default for CommandEnv {
    fn default() -> Self {
        Self {
            inherit_env: DEFAULT_INHERITED_ENV
                .iter()
                .map(|&name| name.to_owned())
                .collect(),
            env: BTreeMap::new(),
        }
    }
}

impl CommandEnv {
    /// Check that every name and value can be passed to a process.
    ///
    /// # Errors
    /// Rejects an empty name, a name containing `=` or NUL, and a value
    /// containing NUL.
    pub fn validate(&self) -> Result<(), ToolError> {
        let mut names = self.inherit_env.iter().chain(self.env.keys());
        if names.any(|name| name.is_empty() || name.contains(['=', '\0'])) {
            return Err(ToolError::InvalidInput(
                "environment names must be nonempty, without '=' or NUL".into(),
            ));
        }
        if self.env.values().any(|value| value.contains('\0')) {
            return Err(ToolError::InvalidInput(
                "environment values must not contain NUL".into(),
            ));
        }
        Ok(())
    }

    /// Replace the command's environment with the configured one, reading the
    /// listed host variables now.
    #[cfg(unix)]
    pub(crate) fn apply(&self, command: &mut tokio::process::Command) {
        command.env_clear();
        for name in &self.inherit_env {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(&self.env);
    }
}
