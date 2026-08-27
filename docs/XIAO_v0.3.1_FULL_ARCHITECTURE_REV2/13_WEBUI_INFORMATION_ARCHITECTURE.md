# 13 — WebUI Information Architecture

## Role

Xiao Manager is a visual control plane over xiaod. It does not own agent state.

## Main tabs

### Explore
- health summary;
- current session AI;
- sessions count;
- runs count;
- memory count;
- runtime status.

### Tools / Management
- Agent Settings;
- Runtime;
- Context;
- Attachments;
- Security;
- Diagnostics;
- Logs.

### Settings
- Telegram;
- Custom Profiles;
- Models;
- Appearance;
- About.

## Subpages

### Agent Settings
Controls:
- max turns;
- max tool calls;
- runtime timeout;
- no-progress threshold;
- provider streaming;
- read-only parallelism;
- max parallel reads;
- execution plan;
- plan cache;
- background learning.

Show note: setting changes apply to new runs; active runs keep their snapshot.

### Custom Profiles
For each profile:
- alias;
- endpoint;
- protocol;
- reachability;
- API key configured yes/no;
- header names only;
- discovered models;
- edit/discover/delete.

### Model detail
- exact profile/model;
- protocol readiness;
- native tools;
- structured output;
- continuation;
- vision;
- file input;
- streaming;
- evidence source/time;
- optional Probe;
- capability override controls.

Selection must target an explicit session.

### Sessions
- list;
- create;
- use;
- rename;
- delete/archive if supported;
- detail;
- Change AI dialog.

Do not silently use the first session as target.

### Runs
Show:
- status;
- goal;
- provider/model;
- verification;
- tool calls;
- dependency installs;
- timing waterfall;
- cache events;
- cancel when running.

### Appearance
Three-state theme:
- System;
- Light;
- Dark.

System follows `prefers-color-scheme` live until user selects an explicit override.

## Android UX

- safe-area top/bottom;
- Android back navigates subpage stack;
- root double-back may exit;
- reduced-motion respected;
- touch feedback;
- no network dependency required to render.
