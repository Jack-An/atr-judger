# 扩展语言与库

[English](languages.md) · **简体中文** · [项目首页](../README.zh-CN.md)

默认镜像支持 C、C++、Python、JavaScript、TypeScript 和 Java。各语言的编译命令、执行命令和标准库可在内置配方中查看。[内置配方](../src/judge/languages.json)

## 注册语言

将工具链及依赖安装到自定义运行镜像的 `/usr`，并将 `RUST_JUDGER_LANGUAGES` 指向可信的配方文件。文件可按语言 ID 添加或覆盖配方。重启服务后，调用 `GET /v1/languages` 验证新语言 ID 和库信息。

例如，在镜像中安装 Ruby 后，把以下条目写入配方文件。

```json
{
  "ruby": {
    "source": "main.rb",
    "run": ["/usr/bin/ruby", "main.rb"],
    "processes": 16,
    "libraries": ["json"]
  }
}
```

## 配方字段

| 字段 | 用途 |
| --- | --- |
| `source` | 用于存放提交源码的文件名 |
| `run` | 单个用例的可执行文件和参数。可执行文件位于 `/usr`，也可以是 `/work/<artifact>` 下的编译产物。 |
| `compile` | 可选对象，包含 `argv` 和 `artifact`。编译器位于 `/usr`，编译产物为传入每个用例沙箱的单个文件。 |
| `processes` | 每个用例的进程和线程上限，范围为 4–128 |
| `memoryOverheadKb` | 可选的运行时固定内存余量，上限为 262,144 KiB |
| `libraries` | 可选的库名称列表，用于语言查询接口展示 |
| `environment` | 可选的环境变量，提交程序可读取。请勿在其中存放凭据。 |

## 打包依赖库

构建运行镜像时预装受信任的库。Python 可以使用发行版软件包或预装在 `/usr` 的虚拟环境。Node.js 配方设置了 `NODE_PATH`，CommonJS 程序可加载安装在 `/usr/local/node_modules` 的包。库信息用于展示。第三方库和提交程序共用相同的文件系统、网络和资源限制。
