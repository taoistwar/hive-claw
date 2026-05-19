//! Async message bus that decouples chat channels from the agent core
//! (Rust port of `nanobot.bus.queue`).
//!
//! Python's `asyncio.Queue` without a `maxsize` is effectively unbounded, so
//! we model it with `tokio::sync::mpsc::unbounded_channel`. Receivers are
//! protected by an async mutex so any number of producers / a single
//! consumer pair can share one `MessageBus` via `Arc`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

use crate::events::{InboundMessage, OutboundMessage};

/// Send side for inbound messages (from channels to agent).
pub type InboundTx = UnboundedSender<InboundMessage>;
/// Send side for outbound messages (from agent to channels).
pub type OutboundTx = UnboundedSender<OutboundMessage>;

struct Lane<T> {
    tx: UnboundedSender<T>,
    rx: Mutex<UnboundedReceiver<T>>,
    pending: AtomicUsize,
}

impl<T> Lane<T> {
    fn new() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(rx),
            pending: AtomicUsize::new(0),
        }
    }

    fn publish(&self, item: T) -> Result<(), tokio::sync::mpsc::error::SendError<T>> {
        self.tx.send(item)?;
        self.pending.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn consume(&self) -> Option<T> {
        let mut rx = self.rx.lock().await;
        let item = rx.recv().await?;
        self.pending.fetch_sub(1, Ordering::SeqCst);
        Some(item)
    }

    fn size(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
}

/// Async message bus that decouples chat channels from the agent core.
///
/// Channels push messages to the inbound queue, and the agent processes
/// them and pushes responses to the outbound queue.
///
/// Cheap to `clone()` via `Arc`.
#[derive(Clone)]
pub struct MessageBus {
    inner: Arc<BusInner>,
}

struct BusInner {
    inbound: Lane<InboundMessage>,
    outbound: Lane<OutboundMessage>,
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageBus {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BusInner {
                inbound: Lane::new(),
                outbound: Lane::new(),
            }),
        }
    }

    /// Publish a message from a channel to the agent.
    pub async fn publish_inbound(&self, msg: InboundMessage) {
        let _ = self.inner.inbound.publish(msg);
    }

    /// Consume the next inbound message (blocks until available).
    /// Returns `None` when all senders have been dropped.
    pub async fn consume_inbound(&self) -> Option<InboundMessage> {
        self.inner.inbound.consume().await
    }

    /// Publish a response from the agent to channels.
    pub async fn publish_outbound(&self, msg: OutboundMessage) {
        let _ = self.inner.outbound.publish(msg);
    }

    /// Consume the next outbound message (blocks until available).
    pub async fn consume_outbound(&self) -> Option<OutboundMessage> {
        self.inner.outbound.consume().await
    }

    /// Number of pending inbound messages.
    pub fn inbound_size(&self) -> usize {
        self.inner.inbound.size()
    }

    /// Number of pending outbound messages.
    pub fn outbound_size(&self) -> usize {
        self.inner.outbound.size()
    }

    /// Cheap standalone producer handle (for channel-side code) for inbound.
    pub fn inbound_sender(&self) -> InboundTx {
        self.inner.inbound.tx.clone()
    }

    /// Cheap standalone producer handle for outbound.
    pub fn outbound_sender(&self) -> OutboundTx {
        self.inner.outbound.tx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_inbound_outbound() {
        let bus = MessageBus::new();
        let msg = InboundMessage {
            channel: "telegram".into(),
            sender_id: "u1".into(),
            chat_id: "c1".into(),
            content: "hi".into(),
            ..Default::default()
        };
        assert_eq!(bus.inbound_size(), 0);
        bus.publish_inbound(msg.clone()).await;
        assert_eq!(bus.inbound_size(), 1);
        assert_eq!(msg.session_key(), "telegram:c1");

        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.content, "hi");
        assert_eq!(bus.inbound_size(), 0);

        let out = OutboundMessage {
            channel: "telegram".into(),
            chat_id: "c1".into(),
            content: "pong".into(),
            ..Default::default()
        };
        bus.publish_outbound(out).await;
        assert_eq!(bus.outbound_size(), 1);
        let got = bus.consume_outbound().await.unwrap();
        assert_eq!(got.content, "pong");
    }

    #[tokio::test]
    async fn session_key_override() {
        let msg = InboundMessage {
            channel: "telegram".into(),
            sender_id: "u".into(),
            chat_id: "c1".into(),
            content: String::new(),
            session_key_override: Some("thread:42".into()),
            ..Default::default()
        };
        assert_eq!(msg.session_key(), "thread:42");
    }
}
