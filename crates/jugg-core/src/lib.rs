//! Shared domain vocabulary for Juggernaught OS.
//!
//! Everything here is plain data: no I/O, no async, no crypto. Every other
//! crate depends on this one, so keep it small and stable.

pub mod config;
pub mod risk;

use serde::{Deserialize, Serialize};

pub use config::Config;
pub use risk::Risk;

/// Kinds of asset the platform produces. Drives the sidebar library,
/// the brand-adherence gate and which tools may produce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Logo,
    LogoVector,
    IconSet,
    Texture,
    Image,
    SocialPost,
    Video,
    Audio,
    EmailSignature,
    BusinessCard,
    Poster,
    Pdf,
    WebPage,
}

/// Lifecycle of an asset. Only `Final` assets may be published or used on the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetStatus {
    /// Produced by a tool, not yet checked.
    Draft,
    /// Failed the brand / standards gate; report attached.
    Rejected,
    /// Passed the gate, waiting for the owner.
    Review,
    /// Approved and usable everywhere.
    Final,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("needs approval: {0}")]
    NeedsApproval(String),
    #[error("internal: {0}")]
    Internal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Time-ordered unique id (UUIDv7) as a string; sortable, safe to expose.
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

pub fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
