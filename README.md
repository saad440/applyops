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

## Configuration

Configuration is read from environment variables with safe local defaults:

| Variable | Default | Purpose |
| --- | --- | --- |
| `APPLYOPS_BIND_ADDR` | `127.0.0.1:8080` | Address the server listens on |
| `APPLYOPS_DB_PATH` | `data/applyops.db` | SQLite database file |
| `RUST_LOG` | `info` | Log filtering |

ApplyOps in its current form is intended for local use and has no authentication. Avoid binding it to an interface that is reachable by others. A warning is logged if you do.
