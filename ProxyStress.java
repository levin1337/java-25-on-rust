import java.lang.reflect.*;

public class ProxyStress {
    interface I1 { boolean f(int x); }
    interface I2 { int g(String s, long l); }
    interface I3 { Object h(int[] a, boolean b); }
    interface I4 { default int d() { return 5; } int e(); }
    interface I5<T> { T m(T t); }
    interface I6 { void v(); long w(double d); }

    static class H implements InvocationHandler {
        public Object invoke(Object p, Method m, Object[] a) {
            Class<?> r = m.getReturnType();
            if (r == boolean.class) return a != null && a.length > 0 && ((Integer)a[0]) > 0;
            if (r == int.class) return 42;
            if (r == long.class) return 7L;
            if (r == void.class) return null;
            return "x";
        }
    }

    public static void main(String[] args) throws Exception {
        InvocationHandler h = new H();
        I1 i1 = (I1) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I1.class}, h);
        System.out.println("i1=" + i1.f(3));
        I2 i2 = (I2) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I2.class}, h);
        System.out.println("i2=" + i2.g("s", 1L));
        I3 i3 = (I3) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I3.class}, h);
        System.out.println("i3=" + i3.h(new int[]{1}, true));
        I4 i4 = (I4) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I4.class}, h);
        System.out.println("i4=" + i4.d() + "," + i4.e());
        I5<String> i5 = (I5<String>) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I5.class}, h);
        System.out.println("i5=" + i5.m("q"));
        I6 i6 = (I6) Proxy.newProxyInstance(ProxyStress.class.getClassLoader(), new Class<?>[]{I6.class}, h);
        i6.v();
        System.out.println("i6=" + i6.w(1.5));
        System.out.println("done");
    }
}
