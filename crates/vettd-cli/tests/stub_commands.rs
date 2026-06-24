//! Fail-loud assertions for web-facing command stubs that remain unimplemented.
//!
//! `not_implemented` calls `std::process::exit`, so it cannot be exercised
//! in-process. These tests launch the real `vettd` binary and assert that each
//! remaining stub: exits with code 2, keeps stdout (the machine channel)
//! empty, and prints the not-implemented notice to stderr. If a stub ever
//! silently exits 0, stops printing the notice, or collapses onto exit(1),
//! these tests fail.
//!
//! Commands removed from this file are now fully implemented:
//!   auth status, contract status, directory search, directory list,
//!   directory view, directory findings, directory compare,
//!   directory trending, directory random.
//!
//! No stubs remain (all #631-blocked items are now implemented).
