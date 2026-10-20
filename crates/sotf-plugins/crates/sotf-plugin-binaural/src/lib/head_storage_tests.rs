//! Immutable dataset ownership and bounded head-preparation work.

use crate::BinauralDecoderPlugin;
use crate::realtime_tests::preparation_heap;
use crate::types::BinauralState;
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use sotf_host::Plugin;
use sotf_host::sofa::{SofaFile, SourcePosition};
use std::sync::Arc;
use std::time::Instant;

fn dataset(measurements: usize, length: usize) -> SofaFile {
    let mut positions = Vec::with_capacity(measurements);
    let mut impulse_responses = vec![0.0; measurements * length * 2];
    for i in 0..measurements {
        let az = i as f32 * 360.0 / measurements as f32 - 180.0;
        positions.push(SourcePosition::new(az, 0.0, 1.0));
        for ear in 0..2 {
            impulse_responses[(i * 2 + ear) * length + 4 + ear * 2] = 0.8;
            impulse_responses[(i * 2 + ear) * length + 10] = -0.08;
        }
    }
    SofaFile {
        sample_rate: 48000.0,
        num_measurements: measurements,
        ir_length: length,
        positions,
        impulse_responses,
        convention: "SimpleFreeFieldHRIR".into(),
        data_sample_rate: Some(48000.0),
    }
}

#[test]
fn completed_poses_share_the_immutable_dataset_without_measurement_sized_allocations() {
    let n = 2048;
    let bins = n / 2 + 1;
    let mut valid = true;
    for (measurements, length) in [(36, 64), (720, 512), (2048, 1024)] {
        let mut p = BinauralDecoderPlugin::new(
            2,
            n,
            None,
            1.0,
            0.0,
            false,
            120.0,
            2.0,
            0.0,
            Default::default(),
        );
        p.initialize(48000.0).unwrap();
        let base = Arc::new(BinauralState {
            hrtf_filters_freq: vec![vec![Complex::new(0.0, 0.0); bins * 2]; 2],
            design_diffuse_field_eq: Some([
                vec![Complex::new(1.0, 0.0); bins],
                vec![Complex::new(1.0, 0.0); bins],
            ]),
            _hrtf_data: Some(dataset(measurements, length).into()),
        });
        let sofa = base._hrtf_data.as_ref().unwrap();
        assert_eq!(Arc::strong_count(sofa), 1);
        let started = Instant::now();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
        let prepared = crate::hrtf::prepare_hrtf_spectra(sofa, n, 48000.0, &fft);
        let preparation_ns = started.elapsed().as_nanos();
        let mut maximum_ns = 0;
        let mut maximum_bytes = 0;
        let mut largest_allocation = 0;
        let mut shared = true;
        for pose in 0..12 {
            let start = Instant::now();
            let (result, bytes, largest, _) = preparation_heap(|| {
                BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
                    &base,
                    &p.config,
                    &prepared,
                    &p.fft,
                    &[],
                    (pose as f32 * 7.25 - 40.0, 0.0, 0.0),
                )
                .unwrap()
            });
            maximum_ns = maximum_ns.max(start.elapsed().as_nanos());
            assert_eq!(Arc::strong_count(sofa), 2);
            maximum_bytes = maximum_bytes.max(bytes);
            largest_allocation = largest_allocation.max(largest);
            shared &= result
                ._hrtf_data
                .as_ref()
                .unwrap()
                .impulse_responses
                .as_ptr()
                == sofa.impulse_responses.as_ptr();
            // Constant measured fields must remain independent of orientation.
            let reference = BinauralDecoderPlugin::compute_head_rotated_hrtf_state(
                &base,
                &p.config,
                &prepared,
                &p.fft,
                &[],
                (0.0, 0.0, 0.0),
            )
            .unwrap();
            assert_eq!(Arc::strong_count(sofa), 3);
            for (actual, expected) in result
                .hrtf_filters_freq
                .iter()
                .flatten()
                .zip(reference.hrtf_filters_freq.iter().flatten())
            {
                assert!((*actual - *expected).norm() < 2.0e-5);
            }
        }
        println!(
            "HEAD_STORAGE measurements={measurements} ir={length} fft={n} dataset_bytes={} prep_ns={preparation_ns} max_pose_ns={maximum_ns} max_pose_bytes={maximum_bytes} largest_allocation={largest_allocation} shared={shared}",
            sofa.impulse_responses.len() * 4
        );
        valid &= shared && largest_allocation <= p.config.freq_size * 2 * size_of::<Complex<f32>>();
    }
    assert!(
        valid,
        "completed poses must share SOFA without measurement-sized allocations"
    );
}

#[test]
fn prepared_storage_admission_checks_boundaries_and_overflow() {
    let budget = 256 * 1024 * 1024;
    for n in [64, 256, 2048, 4096, 65536] {
        let per = 2 * (n / 2 + 1) * size_of::<Complex<f32>>()
            + 2 * size_of::<Vec<Complex<f32>>>()
            + 2 * size_of::<f32>();
        let admitted = budget / per;
        assert_eq!(
            crate::hrtf::validate_prepared_hrtf_storage(admitted, n).unwrap(),
            admitted * per
        );
        assert!(crate::hrtf::validate_prepared_hrtf_storage(admitted + 1, n).is_err());
    }
    assert!(crate::hrtf::validate_prepared_hrtf_storage(usize::MAX, 4096).is_err());
    assert!(crate::hrtf::validate_prepared_hrtf_storage(3, usize::MAX).is_err());
}

#[test]
fn oversized_prepared_cache_rejects_reload_and_initialize_transactionally() {
    let valid = crate::head_cue_tests::sofa(48000, 36, 0.25);
    let too_many = crate::head_cue_tests::sofa(48000, 9000, 0.25);
    let mut p = BinauralDecoderPlugin::new(
        2,
        4096,
        None,
        1.0,
        0.0,
        false,
        120.0,
        2.0,
        0.0,
        Default::default(),
    );
    p.initialize(48000.0).unwrap();
    p.set_parameter(
        "hrtf_file".into(),
        sotf_host::ParameterValue::String(valid.path.to_string_lossy().into_owned()),
    )
    .unwrap();
    let published = p.state.load_full();
    let audible = Arc::clone(&p.crossfade.current_state_snapshot);
    let resource = p.get_data().unwrap();
    let worker = p.hrtf_update_thread.as_ref().unwrap().thread().id();
    let old_path = p.config.hrtf_path.clone();
    let (result, _, largest, _) = preparation_heap(|| {
        p.set_parameter(
            "hrtf_file".into(),
            sotf_host::ParameterValue::String(too_many.path.to_string_lossy().into_owned()),
        )
    });
    assert!(result.unwrap_err().contains("spectral storage"));
    assert!(
        largest < 256 * 1024 * 1024,
        "prepared spectra allocated before rejection"
    );
    assert_eq!(p.config.hrtf_path, old_path);
    assert!(Arc::ptr_eq(&published, &p.state.load_full()));
    assert!(Arc::ptr_eq(&audible, &p.crossfade.current_state_snapshot));
    assert!(Arc::ptr_eq(&resource, &p.get_data().unwrap()));
    assert_eq!(worker, p.hrtf_update_thread.as_ref().unwrap().thread().id());
    p.config.hrtf_path = Some(too_many.path.clone());
    assert!(
        p.initialize(96000.0)
            .unwrap_err()
            .contains("spectral storage")
    );
    assert_eq!(p.config.sample_rate, 48000.0);
    assert!(Arc::ptr_eq(&published, &p.state.load_full()));
    assert!(Arc::ptr_eq(&audible, &p.crossfade.current_state_snapshot));
    assert!(Arc::ptr_eq(&resource, &p.get_data().unwrap()));
    assert_eq!(worker, p.hrtf_update_thread.as_ref().unwrap().thread().id());
}

#[test]
#[ignore = "explicit release run materializes the 256 MiB prepared-cache boundary"]
fn audit_prepared_storage_boundary() {
    let n = 2048;
    let per = 2 * (n / 2 + 1) * size_of::<Complex<f32>>()
        + 2 * size_of::<Vec<Complex<f32>>>()
        + 2 * size_of::<f32>();
    let measurements = (256 * 1024 * 1024) / per;
    let sofa = dataset(measurements, 16);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(n);
    let started = Instant::now();
    let (prepared, total, largest, peak) =
        preparation_heap(|| crate::hrtf::prepare_hrtf_spectra(&sofa, n, 48000.0, &fft));
    let elapsed = started.elapsed().as_nanos();
    std::hint::black_box(&prepared);
    let estimated = crate::hrtf::validate_prepared_hrtf_storage(measurements, n).unwrap();
    assert!(peak >= estimated);
    assert!(
        peak <= estimated + 64 * n,
        "unbounded extra preparation storage"
    );
    println!(
        "HEAD_STORAGE_BOUNDARY measurements={measurements} fft={n} source_bytes={} prepared_bytes={estimated} cumulative_allocated={total} largest_allocation={largest} peak_live_allocated={peak} prep_ns={elapsed}",
        sofa.impulse_responses.len() * 4
    );
}
