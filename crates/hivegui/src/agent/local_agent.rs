//! Local agent runtime (T125 boundary).
//!
//! T125 implementation: public session startup, per-turn immutable
//! Agent/Tool/Skill/Capability snapshots, and direct-child-only
//! routing. The runtime is intentionally local-only; remote
//! execution is never wired in.

#![warn(missing_docs)]

use std::{collections::BTreeMap, sync::Arc};

use parking_lot::RwLock;
use sqlx::{Pool, Sqlite};
use thiserror::Error;
use uuid::Uuid;

use crate::agent::session::{AgentMessage, AgentSession, CancelToken};
use crate::datasource::entity_store::{AgentRecord, AgentStore};

/// Stable handle to a single conversation session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SessionHandle(pub String);

impl SessionHandle {
    /// Borrow the underlying identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors surfaced by the local agent runtime.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LocalAgentError {
    /// `start_session` was called but no default root Agent exists.
    #[error("no default root agent; create one before starting a session")]
    NoDefaultRoot,
    /// The Agent hierarchy is malformed.
    #[error("agent hierarchy is malformed: {0}")]
    InvalidHierarchy(String),
    /// The supplied user_message was empty or larger than 1MiB.
    #[error("user_message rejected: {0}")]
    InvalidUserMessage(&'static str),
    /// The Agent rejected the operation.
    #[error("agent operation rejected: {0}")]
    AgentRejected(&'static str),
    /// A tool call routed to a remote / network target.
    #[error("tool {0} is not local; remote calls are forbidden")]
    RemoteToolForbidden(String),
    /// Database or store failure.
    #[error("store error: {0}")]
    Store(String),
}

/// Per-turn snapshot of Agent, Tool, Skill and Capability state.
#[derive(Debug, Clone)]
pub struct TurnSnapshot {
    /// The Agent this snapshot belongs to.
    pub agent: AgentRecord,
    /// Resolved tool ids.
    pub tool_ids: Vec<i64>,
    /// Resolved skill ids.
    pub skill_ids: Vec<i64>,
    /// Resolved global always-on skill ids.
    pub always_skill_ids: Vec<i64>,
    /// Resolved capability names.
    pub capability_names: Vec<String>,
    /// The child Agents that this Agent is allowed to route to.
    pub direct_children: Vec<AgentRecord>,
}

impl TurnSnapshot {
    /// Build a snapshot from a typed [`AgentStore`].
    pub async fn load(store: &AgentStore, agent_id: i64) -> Result<Self, LocalAgentError> {
        let agent = store
            .fetch_one(agent_id)
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?
            .ok_or(LocalAgentError::InvalidHierarchy(format!(
                "agent {agent_id} not found"
            )))?;
        let tool_ids = agent.tool_ids().to_vec();
        let skill_ids = agent.skill_ids().to_vec();
        let always_skill_ids = agent.always_skill_ids().to_vec();
        let capability_names = agent.capability_names().to_vec();
        let children = store
            .list_children(agent_id)
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(Self {
            agent,
            tool_ids,
            skill_ids,
            always_skill_ids,
            capability_names,
            direct_children: children,
        })
    }
}

/// Local-only agent runtime.
#[derive(Debug, Clone)]
pub struct LocalAgentRuntime {
    pool: Pool<Sqlite>,
    store: AgentStore,
    sessions: Arc<RwLock<BTreeMap<SessionHandle, AgentSession>>>,
    snapshots: Arc<RwLock<BTreeMap<SessionHandle, TurnSnapshot>>>,
}

impl LocalAgentRuntime {
    /// Build a new runtime against the given SQLite pool.
    pub fn new(pool: Pool<Sqlite>) -> Result<Self, LocalAgentError> {
        let store =
            AgentStore::new(pool.clone()).map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(Self {
            pool,
            store,
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
            snapshots: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// Underlying pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Underlying Agent store.
    pub fn store(&self) -> &AgentStore {
        &self.store
    }

    /// Start a new session against the unique default root Agent.
    pub async fn start_session(
        &self,
        user_message: &str,
    ) -> Result<(SessionHandle, CancelToken), LocalAgentError> {
        if user_message.is_empty() {
            return Err(LocalAgentError::InvalidUserMessage("empty"));
        }
        if user_message.len() > 1024 * 1024 {
            return Err(LocalAgentError::InvalidUserMessage("exceeds 1MiB"));
        }
        let default = self
            .store
            .default_agent()
            .await
            .map_err(|e| LocalAgentError::Store(e.to_string()))?
            .ok_or(LocalAgentError::NoDefaultRoot)?;
        let session_id = Uuid::new_v4().to_string();
        let handle = SessionHandle(session_id.clone());
        let mut session = AgentSession::new(std::path::Path::new(":memory:"))
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        let token = session
            .start(user_message)
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        let snapshot = TurnSnapshot::load(&self.store, default.id()).await?;
        self.sessions.write().insert(handle.clone(), session);
        self.snapshots.write().insert(handle.clone(), snapshot);
        Ok((handle, token))
    }

    /// Fetch the current per-turn snapshot for a session.
    pub fn snapshot(&self, handle: &SessionHandle) -> Option<TurnSnapshot> {
        self.snapshots.read().get(handle).cloned()
    }

    /// Reload the per-turn snapshot from the store.
    pub async fn reload_snapshot(
        &self,
        handle: &SessionHandle,
    ) -> Result<TurnSnapshot, LocalAgentError> {
        if !self.sessions.read().contains_key(handle) {
            return Err(LocalAgentError::AgentRejected("session not found"));
        }
        let current = self
            .snapshot(handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;
        let fresh = TurnSnapshot::load(&self.store, current.agent.id()).await?;
        self.snapshots.write().insert(handle.clone(), fresh.clone());
        Ok(fresh)
    }

    /// Route the current turn to a direct child Agent.
    pub async fn route_to_child(
        &self,
        handle: &SessionHandle,
        child_identifier: &str,
    ) -> Result<TurnSnapshot, LocalAgentError> {
        let snapshot = self
            .snapshot(handle)
            .ok_or(LocalAgentError::AgentRejected("snapshot missing"))?;
        let child = snapshot
            .direct_children
            .iter()
            .find(|agent| agent.identifier() == child_identifier)
            .cloned()
            .ok_or(LocalAgentError::AgentRejected(
                "child not in direct_children set",
            ))?;
        let next = TurnSnapshot::load(&self.store, child.id()).await?;
        self.snapshots.write().insert(handle.clone(), next.clone());
        Ok(next)
    }

    /// Append a message to a session.
    pub fn append_message(
        &self,
        handle: &SessionHandle,
        message: AgentMessage,
    ) -> Result<(), LocalAgentError> {
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        session
            .append_message(message)
            .map_err(|e| LocalAgentError::Store(e.to_string()))
    }

    /// Terminate a session.
    pub fn end_session(&self, handle: &SessionHandle) -> Result<(), LocalAgentError> {
        let removed = self.sessions.write().remove(handle).is_some();
        self.snapshots.write().remove(handle);
        if removed {
            Ok(())
        } else {
            Err(LocalAgentError::AgentRejected("session not found"))
        }
    }

    /// Current session state as a stable wire string.
    pub fn session_state(&self, handle: &SessionHandle) -> Option<String> {
        self.sessions.read().get(handle).map(|s| s.state())
    }

    /// Cooperative cancel for a long-running tool call. The supplied
    /// [`CancelHandle`] is signalled, the session state transitions
    /// to `Terminated`, and the late-result guard ensures any
    /// in-flight adapter result is discarded. The session itself
    /// remains in the runtime for diagnostics; callers can
    /// [`Self::end_session`] to remove it.
    pub fn cancel_execution(&self, handle: &SessionHandle) -> Result<(), LocalAgentError> {
        let mut sessions = self.sessions.write();
        let session = sessions
            .get_mut(handle)
            .ok_or(LocalAgentError::AgentRejected("session not found"))?;
        session
            .cancel_in_flight()
            .map_err(|e| LocalAgentError::Store(e.to_string()))?;
        Ok(())
    }
}
