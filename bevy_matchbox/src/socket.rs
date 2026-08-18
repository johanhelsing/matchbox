use bevy::{
    prelude::{Command, Commands, Resource, World},
    tasks::IoTaskPool,
};
pub use matchbox_socket;
use matchbox_socket::{MessageLoopFuture, WebRtcSocket, WebRtcSocketBuilder};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
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
// The message loop task is owned rather than detached: dropping it cancels the loop, on every
// target since Bevy 0.19, which is what makes removing the resource close the socket.
#[allow(dead_code)]
pub struct MatchboxSocket(WebRtcSocket, Box<dyn Debug + Send + Sync>);

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
        let task_pool = IoTaskPool::get();
        let task = task_pool.spawn(message_loop_fut);
        MatchboxSocket(socket, Box::new(task))
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
    use bevy::{prelude::App, tasks::TaskPool};
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

        let watched: MessageLoopFuture = Box::pin(async move {
            let _flag = DropFlag(dropped);
            std::future::pending().await
        });

        MatchboxSocket::from((socket, watched))
    }

    /// The socket owns its message loop task rather than detaching it, so dropping the
    /// resource cancels the loop. Detached, the loop would outlive every socket and
    /// `close_socket` would be a rename of `remove_resource`.
    #[test]
    fn closing_the_socket_cancels_its_message_loop() {
        IoTaskPool::get_or_init(TaskPool::default);

        let dropped = Arc::new(AtomicBool::new(false));
        let mut app = App::new();
        app.insert_resource(socket_watching_its_loop(dropped.clone()));

        assert!(
            !dropped.load(Ordering::SeqCst),
            "the message loop runs while the socket holds it"
        );

        app.world_mut().remove_resource::<MatchboxSocket>();

        // Cancellation hands the future back to the executor to drop, so it is not
        // observable the instant the task goes.
        for _ in 0..200 {
            if dropped.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the message loop was never dropped, so the task was detached, not owned");
    }
}
