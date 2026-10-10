package chodilamide.testrunner;

import org.junit.platform.engine.DiscoverySelector;
import org.junit.platform.engine.TestExecutionResult;
import org.junit.platform.engine.support.descriptor.MethodSource;
import org.junit.platform.launcher.Launcher;
import org.junit.platform.launcher.LauncherDiscoveryRequest;
import org.junit.platform.launcher.TestExecutionListener;
import org.junit.platform.launcher.TestIdentifier;
import org.junit.platform.launcher.core.LauncherFactory;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.platform.engine.discovery.DiscoverySelectors.selectClass;
import static org.junit.platform.engine.discovery.DiscoverySelectors.selectMethod;
import static org.junit.platform.launcher.core.LauncherDiscoveryRequestBuilder.request;

/** JUnit Platform (JUnit 5 and other engines). Only API that exists since Platform 1.0. */
final class PlatformRun {
    static int run(List<String> targets) throws Exception {
        List<DiscoverySelector> selectors = new ArrayList<>();
        for (String t : targets) {
            if (t.contains("#")) {
                Class<?> c = Class.forName(t.substring(0, t.indexOf('#')));
                selectors.add(selectMethod(c, Main.method(c, t.substring(t.indexOf('#') + 1))));
            } else selectors.add(selectClass(Class.forName(t)));
        }
        LauncherDiscoveryRequest req = request().selectors(selectors).build();
        Launcher launcher = LauncherFactory.create();
        final int[] failures = {0};
        final Map<String, Long> startedAt = new HashMap<>();
        launcher.execute(req, new TestExecutionListener() {
            @Override
            public void executionStarted(TestIdentifier id) {
                if (!id.isTest()) return;
                startedAt.put(id.getUniqueId(), System.currentTimeMillis());
                Report.started(cls(id), id.getDisplayName());
            }

            @Override
            public void executionSkipped(TestIdentifier id, String reason) {
                if (id.isTest()) Report.finished(cls(id), method(id), id.getDisplayName(), "skipped", 0, null);
            }

            @Override
            public void executionFinished(TestIdentifier id, TestExecutionResult r) {
                if (!id.isTest()) {
                    // a class-level failure (e.g. @BeforeAll / Spring context) fails every test in it
                    if (r.getStatus() != TestExecutionResult.Status.SUCCESSFUL && id.isContainer()) {
                        failures[0]++;
                        Report.finished(cls(id), "", id.getDisplayName(), "error", 0, r.getThrowable().orElse(null));
                    }
                    return;
                }
                long ms = System.currentTimeMillis() - startedAt.getOrDefault(id.getUniqueId(), System.currentTimeMillis());
                Throwable t = r.getThrowable().orElse(null);
                String status;
                switch (r.getStatus()) {
                    case SUCCESSFUL: status = "passed"; break;
                    case ABORTED: status = "skipped"; break;
                    default:
                        failures[0]++;
                        status = t instanceof AssertionError ? "failed" : "error";
                }
                Report.finished(cls(id), method(id), id.getDisplayName(), status, ms, t);
            }
        });
        return failures[0];
    }

    private static String method(TestIdentifier id) {
        return id.getSource().filter(s -> s instanceof MethodSource).map(s -> ((MethodSource) s).getMethodName()).orElse("");
    }

    private static String cls(TestIdentifier id) {
        return id.getSource().filter(s -> s instanceof MethodSource).map(s -> ((MethodSource) s).getClassName())
                .orElseGet(() -> id.getLegacyReportingName());
    }
}
