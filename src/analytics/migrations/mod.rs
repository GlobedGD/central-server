use anyhow::{Result, anyhow};
use clickhouse::Client;
use include_dir::{Dir, include_dir};
use std::collections::HashSet;
use tracing::{debug, info};

// To add new migrations, simply create a new file in this directory, named similarly to the rest of the files
static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/analytics/migrations");

static MIGRATIONS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS globed_analytics_migrations (
    version UInt64,
    name String,
    applied_at DateTime DEFAULT now()
)
ENGINE = MergeTree()
ORDER BY version;
"#;

struct Migration {
    version: u64,
    full_name: String,
    sql: String,
}

fn collect() -> Result<Vec<Migration>> {
    let mut migrations = Vec::new();

    for entry in MIGRATIONS_DIR.files() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "sql") {
            continue;
        }

        let full_name = path.file_name().unwrap().to_string_lossy();
        let (version_str, _) = full_name
            .split_once('_')
            .ok_or_else(|| anyhow!("Invalid migration file name: '{full_name}'"))?;

        let version = version_str
            .parse::<u64>()
            .map_err(|_| anyhow!("Invalid migration version in file name: '{full_name}'"))?;

        let sql = entry
            .contents_utf8()
            .ok_or_else(|| anyhow!("Failed to read migration file as UTF-8: '{full_name}'"))?;

        migrations.push(Migration {
            version,
            full_name: full_name.to_string(),
            sql: sql.to_string(),
        });
    }

    // sort by version
    migrations.sort_by_key(|m| m.version);

    Ok(migrations)
}

pub async fn run(client: &Client) -> Result<()> {
    let migrations = collect()?;
    debug!("Collected {} migrations", migrations.len());

    // ensure that the migration table exists
    client.query(MIGRATIONS_TABLE_SQL).execute().await?;

    // fetch already applied migrations
    let applied: Vec<u64> =
        client.query("SELECT version FROM globed_analytics_migrations").fetch_all().await?;
    let applied: HashSet<u64> = applied.into_iter().collect();

    for mig in migrations {
        if applied.contains(&mig.version) {
            continue;
        }

        info!("Applying migration '{}'", mig.full_name);

        for stmt in mig.sql.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }

            client
                .query(stmt)
                .execute()
                .await
                .map_err(|e| anyhow!("migration '{}' failed: {e}", mig.full_name))?;
        }

        client
            .query("INSERT INTO globed_analytics_migrations (version, name) VALUES (?, ?)")
            .bind(mig.version)
            .bind(mig.full_name)
            .execute()
            .await?;
    }

    Ok(())
}
