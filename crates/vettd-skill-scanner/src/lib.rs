//! `vettd-skill-scanner` — pure, I/O-free skill scanner for the vettd pipeline.
//!
//! ## Contract
//!
//! This crate performs **no filesystem I/O, no network access, and no stdout/stderr
//! output**. All inputs are pre-loaded by the caller. This boundary is intentional:
//! the scanner is designed to be extracted into a standalone service or container
//! without modification.
//!
//! ## Entry point
//!
//! ```ignore
//! use vettd_skill_scanner::{scan_skill, SkillScanResult};
//! use std::collections::HashMap;
//!
//! let text_files: HashMap<String, String> = /* caller loads from disk or zip */;
//! let all_paths: Vec<String>              = /* all paths including binaries */;
//!
//! let result: SkillScanResult = scan_skill(&text_files, &all_paths);
//! ```
//!
//! See [`scan_skill`] for full documentation.

pub mod consts;

mod chain;
mod engine;
mod finding;
mod result;

pub use engine::scan_skill;
pub use finding::{Finding, FindingCategory, Intent, Severity};
pub use result::SkillScanResult;
