//! `scan full`: a complete walk of every readable location.
//!
//! The mode exists to find project-scoped assets, which can live anywhere, so
//! it has no depth limit and no candidate cap, and it walks the directories
//! where tools physically install things (`node_modules`, `.cache`, `target/`,
//! ...) because matching is by name and costs a directory listing, not a file
//! read. What it skips is narrow and explicit:
//!
//! - virtual filesystems (`/proc`, `/sys`, `/dev`, ... and their OS equivalents),
//!   matched by absolute path or, on Linux, by the mount table's filesystem
//!   type, never by directory name, so a project folder called `run` or `boot`
//!   is still walked
//! - network and FUSE-network mounts (Linux), each named on stderr
//! - VCS metadata (`.git`, `.hg`, `.svn`)
//!
//! Known harness locations are walked first so they are reported before the
//! long tail, then the filesystem roots are walked. Directories that cannot be
//! read are counted and sampled on stderr rather than dropped.

use crate::discovery::{
    host_roots_for, is_included_file, should_descend, walk_harness_root, Candidate,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use walkdir::WalkDir;

const PRUNED_DIR_NAMES: &[&str] = &[".git", ".hg", ".svn"];
const UNREADABLE_SAMPLE: usize = 10;

#[derive(Debug, PartialEq, Eq)]
enum MountKind {
    Virtual,
    Network,
}

#[derive(Debug, PartialEq, Eq)]
struct ExcludedMount {
    path: PathBuf,
    fstype: String,
    kind: MountKind,
}

const VIRTUAL_FSTYPES: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "securityfs",
    "debugfs",
    "tracefs",
    "bpf",
    "pstore",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "binfmt_misc",
    "autofs",
    "rpc_pipefs",
    "nsfs",
    "selinuxfs",
    "efivarfs",
];

const NETWORK_FSTYPES: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "ceph",
    "glusterfs",
    "davfs",
    "fuse.sshfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.davfs2",
    "fuse.gvfsd-fuse",
];

/// Decode the octal escapes (`\040` for a space) used in mountinfo paths.
fn unescape_mount_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4].iter().all(u8::is_ascii_digit)
        {
            if let Ok(value) = u8::from_str_radix(&raw[i + 1..i + 4], 8) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Mounts in `/proc/self/mountinfo` content that `scan full` must not walk.
fn excluded_mounts(mountinfo: &str) -> Vec<ExcludedMount> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let mount_point = left.split_whitespace().nth(4)?;
            let fstype = right.split_whitespace().next()?;
            let kind = if VIRTUAL_FSTYPES.contains(&fstype) {
                MountKind::Virtual
            } else if NETWORK_FSTYPES.contains(&fstype) {
                MountKind::Network
            } else {
                return None;
            };
            Some(ExcludedMount {
                path: PathBuf::from(unescape_mount_path(mount_point)),
                fstype: fstype.to_string(),
                kind,
            })
        })
        .collect()
}

/// Virtual-filesystem locations by absolute path, per OS. These back up the
/// mount-table check (which is Linux only, and absent in some containers).
fn virtual_fs_paths() -> Vec<PathBuf> {
    let paths: &[&str] = match std::env::consts::OS {
        "macos" => &["/dev", "/System", "/cores", "/private/var/vm"],
        "windows" => &[],
        _ => &["/proc", "/sys", "/dev", "/run"],
    };
    paths.iter().map(PathBuf::from).collect()
}

/// Top-level locations to walk: `/` (which reaches every mounted volume) on
/// Unix, every existing drive letter on Windows.
fn filesystem_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        ('A'..='Z')
            .map(|letter| PathBuf::from(format!("{letter}:\\")))
            .filter(|drive| drive.exists())
            .collect()
    } else {
        vec![PathBuf::from("/")]
    }
}

#[derive(Default)]
struct Unreadable {
    count: usize,
    samples: Vec<PathBuf>,
}

impl Unreadable {
    fn record(&mut self, path: Option<&Path>) {
        self.count += 1;
        if let (Some(path), true) = (path, self.samples.len() < UNREADABLE_SAMPLE) {
            self.samples.push(path.to_path_buf());
        }
    }
}

fn walk_full_root(
    root: &Path,
    excluded_paths: &HashSet<PathBuf>,
    unreadable: &mut Unreadable,
    on_tick: Option<&dyn Fn(&str)>,
) -> Vec<Candidate> {
    let names: HashSet<&str> = PRUNED_DIR_NAMES.iter().copied().collect();
    let mut candidates = Vec::new();

    let walker = WalkDir::new(root).follow_links(false);
    let filtered = walker.into_iter().filter_entry(|e| {
        !(e.file_type().is_dir() && excluded_paths.contains(e.path()))
            && should_descend(e, &names, root)
    });

    for entry in filtered {
        match entry {
            Ok(entry) if is_included_file(&entry, root) => {
                candidates.push(Candidate {
                    path: entry.into_path(),
                    origin: "root".to_string(),
                });
                if let Some(tick) = on_tick {
                    if candidates.len() % 10_000 == 0 {
                        tick(&format!("{} files in {}", candidates.len(), root.display()));
                    }
                }
            }
            Ok(_) => {}
            Err(err) => unreadable.record(err.path()),
        }
    }
    candidates
}

pub fn discover_root_surfaces(on_tick: Option<&dyn Fn(&str)>) -> Vec<Candidate> {
    let mut excluded_paths: HashSet<PathBuf> = virtual_fs_paths().into_iter().collect();
    if cfg!(target_os = "linux") {
        if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
            for mount in excluded_mounts(&mountinfo) {
                if mount.kind == MountKind::Network {
                    eprintln!(
                        "note: skipped {} mount {} (network filesystem)",
                        mount.fstype,
                        mount.path.display()
                    );
                }
                excluded_paths.insert(mount.path);
            }
        }
    }

    let mut candidates = Vec::new();
    let mut unreadable = Unreadable::default();

    // Known harness locations first: they are the likeliest to hold assets and
    // are cheap, so they finish (and show progress) before the long tail.
    let anchors = host_roots_for(true);
    for anchor in &anchors {
        let started = Instant::now();
        let found = walk_harness_root(anchor, "root", on_tick);
        eprintln!(
            "walked {}: {} files in {:.1}s",
            anchor.display(),
            found.len(),
            started.elapsed().as_secs_f32()
        );
        candidates.extend(found);
    }
    excluded_paths.extend(anchors);

    for root in filesystem_roots() {
        let started = Instant::now();
        let found = walk_full_root(&root, &excluded_paths, &mut unreadable, on_tick);
        eprintln!(
            "walked {}: {} files in {:.1}s",
            root.display(),
            found.len(),
            started.elapsed().as_secs_f32()
        );
        candidates.extend(found);
    }

    if unreadable.count > 0 {
        eprintln!(
            "warning: {} path(s) could not be read and were not examined, e.g.: {}",
            unreadable.count,
            unreadable
                .samples
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn walk(root: &Path, excluded: &[PathBuf]) -> (Vec<Candidate>, Unreadable) {
        let set: HashSet<PathBuf> = excluded.iter().cloned().collect();
        let mut unreadable = Unreadable::default();
        let found = walk_full_root(root, &set, &mut unreadable, None);
        (found, unreadable)
    }

    #[test]
    fn walks_dependency_cache_and_build_dirs_that_other_modes_prune() {
        let tmp = TempDir::new().unwrap();
        for dir in ["node_modules/p", ".cache/x", "target/debug", "vendor/v"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
            fs::write(tmp.path().join(dir).join("SKILL.md"), "#\n").unwrap();
        }
        let (found, _) = walk(tmp.path(), &[]);
        assert_eq!(found.len(), 4, "found: {found:?}");
    }

    #[test]
    fn prunes_vcs_metadata_only() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
        fs::write(tmp.path().join(".git/objects/blob"), "x").unwrap();
        fs::write(tmp.path().join("SKILL.md"), "#\n").unwrap();
        let (found, _) = walk(tmp.path(), &[]);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn exclusions_match_absolute_paths_not_directory_names() {
        // A project folder that happens to be called `run` or `boot` is
        // ordinary user data; only the excluded absolute path is skipped.
        let tmp = TempDir::new().unwrap();
        let virtual_dir = tmp.path().join("proc");
        let user_dir = tmp.path().join("projects/run");
        fs::create_dir_all(&virtual_dir).unwrap();
        fs::create_dir_all(&user_dir).unwrap();
        fs::write(virtual_dir.join("f"), "x").unwrap();
        fs::write(user_dir.join("SKILL.md"), "#\n").unwrap();

        let (found, _) = walk(tmp.path(), &[virtual_dir]);
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("projects/run/SKILL.md"));
    }

    #[test]
    fn has_no_file_count_cap() {
        let tmp = TempDir::new().unwrap();
        for i in 0..2_000 {
            fs::write(tmp.path().join(format!("f{i}.txt")), "x").unwrap();
        }
        let (found, _) = walk(tmp.path(), &[]);
        assert_eq!(found.len(), 2_000);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directories_are_counted_and_sampled() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("SKILL.md"), "#\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let can_still_read = fs::read_dir(&locked).is_ok();

        let (_, unreadable) = walk(tmp.path(), &[]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if !can_still_read {
            assert_eq!(unreadable.count, 1);
            assert_eq!(unreadable.samples, vec![locked]);
        }
    }

    #[test]
    fn mountinfo_virtual_and_network_mounts_are_excluded_local_ones_are_not() {
        let info = "\
22 28 0:21 / /proc rw,nosuid - proc proc rw
23 28 0:22 / /sys rw,nosuid - sysfs sysfs rw
28 1 259:2 / / rw,relatime - ext4 /dev/nvme0n1p2 rw
40 28 0:50 / /mnt/nas\\040share rw - nfs4 nas:/export rw
41 28 259:3 / /mnt/data rw,relatime - ext4 /dev/nvme1n1 rw
42 28 0:60 / /run/user/1000 rw - tmpfs tmpfs rw
";
        let mounts = excluded_mounts(info);
        let by_path = |p: &str| mounts.iter().find(|m| m.path == Path::new(p));
        assert_eq!(by_path("/proc").unwrap().kind, MountKind::Virtual);
        assert_eq!(by_path("/sys").unwrap().kind, MountKind::Virtual);
        assert_eq!(by_path("/mnt/nas share").unwrap().kind, MountKind::Network);
        assert!(by_path("/").is_none());
        assert!(by_path("/mnt/data").is_none(), "local secondary mount");
        assert!(by_path("/run/user/1000").is_none(), "tmpfs is real storage");
    }
}
