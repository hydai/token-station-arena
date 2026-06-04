You are working in a small Rust workspace.

Task:

- Refactor the duplicated discount calculation in `catalog-core/src/pricing.rs`.
- Introduce one shared helper for computing the discount amount.
- Preserve all public function names, signatures, and behavior.
- Keep the code simple enough that future pricing rules can reuse the helper.
- Do not remove or weaken tests or the custom refactor check.

Before finishing, run:

```bash
cargo test
cargo check
cargo clippy --all-targets -- -D warnings
node scripts/check-refactor.mjs
```
