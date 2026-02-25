CREATE TABLE login_events (
    timestamp DateTime64(3) DEFAULT now(),
    user_id Int32,
    ip_address IPv6,
    connection_type LowCardinality(String),
    globed_version LowCardinality(String),
    geode_version LowCardinality(String),
    platform LowCardinality(String)
)
ENGINE = MergeTree
ORDER BY (timestamp, platform, globed_version, connection_type)
PARTITION BY toYYYYMM(timestamp)
TTL timestamp + INTERVAL 90 DAY;
