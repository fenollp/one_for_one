use std::future::Future;

use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    task::JoinHandle,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Listens to OS signals for ever. Completes on the first one received amon:
/// * SIGINT (^C)
/// * SIGTERM
///
/// Usually, this is awaited on through `tokio::select!`
/// to then call a given supervisor's `sup.die().await`.
pub async fn should_die() {
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to listen to SIGINT");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to listen to SIGTERM");
    select! {
        _ = sigint.recv() => tracing::warn!("Got ^C signal!"),
        _ = sigterm.recv() => tracing::warn!("Got SIGTERM signal!"),
    }
}

/// Supervisor implements a supervision tree
/// where all children get terminated on interrupt
/// but each die on its own when it's their own trauma.
///
/// This supervision strategy is was dubbed "one_for_one" by the Erlang people:
/// See https://learnyousomeerlang.com/supervisors
/// See https://www.erlang.org/doc/man/supervisor.html
#[derive(Clone, Debug, Default)]
pub struct Supervisor {
    token: CancellationToken,
    tasks: TaskTracker,
}

#[test]
fn assert_all() {
    fn assert_clone<T: Clone>() {}
    fn assert_send<T: Send>() {}
    fn assert_sized<T: Sized>() {}
    fn assert_sync<T: Sync>() {}

    assert_clone::<Supervisor>();
    assert_send::<Supervisor>();
    assert_sized::<Supervisor>();
    assert_sync::<Supervisor>();
}

// // TODO: derive(spawn_under("sup")) to auto-impl Supervized

// /// The revolution will be Supervized
// pub trait Supervized {
//     /// Consumes `sup` to keep in `self`
//     fn set_supervisor(self, sup: Supervisor);
// }

impl Supervisor {
    // /// Branch a `Supervized` instance under this tree
    // pub fn supervize(&self, instance: impl Supervized) {
    //     instance.set_supervisor(self.clone());
    // }

    /// Spawn starts a supervised task, calling `spawn`.
    #[inline]
    #[track_caller]
    pub fn spawn<F, G>(&self, task: G) -> JoinHandle<F::Output>
    where
        G: FnOnce(Self) -> F,
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let this = self.clone();
        self.tasks.spawn(task(this))
    }

    /// Spawn starts a supervised task, calling `spawn_blocking`.
    #[inline]
    #[track_caller]
    pub fn spawn_blocking<F, T>(&self, task: F) -> JoinHandle<T>
    where
        F: FnOnce(Self) -> T,
        F: Send + 'static,
        T: Send + 'static,
    {
        let this = self.clone();
        self.tasks.spawn_blocking(|| task(this))
    }

    /// Returns supervision state immediately
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// A future that completes when the supervision
    /// tree is interrupted.
    /// Akin to Go's `<-ctx.Done()` chan recv.
    #[allow(clippy::manual_async_fn)] // FIXME: try dropping (does it keep Send?)
    pub fn done(&self) -> impl Future<Output = ()> + Send + '_ {
        async move {
            // Gets sibling tokens (don't die if a sibling dies)
            let _: () = self.token.child_token().cancelled().await;
            tracing::warn!("Children cancelled");
        }
    }

    /// A future that triggers the interruption or termination
    /// of the supervision tree.
    /// Completes when all children are confirmed dead.
    #[allow(clippy::manual_async_fn)] // FIXME: try dropping (does it keep Send?)
    pub fn die(&self) -> impl Future<Output = ()> + Send + '_ {
        async move {
            tracing::warn!("Terminating...");
            self.tasks.close();
            self.token.cancel();
            self.tasks.wait().await;
            tracing::warn!("Terminated!");
        }
    }
}
