use jellyfin_naming::{NamingOptions, StubResolver, VideoResolver};

#[test]
fn stubs_official_matrix() {
    let options = NamingOptions::default();
    for (path, is_stub, stub_type) in [
        ("video.mkv", false, None),
        ("video.disc", true, None),
        ("video.dvd.disc", true, Some("dvd")),
        ("video.hddvd.disc", true, Some("hddvd")),
        ("video.bluray.disc", true, Some("bluray")),
        ("video.brrip.disc", true, Some("bluray")),
        ("video.bd25.disc", true, Some("bluray")),
        ("video.bd50.disc", true, Some("bluray")),
        ("video.vhs.disc", true, Some("vhs")),
        ("video.hdtv.disc", true, Some("tv")),
        ("video.pdtv.disc", true, Some("tv")),
        ("video.dsr.disc", true, Some("tv")),
        ("", false, Some("tv")),
    ] {
        let result = StubResolver::try_resolve_file(path, &options);
        assert_eq!(result.is_some(), is_stub, "path={path:?}");
        if is_stub {
            assert_eq!(result.flatten().as_deref(), stub_type, "path={path:?}");
        } else {
            assert_eq!(result, None, "path={path:?}");
        }
    }
}

#[test]
fn resolved_stub_has_clean_name() {
    let result = VideoResolver::resolve_file(
        Some("C:/Users/media/Desktop/Video Test/Movies/Oblivion/Oblivion.dvd.disc"),
        &NamingOptions::default(),
    )
    .unwrap();
    assert_eq!(result.name, "Oblivion");
    assert!(result.is_stub);
    assert_eq!(result.stub_type.as_deref(), Some("dvd"));
}
