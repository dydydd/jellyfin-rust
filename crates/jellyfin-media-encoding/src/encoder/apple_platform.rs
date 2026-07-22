use std::error::Error;
use std::fmt;

const AV1_DECODE_BLACKLIST: [&str; 2] = ["M1", "M2"];

/// Operating-system family supplied by the host adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

/// Processor architecture supplied by the host adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuArchitecture {
    Arm64,
    Arm,
    X86_64,
    X86,
    Other,
}

/// Boundary for platform detection and Apple `sysctl` access.
///
/// Keeping all host reads behind this interface makes Apple feature decisions
/// deterministic on every test host.
pub trait ApplePlatformCapability {
    fn platform(&self) -> Platform;

    fn architecture(&self) -> CpuArchitecture;

    /// Returns the raw bytes reported by `sysctlbyname`, including any trailing
    /// null terminator.
    ///
    /// # Errors
    ///
    /// Returns a textual adapter error when the host value cannot be read.
    fn sysctl_value(&self, name: &str) -> Result<Vec<u8>, String>;
}

/// Failure to read and normalize an Apple platform value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplePlatformError {
    UnsupportedPlatform(Platform),
    Sysctl(String),
    EmptyValue(String),
    InvalidUtf8(String),
    EmbeddedNull(String),
}

impl fmt::Display for ApplePlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform(platform) => {
                write!(formatter, "Apple sysctl is unavailable on {platform:?}")
            }
            Self::Sysctl(error) => write!(formatter, "failed to read Apple sysctl: {error}"),
            Self::EmptyValue(name) => write!(formatter, "Apple sysctl {name} returned no value"),
            Self::InvalidUtf8(name) => {
                write!(formatter, "Apple sysctl {name} returned invalid UTF-8")
            }
            Self::EmbeddedNull(name) => {
                write!(formatter, "Apple sysctl {name} contains an embedded null")
            }
        }
    }
}

impl Error for ApplePlatformError {}

/// Reads a UTF-8 Apple `sysctl` string and removes all trailing null bytes.
///
/// # Errors
///
/// Returns an unsupported-platform, adapter, empty-value, invalid-UTF-8, or
/// embedded-null error.
pub fn get_sysctl_value<C: ApplePlatformCapability + ?Sized>(
    capability: &C,
    name: &str,
) -> Result<String, ApplePlatformError> {
    if capability.platform() != Platform::MacOs {
        return Err(ApplePlatformError::UnsupportedPlatform(
            capability.platform(),
        ));
    }
    let mut bytes = capability
        .sysctl_value(name)
        .map_err(ApplePlatformError::Sysctl)?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(ApplePlatformError::EmptyValue(name.to_owned()));
    }
    let value =
        String::from_utf8(bytes).map_err(|_| ApplePlatformError::InvalidUtf8(name.to_owned()))?;
    if value.contains('\0') {
        return Err(ApplePlatformError::EmbeddedNull(name.to_owned()));
    }
    Ok(value)
}

/// Returns whether the supplied host is macOS running natively on Arm64.
#[must_use]
pub fn is_apple_silicon<C: ApplePlatformCapability + ?Sized>(capability: &C) -> bool {
    capability.platform() == Platform::MacOs && capability.architecture() == CpuArchitecture::Arm64
}

/// Applies Jellyfin's Apple Silicon `VideoToolbox` AV1 decoder rules.
///
/// M1 and M2 CPU classes are blacklisted. Platform lookup failures are treated
/// as unsupported, matching Jellyfin's conservative runtime behavior.
#[must_use]
pub fn has_apple_av1_hardware_acceleration<C: ApplePlatformCapability + ?Sized>(
    capability: &C,
) -> bool {
    if !is_apple_silicon(capability) {
        return false;
    }
    let Ok(cpu_brand) = get_sysctl_value(capability, "machdep.cpu.brand_string") else {
        return false;
    };
    let cpu_brand = cpu_brand.to_ascii_lowercase();
    AV1_DECODE_BLACKLIST
        .iter()
        .all(|class| !cpu_brand.contains(&class.to_ascii_lowercase()))
}
