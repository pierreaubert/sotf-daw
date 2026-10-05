//! Probe the packaged plugins through native CLAP activation callbacks.

use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use nih_plug::prelude::{ClapPlugin, Plugin};
use std::ffi::{c_char, c_void};

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn no_request(_: *const clap_host) {}

// The pinned validator probes these rates. CLAP permits rejecting unsupported
// rates; this release QA diagnostic keeps that distinction visible while
// requiring the full validator stress matrix to pass before release.
const VALIDATOR_STRESS_RATES: [f64; 13] = [
    8_000.0,
    22_050.0,
    44_100.0,
    48_000.0,
    88_200.0,
    96_000.0,
    192_000.0,
    384_000.0,
    768_000.0,
    1_234.567_8,
    12_345.678,
    45_678.901,
    123_456.78,
];
const POSITIVE_CONTROL_RATE: f64 = 48_000.0;

fn activation_outcomes<P: Plugin + ClapPlugin>() -> Vec<(f64, bool, bool)> {
    let mut outcomes = Vec::with_capacity(VALIDATOR_STRESS_RATES.len());

    for sample_rate in VALIDATOR_STRESS_RATES {
        let host = Box::new(clap_host {
            clap_version: clap_sys::version::CLAP_VERSION,
            host_data: std::ptr::null_mut(),
            name: c"SOTF activation matrix test".as_ptr(),
            vendor: c"SOTF".as_ptr(),
            url: c"".as_ptr(),
            version: c"1".as_ptr(),
            get_extension: Some(no_extension),
            request_restart: Some(no_request),
            request_process: Some(no_request),
            request_callback: Some(no_request),
        });
        // SAFETY: the host outlives this wrapper and all synchronous callbacks.
        let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<P>::new(&*host) };
        let plugin: *const clap_plugin = wrapper.clap_plugin.as_ptr();
        // SAFETY: callback pointers belong to the live plugin; each successful
        // activation is deactivated before dropping its fresh wrapper.
        let (initialized, activated) = unsafe {
            let initialized = ((*plugin).init.unwrap())(plugin);
            let activated =
                initialized && ((*plugin).activate.unwrap())(plugin, sample_rate, 1, 256);
            if activated {
                ((*plugin).deactivate.unwrap())(plugin);
            }
            (initialized, activated)
        };
        outcomes.push((sample_rate, initialized, activated));
    }

    outcomes
}

fn assert_activation_matrix<P: Plugin + ClapPlugin>(name: &str) {
    let outcomes = activation_outcomes::<P>();
    println!("{name} pinned-validator CLAP activation matrix: {outcomes:?}");
    assert!(
        outcomes
            .iter()
            .any(|(rate, _, activated)| *rate == POSITIVE_CONTROL_RATE && *activated),
        "{name} failed the 48 kHz positive control: {outcomes:?}"
    );
    assert!(
        outcomes.iter().all(|(_, _, activated)| *activated),
        "{name} rejected at least one sample rate: {outcomes:?}"
    );
}

macro_rules! activation_matrix_test {
    ($feature:literal, $test:ident, $plugin:ident) => {
        #[cfg(feature = $feature)]
        #[test]
        fn $test() {
            assert_activation_matrix::<crate::plugin::$plugin>(stringify!($plugin));
        }
    };
}

activation_matrix_test!("band-split", band_split_activation_matrix, SotfBandSplit);
activation_matrix_test!("crossfeed", crossfeed_activation_matrix, SotfCrossfeed);
activation_matrix_test!("crossover", crossover_activation_matrix, SotfCrossover);
activation_matrix_test!("de-esser", de_esser_activation_matrix, SotfDeEsser);
activation_matrix_test!("downmix", downmix_activation_matrix, SotfDownmix);
activation_matrix_test!("dynamic-eq", dynamic_eq_activation_matrix, SotfDynamicEQ);
activation_matrix_test!("eq", eq_activation_matrix, SotfEQ);
activation_matrix_test!(
    "hiss-reducer",
    hiss_reducer_activation_matrix,
    SotfHissReducer
);
activation_matrix_test!(
    "linear-phase-eq",
    linear_phase_eq_activation_matrix,
    SotfLinearPhaseEQ
);
activation_matrix_test!(
    "mono-to-stereo",
    mono_to_stereo_activation_matrix,
    SotfMonoToStereo
);
activation_matrix_test!(
    "multiband-compressor",
    multiband_compressor_activation_matrix,
    SotfMultibandCompressor
);
activation_matrix_test!("saturation", saturation_activation_matrix, SotfSaturation);
activation_matrix_test!(
    "spectrum-analyzer",
    spectrum_analyzer_activation_matrix,
    SotfSpectrumAnalyzer
);
activation_matrix_test!(
    "speech-denoiser",
    speech_denoiser_activation_matrix,
    SotfSpeechDenoiser
);
activation_matrix_test!(
    "stereo-imager",
    stereo_imager_activation_matrix,
    SotfStereoImager
);
activation_matrix_test!("upmixer", upmixer_activation_matrix, SotfUpmixer);
