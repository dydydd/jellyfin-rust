use std::sync::Arc;

use jellyfin_live_tv::listings::{
    GuideRefreshService, ListingProviderConfiguration, LiveTvConfiguration,
    MemoryListingsConfigurationStore, SchedulesDirectClient,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn guide_refresh_runs_the_complete_schedules_direct_chain() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server bind");
    let port = listener
        .local_addr()
        .expect("test server address")
        .port();
    let server = tokio::spawn(async move {
        let responses = [
            (
                "/token",
                r#"{"code":0,"token":"test-token"}"#.as_bytes(),
            ),
            (
                "/lineups/TEST-LINEUP",
                r#"{"map":[{"stationID":"S1","channel":"1"}],"stations":[]}"#.as_bytes(),
            ),
            (
                "/schedules",
                r#"[{"stationID":"S1","programs":[{"programID":"P1"}]}]"#.as_bytes(),
            ),
            (
                "/programs",
                r#"[{"programID":"P1","titles":[{"title120":"Test Program"}]}]"#.as_bytes(),
            ),
        ];
        for (expected_path, body) in responses {
            let (mut socket, _) = listener.accept().await.expect("test server accept");
            let mut buffer = [0_u8; 4096];
            let read = socket.read(&mut buffer).await.expect("request read");
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .expect("request path");
            assert!(path.starts_with(expected_path), "{path} vs {expected_path}");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("headers write");
            socket.write_all(body).await.expect("body write");
        }
    });

    let mut configuration = LiveTvConfiguration::default();
    configuration.guide_days = Some(1);
    configuration.listing_providers.push(ListingProviderConfiguration {
        provider_type: Some("SchedulesDirect".to_owned()),
        username: Some("test-user".to_owned()),
        password: Some("test-password".to_owned()),
        listings_id: Some("TEST-LINEUP".to_owned()),
        ..ListingProviderConfiguration::default()
    });
    let service = GuideRefreshService::new(
        Arc::new(MemoryListingsConfigurationStore::new(configuration)),
        SchedulesDirectClient::with_base_url(format!("http://127.0.0.1:{port}")),
    );

    let summary = service.refresh().await.expect("guide refresh");
    assert_eq!(summary.channels, 1);
    assert_eq!(summary.programs, 1);
    assert_eq!(summary.lineup_id, "TEST-LINEUP");

    server.await.expect("test server completion");
}
