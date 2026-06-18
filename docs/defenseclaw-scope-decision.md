# DefenseClaw Scope Decision

This document records the final scope decision for the remaining DefenseClaw
families after the bounded source-analysis work landed in vettd.

## Approved And Implemented

These DefenseClaw-derived families fit vettd's mission as a local AI execution
inventory and bounded risk surface scanner, and are now in scope:

- structured secret detection in prompt, instruction, and config content
- prompt/content SSRF and cognitive-tampering patterns
- bounded JSON config heuristics for embedded credentials and suspicious destinations
- bounded source heuristics for dynamic imports, non-literal `require()`, and non-literal process execution
- bounded source heuristics for network-context private or internal SSRF targets
- bounded source heuristics for sensitive local credential-path access
- bounded source heuristics for cognitive or identity file targeting and write operations

## Explicit Non-Goals

The following remaining DefenseClaw families are out of scope for vettd's
current mission and should not be imported without a new product decision:

- PII detection
Reason: vettd inventories AI execution artifacts and bounded execution-risk signals, not general data-loss or privacy classification.

- vulnerability or codeguard-style rules
Reason: these shift vettd toward a generic SAST or code-vulnerability scanner, which does not fit the current artifact-first contract or CLI scope.

- malware signatures, reverse-shell signatures, or broad payload-pattern scanning
Reason: these families are better handled by dedicated malware, EDR, or runtime guardrail systems and would create noisy overlap in a filesystem inventory tool.

- broader standalone exfiltration, C2, and DNS-tunneling destination rules outside the current bounded JSON and source-context heuristics
Reason: without stronger runtime context, these rules become generic malware/exfiltration scanning rather than a focused AI execution inventory signal.

## Decision Summary

There is no remaining unclassified DefenseClaw import bucket for vettd.

- Approved families are implemented in the current source, config, and prompt analysis layers.
- Deferred families are explicit non-goals until a future product review changes vettd's mission.
- No follow-on issue is approved right now for additional DefenseClaw corpus imports.