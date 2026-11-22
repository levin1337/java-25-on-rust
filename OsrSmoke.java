public class OsrSmoke {
    static long hot(long n) {
        long s = 0;
        for (long i = 0; i < n; i++) {
            s += (i * 2654435761L) >>> 13;
            s ^= s << 7;
        }
        return s;
    }
    public static void main(String[] a) {
        long r = hot(50_000_000L);
        System.out.println("osr=" + r);
    }
}
