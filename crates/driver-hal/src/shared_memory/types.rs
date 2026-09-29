use super::shared_audio_buffer::ReadCommit;
use crate::reader_state::{ReaderIdentity, StagedPlaintext};

#[derive(Debug, Clone, Copy)]
pub(super) struct EncryptedRecordHeader {
    pub(super) sample_count: usize,
    pub(super) frame_counter: u64,
    pub(super) ciphertext_len: usize,
    pub(super) total_bytes: usize,
    pub(super) slot_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EncryptedRecordRead {
    Empty,
    InvalidHeader,
    OutputTooSmall { sample_count: usize },
    Corrupt { frame_counter: u64 },
    Read { sample_count: usize },
}

#[allow(
    clippy::too_many_arguments,
    reason = "preallocated encrypted read scratch"
)]
pub(super) fn read_encrypted_with_staging(
    commit: &mut ReadCommit<'_>,
    identity: ReaderIdentity,
    output: &mut [f32],
    cipher: &crate::encryption::AudioCipher,
    encrypted_samples_buf: &mut Vec<f32>,
    ciphertext_buf: &mut Vec<u8>,
    decrypted_record_buf: &mut Vec<f32>,
    pending: &mut StagedPlaintext,
) -> usize {
    // Caller checked geometry, encryption and cached cipher under this guard.
    let channels = identity.channel_count as usize;
    let requested_samples = output.len() / channels * channels;
    let mut copied_samples = pending.copy_into(identity, &mut output[..requested_samples]);

    while copied_samples < requested_samples {
        match commit.read_next_encrypted_record_into(
            decrypted_record_buf,
            cipher,
            encrypted_samples_buf,
            ciphertext_buf,
        ) {
            EncryptedRecordRead::Read { sample_count } => {
                let to_copy = sample_count.min(requested_samples - copied_samples);
                output[copied_samples..copied_samples + to_copy]
                    .copy_from_slice(&decrypted_record_buf[..to_copy]);
                copied_samples += to_copy;
                if to_copy < sample_count {
                    if !pending.stage(identity, &decrypted_record_buf[to_copy..sample_count]) {
                        output.fill(0.0);
                        return 0;
                    }
                    break;
                }
            }
            EncryptedRecordRead::OutputTooSmall { sample_count } => {
                if decrypted_record_buf.capacity() < sample_count {
                    pending.invalidate();
                    output.fill(0.0);
                    return 0;
                }
                decrypted_record_buf.resize(sample_count, 0.0);
            }
            EncryptedRecordRead::Corrupt { .. } | EncryptedRecordRead::InvalidHeader => {
                pending.invalidate();
                break;
            }
            EncryptedRecordRead::Empty => break,
        }
    }
    // The caller rechecks identity before returning; key/mode stores are not
    // serialized by the geometry guard in protocol v6.
    if copied_samples == 0 {
        output.fill(0.0);
    }
    copied_samples / channels
}
