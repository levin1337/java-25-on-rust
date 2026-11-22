public class NettyProbe {
    public static void main(String[] a) throws Exception {
        Class<?> c = Class.forName("io.netty.util.internal.PlatformDependent");
        System.out.println("loaded=" + c.getName());
    }
}
