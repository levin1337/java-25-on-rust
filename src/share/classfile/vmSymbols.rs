pub struct VmSymbols;

impl VmSymbols {
    pub const JAVA_LANG_OBJECT: &'static [u8] = b"java/lang/Object";
    pub const JAVA_LANG_CLASS: &'static [u8] = b"java/lang/Class";
    pub const JAVA_LANG_STRING: &'static [u8] = b"java/lang/String";
    pub const JAVA_LANG_THROWABLE: &'static [u8] = b"java/lang/Throwable";
    pub const JAVA_LANG_SYSTEM: &'static [u8] = b"java/lang/System";
    pub const JAVA_IO_PRINTSTREAM: &'static [u8] = b"java/io/PrintStream";

    pub const INIT: &'static [u8] = b"<init>";
    pub const CLINIT: &'static [u8] = b"<clinit>";
    pub const MAIN: &'static [u8] = b"main";
    pub const VOID_SIGNATURE: &'static [u8] = b"()V";
    pub const MAIN_SIGNATURE: &'static [u8] = b"([Ljava/lang/String;)V";
}
