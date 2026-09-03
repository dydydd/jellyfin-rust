use jellyfin_live_tv::channels::{LiveTvChannel, LiveTvChannelImageHelper};

#[test]
fn update_channel_image_if_needed_no_source_does_not_update() {
    let mut channel = LiveTvChannel::new("Test Channel");

    let updated =
        LiveTvChannelImageHelper::update_channel_image_if_needed(&mut channel, None, None);

    assert!(!updated);
    assert!(!channel.has_primary_image());
}

#[test]
fn update_channel_image_if_needed_with_url_applies_url() {
    let mut channel = LiveTvChannel::new("Test Channel");

    let updated = LiveTvChannelImageHelper::update_channel_image_if_needed(
        &mut channel,
        None,
        Some("https://example.com/icon.png"),
    );

    assert!(updated);
    assert!(channel.has_primary_image());
    assert_eq!(
        channel.primary_image_path(),
        Some("https://example.com/icon.png")
    );
}

#[test]
fn update_channel_image_if_needed_same_url_still_updates() {
    let mut channel = LiveTvChannel::new("Test Channel");
    LiveTvChannelImageHelper::update_channel_image_if_needed(
        &mut channel,
        None,
        Some("https://example.com/icon.png"),
    );

    let updated = LiveTvChannelImageHelper::update_channel_image_if_needed(
        &mut channel,
        None,
        Some("https://example.com/icon.png"),
    );

    assert!(updated);
    assert_eq!(
        channel.primary_image_path(),
        Some("https://example.com/icon.png")
    );
}

#[test]
fn local_path_takes_priority_over_provider_url() {
    let mut channel = LiveTvChannel::new("Test Channel");

    let updated = LiveTvChannelImageHelper::update_channel_image_if_needed(
        &mut channel,
        Some("/recordings/channel.png"),
        Some("https://example.com/icon.png"),
    );

    assert!(updated);
    assert_eq!(
        channel.primary_image_path(),
        Some("/recordings/channel.png")
    );
}

#[test]
fn blank_local_path_falls_back_to_provider_url_without_normalizing_it() {
    let mut channel = LiveTvChannel::new("Test Channel");

    let updated = LiveTvChannelImageHelper::update_channel_image_if_needed(
        &mut channel,
        Some(" \t "),
        Some(" https://example.com/icon.png "),
    );

    assert!(updated);
    assert_eq!(
        channel.primary_image_path(),
        Some(" https://example.com/icon.png ")
    );
}
