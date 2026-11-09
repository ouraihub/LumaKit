use std::{
    fmt, process,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

use crate::operation::{OperationGuard, start};

/// 一次顶层操作及其全部子操作共享的诊断上下文；克隆只复制引用计数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: Arc<str>,
}

impl TraceContext {
    /// 开始一个顶层操作：生成新的 TraceId，并以 INFO 级别写出 `EV:ENTRY`。
    ///
    /// 只在真实入口（UI 动作、AI 请求、后台任务、工具调用）调用；下层用 [`crate::operation`]。
    pub fn root(operation: &'static str, layer: &'static str) -> (Self, OperationGuard) {
        let trace = Self {
            trace_id: next_trace_id().into(),
        };
        let guard = start(&trace, operation, layer, true);
        (trace, guard)
    }

    /// 返回用于检索完整调用链的 32 位小写十六进制 TraceId。
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }
}

impl fmt::Display for TraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.trace_id)
    }
}

static STARTED: LazyLock<u128> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
});
static NEXT: AtomicU64 = AtomicU64::new(0);

/// 进程标识、进程内首次取号时间与递增计数的 SHA-256 前 16 字节。
/// 进程内由计数保证唯一，跨进程由 pid 与启动时间区分；没有失败路径。
fn next_trace_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(process::id().to_le_bytes());
    hasher.update(STARTED.to_le_bytes());
    hasher.update(NEXT.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let digest = hasher.finalize();
    let mut id = String::with_capacity(32);
    for byte in &digest[..16] {
        id.push(char::from(HEX[usize::from(byte >> 4)]));
        id.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    id
}
