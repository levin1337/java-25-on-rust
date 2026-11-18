public class C2Smoke {
    static long burn(long n) {
        long s = 0;
        for (long i = 0; i < n; i++) {
            s += (i * 2654435761L) >>> 13;
            s ^= s << 7;
            if ((i & 1023L) == 0L) s += i;
        }
        return s;
    }
    static long sumLoop(long n) {
        long s = 0;
        for (long i = 0; i < n; i++) s += i * 3 + 1;
        return s;
    }
    public static void main(String[] a) {
        long expect = burn(10);
        // warm up burn so C2 compiles it, then verify repeated calls
        for (int k = 0; k < 200000; k++) burn(10);
        long got = burn(10);
        System.out.println("burn=" + got + " expect=" + expect + " ok=" + (got == expect));
        for (int k = 0; k < 200000; k++) sumLoop(1000);
        System.out.println("sum=" + sumLoop(1000000));
    }
}
