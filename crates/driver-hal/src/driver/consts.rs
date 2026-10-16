use std::time::Duration;

/// Budget for the HAL to acknowledge a daemon-initiated config request.
///
/// Swift services requests on a 100 ms maintenance tick and then asks
/// CoreAudio to apply the change asynchronously, so on a busy cold
/// `coreaudiod` 750 ms was routinely exceeded and the first cold-start apply
/// failed spuriously. Three seconds covers the tick plus a slow apply while
/// still bounding IPC mutation latency well inside the client's deadline.
pub(super) const DAEMON_CONFIG_ACK_TIMEOUT: Duration = Duration::from_millis(3000);

pub(super) const DAEMON_CONFIG_ACK_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(super) const DAEMON_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
