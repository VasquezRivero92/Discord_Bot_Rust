# MACHUPEEK

Incremental migration of the Machupeek Discord bot and web from Node.js to Rust.

- `crates/common`: shared types and utilities.
- `crates/data`: data access layer (Firestore).
- `crates/web`: axum web server (`web::app()` builds the router).

Run the tests with `cargo test --workspace`. Lint with
`cargo clippy --workspace -- -D warnings` and `cargo fmt --check`.

Copy `.env.example` to `.env` and fill in values (never commit `.env`).

The legacy Node.js implementation lives in `D:\Discord_Faceit` and is read-only reference.
