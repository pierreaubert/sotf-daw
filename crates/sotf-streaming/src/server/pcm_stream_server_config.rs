use super::consts::DEFAULT_CLIENT_QUEUE_CAPACITY_CHUNKS;
use super::consts::DEFAULT_QUEUE_CAPACITY_CHUNKS;

#[derive(Clone, Debug)]
pub struct PcmStreamServerConfig {
    pub bind_addr: String,
    pub port: u16,
    pub initial_sample_rate: u32,
    pub initial_channels: u16,
    pub queue_capacity_chunks: usize,
    pub client_queue_capacity_chunks: usize,
    /// Maximum simultaneous connections, including incomplete requests.
    pub max_connections: usize,
}

impl Default for PcmStreamServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_string(),
            port: 17_890,
            initial_sample_rate: 48_000,
            initial_channels: 2,
            queue_capacity_chunks: DEFAULT_QUEUE_CAPACITY_CHUNKS,
            client_queue_capacity_chunks: DEFAULT_CLIENT_QUEUE_CAPACITY_CHUNKS,
            max_connections: 16,
        }
    }
}
