/// Minimal Live TV channel state needed by the image source helper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveTvChannel {
    pub name: String,
    primary_image_path: Option<String>,
}

impl LiveTvChannel {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            primary_image_path: None,
        }
    }

    #[must_use]
    pub const fn has_primary_image(&self) -> bool {
        self.primary_image_path.is_some()
    }

    #[must_use]
    pub fn primary_image_path(&self) -> Option<&str> {
        self.primary_image_path.as_deref()
    }
}

/// Keeps a channel's primary image source synchronized with guide data.
pub struct LiveTvChannelImageHelper;

impl LiveTvChannelImageHelper {
    /// Applies the non-blank local image path when present, otherwise the
    /// non-blank provider image URL. The image metadata is set on every call,
    /// including when the selected source is unchanged.
    pub fn update_channel_image_if_needed(
        channel: &mut LiveTvChannel,
        image_path: Option<&str>,
        image_url: Option<&str>,
    ) -> bool {
        let source = image_path
            .filter(|source| !source.trim().is_empty())
            .or_else(|| image_url.filter(|source| !source.trim().is_empty()));
        let Some(source) = source else {
            return false;
        };

        channel.primary_image_path = Some(source.to_owned());
        true
    }
}
