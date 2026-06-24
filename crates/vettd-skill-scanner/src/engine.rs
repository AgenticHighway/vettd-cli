// This file has been split into the following modules:
//
//   scanner.rs         — scan_skill() orchestrator
//   rules.rs           — rule ID constants
//   checks/sensitive.rs  — sensitive pattern + entropy + env file scans
//   checks/behavioral.rs — behavioral injection scan
//   checks/encoding.rs   — base64 and hidden Unicode scans
//   checks/chains.rs     — exfiltration and malicious activity chain detection
//   checks/typosquat.rs  — typosquatting check
//   checks/description.rs — description-behavior mismatch check
//   skill_md/mod.rs      — SKILL.md frontmatter parser
//   skill_md/validate.rs — name validation
//   skill_md/body.rs     — body analysis helpers
//
// This file is no longer compiled. It can be deleted.
