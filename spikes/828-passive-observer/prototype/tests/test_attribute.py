"""attribute.py tests (spike #828). Every name, path, timestamp and value below is invented.

Each test states what it proves and what it cannot prove. Fixtures are temp directories built per
test (TMPDIR decides where); nothing here reads real harness state.
"""
import hashlib
import hmac
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import attribute  # noqa: E402
from attribute import FsIndex, bom_version, canonical_descriptor, descriptor_hash, name_hash  # noqa: E402
from model import (  # noqa: E402
    ASSET_AGENT,
    ASSET_MCP_SERVER,
    ASSET_RULES_FILE,
    ASSET_SKILL,
    BINDING_EXACT,
    BINDING_MTIME,
    BINDING_NA,
    BINDING_UNPROVEN,
    KEY_CONTENT,
    KEY_DESCRIPTOR,
    KEY_NAME,
    TIER_INFERRED,
    InvocationObs,
    RunFacts,
)
from sources.base import InBandAsset, LoadedSetEvent  # noqa: E402

T = 1_756_000_000_000  # invented harness listing timestamp (ms)
SECRET_A = b"invented-observer-secret-aaaa"
SECRET_B = b"invented-observer-secret-bbbb"
# Built at runtime so no secret-shaped literal exists in the file.
FLAG_VALUE = "".join(["sk", "-", "invented", "0123456789", "abcdefghij"])
HEX64 = "0123456789abcdef"


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def make_claude_home(root, node_path="/opt/tools/bin/node", flag_value=FLAG_VALUE, env_value="x"):
    """Invented skill (two files), agent, and MCP descriptor under a temp claude_home."""
    skill = os.path.join(root, "skills", "skill-alpha")
    _write(os.path.join(skill, "SKILL.md"), "---\nname: skill-alpha\n---\nInvented body.\n")
    _write(os.path.join(skill, "reference.md"), "Invented reference text.\n")
    _write(os.path.join(root, "agents", "agent-omega.md"), "---\nname: agent-omega\n---\nInvented agent.\n")
    cfg = {"mcpServers": {"srvfx": {"command": node_path, "args": ["server.js", "--api-key", flag_value, "--port", "8080"],
                                    "env": {"ZZ_TOKEN": env_value}}}}
    _write(os.path.join(root, ".claude.json"), json.dumps(cfg))
    return skill


def make_codex_home(root):
    _write(os.path.join(root, "config.toml"), '[mcp_servers.srvfx]\ncommand = "npx"\nargs = ["-y", "pkg"]\n')


def set_tree_mtime(root, ms):
    ns = ms * 1_000_000
    for dirpath, _dirnames, filenames in os.walk(root, topdown=False):
        for fn in filenames:
            os.utime(os.path.join(dirpath, fn), ns=(ns, ns))
        os.utime(dirpath, ns=(ns, ns))


def make_run(harness="claude_code", events=(), invocations=(), in_band=(), first=T - 1000, last=T + 100_000):
    return RunFacts(session_key="sess-invented-01", harness=harness, harness_version="0.0.0", entrypoint_class="cli",
                    effort="medium", permission_mode="default", model="other", observed_day="2025-08-24",
                    first_ts_ms=first, last_ts_ms=last, run_outcome="completed", invocations=list(invocations),
                    loaded_events=list(events), in_band_assets=list(in_band))


def obs_by_name(attributed, segment=0):
    return {attributed.name_map[o.key.asset_id]: o for o in attributed.observations[segment]}


class TempDirs(unittest.TestCase):
    def tmp(self):
        td = tempfile.TemporaryDirectory()
        self.addCleanup(td.cleanup)
        return td.name


class DescriptorHash(TempDirs):
    def test_path_prefix_and_secret_value_do_not_change_the_hash(self):
        """Proves: two homes whose MCP descriptors differ only in the command's directory, the
        value after --api-key, and an env value hash to the same descriptor_hash, read through
        FsIndex from `.claude.json`; and the hash is hex64.
        Cannot prove: that every secret-carrying shape is stripped (only the listed rules are)."""
        home_a, home_b = self.tmp(), self.tmp()
        make_claude_home(home_a)
        make_claude_home(home_b, node_path="/usr/local/lib/tools/node", flag_value=FLAG_VALUE[::-1], env_value="y")
        h_a = FsIndex(claude_home=home_a).mcp_descriptor("claude_code", "srvfx")
        h_b = FsIndex(claude_home=home_b).mcp_descriptor("claude_code", "srvfx")
        self.assertIsNotNone(h_a)
        self.assertEqual(h_a, h_b)
        self.assertRegex(h_a, r"^[0-9a-f]{64}$")

    def test_command_basename_changes_the_hash(self):
        """Proves: the basename is part of the identity (node vs deno differ), so the previous
        test is not passing because everything collapses to one hash.
        Cannot prove: that basename is a sufficient identity for well-known servers."""
        base = {"args": ["server.js"], "env": {}}
        self.assertNotEqual(descriptor_hash({"command": "/a/node", **base}), descriptor_hash({"command": "/a/deno", **base}))

    def test_secret_shaped_and_path_shaped_args_dropped_but_plain_args_kept(self):
        """Proves: a standalone secret-shaped token, a path-shaped token, and the value glued to a
        secret flag (--api-key=...) are dropped, while a plain value (--port 8080 vs 9090) still
        changes the hash. Cannot prove: the secret-shape heuristics catch every token format."""
        plain = descriptor_hash({"command": "node", "args": ["server.js", "--port", "8080"]})
        with_secret = descriptor_hash({"command": "node", "args": ["server.js", FLAG_VALUE, "--port", "8080"]})
        with_path = descriptor_hash({"command": "node", "args": ["server.js", "/srv/data/cfg.json", "--port", "8080"]})
        glued = canonical_descriptor({"command": "node", "args": ["server.js", "--api-key=" + FLAG_VALUE, "--port", "8080"]})
        other_port = descriptor_hash({"command": "node", "args": ["server.js", "--port", "9090"]})
        self.assertEqual(plain, with_secret)
        self.assertEqual(plain, with_path)
        self.assertEqual(glued["args"], ["server.js", "--api-key", "--port", "8080"])
        self.assertNotEqual(plain, other_port)

    def test_codex_config_toml_descriptor(self):
        """Proves: `[mcp_servers.<name>]` in codex_home/config.toml is read into the same canonical
        form as a JSON descriptor (stdio, basename, args, empty env_names) and hashed identically.
        Cannot prove: Codex's real config keys beyond command/args/env/url."""
        codex_home = self.tmp()
        make_codex_home(codex_home)
        via_index = FsIndex(codex_home=codex_home).mcp_descriptor("codex", "srvfx")
        self.assertEqual(via_index, descriptor_hash({"command": "npx", "args": ["-y", "pkg"]}))
        self.assertEqual(canonical_descriptor({"command": "npx", "args": ["-y", "pkg"]}),
                         {"transport": "stdio", "command": "npx", "args": ["-y", "pkg"], "env_names": []})
        self.assertIsNone(FsIndex(codex_home=codex_home).mcp_descriptor("claude_code", "srvfx"))

    def test_url_descriptor_uses_host_class_not_host(self):
        """Proves: a url server becomes transport http with command = scheme class, so two hosts
        on https hash the same and the hostname never enters the preimage.
        Cannot prove: anything about headers (they are never read)."""
        a = canonical_descriptor({"url": "https://one.invalid/mcp", "headers": {"Authorization": "x"}})
        b = canonical_descriptor({"url": "https://two.invalid/mcp"})
        self.assertEqual(a, b)
        self.assertEqual(a["transport"], "http")
        self.assertEqual(a["command"], "https")
        self.assertEqual(canonical_descriptor({"url": "http://three.invalid/"})["command"], "http")


class NameHash(unittest.TestCase):
    def test_name_hash_is_keyed_hmac_not_sha256(self):
        """Proves: name_hash depends on the secret (two secrets differ), is not sha256 of the name
        or of "type:name", and equals an independently computed HMAC-SHA256 over "type:name".
        Cannot prove: the secret is well managed by the caller."""
        h_a = name_hash(SECRET_A, ASSET_SKILL, "skill-ghost")
        h_b = name_hash(SECRET_B, ASSET_SKILL, "skill-ghost")
        self.assertNotEqual(h_a, h_b)
        self.assertNotEqual(h_a, hashlib.sha256(b"skill-ghost").hexdigest())
        self.assertNotEqual(h_a, hashlib.sha256(b"skill:skill-ghost").hexdigest())
        self.assertEqual(h_a, hmac.new(SECRET_A, b"skill:skill-ghost", "sha256").hexdigest())
        self.assertNotEqual(h_a, name_hash(SECRET_A, ASSET_AGENT, "skill-ghost"))


class BomVersion(unittest.TestCase):
    def test_bom_version_is_order_independent_and_set_sensitive(self):
        """Proves: bom_version of the same ids in a different order is identical, and a different
        set gives a different value. Cannot prove: collision resistance beyond sha256's."""
        ids = [HEX64 * 4, ("f" * 64), ("0" * 63) + "1"]
        self.assertEqual(bom_version(ids), bom_version(list(reversed(ids))))
        self.assertEqual(bom_version(ids), hashlib.sha256(",".join(sorted(ids)).encode()).hexdigest())
        self.assertNotEqual(bom_version(ids), bom_version(ids[:2]))


class Settle(TempDirs):
    def test_pending_mcp_completion_folds_into_the_segment(self):
        """Proves: a delta that only adds mcp__S__* tools for an S reported pending by an earlier
        event does not start a new segment, and S becomes a member of the single segment with its
        schema bytes counted. Cannot prove: that every real async-connect delta has this shape."""
        events = [LoadedSetEvent(ts_ms=T, kind="initial", tool_names=["Bash"], pending_mcp=["srvfx"]),
                  LoadedSetEvent(ts_ms=T + 13_000, kind="delta", tool_names=["mcp__srvfx__list"], tool_schema_bytes={"srvfx": 402})]
        ar = attribute.attribute(make_run(events=events), FsIndex(claude_home=self.tmp()), SECRET_A)
        self.assertEqual(len(ar.segments), 1)
        self.assertEqual(ar.segments[0].loaded_set_basis, "harness_log")
        self.assertIn("mcp_server:srvfx", obs_by_name(ar))
        self.assertEqual(obs_by_name(ar)["mcp_server:srvfx"].context_cost_est, (100, "tool_schema_bytes_div4"))

    def test_removal_splits_the_segment(self):
        """Proves: a delta with a removed name starts a new segment at the delta's timestamp; the
        removed server is a member of the first segment and not of the second, and the two
        bom_versions differ. Cannot prove: the removal timing beyond the harness timestamp."""
        events = [LoadedSetEvent(ts_ms=T, kind="initial", tool_names=["mcp__srvfx__list"]),
                  LoadedSetEvent(ts_ms=T + 5_000, kind="delta", removed=["mcp__srvfx__list"])]
        ar = attribute.attribute(make_run(events=events), FsIndex(claude_home=self.tmp()), SECRET_A)
        self.assertEqual([s.index for s in ar.segments], [0, 1])
        self.assertEqual((ar.segments[0].end_ts_ms, ar.segments[1].start_ts_ms), (T + 5_000, T + 5_000))
        self.assertIn("mcp_server:srvfx", obs_by_name(ar, 0))
        self.assertNotIn("mcp_server:srvfx", obs_by_name(ar, 1))
        self.assertNotEqual(ar.segments[0].bom_version, ar.segments[1].bom_version)

    def test_unexplained_addition_or_readd_splits(self):
        """Proves: a delta adding tools of a server never reported pending, and a delta with a
        re-added name, each start a new segment, while a bare initial-kind event never does.
        Cannot prove: that these are the only config changes a harness can make."""
        not_pending = [LoadedSetEvent(ts_ms=T, kind="initial", tool_names=["Bash"]),
                       LoadedSetEvent(ts_ms=T + 1_000, kind="delta", tool_names=["mcp__srvzz__ping"])]
        readded = [LoadedSetEvent(ts_ms=T, kind="initial", tool_names=["Bash"]),
                   LoadedSetEvent(ts_ms=T + 1_000, kind="delta", readded=["Bash"])]
        two_initials = [LoadedSetEvent(ts_ms=T, kind="initial", skills=["skill-ghost"]),
                        LoadedSetEvent(ts_ms=T + 1_000, kind="initial", agent_types=["agent-omega"])]
        fs = FsIndex(claude_home=self.tmp())
        self.assertEqual(len(attribute.attribute(make_run(events=not_pending), fs, SECRET_A).segments), 2)
        self.assertEqual(len(attribute.attribute(make_run(events=readded), fs, SECRET_A).segments), 2)
        self.assertEqual(len(attribute.attribute(make_run(events=two_initials), fs, SECRET_A).segments), 1)


class Binding(TempDirs):
    def _listing(self):
        return [LoadedSetEvent(ts_ms=T, kind="initial", skills=["skill-alpha"], listing_bytes={"skill-alpha": 121})]

    def test_mtime_proven_when_whole_tree_is_older_than_listing(self):
        """Proves: a listed skill whose local dir (every file and the dir entry) has mtime before
        the listing timestamp gets content_hash with binding mtime_proven, and the hash is the
        published tree hash (sorted relpath/sha256 pairs). Cannot prove: that mtime is monotonic
        on the user's filesystem (a copy tool can preserve old mtimes over new content)."""
        home = self.tmp()
        skill = make_claude_home(home)
        set_tree_mtime(skill, T - 60_000)
        ar = attribute.attribute(make_run(events=self._listing()), FsIndex(claude_home=home), SECRET_A)
        key = obs_by_name(ar)["skill:skill-alpha"].key
        self.assertEqual((key.key_basis, key.binding), (KEY_CONTENT, BINDING_MTIME))
        pairs = []
        for fn in ("SKILL.md", "reference.md"):
            with open(os.path.join(skill, fn), "rb") as fh:
                pairs.append([fn, hashlib.sha256(fh.read()).hexdigest()])
        expected = hashlib.sha256(json.dumps(sorted(pairs), sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        self.assertEqual(key.asset_id, expected)

    def test_unproven_when_any_file_is_newer_than_listing(self):
        """Proves: one file touched after the listing timestamp flips the binding to unproven while
        the content_hash stays the same (the hash is of content, the binding is about time).
        Cannot prove: which file changed or whether the change was meaningful."""
        home = self.tmp()
        skill = make_claude_home(home)
        set_tree_mtime(skill, T - 60_000)
        before = obs_by_name(attribute.attribute(make_run(events=self._listing()), FsIndex(claude_home=home), SECRET_A))
        os.utime(os.path.join(skill, "SKILL.md"), ns=((T + 60_000) * 10**6,) * 2)
        after = obs_by_name(attribute.attribute(make_run(events=self._listing()), FsIndex(claude_home=home), SECRET_A))
        self.assertEqual(after["skill:skill-alpha"].key.binding, BINDING_UNPROVEN)
        self.assertEqual(after["skill:skill-alpha"].key.asset_id, before["skill:skill-alpha"].key.asset_id)

    def test_in_band_rules_file_and_skill_body_bind_exactly(self):
        """Proves: an in-band rules file and an invoked skill body (no local dir) both get
        key_basis content_hash, binding harness_log_exact, and asset_id equal to the in-band
        content sha256, so no filesystem read is involved. Cannot prove: that the source hashed
        the right bytes (see the source tests)."""
        rules_sha = hashlib.sha256(b"invented rules text").hexdigest()
        body_sha = hashlib.sha256(b"invented skill body").hexdigest()
        in_band = [InBandAsset(kind="rules_file", name="RULES.md", content_sha256=rules_sha, byte_len=57, ts_ms=T + 3),
                   InBandAsset(kind="skill_body", name="skill-beta", content_sha256=body_sha, byte_len=90, ts_ms=T + 4_000)]
        invs = [InvocationObs(asset_type=ASSET_SKILL, name="skill-beta", ts_ms=T + 4_000)]
        ar = attribute.attribute(make_run(events=self._listing(), invocations=invs, in_band=in_band),
                                 FsIndex(claude_home=self.tmp()), SECRET_A)
        rows = obs_by_name(ar)
        for label, sha in (("rules_file:RULES.md", rules_sha), ("skill:skill-beta", body_sha)):
            self.assertEqual((rows[label].key.key_basis, rows[label].key.binding, rows[label].key.asset_id),
                             (KEY_CONTENT, BINDING_EXACT, sha), label)

    def test_listed_skill_without_local_dir_gets_name_hash(self):
        """Proves: a listed skill with no local tree and no in-band body falls back to the keyed
        name_hash with binding not_applicable, and the id is not a hash of the name alone.
        Cannot prove: the row has any cross-device meaning (it does not, by design)."""
        ar = attribute.attribute(make_run(events=self._listing()), FsIndex(claude_home=self.tmp()), SECRET_A)
        key = obs_by_name(ar)["skill:skill-alpha"].key
        self.assertEqual((key.key_basis, key.binding), (KEY_NAME, BINDING_NA))
        self.assertEqual(key.asset_id, name_hash(SECRET_A, ASSET_SKILL, "skill-alpha"))
        self.assertNotEqual(key.asset_id, hashlib.sha256(b"skill-alpha").hexdigest())


class Observations(TempDirs):
    def setUp(self):
        self.home = self.tmp()
        make_claude_home(self.home)
        self.events = [
            LoadedSetEvent(ts_ms=T, kind="initial", skills=["skill-alpha", "skill-ghost"],
                           listing_bytes={"skill-alpha": 121, "skill-ghost": 83}),
            LoadedSetEvent(ts_ms=T + 1, kind="initial", tool_names=["Bash", "mcp__srvfx__list"], tool_schema_bytes={"srvfx": 402}),
            LoadedSetEvent(ts_ms=T + 2, kind="initial", agent_types=["Explore", "agent-omega"]),
        ]
        self.in_band = [InBandAsset(kind="rules_file", name="RULES.md", byte_len=57, ts_ms=T + 3,
                                    content_sha256=hashlib.sha256(b"invented rules text").hexdigest())]
        self.invocations = [
            InvocationObs(asset_type=ASSET_SKILL, name="skill-alpha", ts_ms=T + 10_000),
            InvocationObs(asset_type=ASSET_MCP_SERVER, name="srvfx", ts_ms=T + 11_000, latency_ms=250),
            InvocationObs(asset_type=ASSET_AGENT, name="agent-omega", ts_ms=T + 12_000, corroborated=True, child_tokens_total=500),
            InvocationObs(asset_type=ASSET_AGENT, name="Explore", ts_ms=T + 13_000),
        ]
        run = make_run(events=self.events, invocations=self.invocations, in_band=self.in_band)
        self.ar = attribute.attribute(run, FsIndex(claude_home=self.home), SECRET_A)
        self.rows = obs_by_name(self.ar)

    def test_every_row_inferred_and_direct_evidence_only_for_invoked(self):
        """Proves: every observation is tier inferred; direct_evidence_available is True exactly
        for the assets with an invocation in the log (alpha, srvfx, agent-omega) and False for
        listed-only ones (ghost, RULES.md); invocations are attached to their asset.
        Cannot prove: the production collector would reach Direct (that is #965's work)."""
        self.assertTrue(all(o.tier == TIER_INFERRED for o in self.ar.observations[0]))
        direct = {label for label, o in self.rows.items() if o.direct_evidence_available}
        self.assertEqual(direct, {"skill:skill-alpha", "mcp_server:srvfx", "agent:agent-omega"})
        self.assertEqual(self.rows["mcp_server:srvfx"].invocations[0].latency_ms, 250)
        self.assertEqual(self.rows["agent:agent-omega"].harness_corroborations, 1)
        self.assertIsNone(self.rows["skill:skill-alpha"].harness_corroborations)

    def test_context_cost_methods_per_type(self):
        """Proves: skills use listing_bytes//4 (listing_bytes_div4), rules files byte_len//4
        (file_bytes_div4), MCP servers tool_schema_bytes//4 (tool_schema_bytes_div4), agents None.
        Cannot prove: that bytes/4 approximates any tokenizer."""
        self.assertEqual(self.rows["skill:skill-alpha"].context_cost_est, (30, "listing_bytes_div4"))
        self.assertEqual(self.rows["skill:skill-ghost"].context_cost_est, (20, "listing_bytes_div4"))
        self.assertEqual(self.rows["rules_file:RULES.md"].context_cost_est, (14, "file_bytes_div4"))
        self.assertEqual(self.rows["mcp_server:srvfx"].context_cost_est, (100, "tool_schema_bytes_div4"))
        self.assertIsNone(self.rows["agent:agent-omega"].context_cost_est)

    def test_builtin_agent_types_are_not_assets(self):
        """Proves: a builtin agent type appearing in both the listing and an invocation produces
        no observation, no key, and no name_map entry, while the custom agent does.
        Cannot prove: the run-level subagent count still includes it (extract's job)."""
        labels = set(self.rows)
        self.assertNotIn("agent:Explore", labels)
        self.assertIn("agent:agent-omega", labels)
        self.assertFalse(any(v.endswith(":Explore") for v in self.ar.name_map.values()))

    def test_local_agent_file_and_descriptor_keys(self):
        """Proves: an agent with a local agents/<type>.md gets a content_hash of that file (binding
        by the mtime rule), and an MCP server with a descriptor gets descriptor_hash with
        not_applicable. Cannot prove: the harness loaded that agent file rather than another
        scope's copy."""
        agent_key = self.rows["agent:agent-omega"].key
        with open(os.path.join(self.home, "agents", "agent-omega.md"), "rb") as fh:
            self.assertEqual(agent_key.asset_id, hashlib.sha256(fh.read()).hexdigest())
        self.assertEqual(agent_key.key_basis, KEY_CONTENT)
        self.assertIn(agent_key.binding, (BINDING_MTIME, BINDING_UNPROVEN))
        mcp_key = self.rows["mcp_server:srvfx"].key
        self.assertEqual((mcp_key.key_basis, mcp_key.binding), (KEY_DESCRIPTOR, BINDING_NA))

    def test_name_map_contains_every_asset_id(self):
        """Proves: every asset_id in every segment's keys and observations is in name_map with the
        "<type>:<name>" form, every asset_id is hex64, and bom_version equals the sha256 of the
        sorted ids. Cannot prove: name_map never egresses (aggregate's job)."""
        for seg in self.ar.segments:
            ids = [k.asset_id for k in seg.asset_keys]
            self.assertEqual(seg.bom_version, hashlib.sha256(",".join(sorted(ids)).encode()).hexdigest())
            for key in seg.asset_keys:
                self.assertRegex(key.asset_id, r"^[0-9a-f]{64}$")
                self.assertEqual(self.ar.name_map[key.asset_id], f"{key.asset_type}:{key.name}")
            self.assertEqual(sorted(ids), sorted(o.key.asset_id for o in self.ar.observations[seg.index]))
        self.assertEqual(len(self.ar.name_map), len(self.ar.segments[0].asset_keys))

    def test_codex_without_listing_uses_filesystem_basis(self):
        """Proves: a Codex run (no loaded events) gets a single segment with basis filesystem
        seeded from config.toml, the invoked server keyed by descriptor_hash; and a run with no
        events and an empty index has basis none. Cannot prove: the bom was stable over the run."""
        codex_home = self.tmp()
        make_codex_home(codex_home)
        run = make_run(harness="codex", invocations=[InvocationObs(asset_type=ASSET_MCP_SERVER, name="srvfx", ts_ms=T + 500)])
        ar = attribute.attribute(run, FsIndex(codex_home=codex_home), SECRET_A)
        self.assertEqual([s.loaded_set_basis for s in ar.segments], ["filesystem"])
        self.assertEqual(obs_by_name(ar)["mcp_server:srvfx"].key.key_basis, KEY_DESCRIPTOR)
        empty = attribute.attribute(make_run(harness="codex"), FsIndex(codex_home=self.tmp()), SECRET_A)
        self.assertEqual(empty.segments[0].loaded_set_basis, "none")
        self.assertEqual(empty.observations, {0: []})


if __name__ == "__main__":
    unittest.main()
