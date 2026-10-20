//! Source-clock tests isolate request generation from nondeterministic worker scheduling.

use crate::BinauralDecoderPlugin;
use sotf_host::plugin::{Plugin, ProcessContext};
use std::sync::mpsc::{Receiver, sync_channel};

fn make(rate: u32, fft: usize) -> (BinauralDecoderPlugin, Receiver<(f32, f32, f32)>) {
    let params =
        serde_json::from_value(serde_json::json!({"input_channels":2,"fft_size":fft})).unwrap();
    let mut plugin = BinauralDecoderPlugin::try_from_params(params).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let (tx, rx) = sync_channel(4096);
    plugin.hrtf_update_tx = Some(tx);
    (plugin, rx)
}

fn render(rate: u32, fft: usize, chunks: &[usize]) -> (Vec<(f32, f32, f32)>, [f32; 3]) {
    let (mut p, rx) = make(rate, fft);
    let mut callback = 0;
    for (frames, angles) in [
        (3 * fft + 7, [90.0, 30.0, -20.0]),
        (2 * fft + 3, [-60.0, -15.0, 45.0]),
        (4 * fft + 11, [12.0, 5.0, 0.0]),
    ] {
        p.smoothing.head_yaw_deg.set_target(angles[0]);
        p.smoothing.head_pitch_deg.set_target(angles[1]);
        p.smoothing.head_roll_deg.set_target(angles[2]);
        let mut left = frames;
        while left > 0 {
            let count = left.min(chunks[callback % chunks.len()]);
            p.process(
                &vec![0.0; count * 2],
                &mut vec![0.0; count * 2],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
            left -= count;
            callback += 1;
        }
    }
    let before = [
        p.smoothing.head_yaw_deg.current(),
        p.smoothing.head_pitch_deg.current(),
        p.smoothing.head_roll_deg.current(),
    ];
    let messages: Vec<_> = rx.try_iter().collect();
    let mut tail = vec![0.0; fft * 2];
    loop {
        if p.drain(&mut tail, &ProcessContext::new(rate, 0))
            .unwrap()
            .complete
        {
            break;
        }
    }
    assert_eq!(
        before,
        [
            p.smoothing.head_yaw_deg.current(),
            p.smoothing.head_pitch_deg.current(),
            p.smoothing.head_roll_deg.current()
        ],
        "synthetic EOF input advanced orientation"
    );
    assert!(rx.try_iter().next().is_none());
    (messages, before)
}

#[test]
fn head_requests_follow_source_hops_across_callback_partitions() {
    for rate in [44_100, 48_000, 96_000] {
        for fft in [64, 256, 1024] {
            let expected = render(rate, fft, &[1]);
            assert!(!expected.0.is_empty());
            for chunks in [&[17, 193, 1, 511][..], &[4096][..], &[64][..]] {
                assert_eq!(
                    render(rate, fft, chunks),
                    expected,
                    "rate={rate},fft={fft},chunks={chunks:?}"
                );
            }
            eprintln!(
                "HEAD_CLOCK rate={rate} fft={fft} callback_shapes=4 requests={}",
                expected.0.len()
            );
        }
    }
}

#[test]
fn empty_callbacks_and_partial_source_hops_do_not_send_future_angles() {
    for rate in [44_100, 48_000, 96_000] {
        let (mut p, rx) = make(rate, 256);
        p.smoothing.head_yaw_deg.set_target(90.0);
        let before = p.smoothing.head_yaw_deg.current();
        p.process(&[], &mut [], &ProcessContext::new(rate, 0))
            .unwrap();
        assert_eq!(
            p.smoothing.head_yaw_deg.current(),
            before,
            "empty callback advanced head time"
        );
        assert!(rx.try_recv().is_err());
        let hop = p.config.hop_size;
        p.process(
            &vec![0.0; (hop - 1) * 2],
            &mut vec![0.0; (hop - 1) * 2],
            &ProcessContext::new(rate, hop - 1),
        )
        .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "request preceded its source-audio hop"
        );
        p.process(&[0.0; 2], &mut [0.0; 2], &ProcessContext::new(rate, 1))
            .unwrap();
        let angle = rx
            .try_recv()
            .expect("first completed hop must request rotation");
        let expected = 90.0 * (1.0 - (-(hop as f64) / (f64::from(rate) * 0.010)).exp());
        assert!(
            (f64::from(angle.0) - expected).abs() < 0.001,
            "source-clock angle {angle:?} vs {expected}"
        );
        p.process(
            &vec![0.0; (hop - 1) * 2],
            &mut vec![0.0; (hop - 1) * 2],
            &ProcessContext::new(rate, hop - 1),
        )
        .unwrap();
        p.reset();
        p.process(&[0.0; 2], &mut [0.0; 2], &ProcessContext::new(rate, 1))
            .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "reset retained the previous partial-hop clock"
        );
    }
}
