# Existing issue editing v1

Status: accepted from the reviewed editing contract, 2026-11-11.

This specification extends native `update` to title, description, priority and
issue type. It supersedes the creation-only rules for those fields in the native
field guide, bulk manifest v1 and ADR-007. Existing lifecycle, claim and content
protection rules still apply.

## Command and text inputs

```text
bead update ID --title TEXT
bead update ID --description TEXT | --description-file PATH | --clear-description
bead update ID --priority N --reason TEXT
bead update ID --issue-type TEXT
```

These fields may be combined with each other and with the existing status,
assignee, clear-assignee, notes, append-notes and notes-file arguments. One
invocation is one atomic edit and increments the issue revision at most once.
`--if-revision` and `--fencing-token` guard the complete edit.

Description input accepts exactly one of inline text, a UTF-8 file, or the clear
flag. A file path of `-` reads stdin. Description and notes files may be supplied
together; at most one may consume stdin. Resolve text before scanning or opening
the mutation transaction. Preserve exact Unicode, newlines and leading hyphens.
An empty description or `--clear-description` stores an empty string. Empty
notes retain the existing clear behavior.

Titles contain 1–4096 UTF-8 bytes. Description and notes contain at most 4 MiB.
Priority is an integer from 0 through 4. Issue type is a nonempty string; native
editing accepts custom types. Priority requires a nonblank reason. A reason
without a priority is a usage error. Existing content checks cover all incoming
text, including file contents and priority audit attribution.

## Atomicity and preservation

The write transaction starts before reading the issue or validating its revision
and claim. Any invalid field, transition, stale revision, credential conflict or
resource conflict rolls back fields, timestamps, revisions and audit events.
The issue ID, creation time, unknown extensions and related labels, dependencies,
comments, references and structured data survive scalar edits.

Scalar metadata can be corrected on a closed issue while retaining its status,
closed time and close reason. Closing, reopening and release keep their dedicated
commands. A generic update cannot bypass their lifecycle metadata or claim rules.
System identifiers, timestamps, revisions, claim epochs and audit records are
managed by the program.

Labels and dependencies retain their dedicated add/remove commands. Dependency
direction remains blocked issue first, blocker second. The supported native kinds
are `blocks`, `relates_to` and `verifies`.

## Priority audit and no-op behavior

A real priority change records a distinct `priority_changed` event containing
`old_priority`, `new_priority`, `actor`, `reason`, `revision` and
`changed_fields: ["priority"]`. Actor comes from `BEAD_ACTOR`, falling back to
`system`, using the existing attribution validation. The audit actor column also
carries that actor so the public history projection identifies it.

The ordinary `updated` or `assignment_cleared` event describes the complete edit
at the same revision. `priority_changed` is additional audit information and does
not represent a second issue revision. Supplying the existing priority emits no
priority event. A request that supplies only unchanged fields changes no issue
row, timestamp, revision or event. This includes repeated status, assignee and notes values, so
commit and dry-run agree on the existing idempotence contract. Claim and revision
checks still run before accepting a no-op.

## Dry-run and manifest v1

CLI dry-run executes the same guarded operation inside a transaction, projects
the resulting fields and rolls it back. It validates revision, claims, lifecycle
and content exactly as commit does. Its before/after projection includes title,
description, priority, issue type and notes.

Manifest v1 `update` accepts `title`, `description`, `clear_description`,
`priority`, `reason` and `issue_type` alongside its existing fields. Empty
description clears it; `clear_description` conflicts with an explicit description.
File/stdin transports are CLI conveniences; manifests carry text directly.
Priority and reason rules are the same as ordinary update. Per-operation fencing
credentials and claim overrides remain unavailable in manifest v1. Existing
claim ownership therefore cannot be bypassed by composing an edit in a manifest.
All manifest operations, including scalar edits and dependency changes, either
commit together or roll back together. Dry-run uses that same transaction and
rolls it back.

## Capability negotiation

`bead capabilities` advertises this additive contract:

```json
{
  "priority_change": true,
  "issue_editing": {
    "version": 1,
    "fields": ["title", "description", "priority", "issue_type", "notes", "status", "assignee"],
    "clear_description": true,
    "revision_guard": true,
    "priority_reason_required": true
  }
}
```

Consumers must negotiate this document before enabling new fields. The schema
guide and command help describe the same fields, guards and priority audit rule.
