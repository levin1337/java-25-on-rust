/*
 * Port of OpenJDK 25 HotSpot Epsilon GC arguments
 * (share/gc/epsilon/epsilonArguments.hpp / epsilonArguments.cpp,
 *  flags from share/gc/epsilon/epsilon_globals.hpp)
 *
 * Parses -Xms/-Xmx and the Epsilon-specific product flags into a
 * heap configuration.
 */

pub const DEFAULT_MAX_TLAB_SIZE: usize = 4 * 1024 * 1024;
pub const DEFAULT_UPDATE_COUNTERS_STEP: usize = 1024 * 1024;
pub const DEFAULT_PRINT_HEAP_STEPS: usize = 20;
pub const DEFAULT_MIN_HEAP_EXPAND: usize = 128 * 1024 * 1024;
pub const DEFAULT_TLAB_ELASTICITY: f64 = 1.1;
pub const DEFAULT_TLAB_DECAY_TIME_MS: u64 = 1000;

#[derive(Clone, Debug)]
pub struct EpsilonHeapConfig {
    pub min_heap_size: usize,
    pub max_heap_size: usize,
    pub max_tlab_size: usize,
    pub update_counters_step: usize,
    pub print_heap_steps: usize,
    pub min_heap_expand: usize,
    pub elastic_tlab: bool,
    pub elastic_tlab_decay: bool,
    pub tlab_elasticity: f64,
    pub tlab_decay_time_ms: u64,
}

impl Default for EpsilonHeapConfig {
    fn default() -> Self {
        Self {
            min_heap_size: 0,
            max_heap_size: 256 * 1024 * 1024,
            max_tlab_size: DEFAULT_MAX_TLAB_SIZE,
            update_counters_step: DEFAULT_UPDATE_COUNTERS_STEP,
            print_heap_steps: DEFAULT_PRINT_HEAP_STEPS,
            min_heap_expand: DEFAULT_MIN_HEAP_EXPAND,
            elastic_tlab: true,
            elastic_tlab_decay: true,
            tlab_elasticity: DEFAULT_TLAB_ELASTICITY,
            tlab_decay_time_ms: DEFAULT_TLAB_DECAY_TIME_MS,
        }
    }
}

#[derive(Debug)]
pub struct EpsilonArguments {
    pub config: EpsilonHeapConfig,
}

impl EpsilonArguments {
    pub fn new() -> Self {
        Self {
            config: EpsilonHeapConfig::default(),
        }
    }

    pub fn conservative_max_heap_alignment(&self) -> usize {
        8
    }

    pub fn parse_flag(&mut self, flag: &str) -> bool {
        match flag {
            "-XX:+EpsilonElasticTLAB" => self.config.elastic_tlab = true,
            "-XX:-EpsilonElasticTLAB" => self.config.elastic_tlab = false,
            "-XX:+EpsilonElasticTLABDecay" => self.config.elastic_tlab_decay = true,
            "-XX:-EpsilonElasticTLABDecay" => self.config.elastic_tlab_decay = false,
            _ => return false,
        }
        true
    }

    pub fn parse_size_flag(&mut self, flag: &str) -> bool {
        if let Some(v) = parse_kv(flag, "-XX:EpsilonMaxTLABSize=") {
            self.config.max_tlab_size = v;
            return true;
        }
        if let Some(v) = parse_kv(flag, "-XX:EpsilonMinHeapExpand=") {
            self.config.min_heap_expand = v;
            return true;
        }
        if let Some(v) = parse_kv(flag, "-XX:EpsilonUpdateCountersStep=") {
            self.config.update_counters_step = v;
            return true;
        }
        if let Some(v) = parse_kv(flag, "-XX:EpsilonPrintHeapSteps=") {
            self.config.print_heap_steps = v;
            return true;
        }
        if let Some(v) = parse_kv(flag, "-XX:EpsilonTLABDecayTime=") {
            self.config.tlab_decay_time_ms = v as u64;
            return true;
        }
        if let Some(v) = parse_kv(flag, "-XX:EpsilonTLABElasticity=") {
            self.config.tlab_elasticity = v as f64;
            return true;
        }
        false
    }

    pub fn parse_heap_size(&mut self, flag: &str) -> bool {
        if let Some(v) = parse_size(flag, "-Xms") {
            self.config.min_heap_size = v;
            return true;
        }
        if let Some(v) = parse_size(flag, "-Xmx") {
            self.config.max_heap_size = v;
            return true;
        }
        if let Some(v) = parse_size(flag, "-XX:InitialHeapSize=") {
            self.config.min_heap_size = v;
            return true;
        }
        if let Some(v) = parse_size(flag, "-XX:MaxHeapSize=") {
            self.config.max_heap_size = v;
            return true;
        }
        false
    }

    pub fn initialize(&mut self) {
        if self.config.min_heap_size > self.config.max_heap_size {
            self.config.min_heap_size = self.config.max_heap_size;
        }
        self.config.max_tlab_size = self.config.max_tlab_size.max(1);
        self.config.update_counters_step = self.config.update_counters_step.max(1);
        self.config.min_heap_expand = self.config.min_heap_expand.max(1);
    }
}

impl Default for EpsilonArguments {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_size(flag: &str, prefix: &str) -> Option<usize> {
    let rest = flag.strip_prefix(prefix)?;
    let (num, mult) = if let Some(r) = rest.strip_suffix('g') {
        (r, 1usize << 30)
    } else if let Some(r) = rest.strip_suffix('G') {
        (r, 1usize << 30)
    } else if let Some(r) = rest.strip_suffix('m') {
        (r, 1usize << 20)
    } else if let Some(r) = rest.strip_suffix('M') {
        (r, 1usize << 20)
    } else if let Some(r) = rest.strip_suffix('k') {
        (r, 1usize << 10)
    } else if let Some(r) = rest.strip_suffix('K') {
        (r, 1usize << 10)
    } else {
        (rest, 1)
    };
    let n: usize = num.trim().parse().ok()?;
    Some(n.saturating_mul(mult))
}

fn parse_kv<T: std::str::FromStr>(flag: &str, prefix: &str) -> Option<T> {
    let rest = flag.strip_prefix(prefix)?;
    let (num, mult) = if let Some(r) = rest.strip_suffix('m') {
        (r, 1usize)
    } else {
        (rest, 1usize)
    };
    let _ = mult;
    num.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_heap_sizes() {
        let mut args = EpsilonArguments::new();
        assert!(args.parse_heap_size("-Xmx512m"));
        assert!(args.parse_heap_size("-Xms64m"));
        args.initialize();
        assert_eq!(args.config.max_heap_size, 512 << 20);
        assert_eq!(args.config.min_heap_size, 64 << 20);
    }

    #[test]
    fn parse_epsilon_flags() {
        let mut args = EpsilonArguments::new();
        assert!(args.parse_flag("-XX:-EpsilonElasticTLAB"));
        assert!(!args.config.elastic_tlab);
        assert!(args.parse_size_flag("-XX:EpsilonMaxTLABSize=8388608"));
        assert_eq!(args.config.max_tlab_size, 8 << 20);
        assert!(args.parse_size_flag("-XX:EpsilonMinHeapExpand=67108864"));
        assert_eq!(args.config.min_heap_expand, 64 << 20);
    }

    #[test]
    fn min_capped_by_max() {
        let mut args = EpsilonArguments::new();
        args.parse_heap_size("-Xmx16m");
        args.parse_heap_size("-Xms64m");
        args.initialize();
        assert_eq!(args.config.min_heap_size, 16 << 20);
    }
}
