public class C2Sig {
    static double dsum(double a, double b) { return a * 1.5 + b; }
    static float fsum(float a, float b) { return a * 2.0f + b; }
    static long mix(long x, double d, int i, float f, long y) {
        return x + (long)(d * 10) + i * 3 + (long)(f * 100) + y;
    }
    static String id(String s) { return s; }
    static int alen(int[] a) { return a.length; }
    public static void main(String[] a) {
        for (int k = 0; k < 25000; k++) {
            dsum(1.5, 2.5); fsum(1.0f, 2.0f); mix(1, 2.0, 3, 4.0f, 5); id("x"); alen(new int[3]);
        }
        double d = dsum(2.0, 3.0);          // 2*1.5+3 = 6
        float f = fsum(2.0f, 1.0f);         // 2*2+1 = 5
        long m = mix(10, 1.5, 4, 0.5f, 20); // 10+15+12+50+20 = 107
        String s = id("hello");
        int n = alen(new int[7]);
        System.out.println("d=" + d + " f=" + f + " m=" + m + " s=" + s + " n=" + n);
    }
}
