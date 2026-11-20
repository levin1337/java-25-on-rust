import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;

public class BmhProbe {
    public static void main(String[] args) throws Throwable {
        MethodHandles.Lookup lookup = MethodHandles.lookup();

        // bindTo binds the RECEIVER: "hello".startsWith(arg)
        MethodHandle mh = lookup.findVirtual(String.class, "startsWith", MethodType.methodType(boolean.class, String.class));
        MethodHandle bound = mh.bindTo("hello");
        boolean ok = false;
        for (int i = 0; i < 200000; i++) {
            ok = (boolean) bound.invokeExact("he");
        }
        System.out.println("bindTo startsWith=" + ok + " expect=true");

        // two bound args via insertArguments -> deeper species
        MethodHandle two = lookup.findStatic(Math.class, "max", MethodType.methodType(int.class, int.class, int.class));
        MethodHandle b2 = MethodHandles.insertArguments(two, 0, 40);
        MethodHandle b3 = MethodHandles.insertArguments(b2, 0, 7);
        int m = 0;
        for (int i = 0; i < 100000; i++) {
            m = (int) b3.invokeExact();
        }
        System.out.println("max=" + m + " expect=40");

        // lambda via invokedynamic (real path)
        java.util.function.IntUnaryOperator f = x -> x * 2;
        int r = 0;
        for (int i = 0; i < 50000; i++) r = f.applyAsInt(i);
        System.out.println("lambda=" + r + " expect=99998");

        // method ref via invokedynamic
        java.util.function.Function<String, Integer> g = String::length;
        int gl = 0;
        for (int i = 0; i < 50000; i++) gl = g.apply("abc");
        System.out.println("methodref=" + gl + " expect=3");

        System.out.println("BMH OK");
    }
}
