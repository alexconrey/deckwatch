# Backend test integration

The `.rs` files in this directory are staged unit tests for the deckwatch
Rust backend. Because Rust modules can only import module-private items
from within the same crate module tree, they need to be wired into the
source files under `src/` — you can either **(a)** copy the test file next
to its target and reference it via `#[path]`, or **(b)** paste the
`#[cfg(test)] mod tests { ... }` body verbatim into the target file.

Recommended layout: keep the test files where they are (staged) and add
one line to each source file:

## Wiring

| Test file | Add to | One-line snippet |
|---|---|---|
| `kube_ext_tests.rs` | end of `src/kube_ext.rs` | `#[cfg(test)] #[path = "kube_ext_tests.rs"] mod tests;` |
| `error_tests.rs` | end of `src/error.rs` | `#[cfg(test)] #[path = "error_tests.rs"] mod tests;` |
| `config_tests.rs` | end of `src/config.rs` | `#[cfg(test)] #[path = "config_tests.rs"] mod tests;` |
| `state_tests.rs` | end of `src/state.rs` | `#[cfg(test)] #[path = "state_tests.rs"] mod tests;` |
| `handlers_deployments_tests.rs` | end of `src/handlers/deployments.rs` | `#[cfg(test)] #[path = "../../tests/handlers_deployments_tests.rs"] mod tests;` |
| `handlers_addons_tests.rs` | end of `src/handlers/addons.rs` | `#[cfg(test)] #[path = "../../tests/handlers_addons_tests.rs"] mod tests;` |

For the handler tests, copy them under a top-level `tests/` directory in
the deckwatch source tree (or adjust the `#[path]` accordingly).

## Dependencies

The tests use only crates already present in `Cargo.toml` (`kube`,
`k8s-openapi`, `axum`, `tokio`, `serde_json`). No `[dev-dependencies]`
additions are required.

## Running

```bash
cargo test --lib          # backend unit tests only
cargo test --workspace    # everything
cargo test kube_ext       # filter by module name
```
