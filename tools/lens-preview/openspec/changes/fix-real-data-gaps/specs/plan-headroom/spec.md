# Spec Delta

## MODIFIED Requirements

### Requirement: Answer when nothing fills
When no window is on pace to fill before it resets, the answer SHALL say
"No window is on pace to fill before it resets." The exception is a window
whose pace filled it before now with no newer reading to confirm it: the
answer SHALL then say "<agent> <window> may be full: <share>% at <time of
the last reading>, on pace to fill by <time>." On local data without relay
readings, the answer SHALL NOT mention the relay. Instead, the "Last 4
weeks" section SHALL say that Claude windows need the statusline relay and
that Lens doesn't guess them.

Covered by: `local_headroom_names_the_missing_relay_under_history` and
`elapsed_forecast_says_may_be_full` (src/local/tests.rs).

#### Scenario: Nothing filling, no relay
- **WHEN** Plan headroom is shown with `--source local`, no relay data, and no Codex window on pace to fill
- **THEN** the answer is "No window is on pace to fill before it resets.", and under "Last 4 weeks" a note says Claude windows need the statusline relay and that Lens doesn't guess them

#### Scenario: Pace filled the window, unconfirmed
- **WHEN** a Codex 5-hour window's readings are 20% then 60%, its pace reached 100% before now, and no later reading exists
- **THEN** the answer is "Codex 5-hour may be full: 60% at <time>, on pace to fill by <time>.", never "No window is on pace to fill before it resets."
