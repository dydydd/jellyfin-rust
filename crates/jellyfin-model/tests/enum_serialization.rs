use jellyfin_model::{
    AudioSpatialFormat, ChannelType, CollectionType, ItemFields, MediaStreamProtocol, VideoRange,
    VideoRangeType,
};
use serde_json::json;

#[test]
fn enum_member_overrides_match_official_wire_values() {
    assert_eq!(serde_json::to_value(ChannelType::TV).unwrap(), "TV");
    assert_eq!(serde_json::to_value(ItemFields::IsHD).unwrap(), "IsHD");
    assert_eq!(
        serde_json::to_value(MediaStreamProtocol::Http).unwrap(),
        "http"
    );
    assert_eq!(
        serde_json::to_value(AudioSpatialFormat::DtsX).unwrap(),
        "DTSX"
    );
    assert_eq!(serde_json::to_value(VideoRange::Sdr).unwrap(), "SDR");
    assert_eq!(
        serde_json::to_value(VideoRangeType::DoviWithHdr10).unwrap(),
        "DOVIWithHDR10"
    );
    assert_eq!(
        serde_json::to_value(CollectionType::Movies).unwrap(),
        "movies"
    );
    assert_eq!(
        serde_json::from_value::<ChannelType>(json!("TV")).unwrap(),
        ChannelType::TV
    );
}
