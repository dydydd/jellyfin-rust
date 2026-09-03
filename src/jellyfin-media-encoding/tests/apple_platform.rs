use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use jellyfin_media_encoding::encoder::{
    ApplePlatformCapability, ApplePlatformError, CpuArchitecture, Platform, get_sysctl_value,
    has_apple_av1_hardware_acceleration, is_apple_silicon,
};

#[derive(Debug)]
struct FixtureCapability {
    platform: Platform,
    architecture: CpuArchitecture,
    sysctl_result: Result<Vec<u8>, String>,
    sysctl_names: Mutex<Vec<String>>,
    sysctl_calls: AtomicUsize,
}

impl FixtureCapability {
    fn new(platform: Platform, architecture: CpuArchitecture, value: &[u8]) -> Self {
        Self {
            platform,
            architecture,
            sysctl_result: Ok(value.to_vec()),
            sysctl_names: Mutex::new(Vec::new()),
            sysctl_calls: AtomicUsize::new(0),
        }
    }

    fn failing(error: &str) -> Self {
        Self {
            platform: Platform::MacOs,
            architecture: CpuArchitecture::Arm64,
            sysctl_result: Err(error.to_owned()),
            sysctl_names: Mutex::new(Vec::new()),
            sysctl_calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.sysctl_calls.load(Ordering::Relaxed)
    }
}

impl ApplePlatformCapability for FixtureCapability {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn architecture(&self) -> CpuArchitecture {
        self.architecture
    }

    fn sysctl_value(&self, name: &str) -> Result<Vec<u8>, String> {
        self.sysctl_calls.fetch_add(1, Ordering::Relaxed);
        self.sysctl_names.lock().unwrap().push(name.to_owned());
        self.sysctl_result.clone()
    }
}

// Official GetSysctlValue_CpuBrand_NotEmpty: one macOS-only Fact, made host-independent.
#[test]
fn get_sysctl_value_cpu_brand_not_empty() {
    let capability =
        FixtureCapability::new(Platform::MacOs, CpuArchitecture::Arm64, b"Apple M3 Max\0");

    let value = get_sysctl_value(&capability, "machdep.cpu.brand_string").unwrap();

    assert!(!value.is_empty());
    assert!(!value.contains('\0'));
    assert_eq!(value, "Apple M3 Max");
    assert_eq!(capability.call_count(), 1);
    assert_eq!(
        capability.sysctl_names.lock().unwrap().as_slice(),
        ["machdep.cpu.brand_string"]
    );
}

#[test]
fn apple_silicon_requires_macos_and_arm64() {
    for (platform, architecture, expected) in [
        (Platform::MacOs, CpuArchitecture::Arm64, true),
        (Platform::MacOs, CpuArchitecture::Arm, false),
        (Platform::MacOs, CpuArchitecture::X86_64, false),
        (Platform::MacOs, CpuArchitecture::X86, false),
        (Platform::Linux, CpuArchitecture::Arm64, false),
        (Platform::Windows, CpuArchitecture::Arm64, false),
        (Platform::Other, CpuArchitecture::Other, false),
    ] {
        let capability = FixtureCapability::new(platform, architecture, b"Apple M3 Max\0");
        assert_eq!(is_apple_silicon(&capability), expected);
        assert_eq!(capability.call_count(), 0);
    }
}

#[test]
fn av1_hardware_acceleration_matches_apple_cpu_class_matrix() {
    for (brand, expected) in [
        ("Apple M1", false),
        ("Apple M1 Max", false),
        ("Apple M2 Pro", false),
        ("apple m2 ultra", false),
        ("Apple M3", true),
        ("Apple M4 Max", true),
        ("Apple Virtual Platform", true),
    ] {
        let mut value = brand.as_bytes().to_vec();
        value.push(0);
        let capability = FixtureCapability::new(Platform::MacOs, CpuArchitecture::Arm64, &value);
        assert_eq!(has_apple_av1_hardware_acceleration(&capability), expected);
        assert_eq!(capability.call_count(), 1);
    }
}

#[test]
fn non_apple_silicon_rejects_av1_without_reading_sysctl() {
    for (platform, architecture) in [
        (Platform::Linux, CpuArchitecture::Arm64),
        (Platform::MacOs, CpuArchitecture::X86_64),
        (Platform::Windows, CpuArchitecture::X86_64),
    ] {
        let capability = FixtureCapability::new(platform, architecture, b"Apple M4 Max\0");
        assert!(!has_apple_av1_hardware_acceleration(&capability));
        assert_eq!(capability.call_count(), 0);
    }
}

#[test]
fn sysctl_value_rejects_malformed_and_unavailable_values() {
    let cases = [
        (
            FixtureCapability::new(Platform::MacOs, CpuArchitecture::Arm64, b"\0\0"),
            "empty",
        ),
        (
            FixtureCapability::new(Platform::MacOs, CpuArchitecture::Arm64, &[0xff, 0xfe, 0]),
            "utf8",
        ),
        (
            FixtureCapability::new(Platform::MacOs, CpuArchitecture::Arm64, b"Apple\0M3\0"),
            "null",
        ),
        (FixtureCapability::failing("sysctl error 2"), "sysctl"),
    ];

    for (capability, expected) in cases {
        let error = get_sysctl_value(&capability, "machdep.cpu.brand_string").unwrap_err();
        assert!(match expected {
            "empty" => matches!(error, ApplePlatformError::EmptyValue(_)),
            "utf8" => matches!(error, ApplePlatformError::InvalidUtf8(_)),
            "null" => matches!(error, ApplePlatformError::EmbeddedNull(_)),
            "sysctl" => matches!(error, ApplePlatformError::Sysctl(_)),
            _ => false,
        });
    }
}

#[test]
fn sysctl_value_rejects_non_macos_without_host_access() {
    let capability = FixtureCapability::new(Platform::Linux, CpuArchitecture::Arm64, b"Apple M3\0");

    let error = get_sysctl_value(&capability, "machdep.cpu.brand_string").unwrap_err();

    assert_eq!(
        error,
        ApplePlatformError::UnsupportedPlatform(Platform::Linux)
    );
    assert_eq!(capability.call_count(), 0);
}

#[test]
fn sysctl_value_removes_every_trailing_null_terminator() {
    let capability = FixtureCapability::new(
        Platform::MacOs,
        CpuArchitecture::Arm64,
        b"Apple M3 Pro\0\0\0",
    );

    assert_eq!(
        get_sysctl_value(&capability, "machdep.cpu.brand_string").unwrap(),
        "Apple M3 Pro"
    );
}
