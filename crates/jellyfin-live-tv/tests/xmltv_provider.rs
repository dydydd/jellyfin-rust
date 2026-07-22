use std::path::PathBuf;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use jellyfin_live_tv::listings::{
    ProgramFlag, XMLTV_ETAG_PREFIX, XmlTvListingsProvider, XmlTvProviderError, XmlTvProviderInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn window() -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    (
        Utc.with_ymd_and_hms(2022, 11, 4, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2022, 11, 5, 0, 0, 0).unwrap(),
    )
}

async fn serve_once(body: &'static [u8], status: u16, delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).await;
        tokio::time::sleep(delay).await;
        let reason = if status == 200 { "OK" } else { "Error" };
        let header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
    });
    format!("http://{address}/guide.xml")
}

async fn single(path: String) -> jellyfin_live_tv::listings::ProgramInfo {
    let provider = XmlTvListingsProvider::new().unwrap();
    let mut info = XmlTvProviderInfo {
        path,
        ..XmlTvProviderInfo::default()
    };
    info.options.preferred_language = Some("en".to_owned());
    info.options.sports_categories = vec!["sports".to_owned()];
    let (start, end) = window();
    provider
        .get_programs(&info, "3297", start, end)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

fn assert_no_title(program: &jellyfin_live_tv::listings::ProgramInfo) {
    assert_eq!(None, program.name);
    assert_eq!(None, program.series_id);
    assert_eq!(None, program.episode_title);
    assert!(program.flags.contains(ProgramFlag::Sports));
    assert_eq!(Some(true), program.has_image);
    assert_eq!(
        Some("https://domain.tld/image.png"),
        program.image_url.as_deref()
    );
    assert_eq!(Some("3297"), program.channel_id.as_deref());
    assert!(
        program
            .etag
            .as_deref()
            .unwrap()
            .starts_with(XMLTV_ETAG_PREFIX)
    );
}

#[tokio::test]
async fn no_title_local_path() {
    assert_no_title(&single(fixture("notitle.xml")).await);
}

#[tokio::test]
async fn no_title_http_path() {
    let url = serve_once(include_bytes!("fixtures/notitle.xml"), 200, Duration::ZERO).await;
    assert_no_title(&single(url).await);
}

#[tokio::test]
async fn empty_categories_local_path() {
    let program = single(fixture("emptycategory.xml")).await;
    assert!(program.genres.iter().all(|genre| !genre.is_empty()));
}

#[tokio::test]
async fn empty_categories_http_path() {
    let url = serve_once(
        include_bytes!("fixtures/emptycategory.xml"),
        200,
        Duration::ZERO,
    )
    .await;
    let program = single(url).await;
    assert!(program.genres.iter().all(|genre| !genre.is_empty()));
}

#[tokio::test]
async fn etag_same_content_is_stable() {
    assert_eq!(
        single(fixture("etag-base.xml")).await.etag,
        single(fixture("etag-base.xml")).await.etag
    );
}

macro_rules! changed_case {
    ($name:ident, $fixture:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_ne!(
                single(fixture("etag-base.xml")).await.etag,
                single(fixture($fixture)).await.etag
            );
        }
    };
}
changed_case!(etag_title_change, "etag-title-change.xml");
changed_case!(etag_description_change, "etag-description-change.xml");
changed_case!(etag_icon_change, "etag-icon-change.xml");
changed_case!(etag_category_change, "etag-category-change.xml");
changed_case!(etag_progid_change, "etag-progid-change.xml");

macro_rules! equivalent_case {
    ($name:ident, $fixture:literal) => {
        #[tokio::test]
        async fn $name() {
            assert_eq!(
                single(fixture("etag-base.xml")).await.etag,
                single(fixture($fixture)).await.etag
            );
        }
    };
}
equivalent_case!(etag_reordered_equivalent, "etag-reordered.xml");
equivalent_case!(etag_unknown_field_equivalent, "etag-unknown-field.xml");

#[tokio::test]
async fn acquisition_errors_are_explicit() {
    let info = |path| XmlTvProviderInfo {
        path,
        ..XmlTvProviderInfo::default()
    };
    let (start, end) = window();
    let status = serve_once(b"", 503, Duration::ZERO).await;
    assert!(matches!(
        XmlTvListingsProvider::new()
            .unwrap()
            .get_programs(&info(status), "3297", start, end)
            .await,
        Err(XmlTvProviderError::HttpStatus { status: 503, .. })
    ));
    let large = serve_once(&[b'x'; 33], 200, Duration::ZERO).await;
    assert!(matches!(
        XmlTvListingsProvider::with_limits(Duration::from_secs(1), 32)
            .unwrap()
            .get_programs(&info(large), "3297", start, end)
            .await,
        Err(XmlTvProviderError::TooLarge { limit: 32 })
    ));
    let slow = serve_once(b"<tv/>", 200, Duration::from_millis(100)).await;
    assert!(matches!(
        XmlTvListingsProvider::with_limits(Duration::from_millis(10), 1024)
            .unwrap()
            .get_programs(&info(slow), "3297", start, end)
            .await,
        Err(XmlTvProviderError::Timeout(_))
    ));
    let invalid = serve_once(b"<tv>", 200, Duration::ZERO).await;
    assert!(matches!(
        XmlTvListingsProvider::new()
            .unwrap()
            .get_programs(&info(invalid), "3297", start, end)
            .await,
        Err(XmlTvProviderError::Parse(_))
    ));
}

#[tokio::test]
async fn channel_and_lineup_queries_map_xmltv_channels() {
    let path = std::env::temp_dir().join(format!("jellyfin-xmltv-{}.xml", std::process::id()));
    std::fs::write(&path, r#"<tv><channel id="bbc.one"><display-name>1</display-name><display-name>BBC One</display-name><icon src="https://example/icon.png"/></channel></tv>"#).unwrap();
    let info = XmlTvProviderInfo {
        path: path.to_string_lossy().into_owned(),
        ..XmlTvProviderInfo::default()
    };
    let provider = XmlTvListingsProvider::new().unwrap();
    let channels = provider.get_channels(&info).await.unwrap();
    assert_eq!("bbc.one", channels[0].id);
    assert_eq!("1", channels[0].number);
    assert_eq!(
        [("bbc.one".to_owned(), "1".to_owned())],
        provider.get_lineups(&info).await.unwrap().as_slice()
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn provider_metadata_and_validation_match_contract() {
    let provider = XmlTvListingsProvider::new().unwrap();
    assert_eq!("XmlTV", provider.name());
    assert_eq!("xmltv", provider.provider_type());
    assert!(matches!(
        provider.validate(&XmlTvProviderInfo::default()),
        Err(XmlTvProviderError::EmptyPath)
    ));
    assert!(matches!(
        provider.validate(&XmlTvProviderInfo {
            path: "ftp://example.test/guide.xml".to_owned(),
            ..XmlTvProviderInfo::default()
        }),
        Err(XmlTvProviderError::UnsupportedUrl(_))
    ));
    assert!(
        provider
            .validate(&XmlTvProviderInfo {
                path: fixture("notitle.xml"),
                ..XmlTvProviderInfo::default()
            })
            .is_ok()
    );
}
