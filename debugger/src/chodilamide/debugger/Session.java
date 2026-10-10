package chodilamide.debugger;

import com.sun.jdi.AbsentInformationException;
import com.sun.jdi.ArrayReference;
import com.sun.jdi.Bootstrap;
import com.sun.jdi.ClassType;
import com.sun.jdi.Field;
import com.sun.jdi.IncompatibleThreadStateException;
import com.sun.jdi.LocalVariable;
import com.sun.jdi.Location;
import com.sun.jdi.Method;
import com.sun.jdi.ObjectReference;
import com.sun.jdi.PrimitiveValue;
import com.sun.jdi.ReferenceType;
import com.sun.jdi.StackFrame;
import com.sun.jdi.StringReference;
import com.sun.jdi.ThreadReference;
import com.sun.jdi.Value;
import com.sun.jdi.VirtualMachine;
import com.sun.jdi.connect.AttachingConnector;
import com.sun.jdi.connect.Connector;
import com.sun.jdi.event.BreakpointEvent;
import com.sun.jdi.event.ClassPrepareEvent;
import com.sun.jdi.event.Event;
import com.sun.jdi.event.EventSet;
import com.sun.jdi.event.ExceptionEvent;
import com.sun.jdi.event.StepEvent;
import com.sun.jdi.event.ThreadDeathEvent;
import com.sun.jdi.event.ThreadStartEvent;
import com.sun.jdi.event.VMDeathEvent;
import com.sun.jdi.event.VMDisconnectEvent;
import com.sun.jdi.event.VMStartEvent;
import com.sun.jdi.request.BreakpointRequest;
import com.sun.jdi.request.ClassPrepareRequest;
import com.sun.jdi.request.EventRequest;
import com.sun.jdi.request.EventRequestManager;
import com.sun.jdi.request.ExceptionRequest;
import com.sun.jdi.request.StepRequest;

import java.io.BufferedReader;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;

/** One debug session: launches/attaches a JVM through JDI and answers DAP requests. */
final class Session {
    private final Main dap;
    private VirtualMachine vm;
    private Process process;
    private boolean launched;
    private boolean noDebug;
    private volatile boolean terminated;
    private final List<Path> sourcePaths = new ArrayList<>();
    private final List<Path> outputDirs = new ArrayList<>();
    private long lastRedefine = System.currentTimeMillis();

    /** A breakpoint the client set on a source line. */
    private static final class Bp {
        final int id;
        final int line;
        final String condition;
        final String logMessage;
        final List<BreakpointRequest> requests = new ArrayList<>();
        Bp(int id, int line, String condition, String logMessage) {
            this.id = id;
            this.line = line;
            this.condition = condition == null || condition.isBlank() ? null : condition;
            this.logMessage = logMessage == null || logMessage.isBlank() ? null : logMessage;
        }
    }

    /** source file → its breakpoints; top-level class → source files mapped to it. */
    private final Map<String, List<Bp>> breakpoints = new ConcurrentHashMap<>();
    private final Map<String, String> classOfSource = new ConcurrentHashMap<>();
    private final Map<String, Boolean> prepareWatched = new ConcurrentHashMap<>();
    private int nextBpId = 1;
    private ExceptionRequest exceptionRequest;

    // handles valid while suspended; cleared on every resume
    private final Map<Integer, Object> handles = new ConcurrentHashMap<>();
    private int nextHandle = 1;
    private final Map<Long, ObjectReference> lastException = new ConcurrentHashMap<>();

    private record FrameKey(ThreadReference thread, int index) {}
    private record LocalsKey(ThreadReference thread, int index) {}
    private record ObjKey(ObjectReference obj, ThreadReference thread) {}

    Session(Main dap) { this.dap = dap; }

    // ------------------------------------------------------------------ dispatch

    @SuppressWarnings("unchecked")
    void handle(Map<String, Object> req) throws Exception {
        String cmd = (String) req.get("command");
        Map<String, Object> args = req.get("arguments") instanceof Map ? (Map<String, Object>) req.get("arguments") : Map.of();
        switch (cmd) {
            case "initialize" -> {
                dap.respond(req, true, null, Json.obj(
                        "supportsConfigurationDoneRequest", true,
                        "supportsConditionalBreakpoints", true,
                        "supportsLogPoints", true,
                        "supportsEvaluateForHovers", true,
                        "supportsExceptionInfoRequest", true,
                        "supportsTerminateRequest", true,
                        "supportsHotReload", true,
                        "exceptionBreakpointFilters", List.of(
                                Json.obj("filter", "uncaught", "label", "Uncaught Exceptions", "default", true),
                                Json.obj("filter", "caught", "label", "Caught Exceptions", "default", false))));
            }
            case "launch" -> launch(req, args);
            case "attach" -> attach(req, args);
            case "setBreakpoints" -> setBreakpoints(req, args);
            case "setExceptionBreakpoints" -> setExceptionBreakpoints(req, args);
            case "configurationDone" -> {
                dap.respond(req, true, null, null);
                if (vm != null) vm.resume();
            }
            case "threads" -> threads(req);
            case "stackTrace" -> stackTrace(req, args);
            case "scopes" -> scopes(req, args);
            case "variables" -> variables(req, args);
            case "evaluate" -> evaluate(req, args);
            case "continue" -> {
                resumeAll();
                dap.respond(req, true, null, Json.obj("allThreadsContinued", true));
            }
            case "next" -> step(req, args, StepRequest.STEP_OVER);
            case "stepIn" -> step(req, args, StepRequest.STEP_INTO);
            case "stepOut" -> step(req, args, StepRequest.STEP_OUT);
            case "pause" -> {
                vm.suspend();
                dap.respond(req, true, null, null);
                long tid = num(args.get("threadId"));
                if (tid == 0) tid = pauseThread();
                dap.event("stopped", Json.obj("reason", "pause", "threadId", tid, "allThreadsStopped", true));
            }
            case "exceptionInfo" -> exceptionInfo(req, args);
            case "redefineClasses", "hotReload" -> redefine(req);
            case "terminate" -> {
                dap.respond(req, true, null, null);
                terminate(true);
            }
            case "disconnect" -> {
                boolean kill = args.get("terminateDebuggee") == null ? launched : Boolean.TRUE.equals(args.get("terminateDebuggee"));
                dap.respond(req, true, null, null);
                terminate(kill);
                shutdown();
                System.exit(0);
            }
            default -> dap.respond(req, false, "unsupported request: " + cmd, null);
        }
    }

    private static long num(Object o) {
        return o instanceof Number n ? n.longValue() : 0;
    }

    private static String str(Object o) {
        return o == null ? null : o.toString();
    }

    // ------------------------------------------------------------------ launch / attach

    /** A jar holding only a manifest whose Class-Path lists every entry (as file: URIs). */
    static Path classpathJar(List<String> entries) throws java.io.IOException {
        java.util.jar.Manifest m = new java.util.jar.Manifest();
        java.util.jar.Attributes at = m.getMainAttributes();
        at.put(java.util.jar.Attributes.Name.MANIFEST_VERSION, "1.0");
        StringBuilder sb = new StringBuilder();
        for (String e : entries) {
            Path p = Paths.get(e).toAbsolutePath();
            String uri = p.toUri().toString();
            // a directory must end with "/" or it is read as a jar
            if (Files.isDirectory(p) && !uri.endsWith("/")) uri += "/";
            if (sb.length() > 0) sb.append(' ');
            sb.append(uri);
        }
        at.put(java.util.jar.Attributes.Name.CLASS_PATH, sb.toString());
        Path jar = Files.createTempFile("chodilamide-classpath-", ".jar");
        jar.toFile().deleteOnExit();
        try (java.util.jar.JarOutputStream out = new java.util.jar.JarOutputStream(Files.newOutputStream(jar), m)) {
            // the manifest is all
        }
        return jar;
    }

    @SuppressWarnings("unchecked")
    private void launch(Map<String, Object> req, Map<String, Object> a) throws Exception {
        launched = true;
        noDebug = Boolean.TRUE.equals(a.get("noDebug"));
        List<String> classPaths = a.get("classPaths") instanceof List ? (List<String>) a.get("classPaths") : List.of();
        for (Object p : (List<Object>) a.getOrDefault("sourcePaths", List.of())) sourcePaths.add(Paths.get(p.toString()));
        for (String cp : classPaths) {
            Path p = Paths.get(cp);
            if (Files.isDirectory(p)) outputDirs.add(p);
        }
        String java = str(a.get("javaExec"));
        if (java == null || java.isBlank()) java = Paths.get(System.getProperty("java.home"), "bin", "java").toString();
        String mainClass = str(a.get("mainClass"));
        if (mainClass == null) throw new IllegalArgumentException("launch: thiếu mainClass");

        List<String> cmd = new ArrayList<>();
        cmd.add(java);
        cmd.addAll(splitArgs(str(a.get("vmArgs"))));
        if (!noDebug) cmd.add("-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:0");
        if (!classPaths.isEmpty()) {
            String cp = String.join(File.pathSeparator, classPaths);
            cmd.add("-cp");
            // Windows refuses command lines over 32767 characters (CreateProcess error=206): a
            // big Maven classpath goes into the manifest of a small jar instead (IntelliJ's
            // "JAR manifest" shortening, works for JVMs 8+)
            boolean windows = System.getProperty("os.name", "").toLowerCase().contains("win");
            cmd.add(cp.length() > (windows ? 8000 : 100_000) ? classpathJar(classPaths).toString() : cp);
        }
        cmd.add(mainClass);
        Object progArgs = a.get("args");
        if (progArgs instanceof List<?> l) l.forEach(x -> cmd.add(x.toString()));
        else cmd.addAll(splitArgs(str(progArgs)));

        ProcessBuilder pb = new ProcessBuilder(cmd);
        String cwd = str(a.get("cwd"));
        if (cwd != null && !cwd.isBlank()) pb.directory(new File(cwd));
        if (a.get("env") instanceof Map<?, ?> env) env.forEach((k, v) -> pb.environment().put(k.toString(), v == null ? "" : v.toString()));
        process = pb.start();

        CountDownLatch portFound = new CountDownLatch(1);
        int[] port = {-1};
        pipe(process.getInputStream(), "stdout", line -> {
            Matcher m = Pattern.compile("Listening for transport dt_socket at address: (\\d+)").matcher(line);
            if (port[0] < 0 && m.find()) {
                port[0] = Integer.parseInt(m.group(1));
                portFound.countDown();
                return false; // swallow the JDWP banner
            }
            return true;
        });
        pipe(process.getErrorStream(), "stderr", l -> true);
        dap.event("process", Json.obj("name", mainClass, "systemProcessId", process.pid(), "startMethod", "launch"));
        new Thread(() -> {
            try {
                int code = process.waitFor();
                // give JDI a moment to report VMDeath first
                Thread.sleep(200);
                dap.event("exited", Json.obj("exitCode", code));
                finish();
            } catch (InterruptedException ignored) {
            }
        }, "process-wait").start();

        if (noDebug) {
            dap.respond(req, true, null, null);
            dap.event("initialized", null);
            return;
        }
        if (!portFound.await(30, TimeUnit.SECONDS) || port[0] < 0) {
            process.destroyForcibly();
            throw new IllegalStateException("JVM không mở cổng JDWP (kiểm tra javaExec/vmArgs)");
        }
        connect("127.0.0.1", port[0]);
        dap.respond(req, true, null, null);
        dap.event("initialized", null);
    }

    private void attach(Map<String, Object> req, Map<String, Object> a) throws Exception {
        String host = a.get("hostName") != null ? str(a.get("hostName")) : "127.0.0.1";
        int port = (int) num(a.get("port"));
        @SuppressWarnings("unchecked")
        List<Object> sp = (List<Object>) a.getOrDefault("sourcePaths", List.of());
        sp.forEach(p -> sourcePaths.add(Paths.get(p.toString())));
        // the target may not listen yet (e.g. Maven still compiling before Surefire forks):
        // retry until it does, up to `timeout` ms
        long deadline = System.currentTimeMillis() + (a.get("timeout") == null ? 90_000 : num(a.get("timeout")));
        while (true) {
            try {
                connect(host, port);
                break;
            } catch (java.io.IOException e) {
                if (System.currentTimeMillis() > deadline) throw new IllegalStateException("không kết nối được " + host + ":" + port + " - " + e.getMessage());
                Thread.sleep(300);
            }
        }
        dap.respond(req, true, null, null);
        dap.event("initialized", null);
    }

    private void connect(String host, int port) throws Exception {
        AttachingConnector c = Bootstrap.virtualMachineManager().attachingConnectors().stream()
                .filter(x -> x.name().equals("com.sun.jdi.SocketAttach")).findFirst().orElseThrow();
        Map<String, Connector.Argument> args = c.defaultArguments();
        args.get("hostname").setValue(host);
        args.get("port").setValue(Integer.toString(port));
        args.get("timeout").setValue("10000");
        vm = c.attach(args);
        EventRequestManager erm = vm.eventRequestManager();
        var ts = erm.createThreadStartRequest();
        ts.setSuspendPolicy(EventRequest.SUSPEND_NONE);
        ts.enable();
        var td = erm.createThreadDeathRequest();
        td.setSuspendPolicy(EventRequest.SUSPEND_NONE);
        td.enable();
        // breakpoints set before connecting (none yet) + uncaught exceptions by default
        setExceptionFilters(false, true);
        Thread t = new Thread(this::eventLoop, "jdi-events");
        t.setDaemon(true);
        t.start();
    }

    interface LineFilter { boolean keep(String line); }

    private void pipe(InputStream in, String category, LineFilter filter) {
        Thread t = new Thread(() -> {
            try (BufferedReader r = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8))) {
                String line;
                while ((line = r.readLine()) != null) {
                    if (filter.keep(line)) dap.output(category, line + "\n");
                }
            } catch (IOException ignored) {
            }
        }, "pipe-" + category);
        t.setDaemon(true);
        t.start();
    }

    /** Splits a command-line string honoring "double" and 'single' quotes. */
    static List<String> splitArgs(String s) {
        List<String> out = new ArrayList<>();
        if (s == null) return out;
        StringBuilder cur = new StringBuilder();
        char quote = 0;
        boolean has = false;
        for (char c : s.toCharArray()) {
            if (quote != 0) {
                if (c == quote) quote = 0;
                else cur.append(c);
            } else if (c == '"' || c == '\'') {
                quote = c;
                has = true;
            } else if (Character.isWhitespace(c)) {
                if (has || cur.length() > 0) out.add(cur.toString());
                cur.setLength(0);
                has = false;
            } else cur.append(c);
        }
        if (has || cur.length() > 0) out.add(cur.toString());
        return out;
    }

    // ------------------------------------------------------------------ breakpoints

    @SuppressWarnings("unchecked")
    private void setBreakpoints(Map<String, Object> req, Map<String, Object> a) throws IOException {
        Map<String, Object> source = (Map<String, Object>) a.get("source");
        String path = str(source.get("path"));
        List<Object> specs = (List<Object>) a.getOrDefault("breakpoints", List.of());

        // drop the old ones for this file
        for (Bp old : breakpoints.getOrDefault(path, List.of())) {
            if (vm != null) old.requests.forEach(r -> vm.eventRequestManager().deleteEventRequest(r));
        }
        List<Bp> bps = new ArrayList<>();
        for (Object o : specs) {
            Map<String, Object> s = (Map<String, Object>) o;
            bps.add(new Bp(nextBpId++, (int) num(s.get("line")), str(s.get("condition")), str(s.get("logMessage"))));
        }
        breakpoints.put(path, bps);

        String cls = classNameForSource(path);
        List<Object> result = new ArrayList<>();
        if (cls != null) {
            classOfSource.put(path, cls);
            if (vm != null && !noDebug) {
                watchPrepare(cls);
                for (ReferenceType t : loadedClassesFor(cls)) resolve(t, bps);
            }
        }
        for (Bp b : bps) {
            Map<String, Object> r = Json.obj("id", b.id, "verified", !b.requests.isEmpty(), "line", b.line);
            if (b.requests.isEmpty()) r.put("message", cls == null ? "không xác định được class của file" : "chờ class được nạp");
            result.add(r);
        }
        dap.respond(req, true, null, Json.obj("breakpoints", result));
    }

    /** `package a.b;` + file name → "a.b.File" (reads the file, so any source layout works). */
    static String classNameForSource(String path) throws IOException {
        Path p = Paths.get(path);
        if (!Files.isRegularFile(p)) return null;
        String name = p.getFileName().toString();
        if (!name.endsWith(".java")) return null;
        String simple = name.substring(0, name.length() - 5);
        Pattern pkg = Pattern.compile("^\\s*package\\s+([\\w.]+)\\s*;");
        try (Stream<String> lines = Files.lines(p, StandardCharsets.UTF_8)) {
            for (String l : (Iterable<String>) lines.limit(200)::iterator) {
                Matcher m = pkg.matcher(l);
                if (m.find()) return m.group(1) + "." + simple;
            }
        }
        return simple;
    }

    private List<ReferenceType> loadedClassesFor(String cls) {
        List<ReferenceType> out = new ArrayList<>();
        for (ReferenceType t : vm.allClasses()) {
            String n = t.name();
            if (n.equals(cls) || n.startsWith(cls + "$")) out.add(t);
        }
        return out;
    }

    private void watchPrepare(String cls) {
        if (prepareWatched.putIfAbsent(cls, true) != null) return;
        EventRequestManager erm = vm.eventRequestManager();
        for (String filter : List.of(cls, cls + "$*")) {
            ClassPrepareRequest r = erm.createClassPrepareRequest();
            r.addClassFilter(filter);
            r.setSuspendPolicy(EventRequest.SUSPEND_EVENT_THREAD);
            r.enable();
        }
    }

    /** Places not-yet-placed breakpoints of `bps` that have code in class `t`. */
    private boolean resolve(ReferenceType t, List<Bp> bps) {
        boolean any = false;
        for (Bp b : bps) {
            try {
                for (Location loc : t.locationsOfLine(b.line)) {
                    if (b.requests.stream().anyMatch(r -> r.location().equals(loc))) continue;
                    BreakpointRequest r = vm.eventRequestManager().createBreakpointRequest(loc);
                    r.setSuspendPolicy(EventRequest.SUSPEND_ALL);
                    r.putProperty("bp", b);
                    r.enable();
                    b.requests.add(r);
                    any = true;
                }
            } catch (AbsentInformationException ignored) {
                // compiled without -g:lines
            }
        }
        return any;
    }

    @SuppressWarnings("unchecked")
    private void setExceptionBreakpoints(Map<String, Object> req, Map<String, Object> a) {
        List<Object> filters = (List<Object>) a.getOrDefault("filters", List.of());
        if (vm != null) setExceptionFilters(filters.contains("caught"), filters.contains("uncaught"));
        dap.respond(req, true, null, null);
    }

    private void setExceptionFilters(boolean caught, boolean uncaught) {
        EventRequestManager erm = vm.eventRequestManager();
        if (exceptionRequest != null) erm.deleteEventRequest(exceptionRequest);
        exceptionRequest = null;
        if (!caught && !uncaught) return;
        exceptionRequest = erm.createExceptionRequest(null, caught, uncaught);
        exceptionRequest.setSuspendPolicy(EventRequest.SUSPEND_ALL);
        // the JDK throws and catches internally all the time
        for (String ex : List.of("java.*", "javax.*", "jdk.*", "sun.*", "com.sun.*")) exceptionRequest.addClassExclusionFilter(ex);
        exceptionRequest.enable();
    }

    // ------------------------------------------------------------------ events

    private void eventLoop() {
        try {
            while (!terminated) {
                EventSet set = vm.eventQueue().remove();
                boolean resume = true;
                for (Event e : set) {
                    if (e instanceof VMStartEvent) {
                        resume = false; // stays suspended until configurationDone
                    } else if (e instanceof ClassPrepareEvent cp) {
                        onClassPrepare(cp.referenceType());
                    } else if (e instanceof BreakpointEvent be) {
                        if (onBreakpoint(be)) resume = false;
                    } else if (e instanceof StepEvent se) {
                        vm.eventRequestManager().deleteEventRequest(se.request());
                        stopped("step", se.thread(), null);
                        resume = false;
                    } else if (e instanceof ExceptionEvent ee) {
                        lastException.put(ee.thread().uniqueID(), ee.exception());
                        stopped("exception", ee.thread(), ee.exception().referenceType().name());
                        resume = false;
                    } else if (e instanceof ThreadStartEvent ts) {
                        dap.event("thread", Json.obj("reason", "started", "threadId", ts.thread().uniqueID()));
                    } else if (e instanceof ThreadDeathEvent td) {
                        dap.event("thread", Json.obj("reason", "exited", "threadId", td.thread().uniqueID()));
                    } else if (e instanceof VMDeathEvent || e instanceof VMDisconnectEvent) {
                        // launched: the process watcher sends `exited` then `terminated` (DAP order)
                        if (!launched) finish();
                        return;
                    }
                }
                if (resume) set.resume();
            }
        } catch (InterruptedException | com.sun.jdi.VMDisconnectedException e) {
            if (!launched) finish();
        }
    }

    private void onClassPrepare(ReferenceType t) {
        String name = t.name();
        int dollar = name.indexOf('$');
        String top = dollar < 0 ? name : name.substring(0, dollar);
        for (Map.Entry<String, String> e : classOfSource.entrySet()) {
            if (!e.getValue().equals(top)) continue;
            List<Bp> bps = breakpoints.getOrDefault(e.getKey(), List.of());
            List<Bp> before = new ArrayList<>();
            for (Bp b : bps) if (b.requests.isEmpty()) before.add(b);
            if (resolve(t, bps)) {
                for (Bp b : before) {
                    if (!b.requests.isEmpty()) dap.event("breakpoint", Json.obj("reason", "changed",
                            "breakpoint", Json.obj("id", b.id, "verified", true, "line", b.line)));
                }
            }
        }
    }

    /** @return true if the program should stay stopped */
    private boolean onBreakpoint(BreakpointEvent be) {
        Bp b = (Bp) be.request().getProperty("bp");
        ThreadReference thread = be.thread();
        if (b != null && b.condition != null) {
            try {
                Object v = Eval.toJava(new Eval(vm, thread, 0).evaluate(b.condition));
                if (!Boolean.TRUE.equals(v)) return false;
            } catch (Exception ex) {
                dap.output("console", "Điều kiện breakpoint lỗi (dòng " + b.line + "): " + ex.getMessage() + " - vẫn dừng\n");
            }
        }
        if (b != null && b.logMessage != null) {
            dap.output("console", interpolate(b.logMessage, thread) + "\n");
            return false;
        }
        Map<String, Object> body = Json.obj("reason", "breakpoint", "threadId", thread.uniqueID(), "allThreadsStopped", true);
        if (b != null) body.put("hitBreakpointIds", List.of(b.id));
        clearHandles();
        dap.event("stopped", body);
        return true;
    }

    /** Logpoint text: "x = {x}" evaluates each {expr}. */
    private String interpolate(String msg, ThreadReference thread) {
        Matcher m = Pattern.compile("\\{([^}]+)}").matcher(msg);
        StringBuilder b = new StringBuilder();
        while (m.find()) {
            String v;
            try {
                v = display(new Eval(vm, thread, 0).evaluate(m.group(1)));
            } catch (Exception e) {
                v = "<" + e.getMessage() + ">";
            }
            m.appendReplacement(b, Matcher.quoteReplacement(v));
        }
        m.appendTail(b);
        return b.toString();
    }

    private void stopped(String reason, ThreadReference thread, String text) {
        clearHandles();
        Map<String, Object> body = Json.obj("reason", reason, "threadId", thread.uniqueID(), "allThreadsStopped", true);
        if (text != null) body.put("text", text);
        dap.event("stopped", body);
    }

    private synchronized void finish() {
        if (terminated) return;
        terminated = true;
        dap.event("terminated", null);
    }

    // ------------------------------------------------------------------ execution control

    private void clearHandles() {
        handles.clear();
    }

    private void resumeAll() {
        clearHandles();
        lastException.clear();
        vm.resume();
    }

    private ThreadReference thread(long id) {
        for (ThreadReference t : vm.allThreads()) if (t.uniqueID() == id) return t;
        throw new IllegalArgumentException("không có thread " + id);
    }

    private void step(Map<String, Object> req, Map<String, Object> a, int depth) {
        ThreadReference t = thread(num(a.get("threadId")));
        EventRequestManager erm = vm.eventRequestManager();
        for (StepRequest old : new ArrayList<>(erm.stepRequests())) if (old.thread().equals(t)) erm.deleteEventRequest(old);
        StepRequest s = erm.createStepRequest(t, StepRequest.STEP_LINE, depth);
        for (String ex : List.of("java.*", "javax.*", "jdk.*", "sun.*", "com.sun.*", "org.springframework.cglib.*")) s.addClassExclusionFilter(ex);
        s.addCountFilter(1);
        s.setSuspendPolicy(EventRequest.SUSPEND_ALL);
        s.enable();
        dap.respond(req, true, null, null);
        resumeAll();
    }

    private void terminate(boolean kill) {
        try {
            if (vm != null) {
                if (kill) vm.exit(1);
                else vm.dispose();
            }
        } catch (Exception ignored) {
        }
        if (kill && process != null) process.destroy();
    }

    void shutdown() {
        if (launched && process != null && process.isAlive()) process.destroy();
    }

    // ------------------------------------------------------------------ inspection

    private int handle(Object o) {
        int id = nextHandle++;
        handles.put(id, o);
        return id;
    }

    /** Pause without a thread (the IDE's ⏸ button): "main", else the first thread with frames. */
    private long pauseThread() {
        ThreadReference best = null;
        for (ThreadReference t : vm.allThreads()) {
            try {
                if (t.frameCount() == 0) continue;
            } catch (IncompatibleThreadStateException e) {
                continue;
            }
            if (t.name().equals("main")) return t.uniqueID();
            if (best == null && t.threadGroup() != null && !t.threadGroup().name().equals("system")) best = t;
        }
        return best != null ? best.uniqueID() : 0;
    }

    private void threads(Map<String, Object> req) {
        List<Object> out = new ArrayList<>();
        if (vm != null) {
            for (ThreadReference t : vm.allThreads()) {
                out.add(Json.obj("id", t.uniqueID(), "name", t.name()));
            }
        }
        dap.respond(req, true, null, Json.obj("threads", out));
    }

    private void stackTrace(Map<String, Object> req, Map<String, Object> a) throws IncompatibleThreadStateException {
        ThreadReference t = thread(num(a.get("threadId")));
        int start = (int) num(a.get("startFrame"));
        int levels = a.get("levels") == null ? 0 : (int) num(a.get("levels"));
        int total = t.frameCount();
        int end = levels <= 0 ? total : Math.min(total, start + levels);
        List<Object> frames = new ArrayList<>();
        for (int i = start; i < end; i++) {
            Location loc = t.frame(i).location();
            String type = loc.declaringType().name();
            String simple = type.substring(type.lastIndexOf('.') + 1);
            Map<String, Object> f = Json.obj("id", handle(new FrameKey(t, i)), "name", simple + "." + loc.method().name(),
                    "line", Math.max(loc.lineNumber(), 0), "column", 1);
            Path src = sourceFor(loc);
            if (src != null) f.put("source", Json.obj("name", src.getFileName().toString(), "path", src.toString()));
            else f.put("presentationHint", "subtle");
            frames.add(f);
        }
        dap.respond(req, true, null, Json.obj("stackFrames", frames, "totalFrames", total));
    }

    private Path sourceFor(Location loc) {
        try {
            String rel = loc.sourcePath();
            for (Path root : sourcePaths) {
                Path p = root.resolve(rel);
                if (Files.isRegularFile(p)) return p;
            }
        } catch (AbsentInformationException ignored) {
        }
        return null;
    }

    private void scopes(Map<String, Object> req, Map<String, Object> a) {
        Object key = handles.get((int) num(a.get("frameId")));
        if (!(key instanceof FrameKey fk)) throw new IllegalArgumentException("frame không còn hợp lệ");
        dap.respond(req, true, null, Json.obj("scopes", List.of(
                Json.obj("name", "Local", "variablesReference", handle(new LocalsKey(fk.thread(), fk.index())), "expensive", false))));
    }

    private void variables(Map<String, Object> req, Map<String, Object> a) throws Exception {
        Object key = handles.get((int) num(a.get("variablesReference")));
        List<Object> out = new ArrayList<>();
        if (key instanceof LocalsKey lk) {
            StackFrame f = lk.thread().frame(lk.index());
            ObjectReference self = f.thisObject();
            if (self != null) out.add(variable("this", self, lk.thread()));
            try {
                List<LocalVariable> vars = f.visibleVariables();
                Map<LocalVariable, Value> vals = f.getValues(vars);
                for (LocalVariable v : vars) out.add(variable(v.name(), vals.get(v), lk.thread()));
            } catch (AbsentInformationException e) {
                // no local variable table: fall back to raw arguments
                List<Value> argv = f.getArgumentValues();
                for (int i = 0; i < argv.size(); i++) out.add(variable("arg" + i, argv.get(i), lk.thread()));
            }
        } else if (key instanceof ObjKey ok) {
            children(ok, a, out);
        }
        dap.respond(req, true, null, Json.obj("variables", out));
    }

    private void children(ObjKey ok, Map<String, Object> a, List<Object> out) {
        ObjectReference o = ok.obj();
        if (o instanceof ArrayReference arr) {
            int start = (int) num(a.get("start"));
            int count = a.get("count") == null ? Math.min(arr.length() - start, 1000) : (int) num(a.get("count"));
            List<Value> vals = arr.getValues(start, Math.max(0, Math.min(count, arr.length() - start)));
            for (int i = 0; i < vals.size(); i++) out.add(variable("[" + (start + i) + "]", vals.get(i), ok.thread()));
            return;
        }
        // collections and maps: show elements like IntelliJ, fields as a fallback
        try {
            if (isA(o.referenceType(), "java.util.Map")) {
                Value entries = invoke(o, "entrySet", ok.thread());
                ArrayReference arr = (ArrayReference) invoke((ObjectReference) entries, "toArray", ok.thread());
                int n = Math.min(arr.length(), 1000);
                for (Value e : arr.getValues(0, n)) {
                    ObjectReference entry = (ObjectReference) e;
                    Value k = invoke(entry, "getKey", ok.thread());
                    Value v = invoke(entry, "getValue", ok.thread());
                    out.add(variable(display(k), v, ok.thread()));
                }
                return;
            }
            if (isA(o.referenceType(), "java.util.Collection")) {
                ArrayReference arr = (ArrayReference) invoke(o, "toArray", ok.thread());
                int n = Math.min(arr.length(), 1000);
                List<Value> vals = arr.getValues(0, n);
                for (int i = 0; i < vals.size(); i++) out.add(variable("[" + i + "]", vals.get(i), ok.thread()));
                return;
            }
        } catch (Exception ignored) {
            // fall through to fields
        }
        List<Field> fields = new ArrayList<>();
        for (Field f : o.referenceType().allFields()) if (!f.isStatic()) fields.add(f);
        Map<Field, Value> vals = o.getValues(fields);
        for (Field f : fields) out.add(variable(f.name(), vals.get(f), ok.thread()));
    }

    private Value invoke(ObjectReference o, String name, ThreadReference thread) throws Exception {
        List<Method> ms = o.referenceType().methodsByName(name);
        Method m = ms.stream().filter(x -> x.argumentTypeNames().isEmpty()).findFirst().orElseThrow();
        return o.invokeMethod(thread, m, List.of(), ObjectReference.INVOKE_SINGLE_THREADED);
    }

    static boolean isA(ReferenceType t, String iface) {
        if (t.name().equals(iface)) return true;
        if (t instanceof ClassType ct) {
            for (var i : ct.allInterfaces()) if (i.name().equals(iface)) return true;
            return ct.superclass() != null && isA(ct.superclass(), iface);
        }
        if (t instanceof com.sun.jdi.InterfaceType it) {
            for (var i : it.superinterfaces()) if (isA(i, iface)) return true;
        }
        return false;
    }

    private Map<String, Object> variable(String name, Value v, ThreadReference thread) {
        Map<String, Object> m = Json.obj("name", name, "value", display(v), "type", v == null ? "null" : v.type().name(), "variablesReference", 0);
        if (v instanceof ObjectReference o && !(v instanceof StringReference) && !Eval.isBoxed(o)) {
            m.put("variablesReference", handle(new ObjKey(o, thread)));
            if (v instanceof ArrayReference arr) m.put("indexedVariables", arr.length());
        }
        return m;
    }

    /** IntelliJ-like value text: "text", 42, ArrayList (size=3), Foo@1234. */
    String display(Value v) {
        if (v == null) return "null";
        if (v instanceof StringReference s) {
            String t = s.value();
            return "\"" + (t.length() > 1000 ? t.substring(0, 1000) + "…" : t) + "\"";
        }
        if (v instanceof PrimitiveValue p) return p instanceof com.sun.jdi.CharValue c ? "'" + c.value() + "'" : p.toString();
        if (v instanceof ArrayReference a) {
            String type = a.type().name();
            return type.replace("[]", "") + "[" + a.length() + "]";
        }
        ObjectReference o = (ObjectReference) v;
        String type = o.referenceType().name();
        String simple = type.substring(type.lastIndexOf('.') + 1);
        if (Eval.isBoxed(o)) {
            Field f = o.referenceType().fieldByName("value");
            return display(o.getValue(f));
        }
        if (o.referenceType() instanceof ClassType ct && ct.isEnum()) {
            Field f = ct.fieldByName("name");
            Value n = f == null ? null : o.getValue(f);
            if (n instanceof StringReference s) return simple + "." + s.value();
        }
        Field size = o.referenceType().fieldByName("size");
        if (size != null && o.getValue(size) instanceof com.sun.jdi.IntegerValue iv) return simple + " (size=" + iv.value() + ")";
        return simple + "@" + o.uniqueID();
    }

    private void evaluate(Map<String, Object> req, Map<String, Object> a) throws Exception {
        String expr = str(a.get("expression"));
        Object key = a.get("frameId") == null ? null : handles.get((int) num(a.get("frameId")));
        if (!(key instanceof FrameKey fk)) throw new IllegalArgumentException("cần dừng ở một frame để tính biểu thức");
        Value v = new Eval(vm, fk.thread(), fk.index()).evaluate(expr);
        Map<String, Object> var = variable(expr, v, fk.thread());
        dap.respond(req, true, null, Json.obj("result", var.get("value"), "type", var.get("type"), "variablesReference", var.get("variablesReference")));
    }

    private void exceptionInfo(Map<String, Object> req, Map<String, Object> a) {
        ObjectReference ex = lastException.get(num(a.get("threadId")));
        if (ex == null) throw new IllegalArgumentException("thread này không dừng vì exception");
        String msg = null;
        Field f = ex.referenceType().fieldByName("detailMessage");
        if (f != null && ex.getValue(f) instanceof StringReference s) msg = s.value();
        dap.respond(req, true, null, Json.obj("exceptionId", ex.referenceType().name(), "description", msg == null ? "" : msg, "breakMode", "always"));
    }

    // ------------------------------------------------------------------ hot swap

    /** Pushes .class files changed since the last reload into the running JVM. */
    private void redefine(Map<String, Object> req) throws IOException {
        if (vm == null || !vm.canRedefineClasses()) throw new IllegalStateException("JVM này không hỗ trợ hot swap");
        long since = lastRedefine;
        long now = System.currentTimeMillis();
        Map<ReferenceType, byte[]> changes = new LinkedHashMap<>();
        Map<String, Path> seen = new HashMap<>();
        for (Path dir : outputDirs) {
            try (Stream<Path> files = Files.walk(dir)) {
                files.filter(p -> p.toString().endsWith(".class")).forEach(p -> {
                    try {
                        if (Files.getLastModifiedTime(p).toMillis() > since) {
                            String rel = dir.relativize(p).toString();
                            seen.put(rel.substring(0, rel.length() - 6).replace(File.separatorChar, '.'), p);
                        }
                    } catch (IOException ignored) {
                    }
                });
            }
        }
        for (Map.Entry<String, Path> e : seen.entrySet()) {
            for (ReferenceType t : vm.classesByName(e.getKey())) changes.put(t, Files.readAllBytes(e.getValue()));
        }
        List<Object> names = new ArrayList<>();
        changes.keySet().forEach(t -> names.add(t.name()));
        if (!changes.isEmpty()) vm.redefineClasses(changes);
        lastRedefine = now;
        dap.respond(req, true, null, Json.obj("changedClasses", names));
    }
}
