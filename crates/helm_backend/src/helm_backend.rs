//! GitHub backend for the Helm panel: OAuth device-flow sign-in, the REST
//! API, and cloning.
//!
//! No GPUI and no UI state. HTTP goes through the host's client, handed in
//! as `GhState::http`; storing the token is the host's job, which hands it
//! in with `GhState::set_token`. The functions are async and run on a tokio
//! runtime, because the HTTP client and the clone's child process need one;
//! a GPUI host bridges to them with [`on_tokio`].

pub mod github;

use std::future::Future;

/// Why a wait on [`github::GhState`]'s clone events ended without
/// one. Re-exported so a host can match on it without depending on tokio.
pub use tokio::sync::broadcast::error::RecvError as EventRecvError;

/// Runs `future` on the tokio runtime and waits for it from any executor.
/// This is how a GPUI task awaits this crate's functions.
pub async fn on_tokio<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    reqwest_client::runtime()
        .spawn(future)
        .await
        .expect("tokio task panicked")
}
