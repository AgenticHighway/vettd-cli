# C4 Level 1 — System Context

Shows how the **vettd** scanner relates to external actors and systems.

```mermaid
flowchart TD
    User["👤 Operator / Security Reviewer\n(runs scans and reviews results)"]
    Terminal["🖥️ Local Terminal UX\nwizard, progress, reports,\npost-scan next steps"]
    Vettd["🔍 vettd Scanner\n(Rust CLI — detects, analyzes,\nand reports AI execution artifacts)"]
    Backend["🌐 Compatible Backend\n(optional ingest + hosted review UI)\ncurrent hosted example: vettd remote"]
    Contract["📄 Contract Endpoint\n(compatible /api/contract\nfor version negotiation)"]
    ReleaseAPI["📄 Hosted Release Metadata API\nmanifest + signature endpoints"]
    Releases["📦 Release Artifact Host\nGitHub Releases archives"]
    Browser["🌍 Hosted Review UI\n(optional browser workflow)"]
    FS["💻 Target Machine Filesystem\n(AI config files, prompts,\nMCP configs, containers, rules)"]

    User -->|"runs commands"| Terminal
    Terminal -->|"invokes"| Vettd
    Vettd -->|"reads files & directories"| FS
    Vettd -->|"renders local results"| Terminal
    Vettd -->|"POST /api/scans/ingest\n(Bearer token auth)"| Backend
    Vettd -->|"GET /api/contract\n(version negotiation)"| Contract
    Contract -.->|"exposed by"| Backend
    Vettd -->|"GET latest + signature"| ReleaseAPI
    ReleaseAPI -.->|"currently served by"| Backend
    Vettd -->|"downloads platform archive"| Releases
    User -->|"reviews hosted results"| Browser
    Browser -->|"connects to"| Backend
```

## Key Relationships

| From  | To                  | Protocol           | Purpose                                           |
| ----- | ------------------- | ------------------ | ------------------------------------------------- |
| User  | Local Terminal UX   | CLI (stdin/stdout) | Run scans, inspect local results, choose next steps |
| vettd | Filesystem          | OS read            | Discover and analyze AI artifacts                 |
| vettd | Compatible Backend  | HTTPS POST         | Submit scan contract payloads                     |
| vettd | Contract Endpoint   | HTTPS GET          | Contract version negotiation                      |
| vettd | Release Metadata API | HTTPS GET         | Fetch signed update metadata                      |
| vettd | Release Artifact Host | HTTPS GET        | Download platform archives for self-update        |
| User  | Hosted Review UI    | Browser            | Review submitted results in a backend UI          |
