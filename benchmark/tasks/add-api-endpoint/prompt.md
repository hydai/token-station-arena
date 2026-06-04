You are working in a small Rust workspace with a `catalog-core` library crate and a `catalog-api` Axum crate.

Task:

- Add `GET /products/top?limit=<n>` to the Axum app.
- Return JSON products sorted by descending popularity.
- Respect the optional `limit` query parameter. If it is missing, return all products.
- Reuse existing catalog-core logic where possible.
- Do not remove or weaken the integration test.

Before finishing, run:

```bash
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
```
