#[cfg(test)]
mod test {
    use matchbox_socket::{ChannelConfig, Error, WebRtcSocketBuilder};

    #[futures_test::test]
    async fn unreachable_server() {
        // .invalid is a reserved tld for testing and documentation
        let (_socket, fut) = WebRtcSocketBuilder::new("wss://example.invalid")
            .add_channel(ChannelConfig::unreliable())
            .build();

        let result = fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::ConnectionFailed { .. }
        ));
    }

    #[futures_test::test]
    async fn test_signaling_attempts() {
        let (_socket, loop_fut) = WebRtcSocketBuilder::new("wss://example.invalid/")
            .reconnect_attempts(Some(3))
            .add_channel(ChannelConfig::reliable())
            .build();

        let result = loop_fut.await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            Error::ConnectionFailed { .. },
        ));
    }
}
