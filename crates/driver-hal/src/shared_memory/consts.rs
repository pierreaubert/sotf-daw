use super::SharedAudioHeader;
use super::types::EncryptedRecordHeader;

/// Magic number for shared memory header validation: 'SOTF'
pub(super) const SHARED_MEMORY_MAGIC: u32 = 0x534F5446;

/// Current protocol version
/// Version 2: Added encryption fields (encrypted, key_fingerprint, frame_counter)
/// Version 3: Added config negotiation fields for bidirectional HAL-Daemon sync
/// Version 4: Added daemon heartbeat for stale-engine detection in the HAL driver
/// Version 5: Promoted all cross-process geometry/config fields to atomics and
///            added the `configuring` quiesce handshake flag.
/// Version 6: Added `configuring_ack` and a separate requested channel count
///            so pending requests never mutate live ring geometry.
/// Version 7: Added output volume fields (`volume_scalar_bits`,
///            `output_muted`, `volume_generation`) in former padding slots
///            so the HAL volume control (OS media keys) and the daemon
///            share one volume state.
pub(super) const SHARED_MEMORY_VERSION: u32 = 7;

pub const DEFAULT_HAL_CHANNEL_COUNT: u32 = 2;

pub const MAX_HAL_CHANNEL_COUNT: u32 = 32;

pub const MAX_HAL_BUFFER_FRAMES: u32 = 4096;

/// Highest sample rate the transport negotiates (matches the daemon's
/// supported-rate table). Creation sizes the file for the ring depth at
/// this rate so a later mid-session rate increase never hits the mmap edge.
pub const MAX_HAL_SAMPLE_RATE: u32 = 192_000;

/// Ring depth in blocks at the reference rate.
pub(super) const RING_BLOCKS_AT_REFERENCE_RATE: usize = 8;

/// Reference rate for time-based ring sizing (the historical default).
pub(super) const RING_REFERENCE_SAMPLE_RATE: u32 = 48_000;

/// Ring depth in blocks so the ring holds roughly the same *duration* at
/// any sample rate.
///
/// Frames are not time: 8 blocks of 512 stereo frames hold 85 ms at 48 kHz
/// but only 42.7 ms at 96 kHz, so any daemon or decoder stall over ~43 ms
/// lapped the ring at 96 kHz and produced scratchy drop-oldest noise. The
/// Swift HAL computes the identical depth via `ringBufferCount(forSampleRate:)`.
pub(super) fn ring_block_count(sample_rate: u32) -> usize {
    let multiplier = sample_rate
        .max(1)
        .div_ceil(RING_REFERENCE_SAMPLE_RATE)
        .max(1) as usize;
    RING_BLOCKS_AT_REFERENCE_RATE * multiplier
}

/// Total ring capacity in interleaved samples for one geometry.
pub(super) fn ring_capacity_samples(
    buffer_frames: usize,
    channel_count: usize,
    sample_rate: u32,
) -> Option<usize> {
    buffer_frames
        .checked_mul(channel_count)
        .and_then(|value| value.checked_mul(ring_block_count(sample_rate)))
}

/// Bound on the spin period while waiting for the writer to observe
/// `configuring=1`. Reconfig is rare and user-driven, so a small spin
/// is acceptable.
pub(super) const RECONFIG_QUIESCE_TIMEOUT_NS: u64 = 5_000_000; // 5 ms

/// Encrypted audio record magic: 'SEA1' (SotF Encrypted Audio v1)
pub(super) const ENCRYPTED_RECORD_MAGIC: u32 = 0x5345_4131;

pub(super) const ENCRYPTED_RECORD_HEADER_BYTES: usize = 24;

pub(super) const ENCRYPTED_RECORD_HEADER_SLOTS: usize = ENCRYPTED_RECORD_HEADER_BYTES / 4;

/// Upper bound on encrypted record sample counts. Defends against bogus
/// header values that would cause integer overflow before
/// `audio_capacity` would otherwise catch them.
pub(super) const MAX_ENCRYPTED_SAMPLE_COUNT: usize =
    MAX_HAL_BUFFER_FRAMES as usize * MAX_HAL_CHANNEL_COUNT as usize;

pub(super) fn write_encrypted_record_header(
    output: &mut [u8],
    sample_count: usize,
    frame_counter: u64,
    ciphertext_len: usize,
) -> bool {
    if output.len() < ENCRYPTED_RECORD_HEADER_BYTES
        || sample_count > u32::MAX as usize
        || ciphertext_len > u32::MAX as usize
    {
        return false;
    }

    output[0..4].copy_from_slice(&ENCRYPTED_RECORD_MAGIC.to_be_bytes());
    output[4..8].copy_from_slice(&(sample_count as u32).to_be_bytes());
    output[8..16].copy_from_slice(&frame_counter.to_be_bytes());
    output[16..20].copy_from_slice(&(ciphertext_len as u32).to_be_bytes());
    output[20..24].copy_from_slice(&0u32.to_be_bytes());
    true
}

pub(super) fn parse_encrypted_record_header(
    bytes: &[u8],
    audio_capacity: usize,
) -> Option<EncryptedRecordHeader> {
    if bytes.len() < ENCRYPTED_RECORD_HEADER_BYTES {
        return None;
    }

    let magic = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
    if magic != ENCRYPTED_RECORD_MAGIC {
        return None;
    }

    let sample_count = u32::from_be_bytes(bytes[4..8].try_into().ok()?) as usize;
    let frame_counter = u64::from_be_bytes(bytes[8..16].try_into().ok()?);
    let ciphertext_len = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as usize;
    let reserved = u32::from_be_bytes(bytes[20..24].try_into().ok()?);

    if sample_count == 0 || sample_count > MAX_ENCRYPTED_SAMPLE_COUNT || reserved != 0 {
        return None;
    }

    let expected_ciphertext_len = crate::encryption::encrypted_byte_size_checked(sample_count)?;
    if ciphertext_len != expected_ciphertext_len {
        return None;
    }

    let total_bytes = ENCRYPTED_RECORD_HEADER_BYTES.checked_add(ciphertext_len)?;
    let slot_count = total_bytes.div_ceil(4);
    if slot_count == 0 || slot_count > audio_capacity {
        return None;
    }

    Some(EncryptedRecordHeader {
        sample_count,
        frame_counter,
        ciphertext_len,
        total_bytes,
        slot_count,
    })
}

const _: () = assert!(std::mem::size_of::<SharedAudioHeader>() == 144);

const _: () = assert!(std::mem::align_of::<SharedAudioHeader>() == 8);

pub(super) fn pre_alloc_capacity_samples() -> usize {
    MAX_HAL_BUFFER_FRAMES as usize * MAX_HAL_CHANNEL_COUNT as usize
}
