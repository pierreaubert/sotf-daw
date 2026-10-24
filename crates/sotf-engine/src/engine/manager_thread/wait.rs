use super::super::{DecoderThread, ProcessingThread};
use super::consts::SPIN_MS_SLEEP_MANAGER;
use super::error::ConfigError;

pub(in crate::engine::manager_thread) fn wait_for_processing_ack(
    processing: &ProcessingThread,
    request_id: u64,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    let reconciliation_deadline = deadline + timeout.min(std::time::Duration::from_secs(1));
    let mut reconciling_claimed_request = false;

    loop {
        if let Some(response) = processing.try_recv_response_for(request_id) {
            match response {
                super::super::ProcessingResponse::Ok => return Ok(()),
                super::super::ProcessingResponse::Error(e) => return Err(e),
                _ => continue,
            }
        }

        if !reconciling_claimed_request && std::time::Instant::now() >= deadline {
            if !reconciling_claimed_request && !processing.cancel_request(request_id) {
                reconciling_claimed_request = true;
                continue;
            }
            processing.abandon_request(request_id);
            return Err(format!(
                "Timed out waiting for processing thread acknowledgment after {}ms",
                timeout.as_millis()
            ));
        }

        if reconciling_claimed_request && processing.is_finished() {
            processing.abandon_request(request_id);
            return Err("Processing thread stopped while completing a claimed command".to_string());
        }

        if reconciling_claimed_request && std::time::Instant::now() >= reconciliation_deadline {
            processing.quarantine();
            processing.abandon_request(request_id);
            return Err("Processing worker quarantined after a claimed command stalled".into());
        }

        std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_SLEEP_MANAGER));
    }
}

pub(in crate::engine::manager_thread) fn wait_for_parameter_update(
    processing: &ProcessingThread,
    request_id: u64,
    timeout: std::time::Duration,
) -> Result<(usize, u32, usize), String> {
    let deadline = std::time::Instant::now() + timeout;
    let reconciliation_deadline = deadline + timeout.min(std::time::Duration::from_secs(1));
    let mut reconciling_claimed_request = false;
    loop {
        if let Some(response) = processing.try_recv_response_for(request_id) {
            match response {
                super::super::ProcessingResponse::ParameterUpdated {
                    output_channels,
                    output_sample_rate,
                    latency_samples,
                } => return Ok((output_channels, output_sample_rate, latency_samples)),
                super::super::ProcessingResponse::Error(e) => return Err(e),
                _ => continue,
            }
        }
        if !reconciling_claimed_request && std::time::Instant::now() >= deadline {
            if !reconciling_claimed_request && !processing.cancel_request(request_id) {
                reconciling_claimed_request = true;
                continue;
            }
            processing.abandon_request(request_id);
            return Err(format!(
                "Timed out waiting for parameter update after {}ms",
                timeout.as_millis()
            ));
        }
        if reconciling_claimed_request && processing.is_finished() {
            processing.abandon_request(request_id);
            return Err(
                "Processing thread stopped while completing a claimed parameter update".to_string(),
            );
        }
        if reconciling_claimed_request && std::time::Instant::now() >= reconciliation_deadline {
            processing.quarantine();
            processing.abandon_request(request_id);
            return Err(
                "Processing worker quarantined after a claimed parameter update stalled".into(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_SLEEP_MANAGER));
    }
}

pub(in crate::engine::manager_thread) fn wait_for_plugin_chain_update(
    processing: &ProcessingThread,
    request_id: u64,
    generation: u64,
    ticket: &super::super::HostUpdateTicket,
    timeout: std::time::Duration,
) -> Result<(usize, u32, usize), ConfigError> {
    let deadline = std::time::Instant::now() + timeout;
    let reconciliation_deadline = deadline + timeout.min(std::time::Duration::from_secs(1));
    let mut reconciling_committed_update = false;

    loop {
        if let Some(response) = processing.try_recv_response_for(request_id) {
            match response {
                super::super::ProcessingResponse::PluginChainUpdated {
                    generation: response_generation,
                    output_channels,
                    output_sample_rate,
                    latency_samples,
                    ..
                } if response_generation == generation => {
                    return Ok((output_channels, output_sample_rate, latency_samples));
                }
                super::super::ProcessingResponse::PluginChainUpdated { .. } => continue,
                super::super::ProcessingResponse::Error(reason) => {
                    return Err(ConfigError::ProcessingError { reason });
                }
                super::super::ProcessingResponse::PluginData(_)
                | super::super::ProcessingResponse::ParameterUpdated { .. }
                | super::super::ProcessingResponse::Ok => {
                    continue;
                }
            }
        }

        if !reconciling_committed_update && std::time::Instant::now() >= deadline {
            if !reconciling_committed_update && !ticket.cancel() {
                // Processing atomically claimed the update before our timeout.
                // It is no longer cancellable, so reconcile its matching ACK
                // instead of returning a false failure for a committed host.
                reconciling_committed_update = true;
                continue;
            }

            processing.invalidate_host_update(generation);
            return Err(ConfigError::TimeoutError {
                waited_ms: timeout.as_millis() as u64,
            });
        }

        if reconciling_committed_update && processing.is_finished() {
            processing.invalidate_host_update(generation);
            return Err(ConfigError::ProcessingError {
                reason: "Processing thread stopped after claiming a host update before it acknowledged the terminal state".to_string(),
            });
        }

        if reconciling_committed_update && std::time::Instant::now() >= reconciliation_deadline {
            processing.quarantine();
            processing.invalidate_host_update(generation);
            processing.abandon_request(request_id);
            return Err(ConfigError::ProcessingError {
                reason: "Processing worker quarantined after a claimed host update stalled".into(),
            });
        }

        std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_SLEEP_MANAGER));
    }
}

pub(in crate::engine::manager_thread) fn wait_for_decoder_ack(
    decoder: &DecoderThread,
    request_id: u64,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if let Some(response) = decoder.try_recv_response_for(request_id) {
            return match response {
                super::super::DecoderResponse::Ok => Ok(()),
                super::super::DecoderResponse::Error(e) => Err(e),
            };
        }

        std::thread::sleep(std::time::Duration::from_millis(SPIN_MS_SLEEP_MANAGER));
    }

    decoder.abandon_request(request_id);
    Err(format!(
        "Timed out waiting for decoder thread acknowledgment after {}ms",
        timeout.as_millis()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::processing_thread::ProcessingReply;
    use crate::engine::{ProcessingCommand, ProcessingResponse};
    use std::time::Duration;

    fn assert_claimed_wait_is_bounded(
        wait: impl FnOnce(&ProcessingThread, u64) -> bool + Send + 'static,
    ) {
        let (processing, commands, replies) = ProcessingThread::response_probe();
        let request_id = processing
            .send_command(ProcessingCommand::Bypass(true))
            .unwrap();
        let request = commands.recv().unwrap();
        assert!(request.ticket.try_claim());
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let failed = wait(&processing, request_id);
            let quarantined = processing.is_quarantined();
            let rejects_update = processing
                .send_command(ProcessingCommand::Bypass(false))
                .is_err();
            done_tx.send((failed, quarantined, rejects_update)).unwrap();
        });
        let result = done_rx.recv_timeout(Duration::from_millis(500));
        if result.is_err() {
            // Release the old unbounded implementation before failing the test.
            replies
                .send(ProcessingReply {
                    id: request_id,
                    response: ProcessingResponse::Error("released test fixture".into()),
                })
                .unwrap();
        }
        waiter.join().unwrap();
        assert_eq!(
            result.expect("claimed command monopolized the manager"),
            (true, true, true)
        );
    }

    #[test]
    fn claimed_processing_command_has_a_bounded_reconciliation_deadline() {
        assert_claimed_wait_is_bounded(|processing, request_id| {
            wait_for_processing_ack(processing, request_id, Duration::from_millis(5)).is_err()
        });
    }

    #[test]
    fn claimed_parameter_update_has_a_bounded_reconciliation_deadline() {
        assert_claimed_wait_is_bounded(|processing, request_id| {
            wait_for_parameter_update(processing, request_id, Duration::from_millis(5)).is_err()
        });
    }

    #[test]
    fn claimed_host_update_has_a_bounded_reconciliation_deadline() {
        assert_claimed_wait_is_bounded(|processing, request_id| {
            let ticket = crate::engine::HostUpdateTicket::new();
            assert!(ticket.try_commit());
            wait_for_plugin_chain_update(
                processing,
                request_id,
                1,
                &ticket,
                Duration::from_millis(5),
            )
            .is_err()
        });
    }
}
