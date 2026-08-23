-- raw table, insert into this as often as you want
CREATE TABLE player_count_logs (
    timestamp DateTime DEFAULT now() CODEC(Delta, ZSTD(3)),
    players UInt32 CODEC(Delta, ZSTD(3))
)
ENGINE = MergeTree
ORDER BY timestamp
PARTITION BY toYYYYMM(timestamp)
TTL timestamp + INTERVAL 1 DAY;

-- 1m table, rounded down to a minute and stored for a few days
CREATE TABLE player_count_logs_1m (
    timestamp DateTime CODEC(Delta, ZSTD(3)),
    players SimpleAggregateFunction(max, UInt32) CODEC(Delta, ZSTD(3))
)
ENGINE = AggregatingMergeTree
ORDER BY timestamp
PARTITION BY toYYYYMM(timestamp)
TTL timestamp + INTERVAL 3 DAY;

-- mv to populate the 1m table
CREATE MATERIALIZED VIEW player_count_logs_1m_mv
TO player_count_logs_1m
AS
SELECT
    toStartOfMinute(timestamp) AS timestamp,
    max(players) AS players
FROM player_count_logs
GROUP BY timestamp;

-- 1h table, rounded down to an hour and stored forever
CREATE TABLE player_count_logs_1h (
    timestamp DateTime CODEC(Delta, ZSTD(3)),
    players SimpleAggregateFunction(max, UInt32) CODEC(Delta, ZSTD(3))
)
ENGINE = AggregatingMergeTree
ORDER BY timestamp
PARTITION BY toYYYYMM(timestamp);

CREATE MATERIALIZED VIEW player_count_logs_1h_mv
TO player_count_logs_1h
AS
SELECT
    toStartOfHour(timestamp) AS timestamp,
    max(players) AS players
FROM player_count_logs
GROUP BY timestamp;


-- view

CREATE VIEW player_count_logs_view AS
SELECT timestamp, players
FROM player_count_logs
WHERE timestamp >= now() - INTERVAL 1 DAY
UNION ALL
SELECT timestamp, max(players) AS players
FROM player_count_logs_1m
WHERE timestamp >= now() - INTERVAL 3 DAY
  AND timestamp < now() - INTERVAL 1 DAY
GROUP BY timestamp
UNION ALL
SELECT timestamp, max(players) AS players
FROM player_count_logs_1h
WHERE timestamp < now() - INTERVAL 3 DAY
GROUP BY timestamp;
