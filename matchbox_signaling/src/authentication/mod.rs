use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::STANDARD, Engine};
use hyper::StatusCode;

/// Decodes the two parts of basic auth using the colon
pub(crate) fn decode(input: &str) -> Result<(String, Option<String>), Response> {
    // Decode from base64 into a string
    let decoded = STANDARD.decode(input).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "`Authorization` header is not valid base64",
        )
            .into_response()
    })?;
    let decoded = String::from_utf8(decoded).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "`Authorization` header contains invalid characters",
        )
            .into_response()
    })?;

    // Return depending on if password is present
    Ok(
        if let Some((username, password)) = decoded.split_once(':') {
            (username.to_string(), Some(password.to_string()))
        } else {
            (decoded.to_string(), None)
        },
    )
}
