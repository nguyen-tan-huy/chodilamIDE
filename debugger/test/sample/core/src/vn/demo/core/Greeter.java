package vn.demo.core;

import java.util.ArrayList;
import java.util.List;

public class Greeter {
    private final String prefix = "Xin chào";
    private int count;

    public String greet(String name) {
        count++;
        String msg = prefix + " " + name;
        List<String> parts = new ArrayList<>(List.of(prefix, name));
        return msg + " #" + count + " (" + parts.size() + ")";
    }
}
