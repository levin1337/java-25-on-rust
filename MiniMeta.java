import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.util.function.Supplier;

public class MiniMeta {
    public static void main(String[] args) throws Throwable {
        MethodHandles.Lookup lookup = MethodHandles.lookup();
        Supplier<?> s = (Supplier<?>) java.lang.invoke.LambdaMetafactory.metafactory(
            lookup, "get",
            MethodType.methodType(Supplier.class),
            MethodType.methodType(Object.class),
            lookup.findStatic(MiniMeta.class, "make", MethodType.methodType(Object.class)),
            MethodType.methodType(Object.class)).getTarget().invokeExact();
        System.out.println("META=" + s.get());
    }
    static Object make() { return "ok"; }
}
