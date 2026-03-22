ALTER TABLE login_events
ADD COLUMN platform_desc LowCardinality(String)
TTL timestamp + INTERVAL 30 DAY;
