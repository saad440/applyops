# ApplyOps

ApplyOps is a local-first opportunity and application tracker, built with Rust, SQLite, and server-rendered HTML.

The 2011 me would have written a perfectly sensible desktop application. The 2026 me, having duly considered the project's primary long-term objective (see below), deemed a full-stack architecture more appropriate.

## Why ApplyOps?

Keeping track of opportunities, deadlines, applications, and follow-up obligations can become very tedious. ApplyOps is intended to bring that information together in a lightweight, locally hosted application.

The project is currently single-user and under active development, with an architecture that leaves room for future expansion.

**The long-term objective is to make this software obsolete for its author.**

## Requirements

- A Rust toolchain with `cargo`, supporting the 2024 edition (Rust 1.85 or newer).

## Running

```sh
cargo run
```

Then open <http://127.0.0.1:8080>.

On first start the application creates an SQLite database at `data/applyops.db` and applies the SQL migrations from `migrations/`.

## Usage

1. Pick **Add an opportunity** to record a lead. Organization, title, stream,
   and stage are required; deadline, location, link, and notes are optional
   and only stored when you enter them.
2. The home page lists all opportunities, newest first. The search box
   filters by organization, title, notes, or stream.
3. Open a lead to see its details and use **Edit** to update it — for example
   to move it along its stages: `discovered` → `researching` → `preparing`,
   and `archived` once you stop pursuing it. Archived leads stay listed and
   remain editable.
4. Streams are free-form labels: the form suggests a few built-in defaults
   and remembers every new label you use.

## Configuration

Configuration is read from environment variables with safe local defaults:

| Variable | Default | Purpose |
| --- | --- | --- |
| `APPLYOPS_BIND_ADDR` | `127.0.0.1:8080` | Address the server listens on |
| `APPLYOPS_DB_PATH` | `data/applyops.db` | SQLite database file |
| `APPLYOPS_TIME_ZONE` | `Europe/Berlin` | IANA name of the time zone that countdowns and overdue labels are measured in (for example `Europe/Berlin`, `America/New_York`); an unresolvable name falls back to UTC |
| `RUST_LOG` | `info` | Log filtering |

ApplyOps in its current form is intended for local use and has no authentication. Avoid binding it to an interface that is reachable by others. A warning is logged if you do.
