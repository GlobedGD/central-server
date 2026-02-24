use std::net::{IpAddr, Ipv6Addr};

use chrono::{DateTime, Utc};
use clickhouse::Row;
use serde::Serialize;

#[derive(Serialize, Row)]
pub struct LoginEvent {
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub timestamp: DateTime<Utc>,
    pub user_id: i32,
    pub ip_address: Ipv6Addr,
    pub globed_version: heapless::String<16>,
    pub geode_version: heapless::String<16>,
    pub platform: heapless::String<16>,
}

fn convert_str<const N: usize>(mut s: &str) -> heapless::String<N> {
    if s.len() > N {
        s = &s[..N];
    }

    heapless::String::try_from(s).unwrap()
}

impl LoginEvent {
    pub fn new(
        user_id: i32,
        ip_address: IpAddr,
        globed_version: &str,
        geode_version: &str,
        platform: &str,
    ) -> Self {
        let ip_address = match ip_address {
            IpAddr::V4(v4) => v4.to_ipv6_mapped(),
            IpAddr::V6(v6) => v6,
        };

        Self {
            timestamp: Utc::now(),
            user_id,
            ip_address,
            globed_version: convert_str(globed_version),
            geode_version: convert_str(geode_version),
            platform: convert_str(platform),
        }
    }
}
