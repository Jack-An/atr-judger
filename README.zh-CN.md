# atr-judger

[English](README.md) · **简体中文**

atr-judger 是独立的 Rust 判题服务，在 Linux 原生沙箱中编译和执行提交程序。任何在线判题平台都可以调用其带版本的 HTTP API。服务无需数据库或消息队列。

- [HTTP API](docs/api.zh-CN.md)
- [扩展语言与库](docs/languages.zh-CN.md)

## 快速开始

部署到专用 Linux 主机或虚拟机，内核须为 5.19 或更高版本，启用 cgroup v2，并委派 `cpu`、`memory` 和 `pids` 控制器。沙箱监督进程需要配置命名空间和 cgroup 的权限。请将执行器与数据库和敏感卷分开部署。远程调用经私有网络上的 TLS 反向代理接入。

进入 atr-judger 项目目录，构建镜像并启动服务。

```sh
docker build -t atr-judger .
export RUST_JUDGER_TOKEN="$(openssl rand -hex 32)"
docker run --rm --name atr-judger --privileged --cgroupns=private \
  -p 127.0.0.1:5060:5060 -e RUST_JUDGER_TOKEN atr-judger
```

在另一个终端中使用相同的令牌，检查服务是否就绪。

```sh
curl -fsS http://127.0.0.1:5060/readyz \
  -H "Authorization: Bearer ${RUST_JUDGER_TOKEN}"
```

## 配置

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `RUST_JUDGER_TOKEN` | 必填 | Bearer 令牌，长度为 32–256 个可见 ASCII 字符 |
| `RUST_JUDGER_BIND` | `127.0.0.1:5060` | HTTP 监听地址。容器镜像设为 `0.0.0.0:5060`。 |
| `RUST_JUDGER_CGROUP_ROOT` | 必填 | 独占且已委派的 cgroup v2 域。容器镜像使用私有 cgroup 命名空间中的 `/sys/fs/cgroup`。 |
| `RUST_JUDGER_RUNTIME_ROOT` | `/` | 可信运行时目录。沙箱只读挂载目录下的 `usr` 和可选的 `etc/java-21-openjdk`。 |
| `RUST_JUDGER_CONCURRENCY` | `4` | 同时执行的判题请求数，范围为 1–64 |
| `RUST_JUDGER_LANGUAGES` | 未设置 | 用于添加或覆盖语言配方的可信 JSON 文件 |

服务在验证 cgroup 委派并通过沙箱启动检查后才会打开 HTTP 端口。在 Linux 上从源码构建时，先安装 Rust 1.91 或更高版本以及 libseccomp 开发包，然后执行 `cargo build --locked --release`。生成的可执行文件为 `target/release/atr-judger`。直接启动时，还需安装[内置语言配方](src/judge/languages.json)所引用的工具链。

## 架构

| 模块 | 职责 |
| --- | --- |
| `src/api/` | HTTP 合约、认证和并发控制 |
| `src/judge/` | 语言目录、编译、用例执行和输出比较 |
| `src/sandbox/` | 进程监督、命名空间、cgroup v2 计量和 seccomp |

每次编译和每个用例均以 UID 和 GID 65534 运行，使用独立的 `tmpfs`、只读工具链和受限的 `/proc`。seccomp 策略禁止创建套接字，监督进程在阶段结束后回收子进程。[Linux namespaces](https://man7.org/linux/man-pages/man7/namespaces.7.html) · [cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) · [libseccomp](https://github.com/seccomp/libseccomp)

## 验证

使用相同的 Bearer 令牌，对运行中的服务执行冒烟检查。默认访问 `http://127.0.0.1:5060`，可通过 `RUST_JUDGER_URL` 指定其他地址。

```sh
python3 scripts/smoke.py
```
