use crate::signaling_server::{handlers::WsUpgradeMeta, Authentication};
use axum::response::{IntoResponse, Response};
use hyper::{header::AUTHORIZATION, StatusCode};

#[derive(Default, Debug, Clone)]
pub struct AuthKey(pub String);

/// No authentication is used, all connections are granted.
pub struct NoAuthentication;
impl Authentication for NoAuthentication {
    fn verify(_meta: WsUpgradeMeta, _auth_key: AuthKey) -> Result<bool, Response> {
        Ok(true)
    }
}

/// Basic authentication requires an username and password in base64 (padded) delivered in the
/// header on connect.
pub struct BasicAuthentication;
impl Authentication for BasicAuthentication {
    fn verify(meta: WsUpgradeMeta, auth_key: AuthKey) -> Result<bool, Response> {
        let authorization = meta
            .headers
            .get(AUTHORIZATION)
            .ok_or((StatusCode::BAD_REQUEST, "`Authorization` header is missing").into_response())?
            .to_str()
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "`Authorization` header contains invalid characters",
                )
                    .into_response()
            })?;

        // Check that its well-formed basic auth then decode
        let split = authorization.split_once(' ');
        let authenticated = match split {
            Some((name, contents)) if name == "Basic" => {
                // Return depending on if password is present
                if contents == auth_key.0 {
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            _ => Err((
                StatusCode::BAD_REQUEST,
                "`Authorization` header must be for basic authentication",
            )
                .into_response())?,
        }?;

        Ok(authenticated)
    }
}
