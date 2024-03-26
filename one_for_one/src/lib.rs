use std::future::Future;

use tokio::{
    select,
    signal::unix::{signal, SignalKind},
    task::{futures::TaskLocalFuture, JoinHandle},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::warn;

/// Listens to OS signals for ever. Completes on the first one received amon:
/// * SIGINT (^C)
/// * SIGTERM
///
/// Usually, this is awaited on through `tokio::select!`
/// to then call a given supervisor's `sup.die().await`.
pub async fn should_die() {
    //TODO: take in bitflags
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to listen to SIGINT");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to listen to SIGTERM");
    select! {
        _ = sigint.recv() => warn!("Got ^C signal!"),
        _ = sigterm.recv() => warn!("Got SIGTERM signal!"),
        // TODO: also wait for Notify (all_waiters)
    }
}

tokio::task_local! {
    static SUPERVIZED: Supervisor;
}

/// Gets a handle on this coode's Supervisor.
///
/// Returns `None` if outside of `supervized(async move { .. })`
/// or of `sync_supervized(|| ..)`.
#[inline]
pub fn current() -> Option<Supervisor> {
    SUPERVIZED.try_with(Clone::clone).ok()
}

/// Scopes (async) code that needs supervision.
pub fn supervized<F>(f: F) -> TaskLocalFuture<Supervisor, F>
where
    F: std::future::Future,
{
    let slf = current().unwrap_or_default();
    SUPERVIZED.scope(slf, f)
}

/// Scopes code that needs supervision.
#[track_caller]
pub fn sync_supervized<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let slf = current().unwrap_or_default();
    SUPERVIZED.sync_scope(slf, f)
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

const REASON: &str = "Not running within supervized(async move { .. }) or sync_supervized(|| ..)";

impl Supervisor {
    /// Spawn starts a supervised task, calling `spawn`.
    #[inline]
    #[track_caller]
    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let slf = current().expect(REASON);
        slf.tasks.spawn(future)
    }

    /// Spawn starts a supervised task, calling `spawn_blocking`.
    #[track_caller]
    pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let slf = current().expect(REASON);
        slf.tasks.spawn_blocking(f)
    }

    /// Returns supervision state immediately
    #[must_use]
    pub fn is_cancelled() -> bool {
        let slf = current().expect(REASON);
        slf.token.is_cancelled()
    }

    /// A future that completes when the supervision
    /// tree is interrupted.
    /// Akin to Go's `<-ctx.Done()` chan recv.
    pub async fn done() {
        let slf = current().expect(REASON);
        // Gets sibling tokens (don't die if a sibling dies)
        let _: () = slf.token.child_token().cancelled().await;
        warn!("Children cancelled");
    }

    /// A future that triggers the interruption or termination
    /// of the supervision tree.
    /// Completes when all children are confirmed dead.
    pub async fn terminate() {
        let slf = current().expect(REASON);
        warn!("Terminating...");
        slf.tasks.close();
        slf.token.cancel();
        slf.tasks.wait().await;
        warn!("Terminated!");
    }
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
