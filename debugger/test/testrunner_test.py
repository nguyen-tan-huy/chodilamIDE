#!/usr/bin/env python3
"""JUnit through the debugger (no Maven): Run and Debug of JUnit 5 + JUnit 4 tests.
Usage: testrunner_test.py <maven project with target/test-classes and cp.txt>"""
import json, subprocess, sys, threading, queue, time, pathlib, glob, os
proj = pathlib.Path(sys.argv[1])
base = pathlib.Path(__file__).resolve().parent.parent
m2 = pathlib.Path.home() / ".m2/repository"
deps = (proj / "cp.txt").read_text().strip().split(":")
engine = next(p for p in deps if "junit-platform-engine-" in p)
ver = engine.rsplit("junit-platform-engine-", 1)[1][:-4]
launcher = m2 / f"org/junit/platform/junit-platform-launcher/{ver}/junit-platform-launcher-{ver}.jar"
cp = [str(proj / "target/test-classes"), str(proj / "target/classes")] + deps + [str(launcher), str(base / "build/chodilamide-testrunner.jar")]
test_file = proj / "src/test/java/vn/demo/GiaTest.java"
bp_line = next(i + 1 for i, l in enumerate(test_file.read_text().splitlines()) if "assertEquals(99" in l)
results = []
ok = lambda label, cond, info="": results.append(cond) or print(("PASS " if cond else "FAIL ") + label + (f"  [{info}]" if info else ""))

def session(targets, debug, condition=None):
    d = subprocess.Popen(["/usr/lib/jvm/java-21-openjdk/bin/java", "-jar", str(base / "build/chodilamide-debugger.jar")], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    q, seq, out = queue.Queue(), [0], []
    def reader():
        while True:
            h, n = d.stdout.readline(), None
            if not h: return
            while h.strip():
                if h.lower().startswith(b"content-length"): n = int(h.split(b":")[1])
                h = d.stdout.readline()
            m = json.loads(d.stdout.read(n))
            if m.get("event") == "output": out.append(m["body"]["output"])
            q.put(m)
    threading.Thread(target=reader, daemon=True).start()
    def send(cmd, **a):
        seq[0] += 1
        b = json.dumps({"seq": seq[0], "type": "request", "command": cmd, "arguments": a}).encode()
        d.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); d.stdin.flush(); return seq[0]
    def req(cmd, **a):
        my = send(cmd, **a)
        while True:
            m = q.get(timeout=30)
            if m.get("type") == "response" and m["request_seq"] == my: assert m["success"], m; return m.get("body") or {}
    def event(*names):
        while True:
            m = q.get(timeout=60)
            if m.get("event") in names: return m
    t0 = time.time()
    req("initialize", adapterID="java")
    req("launch", mainClass="chodilamide.testrunner.Main", classPaths=cp, sourcePaths=[str(proj / "src/test/java"), str(proj / "src/main/java")],
        args=" ".join(targets), vmArgs="", env={}, cwd=str(proj), javaExec="/usr/lib/jvm/java-11-openjdk/bin/java", noDebug=not debug)
    event("initialized")
    stopped_info = None
    bp = {"line": bp_line}
    if condition: bp["condition"] = condition
    if debug: req("setBreakpoints", source={"path": str(test_file)}, breakpoints=[bp])
    req("configurationDone")
    if debug and condition != "ketQua == 1":
        st = event("stopped")
        f = req("stackTrace", threadId=st["body"]["threadId"])["stackFrames"][0]
        vs = {v["name"]: v["value"] for v in req("variables", variablesReference=req("scopes", frameId=f["id"])["scopes"][0]["variablesReference"])["variables"]}
        stopped_info = (f["line"], vs.get("ketQua"))
        req("continue", threadId=st["body"]["threadId"])
    ex = event("exited")
    event("terminated")
    d.kill()
    lines = "".join(out).splitlines()
    res = [json.loads(l[6:]) for l in lines if l.startswith("@@CDL ")]
    return time.time() - t0, res, ex["body"]["exitCode"], stopped_info

dt, res, code, _ = session(["vn.demo.GiaTest", "vn.demo.CuTest"], False)
fin = {(r["class"].split(".")[-1], r["name"]): r for r in res if r["event"] == "finished"}
ok(f"run 2 class (JUnit 5 + JUnit 4) không qua Maven: {dt:.2f}s", len(fin) == 4, str(sorted(fin)))
ok("JUnit 5 pass/fail", fin[("GiaTest", "giamMuoiPhanTram()")]["status"] == "passed" and fin[("GiaTest", "giamSai()")]["status"] == "failed")
ok("thông báo lỗi tiếng Việt", "Tiếng Việt: sai giá" in fin[("GiaTest", "giamSai()")]["message"])
ok("JUnit 4 qua JUnitCore", fin[("CuTest", "junit4Dung")]["status"] == "passed" and fin[("CuTest", "junit4Sai")]["status"] == "failed")
ok("exit code = có test lỗi", code == 1)
dt, res, code, stop = session(["vn.demo.GiaTest#giamSai"], True)
fin = [r for r in res if r["event"] == "finished"]
ok(f"debug 1 method: dừng ở breakpoint, đọc biến ({dt:.2f}s)", stop == (bp_line, "100"), str(stop))
ok("chỉ chạy đúng method đã chọn", [r["name"] for r in fin] == ["giamSai()"], str([r["name"] for r in fin]))
dt, res, code, stop = session(["vn.demo.GiaTest#giamSai"], True, "ketQua == 100")
ok("debug test: breakpoint điều kiện đúng -> dừng", stop == (bp_line, "100"), str(stop))
dt, res, code, stop = session(["vn.demo.GiaTest#giamSai"], True, "ketQua == 1")
ok("debug test: điều kiện sai -> không dừng, test chạy hết", stop is None and any(r["event"] == "finished" for r in res), str(stop))
print(f"\n{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
