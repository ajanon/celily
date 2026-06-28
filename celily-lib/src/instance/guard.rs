use std::sync::Arc;

use tracing::{debug, error, info};

use crate::backend::InstanceBackend;

/// RAII guard that deletes the instance on drop unless `keep` is set.
///
/// Created immediately after `backend.create()` succeeds inside
/// `Instance::init()` -- any failure after that unwinds through this
/// guard and cleans up the instance. When `init()` completes, the
/// guard is moved into the `Initialized` state where it stays for
/// the remainder of the instance lifecycle.
pub struct InstanceGuard<B: InstanceBackend> {
    name: String,
    backend: Arc<B>,
    keep: bool,
}

impl<B: InstanceBackend> InstanceGuard<B> {
    pub fn new(name: String, backend: Arc<B>, keep: bool) -> Self {
        Self {
            name,
            backend,
            keep,
        }
    }
}

impl<B: InstanceBackend> Drop for InstanceGuard<B> {
    fn drop(&mut self) {
        if self.keep {
            info!(name = %self.name, "keeping instance");
            return;
        }
        info!(name = %self.name, "destroying instance");
        match self.backend.delete(&self.name) {
            Ok(()) => debug!(name = %self.name, "instance destroyed"),
            Err(e) => error!(name = %self.name, error = %e, "failed to delete instance"),
        }
    }
}
