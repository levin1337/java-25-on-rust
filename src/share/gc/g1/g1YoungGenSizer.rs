/*
 * Port of OpenJDK 25 HotSpot G1YoungGenSizer
 * (share/gc/g1/g1YoungGenSizer.hpp/.cpp) — computes min/max bounds on
 * the young generation length in regions from ergonomics
 * (NewSize/MaxNewSize/NewRatio/G1NewSizePercent/G1MaxNewSizePercent).
 */

/// `G1YoungGenSizer::SizerKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizerKind {
    Defaults,
    NewSizeOnly,
    MaxNewSizeOnly,
    MaxAndNewSize,
    NewRatio,
}

/// Ergonomic defaults from g1_globals.hpp:
/// `G1NewSizePercent` = 5, `G1MaxNewSizePercent` = 60 (for >1GB heaps
/// the percent is scaled down in HotSpot; we keep the simple percent
/// form used by `calculate_default_*_length`).
pub const G1_NEW_SIZE_PERCENT: u32 = 5;
pub const G1_MAX_NEW_SIZE_PERCENT: u32 = 60;

#[derive(Debug, Clone)]
pub struct YoungGenSizingParams {
    /// -XX:NewSize in bytes (0 = unset).
    pub new_size: u64,
    /// -XX:MaxNewSize in bytes (0 = unset).
    pub max_new_size: u64,
    /// -XX:NewRatio (0 = unset).
    pub new_ratio: u32,
}

impl Default for YoungGenSizingParams {
    fn default() -> Self {
        Self { new_size: 0, max_new_size: 0, new_ratio: 0 }
    }
}

/// `G1YoungGenSizer`.
#[derive(Debug)]
pub struct G1YoungGenSizer {
    sizer_kind: SizerKind,
    use_adaptive_sizing: bool,
    min_desired_young_length: u32,
    max_desired_young_length: u32,
    params: YoungGenSizingParams,
    /// Region grain in bytes.
    region_grain_bytes: u64,
}

impl G1YoungGenSizer {
    /// `G1YoungGenSizer::G1YoungGenSizer` — resolves sizer kind from
    /// the flag-like params.
    pub fn new(params: YoungGenSizingParams, region_grain_bytes: u64) -> Self {
        let mut sizer = Self {
            sizer_kind: SizerKind::Defaults,
            use_adaptive_sizing: true,
            min_desired_young_length: 0,
            max_desired_young_length: 0,
            params,
            region_grain_bytes: region_grain_bytes.max(1),
        };
        sizer.resolve_kind();
        sizer
    }

    fn resolve_kind(&mut self) {
        let p = &self.params;
        if p.new_ratio != 0 {
            // NewRatio overrides NewSize/MaxNewSize (HotSpot warns and
            // ignores them).
            self.sizer_kind = SizerKind::NewRatio;
            self.use_adaptive_sizing = false;
            return;
        }
        if p.new_size != 0 {
            self.min_desired_young_length =
                ((p.new_size / self.region_grain_bytes) as u32).max(1);
            if p.max_new_size != 0 {
                self.max_desired_young_length =
                    ((p.max_new_size / self.region_grain_bytes) as u32).max(1);
                // NewSize > MaxNewSize -> MaxNewSize := NewSize
                // (HotSpot FLAG_SET_ERGO behavior).
                if self.max_desired_young_length < self.min_desired_young_length {
                    self.max_desired_young_length = self.min_desired_young_length;
                }
                self.sizer_kind = SizerKind::MaxAndNewSize;
                self.use_adaptive_sizing =
                    self.min_desired_young_length != self.max_desired_young_length;
            } else {
                self.sizer_kind = SizerKind::NewSizeOnly;
            }
        } else if p.max_new_size != 0 {
            self.max_desired_young_length =
                ((p.max_new_size / self.region_grain_bytes) as u32).max(1);
            self.sizer_kind = SizerKind::MaxNewSizeOnly;
        }
    }

    /// `calculate_default_min_length`.
    pub fn calculate_default_min_length(&self, num_heap_regions: u32) -> u32 {
        ((num_heap_regions * G1_NEW_SIZE_PERCENT) / 100).max(1)
    }

    /// `calculate_default_max_length`.
    pub fn calculate_default_max_length(&self, num_heap_regions: u32) -> u32 {
        ((num_heap_regions * G1_MAX_NEW_SIZE_PERCENT) / 100).max(1)
    }

    /// `recalculate_min_max_young_length`.
    pub fn recalculate_min_max_young_length(
        &self,
        num_heap_regions: u32,
    ) -> (u32, u32) {
        let (mut min_len, max_len) = match self.sizer_kind {
            SizerKind::Defaults => (
                self.calculate_default_min_length(num_heap_regions),
                self.calculate_default_max_length(num_heap_regions),
            ),
            SizerKind::NewSizeOnly => {
                let max = self
                    .calculate_default_max_length(num_heap_regions)
                    .max(self.min_desired_young_length);
                (self.min_desired_young_length, max)
            }
            SizerKind::MaxNewSizeOnly => {
                let min = self
                    .calculate_default_min_length(num_heap_regions)
                    .min(self.max_desired_young_length);
                (min, self.max_desired_young_length)
            }
            SizerKind::MaxAndNewSize => {
                (self.min_desired_young_length, self.max_desired_young_length)
            }
            SizerKind::NewRatio => {
                let min = (num_heap_regions / (self.params.new_ratio + 1)).max(1);
                (min, min)
            }
        };
        if min_len > max_len {
            min_len = max_len;
        }
        (min_len, max_len)
    }

    /// `heap_size_changed`.
    pub fn heap_size_changed(&mut self, num_heap_regions: u32) {
        let (min, max) = self.recalculate_min_max_young_length(num_heap_regions);
        self.min_desired_young_length = min;
        self.max_desired_young_length = max;
    }

    /// `adjust_max_new_size`: returns the MaxNewSize in bytes the
    /// ergonomics would set (for flag reporting parity).
    pub fn adjust_max_new_size(&self, num_heap_regions: u32) -> u64 {
        let (_, max) = self.recalculate_min_max_young_length(num_heap_regions);
        max as u64 * self.region_grain_bytes
    }

    #[inline]
    pub fn min_desired_young_length(&self) -> u32 {
        self.min_desired_young_length
    }

    #[inline]
    pub fn max_desired_young_length(&self) -> u32 {
        self.max_desired_young_length
    }

    #[inline]
    pub fn sizer_kind(&self) -> SizerKind {
        self.sizer_kind
    }

    #[inline]
    pub fn use_adaptive_sizing(&self) -> bool {
        self.use_adaptive_sizing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bounds() {
        let sizer = G1YoungGenSizer::new(YoungGenSizingParams::default(), 1024 * 1024);
        let (min, max) = sizer.recalculate_min_max_young_length(200);
        // 5% and 60% of 200 regions
        assert_eq!(min, 10);
        assert_eq!(max, 120);
    }

    #[test]
    fn new_size_only() {
        let params = YoungGenSizingParams { new_size: 50 * 1024 * 1024, max_new_size: 0, new_ratio: 0 };
        let mut sizer = G1YoungGenSizer::new(params, 1024 * 1024);
        // 50 MiB / 1 MiB = 50 regions min
        sizer.heap_size_changed(200);
        let (min, max) = sizer.recalculate_min_max_young_length(200);
        assert_eq!(min, 50);
        assert_eq!(max, 120); // default max 60%
    }

    #[test]
    fn new_ratio() {
        let params = YoungGenSizingParams { new_size: 0, max_new_size: 0, new_ratio: 3 };
        let sizer = G1YoungGenSizer::new(params, 1024 * 1024);
        let (min, max) = sizer.recalculate_min_max_young_length(200);
        assert_eq!(min, 50); // 200/(3+1)
        assert_eq!(max, 50);
        assert!(!sizer.use_adaptive_sizing());
    }
}
