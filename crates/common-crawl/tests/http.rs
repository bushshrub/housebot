use common_crawl::CommonCrawlClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn searches_latest_collection_and_parses_ndjson() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for body in [
            r#"[{"id":"CC-MAIN-test"}]"#,
            r#"{"url":"https://example.com/a","timestamp":"20250101000000","status":"200","mime":"text/html","length":"12","offset":"3","filename":"crawl/a.warc.gz"}
"#,
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let base = format!("http://{address}");
    let captures = CommonCrawlClient::with_endpoints(&base, format!("{base}/collinfo.json"))
        .search("example.com/*", None, Some("prefix"), 10)
        .await
        .unwrap();

    server.await.unwrap();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].url, "https://example.com/a");
}
