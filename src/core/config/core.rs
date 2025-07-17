use std::path::PathBuf;

use rand::distr::SampleString;
use serde::{Deserialize, Serialize};
use server_shared::config::env_replace;

// Memory

fn default_memory_usage() -> u32 {
    3
}

// Logging

fn default_log_file_enabled() -> bool {
    true
}

fn default_log_directory() -> PathBuf {
    "logs".into()
}

fn default_log_level() -> String {
    "info".into()
}

fn default_log_filename() -> String {
    "central-server.log".into()
}

fn default_log_rolling() -> bool {
    false
}

// QUIC

fn default_enable_quic() -> bool {
    false
}

fn default_quic_address() -> String {
    "[::]:4341".into()
}

fn default_quic_tls_cert() -> String {
    String::new()
}

fn default_quic_tls_key() -> String {
    String::new()
}

// TCP

fn default_enable_tcp() -> bool {
    true
}

fn default_tcp_address() -> String {
    "[::]:4340".into()
}

// UDP

fn default_enable_udp() -> bool {
    true
}

fn default_udp_ping_only() -> bool {
    true
}

fn default_udp_address() -> String {
    "[::]:4340".into()
}

// QDB stuff

fn default_qdb_path() -> Option<PathBuf> {
    None
}

// Game server stuff

fn default_gs_password() -> String {
    rand::distr::Alphanumeric.sample_string(&mut rand::rng(), 32)
}

fn default_gs_tcp_address() -> Option<String> {
    Some("[::]:4342".into())
}

fn default_gs_quic_address() -> Option<String> {
    Some("[::]:4343".into())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CoreConfig {
    /// The memory usage value (1 to 11), determines how much memory the server will preallocate for operations.
    #[serde(default = "default_memory_usage")]
    pub memory_usage: u32,

    /// Whether to enable logging to a file. If disabled, logs will only be printed to stdout.
    #[serde(default = "default_log_file_enabled")]
    pub log_file_enabled: bool,
    /// The directory where logs will be stored.
    #[serde(default = "default_log_directory")]
    pub log_directory: PathBuf,
    /// Minimum log level to print. Logs below this level will be ignored. Possible values: 'trace', 'debug', 'info', 'warn', 'error'.
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Prefix for the filename of the log file.
    #[serde(default = "default_log_filename")]
    pub log_filename: String,
    /// Whether to roll the log file daily. If enabled, rather than overwriting the same log file on restart,
    /// a new log file will be created with the current date appended to the filename.
    #[serde(default = "default_log_rolling")]
    pub log_rolling: bool,

    /// Whether to enable incoming QUIC connections. This requires the "quic_address", "quic_tls_cert" and "quic_tls_key" parameters to be set.
    #[serde(default = "default_enable_quic")]
    pub enable_quic: bool,
    /// The address to listen for QUIC connections on.
    #[serde(default = "default_quic_address")]
    pub quic_address: String,
    /// The path to the TLS certificate for QUIC connections.
    #[serde(default = "default_quic_tls_cert")]
    pub quic_tls_cert: String,
    /// The path to the TLS key for QUIC connections.
    #[serde(default = "default_quic_tls_key")]
    pub quic_tls_key: String,

    /// Whether to enable incoming TCP connections. This requires the "tcp_address" parameter to be set.
    #[serde(default = "default_enable_tcp")]
    pub enable_tcp: bool,
    /// The address to listen for TCP connections on.
    #[serde(default = "default_tcp_address")]
    pub tcp_address: String,

    /// Whether to enable incoming UDP connections. This requires the "udp_address" parameter to be set.
    #[serde(default = "default_enable_udp")]
    pub enable_udp: bool,
    /// Whether to use UDP solely for "Discovery" (ping) purposes. New connections will not be established if this is enabled.
    /// Note: `enable_udp` must be enabled for this to have any effect, otherwise pings will be ignored.
    #[serde(default = "default_udp_ping_only")]
    pub udp_ping_only: bool,
    /// The address to listen for UDP connections or pings on.
    #[serde(default = "default_udp_address")]
    pub udp_address: String,

    /// The path to the QDB file.
    #[serde(default = "default_qdb_path")]
    pub qdb_path: Option<PathBuf>,

    /// The password for the game server
    #[serde(default = "default_gs_password")]
    pub gs_password: String,
    /// Address for accepting TCP connections from game servers. If blank, TCP is not used.
    #[serde(default = "default_gs_tcp_address")]
    pub gs_tcp_address: Option<String>,
    /// Address for accepting QUIC connections from game servers. If blank, QUIC is not used.
    #[serde(default = "default_gs_quic_address")]
    pub gs_quic_address: Option<String>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            memory_usage: default_memory_usage(),
            log_file_enabled: default_log_file_enabled(),
            log_directory: default_log_directory(),
            log_level: default_log_level(),
            log_filename: default_log_filename(),
            log_rolling: default_log_rolling(),
            enable_quic: default_enable_quic(),
            quic_address: default_quic_address(),
            quic_tls_cert: default_quic_tls_cert(),
            quic_tls_key: default_quic_tls_key(),
            enable_tcp: default_enable_tcp(),
            tcp_address: default_tcp_address(),
            enable_udp: default_enable_udp(),
            udp_ping_only: default_udp_ping_only(),
            udp_address: default_udp_address(),
            qdb_path: default_qdb_path(),
            gs_password: default_gs_password(),
            gs_tcp_address: default_gs_tcp_address(),
            gs_quic_address: default_gs_quic_address(),
        }
    }
}

impl CoreConfig {
    pub fn replace_with_env(&mut self) {
        env_replace("GLOBED_CORE_MEMORY_USAGE", &mut self.memory_usage);

        env_replace("GLOBED_CORE_LOG_FILE_ENABLED", &mut self.log_file_enabled);
        env_replace("GLOBED_CORE_LOG_DIRECTORY", &mut self.log_directory);
        env_replace("GLOBED_CORE_LOG_LEVEL", &mut self.log_level);
        env_replace("GLOBED_CORE_LOG_FILENAME", &mut self.log_filename);
        env_replace("GLOBED_CORE_LOG_ROLLING", &mut self.log_rolling);

        env_replace("GLOBED_CORE_ENABLE_QUIC", &mut self.enable_quic);
        env_replace("GLOBED_CORE_QUIC_ADDRESS", &mut self.quic_address);
        env_replace("GLOBED_CORE_QUIC_TLS_CERT", &mut self.quic_tls_cert);
        env_replace("GLOBED_CORE_QUIC_TLS_KEY", &mut self.quic_tls_key);

        env_replace("GLOBED_CORE_ENABLE_TCP", &mut self.enable_tcp);
        env_replace("GLOBED_CORE_TCP_ADDRESS", &mut self.tcp_address);

        env_replace("GLOBED_CORE_ENABLE_UDP", &mut self.enable_udp);
        env_replace("GLOBED_CORE_UDP_PING_ONLY", &mut self.udp_ping_only);
        env_replace("GLOBED_CORE_UDP_ADDRESS", &mut self.udp_address);

        env_replace("GLOBED_CORE_QDB_PATH", &mut self.qdb_path);
    }
}
