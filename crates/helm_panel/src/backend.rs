pub mod github;

use std::future::Future;

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
