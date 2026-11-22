public class ToLowerCrash {
    public static void main(String[] a) {
        String s = "HELLO WORLD FOO BAR BAZ 0123456789";
        long sum = 0;
        for (int i = 0; i < 60_000; i++) {
            String t = s.toLowerCase(java.util.Locale.ROOT);
            sum += t.length() + t.charAt(0);
        }
        System.out.println("sum=" + sum);
    }
}
