use nodeharbor_agent::Agent;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn oversized_chunked_enrollment_response_is_rejected_without_saving_credentials() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut byte = [0];
            connection.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        let headers = String::from_utf8(request).unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        connection.read_exact(&mut vec![0; length]).await.unwrap();
        connection.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
        let body = json!({
            "deviceId":"9511182e-9c48-4d20-a15b-1da8bb441386",
            "token":"x".repeat(64),
            "padding":"x".repeat(2 * 1024 * 1024)
        })
        .to_string();
        for chunk in body.as_bytes().chunks(8192) {
            let frame = format!("{:x}\r\n", chunk.len());
            if connection.write_all(frame.as_bytes()).await.is_err()
                || connection.write_all(chunk).await.is_err()
                || connection.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        let _ = connection.write_all(b"0\r\n\r\n").await;
    });
    let directory = tempfile::tempdir().unwrap();
    let agent = Agent::open(directory.path()).unwrap();
    let result = agent.enroll(&url, "test-enrollment-code").await;
    assert!(
        result.is_err(),
        "Omitting Content-Length must not bypass the response limit"
    );
    assert!(result.err().unwrap().to_string().contains("too large"));
    assert!(agent.store.load().unwrap().device_token.is_none());
    server.abort();
}
