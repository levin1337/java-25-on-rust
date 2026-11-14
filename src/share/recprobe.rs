use std::cell::Cell;
thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}
pub struct Guard;
impl Guard {
    pub fn enter(site: &'static str) -> Self {
        let d = DEPTH.with(|c| {
            let v = c.get() + 1;
            c.set(v);
            v
        });
        if d == 5000 {
            eprintln!("[recprobe] depth 5000 at {site}");
        }
        if d > 20000 {
            panic!("recursion runaway at {site}");
        }
        Guard
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        DEPTH.with(|c| c.set(c.get().saturating_sub(1)));
    }
}
