# Spec Delta

## ADDED Requirements

### Requirement: Log files are read a line at a time
The local readers SHALL read each log file a line at a time and SHALL NOT
hold a whole file's bytes in memory. The Claude reader SHALL NOT hold more
than one parsed line of a file at once. Which lines are read SHALL NOT
change.

Covered by: `streamed_lines_match_the_whole_file_rules`
(src/local/tests.rs). Peak memory is measured by hand, not tested.

#### Scenario: One very large recent log
- **WHEN** `--source local --json` runs over logs where one recent Claude file is 652 MB
- **THEN** the export is the same as with a whole-file read, and peak memory stays far below the file's size

#### Scenario: Line endings and blank lines
- **WHEN** a file mixes "\n" and "\r\n" endings and has blank or whitespace-only lines
- **THEN** each JSON line is read once and the blank lines are skipped

#### Scenario: Cut-off last line
- **WHEN** a file's last line is cut off part-way
- **THEN** its earlier lines are read, the cut-off line is skipped, and the session is marked partial

#### Scenario: Unreadable file
- **WHEN** a log file can't be opened
- **THEN** it gives no lines and its session is marked partial
