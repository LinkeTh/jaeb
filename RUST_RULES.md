# Rust 2024 — Idiomatic Code & Architecture Guidelines

## Edition & Toolchain

- Target `edition = "2024"` in `Cargo.toml`; use `rust-analyzer` + `clippy` + `rustfmt` in CI
- Run `cargo clippy -- -D warnings` to treat lints as errors; never suppress without a comment explaining why

---

## Ownership & Borrowing

- Prefer borrowing (`&T`, `&mut T`) over cloning; clone only at explicit boundaries (e.g., crossing thread or async task boundaries)
- Return owned types from constructors and transforming functions; accept borrowed types in read-only functions

```rust
// ✅ Accept a slice, return an owned Vec
fn deduplicated(items: &[String]) -> Vec<String> { … }

// ❌ Unnecessary clone in signature
fn process(items: Vec<String>) -> Vec<String> { … }
```

- Use `Cow<'_, str>` when a function sometimes needs to allocate and sometimes doesn't

---

## Error Handling

- Define a crate-level error type with `thiserror`; never expose `Box<dyn Error>` in public APIs

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found: {id}")]
    NotFound { id: u64 },
}
```

- Use `anyhow::Result` only in binaries and test code, never in library public APIs
- Propagate with `?`; avoid `.unwrap()` outside tests — use `.expect("reason")` where a panic is truly unrecoverable
- Model absence with `Option`, not sentinel values (`-1`, `""`)

---

## Types & Traits

- Prefer newtypes over raw primitives to encode invariants in the type system

```rust
struct UserId(u64);
struct EmailAddress(String); // validate on construction
```

- Implement `From`/`Into` rather than bespoke `to_x()` conversion methods
- Derive `Debug`, `Clone`, `PartialEq` on data types by default; derive `Copy` only when the type is trivially cheap to copy
- Use `#[non_exhaustive]` on public enums and structs to keep semver flexibility

---

## Iterators & Functional Style

- Chain iterator adapters instead of imperative loops; collect only at the final step

```rust
// ✅
let totals: Vec<u64> = orders
    .iter()
    .filter(|o| o.is_paid())
    .map(|o| o.amount)
    .collect();

// ❌
let mut totals = vec![];
for o in &orders {
    if o.is_paid() { totals.push(o.amount); }
}
```

- Use `flat_map`, `filter_map`, and `chain` instead of nested loops + conditionals
- Avoid `collect::<Vec<_>>()` followed immediately by another `.iter()` — keep the pipeline lazy

---

## Structs & Encapsulation

- Make fields private by default; expose via methods or the builder pattern for complex construction
- Use the **builder pattern** for structs with many optional fields; implement it with owned `self` mutation

```rust
let req = RequestBuilder::new("https://example.com")
    .timeout(Duration::from_secs(5))
    .header("Accept", "application/json")
    .build()?;
```

- Prefer **composition over inheritance**; share behaviour through traits, not deep struct embedding

---

## Concurrency

- Lean on the type system: `Send + Sync` bounds surface data-race issues at compile time
- Use `Arc<Mutex<T>>` only when shared mutable state is unavoidable; prefer message passing (`mpsc`, `tokio::sync::mpsc`) otherwise
- In async code, avoid blocking inside `.await` contexts — offload with `tokio::task::spawn_blocking`
- Keep `async fn` in traits via the `#[trait_variant::make]` pattern (stable in 2024) or `async-trait` crate for broader compatibility

---

## Module & Crate Architecture

- One concept per module; name modules as nouns (`user`, `auth`, `db`) not verbs
- Re-export the public API from `lib.rs` or a top-level `mod.rs`; keep internal modules private
- Split large crates into a **workspace** with focused sub-crates (`core`, `cli`, `server`) to improve build times and enforce boundaries
- Feature-flag optional dependencies with `[features]` rather than conditional logic in code

---

## Performance

- Profile before optimising (`cargo flamegraph`, `criterion` benchmarks)
- Avoid premature allocation: prefer `&str` over `String`, slices over `Vec` in hot paths
- Use `#[inline]` sparingly; trust the compiler — add it only when benchmarks justify it
- Prefer stack allocation; box only when sizes are unknown at compile time or for trait objects

---

## Testing

- Unit-test in the same file using `#[cfg(test)] mod tests { … }`
- Integration tests live in `tests/`; they test the public API only
- Use `#[should_panic(expected = "…")]` to assert panic messages, not just that a panic occurred
- Property-test invariants with `proptest` or `quickcheck` for data-transformation functions

---

## Clippy Lints Worth Enabling Globally

```toml
# .clippy.toml or Cargo.toml [lints] section
[lints.clippy]
pedantic    = "warn"
unwrap_used = "deny"
clone_on_ref_ptr = "warn"
```
