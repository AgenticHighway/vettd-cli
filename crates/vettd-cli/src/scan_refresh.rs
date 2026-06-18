#[cfg(target_os = "macos")]
use crate::scan_cache::RootCursor;
use crate::scan_cache::ScanCache;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;

pub const MACOS_FSEVENTS_BACKEND: &str = "macos_fsevents_v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRoot {
    pub path: PathBuf,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootRefreshAction {
    ReuseCached,
    Rescan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCursorUpdate {
    pub root_path: String,
    pub backend_type: String,
    pub cursor_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRefreshPlan {
    pub root: DiscoveryRoot,
    pub action: RootRefreshAction,
    pub cursor_update: Option<RootCursorUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MacosCursorToken {
    last_event_id: u64,
    device_id: u64,
}

pub fn plan_root_refresh(
    cache: Option<&ScanCache>,
    roots: &[DiscoveryRoot],
) -> Vec<RootRefreshPlan> {
    let Some(cache) = cache else {
        return roots
            .iter()
            .cloned()
            .map(|root| rescan_plan(root, None))
            .collect();
    };

    #[cfg(target_os = "macos")]
    {
        roots
            .iter()
            .cloned()
            .map(|root| plan_macos_root_refresh(cache, root))
            .collect()
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = cache;
        roots
            .iter()
            .cloned()
            .map(|root| rescan_plan(root, None))
            .collect()
    }
}

fn rescan_plan(root: DiscoveryRoot, cursor_update: Option<RootCursorUpdate>) -> RootRefreshPlan {
    RootRefreshPlan {
        root,
        action: RootRefreshAction::Rescan,
        cursor_update,
    }
}

fn reuse_plan(root: DiscoveryRoot, cursor_update: Option<RootCursorUpdate>) -> RootRefreshPlan {
    RootRefreshPlan {
        root,
        action: RootRefreshAction::ReuseCached,
        cursor_update,
    }
}

fn root_cursor_update(root_path: &str, token: &MacosCursorToken) -> Option<RootCursorUpdate> {
    serde_json::to_string(token)
        .ok()
        .map(|cursor_token| RootCursorUpdate {
            root_path: root_path.to_string(),
            backend_type: MACOS_FSEVENTS_BACKEND.to_string(),
            cursor_token,
        })
}

fn plan_from_cursor_state(
    root: DiscoveryRoot,
    previous: Option<MacosCursorToken>,
    current: Option<MacosCursorToken>,
) -> RootRefreshPlan {
    let root_path = root.path.to_string_lossy().to_string();
    let cursor_update = current
        .as_ref()
        .and_then(|token| root_cursor_update(&root_path, token));
    let Some(current) = current else {
        return rescan_plan(root, None);
    };
    let Some(previous) = previous else {
        return rescan_plan(root, cursor_update);
    };
    if previous.device_id != current.device_id {
        return rescan_plan(root, cursor_update);
    }

    // Reuse the cached root only when the macOS FSEvents cursor is unchanged.
    // This is conservative, but avoids the async replay callback path that can
    // segfault on host-root scans.
    if previous.last_event_id == current.last_event_id {
        return reuse_plan(root, cursor_update);
    }

    rescan_plan(root, cursor_update)
}

#[cfg(target_os = "macos")]
fn plan_macos_root_refresh(cache: &ScanCache, root: DiscoveryRoot) -> RootRefreshPlan {
    let root_path = root.path.to_string_lossy().to_string();
    let previous = load_macos_cursor(cache, &root_path);
    let current = current_cursor_token(&root.path);
    plan_from_cursor_state(root, previous, current)
}

#[cfg(target_os = "macos")]
fn load_macos_cursor(cache: &ScanCache, root_path: &str) -> Option<MacosCursorToken> {
    match cache.load_root_cursor(root_path, MACOS_FSEVENTS_BACKEND) {
        Ok(Some(RootCursor { cursor_token, .. })) => serde_json::from_str(&cursor_token).ok(),
        Ok(None) => None,
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn current_cursor_token(root_path: &Path) -> Option<MacosCursorToken> {
    use fsevent_sys::FSEventsGetCurrentEventId;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(root_path).ok()?;
    Some(MacosCursorToken {
        last_event_id: unsafe { FSEventsGetCurrentEventId() },
        device_id: metadata.dev(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str, origin: &str) -> DiscoveryRoot {
        DiscoveryRoot {
            path: PathBuf::from(path),
            origin: origin.to_string(),
        }
    }

    #[test]
    fn plan_from_cursor_state_reuses_cached_when_root_is_unchanged() {
        let plan = plan_from_cursor_state(
            root("/tmp/root", "host"),
            Some(MacosCursorToken {
                last_event_id: 10,
                device_id: 7,
            }),
            Some(MacosCursorToken {
                last_event_id: 10,
                device_id: 7,
            }),
        );

        assert_eq!(plan.action, RootRefreshAction::ReuseCached);
        assert!(plan.cursor_update.is_some());
    }

    #[test]
    fn plan_from_cursor_state_rescans_on_device_change() {
        let plan = plan_from_cursor_state(
            root("/tmp/root", "home"),
            Some(MacosCursorToken {
                last_event_id: 10,
                device_id: 7,
            }),
            Some(MacosCursorToken {
                last_event_id: 12,
                device_id: 8,
            }),
        );

        assert_eq!(plan.action, RootRefreshAction::Rescan);
        assert!(plan.cursor_update.is_some());
    }

    #[test]
    fn plan_from_cursor_state_rescans_when_event_id_advances() {
        let plan = plan_from_cursor_state(
            root("/tmp/root", "host"),
            Some(MacosCursorToken {
                last_event_id: 10,
                device_id: 7,
            }),
            Some(MacosCursorToken {
                last_event_id: 12,
                device_id: 7,
            }),
        );

        assert_eq!(plan.action, RootRefreshAction::Rescan);
        assert!(plan.cursor_update.is_some());
    }

    #[test]
    fn plan_root_refresh_without_cache_rescans_all_roots() {
        let plans = plan_root_refresh(None, &[root("/tmp/root", "host")]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].action, RootRefreshAction::Rescan);
        assert!(plans[0].cursor_update.is_none());
    }
}
