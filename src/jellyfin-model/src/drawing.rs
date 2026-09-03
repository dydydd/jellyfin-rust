use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum ImageOrientation {
    #[default]
    TopLeft = 1,
    TopRight = 2,
    BottomRight = 3,
    BottomLeft = 4,
    LeftTop = 5,
    RightTop = 6,
    RightBottom = 7,
    LeftBottom = 8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum ImageFormat {
    Bmp = 0,
    Gif = 1,
    Jpg = 2,
    Png = 3,
    Webp = 4,
    Svg = 5,
}

impl ImageFormat {
    pub const ALL: [Self; 6] = [
        Self::Bmp,
        Self::Gif,
        Self::Jpg,
        Self::Png,
        Self::Webp,
        Self::Svg,
    ];

    #[must_use]
    pub const fn get_mime_type(self) -> &'static str {
        self.mime_type()
    }

    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Bmp => "image/bmp",
            Self::Gif => "image/gif",
            Self::Jpg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Svg => "image/svg+xml",
        }
    }

    #[must_use]
    pub const fn get_extension(self) -> &'static str {
        self.extension()
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Bmp => ".bmp",
            Self::Gif => ".gif",
            Self::Jpg => ".jpg",
            Self::Png => ".png",
            Self::Webp => ".webp",
            Self::Svg => ".svg",
        }
    }
}

impl TryFrom<i32> for ImageFormat {
    type Error = InvalidImageFormat;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Bmp),
            1 => Ok(Self::Gif),
            2 => Ok(Self::Jpg),
            3 => Ok(Self::Png),
            4 => Ok(Self::Webp),
            5 => Ok(Self::Svg),
            _ => Err(InvalidImageFormat(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidImageFormat(pub i32);

impl fmt::Display for InvalidImageFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid image format value: {}", self.0)
    }
}

impl Error for InvalidImageFormat {}
