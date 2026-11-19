public class C2Fields {
    long acc;
    int count;
    static long mul(long a, long b) { return a * b + 7; }
    long bump(long x) {
        acc += x;
        count++;
        return acc;
    }
    long run(long n) {
        for (long i = 0; i < n; i++) bump(mul(i, 3));
        return acc + count;
    }
    public static void main(String[] a) {
        C2Fields f = new C2Fields();
        for (int k = 0; k < 30000; k++) f.run(100);
        C2Fields g = new C2Fields();
        long r = g.run(1000);
        long expect = 0, c = 0;
        for (long i = 0; i < 1000; i++) { expect += i * 3 + 7; c++; }
        expect += c;
        System.out.println("fields=" + r + " expect=" + expect + " ok=" + (r == expect));
    }
}
