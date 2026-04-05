use futures::future::Future;
use std::fmt::Display;
use tokio::sync::mpsc::{self, Sender};

pub enum BufferSize {
    Default,
    Size(usize),
}

impl BufferSize {
    fn unwrap_or(&self, default_value: usize) -> usize {
        match self {
            BufferSize::Default => default_value,
            BufferSize::Size(x) => *x,
        }
    }
}

#[derive(Debug)]
pub struct MailboxProcessorError {
    msg: String,
}

#[derive(Debug)]
pub struct MailboxProcessor<Msg, ReplyMsg> {
    message_sender: Sender<(Msg, Option<Sender<ReplyMsg>>)>,
}

// Manual Clone: only clones the Sender (cheap Arc increment).
// Does NOT require Msg/ReplyMsg to be Clone.
impl<Msg, ReplyMsg> Clone for MailboxProcessor<Msg, ReplyMsg> {
    fn clone(&self) -> Self {
        Self {
            message_sender: self.message_sender.clone(),
        }
    }
}

impl<Msg: 'static + Send, ReplyMsg: 'static + Send> MailboxProcessor<Msg, ReplyMsg> {
    pub async fn new<State: 'static + Send, F>(
        buffer_size: BufferSize,
        initial_state: State,
        message_processing_function: impl Fn(Msg, State, Option<Sender<ReplyMsg>>) -> F
            + Send
            + Sync
            + 'static,
    ) -> Self
    where
        F: Future<Output = State> + Send,
    {
        let (s, mut r) = mpsc::channel(buffer_size.unwrap_or(1_000));

        tokio::task::spawn(async move {
            let mut state = initial_state;
            while let Some((msg, reply_channel)) = r.recv().await {
                state = message_processing_function(msg, state, reply_channel).await;
            }
        });

        MailboxProcessor { message_sender: s }
    }

    /// Sync handler on a dedicated OS thread. For blocking FFI / !Sync resources.
    ///
    /// The handler runs on a `std::thread` (not a tokio task), making it safe
    /// for DuckDB's synchronous C++ FFI calls. Messages are received via
    /// `blocking_recv()` so the thread sleeps when idle.
    pub fn new_blocking<State: 'static + Send>(
        buffer_size: BufferSize,
        initial_state: State,
        thread_name: &str,
        handler: impl Fn(Msg, State, Option<Sender<ReplyMsg>>) -> State + Send + 'static,
    ) -> Self {
        let (s, mut r) = mpsc::channel(buffer_size.unwrap_or(1_000));
        std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                let mut state = initial_state;
                while let Some((msg, reply_channel)) = r.blocking_recv() {
                    state = handler(msg, state, reply_channel);
                }
            })
            .expect("failed to spawn mailbox processor thread");
        MailboxProcessor { message_sender: s }
    }

    pub async fn send(&self, msg: Msg) -> Result<ReplyMsg, MailboxProcessorError> {
        let (s, mut r) = mpsc::channel(1);
        self.message_sender
            .send((msg, Some(s)))
            .await
            .map_err(|_| MailboxProcessorError {
                msg: "the mailbox channel is closed".to_owned(),
            })?;

        r.recv().await.ok_or(MailboxProcessorError {
            msg: "the response channel is closed (did you mean to call fire_and_forget()?)".to_owned(),
        })
    }

    pub async fn fire_and_forget(&self, msg: Msg) -> Result<(), MailboxProcessorError> {
        self.message_sender
            .send((msg, None))
            .await
            .map_err(|_| MailboxProcessorError {
                msg: "the mailbox channel is closed".to_owned(),
            })
    }
}

impl Display for MailboxProcessorError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::OptionFuture;

    #[tokio::test]
    async fn mailbox_processor_tests() {
        enum SendMessageTypes {
            Increment(i32),
            GetCurrentCount,
            Decrement(i32),
        }

        let mb = MailboxProcessor::<SendMessageTypes, i32>::new(
            BufferSize::Default,
            0,
            |msg, state, reply_channel| async move {
                match msg {
                    SendMessageTypes::Increment(x) => {
                        OptionFuture::from(
                            reply_channel.map(|rc| async move { rc.send(state + x).await.unwrap() }),
                        )
                        .await;
                        state + x
                    }
                    SendMessageTypes::GetCurrentCount => {
                        OptionFuture::from(
                            reply_channel.map(|rc| async move { rc.send(state).await.unwrap() }),
                        )
                        .await;
                        state
                    }
                    SendMessageTypes::Decrement(x) => {
                        OptionFuture::from(
                            reply_channel.map(|rc| async move { rc.send(state - x).await.unwrap() }),
                        )
                        .await;
                        state - x
                    }
                }
            },
        )
        .await;

        assert_eq!(mb.send(SendMessageTypes::GetCurrentCount).await.unwrap(), 0);
        mb.fire_and_forget(SendMessageTypes::Increment(55))
            .await
            .unwrap();
        assert_eq!(
            mb.send(SendMessageTypes::GetCurrentCount).await.unwrap(),
            55
        );
        assert_eq!(
            mb.send(SendMessageTypes::Increment(55)).await.unwrap(),
            110
        );
        assert_eq!(
            mb.send(SendMessageTypes::Decrement(10)).await.unwrap(),
            100
        );
    }

    #[tokio::test]
    async fn new_blocking_roundtrip() {
        enum Msg {
            Add(i32),
            Get,
        }

        let mb = MailboxProcessor::<Msg, i32>::new_blocking(
            BufferSize::Size(100),
            0i32,
            "test-blocking",
            |msg, state, reply_channel| {
                let new_state = match msg {
                    Msg::Add(n) => state + n,
                    Msg::Get => state,
                };
                if let Some(ch) = reply_channel {
                    let _ = ch.blocking_send(new_state);
                }
                new_state
            },
        );

        assert_eq!(mb.send(Msg::Get).await.unwrap(), 0);
        assert_eq!(mb.send(Msg::Add(10)).await.unwrap(), 10);
        assert_eq!(mb.send(Msg::Add(20)).await.unwrap(), 30);
        assert_eq!(mb.send(Msg::Get).await.unwrap(), 30);
    }
}
