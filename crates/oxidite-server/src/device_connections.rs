//! Active device connection registry enforcing immediate local revocation and 30s cross-node revalidation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone)]
struct ActiveConnection {
    device_id: Uuid,
    close_tx: mpsc::Sender<()>,
    last_validated: Instant,
}

#[derive(Clone)]
pub struct DeviceConnectionRegistry {
    max_connections: usize,
    connections: Arc<Mutex<HashMap<u64, ActiveConnection>>>,
    by_device: Arc<Mutex<HashMap<Uuid, Vec<u64>>>>,
}

impl Default for DeviceConnectionRegistry {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl DeviceConnectionRegistry {
    pub fn new(max_connections: usize) -> Self {
        Self {
            max_connections,
            connections: Arc::new(Mutex::new(HashMap::new())),
            by_device: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers an active connection for a device.
    pub fn register(
        &self,
        device_id: Uuid,
        connection_id: u64,
        close_tx: mpsc::Sender<()>,
    ) -> bool {
        let mut conns = self.connections.lock().unwrap();
        if conns.len() >= self.max_connections {
            return false;
        }

        conns.insert(
            connection_id,
            ActiveConnection {
                device_id,
                close_tx,
                last_validated: Instant::now(),
            },
        );

        let mut by_dev = self.by_device.lock().unwrap();
        by_dev.entry(device_id).or_default().push(connection_id);
        true
    }

    /// Unregisters a connection when it disconnects or terminates.
    pub fn unregister(&self, connection_id: u64) {
        let dev_id = {
            let mut conns = self.connections.lock().unwrap();
            conns.remove(&connection_id).map(|c| c.device_id)
        };

        if let Some(device_id) = dev_id {
            let mut by_dev = self.by_device.lock().unwrap();
            if let Some(list) = by_dev.get_mut(&device_id) {
                list.retain(|&id| id != connection_id);
                if list.is_empty() {
                    by_dev.remove(&device_id);
                }
            }
        }
    }

    /// Immediately closes all active local connections associated with `device_id`.
    pub fn close_local(&self, device_id: Uuid) {
        let conn_ids = {
            let mut by_dev = self.by_device.lock().unwrap();
            by_dev.remove(&device_id).unwrap_or_default()
        };

        let mut conns = self.connections.lock().unwrap();
        for id in conn_ids {
            if let Some(conn) = conns.remove(&id) {
                let _ = conn.close_tx.try_send(());
            }
        }
    }

    /// Checks whether this connection has exceeded `max_age` since its last validation.
    pub fn requires_revalidation(&self, connection_id: u64, max_age: Duration) -> bool {
        let conns = self.connections.lock().unwrap();
        match conns.get(&connection_id) {
            Some(conn) => conn.last_validated.elapsed() >= max_age,
            None => false,
        }
    }

    /// Updates the last validated timestamp for a connection.
    pub fn mark_validated(&self, connection_id: u64) {
        let mut conns = self.connections.lock().unwrap();
        if let Some(conn) = conns.get_mut(&connection_id) {
            conn.last_validated = Instant::now();
        }
    }
}
