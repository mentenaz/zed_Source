use thiserror::Error;

/// Typed errors surfaced by connection lifecycle and metadata operations.
///
/// Driver-specific variants (e.g. distinguishing a TLS failure from a bad
/// password) are added as each driver lands; this Phase 0 shape covers the
/// categories every driver needs regardless of backend.
#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("connection failed: {0}")]
    Connection(String),

    #[error("connection not found: {0:?}")]
    NotFound(crate::connection::ConnectionId),

    #[error("unsupported operation for this database type: {0}")]
    Unsupported(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
