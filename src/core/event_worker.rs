use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use server_shared::{
    encoding::EncodeMessageError,
    events::OwnedEvent,
    qunet::{
        buffers::HeapByteWriter,
        message::channel::{Receiver, Sender, new_channel},
        server::ServerHandle,
        transport::QunetMessageOpts,
    },
};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tracing::{debug, error, trace};

use crate::core::handler::{ClientStateHandle, ConnectionHandler};

struct PendingEvent {
    event: OwnedEvent,
    target: ClientStateHandle,
}

#[derive(Default)]
struct SharedState {
    server: OnceLock<ServerHandle<ConnectionHandler>>,
}

pub struct EventWorker {
    tx: Sender<PendingEvent>,
    task: JoinHandle<()>,
    state: Arc<SharedState>,
}

impl EventWorker {
    pub fn new() -> Self {
        let (tx, rx) = new_channel(512);

        let state = Arc::new(SharedState::default());
        let task = tokio::spawn(worker_func(rx, state.clone()));

        Self { tx, task, state }
    }

    pub fn abort(&self) {
        self.task.abort();
    }

    pub fn set_server(&self, server: ServerHandle<ConnectionHandler>) {
        let _ = self.state.server.set(server);
    }

    pub async fn enqueue(&self, event: OwnedEvent, target: ClientStateHandle) -> bool {
        if !target.knows_event(&event.id) {
            return false;
        }

        self.tx.send_async(PendingEvent { event, target }).await
    }

    pub fn try_enqueue(&self, event: OwnedEvent, target: ClientStateHandle) -> bool {
        if !target.knows_event(&event.id) {
            return false;
        }

        self.tx.send(PendingEvent { event, target })
    }
}

async fn worker_func(rx: Receiver<PendingEvent>, state: Arc<SharedState>) {
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    struct PendingEntry {
        events: Vec<OwnedEvent>,
        user: ClientStateHandle,
    }

    let mut pending = HashMap::<i32, PendingEntry>::new();
    let mut vec_cache = Vec::new();

    let mut flushes = 0usize;

    loop {
        tokio::select! {
            Some(p) = rx.recv() => {
                let account_id = p.target.account_id();
                if account_id == 0 {
                    continue;
                }

                // insert into the map at the account id,
                // grab a vec from the cache if possible, otherwise create a new one
                pending
                    .entry(account_id)
                    .or_insert_with(|| PendingEntry { events: vec_cache.pop().unwrap_or_else(Vec::new), user: p.target })
                    .events
                    .push(p.event);
            },

            _ = interval.tick() => {
                let Some(server) = state.server.get() else {
                    continue;
                };

                // flush
                for (account_id, PendingEntry { events, user }) in pending.drain() {
                    trace!("flushing {} events for user {}", events.len(), account_id);

                    let Some(encoder) = user.event_encoder() else {
                        continue;
                    };

                    let mut writer = HeapByteWriter::new();
                    if let Err(e) = encoder.encode_events(&events, &mut writer) {
                        error!("failed to encode {} events for {}: {e}", events.len(), account_id);
                        continue;
                    }

                    let reliable = events.iter().any(|e| e.options.reliable);
                    if let Err(e) = do_send_events(server, &user, writer.written(), reliable).await {
                        error!("failed to send events to {}: {e}", account_id);
                    }
                }

                flushes += 1;
                if flushes.is_multiple_of(100000) {
                    // clear caches to release memory
                    vec_cache.clear();
                    pending.shrink_to_fit();
                }
            }
        }
    }
}

async fn do_send_events(
    server: &ServerHandle<ConnectionHandler>,
    user: &ClientStateHandle,
    data: &[u8],
    reliable: bool,
) -> Result<(), EncodeMessageError> {
    let cap = data.len() + 32;

    let buf = server_shared::encode_message_heap!(server_shared::schema::main, server, cap, msg => {
        msg.set_events(data);
    })?;

    user.send_data_bufkind_opts(buf, QunetMessageOpts { reliable, ..Default::default() });
    debug!("sent {} bytes event buffer to {}", data.len(), user.account_id());

    Ok(())
}
