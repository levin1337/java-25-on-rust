public class C2Catch {
    // Matching typed catch: ArithmeticException caught by type.
    static long caught(long n) {
        long s = 0;
        for (long i = 0; i < n; i++) {
            try {
                s += 100 / (i - n + 1); // throws once at i = n-1
            } catch (ArithmeticException e) {
                s += 7;
            }
        }
        return s;
    }

    // Non-matching typed catch: NullPointerException skips the typed
    // handler and lands in catch-all.
    static long fallsThrough(long n) {
        long s = 0;
        String x = null;
        for (long i = 0; i < n; i++) {
            try {
                s += x.length();
            } catch (ArithmeticException e) {
                s += 1000; // must NOT run
            } catch (RuntimeException e) {
                s += 3;
            }
        }
        return s;
    }

    // Exception propagates out of the typed catch when no handler
    // matches: caller's catch(RuntimeException) gets it.
    static long propagates(long n) {
        long s = 0;
        try {
            for (long i = 0; i < n; i++) {
                try {
                    if (i == n - 1) throw new IllegalStateException();
                    s += i;
                } catch (ArithmeticException e) {
                    s += 50; // must NOT run
                }
            }
        } catch (RuntimeException e) {
            s += 11;
        }
        return s;
    }

    public static void main(String[] a) {
        for (int k = 0; k < 800; k++) {
            caught(40); fallsThrough(40); propagates(40);
        }
        long n = 100;
        long c1 = caught(n);
        // expected: sum_{i=0}^{n-1} 100/(i+1)... careful: i-n+1 = i-(n-1)
        // at i=n-1 -> /0 throws; other i give 100/(negative) trunc toward 0.
        // compute directly:
        long e1 = 0;
        for (long i = 0; i < n; i++) {
            try { e1 += 100 / (i - n + 1); }
            catch (ArithmeticException e) { e1 += 7; }
        }
        long c2 = fallsThrough(n);
        long e2 = 3L * n; // NPE -> RuntimeException catch adds 3 each iter
        long c3 = propagates(n);
        long e3 = 0;
        for (long i = 0; i < n - 1; i++) e3 += i;
        e3 += 11;
        System.out.println("caught=" + c1 + " expect=" + e1 + " ok=" + (c1 == e1));
        System.out.println("falls=" + c2 + " expect=" + e2 + " ok=" + (c2 == e2));
        System.out.println("prop=" + c3 + " expect=" + e3 + " ok=" + (c3 == e3));
    }
}
