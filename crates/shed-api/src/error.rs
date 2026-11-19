//! Errors, as codes the apps turn into text.

use serde::{Deserialize, Serialize};
use typeshare::typeshare;

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Not signed in, or the token is no longer valid.
    Unauthorized,
    /// Signed in, but this is not allowed (a workspace in view-only mode).
    Forbidden,
    /// The workspace, bench, tab, pane or file is not there.
    NotFound,
    /// The workspace is not connected to the nexus.
    Offline,
    /// The request does not make sense (a bad argument).
    Invalid,
    /// The workspace or nexus does not know this call (another version).
    Unsupported,
    /// Too many attempts; try again later.
    RateLimited,
    /// The wrong one-time code.
    WrongCode,
    /// It took too long.
    Timeout,
    /// Something failed on the way; `message` says what.
    Failed,
}

#[typeshare]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ErrorCode,
    /// For people, in English; the apps show their own text for the code and
    /// this as detail.
    #[serde(default)]
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError { code, message: message.into() }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}
