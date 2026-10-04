use sotf_host::{CountingAlloc, Plugin, ProcessContext, TailLength, assert_no_allocs};
use sotf_plugin_matrix::MatrixPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn routing_and_gain_transitions_emit_no_audio_from_prior_input() {
    for (inputs, outputs) in [(1, 1), (2, 3), (3, 2), (8, 8)] {
        let mut plugin = MatrixPlugin::new(inputs, outputs);
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
        plugin.initialize(48_000.0).unwrap();
        let hot = vec![0.75; 257 * inputs];
        let mut output = vec![0.0; 257 * outputs];
        plugin
            .process(&hot, &mut output, &ProcessContext::new(48_000, 257))
            .unwrap();
        assert!(output.iter().any(|sample| sample.abs() > 0.1));
        plugin.set_gain(0, 0, 0.25).unwrap();
        let zeros = vec![0.0; 257 * inputs];
        std::thread::spawn(move || {
            assert_no_allocs("Matrix zero tail during routing transition", || {
                for frames in [1, 17, 257, 257] {
                    output.fill(f32::NAN);
                    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
                    assert_eq!(
                        plugin
                            .process(
                                &zeros[..frames * inputs],
                                &mut output[..frames * outputs],
                                &ProcessContext::new(48_000, frames)
                            )
                            .unwrap(),
                        frames
                    );
                    assert!(
                        output[..frames * outputs]
                            .iter()
                            .all(|sample| *sample == 0.0)
                    );
                }
            });
        })
        .join()
        .unwrap();
    }
}
