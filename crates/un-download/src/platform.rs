/// Platform keywords used for matching release asset names.
/// Ordered from most specific to least specific — first match wins.

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
static PLATFORM_KEYWORDS: &[&str] = &["win64", "windows-x86_64", "windows-amd64", "windows"];

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
static PLATFORM_KEYWORDS: &[&str] = &["windows-aarch64", "windows-arm64"];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
static PLATFORM_KEYWORDS: &[&str] = &[
    "macos-x86_64",
    "darwin-x86_64",
    "macos-amd64",
    "darwin-amd64",
    "macos",
    "darwin",
];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
static PLATFORM_KEYWORDS: &[&str] = &[
    "macos-arm64",
    "darwin-arm64",
    "macos-aarch64",
    "darwin-aarch64",
    "macos",
    "darwin",
];

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
static PLATFORM_KEYWORDS: &[&str] = &["linux-x86_64", "linux-amd64", "linux"];

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
static PLATFORM_KEYWORDS: &[&str] = &["linux-arm64", "linux-aarch64", "linux"];

#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
static PLATFORM_KEYWORDS: &[&str] = &["linux"];

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
static PLATFORM_KEYWORDS: &[&str] = &[];

/// Returns platform keywords for matching release asset names, most specific first.
pub fn platform_keywords() -> &'static [&'static str] {
    PLATFORM_KEYWORDS
}

/// Returns the current platform identifier (e.g. "linux-x86_64").
pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "linux-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "linux-aarch64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "macos-x86_64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-aarch64";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x86_64";
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "windows-aarch64";
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    return "unknown";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_keywords_not_empty() {
        assert!(!platform_keywords().is_empty());
    }

    #[test]
    fn current_platform_not_unknown() {
        assert_ne!(current_platform(), "unknown");
    }
}
