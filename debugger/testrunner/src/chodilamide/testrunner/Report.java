package chodilamide.testrunner;

import java.io.PrintWriter;
import java.io.StringWriter;

/** Writes one result line; the prefix never occurs in normal test output. */
final class Report {
    static synchronized void line(String json) {
        System.out.println("@@CDL " + json);
        System.out.flush();
    }

    static void finished(String cls, String method, String name, String status, long ms, Throwable t) {
        StringBuilder b = new StringBuilder("{\"event\":\"finished\",\"class\":").append(q(cls))
                .append(",\"method\":").append(q(method)).append(",\"name\":").append(q(name)).append(",\"status\":").append(q(status)).append(",\"ms\":").append(ms);
        if (t != null) {
            StringWriter sw = new StringWriter();
            t.printStackTrace(new PrintWriter(sw));
            b.append(",\"message\":").append(q(String.valueOf(t.getMessage()))).append(",\"trace\":").append(q(sw.toString()));
        }
        line(b.append('}').toString());
    }

    static void started(String cls, String name) {
        line("{\"event\":\"started\",\"class\":" + q(cls) + ",\"name\":" + q(name) + "}");
    }

    static String q(String s) {
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"': b.append("\\\""); break;
                case '\\': b.append("\\\\"); break;
                case '\n': b.append("\\n"); break;
                case '\r': break;
                case '\t': b.append("\\t"); break;
                default: if (c < 0x20) b.append(' '); else b.append(c);
            }
        }
        return b.append('"').toString();
    }
}
