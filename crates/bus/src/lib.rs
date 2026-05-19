//! Message bus module for decoupled channel-agent communication.

pub mod events;
pub mod queue;

pub use events::{InboundMessage, OutboundMessage};
pub use queue::{InboundTx, MessageBus, OutboundTx};
