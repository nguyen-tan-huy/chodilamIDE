package vn.demo.app;

import java.util.LinkedHashMap;
import java.util.Map;
import vn.demo.core.Greeter;

public class Main {
    public static void main(String[] args) throws Exception {
        Greeter g = new Greeter();
        Map<String, Integer> ages = new LinkedHashMap<>();
        ages.put("Huy", 30);
        ages.put("Lan", 25);
        ages.put("Minh", 41);
        for (String name : ages.keySet()) {
            System.out.println(g.greet(name));
        }
        Thread.sleep(Long.parseLong(System.getenv().getOrDefault("DEMO_SLEEP", "0")));
        System.out.println("xong " + System.getenv("DEMO_ENV"));
    }
}
