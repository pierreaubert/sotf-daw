//! Shipping-direction consumer for the Swift HAL sequence regression.

// Rust guideline compliant 2026-02-21

#[cfg(target_os = "macos")]
#[path = "hal_capture_stress/metrics.rs"]
mod metrics;

#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    use driver_hal::HalInputReader;
    use std::time::{Duration, Instant};

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["control"] {
        return control(false, false);
    }
    if arguments == ["broker"] || arguments == ["broker-encrypted"] {
        return control(true, arguments == ["broker-encrypted"]);
    }
    if arguments.len() != 4 {
        return Err(
            "usage: hal-capture-stress <channels> <frames> <sample-rate> <buffer-frames>".into(),
        );
    }
    let channels: usize = arguments[0].parse().map_err(|_| "invalid channels")?;
    let total: usize = arguments[1].parse().map_err(|_| "invalid frames")?;
    let rate: u32 = arguments[2].parse().map_err(|_| "invalid sample rate")?;
    let block_frames: u32 = arguments[3].parse().map_err(|_| "invalid buffer frames")?;
    if ![2, 8, 32].contains(&channels)
        || ![48_000, 96_000, 192_000].contains(&rate)
        || ![64, 128, 512].contains(&block_frames)
        || total != block_frames as usize * 8 * (rate as usize).div_ceil(48_000) * 32
        || total * channels > 16_777_216
    {
        return Err("unsupported sequence geometry".into());
    }
    // SAFETY: the Swift harness owns the explicit fixture until this worker exits.
    let mut reader =
        unsafe { HalInputReader::new_for_fixture() }.ok_or("could not attach HAL reader")?;
    let format = reader.current_format().map_err(|error| error.to_string())?;
    if format != (rate, channels as u32, block_frames) {
        return Err(format!("fixture geometry mismatch: {format:?}"));
    }
    if reader.needs_cipher_reload() {
        reader.reload_cipher().map_err(|error| error.to_string())?;
    }
    // Different producer/consumer block sizes exercise encrypted staging as
    // well as ring wraparound. Integer markers remain exactly representable.
    let storm = if std::env::var_os("SOTF_HAL_MEASUREMENT_STORM").is_some() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        // SAFETY: this worker's parent owns the explicit private fixture extent
        // until the worker exits, including the joined control-storm thread.
        let control = unsafe {
            driver_hal::SharedAudioBuffer::open_fixture(driver_hal::get_shared_memory_path())
        }
        .map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let mut updates = 0u64;
            while !worker_stop.load(Ordering::Acquire) {
                control.patch_output_volume(Some(0.5), Some(false));
                if let Some(id) = control.publish_config_request(
                    driver_common::DriverConfig::new(rate, block_frames, channels as u32),
                    2,
                ) {
                    // Negotiation errors exercise publication churn without
                    // changing the sequence geometry or writing audio cursors.
                    if control.acknowledge_config_request(
                        id,
                        driver_common::DriverConfig::new(rate, block_frames, channels as u32),
                        3,
                        1,
                    ) {
                        updates += 1;
                    }
                }
                std::thread::sleep(Duration::from_micros(50));
            }
            updates
        });
        Some((stop, worker))
    } else {
        None
    };
    let mut samples = vec![0.0; 47 * channels];
    // The peer Swift producer is instrumented in sanitizer runs. This changes
    // only their liveness allowance; normal release measurements retain 15 s.
    let sanitizer = std::env::var("SOTF_HAL_SANITIZER").unwrap_or_default();
    let timeout_seconds = if matches!(sanitizer.as_str(), "thread" | "address") {
        45
    } else {
        15
    };
    let sequence_started = Instant::now();
    let deadline = sequence_started + Duration::from_secs(timeout_seconds);
    let mut read_calls = 0u64;
    let mut empty_reads = 0u64;
    let mut read_ns = 0u128;
    let mut verification_ns = 0u128;
    let mut wait_ns = 0u128;
    // This is a synthetic slow-consumer clock, not a per-read minimum delay.
    // Preserve phase after scheduler oversleep so 11,156 fragmented reads do
    // not accumulate timer coalescing into a false transport liveness failure.
    let consumer_period = Duration::from_micros(100);
    let mut next_consumer_tick = sequence_started;
    let mut received = 0;
    let mut metrics = metrics::ReadMetrics::enabled();
    let verification = (|| -> Result<(), String> {
        while received < total && Instant::now() < deadline {
            let read_started = Instant::now();
            let frames = if let Some(metrics) = &mut metrics {
                metrics.measure(|| reader.read(&mut samples))
            } else {
                reader.read(&mut samples)
            };
            read_ns += read_started.elapsed().as_nanos();
            read_calls += 1;
            empty_reads += u64::from(frames == 0);
            let verification_started = Instant::now();
            if frames > 47 || received + frames > total {
                return Err(format!("unexpected frame count {frames} after {received}"));
            }
            for (offset, &sample) in samples[..frames * channels].iter().enumerate() {
                let expected = (received * channels + offset + 1) as f32;
                if sample != expected {
                    return Err(format!(
                        "sequence mismatch at sample {}: expected {expected}, got {sample}",
                        received * channels + offset
                    ));
                }
            }
            received += frames;
            verification_ns += verification_started.elapsed().as_nanos();
            let wait_started = Instant::now();
            // Deliberately let the producer fill the ring repeatedly, without
            // adding another full delay to clock debt from a late wakeup.
            next_consumer_tick += consumer_period;
            let delay = next_consumer_tick.saturating_duration_since(Instant::now());
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            wait_ns += wait_started.elapsed().as_nanos();
        }
        Ok(())
    })();
    let control_updates = if let Some((stop, worker)) = storm {
        stop.store(true, std::sync::atomic::Ordering::Release);
        let updates = worker.join().map_err(|_| "control storm panicked")?;
        if updates == 0 {
            return Err("control storm made no acknowledged updates".into());
        }
        updates
    } else {
        0
    };
    // Emit partial progress before propagating failures, so a timeout does not
    // discard the evidence needed to distinguish reader work from pacing.
    let elapsed_ns = sequence_started.elapsed().as_nanos();
    let verified = verification.is_ok() && received == total;
    println!(
        "sequence_diagnostics pacing=phase-preserving channels={channels} rate={rate} block_frames={block_frames} verified={verified} received={received} expected={total} timeout_seconds={timeout_seconds} elapsed_ns={elapsed_ns} read_calls={read_calls} empty_reads={empty_reads} read_ns={read_ns} verification_ns={verification_ns} wait_ns={wait_ns}"
    );
    verification?;
    if received != total {
        return Err(format!(
            "sequence timeout: received {received}/{total} frames"
        ));
    }
    if let Some(metrics) = metrics {
        metrics.finish(channels, rate, block_frames, received, control_updates)?;
    }
    println!(
        "verified {received} frames, {channels} channels, {rate} Hz/{block_frames}, {control_updates} control updates"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn control(brokered: bool, encrypted: bool) -> Result<(), String> {
    use driver_common::DriverConfig;
    use driver_hal::SharedAudioBuffer;
    use std::io::{BufRead, Write};

    if std::env::var_os("SOTF_HAL_SHARED_MEMORY_PATH").is_none() {
        return Err("test worker requires an explicit private mapping path".into());
    }
    let buffer = if brokered {
        let buffer = SharedAudioBuffer::create_replacement_default(48_000, 64, 2)
            .map_err(|error| error.to_string())?;
        if encrypted {
            let key = std::array::from_fn(|index| index as u8);
            buffer.set_key_fingerprint(driver_hal::compute_fingerprint(&key));
            buffer.set_encrypted(true);
        }
        buffer.set_engine_ready(true);
        println!("ready");
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        buffer
    } else {
        // SAFETY: the Swift harness owns and pins the explicit fixture extent.
        unsafe { SharedAudioBuffer::open_fixture(driver_hal::get_shared_memory_path()) }
            .map_err(|error| error.to_string())?
    };
    let mut reader: Option<driver_hal::HalInputReader> = None;
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        let words: Vec<_> = line.split_whitespace().collect();
        let number = |index: usize| -> Result<u64, String> {
            words
                .get(index)
                .ok_or("missing number")?
                .parse()
                .map_err(|_| "invalid number".into())
        };
        match words.first().copied() {
            Some("request") => {
                let config =
                    DriverConfig::new(number(1)? as u32, number(2)? as u32, number(3)? as u32);
                let id = buffer
                    .publish_config_request(config, 2)
                    .ok_or("mailbox busy")?;
                println!("request {id}");
            }
            Some("ack") => {
                let actual =
                    DriverConfig::new(number(2)? as u32, number(3)? as u32, number(4)? as u32);
                let accepted =
                    buffer.acknowledge_config_request(number(1)?, actual, number(5)? as u32, 0);
                println!("ack {accepted}");
            }
            Some("snapshot") => {
                let snapshot = buffer.configuration_snapshot().ok_or("mailbox busy")?;
                println!(
                    "snapshot {} {} {} {} {} {} {}",
                    snapshot.request.request_id,
                    snapshot.request.config.sample_rate,
                    snapshot.request.config.buffer_frames,
                    snapshot.request.config.channel_count,
                    snapshot.status,
                    snapshot.actual.channel_count,
                    snapshot.pending
                );
            }
            Some("volume" | "patch") => {
                let iterations = number(1)?;
                if iterations > 100_000 {
                    return Err("too many iterations".into());
                }
                let patch = words[0] == "patch";
                for index in 0..iterations {
                    if patch {
                        buffer.patch_output_volume(Some((index % 1024) as f32 / 1024.0), None);
                    } else {
                        buffer.set_output_volume(0.25, false);
                    }
                    let (volume, muted, _) = buffer.output_volume();
                    if patch {
                        if !muted {
                            return Err("scalar patch lost concurrent mute intent".into());
                        }
                    } else if !((volume == 0.25 && !muted) || (volume == 0.75 && muted)) {
                        return Err(format!("torn volume tuple: {volume}/{muted}"));
                    }
                }
                println!("ok");
            }
            Some("getvolume") => {
                let (scalar, muted, generation) = buffer.output_volume();
                println!("volume {} {muted} {generation}", scalar.to_bits());
            }
            Some("verify") if brokered => {
                let frames = usize::try_from(number(1)?).map_err(|_| "invalid frames")?;
                if frames == 0 || frames > 2048 {
                    return Err("invalid verification frame count".into());
                }
                let bits = u32::try_from(number(2)?).map_err(|_| "invalid sample bits")?;
                if reader.is_none() {
                    reader =
                        Some(driver_hal::HalInputReader::new().ok_or("broker attachment failed")?);
                }
                let reader = reader.as_mut().ok_or("missing reader")?;
                if reader.needs_cipher_reload() {
                    reader.reload_cipher().map_err(|error| error.to_string())?;
                }
                let mut samples = vec![0.0; frames * 2];
                let received = reader.read(&mut samples);
                if received != frames || samples.iter().any(|sample| sample.to_bits() != bits) {
                    return Err(format!("verification failed: received {received}/{frames}"));
                }
                println!("verified {frames}");
            }
            Some("quit") => return Ok(()),
            _ => return Err("invalid control command".into()),
        }
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn main() {
    #[cfg(target_os = "macos")]
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("hal-capture-stress requires macOS");
        std::process::exit(1);
    }
}
