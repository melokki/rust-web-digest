# Source Registry Policy

The source registry is deliberately curated. The collector should prefer a small number of reliable, high-signal sources over broad crawling.

## Source classes

### Project sources

Tracked projects provide structured signals through:

- GitHub Releases
- crates.io versions
- RustSec/OSV advisories

These sources are configured under `[[projects]]` in `config/sources.toml`.

### Article feeds

Article feeds provide explanatory context that structured release sources often lack. They are configured under `[[feeds]]`.

A feed is accepted when:

1. the feed URL is published by the source itself;
2. the source is official or has a strong technical track record in the Rust ecosystem;
3. the content can add context that release metadata alone does not provide;
4. relevance can be bounded with explicit inclusion and exclusion terms;
5. the source does not merely duplicate another feed already in the registry.

## Current feed registry

### Rust Blog

Publisher: Rust Project

Purpose: stable-language, Cargo, WebAssembly, security, networking, and ecosystem announcements that may affect Rust web developers.

Filtering: conservative Rust-web keywords, with governance/community-program noise exclusions.

### Inside Rust

Publisher: Rust Project

Purpose: implementation and project-team developments relevant to async Rust, networking, WebAssembly, runtime internals, and related foundations.

Filtering: Rust-web and async-specific keywords, with meeting/triage noise exclusions.

### without.boats

Publisher: without.boats

Purpose: high-signal writing on async Rust foundations, futures, streams, executors, pinning, tasks, and concurrency.

Filtering: async/runtime/networking terms only.

### baby steps

Publisher: Niko Matsakis

Purpose: Rust language and async design writing that can affect server-side and async Rust architecture.

Filtering: async/runtime/networking/web-service terms only.

## Filtering semantics

For each feed:

- `excluded_any` is evaluated first;
- if any excluded term appears in the title, summary, or feed content, the entry is rejected;
- if `required_any` is empty, the remaining entry is accepted;
- otherwise at least one required term must appear in the title, summary, or feed content.

Do not add `rust` as a required keyword to a Rust-only source. It removes nearly all filtering value.

## Sources intentionally not included yet

Some project websites publish useful articles but do not currently have a feed endpoint confirmed in the registry work. Do not guess feed URLs. Until a publisher-provided RSS/Atom endpoint is verified, those projects remain covered through GitHub Releases, crates.io, advisories, and any confirmed related source that reconciliation can attach.

This currently applies to project-blog coverage such as Tokio, Dioxus, and Leptos.

## Adding a source

Before adding a feed:

1. verify the endpoint from the publisher website;
2. inspect several recent entries;
3. define a narrow `required_any` list;
4. define `excluded_any` terms for predictable noise;
5. run the collector over a historical window;
6. inspect accepted and rejected entries manually;
7. only then merge the registry change.

The goal is not to collect everything. The goal is to produce a small, explainable candidate set that can support a useful monthly digest.
