//! Exact, checked timeline units for plugin clocks negotiated as `f64`.
//!
//! A finite rate is kept as its reduced binary rational `p/q`. The shared
//! tick clock uses the least common multiple of active numerators, so every
//! sample period is an integer tick count. Rates, graph combinations, and
//! timeline horizons that exceed `u128` are refused before processing; no
//! sample rate or position is rounded to fit the clock.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactRate {
    pub numerator: u128,
    pub denominator: u128,
}

impl ExactRate {
    pub fn new(rate: f64) -> Result<Self, String> {
        if !rate.is_finite() || rate <= 0.0 {
            return Err(format!("Invalid graph sample rate {rate}"));
        }
        let bits = rate.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let fraction = bits & ((1_u64 << 52) - 1);
        let mut significand = if exponent == 0 {
            fraction
        } else {
            fraction | (1_u64 << 52)
        };
        let mut binary_exponent = (if exponent == 0 {
            -1022
        } else {
            exponent - 1023
        }) - 52;
        if binary_exponent < 0 {
            let common_twos = significand.trailing_zeros().min((-binary_exponent) as u32);
            significand >>= common_twos;
            binary_exponent += common_twos as i32;
        }
        let (numerator, denominator) = if binary_exponent >= 0 {
            (
                u128::from(significand)
                    .checked_shl(binary_exponent as u32)
                    .filter(|value| (*value >> binary_exponent) == u128::from(significand))
                    .ok_or_else(|| format!("Graph sample rate {rate} exceeds exact clock range"))?,
                1,
            )
        } else {
            let shift = (-binary_exponent) as u32;
            (
                u128::from(significand),
                1_u128
                    .checked_shl(shift)
                    .filter(|value| *value != 0)
                    .ok_or_else(|| format!("Graph sample rate {rate} exceeds exact clock range"))?,
            )
        };
        Ok(Self {
            numerator,
            denominator,
        })
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ExactClock {
    ticks_per_second: u128,
}

impl ExactClock {
    pub fn new(rates: impl IntoIterator<Item = ExactRate>) -> Result<Self, String> {
        let mut ticks_per_second = 1_u128;
        for rate in rates {
            ticks_per_second = (ticks_per_second / gcd(ticks_per_second, rate.numerator))
                .checked_mul(rate.numerator)
                .ok_or_else(|| "Graph sample rates exceed the exact clock range".to_owned())?;
        }
        Ok(Self { ticks_per_second })
    }

    pub fn ticks_per_sample(&self, rate: ExactRate) -> Result<u128, String> {
        if self.ticks_per_second % rate.numerator != 0 {
            return Err("Graph rate is absent from the exact clock".to_owned());
        }
        (self.ticks_per_second / rate.numerator)
            .checked_mul(rate.denominator)
            .ok_or_else(|| "Graph sample period exceeds the exact clock range".to_owned())
    }

    pub fn position_ticks(&self, position: u64, rate: ExactRate) -> Result<u128, String> {
        self.ticks_per_sample(rate)?
            .checked_mul(u128::from(position))
            .ok_or_else(|| "Graph timeline position exceeds the exact clock range".to_owned())
    }

    pub fn preflight_horizon(
        &self,
        position: u64,
        frames: usize,
        rate: ExactRate,
    ) -> Result<(), String> {
        let end = u128::from(position)
            .checked_add(frames as u128)
            .ok_or_else(|| "Graph timeline horizon exceeds the exact clock range".to_owned())?;
        let end = u64::try_from(end)
            .map_err(|_| "Graph timeline horizon exceeds u64 frames".to_owned())?;
        self.position_ticks(end, rate).map(|_| ())
    }

    pub fn ticks_to_frames_ceil(&self, ticks: u128, rate: ExactRate) -> Result<usize, String> {
        let period = self.ticks_per_sample(rate)?;
        let frames = ticks.div_ceil(period);
        usize::try_from(frames).map_err(|_| "Graph latency exceeds addressable frames".to_owned())
    }

    pub fn convert_position(
        &self,
        position: u64,
        source: ExactRate,
        target: ExactRate,
    ) -> Result<u64, String> {
        let source_ticks = self.position_ticks(position, source)?;
        let target_period = self.ticks_per_sample(target)?;
        u64::try_from(source_ticks / target_period)
            .map_err(|_| "Converted graph timeline position exceeds u64".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{ExactClock, ExactRate};

    #[test]
    fn integer_clock_matches_existing_position_conversion() {
        let source = ExactRate::new(48_000.0).unwrap();
        let target = ExactRate::new(96_000.0).unwrap();
        let clock = ExactClock::new([source, target]).unwrap();
        assert_eq!(clock.ticks_per_sample(source).unwrap(), 2);
        assert_eq!(clock.ticks_per_sample(target).unwrap(), 1);
        assert_eq!(clock.convert_position(17, source, target).unwrap(), 34);
    }

    #[test]
    fn nondivisible_positions_floor_and_latencies_ceil() {
        let slower = ExactRate::new(44_100.0).unwrap();
        let faster = ExactRate::new(48_000.0).unwrap();
        let clock = ExactClock::new([slower, faster]).unwrap();
        assert_eq!(clock.convert_position(1, faster, slower).unwrap(), 0);
        assert_eq!(clock.convert_position(1, slower, faster).unwrap(), 1);
        assert_eq!(
            clock
                .ticks_to_frames_ceil(clock.position_ticks(1, faster).unwrap(), slower)
                .unwrap(),
            1
        );
        assert_eq!(
            clock
                .ticks_to_frames_ceil(clock.position_ticks(1, slower).unwrap(), faster)
                .unwrap(),
            2
        );
    }

    #[test]
    fn fractional_clock_retains_binary_input_exactly() {
        let source = ExactRate::new(1234.5678).unwrap();
        let target = ExactRate::new(48_000.0).unwrap();
        let other = ExactRate::new(44_100.0).unwrap();
        let clock = ExactClock::new([source, target, other]).unwrap();
        assert_eq!(clock.convert_position(17, source, source).unwrap(), 17);
        let ticks = clock.position_ticks(17, source).unwrap();
        assert_eq!(ticks / clock.ticks_per_sample(source).unwrap(), 17);
        assert!(clock.convert_position(17, source, target).unwrap() > 17);
        assert_eq!(clock.convert_position(441, other, target).unwrap(), 480);
    }

    #[test]
    fn invalid_and_unrepresentable_rates_fail() {
        for rate in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::MIN_POSITIVE,
            f64::MAX,
        ] {
            assert!(ExactRate::new(rate).is_err());
        }
    }

    #[test]
    fn reduces_binary_significand_before_denominator_range_check() {
        assert_eq!(
            ExactRate::new(2_f64.powi(-80)).unwrap(),
            ExactRate {
                numerator: 1,
                denominator: 1_u128 << 80,
            }
        );
        assert!(ExactRate::new(2_f64.powi(-129)).is_err());
        assert!(ExactRate::new(2_f64.powi(128)).is_err());
    }

    #[test]
    fn oversized_timeline_product_is_an_error() {
        let rate = ExactRate::new(1.0).unwrap();
        let clock = ExactClock {
            ticks_per_second: u128::MAX,
        };
        assert!(clock.position_ticks(u64::MAX, rate).is_err());
        assert!(clock.preflight_horizon(u64::MAX - 1, 2, rate).is_err());
    }
}
