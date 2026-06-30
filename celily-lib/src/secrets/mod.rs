mod rbw;

use crate::command::CommandError;

/// Errors that can occur when interacting with a [`SecretProvider`].
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// An `auth.secret` references a named secret but no provider is
    /// configured.
    #[error(
        "auth.secret '{secret}' requires a secret provider, but none is configured (set \
         secret_provider in your config)"
    )]
    NoProvider { secret: String },
    /// The `rbw` backend returned an error.
    #[error("rbw: {0}")]
    Rbw(#[from] CommandError),
}

/// A backend that can resolve named secret items to their values.
pub trait SecretProvider: Send + Sync {
    /// The error type returned by provider operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Human-readable name, matches the config value (e.g. `"rbw"`).
    fn name(&self) -> &'static str;

    /// Verify the provider is usable (vault unlocked, CLI installed, etc.).
    fn check_available(&self) -> Result<(), Self::Error>;

    /// Resolve a named item to its secret value.
    fn resolve(&self, item: &str) -> Result<String, Self::Error>;
}

/// Available secret providers.
///
/// Parsed from the `secret_provider` config key. Call
/// [`resolve`](Self::resolve) to obtain a [`SecretProvider`] trait
/// object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Providers {
    /// [Bitwarden](https://bitwarden.com/) via the `rbw` CLI.
    Rbw,
}

impl Providers {
    /// Convert this provider to a boxed [`SecretProvider`] trait object.
    pub fn resolve(self) -> Box<dyn SecretProvider<Error = SecretError>> {
        match self {
            Self::Rbw => Box::new(rbw::RbwProvider),
        }
    }
}
