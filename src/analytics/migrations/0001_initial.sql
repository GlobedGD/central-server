CREATE TABLE login_events (
    timestamp DateTime64(3) DEFAULT now(),
    -- since user ids are potentially identifiable info, we store them for a much shorter duration than the rest of the table
    -- we still retain them temporarily to prevent abuse and stuff
    user_id Int32 TTL timestamp + INTERVAL 7 DAY,
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
