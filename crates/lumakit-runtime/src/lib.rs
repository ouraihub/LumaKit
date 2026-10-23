//! Cancellation and shutdown primitives shared by desktop hosts.
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("task join failed")]
    Join,
}
#[derive(Clone)]
pub struct Runtime {
    root: CancellationToken,
}
impl Runtime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: CancellationToken::new(),
        }
    }
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.root.child_token()
    }
    pub fn shutdown(&self) {
        self.root.cancel();
    }
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.root.is_cancelled()
    }
    pub async fn run<F>(&self, task: F) -> Result<(), RuntimeError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let token = self.token();
        tokio::select! { _ = task => Ok(()), _ = token.cancelled() => Ok(()) }
    }
}
impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
#[allow(dead_code)]
fn _arc_marker<T>(value: Arc<T>) -> Arc<T> {
    value
}
