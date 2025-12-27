use thiserror::Error;

#[derive(Debug, Error)]
pub enum KiroError {
    #[error("authentication error: {0}")]
    Auth(#[from] AuthError),
    #[error("API error: {0}")]
    Api(#[from] ApiError),
    #[error("network error: {0}")]
    Network(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("not authenticated")]
    NotAuthenticated,
    #[error("token expired")]
    TokenExpired,
    #[error("refresh failed: {0}")]
    RefreshFailed(String),
    #[error("device flow failed: {0}")]
    DeviceFlowFailed(String),
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("throttling: {message}")]
    Throttling { message: String, retry_after: Option<u64> },
    #[error("validation error: {message}")]
    Validation { message: String },
    #[error("access denied: {message}")]
    AccessDenied { message: String },
    #[error("internal server error")]
    InternalServerError,
    #[error("service unavailable")]
    ServiceUnavailable,
}
