use std::{
    error::Error,
    sync::{Arc, atomic::AtomicBool},
};

use google_sheets4::{
    Sheets,
    api::{
        AddSheetRequest, BatchUpdateSpreadsheetRequest, ClearValuesRequest, GridProperties,
        Request, SheetProperties, ValueRange,
    },
    hyper_rustls::{self, HttpsConnector},
    hyper_util::{
        client::legacy::{Client, connect::HttpConnector},
        rt::TokioExecutor,
    },
    yup_oauth2,
};
use serde_json::Value;
use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tracing::{debug, error, info, warn};

use crate::features::database::{FeaturedLevelModel, QueuedLevelModel, SentLevelModel};

use super::database::{FeaturedLevel, QueuedLevel, SentLevel};

#[derive(Debug)]
enum WorkerRequest {
    Featured(Vec<FeaturedLevelModel>),
    Queued(Vec<QueuedLevelModel>),
    Sent(Vec<SentLevelModel>),
}

struct WorkerState {
    hub: Sheets<HttpsConnector<HttpConnector>>,
    id: String,
    tx: Sender<WorkerRequest>,
}

pub struct SheetsClient {
    state: Arc<WorkerState>,
}

impl WorkerState {
    pub async fn run_worker_loop(
        &self,
        mut rx: Receiver<WorkerRequest>,
    ) -> Result<(), Box<dyn Error>> {
        self.create_sheets().await?;

        while let Some(req) = rx.recv().await {
            debug!("Received sheets worker request: {req:?}");

            let (sheet, rows) = match req {
                WorkerRequest::Featured(levels) => ("Featured", Self::levels_to_rows(levels)),
                WorkerRequest::Queued(levels) => ("Queued", Self::levels_to_rows(levels)),
                WorkerRequest::Sent(_) => continue,
            };

            let columns = rows.first().unwrap().len();

            let range = format!("{sheet}!A1:{}{}", char::from(b'A' + columns as u8), rows.len());

            let value_range = ValueRange {
                range: Some(range.clone()),
                values: Some(rows),
                ..Default::default()
            };

            // clear the entire sheet first
            self.hub
                .spreadsheets()
                .values_clear(ClearValuesRequest::default(), &self.id, sheet)
                .doit()
                .await?;

            // now write the new values
            self.hub
                .spreadsheets()
                .values_update(value_range, &self.id, &range)
                .value_input_option("USER_ENTERED")
                .doit()
                .await?;
        }
        std::iter::

        Ok(())
    }

    fn levels_to_rows<T: LevelToRow>(levels: Vec<T>) -> Vec<Vec<Value>> {
        let mut out = Vec::with_capacity(levels.len() + 1);
        out.push(T::header_row());
        out.extend(levels.into_iter().map(|lvl| lvl.into_row()));
        out
    }

    pub async fn create_sheets(&self) -> Result<(), Box<dyn Error>> {
        info!("Ensuring all necessary sheets exist..");

        let (_, spsh) = self.hub.spreadsheets().get(&self.id).doit().await?;
        let sheets = spsh.sheets.ok_or("no sheets found")?;

        let add_one = async |title: &str, columns: i32| -> Result<(), Box<dyn Error>> {
            for sheet in &sheets {
                if sheet
                    .properties
                    .as_ref()
                    .is_some_and(|p| p.title.as_ref().is_some_and(|t| t == title))
                {
                    return Ok(());
                }
            }

            // add the sheet!
            let req = AddSheetRequest {
                properties: Some(SheetProperties {
                    title: Some(title.to_owned()),
                    grid_properties: Some(GridProperties {
                        column_count: Some(columns),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            };
            info!("Creating sheet '{title}'..");

            self.hub
                .spreadsheets()
                .batch_update(
                    BatchUpdateSpreadsheetRequest {
                        requests: Some(vec![Request {
                            add_sheet: Some(req),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                    &self.id,
                )
                .doit()
                .await?;

            Ok(())
        };

        // TODO: columns
        add_one("Featured", 10).await?;
        add_one("Queued", 10).await?;
        add_one("Sent", 10).await?;

        Ok(())
    }
}

impl SheetsClient {
    pub async fn new(creds: &str, spreadsheet_id: String) -> Self {
        let auth = yup_oauth2::ServiceAccountAuthenticator::builder(
            serde_json::from_str::<yup_oauth2::ServiceAccountKey>(creds)
                .expect("failed to parse google credentials"),
        )
        .build()
        .await
        .unwrap();

        let client = Client::builder(TokioExecutor::new()).build(
            hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_or_http()
                .enable_all_versions()
                .build(),
        );

        let hub = Sheets::new(client, auth);
        let (tx, rx) = tokio::sync::mpsc::channel(8);

        let state = Arc::new(WorkerState { hub, id: spreadsheet_id, tx });

        let wstate = state.clone();

        tokio::spawn(async move {
            if let Err(e) = wstate.run_worker_loop(rx).await {
                error!("Sheets worker failed: {e}");
            }
        });

        Self { state }
    }

    pub async fn update_featured_sheet(
        &self,
        levels: Vec<FeaturedLevelModel>,
    ) -> Result<(), Box<dyn Error>> {
        self.state.tx.try_send(WorkerRequest::Featured(levels))?;
        Ok(())
    }

    pub async fn update_queued_sheet(
        &self,
        levels: Vec<QueuedLevelModel>,
    ) -> Result<(), Box<dyn Error>> {
        self.state.tx.try_send(WorkerRequest::Queued(levels))?;
        Ok(())
    }
}

trait LevelToRow {
    fn into_row(self) -> Vec<Value>;
    fn header_row() -> Vec<Value>;
}

impl LevelToRow for FeaturedLevelModel {
    fn into_row(self) -> Vec<Value> {
        vec![
            Value::String(self.name),
            Value::Number(self.id.into()),
            Value::String(self.author_name),
            Value::Number(self.author.into()),
            Value::String(format_timestamp(self.featured_at)),
            Value::String(format_rate_tier(self.rate_tier)),
            Value::String(format_dur_seconds(self.feature_duration.unwrap_or(0))),
        ]
    }

    fn header_row() -> Vec<Value> {
        vec![
            Value::String("Level Name".to_owned()),
            Value::String("Level ID".to_owned()),
            Value::String("Author Name".to_owned()),
            Value::String("Author ID".to_owned()),
            Value::String("Featured At".to_owned()),
            Value::String("Rate Tier".to_owned()),
            Value::String("Feature Duration".to_owned()),
        ]
    }
}

impl LevelToRow for QueuedLevelModel {
    fn into_row(self) -> Vec<Value> {
        vec![
            Value::String(self.name),
            Value::Number(self.id.into()),
            Value::String(self.author_name),
            Value::Number(self.author.into()),
            Value::String(format_rate_tier(self.rate_tier)),
            Value::String(format_dur_seconds(self.feature_duration.unwrap_or(0))),
        ]
    }

    fn header_row() -> Vec<Value> {
        vec![
            Value::String("Level Name".to_owned()),
            Value::String("Level ID".to_owned()),
            Value::String("Author Name".to_owned()),
            Value::String("Author ID".to_owned()),
            Value::String("Featured At".to_owned()),
            Value::String("Rate Tier".to_owned()),
            Value::String("Feature Duration".to_owned()),
        ]
    }
}

fn format_timestamp(ts: i64) -> String {
    time_format::strftime_utc("%Y-%m-%d %H:%M:%S", ts).unwrap()
}

fn format_dur_seconds(secs: i32) -> String {
    use std::fmt::Write;

    if secs == 0 {
        return "Default".to_owned();
    }

    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    let mut out = String::new();
    if hours > 0 {
        write!(out, "{}h", hours).unwrap();
    }

    if mins > 0 {
        write!(out, "{}m", mins).unwrap();
    }

    if secs > 0 {
        write!(out, "{}s", secs).unwrap();
    }

    out
}

fn format_rate_tier(tier: i32) -> String {
    match tier {
        0 => "Normal",
        1 => "Epic",
        2 => "Outstanding",
        _ => "Unknown",
    }
    .to_owned()
}
