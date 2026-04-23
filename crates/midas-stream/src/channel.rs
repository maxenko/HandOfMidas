//! [`ChannelBarStream`] — wraps a `tokio::sync::mpsc::Receiver<Candle>`.
//!
//! Live-style stream: implements [`BarStream`] but NOT
//! [`SeekableBarStream`]. Snapshots return
//! [`StreamError::NotSeekable`] — snapshots are historical by nature,
//! and a live broadcast has no notion of replay.

use async_trait::async_trait;
use midas_bars::Candle;
use tokio::sync::mpsc;

use crate::{BarStream, BarStreamMeta, StreamError, TimeRange};

/// Thin adapter: a `mpsc::Receiver<Candle>` behind the [`BarStream`] trait.
pub struct ChannelBarStream {
    meta: BarStreamMeta,
    rx: mpsc::Receiver<Candle>,
}

impl ChannelBarStream {
    /// Wire the stream to an `mpsc::Receiver<Candle>`. The sender side
    /// is owned elsewhere (e.g. a provider that pushes aggregated
    /// candles from tick data).
    pub fn new(meta: BarStreamMeta, rx: mpsc::Receiver<Candle>) -> Self {
        Self { meta, rx }
    }

    /// Close the receiver. After calling `close`, `next` will drain any
    /// already-sent candles and then return `None`.
    pub fn close(&mut self) {
        self.rx.close();
    }
}

#[async_trait]
impl BarStream for ChannelBarStream {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }

    async fn next(&mut self) -> Option<Candle> {
        self.rx.recv().await
    }

    async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
        Err(StreamError::NotSeekable)
    }
}
