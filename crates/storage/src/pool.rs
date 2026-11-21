//! Bounded read-connection pool.
//!
//! Writers are serialized by [`crate::actor::StorageActor`]; reads use this
//! small fixed pool so queries can proceed concurrently with WAL checkpoints.
//! The pool never opens unbounded extra connections: when every connection is
//! checked out, callers wait for one to be returned.

use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};

use rusqlite::Connection;

use crate::db;
use crate::error::StorageError;

pub(crate) struct ReadPool {
    sender: Sender<Connection>,
    receiver: Mutex<Receiver<Connection>>,
}

impl ReadPool {
    pub(crate) fn open(path: &Path, size: usize) -> Result<Self, StorageError> {
        if !(1..=16).contains(&size) {
            return Err(StorageError::InvalidConfig {
                reason: "read_connections must be between 1 and 16".to_owned(),
            });
        }
        let (sender, receiver) = mpsc::channel();
        for _ in 0..size {
            sender
                .send(db::open_reader(path)?)
                .map_err(|_| StorageError::Closed)?;
        }
        Ok(Self {
            sender,
            receiver: Mutex::new(receiver),
        })
    }

    pub(crate) fn acquire(&self) -> Result<PooledConnection<'_>, StorageError> {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let connection = receiver.recv().map_err(|_| StorageError::Closed)?;
        drop(receiver);
        Ok(PooledConnection {
            sender: &self.sender,
            connection: Some(connection),
        })
    }
}

pub(crate) struct PooledConnection<'pool> {
    sender: &'pool Sender<Connection>,
    connection: Option<Connection>,
}

impl PooledConnection<'_> {
    pub(crate) fn with<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        match self.connection.as_ref() {
            Some(connection) => operation(connection),
            None => Err(StorageError::Closed),
        }
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            // A disconnected receiver means the store is shutting down; the
            // connection is simply dropped.
            let _ = self.sender.send(connection);
        }
    }
}
