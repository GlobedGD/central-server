CREATE TABLE login_events (
    timestamp DateTime64(3) DEFAULT now64(3) CODEC(Delta(8), ZSTD(3)),
    user_id Int32 CODEC(ZSTD(3)),
    ip_address IPv6 CODEC(ZSTD(3)),
    connection_type LowCardinality(String),
    globed_version LowCardinality(String),
    geode_version LowCardinality(String),
    platform LowCardinality(String)
)
ENGINE = MergeTree
ORDER BY (timestamp, platform, globed_version, connection_type)
PARTITION BY toYYYYMM(timestamp)
TTL timestamp + INTERVAL 7 DAY;
