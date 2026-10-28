# ApplyOps

ApplyOps is a local-first, single-user opportunity and application tracker.
It is a server-rendered Rust web application backed by SQLite.

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
| `RUST_LOG` | `info` | Log filtering |

ApplyOps in its current form is intended for local use and has no authentication. Avoid binding it to an interface that is reachable by others. A warning is logged if you do.
