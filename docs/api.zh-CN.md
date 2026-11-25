# HTTP API v1

[English](api.md) · **简体中文** · [项目首页](../README.zh-CN.md)

所有接口均需要 `Authorization: Bearer <token>`。POST 请求使用 `Content-Type: application/json`。判题接口同步返回结果。

| 接口 | 返回值 |
| --- | --- |
| `GET /readyz` | 服务版本和沙箱类型，例如 `{"version":"0.1.0","sandbox":"linux-cgroup-v2"}` |
| `GET /v1/languages` | 语言列表，条目示例为 `{"id":"python","compiled":false,"libraries":["Python standard library"]}` |
| `POST /v1/judge` | 单个用例的判题结果 |
| `POST /v1/judge/batch` | 按请求顺序返回用例结果，所有用例共享一次编译 |

## 单次判题

```sh
curl -fsS http://127.0.0.1:5060/v1/judge \
  -H "Authorization: Bearer ${RUST_JUDGER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -d '{"language":"python","sourceCode":"print(int(input()) * 2)","stdin":"21","expectedOutput":"42","limits":{"cpuMs":1000,"wallMs":2000,"memoryKb":32768}}'
```

```json
{
  "status": "Accepted",
  "time": 1254000,
  "memory": 4751360,
  "files": { "stdout": "42\n", "stderr": "" }
}
```

`stdin`、`expectedOutput` 和 `limits` 均可省略。提供预期输出时，引擎会去掉两端字符串的首尾空白再比较。省略预期输出时，程序执行成功即返回 `Accepted`。`time` 是以纳秒计量的 CPU 时间。`memory` 是以字节计量的 cgroup 内存峰值，包含沙箱开销。输出字段为 UTF-8 文本，无效字节用替换字符表示。

## 批量判题

向 `POST /v1/judge/batch` 提交公用的源码、语言和可选资源限制。

```json
{
  "language": "python",
  "sourceCode": "print(int(input()) * 2)",
  "cases": [
    { "stdin": "21", "expectedOutput": "42" },
    { "stdin": "5", "expectedOutput": "10" }
  ]
}
```

每个用例都在全新的沙箱中运行。编译成功时，响应中的 `compilation` 为 `null`，`results` 按请求顺序排列。编译失败时，`compilation` 包含错误结果，`results` 为空。

## 并发和任务留存

`RUST_JUDGER_CONCURRENCY` 设置每个实例的请求槽位数，默认 4 个。单次请求占 1 个槽位，批量请求也占 1 个槽位，编译后依次运行各用例。槽位全部占用时，服务在执行前返回 `429`。调用方可在收到 `429` 后退避重试。

已开始的执行会占用槽位直到清理完成，即使 HTTP 客户端中途断开。服务不持久化任务 ID 或结果。若服务在返回结果前重启，调用方需要重新提交。需要任务跨重启保留或等待执行容量时，可由调用端接入外部队列。

## 限制和状态码

| 字段 | 允许范围 | 默认值 |
| --- | --- | --- |
| `limits.cpuMs` | 1–65,000 毫秒 | 2,000 毫秒 |
| `limits.wallMs` | 1–70,000 毫秒 | 5,000 毫秒 |
| `limits.memoryKb` | 2,048–1,048,576 KiB | 262,144 KiB |
| `limits.outputBytes` | 每个输出流 1–262,144 字节 | 65,536 字节 |

运行时可按语言配方增加内存余量。单个 `expectedOutput` 最多使用 `outputBytes` 字节。每个请求的源码上限为 256 KiB，标准输入合计上限为 1 MiB，HTTP 请求体上限为 8 MiB。每批可有 1–16 个用例，实际耗时预算合计最多 120,000 毫秒，每个输出流的预算合计最多 1 MiB。编译有独立的 30 秒实际耗时预算。

判题结果包括 `Accepted`、`Wrong Answer`、`Compilation Error`、`Time Limit Exceeded`、`Memory Limit Exceeded`、`Output Limit Exceeded` 和 `Nonzero Exit Status`。

| HTTP 状态码 | 含义 |
| --- | --- |
| `400` | JSON 格式错误 |
| `401` | 令牌缺失或无效 |
| `408` | 读取请求体超时 |
| `413` | 请求体超过大小限制 |
| `415` | 请求须声明 JSON 内容类型 |
| `422` | 字段、语言 ID 或资源限制无效 |
| `429` | 请求开始执行前，所有执行名额均已占用 |
| `500` | 执行器故障 |
