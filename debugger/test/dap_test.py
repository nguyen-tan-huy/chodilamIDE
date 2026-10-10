#!/usr/bin/env python3
"""End-to-end DAP test for chodilamide-debugger against test/sample (two modules, JDK 11 target):
breakpoint in another module (set before the class loads), condition, stack, variables
(collections), evaluate, step, logpoint, hot swap, program output, exit code."""
import json, os, subprocess, sys, threading, queue, time, pathlib

HERE = pathlib.Path(__file__).resolve().parent
JAR = HERE.parent / "build/chodilamide-debugger.jar"
S = HERE / "sample"
JAVA21 = "/usr/lib/jvm/java-21-openjdk/bin/java"
JAVA11 = "/usr/lib/jvm/java-11-openjdk/bin/java"
GREETER = str(S / "core/src/vn/demo/core/Greeter.java")
MAIN = str(S / "app/src/vn/demo/app/Main.java")
line_of = lambda path, text: next(i + 1 for i, l in enumerate(open(path, encoding="utf-8")) if text in l)
PRINT_LINE = line_of(MAIN, "System.out.println(g.greet")

# always start from freshly compiled sample classes (the hot-swap step recompiles Greeter)
JAVAC11 = "/usr/lib/jvm/java-11-openjdk/bin/javac"
subprocess.run([JAVAC11, "-g", "-d", str(S / "core/out"), GREETER], check=True)
subprocess.run([JAVAC11, "-g", "-cp", str(S / "core/out"), "-d", str(S / "app/out"), MAIN], check=True)

p = subprocess.Popen([JAVA21, "-jar", str(JAR)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
msgs, seq = queue.Queue(), [0]
output = []

def reader():
    while True:
        line, length = p.stdout.readline(), None
        if not line: return
        while line.strip():
            if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
            line = p.stdout.readline()
        m = json.loads(p.stdout.read(length))
        if m.get("event") == "output": output.append(m["body"]["output"])
        msgs.put(m)
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
    raise SystemExit(f"TIMEOUT waiting for {what}; output so far: {''.join(output)}")

def req(cmd, **args):
    s = send(cmd, **args)
    r = wait(lambda m: m.get("type") == "response" and m.get("request_seq") == s, cmd)
    if not r["success"]: raise SystemExit(f"{cmd} FAILED: {r.get('message')}")
    return r.get("body") or {}

def dump(): print("OUTPUT:", "".join(output)[:1500])
def stopped(): return wait(lambda m: m.get("event") == "stopped", "stopped")
ok = lambda label, cond, info="": print(("PASS " if cond else "FAIL ") + label + (f"  [{info}]" if info else "")) or cond
results = []

req("initialize", adapterID="java", linesStartAt1=True)
req("launch", mainClass="vn.demo.app.Main", javaExec=JAVA11,
    classPaths=[str(S / "app/out"), str(S / "core/out")],
    sourcePaths=[str(S / "app/src"), str(S / "core/src")],
    vmArgs="-Dx=1", args="", env={"DEMO_ENV": "môi trường", "DEMO_SLEEP": "1500"}, cwd=str(S))
wait(lambda m: m.get("event") == "initialized", "initialized")
# line 13 = `List<String> parts = ...` in Greeter (other module, not loaded yet) - only when name is Lan
b = req("setBreakpoints", source={"path": GREETER}, breakpoints=[{"line": 13, "condition": "name.equals(\"Lan\") && count == 2"}])
results.append(ok("breakpoint pending before class load", b["breakpoints"][0]["verified"] is False))
# logpoint in Main line 14 (the println)
req("setBreakpoints", source={"path": MAIN}, breakpoints=[{"line": PRINT_LINE, "logMessage": "log: name={name}"}])
req("configurationDone")

ev = stopped()
st = req("stackTrace", threadId=ev["body"]["threadId"])
top = st["stackFrames"][0]
results.append(ok("stopped in other module, right line", top["source"]["path"] == GREETER and top["line"] == 13, f"{top['name']}:{top['line']}"))
results.append(ok("caller frame maps to app module", st["stackFrames"][1]["source"]["path"] == MAIN))
sc = req("scopes", frameId=top["id"])
vs = {v["name"]: v for v in req("variables", variablesReference=sc["scopes"][0]["variablesReference"])["variables"]}
results.append(ok("locals: name (Vietnamese-safe) + this", vs["name"]["value"] == '"Lan"' and "this" in vs, vs["name"]["value"]))
this_fields = {v["name"]: v["value"] for v in req("variables", variablesReference=vs["this"]["variablesReference"])["variables"]}
results.append(ok("condition respected (count==2)", this_fields.get("count") == "2", str(this_fields)))
e = req("evaluate", expression='prefix + " " + name + " " + name.length()', frameId=top["id"], context="watch")
results.append(ok("evaluate concat + method call", e["result"] == '"Xin chào Lan 3"', e["result"]))

req("next", threadId=ev["body"]["threadId"])
ev2 = stopped()
top2 = req("stackTrace", threadId=ev2["body"]["threadId"])["stackFrames"][0]
results.append(ok("step over → next line", top2["line"] == 14, str(top2["line"])))
vs3 = {v["name"]: v for v in req("variables", variablesReference=req("scopes", frameId=top2["id"])["scopes"][0]["variablesReference"])["variables"]}
results.append(ok("collection shown with size", vs3["parts"]["value"].startswith("ArrayList (size=2)"), vs3["parts"]["value"]))
kids = req("variables", variablesReference=vs3["parts"]["variablesReference"])["variables"]
results.append(ok("collection elements expand", [k["value"] for k in kids] == ['"Xin chào"', '"Lan"'], str([k["value"] for k in kids])))
req("stepOut", threadId=ev2["body"]["threadId"])
ev3 = stopped()
top3 = req("stackTrace", threadId=ev3["body"]["threadId"])["stackFrames"][0]
results.append(ok("step out → back in caller (other module)", top3["source"]["path"] == MAIN and top3["line"] == PRINT_LINE, f"{top3['name']}:{top3['line']}"))

# hot swap: change the returned text, recompile Greeter with JDK 11, redefine
src = pathlib.Path(GREETER).read_text()
pathlib.Path(GREETER).write_text(src.replace('" #" + count', '" [hot] #" + count'))
time.sleep(1.1)
subprocess.run(["/usr/lib/jvm/java-11-openjdk/bin/javac", "-g", "-d", str(S / "core/out"), GREETER], check=True)
pathlib.Path(GREETER).write_text(src)
hs = req("redefineClasses")
results.append(ok("hot swap redefines changed class", hs.get("changedClasses") == ["vn.demo.core.Greeter"], str(hs)))
req("setBreakpoints", source={"path": GREETER}, breakpoints=[])
req("continue", threadId=ev3["body"]["threadId"])
ex = wait(lambda m: m.get("event") in ("exited", "terminated"), "exited")
results.append(ok("exited comes before terminated", ex.get("event") == "exited"))
wait(lambda m: m.get("event") == "terminated", "terminated")
out = "".join(output)
results.append(ok("program output streamed", "Xin chào Huy #1 (2)" in out, ""))
results.append(ok("hot-swapped code ran on the next call", "Xin chào Minh [hot] #3 (2)" in out, ""))
results.append(ok("logpoint printed, no stop", "log: name=\"Huy\"" in out and "log: name=\"Lan\"" in out))
results.append(ok("env + cwd passed", "xong môi trường" in out))
results.append(ok("exit code 0", ex["body"]["exitCode"] == 0))
try: req("disconnect")
except SystemExit: pass
# leave clean (non hot-swapped) classes for other tests using the sample
subprocess.run([JAVAC11, "-g", "-d", str(S / "core/out"), GREETER], check=True)
print(f"\n{sum(results)}/{len(results)} passed")
sys.exit(0 if all(results) else 1)
