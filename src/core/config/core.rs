use std::path::PathBuf;

use rand::distr::SampleString;
use serde::{Deserialize, Serialize};
use server_shared::{config::env_replace, logging::LoggerConfig};
use validator::Validate;

// Performance

fn default_memory_usage() -> u32 {
    3
}

fn default_compression_level() -> u32 {
    3
}

// Logging

fn default_logging() -> LoggerConfig {
    LoggerConfig {
        filename: "central-server.log".to_owned(),
        ..Default::default()
    }
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

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct QuicConfig {
    /// Whether to enable incoming QUIC connections. This requires all the other parameters in this section to be set.
    #[serde(default = "default_enable_quic")]
    pub enable: bool,
    #[serde(default = "default_quic_address")]
    pub address: String,
    /// The path to the TLS certificate for QUIC connections.
    #[serde(default = "default_quic_tls_cert")]
    pub tls_cert: String,
    /// The path to the TLS key for QUIC connections.
    #[serde(default = "default_quic_tls_key")]
    pub tls_key: String,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            enable: default_enable_quic(),
            address: default_quic_address(),
            tls_cert: default_quic_tls_cert(),
            tls_key: default_quic_tls_key(),
        }
    }
}

// TCP

fn default_enable_tcp() -> bool {
    true
}

fn default_tcp_address() -> String {
    "[::]:4340".into()
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct TcpConfig {
    /// Whether to enable incoming TCP connections. This requires the "address" option to be set.
    #[serde(default = "default_enable_tcp")]
    pub enable: bool,
    #[serde(default = "default_tcp_address")]
    pub address: String,
}

impl Default for TcpConfig {
    fn default() -> Self {
        Self {
            enable: default_enable_tcp(),
            address: default_tcp_address(),
        }
    }
}

// WS

fn default_enable_ws() -> bool {
    false
}

fn default_ws_address() -> String {
    "[::]:4341".into()
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct WsConfig {
    /// Whether to enable incoming WebSocket connections. This requires the "address" option to be set.
    #[serde(default = "default_enable_ws")]
    pub enable: bool,
    #[serde(default = "default_ws_address")]
    pub address: String,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            enable: default_enable_ws(),
            address: default_ws_address(),
        }
    }
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

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct UdpConfig {
    /// Whether to enable incoming UDP connections. This requires the "address" option to be set.
    #[serde(default = "default_enable_udp")]
    pub enable: bool,
    /// Whether to use UDP solely for "Discovery" (ping) purposes. New connections will not be established if this is enabled.
    /// Note: `enable_udp` must be enabled for this to have any effect, otherwise pings will be ignored.
    #[serde(default = "default_udp_ping_only")]
    pub ping_only: bool,
    /// The address to listen for UDP connections or pings on.
    #[serde(default = "default_udp_address")]
    pub address: String,
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            enable: default_enable_udp(),
            ping_only: default_udp_ping_only(),
            address: default_udp_address(),
        }
    }
}

// qunet stuff

fn default_qdb_path() -> Option<PathBuf> {
    None
}

fn default_enable_stat_tracking() -> bool {
    false
}

// Game server stuff

fn default_gs_password() -> String {
    rand::distr::Alphanumeric.sample_string(&mut rand::rng(), 32)
}

fn default_gs_tcp_address() -> Option<String> {
    Some("[::]:4342".into())
}

fn default_gs_quic_address() -> Option<String> {
    None
}

#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct CoreConfig {
    /// The memory usage value (1 to 11), determines how much memory the server will preallocate for operations.
    #[serde(default = "default_memory_usage")]
    #[validate(range(min = 1, max = 11))]
    pub memory_usage: u32,
    /// How aggressive compression of data should be.
    /// 0 means no compression, 6 means prefer zstd almost always.
    #[serde(default = "default_compression_level")]
    #[validate(range(min = 0, max = 6))]
    pub compression_level: u32,

    /// Logging options
    #[serde(default = "default_logging")]
    pub logging: LoggerConfig,

    #[serde(default)]
    pub quic: QuicConfig,
    #[serde(default)]
    pub tcp: TcpConfig,
    #[serde(default)]
    pub ws: WsConfig,
    #[serde(default)]
    pub udp: UdpConfig,

    /// The path to the QDB file.
    #[serde(default = "default_qdb_path")]
    pub qdb_path: Option<PathBuf>,
    /// Whether to enable connection stat tracking
    #[serde(default = "default_enable_stat_tracking")]
    pub enable_stat_tracking: bool,

    /// The password for the game server
    #[serde(default = "default_gs_password")]
    pub gs_password: String,
    /// Address for accepting TCP connections from game servers. If blank, TCP is not used.
    #[serde(default = "default_gs_tcp_address")]
    pub gs_tcp_address: Option<String>,
    /// Address for accepting QUIC connections from game servers. If blank, QUIC is not used.
    #[serde(default = "default_gs_quic_address")]
    pub gs_quic_address: Option<String>,

    /// Override for the base URL used for communication with the GD servers.
    /// Change this if you are hosting a server for a GDPS.
    /// This should include the /database path part, e.g. "https://www.boomlings.com/database"
    #[serde(default)]
    pub gd_api_base_url: Option<String>,
    /// Auth token for GD api requests, optional.
    #[serde(default)]
    pub gd_api_auth_token: Option<String>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            memory_usage: default_memory_usage(),
            compression_level: default_compression_level(),
            logging: default_logging(),
            quic: QuicConfig::default(),
            tcp: TcpConfig::default(),
            ws: WsConfig::default(),
            udp: UdpConfig::default(),
            qdb_path: default_qdb_path(),
            enable_stat_tracking: default_enable_stat_tracking(),
            gs_password: default_gs_password(),
            gs_tcp_address: default_gs_tcp_address(),
            gs_quic_address: default_gs_quic_address(),
            gd_api_base_url: None,
            gd_api_auth_token: None,
        }
    }
}

impl CoreConfig {
    pub fn replace_with_env(&mut self) {
        env_replace("GLOBED_CORE_MEMORY_USAGE", &mut self.memory_usage);
        env_replace("GLOBED_CORE_COMPRESSION_LEVEL", &mut self.compression_level);

        env_replace("GLOBED_CORE_LOG_FILE_ENABLED", &mut self.logging.file_enabled);
        env_replace("GLOBED_CORE_LOG_DIRECTORY", &mut self.logging.directory);
        env_replace("GLOBED_CORE_CONSOLE_LOG_LEVEL", &mut self.logging.console_level);
        env_replace("GLOBED_CORE_FILE_LOG_LEVEL", &mut self.logging.file_level);
        env_replace("GLOBED_CORE_LOG_FILENAME", &mut self.logging.filename);
        env_replace("GLOBED_CORE_LOG_ROLLING", &mut self.logging.rolling);

        env_replace("GLOBED_CORE_ENABLE_QUIC", &mut self.quic.enable);
        env_replace("GLOBED_CORE_QUIC_ADDRESS", &mut self.quic.address);
        env_replace("GLOBED_CORE_QUIC_TLS_CERT", &mut self.quic.tls_cert);
        env_replace("GLOBED_CORE_QUIC_TLS_KEY", &mut self.quic.tls_key);

        env_replace("GLOBED_CORE_ENABLE_TCP", &mut self.tcp.enable);
        env_replace("GLOBED_CORE_TCP_ADDRESS", &mut self.tcp.address);

        env_replace("GLOBED_CORE_ENABLE_WS", &mut self.ws.enable);
        env_replace("GLOBED_CORE_WS_ADDRESS", &mut self.ws.address);

        env_replace("GLOBED_CORE_ENABLE_UDP", &mut self.udp.enable);
        env_replace("GLOBED_CORE_UDP_PING_ONLY", &mut self.udp.ping_only);
        env_replace("GLOBED_CORE_UDP_ADDRESS", &mut self.udp.address);

        env_replace("GLOBED_CORE_QDB_PATH", &mut self.qdb_path);
        env_replace("GLOBED_CORE_ENABLE_STAT_TRACKING", &mut self.enable_stat_tracking);

        env_replace("GLOBED_CORE_GS_PASSWORD", &mut self.gs_password);
        env_replace("GLOBED_CORE_GS_TCP_ADDRESS", &mut self.gs_tcp_address);
        env_replace("GLOBED_CORE_GS_QUIC_ADDRESS", &mut self.gs_quic_address);

        env_replace("GLOBED_CORE_GD_API_BASE_URL", &mut self.gd_api_base_url);
        env_replace("GLOBED_CORE_GD_API_AUTH_TOKEN", &mut self.gd_api_auth_token);
    }
}
