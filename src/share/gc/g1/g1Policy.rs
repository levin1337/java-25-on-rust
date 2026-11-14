/*
 * Port of OpenJDK 25 HotSpot G1Policy (share/gc/g1/g1Policy.hpp)
 *
 * G1Policy regulates dynamic sizing of Eden, Survivor, and Old regions
 * to satisfy soft real-time pause time targets.
 */

#[derive(Debug, Clone)]
pub struct G1Policy {
    max_pause_time_ms: u32,
    young_gen_percent_min: u32,
    young_gen_percent_max: u32,
    survivor_ratio: u32,
    tenuring_threshold: u32,
}

impl G1Policy {
    pub fn new() -> Self {
        Self {
            max_pause_time_ms: 200,
            young_gen_percent_min: 5,
            young_gen_percent_max: 60,
            survivor_ratio: 8,
            tenuring_threshold: 15,
        }
    }

    #[inline]
    pub fn max_pause_time_ms(&self) -> u32 {
        self.max_pause_time_ms
    }

    #[inline]
    pub fn set_max_pause_time_ms(&mut self, pause_ms: u32) {
        self.max_pause_time_ms = pause_ms;
    }

    #[inline]
    pub fn tenuring_threshold(&self) -> u32 {
        self.tenuring_threshold
    }

    #[inline]
    pub fn survivor_ratio(&self) -> u32 {
        self.survivor_ratio
    }

    pub fn calculate_young_bounds(&self, total_regions: usize) -> (usize, usize) {
        let min_regions = (total_regions * self.young_gen_percent_min as usize) / 100;
        let max_regions = (total_regions * self.young_gen_percent_max as usize) / 100;
        (min_regions.max(1), max_regions.max(1))
    }
}

impl Default for G1Policy {
    fn default() -> Self {
        Self::new()
    }
}
