# Add languages and libraries

**English** · [简体中文](languages.zh-CN.md) · [Project home](../README.md)

The default image supports C, C++, Python, JavaScript, TypeScript, and Java. The built-in recipes show the compiler commands, runtime commands, and standard libraries for each language. [Built-in recipes](../src/judge/languages.json)

## Register a language

Install the toolchain and dependencies under `/usr` in a custom runtime image. Set `RUST_JUDGER_LANGUAGES` to the path of a trusted manifest. The manifest adds or replaces recipes by language ID. Restart the service, then query `GET /v1/languages` to confirm the new ID and library metadata.

For example, install Ruby in the image and add this entry to the manifest.

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

## Recipe fields

| Field | Purpose |
| --- | --- |
| `source` | Filename used to stage the submitted source |
| `run` | Executable and arguments for a test case. The executable lives under `/usr`, or is the compiled artifact at `/work/<artifact>`. |
| `compile` | Optional object with `argv` and `artifact`. The compiler lives under `/usr`. The artifact is a single file transferred into each case sandbox. |
| `processes` | Process and thread limit for each case, from 4 to 128 |
| `memoryOverheadKb` | Optional fixed memory allowance for runtime overhead, up to 262,144 KiB |
| `libraries` | Optional library names shown by the language discovery API |
| `environment` | Optional environment variables visible to submitted programs. Keep credentials out of this object. |

## Package libraries

Install approved libraries when building the runtime image. Python can use distribution packages or a preinstalled virtual environment under `/usr`. The Node.js recipes set `NODE_PATH` so CommonJS submissions can load packages installed at `/usr/local/node_modules`. Library metadata is descriptive. Submitted code and libraries run under the same filesystem, network, and resource limits.
