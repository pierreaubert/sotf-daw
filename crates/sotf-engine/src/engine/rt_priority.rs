// ============================================================================
// Real-Time Thread Priority
// ============================================================================
//
// Platform-specific thread priority elevation for audio threads.
// Gracefully degrades if RT privileges are unavailable.

/// Priority level for audio threads.
#[derive(Debug, Clone, Copy)]
pub enum RtPriority {
    /// Reserved for a backend-owned hardware callback integration. The cpal
    /// playback feeder must not use this hard realtime policy.
    #[allow(
        dead_code,
        reason = "reserved for a future callback-owned backend hook"
    )]
    Playback,
    /// Elevated but not hard-RT (processing thread)
    Processing,
}

/// Attempt to elevate the calling thread's priority.
///
/// Returns `Ok(true)` if priority was successfully set, `Ok(false)` if the
/// platform doesn't support it, or `Err` on failure.
// `audio_timing` is consumed only by the macOS priority path.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub fn set_realtime_priority(
    level: RtPriority,
    audio_timing: Option<(u32, usize)>,
) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        set_priority_macos(level, audio_timing)
    }

    #[cfg(target_os = "linux")]
    {
        set_priority_linux(level)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (level, audio_timing);
        Ok(false)
    }
}

// ============================================================================
// macOS: QoS class for processing, time-constraint for playback
// ============================================================================

#[cfg(target_os = "macos")]
fn set_priority_macos(
    level: RtPriority,
    audio_timing: Option<(u32, usize)>,
) -> Result<bool, String> {
    match level {
        RtPriority::Playback => match set_time_constraint_priority_macos(audio_timing) {
            Ok(()) => {
                log::info!("[RT Priority] macOS: set THREAD_TIME_CONSTRAINT_POLICY for playback");
                Ok(true)
            }
            Err(err) => {
                log::warn!(
                    "[RT Priority] macOS: failed to set time-constraint policy ({err}), falling back to QoS"
                );
                set_qos_priority_macos(level)
            }
        },
        RtPriority::Processing => set_qos_priority_macos(level),
    }
}

#[cfg(target_os = "macos")]
fn set_qos_priority_macos(level: RtPriority) -> Result<bool, String> {
    // pthread_set_qos_class_self_np(qos_class, relative_priority)
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }

    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

    let ret = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    if ret == 0 {
        log::info!(
            "[RT Priority] macOS: set QoS class to USER_INTERACTIVE for {:?}",
            level
        );
        Ok(true)
    } else {
        log::warn!(
            "[RT Priority] macOS: failed to set QoS class (errno={}), continuing at default priority",
            ret
        );
        Ok(false)
    }
}

#[cfg(target_os = "macos")]
fn set_time_constraint_priority_macos(audio_timing: Option<(u32, usize)>) -> Result<(), String> {
    use std::mem::MaybeUninit;

    type BooleanT = i32;
    type IntegerT = i32;
    type KernReturnT = i32;
    type MachMsgTypeNumberT = u32;
    type MachPortT = u32;
    type ThreadPolicyFlavorT = i32;

    const KERN_SUCCESS: KernReturnT = 0;
    const MACH_PORT_NULL: MachPortT = 0;
    const THREAD_TIME_CONSTRAINT_POLICY: ThreadPolicyFlavorT = 2;
    const THREAD_TIME_CONSTRAINT_POLICY_COUNT: MachMsgTypeNumberT = 4;

    let audio_period_ns = audio_timing
        .and_then(|(sample_rate, frame_size)| audio_period_nanos(sample_rate, frame_size))
        .unwrap_or(2_900_000);
    let audio_computation_ns = (audio_period_ns / 2).max(1);

    #[repr(C)]
    struct MachTimebaseInfoData {
        numer: u32,
        denom: u32,
    }

    #[repr(C)]
    struct ThreadTimeConstraintPolicyData {
        period: u32,
        computation: u32,
        constraint: u32,
        preemptible: BooleanT,
    }

    unsafe extern "C" {
        static mach_task_self_: MachPortT;

        fn mach_thread_self() -> MachPortT;
        fn mach_port_deallocate(task: MachPortT, name: MachPortT) -> KernReturnT;
        fn mach_timebase_info(info: *mut MachTimebaseInfoData) -> KernReturnT;
        fn thread_policy_set(
            thread: MachPortT,
            flavor: ThreadPolicyFlavorT,
            policy_info: *mut IntegerT,
            count: MachMsgTypeNumberT,
        ) -> KernReturnT;
    }

    let mut timebase = MaybeUninit::<MachTimebaseInfoData>::uninit();
    let timebase_result = unsafe { mach_timebase_info(timebase.as_mut_ptr()) };
    if timebase_result != KERN_SUCCESS {
        return Err(format!("mach_timebase_info failed: {timebase_result}"));
    }
    let timebase = unsafe { timebase.assume_init() };

    let mut policy = ThreadTimeConstraintPolicyData {
        period: nanos_to_mach_absolute_time(audio_period_ns, timebase.numer, timebase.denom)?,
        computation: nanos_to_mach_absolute_time(
            audio_computation_ns,
            timebase.numer,
            timebase.denom,
        )?,
        constraint: nanos_to_mach_absolute_time(audio_period_ns, timebase.numer, timebase.denom)?,
        preemptible: 1,
    };

    let thread = unsafe { mach_thread_self() };
    if thread == MACH_PORT_NULL {
        return Err("mach_thread_self returned MACH_PORT_NULL".to_string());
    }

    let result = unsafe {
        thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &mut policy as *mut ThreadTimeConstraintPolicyData as *mut IntegerT,
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    let _ = unsafe { mach_port_deallocate(mach_task_self_, thread) };

    if result == KERN_SUCCESS {
        Ok(())
    } else {
        Err(format!("thread_policy_set failed: {result}"))
    }
}

#[cfg(target_os = "macos")]
fn audio_period_nanos(sample_rate: u32, frame_size: usize) -> Option<u64> {
    if sample_rate == 0 || frame_size == 0 {
        return None;
    }
    let nanos = (frame_size as u128)
        .saturating_mul(1_000_000_000)
        .div_ceil(sample_rate as u128);
    u64::try_from(nanos).ok()
}

#[cfg(target_os = "macos")]
fn nanos_to_mach_absolute_time(nanos: u64, numer: u32, denom: u32) -> Result<u32, String> {
    if numer == 0 || denom == 0 {
        return Err("invalid mach timebase ratio".to_string());
    }

    let absolute = (nanos as u128)
        .saturating_mul(denom as u128)
        .checked_div(numer as u128)
        .ok_or_else(|| "invalid mach timebase ratio".to_string())?;

    u32::try_from(absolute).map_err(|_| format!("mach absolute time out of range: {absolute}"))
}

/// CoreAudio device name of the SotF HAL virtual device. Used to join the
/// workgroup of the coreaudiod IO thread that drives systemwide capture.
pub const HAL_VIRTUAL_DEVICE_NAME: &str = "SotF Virtual Audio";

/// RAII membership in a CoreAudio IO-thread workgroup (macOS).
///
/// Joining the device workgroup lets the kernel schedule this thread
/// cooperatively with the device IO thread in coreaudiod: deadline-aware,
/// performance-core placement under load, and no root requirement. The
/// membership is bound to the joining thread; dropping it leaves the
/// workgroup, so hold it in a binding that lives for the thread's work
/// loop. On other platforms this is an inert placeholder.
#[cfg(target_os = "macos")]
pub struct WorkgroupMembership {
    workgroup: *mut std::ffi::c_void,
    token: workgroup_ffi::OsWorkgroupJoinToken,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(not(target_os = "macos"))]
pub struct WorkgroupMembership {
    _private: (),
}

impl std::fmt::Debug for WorkgroupMembership {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WorkgroupMembership(..)")
    }
}

#[cfg(target_os = "macos")]
impl Drop for WorkgroupMembership {
    fn drop(&mut self) {
        unsafe {
            workgroup_ffi::os_workgroup_leave(self.workgroup, &mut self.token);
            workgroup_ffi::os_release(self.workgroup);
        }
    }
}

/// Join the calling thread to the IO-thread workgroup of the named audio
/// device (`input` selects capture devices), or of the system default
/// output/input device when `device_name` is None. Returns None when
/// disabled (`SOTF_AUDIO_WORKGROUP=0`), unsupported, or unavailable; the
/// caller simply keeps its QoS elevation in that case.
#[cfg(target_os = "macos")]
pub fn join_device_workgroup(
    device_name: Option<&str>,
    input: bool,
    context: &str,
) -> Option<WorkgroupMembership> {
    if std::env::var("SOTF_AUDIO_WORKGROUP").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false"
        )
    }) {
        log::debug!("[Workgroup] disabled via SOTF_AUDIO_WORKGROUP for {context}");
        return None;
    }
    let device_id = workgroup_ffi::audio_device_id(device_name, input)?;
    let workgroup = workgroup_ffi::fetch_io_thread_workgroup(device_id)?;
    let mut token = workgroup_ffi::OsWorkgroupJoinToken { opaque: [0; 36] };
    let status = unsafe { workgroup_ffi::os_workgroup_join(workgroup, &mut token) };
    if status != 0 {
        log::warn!("[Workgroup] join failed for {context} (errno={status}), keeping QoS only");
        unsafe { workgroup_ffi::os_release(workgroup) };
        return None;
    }
    log::info!(
        "[Workgroup] {context} joined {} device workgroup (device_id={device_id})",
        if input { "input" } else { "output" }
    );
    Some(WorkgroupMembership {
        workgroup,
        token,
        _thread_bound: std::marker::PhantomData,
    })
}

#[cfg(not(target_os = "macos"))]
pub fn join_device_workgroup(
    _device_name: Option<&str>,
    _input: bool,
    _context: &str,
) -> Option<WorkgroupMembership> {
    None
}

#[cfg(target_os = "macos")]
mod workgroup_ffi {
    use std::ffi::c_void;

    #[repr(C)]
    pub(super) struct OsWorkgroupJoinToken {
        pub(super) opaque: [u8; 36],
    }

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
    const PROPERTY_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
    const PROPERTY_IO_THREAD_OS_WORKGROUP: u32 = u32::from_be_bytes(*b"oswg");
    const PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = u32::from_be_bytes(*b"dOut");
    const PROPERTY_DEFAULT_INPUT_DEVICE: u32 = u32::from_be_bytes(*b"dIn ");

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            object_id: u32,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    unsafe extern "C" {
        pub(super) fn os_workgroup_join(
            workgroup: *mut c_void,
            token_out: *mut OsWorkgroupJoinToken,
        ) -> i32;
        pub(super) fn os_workgroup_leave(
            workgroup: *mut c_void,
            token: *mut OsWorkgroupJoinToken,
        ) -> i32;
        pub(super) fn os_release(object: *mut c_void);
    }

    pub(super) fn audio_device_id(device_name: Option<&str>, input: bool) -> Option<u32> {
        use coreaudio_rs::audio_unit::macos_helpers::get_device_id_from_name;
        match device_name {
            // The workgroup property is device-global; the scope only gates
            // the by-name search. Virtual capture devices such as the SotF
            // HAL present as output devices to applications, so fall back
            // to the opposite scope instead of giving up.
            Some(name) => get_device_id_from_name(name, input)
                .or_else(|| get_device_id_from_name(name, !input)),
            None => {
                let selector = if input {
                    PROPERTY_DEFAULT_INPUT_DEVICE
                } else {
                    PROPERTY_DEFAULT_OUTPUT_DEVICE
                };
                let address = AudioObjectPropertyAddress {
                    selector,
                    scope: PROPERTY_SCOPE_GLOBAL,
                    element: 0,
                };
                let mut device_id: u32 = 0;
                let mut data_size = size_of::<u32>() as u32;
                let status = unsafe {
                    AudioObjectGetPropertyData(
                        AUDIO_OBJECT_SYSTEM_OBJECT,
                        &address,
                        0,
                        std::ptr::null(),
                        &mut data_size,
                        &mut device_id as *mut u32 as *mut c_void,
                    )
                };
                (status == 0 && device_id != 0).then_some(device_id)
            }
        }
    }

    /// Fetch the retained `os_workgroup_t` for a device's IO thread. The
    /// caller owns the reference and must `os_release` it after leaving.
    pub(super) fn fetch_io_thread_workgroup(device_id: u32) -> Option<*mut c_void> {
        let address = AudioObjectPropertyAddress {
            selector: PROPERTY_IO_THREAD_OS_WORKGROUP,
            scope: PROPERTY_SCOPE_GLOBAL,
            element: 0,
        };
        let mut workgroup: *mut c_void = std::ptr::null_mut();
        let mut data_size = size_of::<*mut c_void>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device_id,
                &address,
                0,
                std::ptr::null(),
                &mut data_size,
                &mut workgroup as *mut *mut c_void as *mut c_void,
            )
        };
        (status == 0 && !workgroup.is_null()).then_some(workgroup)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod workgroup_tests {
    #[test]
    fn join_unknown_device_is_a_graceful_noop() {
        let membership =
            super::join_device_workgroup(Some("sotf-definitely-not-a-real-device"), false, "test");
        assert!(membership.is_none());
    }

    /// Manual hardware probe: joining the default output device's
    /// workgroup must succeed on a Mac with audio hardware. Run with
    /// `cargo test -p sotf-engine --lib workgroup -- --ignored`.
    #[test]
    #[ignore = "requires a real CoreAudio output device"]
    fn join_default_output_device_workgroup() {
        let membership = super::join_device_workgroup(None, false, "test");
        assert!(
            membership.is_some(),
            "expected to join the default output workgroup"
        );
    }

    /// Manual hardware probe for systemwide installs: the SotF HAL
    /// virtual device must expose a joinable IO-thread workgroup. Run it
    /// from a normal (unsandboxed) terminal: sandboxed processes do not
    /// enumerate HAL plug-in devices at all.
    #[test]
    #[ignore = "requires the SotF HAL driver to be installed"]
    fn join_hal_virtual_device_workgroup() {
        let membership =
            super::join_device_workgroup(Some(super::HAL_VIRTUAL_DEVICE_NAME), true, "test");
        assert!(
            membership.is_some(),
            "expected to join the HAL virtual device workgroup"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::{audio_period_nanos, nanos_to_mach_absolute_time};

    #[test]
    fn nanos_to_mach_absolute_time_converts_and_rejects_invalid_timebase() {
        assert_eq!(nanos_to_mach_absolute_time(1_000, 1, 1).unwrap(), 1_000);
        assert_eq!(nanos_to_mach_absolute_time(1_000, 2, 1).unwrap(), 500);
        assert_eq!(nanos_to_mach_absolute_time(1_000, 1, 2).unwrap(), 2_000);
        assert!(nanos_to_mach_absolute_time(1_000, 0, 1).is_err());
        assert!(nanos_to_mach_absolute_time(1_000, 1, 0).is_err());
    }

    #[test]
    fn audio_period_uses_configured_frame_size_and_sample_rate() {
        assert_eq!(audio_period_nanos(48_000, 480), Some(10_000_000));
        assert_eq!(audio_period_nanos(44_100, 128), Some(2_902_495));
        assert_eq!(audio_period_nanos(0, 128), None);
    }
}

// ============================================================================
// Linux: SCHED_FIFO for playback, SCHED_RR for processing
// ============================================================================

#[cfg(target_os = "linux")]
fn set_priority_linux(level: RtPriority) -> Result<bool, String> {
    use libc::{SCHED_FIFO, SCHED_RR, sched_param, sched_setscheduler};

    let (policy, priority) = match level {
        RtPriority::Playback => (SCHED_FIFO, 70),
        RtPriority::Processing => (SCHED_RR, 50),
    };

    let param = sched_param {
        sched_priority: priority,
    };

    let ret = unsafe { sched_setscheduler(0, policy, &param) };
    if ret == 0 {
        log::info!(
            "[RT Priority] Linux: set {:?} to policy={}, priority={}",
            level,
            if policy == SCHED_FIFO {
                "SCHED_FIFO"
            } else {
                "SCHED_RR"
            },
            priority
        );
        Ok(true)
    } else {
        let errno = std::io::Error::last_os_error();
        log::warn!(
            "[RT Priority] Linux: failed to set scheduler ({:?}), continuing at default priority. \
             Hint: run with CAP_SYS_NICE or as root for RT priority.",
            errno
        );
        Ok(false)
    }
}
