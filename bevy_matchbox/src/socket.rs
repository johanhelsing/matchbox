use bevy::{
    prelude::{Command, Commands, Resource, World},
    tasks::{IoTaskPool, Task},
};
pub use matchbox_socket;
use matchbox_socket::{Error, MessageLoopFuture, WebRtcSocket, WebRtcSocketBuilder};
use std::{
    fmt,
    ops::{Deref, DerefMut},
};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    future::poll_fn,
    sync::{Arc, Mutex, PoisonError},
    task::{Poll, ready},
};

/// A [`WebRtcSocket`] as a [`Resource`].
///
/// With [`Commands`]
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///     commands.open_socket(WebRtcSocketBuilder::new(room_url).add_channel(ChannelConfig::reliable()));
/// }
///
/// fn close_socket_system(mut commands: Commands) {
///     commands.close_socket();
/// }
/// ```
///
/// Directly
/// ```
/// use bevy_matchbox::prelude::*;
/// use bevy::prelude::*;
///
/// fn open_socket_system(mut commands: Commands) {
///     let room_url = "wss://matchbox.example.com";
///
///     let socket: MatchboxSocket = WebRtcSocketBuilder::new(room_url)
///         .add_channel(ChannelConfig::reliable())
///         .into();
///
///     commands.insert_resource(socket);
/// }
///
/// fn close_socket_system(mut commands: Commands) {
///     commands.remove_resource::<MatchboxSocket>();
/// }
/// ```
#[derive(Resource, Debug)]
// The message loop is owned rather than detached: dropping it ends the loop, which is what makes
// removing the resource close the socket.
#[allow(dead_code)]
pub struct MatchboxSocket(WebRtcSocket, MessageLoop);

impl Deref for MatchboxSocket {
    type Target = WebRtcSocket;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MatchboxSocket {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<WebRtcSocketBuilder> for MatchboxSocket {
    fn from(builder: WebRtcSocketBuilder) -> Self {
        Self::from(builder.build())
    }
}

impl From<(WebRtcSocket, MessageLoopFuture)> for MatchboxSocket {
    fn from((socket, message_loop_fut): (WebRtcSocket, MessageLoopFuture)) -> Self {
        MatchboxSocket(socket, MessageLoop::spawn(message_loop_fut))
    }
}

/// The message loop of a [`MatchboxSocket`], running on the [`IoTaskPool`].
///
/// The task is owned rather than detached, and dropping this ends the loop: that is what makes
/// removing the resource close the socket.
struct MessageLoop {
    // Never read, only held: dropping a task cancels it.
    #[allow(dead_code)]
    task: Task<Result<(), Error>>,
    /// The loop itself, shared with `task`, which polls it through this slot.
    ///
    /// Cancelling a task only asks its executor to drop the future, whenever it next gets to it.
    /// With Bevy's single-threaded task pool, that executor is a thread local of the main thread,
    /// ticked once per frame; after the app's last frame, the cancelled loop sits in it until the
    /// thread-local destructors run at process exit. By then tokio's thread-local context is gone,
    /// and the loop's webrtc futures cannot be dropped without it (async-compat enters the tokio
    /// runtime to drop them), so the process aborts on its way out. Taking the loop out of this
    /// slot in [`Drop`] ends it right away instead, on the thread dropping the socket.
    #[cfg(not(target_arch = "wasm32"))]
    future: Arc<Mutex<Option<MessageLoopFuture>>>,
}

impl MessageLoop {
    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(future: MessageLoopFuture) -> Self {
        let future = Arc::new(Mutex::new(Some(future)));
        let slot = Arc::clone(&future);
        let task = IoTaskPool::get().spawn(poll_fn(move |cx| {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(future) = slot.as_mut() else {
                // The socket was dropped, and took the loop with it.
                return Poll::Ready(Ok(()));
            };
            let result = ready!(future.as_mut().poll(cx));
            *slot = None;
            Poll::Ready(result)
        }));
        Self { task, future }
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn(future: MessageLoopFuture) -> Self {
        Self {
            task: IoTaskPool::get().spawn(future),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for MessageLoop {
    fn drop(&mut self) {
        // Take the loop out under the lock, but drop it only after releasing the lock, so that a
        // concurrent poll of the task on another thread is not held up by the loop's teardown.
        let future = self
            .future
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(future);
    }
}

impl fmt::Debug for MessageLoop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageLoop").finish_non_exhaustive()
    }
}

/// A [`Command`] used to open a [`MatchboxSocket`] and allocate it as a resource.
struct OpenSocket(WebRtcSocketBuilder);

impl Command for OpenSocket {
    type Out = ();

    fn apply(self, world: &mut World) {
        world.insert_resource(MatchboxSocket::from(self.0));
    }
}

/// A [`Commands`] extension used to open a [`MatchboxSocket`] and allocate it as a resource.
pub trait OpenSocketExt {
    /// Opens a [`MatchboxSocket`] and allocates it as a resource.
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder);
}

impl OpenSocketExt for Commands<'_, '_> {
    fn open_socket(&mut self, socket_builder: WebRtcSocketBuilder) {
        self.queue(OpenSocket(socket_builder))
    }
}

/// A [`Command`] used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`] resource.
struct CloseSocket;

impl Command for CloseSocket {
    type Out = ();

    fn apply(self, world: &mut World) {
        world.remove_resource::<MatchboxSocket>();
    }
}

/// A [`Commands`] extension used to close a [`WebRtcSocket`], deleting the [`MatchboxSocket`]
/// resource.
pub trait CloseSocketExt {
    /// Delete the [`MatchboxSocket`] resource.
    fn close_socket(&mut self);
}

impl CloseSocketExt for Commands<'_, '_> {
    fn close_socket(&mut self) {
        self.queue(CloseSocket)
    }
}

impl MatchboxSocket {
    /// Create a new socket with a single unreliable channel
    ///
    /// ```rust
    /// use bevy_matchbox::prelude::*;
    /// use bevy::prelude::*;
    ///
    /// fn open_channel_system(mut commands: Commands) {
    ///     let room_url = "wss://matchbox.example.com";
    ///     let socket = MatchboxSocket::new_unreliable(room_url);
    ///     commands.insert_resource(socket);
    /// }
    /// ```
    pub fn new_unreliable(room_url: impl Into<String>) -> MatchboxSocket {
        Self::from(WebRtcSocket::new_unreliable(room_url))
    }

    /// Create a new socket with a single reliable channel
    ///
    /// ```rust
    /// use bevy_matchbox::prelude::*;
    /// use bevy::prelude::*;
    ///
    /// fn open_channel_system(mut commands: Commands) {
    ///     let room_url = "wss://matchbox.example.com";
    ///     let socket = MatchboxSocket::new_reliable(room_url);
    ///     commands.insert_resource(socket);
    /// }
    /// ```
    pub fn new_reliable(room_url: impl Into<String>) -> MatchboxSocket {
        Self::from(WebRtcSocket::new_reliable(room_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{prelude::App, tasks::TaskPoolBuilder};
    use matchbox_socket::ChannelConfig;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    /// Sets a flag when dropped, so a cancelled future is observable.
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// The real message loop is discarded: what is under test is what owning the task
    /// buys, not connecting to anything.
    fn socket_watching_its_loop(dropped: Arc<AtomicBool>) -> MatchboxSocket {
        let (socket, _real_loop) = WebRtcSocketBuilder::new("ws://localhost:1/drop_test")
            .add_channel(ChannelConfig::reliable())
            .build();

        // Captured rather than created in the async block, so that dropping the loop sets the
        // flag even if the loop was never polled.
        let flag = DropFlag(dropped);
        let watched: MessageLoopFuture = Box::pin(async move {
            let _flag = flag;
            std::future::pending().await
        });

        MatchboxSocket::from((socket, watched))
    }

    /// The socket owns its message loop rather than detaching it, so dropping the resource
    /// ends the loop. Detached, the loop would outlive every socket and `close_socket` would
    /// be a rename of `remove_resource`.
    ///
    /// The loop must be gone by the time the resource is: leaving it to the executor to drop
    /// the cancelled task is not enough. Nothing ticks this app's task pool (nor, with Bevy's
    /// single-threaded pool, an app that has exited), which is exactly when a deferred drop
    /// ends up in the thread-local destructors and aborts the process.
    #[test]
    fn closing_the_socket_ends_its_message_loop() {
        // No worker threads: with the multi-threaded pool too, nothing ever runs the task, so a
        // loop left to the executor would never be dropped, deterministically.
        IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());

        let dropped = Arc::new(AtomicBool::new(false));
        let mut app = App::new();
        app.insert_resource(socket_watching_its_loop(dropped.clone()));

        assert!(
            !dropped.load(Ordering::SeqCst),
            "the message loop runs while the socket holds it"
        );

        app.world_mut().remove_resource::<MatchboxSocket>();

        assert!(
            dropped.load(Ordering::SeqCst),
            "removing the socket must drop its message loop, not leave it to the executor"
        );
    }
}
