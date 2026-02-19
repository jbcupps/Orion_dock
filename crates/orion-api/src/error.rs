use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
    Unauthorized(String),
    Forbidden(String),
    Conflict(String),
    ServiceUnavailable(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(m) => write!(f, "{}", m),
            Self::BadRequest(m) => write!(f, "{}", m),
            Self::Internal(m) => write!(f, "{}", m),
            Self::Unauthorized(m) => write!(f, "{}", m),
            Self::Forbidden(m) => write!(f, "{}", m),
            Self::Conflict(m) => write!(f, "{}", m),
            Self::ServiceUnavailable(m) => write!(f, "{}", m),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::ServiceUnavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
        };
        (status, message).into_response()
    }
}
