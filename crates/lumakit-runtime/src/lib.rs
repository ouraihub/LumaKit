//! Cancellation and shutdown primitives shared by desktop hosts.
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// 后台任务执行错误；当前 [`Runtime::run`] 不创建 JoinHandle，也不产生该错误。
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// 任务句柄等待失败；当前执行路径未使用此变体。
    #[error("task join failed")]
    Join,
}
/// 共享根取消令牌；克隆实例共享关闭状态，不自行创建线程或 Tokio 运行时。
#[derive(Clone)]
pub struct Runtime {
    root: CancellationToken,
}
impl Runtime {
    /// 创建未取消的任务作用域；构造本身不启动任何任务。
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: CancellationToken::new(),
        }
    }
    /// 创建子令牌；父作用域关闭会取消它，取消子令牌不会关闭父作用域。
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.root.child_token()
    }
    /// 请求整个作用域取消；重复调用无副作用，不等待任务退出。
    pub fn shutdown(&self) {
        self.root.cancel();
    }
    /// 返回是否已请求关闭，不表示所有任务已完成清理。
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.root.is_cancelled()
    }
    /// 在当前执行器中等待任务完成或作用域取消，不单独 spawn。
    ///
    /// 取消分支获胜时直接丢弃任务 Future；异步清理必须由调用方安排。
    ///
    /// # Errors
    /// 当前实现对正常完成和取消均返回 `Ok(())`，不会生成 [`RuntimeError`]。
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
