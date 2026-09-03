use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, Cursor};
use std::path::Path;
use std::process::{Command, ExitStatus};

use crate::KeyframeData;

const TICKS_PER_SECOND: u128 = 10_000_000;

#[derive(Clone, Copy)]
struct Decimal {
    numerator: u128,
    scale: u128,
}

impl Decimal {
    const fn is_positive(self) -> bool {
        self.numerator > 0
    }

    fn to_ticks(self) -> Option<i64> {
        let scaled = self.numerator.checked_mul(TICKS_PER_SECOND)?;
        let quotient = scaled / self.scale;
        i64::try_from(quotient).ok()
    }
}

/// Failure while starting or running ffprobe, or reading its output.
#[derive(Debug)]
pub enum FfprobeError {
    Io(io::Error),
    Failed { status: ExitStatus, stderr: String },
}

impl fmt::Display for FfprobeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "ffprobe I/O failed: {error}"),
            Self::Failed { status, stderr } => {
                write!(formatter, "ffprobe exited with {status}: {}", stderr.trim())
            }
        }
    }
}

impl Error for FfprobeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Failed { .. } => None,
        }
    }
}

impl From<io::Error> for FfprobeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Runs ffprobe and extracts video keyframe timestamps from its CSV output.
///
/// # Errors
///
/// Returns [`FfprobeError::Io`] if the executable cannot be launched or its
/// output cannot be read. Returns [`FfprobeError::Failed`] for a non-zero exit.
pub fn extract_keyframes(
    ffprobe_path: impl AsRef<OsStr>,
    file_path: impl AsRef<Path>,
) -> Result<KeyframeData, FfprobeError> {
    let output = Command::new(ffprobe_path)
        .args([
            "-fflags",
            "+genpts",
            "-v",
            "error",
            "-skip_frame",
            "nokey",
            "-show_entries",
            "format=duration",
            "-show_entries",
            "stream=duration",
            "-show_entries",
            "packet=pts_time,flags",
            "-select_streams",
            "v",
            "-of",
            "csv",
        ])
        .arg(file_path.as_ref())
        .output()?;

    if !output.status.success() {
        return Err(FfprobeError::Failed {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    parse_ffprobe_output(Cursor::new(output.stdout)).map_err(FfprobeError::Io)
}

/// Parses ffprobe CSV output produced by [`extract_keyframes`].
///
/// Stream duration is preferred over format duration when it is positive.
/// Unknown records and malformed values are ignored, as in Jellyfin's parser.
///
/// # Errors
///
/// Returns an I/O error if a line cannot be read from `reader`.
pub fn parse_ffprobe_output(reader: impl BufRead) -> io::Result<KeyframeData> {
    let mut keyframes = Vec::new();
    let mut stream_duration = None;
    let mut format_duration = None;

    for line in reader.lines() {
        let line = line?;
        let mut fields = line.splitn(3, ',');
        let Some(line_type) = fields.next() else {
            continue;
        };
        let Some(value) = fields.next() else {
            continue;
        };

        if line_type.eq_ignore_ascii_case("packet") {
            let Some(flags) = fields.next() else {
                continue;
            };
            if flags.starts_with("K_")
                && let Some(ticks) = parse_decimal(value).and_then(Decimal::to_ticks)
            {
                keyframes.push(ticks);
            }
        } else if line_type.eq_ignore_ascii_case("stream") {
            if let Some(duration) = parse_decimal(value) {
                stream_duration = Some(duration);
            }
        } else if line_type.eq_ignore_ascii_case("format")
            && let Some(duration) = parse_decimal(value)
        {
            format_duration = Some(duration);
        }
    }

    let duration = stream_duration
        .filter(|duration| duration.is_positive())
        .or(format_duration);
    Ok(KeyframeData::new(
        duration.and_then(Decimal::to_ticks).unwrap_or_default(),
        keyframes,
    ))
}

fn parse_decimal(value: &str) -> Option<Decimal> {
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    if fractional.contains('.')
        || (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().ok()?
    };
    let fractional_value = if fractional.is_empty() {
        0
    } else {
        fractional.parse::<u128>().ok()?
    };
    let fractional_digits = u32::try_from(fractional.len()).ok()?;
    let scale = 10_u128.checked_pow(fractional_digits)?;
    let numerator = whole.checked_mul(scale)?.checked_add(fractional_value)?;
    Some(Decimal { numerator, scale })
}
