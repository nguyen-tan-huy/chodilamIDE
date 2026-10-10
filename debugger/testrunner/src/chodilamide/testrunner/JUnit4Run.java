package chodilamide.testrunner;

import org.junit.runner.Description;
import org.junit.runner.JUnitCore;
import org.junit.runner.Request;
import org.junit.runner.notification.Failure;
import org.junit.runner.notification.RunListener;

import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** JUnit 4 classes, run with JUnitCore (only loaded when such classes are present). */
final class JUnit4Run {
    static int run(List<String> targets) throws Exception {
        JUnitCore core = new JUnitCore();
        final Map<Description, Long> started = new HashMap<>();
        final Set<Description> failed = new HashSet<>();
        final int[] failures = {0};
        core.addListener(new RunListener() {
            @Override
            public void testStarted(Description d) {
                started.put(d, System.currentTimeMillis());
                Report.started(d.getClassName(), d.getMethodName());
            }

            @Override
            public void testFailure(Failure f) {
                failed.add(f.getDescription());
                failures[0]++;
                Throwable t = f.getException();
                Report.finished(f.getDescription().getClassName(), String.valueOf(f.getDescription().getMethodName()), String.valueOf(f.getDescription().getMethodName()),
                        t instanceof AssertionError ? "failed" : "error", elapsed(started, f.getDescription()), t);
            }

            @Override
            public void testIgnored(Description d) {
                Report.finished(d.getClassName(), d.getMethodName(), d.getMethodName(), "skipped", 0, null);
            }

            @Override
            public void testFinished(Description d) {
                if (!failed.contains(d)) Report.finished(d.getClassName(), d.getMethodName(), d.getMethodName(), "passed", elapsed(started, d), null);
            }
        });
        for (String t : targets) {
            Class<?> c = Class.forName(t.contains("#") ? t.substring(0, t.indexOf('#')) : t);
            core.run(t.contains("#") ? Request.method(c, t.substring(t.indexOf('#') + 1)) : Request.aClass(c));
        }
        return failures[0];
    }

    private static long elapsed(Map<Description, Long> started, Description d) {
        Long s = started.get(d);
        return s == null ? 0 : System.currentTimeMillis() - s;
    }
}
