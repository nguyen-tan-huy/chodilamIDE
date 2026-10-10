package chodilamide.debugger;

import java.io.BufferedInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.Map;

/**
 * chodilamide-debugger: a Debug Adapter Protocol server backed directly by JDI - no jdtls.
 * Speaks DAP over stdin/stdout. One process = one debug session.
 */
public final class Main {
    private final OutputStream out;
    private int seq = 1;
    private final Session session;

    Main(OutputStream out) {
        this.out = out;
        this.session = new Session(this);
    }

    public static void main(String[] args) throws Exception {
        // keep System.out for the protocol only; anything else printed by accident goes to stderr
        OutputStream protocol = new java.io.FileOutputStream(java.io.FileDescriptor.out);
        System.setOut(System.err);
        Main m = new Main(protocol);
        m.loop(new BufferedInputStream(System.in));
    }

    private void loop(InputStream in) throws IOException {
        while (true) {
            String msg = readMessage(in);
            if (msg == null) break;
            @SuppressWarnings("unchecked")
            Map<String, Object> req = (Map<String, Object>) Json.parse(msg);
            if (!"request".equals(req.get("type"))) continue;
            try {
                session.handle(req);
            } catch (Throwable t) {
                String m = t.getMessage() != null ? t.getMessage() : t.toString();
                respond(req, false, m, null);
            }
        }
        session.shutdown();
        System.exit(0);
    }

    private static String readMessage(InputStream in) throws IOException {
        int length = -1;
        StringBuilder line = new StringBuilder();
        while (true) {
            int c = in.read();
            if (c < 0) return null;
            if (c == '\n') {
                String l = line.toString().trim();
                line.setLength(0);
                if (l.isEmpty()) {
                    if (length >= 0) break;
                    continue;
                }
                if (l.toLowerCase().startsWith("content-length:")) length = Integer.parseInt(l.substring(15).trim());
            } else line.append((char) c);
        }
        byte[] body = in.readNBytes(length);
        return new String(body, StandardCharsets.UTF_8);
    }

    synchronized void send(Map<String, Object> msg) {
        msg.put("seq", seq++);
        byte[] body = Json.write(msg).getBytes(StandardCharsets.UTF_8);
        try {
            ByteArrayOutputStream b = new ByteArrayOutputStream();
            b.write(("Content-Length: " + body.length + "\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            b.write(body);
            out.write(b.toByteArray());
            out.flush();
        } catch (IOException e) {
            System.exit(1);
        }
    }

    void respond(Map<String, Object> req, boolean ok, String message, Object body) {
        Map<String, Object> r = Json.obj("type", "response", "request_seq", req.get("seq"), "success", ok, "command", req.get("command"));
        if (message != null) r.put("message", message);
        if (body != null) r.put("body", body);
        send(r);
    }

    void event(String name, Object body) {
        Map<String, Object> e = Json.obj("type", "event", "event", name);
        if (body != null) e.put("body", body);
        send(e);
    }

    void output(String category, String text) {
        event("output", Json.obj("category", category, "output", text));
    }
}
