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
        (3 * fft + 1, [179.0, -179.0, 179.0]),
        (2 * fft + 5, [-179.0, 179.0, -179.0]),
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

#[test]
fn wrong_clock_preserves_audio_controls_and_finite_stream_epoch() {
    for wrong in [
        44_100.0,
        96_000.0,
        48_000.5,
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
    ] {
        let (mut tested, _rx) = make(48_000, 256);
        let (mut baseline, _baseline_rx) = make(48_000, 256);
        for p in [&mut tested, &mut baseline] {
            p.smoothing.head_yaw_deg.set_target(45.0);
            p.process(
                &[0.125; 26],
                &mut [0.0; 26],
                &ProcessContext::new(48_000, 13),
            )
            .unwrap();
        }
        let before = tested.smoothing.head_yaw_deg.current();
        for frames in [0, 37] {
            let mut sentinel = vec![123.0; frames * 2];
            assert!(
                tested
                    .process(
                        &vec![0.25; frames * 2],
                        &mut sentinel,
                        &ProcessContext::new(wrong, frames)
                    )
                    .is_err(),
                "wrong process clock {wrong}"
            );
            assert!(sentinel.iter().all(|x| *x == 123.0));
            assert_eq!(tested.smoothing.head_yaw_deg.current(), before);
        }
        assert!(
            tested
                .drain(&mut [123.0; 2], &ProcessContext::new(wrong, 0))
                .is_err()
        );
        let mut a = [0.0; 146];
        let mut b = [0.0; 146];
        tested
            .process(&[0.125; 146], &mut a, &ProcessContext::new(48_000, 73))
            .unwrap();
        baseline
            .process(&[0.125; 146], &mut b, &ProcessContext::new(48_000, 73))
            .unwrap();
        assert_eq!(a, b);
        let context = ProcessContext::new(48_000, 0);
        let first_a = tested.drain(&mut a[..2], &context).unwrap();
        let first_b = baseline.drain(&mut b[..2], &context).unwrap();
        assert_eq!(
            (first_a.frames, first_a.complete),
            (first_b.frames, first_b.complete)
        );
        assert_eq!(a[..2], b[..2]);
        let mut sentinel = [123.0; 146];
        assert!(
            tested
                .drain(&mut sentinel, &ProcessContext::new(wrong, 0))
                .is_err()
        );
        assert_eq!(sentinel, [123.0; 146]);
        loop {
            let x = tested.drain(&mut a, &context).unwrap();
            let y = baseline.drain(&mut b, &context).unwrap();
            assert_eq!((x.frames, x.complete), (y.frames, y.complete));
            assert_eq!(a[..x.frames * 2], b[..y.frames * 2]);
            if x.complete {
                break;
            }
        }
        assert!(
            tested
                .drain(&mut [], &ProcessContext::new(wrong, 0))
                .is_err()
        );
    }
}
