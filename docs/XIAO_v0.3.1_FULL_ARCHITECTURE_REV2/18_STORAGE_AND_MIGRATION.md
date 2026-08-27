# 18 — Storage and Migration

## Existing v0.3.1 schema direction

Migration 26→27 includes durable tables for:
- provider capability evidence;
- learning jobs;
- tool run substeps;
- agent run events.

Preserve existing sessions, messages, memory, skills, profiles, and attachments.

## Additional schema changes

Avoid adding a new migration unless required. If needed for:
- delivery ACK metadata;
- cache persistence;
- new timing constraints;

create a new idempotent migration and test upgrade from schema 26 and 27.

## Learning jobs

Required fields/concepts:
- pending delivery;
- delivered_at;
- claim/lease;
- retries;
- final status;
- error summary.

## Capability evidence

Scope by exact:
- profile;
- model;
- protocol;
- capability.

Store:
- automatic state;
- owner override;
- source;
- observed_at.

## Cache persistence

Plan/script cache may be in-memory for initial optimization only if process lifetime reuse is sufficient and tests prove production use. Durable cache is optional, but file-backed scripts require auditable manifests.

## Migration test

Test:
1. create schema-26 representative data;
2. upgrade;
3. verify all old rows;
4. verify new tables;
5. reopen DB;
6. verify evidence/learning survive restart.
