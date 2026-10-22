//! Real-file UPC cache layout and independent windowed DFT checks.

// Rust guideline compliant 2026-02-21
use super::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn coefficient(frame: usize, channel: usize) -> i16 {
    ((frame * 37 + channel * 7919) % 20001) as i16 - 10000
}
fn fixture(frames: usize, channels: usize) -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "sotf-upc-real-{}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let fixture = Fixture(path);
    let size = (frames * channels * 2) as u32;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + size).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&(channels as u16).to_le_bytes()).unwrap();
    file.write_all(&48000_u32.to_le_bytes()).unwrap();
    file.write_all(&(48000 * channels as u32 * 2).to_le_bytes())
        .unwrap();
    file.write_all(&(channels as u16 * 2).to_le_bytes())
        .unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&size.to_le_bytes()).unwrap();
    for frame in 0..frames {
        for channel in 0..channels {
            file.write_all(&coefficient(frame, channel).to_le_bytes())
                .unwrap();
        }
    }
    drop(file);
    fixture
}

#[test]
fn real_file_upc_uses_half_spectra_and_matches_independent_f64_dft() {
    let expected_bins = FFT_SIZE / 2 + 1;
    let mut cases = 0;
    let mut checks = 0;
    let mut failures = 0;
    let mut dft_checks = 0;
    let mut maximum_error = 0.0_f64;
    for frames in [1, 1025, 4099] {
        for (outputs, ir_channels, matrix) in
            [(1, 1, false), (12, 1, false), (12, 3, false), (2, 4, true)]
        {
            let file = fixture(frames, ir_channels);
            let plugin = ConvolutionPlugin::from_params_with_routing(
                outputs,
                48000,
                ConvolutionPluginParams {
                    ir_file: file.0.to_string_lossy().into_owned(),
                    use_nupc: false,
                    ..Default::default()
                },
                matrix,
            )
            .unwrap();
            let state = plugin.ir_runtime.state.as_ref().as_ref().unwrap();
            for cache in state
                .partitions
                .iter()
                .flat_map(|p| p.iter())
                .chain([&plugin.fft_spectrum, &plugin.fft_sum])
                .chain(plugin.rayon_accum_pool.iter())
            {
                checks += 1;
                failures += usize::from(cache.len() != expected_bins);
            }
            checks += 1;
            failures += usize::from(
                plugin.fdl_flat.len() != state.num_partitions * outputs * expected_bins,
            );
            assert_eq!(plugin.latency_samples(), PARTITION_SIZE);
            for channel in [0, ir_channels - 1] {
                for part in [0, state.num_partitions - 1] {
                    for bin in [0, 1, 31, 512, 1024] {
                        let mut expected = Complex::new(0.0_f64, 0.0);
                        let start = part * PARTITION_SIZE;
                        for frame in start..(start + PARTITION_SIZE).min(frames) {
                            let phase =
                                -std::f64::consts::TAU * bin as f64 * (frame - start) as f64
                                    / FFT_SIZE as f64;
                            let sample = f64::from(coefficient(frame, channel)) / 32768.0;
                            expected += Complex::new(sample * phase.cos(), sample * phase.sin());
                        }
                        let actual = state.partitions[channel][part][bin];
                        let error = (Complex::new(f64::from(actual.re), f64::from(actual.im))
                            - expected)
                            .norm();
                        maximum_error = maximum_error.max(error);
                        assert!(
                            error < 2e-5,
                            "channel={channel} part={part} bin={bin} error={error}"
                        );
                        dft_checks += 1;
                    }
                }
            }
            cases += 1;
        }
    }
    println!(
        "upc_real_cache cases={cases} checks={checks} failures={failures} dft_checks={dft_checks} maximum_error={maximum_error:e}"
    );
    assert_eq!(cases, 12);
    assert_eq!(failures, 0, "UPC needs N/2+1 bins and matching FDL stride");
}
