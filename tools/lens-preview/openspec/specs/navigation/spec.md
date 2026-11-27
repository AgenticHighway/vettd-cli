# navigation Specification

## Purpose
What each key does in the Lens preview, and how the selection and scroll
position are kept as a person moves between views.

## Requirements

### Requirement: Esc goes back
Esc SHALL return to the previous view with that view's selection and scope
restored. At Home, Esc SHALL do nothing: it SHALL NOT quit or change the
screen.

Covered by: `plan_journey_goes_deep_and_comes_back_to_the_same_finding`,
`spend_journey_reaches_the_run_and_restores_selection`,
`maintainer_journey_filters_clears_and_returns_to_the_row`,
`help_opens_and_returns` and `esc_at_home_keeps_the_person_at_home`
(src/e2e/mod.rs).

#### Scenario: Back to the same finding
- **WHEN** a person opens the first Home finding, opens Nov 18 from Plan headroom, and presses Esc twice
- **THEN** the breadcrumb is "Home" and the Claude 5-hour finding is selected again

#### Scenario: Esc at Home
- **WHEN** Esc is pressed at Home
- **THEN** the preview keeps running and the screen is unchanged

### Requirement: Arrow and page keys move the selection
Up, Down, PageUp and PageDown SHALL move the selection between rows that open
something while such a row is available in that direction.

Covered by: `spend_journey_reaches_the_run_and_restores_selection` and
`unit_toggle_keeps_the_selected_target` (src/e2e/mod.rs).

#### Scenario: Down on Home
- **WHEN** Down is pressed once on Home at 80×24
- **THEN** the Nov 16 finding is selected

### Requirement: Arrow and page keys scroll past the actions
When there is no selectable row to move to in that direction, the same keys
SHALL scroll the view instead: on views with no selectable rows, above the
first one, and below the last one.

Covered by: `token_detail_scrolls_to_its_last_row`, `help_scrolls_to_quit`,
`exact_copy_reveals_rows_above_its_first_action` and
`plan_headroom_reaches_its_trailing_explanation` (src/e2e/scrolling.rs, at
38×10).

#### Scenario: View with no selectable rows
- **WHEN** token detail is open at 38×10 and Down is pressed 12 times
- **THEN** the last rows ("Output", "21000 tokens") come into view, and 12 Up presses bring back "Main / Large"

#### Scenario: Rows below the last action
- **WHEN** Plan headroom is open at 38×10 and Down is pressed 14 times
- **THEN** "Provider meters are" comes into view, and 14 Up presses return the selection to the Claude 5-hour row

#### Scenario: Rows above the first action
- **WHEN** a copy (Placement) view is open at 38×10 and Up is pressed 12 times
- **THEN** its "Locator" row comes into view

### Requirement: Viewport follows the selection only when on screen
Once the person scrolls, the view SHALL stop following the selection, so it
doesn't snap back while they read. It SHALL follow the selection again once
the selected row is back on screen.

Covered by: `exact_copy_reveals_rows_above_its_first_action` and
`plan_headroom_reaches_its_trailing_explanation` (src/e2e/scrolling.rs).

#### Scenario: Scrolling back to the action
- **WHEN** a copy view has been scrolled up past its first action and Down is pressed 12 times
- **THEN** the selected action is drawn again

### Requirement: Find takes text
In Find, letters, digits, punctuation and space, `q` included, SHALL be typed
into the search and SHALL NOT run commands. Backspace SHALL delete the last
character. Ctrl+C SHALL still quit.

Covered by: `find_types_letters_instead_of_running_commands` (src/e2e/mod.rs).

#### Scenario: q while typing
- **WHEN** a person opens Find, types "rev", then types "q"
- **THEN** the preview doesn't quit and shows "No matches. Edit search.", and after Backspace and Enter it opens "review-kit (week)"

### Requirement: Keys screen doesn't stack
Pressing `?` SHALL open Keys, except while Keys is already showing, where it
SHALL do nothing. One Esc SHALL always leave Keys.

Covered by: `repeated_help_takes_one_esc_to_leave` and `help_opens_and_returns`
(src/e2e/mod.rs).

#### Scenario: Repeated ?
- **WHEN** `?` is pressed three times at Home and then Esc once
- **THEN** the breadcrumb is "Home"

### Requirement: Unit toggle where offered
`c` SHALL switch between priced USD and request counts only on Home, the
daily view, a day, a session and token detail, which are the views whose
footer offers it. On other views `c` SHALL do nothing.

Covered by: `unit_toggle_switches_home_between_spend_and_requests` and
`unit_toggle_is_ignored_where_it_is_not_offered` (src/e2e/mod.rs).

#### Scenario: Toggle on Home
- **WHEN** `c` is pressed on Home, then pressed again
- **THEN** Home shows "DAILY REQUESTS", then "DAILY SPEND" again

#### Scenario: Not offered
- **WHEN** `c` is pressed on the component inventory and Esc returns to Home
- **THEN** Home still shows "DAILY SPEND"

### Requirement: Unit toggle keeps the selected target
After a unit switch, the selection SHALL stay on the row that opens the same
view. If no row in the new unit opens that view, the first row SHALL be
selected.

Covered by: `unit_toggle_keeps_the_selected_target` (src/e2e/mod.rs).

#### Scenario: Selection follows its target
- **WHEN** the review-kit finding is selected on Home and `c` is pressed
- **THEN** review-kit is still selected, although it moved up a row because Nov 16 isn't unusual in requests

#### Scenario: Target vanishes
- **WHEN** the Nov 16 finding is selected on Home and `c` is pressed
- **THEN** the first row (the Claude 5-hour finding) is selected
