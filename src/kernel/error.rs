//! Explicit error types. Domain: kernel failures. Bound: no heap in mapping
//! besides `Module`/`Config` strings. Criticality C2.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("parse")]
    Parse,
    #[error("timeout")]
    Timeout,
    #[error("body too large")]
    BodyTooLarge,
    #[error("json depth")]
    JsonDepth,
    #[error("json invalid")]
    JsonInvalid,
    #[error("no rule")]
    NoRule,
    #[error("module {0}")]
    Module(Box<str>),
    #[error("capacity")]
    Capacity,
    #[error("forbidden")]
    Forbidden,
    #[error("unauthorized")]
    Unauthorized,
    #[error("config: {0}")]
    Config(Box<str>),
    #[error("rules: {0}")]
    Rules(#[from] crate::rules::RuleError),
}

impl ServeError {
    /// HTTP status for this error. Invariant: never 2xx.
    pub fn status(&self) -> u16 {
        match self {
            ServeError::Io(_) => 500,
            ServeError::Parse => 400,
            ServeError::Timeout => 408,
            ServeError::BodyTooLarge => 413,
            ServeError::JsonDepth | ServeError::JsonInvalid => 400,
            ServeError::NoRule => 404,
            ServeError::Module(_) => 500,
            ServeError::Capacity => 503,
            ServeError::Forbidden => 403,
            ServeError::Unauthorized => 401,
            ServeError::Config(_) => 500,
            ServeError::Rules(_) => 500,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtomError {
    #[error("unknown atom {0}")]
    Unknown(Box<str>),
    #[error("not found")]
    NotFound,
    #[error("conflict")]
    Conflict,
    #[error("pure atom cannot actuate")]
    PureActuate,
    #[error("invalid input: {0}")]
    Input(Box<str>),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(Box<str>),
    #[error("bound")]
    Bound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_too_large_is_413() {
        assert_eq!(ServeError::BodyTooLarge.status(), 413);
    }

    #[test]
    fn json_depth_is_400() {
        assert_eq!(ServeError::JsonDepth.status(), 400);
    }

    #[test]
    fn capacity_is_503() {
        assert_eq!(ServeError::Capacity.status(), 503);
    }
}
