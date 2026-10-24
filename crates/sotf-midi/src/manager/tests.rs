use super::input_buffer::InputBuffer;
use super::midi_manager::MidiManager;
use crate::error::MidiError;
use crate::message::MidiMessage;

#[test]
fn test_manager_creation() {
    let manager = MidiManager::new();
    assert!(manager.is_ok());
}

#[test]
fn long_messages_reach_output_connection_check_without_panicking() {
    let manager = MidiManager::new().unwrap();
    for message in [
        MidiMessage::IdentityRequest { device: 0x7f },
        MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x01, 0x02, 0xF7],
        },
    ] {
        assert!(
            matches!(manager.send_message(&message), Err(MidiError::NotConnected)),
            "long message should reach the disconnected-output check: {message:?}"
        );
    }
}

#[test]
fn test_device_enumeration() {
    let mut manager = MidiManager::new().unwrap();

    // Should not panic even if no devices are available
    let inputs = manager.list_input_devices();
    if let Err(err) = inputs {
        eprintln!("Skipping MIDI enumeration test: input backend unavailable ({err})");
        return;
    }

    let outputs = manager.list_output_devices();
    if let Err(err) = outputs {
        eprintln!("Skipping MIDI enumeration test: output backend unavailable ({err})");
    }
}

#[test]
fn input_buffer_reassembles_split_sysex() {
    let mut buffer = InputBuffer::new();

    assert!(buffer.parse_message(&[0xF0, 0x7D, 0x01]).is_none());
    let msg = buffer.parse_message(&[0x02, 0x03, 0xF7]).unwrap().unwrap();

    assert_eq!(
        msg,
        MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x01, 0x02, 0x03, 0xF7]
        }
    );
}

#[test]
fn input_buffer_passes_stack_sized_system_messages() {
    let mut buffer = InputBuffer::new();

    let msg = buffer.parse_message(&[0xF8]).unwrap().unwrap();

    assert_eq!(
        msg,
        MidiMessage::System {
            status: 0xF8,
            data: [0, 0],
            len: 0
        }
    );
}

#[test]
fn input_buffer_preserves_sysex_when_realtime_arrives_mid_packet() {
    let mut buffer = InputBuffer::new();

    assert!(buffer.parse_message(&[0xF0, 0x7D, 0x01]).is_none());
    let clock = buffer.parse_message(&[0xF8]).unwrap().unwrap();
    assert!(matches!(clock, MidiMessage::System { status: 0xF8, .. }));

    let sysex = buffer.parse_message(&[0x02, 0xF7]).unwrap().unwrap();
    assert_eq!(
        sysex,
        MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x01, 0x02, 0xF7]
        }
    );
}

#[test]
fn input_buffer_bounds_unfinished_sysex_and_recovers_after_terminator() {
    const MAX_SYSEX_BYTES: usize = 1024 * 1024;
    let mut buffer = InputBuffer::new();
    assert!(buffer.parse_message(&[0xF0]).is_none());
    let chunk = vec![0x01; MAX_SYSEX_BYTES];
    assert!(matches!(
        buffer.parse_message(&chunk),
        Some(Err(MidiError::InvalidMessage(_)))
    ));
    assert!(buffer.sysex.is_empty());
    assert!(buffer.parse_message(&[0x02, 0x03]).is_none());
    assert!(matches!(
        buffer.parse_message(&[0xF8]),
        Some(Ok(MidiMessage::System { status: 0xF8, .. }))
    ));
    assert!(buffer.parse_message(&[0xF7]).is_none());
    assert_eq!(
        buffer.parse_message(&[0x90, 60, 100]).unwrap().unwrap(),
        MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        }
    );
}

#[test]
fn input_buffer_rejects_oversized_complete_sysex() {
    const MAX_SYSEX_BYTES: usize = 1024 * 1024;
    let mut buffer = InputBuffer::new();
    let mut message = vec![0x01; MAX_SYSEX_BYTES + 1];
    message[0] = 0xF0;
    *message.last_mut().unwrap() = 0xF7;
    assert!(matches!(
        buffer.parse_message(&message),
        Some(Err(MidiError::InvalidMessage(_)))
    ));
    assert!(buffer.sysex.is_empty());
}

#[test]
fn input_buffer_recovers_channel_status_after_discarded_sysex() {
    let mut buffer = InputBuffer::new();
    assert!(buffer.parse_message(&[0xF0]).is_none());
    let chunk = vec![0x01; super::input_buffer::MAX_SYSEX_BYTES];
    assert!(matches!(
        buffer.parse_message(&chunk),
        Some(Err(MidiError::InvalidMessage(_)))
    ));
    assert_eq!(buffer.discarded_sysex_count(), 1);
    assert!(matches!(
        buffer.parse_message(&[0xF8]),
        Some(Ok(MidiMessage::System { status: 0xF8, .. }))
    ));
    assert_eq!(
        buffer.parse_message(&[0x80, 60, 0]).unwrap().unwrap(),
        MidiMessage::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        }
    );
    assert_eq!(buffer.discarded_sysex_count(), 1);
}

#[test]
fn input_buffer_restarts_interrupted_sysex_and_counts_discard() {
    let mut buffer = InputBuffer::new();
    assert!(buffer.parse_message(&[0xF0, 0x7D, 0x01]).is_none());
    assert_eq!(
        buffer.parse_message(&[0x90, 64, 100]).unwrap().unwrap(),
        MidiMessage::NoteOn {
            channel: 0,
            note: 64,
            velocity: 100,
        }
    );
    assert_eq!(buffer.discarded_sysex_count(), 1);
    assert!(buffer.parse_message(&[0xF0, 0x7D, 0x02]).is_none());
    assert_eq!(
        buffer.parse_message(&[0xF7]).unwrap().unwrap(),
        MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x02, 0xF7]
        }
    );
}

#[test]
fn timed_input_keeps_sysex_start_and_completion_metadata() {
    let mut buffer = InputBuffer::new();
    assert!(buffer
        .parse_timed_message(100, 2, 7, &[0xF0, 0x7D])
        .is_none());
    let clock = buffer
        .parse_timed_message(110, 2, 7, &[0xF8])
        .unwrap()
        .unwrap();
    assert_eq!(clock.timestamp_micros, 110);
    assert_eq!(clock.received_at_micros, 110);
    assert_eq!(clock.sequence, 1);
    let sysex = buffer
        .parse_timed_message(120, 2, 7, &[0x01, 0xF7])
        .unwrap()
        .unwrap();
    assert_eq!(sysex.timestamp_micros, 100);
    assert_eq!(sysex.received_at_micros, 120);
    assert_eq!(sysex.port_index, 2);
    assert_eq!(sysex.connection_id, 7);
    assert_eq!(sysex.sequence, 2);
    assert_eq!(
        sysex.message,
        MidiMessage::SystemExclusive {
            data: vec![0xF0, 0x7D, 0x01, 0xF7]
        }
    );
}

fn assert_missing_device_or_skip_backend(result: crate::error::Result<()>, device_kind: &str) {
    match result {
        Err(crate::error::MidiError::ConnectionError(_)) => {}
        Err(crate::error::MidiError::InitError(err)) => {
            eprintln!(
                "Skipping missing {device_kind} device test: MIDI backend unavailable ({err})"
            );
        }
        other => panic!(
            "expected ConnectionError for missing {device_kind} device, got {:?}",
            other
        ),
    }
}

#[test]
fn connect_input_by_name_missing_device_returns_error() {
    let mut manager = MidiManager::new().unwrap();

    let result = manager.connect_input_by_name("Definitely Not Present Input", |_msg| {});
    assert_missing_device_or_skip_backend(result, "input");
}

#[test]
fn connect_output_by_name_missing_device_returns_error() {
    let mut manager = MidiManager::new().unwrap();

    let result = manager.connect_output_by_name("Definitely Not Present Output");
    assert_missing_device_or_skip_backend(result, "output");
}

#[test]
fn manager_with_missing_default_devices_constructs_safely() {
    use crate::config::MidiConfig;

    let config = MidiConfig {
        default_input: Some("Missing Input".to_string()),
        default_output: Some("Missing Output".to_string()),
        ..Default::default()
    };

    let manager = MidiManager::with_config(config).unwrap();
    assert!(!manager.is_input_connected());
    assert!(!manager.is_output_connected());
    assert_eq!(
        manager.config().default_input,
        Some("Missing Input".to_string())
    );
}
