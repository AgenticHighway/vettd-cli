//! Shared constants for the vettd skill scanner.
//!
//! Any constant used in more than one module, or that may need to be shared
//! with callers, lives here instead of in the file that first needs it.

/// Scanner version. Must stay in sync with `CURRENT_SCANNER_VERSION` in the
/// vettd web app's `skill-analyzer.ts`. The server uses this value to detect
/// stale scan results and trigger re-scans.
pub const CURRENT_SCANNER_VERSION: u32 = 9;

/// Default `source` value for findings produced by this scanner.
pub const DEFAULT_SOURCE: &str = "vettd";

/// Required prefix for the `detail` field on file-specific findings.
///
/// Chain detection in the vettd web scanner parses the filepath out of this
/// string with the regex `Detected in ([^:]+):`. The full format is:
///
/// ```text
/// Detected in <filepath>:<linenum> — `snippet`
/// ```
///
/// The Rust scanner must emit `detail` strings in this format for any finding
/// that has an associated file and line number so that chain detection can
/// co-locate credential-source and network-sink findings.
pub const DETAIL_DETECTED_IN_PREFIX: &str = "Detected in ";
