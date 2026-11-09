# 可追踪分层日志需求模板

> 用途：把这份文档交给大模型，要求它为另一个 Rust、C#、Java、Go 或 TypeScript 项目设计/改造可检索的分层日志链路。
>
> 目标不是“多打印日志”，而是让一次请求或用户操作可以通过一个 Trace ID 还原完整调用链。

## 给大模型的任务描述

请为当前项目设计并实现一套**可按 Trace ID 检索的分层结构化日志系统**。

系统需要满足以下目标：

```text
一次用户请求 / 一次命令 / 一次后台任务 / 一次消息处理
    └─ trace_id
        ├─ 入口层（HTTP Controller / RPC Handler / UI Action / CLI Command）
        ├─ Service / Use Case / Application 层
        ├─ Repository / DAO / Database / File / Git 层
        ├─ 外部 HTTP / RPC / Agent / Process 调用
        ├─ 完成结果与耗时
        └─ 异常、取消、超时和资源清理
```

当用户拿到一个 `trace_id` 后，应该可以在日志中搜索到这次操作从入口到出口的全部相关记录，并能看出：

- 调用了哪些层；
- 每层调用了哪个方法；
- 每层开始和结束的时间；
- 每层耗时；
- 最终成功、失败、取消还是超时；
- 错误发生在哪一层；
- 子调用与父调用的关系。

不要只在入口打印一条 Trace ID，也不要只靠手工字符串拼接。必须设计上下文传播和结构化字段。

## 先调研，再修改

在修改代码前，必须先阅读并报告：

1. 当前日志初始化位置；
2. 当前日志级别和过滤器实现；
3. 当前请求/命令/后台任务入口；
4. Controller/Handler → Service → Repository/DAO 的调用链；
5. async task、thread、channel、worker 和外部进程边界；
6. 当前错误处理和取消/超时路径；
7. 当前日志文件、轮转、tail、查看页面和持久化设置；
8. 项目已有的 tracing/logging/observability crate 和可复用 helper；
9. 所有需要改动的公开接口和调用方；
10. 现有测试及实际验证命令。

不得猜测未读取的 API、文件或项目规范。应优先复用现有日志系统，不要建立第二套平行日志框架。

## 日志级别契约

日志级别应采用标准阈值语义：选择某级别时，记录该级别及更严重事件。

```text
ERROR：系统异常、未处理异常、进程失败、数据损坏
WARN ：取消、超时、业务拒绝、可恢复异常
INFO ：正常启动、正常完成、关键生命周期状态
DEBUG：方法入口、方法出口、耗时、关键输入/输出摘要
TRACE：协议帧、底层通信、事件归约和极细节诊断
```

默认级别为 `INFO`。

项目应支持：

- 运行时切换日志级别；
- 将用户选择持久化；
- 程序重启后恢复上次选择；
- 环境变量或命令行参数临时覆盖持久化设置；
- `DEBUG` 模式显示方法入口/出口；
- `TRACE` 模式显示底层协议细节。

重要限制：日志级别只能控制未来产生的日志。过去在 `INFO` 模式下被过滤掉的 `DEBUG` 日志不能在重启后恢复。

## TraceContext 契约

实现一个可跨线程和异步任务传播的 TraceContext。不要使用全局可变的“当前 Trace ID”来隐式关联业务。

建议接口：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
}

impl TraceContext {
    pub fn root(operation: &'static str) -> (Self, tracing::Span);
    pub fn child(&self, operation: &'static str) -> tracing::Span;
}
```

如果项目不是 Rust，应使用该语言等价的不可变/可复制上下文对象。

契约：

- 顶层操作创建 root trace；
- 子层创建 child span；
- `trace_id` 在整个调用链保持不变；
- `span_id` 和 `parent_span_id` 表示局部层级；
- 跨 thread、async task、channel、worker、队列和外部进程边界时显式传递；
- 不能依赖线程本地变量猜测父级关系；
- 一个用户会话 ID、聊天 ID 或请求参数 ID 不能替代 Trace ID；
- 不同请求之间绝不能串用同一个 Trace ID。

## 统一结构化字段

所有边界日志尽量使用结构化字段，不要把所有字段拼进不可解析的大字符串。

至少包含：

```text
trace_id
span_id
parent_span_id
operation
component
layer
event
result
duration_ms
error_code
```

有业务上下文时追加：

```text
request_kind
workspace_id
chat_id
user_id（仅在安全且确有必要时）
backend
provider
model
item_id
```

字段名应稳定、简洁、可搜索。推荐使用 snake_case。

## 方法入口/出口日志

每个有副作用、跨边界、可能失败或可能耗时的公共方法，必须有统一生命周期日志。

### 入口

```text
event=entry
operation=<stable.operation.name>
layer=<controller|service|repository|external>
trace_id=<...>
```

记录必要的输入摘要，但不能记录敏感信息或无限长度内容。

### 成功出口

```text
event=exit
result=ok
duration_ms=<...>
```

记录有限的结果摘要。不要默认序列化完整响应、完整数据库对象或完整文件内容。

### 失败出口

```text
event=error
error_code=<stable code>
error_type=<type>
duration_ms=<...>
```

记录异常堆栈或脱敏摘要，保留原始错误用于上层处理。

### 取消和超时

取消和超时必须有独立终态：

```text
event=cancel
reason=<...>
duration_ms=<...>
```

```text
event=timeout
duration_ms=<...>
timeout_ms=<...>
```

正常用户取消不应伪装成系统 ERROR；如果取消本身导致资源清理失败，再单独记录 ERROR。

## 层级边界

按项目实际架构映射，不要强行制造不存在的层。

### Web/API 项目

```text
HTTP middleware
  → Controller
  → Application Service
  → Domain Service
  → Repository / DAO
  → Database / External API
```

### 桌面应用

```text
UI action
  → application service / command handler
  → background job
  → storage / network / process / engine
```

### AI/Agent 应用

```text
chat action / tool call
  → routing
  → engine session
  → external process / HTTP provider
  → event stream
  → persistence / UI completion
```

每层只记录自己的边界，不在每一行重复打印完整上下文。

## 外部系统边界

对于 HTTP、RPC、数据库、Git、子进程或 Agent engine：

必须记录：

- 开始；
- 目标类别，而不是未经处理的完整 URL/命令；
- 允许记录的 request id；
- 结束状态；
- 状态码/退出码；
- 耗时；
- 超时、取消和错误。

禁止记录：

- API key；
- Authorization；
- Cookie；
- 密码；
- 完整 prompt；
- 完整文件内容；
- Base64 附件；
- 包含 query secret 的原始 URL；
- 未限制大小的响应正文。

## 线程和异步传播要求

以下代码模式必须明确传播 TraceContext：

```text
spawn / thread::spawn
background executor
async task
channel send/receive
worker queue
callback
external process reader
```

示例：

```rust
let child_trace = trace.clone();
std::thread::spawn(move || {
    let _span = child_trace.child("repository.query").entered();
    // 这里的日志必须带原 trace_id
});
```

不要只在入口函数创建 span，然后把工作闭包脱离上下文丢给线程。

## 日志查看和用户诊断

产品层应提供：

- 按 `trace_id` 搜索；
- 按 `operation`、`layer`、`component` 筛选；
- 复制 Trace ID；
- 错误提示中显示可复制的诊断 ID；
- 查看同一 trace 的时间顺序和耗时；
- 默认只显示 INFO/WARN/ERROR，DEBUG/TRACE 可展开或通过设置开启。

日志文本输出必须 grep 友好；未来增加 JSON sink 时不应改变字段契约。

## 多项目复用边界

公共 observability crate 只提供：

```text
LogLevel
LogHandle
TraceContext
OperationGuard
structured field conventions
redaction helpers
file writer / rotation / tail helpers
```

公共 crate 不提供：

```text
AppPrefs
数据库 schema
UI 页面
产品错误文案
业务实体
Controller/Service/DAO 接口
具体 chat/workspace/review 字段
```

业务项目通过结构化字段追加自己的上下文，但不修改公共字段语义。

## 实施顺序

1. 调研项目现有日志与调用边界；
2. 不改变现有日志输出路径，先加入 TraceContext；
3. 让顶层请求/操作创建 root trace；
4. 改造后台任务、线程和 channel 的上下文传播；
5. 给入口、Service/Application、Repository/DAO、外部系统增加 entry/exit/error；
6. 增加日志级别持久化和重启恢复；
7. 增加 Trace ID 搜索和复制；
8. 添加并发隔离、失败、取消、超时和脱敏测试；
9. 再考虑 OpenTelemetry/OTLP 等远端后端，不要第一阶段提前引入。

## 验收标准

实现完成前必须证明：

- 一个正常请求能按 Trace ID 找到完整链路；
- 一个失败请求能定位失败层级和方法；
- 方法入口、出口和耗时在 DEBUG 级别出现；
- INFO 级别不会被普通方法入口日志淹没；
- Error/Warn/Info/Debug/Trace 过滤行为正确；
- 持久化级别重启后恢复；
- 运行时切换不需要重启，若产品选择重启式 UX 也必须说明；
- 跨线程、异步 task、channel 后 Trace ID 不丢失；
- 并发请求不会串 Trace ID；
- 取消和超时有终态日志；
- API key、token、密码、完整 prompt 和文件正文不出现在日志；
- 日志查看器能按 Trace ID 搜索；
- 测试覆盖成功、失败、取消、超时、并发和 cleanup。

## 给大模型的交付要求

请先输出：

1. 当前项目日志链路调查；
2. 当前日志缺口；
3. 需要改动的模块和所有调用方；
4. TraceContext 的传播边界；
5. 日志级别与持久化方案；
6. 验证计划。

然后再修改代码。交付时必须列出：

- 修改文件；
- 公共 API 变化；
- 日志字段契约；
- 脱敏策略；
- 实际运行过的检查命令及结果；
- 尚未验证的外部依赖或真实环境。

不要把“函数被调用”或“日志字符串存在”当作链路验证；必须运行真实操作并根据 Trace ID 观察入口、出口和错误记录。
