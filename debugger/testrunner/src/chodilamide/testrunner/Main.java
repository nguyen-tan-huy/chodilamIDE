package chodilamide.testrunner;

import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;

/**
 * Runs JUnit tests inside the target JVM (launched by chodilamide-debugger, so Run and Debug
 * share one path and no Maven is involved). Arguments: "a.b.Cls" or "a.b.Cls#method".
 * Results go to stdout as lines "@@CDL {json}" which the IDE turns into the test tree.
 * JUnit 5 (and anything on the JUnit Platform) runs through the Platform launcher; classes
 * using JUnit 4 annotations run through JUnitCore, because projects often have junit-4
 * without the vintage engine.
 */
public final class Main {
    public static void main(String[] args) throws Exception {
        List<String> platform = new ArrayList<>();
        List<String> junit4 = new ArrayList<>();
        for (String a : args) {
            String cls = a.contains("#") ? a.substring(0, a.indexOf('#')) : a;
            if (usesJUnit4(Class.forName(cls, false, Main.class.getClassLoader()))) junit4.add(a);
            else platform.add(a);
        }
        int failures = 0;
        if (!platform.isEmpty()) failures += PlatformRun.run(platform);
        if (!junit4.isEmpty()) failures += JUnit4Run.run(junit4);
        Report.line("{\"event\":\"done\",\"failures\":" + failures + "}");
        System.out.flush();
        System.exit(failures == 0 ? 0 : 1);
    }

    static boolean usesJUnit4(Class<?> c) {
        for (Class<?> k = c; k != null && k != Object.class; k = k.getSuperclass()) {
            for (Method m : k.getDeclaredMethods()) {
                for (java.lang.annotation.Annotation an : m.getAnnotations()) {
                    if (an.annotationType().getName().equals("org.junit.Test")) return true;
                }
            }
        }
        return false;
    }

    /** Finds a method by name (the IDE only knows names; parameterized tests have params). */
    static Method method(Class<?> c, String name) {
        for (Class<?> k = c; k != null; k = k.getSuperclass()) {
            for (Method m : k.getDeclaredMethods()) if (m.getName().equals(name)) return m;
        }
        throw new IllegalArgumentException("không có method " + name + " trong " + c.getName());
    }
}
