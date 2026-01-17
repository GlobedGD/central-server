use std::sync::{Arc, Weak};

use dashmap::DashMap;
use server_shared::{MAX_USERNAME_LENGTH, UsernameString};

use crate::core::handler::{ClientStateHandle, WeakClientStateHandle};

#[derive(Default)]
pub struct ClientStore {
    map: DashMap<i32, WeakClientStateHandle>,
    username_map: DashMap<UsernameString, WeakClientStateHandle>,
}

impl ClientStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.map.len()
    }

    pub fn find(&self, account_id: i32) -> Option<ClientStateHandle> {
        self.map.get(&account_id).and_then(|x| x.upgrade())
    }

    pub fn find_by_name(&self, username: &str) -> Option<ClientStateHandle> {
        let norm_name = normalize_username(username);
        self.username_map.get(&norm_name).and_then(|x| x.upgrade())
    }

    // this is now obsolete
    // pub fn find_by_name_slow(&self, username: &str) -> Option<ClientStateHandle> {
    //     self.map
    //         .iter()
    //         .filter_map(|r| match r.value().upgrade() {
    //             Some(c) if c.username().eq_ignore_ascii_case(username) => Some(c),
    //             _ => None,
    //         })
    //         .next()
    // }

    /// Inserts a new client into the map, returning any previous client with the same account ID
    pub fn insert(
        &self,
        account_id: i32,
        username: &str,
        client: &ClientStateHandle,
    ) -> Option<ClientStateHandle> {
        let old = self.map.insert(account_id, Arc::downgrade(client)).and_then(|x| x.upgrade());
        let username = normalize_username(username);
        self.username_map.insert(username, Arc::downgrade(client));

        old
    }

    pub fn remove_if_same(&self, account_id: i32, client: &ClientStateHandle) {
        let removed = self.map.remove_if(&account_id, |_, current_client| {
            Weak::ptr_eq(current_client, &Arc::downgrade(client))
        });

        if let Some((_, client)) = removed {
            if let Some(c) = client.upgrade() {
                let username = normalize_username(c.username());

                self.username_map.remove(&username);

                // if a client connected again with the same account ID, readd the username
                // this is very rare, but not impossible for it to happen
                if let Some(c) = self.find(account_id) {
                    self.username_map.insert(username, Arc::downgrade(&c));
                }
            }
        }
    }

    pub fn vacuum(&self) -> usize {
        let mut removed = 0;

        self.map.retain(|_, client| {
            if client.upgrade().is_none() {
                removed += 1;
                false
            } else {
                true
            }
        });

        removed
    }

    pub fn collect_all(&self) -> Vec<ClientStateHandle> {
        self.collect_all_pred(|_| true)
    }

    pub fn collect_all_authorized(&self) -> Vec<ClientStateHandle> {
        self.collect_all_pred(|client| client.authorized())
    }

    pub fn collect_all_pred<F: Fn(&ClientStateHandle) -> bool>(
        &self,
        predicate: F,
    ) -> Vec<ClientStateHandle> {
        self.map
            .iter()
            .filter_map(|entry| entry.value().upgrade())
            .filter(|client| predicate(client))
            .collect()
    }
}

fn normalize_username(username: &str) -> UsernameString {
    debug_assert!(username.len() <= MAX_USERNAME_LENGTH);

    let mut name = UsernameString::new();
    let _ = name.push_str(&username[..username.len().min(MAX_USERNAME_LENGTH)]);
    name.make_ascii_lowercase();
    name
}
