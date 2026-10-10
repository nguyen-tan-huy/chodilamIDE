package chodilamide.debugger;

import com.sun.jdi.AbsentInformationException;
import com.sun.jdi.ArrayReference;
import com.sun.jdi.BooleanValue;
import com.sun.jdi.ClassType;
import com.sun.jdi.Field;
import com.sun.jdi.LocalVariable;
import com.sun.jdi.Method;
import com.sun.jdi.ObjectReference;
import com.sun.jdi.PrimitiveValue;
import com.sun.jdi.ReferenceType;
import com.sun.jdi.StackFrame;
import com.sun.jdi.StringReference;
import com.sun.jdi.ThreadReference;
import com.sun.jdi.Value;
import com.sun.jdi.VirtualMachine;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;

/**
 * Small Java expression evaluator for conditions, watches and hovers:
 * locals, this/fields (a.b.c), array index, no-arg and literal-arg method calls,
 * literals, ! - + * / %, comparisons, == != (strings compared by value), && ||.
 */
final class Eval {
    private final VirtualMachine vm;
    private final ThreadReference thread;
    private final int frameIndex;
    private List<String> toks;
    private int pos;

    /** Frames are re-fetched on every use: invoking a method resumes the thread briefly,
     *  which invalidates StackFrame objects taken before the call. */
    Eval(VirtualMachine vm, ThreadReference thread, int frameIndex) {
        this.vm = vm;
        this.thread = thread;
        this.frameIndex = frameIndex;
    }

    private StackFrame frame() {
        try {
            return thread.frame(frameIndex);
        } catch (com.sun.jdi.IncompatibleThreadStateException e) {
            throw new IllegalStateException("thread không còn dừng");
        }
    }

    private static final Set<String> BOXED = Set.of("java.lang.Integer", "java.lang.Long", "java.lang.Short", "java.lang.Byte",
            "java.lang.Double", "java.lang.Float", "java.lang.Boolean", "java.lang.Character");

    static boolean isBoxed(ObjectReference o) {
        return BOXED.contains(o.referenceType().name());
    }

    Value evaluate(String expr) throws Exception {
        toks = tokenize(expr);
        pos = 0;
        Object v = or();
        if (pos != toks.size()) throw new IllegalArgumentException("thừa ký tự: " + toks.get(pos));
        return mirror(v);
    }

    // ---- values: JDI Value, or plain Java (Long, Double, Boolean, String, Character, null)

    static Object toJava(Value v) {
        if (v == null) return null;
        if (v instanceof StringReference s) return s.value();
        if (v instanceof BooleanValue b) return b.value();
        if (v instanceof com.sun.jdi.CharValue c) return c.value();
        if (v instanceof com.sun.jdi.DoubleValue d) return d.value();
        if (v instanceof com.sun.jdi.FloatValue f) return (double) f.value();
        if (v instanceof PrimitiveValue p) return p.longValue();
        if (v instanceof ObjectReference o && isBoxed(o)) return toJava(o.getValue(o.referenceType().fieldByName("value")));
        return v;
    }

    private Value mirror(Object v) {
        if (v == null || v instanceof Value) return (Value) v;
        if (v instanceof String s) return vm.mirrorOf(s);
        if (v instanceof Boolean b) return vm.mirrorOf(b);
        if (v instanceof Character c) return vm.mirrorOf(c);
        if (v instanceof Double d) return vm.mirrorOf(d);
        if (v instanceof Long l) return (l >= Integer.MIN_VALUE && l <= Integer.MAX_VALUE) ? vm.mirrorOf(l.intValue()) : vm.mirrorOf(l);
        throw new IllegalArgumentException("không đổi được giá trị " + v);
    }

    private static Object java(Object v) {
        return v instanceof Value val ? toJava(val) : v;
    }

    // ---- parser

    private String peek() { return pos < toks.size() ? toks.get(pos) : ""; }

    private boolean eat(String t) {
        if (peek().equals(t)) { pos++; return true; }
        return false;
    }

    private Object or() throws Exception {
        Object l = and();
        while (eat("||")) {
            if (Boolean.TRUE.equals(java(l))) { and(); l = true; }
            else l = Boolean.TRUE.equals(java(and()));
        }
        return l;
    }

    private Object and() throws Exception {
        Object l = equality();
        while (eat("&&")) {
            Object r = equality();
            l = Boolean.TRUE.equals(java(l)) && Boolean.TRUE.equals(java(r));
        }
        return l;
    }

    private Object equality() throws Exception {
        Object l = relational();
        while (peek().equals("==") || peek().equals("!=")) {
            boolean eq = toks.get(pos++).equals("==");
            Object r = relational();
            boolean same = equalValues(l, r);
            l = eq == same;
        }
        return l;
    }

    private static boolean equalValues(Object a, Object b) {
        Object x = java(a), y = java(b);
        if (x == null || y == null) return x == y;
        if (x instanceof Number n && y instanceof Number m) return n.doubleValue() == m.doubleValue();
        if (x instanceof Character c && y instanceof Number m) return c == m.longValue();
        if (x instanceof ObjectReference o && y instanceof ObjectReference p) return o.equals(p);
        return x.equals(y);
    }

    private Object relational() throws Exception {
        Object l = additive();
        while (List.of("<", ">", "<=", ">=").contains(peek())) {
            String op = toks.get(pos++);
            double a = number(additive()), b0 = number(l);
            l = switch (op) {
                case "<" -> b0 < a;
                case ">" -> b0 > a;
                case "<=" -> b0 <= a;
                default -> b0 >= a;
            };
        }
        return l;
    }

    private static double number(Object v) {
        Object x = java(v);
        if (x instanceof Number n) return n.doubleValue();
        if (x instanceof Character c) return c;
        throw new IllegalArgumentException("không phải số: " + x);
    }

    private Object additive() throws Exception {
        Object l = multiplicative();
        while (peek().equals("+") || peek().equals("-")) {
            String op = toks.get(pos++);
            Object r = multiplicative();
            Object a = java(l), b = java(r);
            if (op.equals("+") && (a instanceof String || b instanceof String)) l = str(a) + str(b);
            else l = arith(op, a, b);
        }
        return l;
    }

    private String str(Object v) {
        if (v instanceof ObjectReference o) return "@" + o.uniqueID();
        return String.valueOf(v);
    }

    private Object multiplicative() throws Exception {
        Object l = unary();
        while (peek().equals("*") || peek().equals("/") || peek().equals("%")) {
            String op = toks.get(pos++);
            l = arith(op, java(l), java(unary()));
        }
        return l;
    }

    private static Object arith(String op, Object a, Object b) {
        if (a instanceof Long x && b instanceof Long y) {
            return switch (op) {
                case "+" -> x + y;
                case "-" -> x - y;
                case "*" -> x * y;
                case "/" -> x / y;
                default -> x % y;
            };
        }
        double x = number(a), y = number(b);
        return switch (op) {
            case "+" -> x + y;
            case "-" -> x - y;
            case "*" -> x * y;
            case "/" -> x / y;
            default -> x % y;
        };
    }

    private Object unary() throws Exception {
        if (eat("!")) return !Boolean.TRUE.equals(java(unary()));
        if (eat("-")) return arith("-", 0L, java(unary()));
        return postfix();
    }

    private Object postfix() throws Exception {
        Object v = primary();
        while (true) {
            if (eat(".")) {
                String name = toks.get(pos++);
                if (eat("(")) v = call(v, name, args());
                else v = member(v, name);
            } else if (eat("[")) {
                long idx = (long) number(or());
                expect("]");
                Object arr = v;
                if (!(arr instanceof ArrayReference a)) throw new IllegalArgumentException("không phải mảng");
                v = a.getValue((int) idx);
            } else return v;
        }
    }

    private List<Object> args() throws Exception {
        List<Object> out = new ArrayList<>();
        if (eat(")")) return out;
        do out.add(or()); while (eat(","));
        expect(")");
        return out;
    }

    private void expect(String t) {
        if (!eat(t)) throw new IllegalArgumentException("thiếu '" + t + "'");
    }

    private Object primary() throws Exception {
        String t = toks.get(pos++);
        if (t.equals("(")) {
            Object v = or();
            expect(")");
            return v;
        }
        if (t.startsWith("\"")) return t.substring(1, t.length() - 1).replace("\\\"", "\"").replace("\\n", "\n");
        if (t.startsWith("'")) return t.charAt(1);
        if (Character.isDigit(t.charAt(0))) {
            String n = t.replaceAll("[lLdDfF_]$", "").replace("_", "");
            return n.contains(".") ? (Object) Double.parseDouble(n) : (Object) Long.parseLong(n);
        }
        switch (t) {
            case "true": return true;
            case "false": return false;
            case "null": return null;
            case "this": return frame().thisObject();
        }
        if (peek().equals("(")) { // method on this / static in current class
            pos++;
            List<Object> a = args();
            ObjectReference self = frame().thisObject();
            return self != null ? call(self, t, a) : callStatic(frame().location().declaringType(), t, a);
        }
        return name(t);
    }

    private Object name(String n) throws Exception {
        try {
            StackFrame f = frame();
            LocalVariable lv = f.visibleVariableByName(n);
            if (lv != null) return f.getValue(lv);
        } catch (AbsentInformationException ignored) {
        }
        ObjectReference self = frame().thisObject();
        if (self != null) {
            Field f = self.referenceType().fieldByName(n);
            if (f != null) return self.getValue(f);
        }
        ReferenceType here = frame().location().declaringType();
        Field sf = here.fieldByName(n);
        if (sf != null && sf.isStatic()) return here.getValue(sf);
        // a class name for static access: fully qualified, same package, or java.lang
        String pkg = here.name().contains(".") ? here.name().substring(0, here.name().lastIndexOf('.') + 1) : "";
        for (String cand : List.of(n, pkg + n, "java.lang." + n)) {
            List<ReferenceType> ts = vm.classesByName(cand);
            if (!ts.isEmpty()) return ts.get(0);
        }
        throw new IllegalArgumentException("không biết '" + n + "'");
    }

    private Object member(Object target, String name) {
        if (target instanceof ArrayReference a && name.equals("length")) return (long) a.length();
        if (target instanceof ReferenceType t) {
            Field f = t.fieldByName(name);
            if (f == null) throw new IllegalArgumentException("không có field static " + name);
            return t.getValue(f);
        }
        if (!(target instanceof ObjectReference o)) throw new IllegalArgumentException("không truy cập được ." + name + " trên " + target);
        Field f = o.referenceType().fieldByName(name);
        if (f == null) throw new IllegalArgumentException(o.referenceType().name() + " không có field " + name);
        return o.getValue(f);
    }

    private Object call(Object target, String name, List<Object> args) throws Exception {
        if (target instanceof ReferenceType t) return callStatic(t, name, args);
        if (target instanceof String s) target = vm.mirrorOf(s);
        if (!(target instanceof ObjectReference o)) throw new IllegalArgumentException("không gọi được ." + name + "() trên " + target);
        Method m = pick(o.referenceType().methodsByName(name), args.size(), name);
        return o.invokeMethod(thread, m, mirrors(args), ObjectReference.INVOKE_SINGLE_THREADED);
    }

    private Object callStatic(ReferenceType t, String name, List<Object> args) throws Exception {
        if (!(t instanceof ClassType ct)) throw new IllegalArgumentException("không gọi static trên " + t.name());
        Method m = pick(ct.methodsByName(name), args.size(), name);
        return ct.invokeMethod(thread, m, mirrors(args), ObjectReference.INVOKE_SINGLE_THREADED);
    }

    private static Method pick(List<Method> ms, int argc, String name) {
        return ms.stream().filter(m -> m.argumentTypeNames().size() == argc).findFirst()
                .orElseThrow(() -> new IllegalArgumentException("không có method " + name + " với " + argc + " tham số"));
    }

    private List<Value> mirrors(List<Object> args) {
        List<Value> out = new ArrayList<>();
        for (Object a : args) out.add(mirror(a));
        return out;
    }

    // ---- tokenizer

    static List<String> tokenize(String s) {
        List<String> out = new ArrayList<>();
        int i = 0;
        while (i < s.length()) {
            char c = s.charAt(i);
            if (Character.isWhitespace(c)) { i++; continue; }
            if (Character.isJavaIdentifierStart(c)) {
                int j = i;
                while (j < s.length() && Character.isJavaIdentifierPart(s.charAt(j))) j++;
                out.add(s.substring(i, j));
                i = j;
            } else if (Character.isDigit(c)) {
                int j = i;
                while (j < s.length() && (Character.isLetterOrDigit(s.charAt(j)) || s.charAt(j) == '.' || s.charAt(j) == '_')) j++;
                out.add(s.substring(i, j));
                i = j;
            } else if (c == '"' || c == '\'') {
                int j = i + 1;
                while (j < s.length() && s.charAt(j) != c) j += s.charAt(j) == '\\' ? 2 : 1;
                out.add(s.substring(i, Math.min(j + 1, s.length())));
                i = j + 1;
            } else {
                String two = i + 1 < s.length() ? s.substring(i, i + 2) : "";
                if (List.of("==", "!=", "<=", ">=", "&&", "||").contains(two)) { out.add(two); i += 2; }
                else { out.add(String.valueOf(c)); i++; }
            }
        }
        return out;
    }
}
