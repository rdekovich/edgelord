use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

// Local HTTP fixtures verify request paths and headers, not only deserialization.
async fn server(
    responses: Vec<(u16, &'static str)>,
) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let len = socket.read(&mut chunk).await.unwrap();
                assert!(len > 0);
                request.extend_from_slice(&chunk[..len]);
                if request.windows(4).any(|s| s == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(String::from_utf8(request).unwrap());
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}

fn credentials() -> Credentials {
    Credentials::new("test-id", &STANDARD.encode([7u8; 32])).unwrap()
}

#[tokio::test]
async fn separates_public_and_private_origins_and_signs_without_query() {
    let (public, public_task) = server(vec![(200, r#"{"markets":[]}"#)]).await;
    let (private, private_task) = server(vec![(200, r#"{"balances":[{"currency":"USD","currentBalance":0.1234567890123456789012345678,"buyingPower":0.01}]}"#)]).await;
    let client = UsClient::with_origins(public, private, Some(credentials())).unwrap();
    assert!(client.markets(1).await.unwrap().is_empty());
    let balance = client.balances().await.unwrap().remove(0);
    assert_eq!(
        balance.current_balance.to_string(),
        "0.1234567890123456789012345678"
    );
    let public = public_task.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(public.starts_with("get /v1/markets?limit=1&active=true&closed=false "));
    assert!(!public.contains("x-pm-"));
    let private = private_task.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(private.starts_with("get /v1/account/balances "));
    assert!(private.contains("x-pm-access-key: test-id"));
    assert!(private.contains("x-pm-signature:"));
}

#[tokio::test]
async fn follows_position_pages_and_preserves_negative_fractional_positions() {
    let (url, task) = server(vec![
        (200, r#"{"positions":{"a":{"netPosition":"-2","netPositionDecimal":"-2.5","cost":null,"realized":{"value":"0.0","currency":"USD"},"expired":false}},"nextCursor":"page 2","eof":false}"#),
        (200, r#"{"positions":{},"eof":true}"#),
    ]).await;
    let client = UsClient::with_origins(url.clone(), url, Some(credentials())).unwrap();
    assert_eq!(
        client.positions().await.unwrap()["a"]
            .quantity()
            .to_string(),
        "-2.5"
    );
    assert!(task.await.unwrap()[1].contains("cursor=page+2"));
}

#[tokio::test]
async fn pagination_cannot_silently_lose_inventory() {
    for pages in [
        vec![(200, r#"{"positions":{},"eof":false}"#)],
        vec![
            (200, r#"{"positions":{},"nextCursor":"x"}"#),
            (200, r#"{"positions":{},"nextCursor":"x"}"#),
        ],
    ] {
        let (url, task) = server(pages).await;
        let client = UsClient::with_origins(url.clone(), url, Some(credentials())).unwrap();
        assert!(matches!(
            client.positions().await,
            Err(UsError::Protocol(_))
        ));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn failures_are_not_empty_successes_or_sensitive_response_bodies() {
    for (status, body) in [
        (429, "secret server detail"),
        (200, r#"{}"#),
        (302, "redirect"),
    ] {
        let (url, task) = server(vec![(status, body)]).await;
        let client = UsClient::with_origins(url.clone(), url, Some(credentials())).unwrap();
        let error = client.balances().await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret server detail"));
        assert_eq!(task.await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn missing_credentials_and_path_injection_fail_before_network() {
    let client = UsClient::new(None).unwrap();
    assert!(matches!(
        client.balances().await,
        Err(UsError::Credentials(_))
    ));
    assert!(matches!(
        client.book("../account/balances?key=x").await,
        Err(UsError::Protocol(_))
    ));
}
