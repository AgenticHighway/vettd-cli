"""Causal-language lint tests (spike #828).

Each test states what it proves and what it cannot prove. None can prove the phrase list is complete:
they prove the lint rejects what the contract names and accepts the hedged, count-based phrasing the
ranking output is required to use.
"""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from lint_copy import FORBIDDEN_PHRASES, RATE_RULE, lint  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# One offending sentence per rule; the sentence must not trip any other rule.
OFFENDERS = {
    "causes": "this skill causes fewer errors",
    "because_of": "fewer errors because of this skill",
    "improves": "this skill improves outcomes",
    "makes_you": "this skill makes you productive",
    "faster_than": "this skill is faster than the other one",
    "better_than": "this skill is better than the other one",
    "worse_than": "this skill is worse than the other one",
    "percent_better_worse": "this skill is 12% better",
    "saves": "this skill saves tokens",
    "proves": "the data proves this skill works",
    "guarantee": "no guarantee of success",
    "dollar_amount": "cost $4 across observed runs",
    "bare_reliable": "this skill is reliable",
}


def rules_of(findings):
    return {f.split(": ")[1] for f in findings}


class LintTests(unittest.TestCase):
    def test_every_contract_phrase_has_an_offender_and_is_flagged(self):
        """Proves each forbidden phrase in the contract is caught by exactly its own rule and that the
        test table covers every rule (no rule silently untested). Cannot prove paraphrases are caught."""
        self.assertEqual(set(OFFENDERS), {rule for rule, _ in FORBIDDEN_PHRASES})
        for rule, sentence in OFFENDERS.items():
            with self.subTest(rule=rule):
                self.assertEqual(rules_of(lint(sentence)), {rule}, lint(sentence))

    def test_phrases_are_case_insensitive(self):
        """Proves capitalisation does not evade the lint. Cannot prove spaced-out or hyphenated variants
        (e.g. "im-proves") are caught."""
        self.assertEqual(rules_of(lint("This CAUSES fewer errors")), {"causes"})
        self.assertEqual(rules_of(lint("It Improves things")), {"improves"})

    def test_bare_reliable_is_flagged_but_observed_reliable_is_not(self):
        """Proves "reliable"/"unreliable" is only permitted immediately after "observed", and that
        "reliability" (the #916 proxy's name) is not caught. Cannot prove other hedges are honoured."""
        self.assertEqual(rules_of(lint("marked unreliable")), {"bare_reliable"})
        self.assertEqual(lint("observed reliable in this window"), [])
        self.assertEqual(lint("observed unreliable in this window"), [])
        self.assertEqual(lint("the reliability proxy from static analysis"), [])

    def test_rate_line_without_hedge_is_flagged(self):
        """Proves a line naming a rate with neither "observed" nor "in N calls" fails the hedge rule.
        Cannot prove the hedge is placed meaningfully, only that it is present on the same line."""
        self.assertEqual(rules_of(lint("non-success rate 0.10")), {RATE_RULE})
        self.assertEqual(rules_of(lint("failure rates were high")), {RATE_RULE})

    def test_rate_line_with_hedge_passes(self):
        """Proves either hedge form satisfies the rule, case-insensitively. Cannot prove the number on
        the line is the observed one."""
        self.assertEqual(lint("observed non-success rate 0.10"), [])
        self.assertEqual(lint("Observed rate 0.10"), [])
        self.assertEqual(lint("non-success rate 0.10 in 40 calls"), [])

    def test_rate_inside_another_word_does_not_trigger_hedge(self):
        """Proves the hedge rule keys on the word "rate", not the substring, so "generate", "separate"
        and identifiers like rate_show pass. Cannot prove a rate described without the word is hedged."""
        self.assertEqual(lint("generate a separate list; floor rate_show applies"), [])

    def test_compliant_sentence_passes(self):
        """Proves the count-based phrasing the ranking output uses is clean under every rule.
        Cannot prove every template in rank.py is clean; that is rank's own test."""
        self.assertEqual(lint("observed 3 non-successes in 40 calls"), [])

    def test_findings_name_the_line_number(self):
        """Proves findings carry the 1-based line of the offending text so a doc can be fixed from the
        CLI output. Cannot prove column positions."""
        findings = lint("first line is clean\nsecond line causes trouble\nthird rate line")
        self.assertEqual(findings[0].split(": ")[0], "2")
        self.assertEqual(findings[1].split(": ")[0], "3")
        self.assertEqual(len(findings), 2)

    def test_cli_exit_codes(self):
        """Proves the CLI exits 1 and names path:line when a file has findings, 0 when clean, 2 with no
        arguments. Cannot prove behaviour on unreadable files beyond a non-zero exit."""
        script = os.path.join(PROTO_DIR, "lint_copy.py")
        with tempfile.TemporaryDirectory() as tmp:
            clean = os.path.join(tmp, "clean.txt")
            dirty = os.path.join(tmp, "dirty.txt")
            with open(clean, "w", encoding="utf-8") as fh:
                fh.write("observed 3 non-successes in 40 calls\n")
            with open(dirty, "w", encoding="utf-8") as fh:
                fh.write("fine\nthis skill saves tokens\n")
            ok = subprocess.run([sys.executable, script, clean], capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            bad = subprocess.run([sys.executable, script, clean, dirty], capture_output=True, text=True)
            self.assertEqual(bad.returncode, 1)
            self.assertIn(f"{dirty}:2: saves:", bad.stdout)
            none = subprocess.run([sys.executable, script], capture_output=True, text=True)
            self.assertEqual(none.returncode, 2)


if __name__ == "__main__":
    unittest.main()


class RateLimitIsNotAStatistic(unittest.TestCase):
    """Shows: 'rate limit' / 'rate-limit' do not trigger the rate hedge, while a bare statistical
    'rate' still does. Cannot show: that every non-statistical use of the word is covered."""

    def test_rate_limit_is_exempt(self):
        self.assertEqual([], lint("the route has a durable rate limit and a rate-limit policy row"))

    def test_bare_rate_still_needs_a_hedge(self):
        self.assertTrue(any("rate_without_hedge" in f for f in lint("the failure rate is 3%")))

