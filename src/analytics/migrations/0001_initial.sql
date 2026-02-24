CREATE TABLE login_events (
    timestamp DateTime64(3) DEFAULT now(),
    user_id Int32,
    ip_address IPv6,
    globed_version LowCardinality(String),
    geode_version LowCardinality(String),
    platform LowCardinality(String)
)
ENGINE = MergeTree
ORDER BY (timestamp, platform, globed_version);
