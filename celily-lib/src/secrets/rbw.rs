use async_trait::async_trait;

use super::{SecretError, SecretProvider};
use crate::command::AsyncCommandExt;

pub(crate) struct RbwProvider;

#[async_trait]
impl SecretProvider for RbwProvider {
    type Error = SecretError;

    fn name(&self) -> &'static str {
        "rbw"
    }

    async fn check_available(&self) -> Result<(), SecretError> {
        tokio::process::Command::new("rbw")
            .arg("unlocked")
            .run()
            .await?;
        Ok(())
    }

    async fn resolve(&self, item: &str) -> Result<String, SecretError> {
        let output = tokio::process::Command::new("rbw")
            .args(["get", item])
            .run_output()
            .await?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }
}
