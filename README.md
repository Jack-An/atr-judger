# atr-judger

**English** · [简体中文](README.zh-CN.md)

atr-judger is a standalone Rust service that compiles and runs submissions in native Linux sandboxes. Any online judge can call its versioned HTTP API. The service has no database or queue dependency.

- [HTTP API](docs/api.md)
- [Add languages and libraries](docs/languages.md)

## Quick start

Run on a dedicated Linux 5.19+ host or VM with cgroup v2 and delegated `cpu`, `memory`, and `pids` controllers. The sandbox supervisor needs privileges to set up namespaces and cgroups. Keep this host separate from your database and sensitive volumes. For remote clients, use a TLS proxy on a private network.

From the atr-judger project directory, build the image and run the service.

```sh
docker build -t atr-judger .
export RUST_JUDGER_TOKEN="$(openssl rand -hex 32)"
docker run --rm --name atr-judger --privileged --cgroupns=private \
  -p 127.0.0.1:5060:5060 -e RUST_JUDGER_TOKEN atr-judger
```

In a second shell with the same token, check that the service is ready.

```sh
curl -fsS http://127.0.0.1:5060/readyz \
  -H "Authorization: Bearer ${RUST_JUDGER_TOKEN}"
```

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUST_JUDGER_TOKEN` | Required | Bearer token with 32–256 visible ASCII characters |
| `RUST_JUDGER_BIND` | `127.0.0.1:5060` | HTTP listen address. The image sets `0.0.0.0:5060`. |
| `RUST_JUDGER_CGROUP_ROOT` | Required | Dedicated, delegated cgroup v2 domain. The image uses `/sys/fs/cgroup` inside its private cgroup namespace. |
| `RUST_JUDGER_RUNTIME_ROOT` | `/` | Trusted runtime tree. The sandbox mounts its `usr` and optional `etc/java-21-openjdk` read-only. |
| `RUST_JUDGER_CONCURRENCY` | `4` | Concurrent judging requests, from 1 to 64 |
| `RUST_JUDGER_LANGUAGES` | Unset | Trusted JSON file that adds or overrides language recipes |

The service opens its HTTP port after validating delegation and passing a sandbox startup check. To build it from source on Linux, install Rust 1.91+ and the libseccomp development package, then run `cargo build --locked --release`. The executable is `target/release/atr-judger`. Running it directly also requires the toolchains in the [built-in language recipes](src/judge/languages.json).

## Architecture

| Module | Responsibility |
| --- | --- |
| `src/api/` | HTTP contract, authentication, and concurrency admission |
| `src/judge/` | Language catalog, compilation, case execution, and output comparison |
| `src/sandbox/` | Process supervision, namespaces, cgroup v2 accounting, and seccomp |

Each compilation and test case runs as UID and GID 65534 with a fresh `tmpfs`, read-only toolchains, and a restricted `/proc`. The seccomp policy blocks sockets, and the supervisor reaps descendants between stages. [Linux namespaces](https://man7.org/linux/man-pages/man7/namespaces.7.html) · [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) · [libseccomp](https://github.com/seccomp/libseccomp)

## Verification

Run the smoke checks against a running service with the same Bearer token. Set `RUST_JUDGER_URL` to use an address other than `http://127.0.0.1:5060`.

```sh
python3 scripts/smoke.py
```
