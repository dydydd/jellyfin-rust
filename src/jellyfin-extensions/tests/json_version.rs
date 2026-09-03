use jellyfin_extensions::json::JsonVersion;

#[test]
fn deserialize_version_successfully() {
    assert_eq!(
        serde_json::from_str::<JsonVersion>(r#""1.025.222""#).unwrap(),
        JsonVersion::new(1, 25).with_build(222)
    );
}

#[test]
fn serialize_version_successfully() {
    assert_eq!(
        serde_json::to_string(&JsonVersion::new(1, 9).with_build(59)).unwrap(),
        r#""1.9.59""#
    );
}
