# ADR-0001: Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-11-18
- **Sources:** this record

## Context

Warpine's architectural decisions were recorded in `CLAUDE.md`, the developer guide, the reference manual and commit messages. The reasons for a decision were hard to find later, and a change could undo a decision without anyone noticing.

## Decision

Each architectural decision gets an Architecture Decision Record in `doc/adr/`, in the format of Michael Nygard's "Documenting Architecture Decisions": Status, Date, Sources, Context, Decision, Consequences.

- Files are named `NNNN-short-title.md` and numbered in order; numbers are never reused.
- A record is not rewritten when a decision changes: a new record supersedes it, and the old one's status becomes "Superseded by ADR-NNNN".
- Records 0002 onwards were written on 2026-11-18 from the project's documents and history; their dates are those of the first commit that put the decision in place, given under Sources.
- `doc/adr/README.md` lists every record.

## Consequences

- A decision's context and trade-offs stay with the code.
- The developer guide and `CLAUDE.md` keep describing how things work; ADRs say why.
- A new architectural decision is not complete until its ADR exists.
