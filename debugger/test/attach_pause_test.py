#!/usr/bin/env python3
"""Remote JVM Debug: attach to a JVM listening on a jdwp port, pause without a thread id
(the IDE's ⏸ button) → stopped on "main" with a stack, threads list, continue, disconnect."""
import json, subprocess, threading, queue, time, pathlib, socket, tempfile, os

HERE = pathlib.Path(__file__).resolve().parent
JAR = HERE.parent / "build/chodilamide-debugger.jar"
JAVA = "/usr/lib/jvm/java-21-openjdk/bin/java"
JAVAC = "/usr/lib/jvm/java-21-openjdk/bin/javac"
tmp = pathlib.Path(tempfile.mkdtemp())
(tmp / "Loop.java").write_text("public class Loop { public static void main(String[] a) throws Exception { while (true) { Thread.sleep(50); } } }\n")
subprocess.run([JAVAC, "-g", "-d", str(tmp), str(tmp / "Loop.java")], check=True)
s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
target = subprocess.Popen([JAVA, f"-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=127.0.0.1:{port}", "-cp", str(tmp), "Loop"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
p = subprocess.Popen([JAVA, "-jar", str(JAR)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
msgs, seq = queue.Queue(), [0]

def reader():
    while True:
        line, length = p.stdout.readline(), None
        if not line: return
        while line.strip():
            if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
            line = p.stdout.readline()
        msgs.put(json.loads(p.stdout.read(length)))
threading.Thread(target=reader, daemon=True).start()

def send(cmd, **args):
    seq[0] += 1
    body = json.dumps({"seq": seq[0], "type": "request", "command": cmd, "arguments": args}).encode()
    p.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body); p.stdin.flush()
    return seq[0]

def wait(pred, what, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        try: m = msgs.get(timeout=0.2)
        except queue.Empty: continue
        if pred(m): return m
    raise SystemExit(f"TIMEOUT {what}")

def req(cmd, **args):
    s = send(cmd, **args)
    r = wait(lambda m: m.get("type") == "response" and m.get("request_seq") == s, cmd)
    if not r["success"]: raise SystemExit(f"{cmd} FAILED: {r.get('message')}")
    return r.get("body") or {}

results = []
ok = lambda label, cond, info="": results.append(cond) or print(("PASS " if cond else "FAIL ") + label + (f"  [{info}]" if info else ""))
try:
    req("initialize", adapterID="java")
    req("attach", hostName="127.0.0.1", port=port, sourcePaths=[str(tmp)], timeout=10000)
    req("configurationDone")
    time.sleep(0.5)
    req("pause", threadId=0)
    st = wait(lambda m: m.get("event") == "stopped", "stopped")
    tid = st["body"]["threadId"]
    ok("pause → stopped trên một thread thật", tid != 0, f"threadId={tid}")
    frames = req("stackTrace", threadId=tid, startFrame=0, levels=20)["stackFrames"]
    ok("stack có Loop.main", any("main" in f["name"] for f in frames), frames[0]["name"] if frames else "")
    names = [t["name"] for t in req("threads")["threads"]]
    ok("threads có main", "main" in names, f"{len(names)} thread")
    req("continue", threadId=tid)
    ok("continue", True)
    req("disconnect", terminateDebuggee=True)
finally:
    target.kill(); p.kill()
print(f"{sum(results)}/{len(results)}")
raise SystemExit(0 if all(results) else 1)
