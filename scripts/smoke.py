"""Run contract and isolation checks against a local native judging service."""
import json
import os
import urllib.error
import urllib.request


URL = os.environ.get("RUST_JUDGER_URL", "http://127.0.0.1:5060")
TOKEN = os.environ["RUST_JUDGER_TOKEN"]


def call(path, payload=None, *, token=TOKEN, expected_code=200):
    headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(URL + path, data, headers)
    try:
        with urllib.request.urlopen(request, timeout=165) as response:
            assert response.status == expected_code, response.status
            return json.load(response)
    except urllib.error.HTTPError as error:
        assert error.code == expected_code, (error.code, error.read())
        return None


def execute(source, status="Accepted", *, language="python", stdin="", **limits):
    result = call("/v1/judge", {
        "language": language,
        "sourceCode": source,
        "stdin": stdin,
        "limits": limits,
    })
    assert result["status"] == status, (language, status, result)
    assert result["time"] > 0, result
    assert result["memory"] > 0, result
    return result["files"]["stdout"]


def main():
    call("/readyz", token="invalid", expected_code=401)
    assert call("/readyz")["sandbox"] == "linux-cgroup-v2"
    language_ids = {entry["id"] for entry in call("/v1/languages")}
    samples = {
        "python": "print(input())",
        "c": '#include <stdio.h>\nint main(void) { int n; scanf("%d", &n); printf("%d\\n", n); }',
        "cpp": '#include <iostream>\nint main() { int n; std::cin >> n; std::cout << n << "\\n"; }',
        "javascript": "console.log(require('node:fs').readFileSync('/dev/stdin','utf8').trim())",
        "typescript": "import fs from 'node:fs'; console.log(fs.readFileSync(0,'utf8').trim());",
        "java": "public class Main { public static void main(String[] a) { System.out.println(new java.util.Scanner(System.in).nextInt()); } }",
    }
    assert samples.keys() <= language_ids
    for language, source in samples.items():
        assert execute(source, language=language, stdin="42\n") == "42\n"

    common = {"language": "c", "sourceCode": samples["c"]}
    call("/v1/judge", {**common, "argv": ["false"]}, expected_code=422)
    call("/v1/judge", {**common, "limits": {"cpuMs": 0}}, expected_code=422)
    batch = call("/v1/judge/batch", {
        **common,
        "cases": [{"stdin": "1\n", "expectedOutput": "1"},
                  {"stdin": "2\n", "expectedOutput": "3"}],
    })
    assert batch["compilation"] is None, batch
    assert [case["status"] for case in batch["results"]] == ["Accepted", "Wrong Answer"], batch
    failed_batch = call("/v1/judge/batch", {
        "language": "c", "sourceCode": "bad compiler input", "cases": [{}],
    })
    assert failed_batch["compilation"]["status"] == "Compilation Error", failed_batch
    assert failed_batch["results"] == []

    isolated_batch = call("/v1/judge/batch", {
        "language": "python",
        "sourceCode": "import os; print(os.path.exists('case')); open('case', 'w').close()",
        "cases": [{"expectedOutput": "False"}, {"expectedOutput": "False"}],
    })
    assert all(case["status"] == "Accepted" for case in isolated_batch["results"]), isolated_batch
    assert execute("print('compatible')", cpuMs=60500, wallMs=62500) == "compatible\n"
    execute("raise ValueError('sample')", "Nonzero Exit Status")
    execute("while True: pass", "Time Limit Exceeded", cpuMs=200, wallMs=2000)
    execute("import time; time.sleep(4)", "Time Limit Exceeded", wallMs=200)
    execute("blocks = [bytearray(1024 * 1024) for _ in range(256)]",
            "Memory Limit Exceeded", memoryKb=32768)
    assert len(execute("print('x' * 4096)", "Output Limit Exceeded", outputBytes=64)) == 64

    assert execute(
        "import os, socket, subprocess\n"
        "assert os.getuid() == 65534\n"
        "assert os.statvfs('/usr').f_flag & os.ST_RDONLY\n"
        "assert not os.path.exists('/sys/fs/cgroup')\n"
        "assert 'RUST_JUDGER_TOKEN' not in os.environ\n"
        "try:\n"
        "    socket.socket()\n"
        "except PermissionError:\n"
        "    subprocess.Popen(['/usr/bin/sleep', '30'])\n"
        "    print('isolated')\n"
    ) == "isolated\n"
    print("atr-judger smoke checks passed.")


if __name__ == "__main__":
    main()
