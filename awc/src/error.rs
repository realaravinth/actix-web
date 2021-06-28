//! HTTP client errors

pub use actix_http::client::{ConnectError, FreezeRequestError, InvalidUrl, SendRequestError};
pub use actix_http::error::PayloadError;
pub use actix_http::http::Error as HttpError;
pub use actix_http::ws::HandshakeError as WsHandshakeError;
pub use actix_http::ws::ProtocolError as WsProtocolError;

use serde_json::error::Error as JsonError;

use actix_http::http::{header::HeaderValue, StatusCode};
use derive_more::{Display, From};

/// Websocket client error
#[derive(Debug, Display, From)]
pub enum WsClientError {
    /// Invalid response status
    #[display(fmt = "Invalid response status")]
    InvalidResponseStatus(StatusCode),

    /// Invalid upgrade header
    #[display(fmt = "Invalid upgrade header")]
    InvalidUpgradeHeader,

    /// Invalid connection header
    #[display(fmt = "Invalid connection header")]
    InvalidConnectionHeader(HeaderValue),

    /// Missing Connection header
    #[display(fmt = "Missing Connection header")]
    MissingConnectionHeader,

    /// Missing Sec-Websocket-Accept header
    #[display(fmt = "Missing Sec-Websocket-Accept header")]
    MissingWebSocketAcceptHeader,

    /// Invalid challenge response
    #[display(fmt = "Invalid challenge response")]
    InvalidChallengeResponse([u8; 28], HeaderValue),

    /// Protocol error
    #[display(fmt = "{}", _0)]
    Protocol(WsProtocolError),

    /// Send request error
    #[display(fmt = "{}", _0)]
    SendRequest(SendRequestError),
}

impl std::error::Error for WsClientError {}

impl From<InvalidUrl> for WsClientError {
    fn from(err: InvalidUrl) -> Self {
        WsClientError::SendRequest(err.into())
    }
}

impl From<HttpError> for WsClientError {
    fn from(err: HttpError) -> Self {
        WsClientError::SendRequest(err.into())
    }
}

/// A set of errors that can occur during parsing json payloads
#[derive(Debug, Display, From)]
pub enum JsonPayloadError {
    /// Content type error
    #[display(fmt = "Content type error")]
    ContentType,
    /// Deserialize error
    #[display(fmt = "Json deserialize error: {}", _0)]
    Deserialize(JsonError),
    /// Payload error
    #[display(fmt = "Error that occur during reading payload: {}", _0)]
    Payload(PayloadError),
}

impl std::error::Error for JsonPayloadError {}
