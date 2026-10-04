//! Sample-clock accumulation and invalid plugin declaration regressions.

use crate::offline_renderer::render::{round_signal_delay, serial_signal_delay};
use sotf_plugins::{
    DawHost, Parameter, ParameterId, ParameterValue, Plugin, PluginInfo, ProcessContext,
};

struct DeclaredDelay {
    rate: u32,
    delay: f64,
}

impl Plugin for DeclaredDelay {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Declared delay", "1", "Test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("No parameters".to_owned())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn output_sample_rate(&self, _: f64) -> f64 {
        f64::from(self.rate)
    }
    fn signal_delay_samples(&self) -> f64 {
        self.delay
    }
    fn process(&mut self, _: &[f32], _: &mut [f32], _: &ProcessContext) -> Result<usize, String> {
        unreachable!("declaration-only test")
    }
}

#[test]
fn fractional_signal_delays_are_rounded_only_at_the_terminal_clock() {
    let mut host = DawHost::new(1, 48_000);
    // 0.4 final-clock frames from each of three stages. Rounding each
    // declaration or each converted contribution would incorrectly yield 0.
    for (rate, delay) in [(96_000, 0.8), (44_100, 0.3675), (48_000, 0.4)] {
        host.add_plugin(Box::new(DeclaredDelay { rate, delay }))
            .unwrap();
    }
    let delay = serial_signal_delay(&host, 48_000).unwrap();
    assert!((delay - 1.2).abs() < 1e-12);
    assert_eq!(round_signal_delay(delay).unwrap(), 1);
}

#[test]
fn nonfinite_negative_or_unaddressable_signal_delay_is_rejected() {
    for delay in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.5, f64::MAX] {
        let mut host = DawHost::new(1, 48_000);
        host.add_plugin(Box::new(DeclaredDelay {
            rate: 48_000,
            delay,
        }))
        .unwrap();
        assert!(
            serial_signal_delay(&host, 48_000).is_err(),
            "accepted {delay}"
        );
    }
}
