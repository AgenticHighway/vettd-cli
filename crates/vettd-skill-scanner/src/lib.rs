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

mod checks;
mod finding;
mod result;
mod rules;
mod scanner;
mod skill_md;

pub use finding::{Finding, FindingCategory, Intent, Severity};
pub use result::SkillScanResult;
pub use scanner::scan_skill;
