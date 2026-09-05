# Jellyfin Rust contributor guide

## Scope

- This repository reimplements the Jellyfin server in Rust while preserving compatibility with the official Jellyfin API and web client.
- The checked-out official server source in `jellyfin/` is the behavioral reference. Prefer matching its externally visible behavior, defaults, validation, authorization, ordering, and error semantics over inventing new behavior.
- Current optimization priorities are media-library management and scanning, users and policies, metadata scraping/providers, and PostgreSQL-backed data access.
- Do not work on Live TV unless a task explicitly asks for it. Avoid incidental changes under `src/jellyfin-live-tv`.

## Working practices

- Read the relevant Rust implementation and its official C# counterpart before changing behavior. Record important parity assumptions in tests or focused comments.
- Keep changes small and independently reviewable. Complete one coherent fix, run its focused tests, and commit it before starting another fix.
- Preserve unrelated user changes and existing commits. Never rewrite history or use destructive Git commands.
- Prefer bounded concurrency, streaming or pagination, batched PostgreSQL operations, and short-lived buffers for library scans. Do not collect an entire library into memory when work can be processed incrementally.
- Keep filesystem watcher queues bounded and deduplicated. Coalesce changed paths by virtual library before scanning, reuse one short-lived directory snapshot for sibling media discovery, and batch PostgreSQL reads and writes instead of issuing per-item queries.
- Keep database invariants in PostgreSQL where practical (constraints, indexes, atomic upserts, transactions), while keeping domain rules explicit in Rust.
- Avoid N+1 queries. Use set-based queries or bounded batches, and add migrations for indexes or constraints required by new query patterns.
- Project inherited images for an item page with one batched DTO-image lookup. Do not call the image projector once per item.
- Coordinate remote-image downloads by URL so concurrent items share one bounded download. Validate that upstream content is an image, and remove or otherwise suppress permanently invalid remote references according to official behavior.
- Treat passwords, access tokens, API keys, and deployment credentials as secrets. Do not log or commit them.

## Compatibility expectations

- Match official Jellyfin DTO field names, nullability, defaults, HTTP status codes, authorization requirements, sorting, pagination, and case-insensitive matching.
- ASP.NET route, query-name, and JSON-property binding is case-insensitive. Compatibility tests must cover PascalCase, camelCase, and representative lowercase legacy requests; do not assume an Axum route or Serde field is equivalent merely because one casing works.
- Follow the official `JsonDefaults` value semantics. Where it permits them, accept numeric strings and case-insensitive or integer enum representations, and mirror the full official parameter set when implementing a legacy endpoint.
- Treat generated SDK models as executable compatibility specifications alongside the C# DTOs. Swift `Codable` rejects the entire enclosing item or page when one nested object, enum, dictionary value, or date has the wrong wire shape.
- Audit DTOs recursively: preserve object-array shapes, serialize API enums by their official names, keep string dictionaries string-valued, and emit full API `DateTime` values rather than storage-only dates.
- Preserve unknown or optional metadata where the official server does; a partial provider response must not erase valid existing metadata.
- Metadata providers must have deterministic priority and merge behavior. Network calls need timeouts, bounded concurrency, and useful error context.
- Cancellation of scans and refreshes must promptly stop new work, release locks/permits, and leave the database in a consistent state.

## Validation

Run the narrowest relevant checks while iterating, then broaden validation before committing:

```bash
cargo fmt --all -- --check
cargo test -p <affected-crate>
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Some `jellyfin-data` integration tests require PostgreSQL and create temporary databases whose names begin with `jellyfin_`. Do not point those tests at a database containing user data.

For scan-memory work, include a repeatable large-directory or synthetic-library measurement when possible. Report baseline, peak, 60-second, and 300-second post-scan values. Separate process RSS and anonymous memory (`RssAnon` or `smaps_rollup` Anonymous) from cgroup `file` and `inactive_file`; metadata image page cache is reclaimable and must not be reported as a Rust heap leak. Also report whether memory returns after the scan, and do not infer a leak from allocator-retained RSS alone.

## Deployment verification

- The deployment checkout is `/home/lqs/jellyfin-rust` on the configured test host. Inspect its current state before changing it; do not assume the local workspace path is valid remotely.
- Deploy only committed revisions. Record the revision tested and verify health, relevant API behavior, scan completion/cancellation, and service logs.
- Do not delete databases, media, configuration, containers, or volumes during deployment validation unless the user explicitly requests it.

## Commit style

- One coherent, tested change per commit.
- Use concise imperative subjects with a conventional prefix when appropriate, for example `fix: bound library scan buffering` or `perf: batch item upserts`.
- Do not include generated build output, credentials, local logs, or deployment-only files.
