# Spec Delta

## MODIFIED Requirements

### Requirement: Files outside the history span are skipped
For the dashboard, log files last modified more than 28 days before the run
SHALL NOT be opened. Files modified within 28 days SHALL be read for limit
hits. Only requests in the 7-day period SHALL count towards usage. History's
read, which opens every log file whatever its age, SHALL NOT change any
dashboard figure or the export.

#### Scenario: Old and recent files
- **WHEN** one file was last modified 30 days ago and another 20 days ago, with no request in the period
- **THEN** the dashboard doesn't use the first, the second's limit hits count, and neither adds usage

#### Scenario: Old file listed only in History
- **WHEN** a file last modified 30 days ago holds a session
- **THEN** History lists that session, and Home's session count, usage and limit hits are the same as without that file

### Requirement: Startup reads recent files in full
With `--source local`, the preview SHALL read every log file modified in the
last 28 days before it draws the first screen or prints the export, and SHALL
print "Reading local logs…" on stderr before it starts reading. No startup
time limit is set: about 270 files (1.4 GB) took 7–14 s warm. Files older
than 28 days SHALL be read only for History, after the first screen is
drawn, and never with `--json`.

Covered by: `local_export_says_it_is_reading_on_stderr` (tests/startup.rs);
timing is not tested.

#### Scenario: Large recent history
- **WHEN** `--source local --json` runs over a large recent history
- **THEN** "Reading local logs…" is on stderr before the export is on stdout, and the export is complete

#### Scenario: Old files don't slow the export
- **WHEN** `--source local --json` runs where most log files are older than 28 days
- **THEN** no file older than 28 days is opened
