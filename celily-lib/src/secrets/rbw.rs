use std::process::Command;

use super::{SecretError, SecretProvider};
use crate::command::CommandExt;

pub(crate) struct RbwProvider;

impl SecretProvider for RbwProvider {
    type Error = SecretError;

    fn name(&self) -> &'static str {
        "rbw"
    }

    fn check_available(&self) -> Result<(), SecretError> {
        Command::new("rbw").arg("unlocked").run()?;
        Ok(())
    }

    fn resolve(&self, item: &str) -> Result<String, SecretError> {
        let output = Command::new("rbw").args(["get", item]).run_output()?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }
}
