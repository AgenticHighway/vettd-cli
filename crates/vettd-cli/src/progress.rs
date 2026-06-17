//! Scan progress indicator for terminal output.
//!
//! Writes transient status lines to stderr using ANSI escapes.
//! In quiet mode all output is suppressed.

use std::io::{self, Write};
use std::time::Instant;

pub struct ScanProgress {
    quiet: bool,
    start: Instant,
    phase: String,
    last_update: f64,
}

impl ScanProgress {
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            start: Instant::now(),
            phase: String::new(),
            last_update: 0.0,
        }
    }

    fn elapsed_str(&self) -> String {
        let secs = self.start.elapsed().as_secs_f64();
        if secs < 60.0 {
            format!("{:.1}s", secs)
        } else {
            format!("{}m{:02}s", secs as u64 / 60, secs as u64 % 60)
        }
    }

    pub fn phase(&mut self, name: &str) {
        self.phase = name.to_string();
        self.last_update = 0.0;
        self.write(name);
    }

    pub fn tick(&mut self, detail: &str) {
        let now = self.start.elapsed().as_secs_f64();
        if now - self.last_update < 0.1 {
            return;
        }
        self.last_update = now;
        let msg = format!("{}  {}", self.phase, detail);
        self.write(&msg);
    }

    pub fn done(&mut self, message: Option<&str>) {
        if self.quiet {
            return;
        }
        eprint!("\r\x1b[K");
        if let Some(msg) = message {
            eprintln!("  \x1b[2m{}  ({})\x1b[0m", msg, self.elapsed_str());
        }
    }

    fn write(&self, text: &str) {
        if self.quiet {
            return;
        }
        eprint!(
            "\r\x1b[K  \x1b[36m⠿\x1b[0m {}  \x1b[2m{}\x1b[0m",
            text,
            self.elapsed_str()
        );
        let _ = io::stderr().flush();
    }
}
