# HTTP API v1

**English** · [简体中文](api.zh-CN.md) · [Project home](../README.md)

Every route requires `Authorization: Bearer <token>`. POST requests use `Content-Type: application/json`. The judging routes return results synchronously.

| Route | Response |
| --- | --- |
| `GET /readyz` | Service version and sandbox type, for example `{"version":"0.1.0","sandbox":"linux-cgroup-v2"}` |
| `GET /v1/languages` | Array of entries such as `{"id":"python","compiled":false,"libraries":["Python standard library"]}` |
| `POST /v1/judge` | Result for one test case |
| `POST /v1/judge/batch` | Results in request order, with one compilation shared across the batch |

## Single submission

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

`stdin`, `expectedOutput`, and `limits` are optional. With an expected output, the engine compares the strings after trimming leading and trailing whitespace. Without an expected output, a successful execution returns `Accepted`. The `time` value is CPU time in nanoseconds. The `memory` value is the cgroup peak in bytes, including sandbox overhead. The output fields contain UTF-8 text with replacement characters for invalid bytes.

## Batch submission

The request to `POST /v1/judge/batch` supplies shared source code, language, and optional limits.

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

Each case runs in a fresh sandbox. On successful compilation, the response has `"compilation": null` and an ordered `results` array. On compilation failure, `compilation` contains the error result and `results` is empty.

## Concurrency and task retention

`RUST_JUDGER_CONCURRENCY` sets the number of request slots per instance. The default is four. A single-case request uses one slot. A batch also uses one slot and runs its cases sequentially after compilation. When all slots are occupied, the service returns `429` before executing the submission. Callers can retry a `429` with backoff.

An admitted execution holds its slot through cleanup, even if the HTTP client disconnects. The service keeps no persistent task IDs or results. If the service restarts before returning the result, the caller must submit the request again. Use an external queue when submissions must survive restarts or wait for capacity.

## Limits and status codes

| Field | Allowed range | Default |
| --- | --- | --- |
| `limits.cpuMs` | 1–65,000 ms | 2,000 ms |
| `limits.wallMs` | 1–70,000 ms | 5,000 ms |
| `limits.memoryKb` | 2,048–1,048,576 KiB | 262,144 KiB |
| `limits.outputBytes` | 1–262,144 bytes per stream | 65,536 bytes |

The runtime may add memory headroom specified by its language recipe. Each `expectedOutput` may use up to `outputBytes` bytes. Source code is limited to 256 KiB, combined standard input to 1 MiB per request, and the HTTP request body to 8 MiB. A batch accepts 1–16 cases with at most 120,000 ms of combined wall-time budget and 1 MiB of combined output budget per stream. Compilation has a separate 30-second wall-time budget.

Execution results use `Accepted`, `Wrong Answer`, `Compilation Error`, `Time Limit Exceeded`, `Memory Limit Exceeded`, `Output Limit Exceeded`, or `Nonzero Exit Status`.

| HTTP status | Meaning |
| --- | --- |
| `400` | Malformed JSON |
| `401` | Missing or invalid token |
| `408` | Request body read timed out |
| `413` | Request body exceeds the size limit |
| `415` | JSON content type required |
| `422` | Invalid fields, language ID, or limits |
| `429` | All execution slots were occupied before the request could start |
| `500` | Executor failure |
