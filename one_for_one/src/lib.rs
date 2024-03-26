use std::{future::Future /*sync::OnceLock*/};

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

// static ONCE_LOCK: OnceLock<Supervisor> = OnceLock::new();

tokio::task_local! {
    static SUPERVIZED: Supervisor;
}

/// Scopes (async) code that needs supervision.
pub async fn supervized<F>(f: F) -> TaskLocalFuture<Supervisor, F>
where
    F: std::future::Future,
{
    assert!(SUPERVIZED.try_with(|_| ()).is_err());
    SUPERVIZED.scope(Supervisor::default(), f)
}

/// Scopes code that needs supervision.
#[track_caller]
pub fn sync_supervized<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    assert!(SUPERVIZED.try_with(|_| ()).is_err());
    SUPERVIZED.sync_scope(Supervisor::default(), f)
}

/// Spawn starts a supervised task, calling `spawn`.
#[inline]
#[track_caller]
pub fn spawn<F, G>(task: G) -> JoinHandle<F::Output>
where
    G: FnOnce(Supervisor) -> F,
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let slf = SUPERVIZED.with(Clone::clone); //TODO: try_with -> thiserror + try_spawn*?
    let this = slf.clone();
    slf.tasks.spawn(task(this))
}

/// Spawn starts a supervised task, calling `spawn_blocking`.
#[inline]
#[track_caller]
pub fn spawn_blocking<F, T>(task: F) -> JoinHandle<T>
where
    F: FnOnce(Supervisor) -> T,
    F: Send + 'static,
    T: Send + 'static,
{
    let slf = SUPERVIZED.with(Clone::clone);
    let this = slf.clone();
    slf.tasks.spawn_blocking(|| task(this))
}

/// Returns supervision state immediately
#[must_use]
pub fn is_cancelled() -> bool {
    let slf = SUPERVIZED.with(Clone::clone);
    slf.token.is_cancelled()
}

/// A future that completes when the supervision
/// tree is interrupted.
/// Akin to Go's `<-ctx.Done()` chan recv.
pub async fn done() {
    let slf = SUPERVIZED.with(Clone::clone);
    // Gets sibling tokens (don't die if a sibling dies)
    let _: () = slf.token.child_token().cancelled().await;
    warn!("Children cancelled");
}

/// A future that triggers the interruption or termination
/// of the supervision tree.
/// Completes when all children are confirmed dead.
pub async fn die() {
    let slf = SUPERVIZED.with(Clone::clone);
    warn!("Terminating...");
    slf.tasks.close();
    slf.token.cancel();
    slf.tasks.wait().await;
    warn!("Terminated!");
}

impl Supervisor {
    // /// Branch a `Supervized` instance under this tree
    // pub fn supervize(&self, instance: impl Supervized) {
    //     instance.set_supervisor(self.clone());
    // }

    // pub fn init() -> Supervisor {
    //     let supervisor = Supervisor::default();
    //     ONCE_LOCK.set(supervisor.clone()).expect("setting supervisor singleton");
    //     supervisor
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
            warn!("Children cancelled");
        }
    }

    /// A future that triggers the interruption or termination
    /// of the supervision tree.
    /// Completes when all children are confirmed dead.
    #[allow(clippy::manual_async_fn)] // FIXME: try dropping (does it keep Send?)
    pub fn die(&self) -> impl Future<Output = ()> + Send + '_ {
        async move {
            warn!("Terminating...");
            self.tasks.close();
            self.token.cancel();
            self.tasks.wait().await;
            warn!("Terminated!");
        }
    }
}
