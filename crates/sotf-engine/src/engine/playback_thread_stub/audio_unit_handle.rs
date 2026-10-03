use super::super::{PlaybackCommand, ProcessingMessage, ThreadEvent};
use super::misc::core_audio_ffi as ca;
use super::misc::playback_buffer_capacity;
use super::playback_state::PlaybackState;
use super::types::RenderContext;
use super::types::render_callback;
use rtrb::{Consumer, RingBuffer};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender};

pub(super) struct AudioUnitHandle {
    pub(super) instance: ca::AudioComponentInstance,
    // RenderContext is heap-allocated and lives as long as the AudioUnit.
    // The raw pointer is passed to the render callback.
    pub(super) _render_ctx: Box<RenderContext>,
}

impl AudioUnitHandle {
    pub(super) fn new(
        sample_rate: u32,
        channels: usize,
        consumer: Consumer<f32>,
        state: Arc<PlaybackState>,
    ) -> Result<Self, String> {
        let desc = ca::AudioComponentDescription {
            component_type: ca::kAudioUnitType_Output,
            component_sub_type: ca::kAudioUnitSubType_RemoteIO,
            component_manufacturer: ca::kAudioUnitManufacturer_Apple,
            component_flags: 0,
            component_flags_mask: 0,
        };

        let component = unsafe { ca::AudioComponentFindNext(std::ptr::null_mut(), &desc) };
        if component.is_null() {
            return Err("RemoteIO AudioComponent not found".to_string());
        }

        let mut instance: ca::AudioComponentInstance = std::ptr::null_mut();
        let status = unsafe { ca::AudioComponentInstanceNew(component, &mut instance) };
        if status != ca::noErr {
            return Err(format!("AudioComponentInstanceNew failed: {}", status));
        }

        // Enable output on bus 0
        let enable_output: u32 = 1;
        let status = unsafe {
            ca::AudioUnitSetProperty(
                instance,
                ca::kAudioOutputUnitProperty_EnableIO,
                ca::kAudioUnitScope_Output,
                0,
                &enable_output as *const u32 as *const _,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if status != ca::noErr {
            log::warn!(
                "[iOS AudioUnit] EnableIO failed: {} (continuing anyway)",
                status
            );
        }

        // Set stream format: interleaved f32
        let asbd = ca::AudioStreamBasicDescription {
            sample_rate: sample_rate as f64,
            format_id: ca::kAudioFormatLinearPCM,
            format_flags: ca::kAudioFormatFlagIsFloat | ca::kAudioFormatFlagIsPacked,
            bytes_per_packet: (channels * std::mem::size_of::<f32>()) as u32,
            frames_per_packet: 1,
            bytes_per_frame: (channels * std::mem::size_of::<f32>()) as u32,
            channels_per_frame: channels as u32,
            bits_per_channel: 32,
            reserved: 0,
        };

        let status = unsafe {
            ca::AudioUnitSetProperty(
                instance,
                ca::kAudioUnitProperty_StreamFormat,
                ca::kAudioUnitScope_Input,
                0, // bus 0 = output
                &asbd as *const ca::AudioStreamBasicDescription as *const _,
                std::mem::size_of::<ca::AudioStreamBasicDescription>() as u32,
            )
        };
        if status != ca::noErr {
            unsafe { ca::AudioComponentInstanceDispose(instance) };
            return Err(format!("Set stream format failed: {}", status));
        }

        // Set max frames per slice
        let max_frames: u32 = 4096;
        let status = unsafe {
            ca::AudioUnitSetProperty(
                instance,
                ca::kAudioUnitProperty_MaximumFramesPerSlice,
                ca::kAudioUnitScope_Global,
                0,
                &max_frames as *const u32 as *const _,
                std::mem::size_of::<u32>() as u32,
            )
        };
        if status != ca::noErr {
            log::warn!(
                "[iOS AudioUnit] MaxFramesPerSlice failed: {status}; render callback supports the device-provided slice directly"
            );
        }

        // Create render context (heap-allocated, stable address)
        let render_ctx = Box::new(RenderContext {
            consumer,
            state,
            sample_rate,
            channels,
        });

        // Set render callback
        let callback_struct = ca::AURenderCallbackStruct {
            input_proc: Some(render_callback),
            input_proc_ref_con: &*render_ctx as *const RenderContext as *mut _,
        };

        let status = unsafe {
            ca::AudioUnitSetProperty(
                instance,
                ca::kAudioUnitProperty_SetRenderCallback,
                ca::kAudioUnitScope_Input,
                0,
                &callback_struct as *const ca::AURenderCallbackStruct as *const _,
                std::mem::size_of::<ca::AURenderCallbackStruct>() as u32,
            )
        };
        if status != ca::noErr {
            unsafe { ca::AudioComponentInstanceDispose(instance) };
            return Err(format!("Set render callback failed: {}", status));
        }

        // Initialize
        let status = unsafe { ca::AudioUnitInitialize(instance) };
        if status != ca::noErr {
            unsafe { ca::AudioComponentInstanceDispose(instance) };
            return Err(format!("AudioUnitInitialize failed: {}", status));
        }

        // Start
        let status = unsafe { ca::AudioOutputUnitStart(instance) };
        if status != ca::noErr {
            unsafe {
                ca::AudioUnitUninitialize(instance);
                ca::AudioComponentInstanceDispose(instance);
            }
            return Err(format!("AudioOutputUnitStart failed: {}", status));
        }

        log::info!(
            "[iOS AudioUnit] Started: {}Hz, {}ch, interleaved f32",
            sample_rate,
            channels
        );

        Ok(Self {
            instance,
            _render_ctx: render_ctx,
        })
    }
}

impl Drop for AudioUnitHandle {
    fn drop(&mut self) {
        unsafe {
            ca::AudioOutputUnitStop(self.instance);
            ca::AudioUnitUninitialize(self.instance);
            ca::AudioComponentInstanceDispose(self.instance);
        }
        log::info!("[iOS AudioUnit] Stopped and disposed");
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Keeps native worker dependencies explicit for the iOS playback thread"
)]
pub(super) fn run_playback_ios(
    message_rx: Receiver<ProcessingMessage>,
    command_rx: Receiver<PlaybackCommand>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    sample_rate: u32,
    buffer_ms: u32,
    channels: usize,
    frame_size: usize,
    recycle_tx: SyncSender<Vec<f32>>,
    shared_output_peak_bits: Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), String> {
    // Create ring buffer
    let buffer_capacity = playback_buffer_capacity(sample_rate, channels, buffer_ms)
        .max(frame_size.saturating_mul(channels));
    let (producer, consumer) = RingBuffer::<f32>::new(buffer_capacity);

    // Create shared state (peak atomic shared with the wrapper, which
    // retains it past thread exit for the worker-death residual fold).
    let state = Arc::new(PlaybackState::new_sharing_peak(
        buffer_capacity,
        shared_output_peak_bits,
    ));

    // Create CoreAudio AudioUnit
    let _audio_unit = AudioUnitHandle::new(sample_rate, channels, consumer, Arc::clone(&state))?;

    event_tx
        .try_send(ThreadEvent::PlaybackChannelsChanged(channels))
        .ok();

    log::info!(
        "[Playback Thread iOS] Started - {}Hz, {}ch, buffer={}ms ({}samples)",
        sample_rate,
        channels,
        buffer_ms,
        buffer_capacity,
    );

    super::feeder::run_feeder(
        message_rx,
        command_rx,
        event_tx,
        sample_rate,
        channels,
        producer,
        state,
        recycle_tx,
    )
}
