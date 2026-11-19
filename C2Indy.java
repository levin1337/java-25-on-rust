public class C2Indy {
    // String concatenation compiles to invokedynamic (StringConcatFactory).
    static String concat(long i, long j) {
        return "v=" + i + "," + j + "!";
    }

    // Lambda metafactory invokedynamic.
    static long apply(long x) {
        java.util.function.LongUnaryOperator f = v -> v * 3 + 1;
        return f.applyAsLong(x);
    }

    public static void main(String[] a) {
        long sum = 0;
        for (int k = 0; k < 20000; k++) {
            String s = concat(k, k + 1);
            if (!s.equals("v=" + k + "," + (k + 1) + "!")) {
                System.out.println("BAD concat: " + s);
                return;
            }
            sum += apply(k);
        }
        // spot-check apply: f(x) = 3x+1
        long expect = 0;
        for (int k = 0; k < 20000; k++) expect += 3L * k + 1;
        System.out.println("indy ok sum=" + sum + " expect=" + expect + " ok=" + (sum == expect));
    }
}
