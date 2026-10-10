#!/usr/bin/env python3
"""Debug a JUnit test run by Maven Surefire: mvn waits on a JDWP port, the debugger attaches,
stops at a breakpoint in the test, reads a local, continues. Usage: attach_surefire_test.py <project>"""
import json, os, re, socket, subprocess, sys, threading, queue, time, pathlib
proj = pathlib.Path(sys.argv[1])
test_file = proj / "src/test/java/vn/demo/GiaTest.java"
line = next(i + 1 for i, l in enumerate(test_file.read_text().splitlines()) if "assertEquals(99" in l)
s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
mvn = subprocess.Popen(["mvn", "-o", "test", "-B", "-Dtest=vn.demo.GiaTest#giamSai", "-DforkCount=1",
    f"-Dmaven.surefire.debug=-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{port}"],
    cwd=proj, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
t0 = time.time()
# Surefire 3 does not echo the JDWP banner: the debugger retries attaching until the port opens
out = []
threading.Thread(target=lambda: out.extend(mvn.stdout), daemon=True).start()
jar = pathlib.Path(__file__).resolve().parent.parent / "build/chodilamide-debugger.jar"
d = subprocess.Popen(["/usr/lib/jvm/java-21-openjdk/bin/java", "-jar", str(jar)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
q, seq = queue.Queue(), [0]
def reader():
    while True:
        h, n = d.stdout.readline(), None
        if not h: return
        while h.strip():
            if h.lower().startswith(b"content-length"): n = int(h.split(b":")[1])
            h = d.stdout.readline()
        q.put(json.loads(d.stdout.read(n)))
threading.Thread(target=reader, daemon=True).start()
def req(cmd, **a):
    seq[0] += 1; my = seq[0]
    b = json.dumps({"seq": my, "type": "request", "command": cmd, "arguments": a}).encode()
    d.stdin.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); d.stdin.flush()
    while True:
        m = q.get(timeout=30)
        if m.get("type") == "response" and m["request_seq"] == my:
            assert m["success"], m; return m.get("body") or {}
def event(name):
    while True:
        m = q.get(timeout=60)
        if m.get("event") == name: return m
req("initialize", adapterID="java")
req("attach", hostName="127.0.0.1", port=port, sourcePaths=[str(proj / "src/test/java"), str(proj / "src/main/java")])
event("initialized")
bp = req("setBreakpoints", source={"path": str(test_file)}, breakpoints=[{"line": line}])
req("configurationDone")
st = event("stopped")
f = req("stackTrace", threadId=st["body"]["threadId"])["stackFrames"][0]
vs = {v["name"]: v["value"] for v in req("variables", variablesReference=req("scopes", frameId=f["id"])["scopes"][0]["variablesReference"])["variables"]}
ok = f["source"]["path"] == str(test_file) and f["line"] == line and vs.get("ketQua") == "100"
print(("PASS" if ok else "FAIL"), f"dừng ở {f['name']}:{f['line']}  ketQua={vs.get('ketQua')}")
req("continue", threadId=st["body"]["threadId"])
event("terminated")
mvn.wait(); time.sleep(0.3); rest = "".join(out)
print(f"tổng thời gian {time.time()-t0:.1f}s")
print("PASS" if "Tests run: 1, Failures: 1" in rest else "FAIL", "mvn báo kết quả test sau khi debug xong")
d.kill(); sys.exit(0 if ok else 1)
