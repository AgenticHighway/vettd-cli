pub mod base;
pub mod browser_footprints;
pub mod containers;
pub mod custom_rules;
pub mod mcp_configs;
pub mod source_risks;

use base::Detector;
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn read_utf8_head(path: &Path, max_bytes: usize) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut limited = file.take(max_bytes as u64);
    let mut bytes = Vec::with_capacity(max_bytes);
    limited.read_to_end(&mut bytes).ok()?;
    match String::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(err) => {
            let valid_up_to = err.utf8_error().valid_up_to();
            if valid_up_to == 0 {
                return None;
            }
            let bytes = err.into_bytes();
            std::str::from_utf8(&bytes[..valid_up_to])
                .ok()
                .map(str::to_owned)
        }
    }
}

pub fn get_all_detectors(mode: &str) -> Vec<Box<dyn Detector>> {
    let mut d: Vec<Box<dyn Detector>> = vec![
        Box::new(custom_rules::CustomRulesDetector::load()),
        Box::new(containers::ContainerDetector),
        Box::new(mcp_configs::MCPConfigDetector),
    ];
    if matches!(mode, "workdir" | "file") {
        d.push(Box::new(source_risks::SourceRiskDetector::new(mode)));
    }
    if matches!(mode, "host" | "scan" | "filesystem" | "home" | "root") {
        d.push(Box::new(browser_footprints::BrowserFootprintDetector));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn workdir_mode_has_four_detectors() {
        let detectors = get_all_detectors("workdir");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "source_risks"));
    }

    #[test]
    fn host_mode_includes_browser_detector() {
        let detectors = get_all_detectors("host");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn scan_mode_includes_browser_detector() {
        let detectors = get_all_detectors("scan");
        assert_eq!(detectors.len(), 4);
        assert!(detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn root_mode_includes_browser_detector() {
        let detectors = get_all_detectors("root");
        assert_eq!(detectors.len(), 4);
    }

    #[test]
    fn file_mode_excludes_browser_detector() {
        let detectors = get_all_detectors("file");
        assert!(detectors.iter().any(|d| d.name() == "source_risks"));
        assert!(!detectors.iter().any(|d| d.name() == "browser_footprints"));
    }

    #[test]
    fn all_detectors_have_names() {
        let detectors = get_all_detectors("host");
        for d in &detectors {
            assert!(!d.name().is_empty());
        }
    }

    #[test]
    fn read_utf8_head_reads_only_the_requested_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "abcdefghijk").unwrap();

        let content = read_utf8_head(&path, 8).unwrap();

        assert_eq!(content, "abcdefgh");
    }

    #[test]
    fn read_utf8_head_preserves_complete_multibyte_prefixes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "ééé").unwrap();

        let content = read_utf8_head(&path, 4).unwrap();

        assert_eq!(content, "éé");
    }

    #[test]
    fn read_utf8_head_drops_partial_multibyte_suffix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("artifact.txt");
        std::fs::write(&path, "1234567€").unwrap();

        let content = read_utf8_head(&path, 8).unwrap();

        assert_eq!(content, "1234567");
    }

    #[cfg(unix)]
    #[test]
    fn read_utf8_head_does_not_wait_for_stream_eof_after_limit() {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::process::Command;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let dir = tempdir().unwrap();
        let fifo_path = dir.path().join("artifact.fifo");
        let status = Command::new("mkfifo").arg(&fifo_path).status().unwrap();
        assert!(status.success());

        let (tx, rx) = mpsc::channel();
        let reader_path = fifo_path.clone();
        let reader = thread::spawn(move || {
            tx.send(read_utf8_head(&reader_path, 8)).unwrap();
        });

        let writer = thread::spawn(move || {
            let mut fifo = OpenOptions::new().write(true).open(&fifo_path).unwrap();
            fifo.write_all(b"12345678").unwrap();
            fifo.flush().unwrap();
            thread::sleep(Duration::from_secs(2));
        });

        let content = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("bounded read should finish before the writer closes the fifo")
            .unwrap();
        assert_eq!(content, "12345678");

        writer.join().unwrap();
        reader.join().unwrap();
    }
}
