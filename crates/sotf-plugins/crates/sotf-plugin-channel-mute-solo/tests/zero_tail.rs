use sotf_host::{
    CountingAlloc, ParametricInPlacePlugin, ProcessContext, TailLength, assert_no_allocs,
};
use sotf_plugin_channel_mute_solo::ChannelMuteSoloPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn channel_fades_emit_no_audio_from_prior_input() {
    for channels in [1, 2, 8] {
        let mut plugin = ChannelMuteSoloPlugin::new(channels, true);
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
        plugin.initialize(48_000.0).unwrap();
        let mut buffer = vec![0.75; 257 * channels];
        plugin
            .process_in_place(&mut buffer, &ProcessContext::new(48_000, 257))
            .unwrap();
        assert!(buffer.iter().any(|sample| sample.abs() > 0.1));
        plugin.set_channel_state(0, true, false, false).unwrap();
        std::thread::spawn(move || {
            assert_no_allocs("ChannelMuteSolo zero tail during channel fade", || {
                for frames in [1, 17, 257, 257] {
                    buffer.fill(0.0);
                    assert_eq!(plugin.tail_length(), TailLength::Finite(0));
                    assert_eq!(
                        plugin
                            .process_in_place(
                                &mut buffer[..frames * channels],
                                &ProcessContext::new(48_000, frames)
                            )
                            .unwrap(),
                        frames
                    );
                    assert!(
                        buffer[..frames * channels]
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
