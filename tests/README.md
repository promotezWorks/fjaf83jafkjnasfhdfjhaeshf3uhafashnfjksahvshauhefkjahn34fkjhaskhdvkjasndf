# Integration Tests

Unit tests for internal logic live inline in `src/` (see `src/config.rs`, `src/stoat.rs`).

This directory is for integration tests that exercise the agent end to end against a live or
mocked Stoat session. Because `stoat-rat` is a binary crate, integration tests here cannot import
its modules directly; add reusable logic to a future `lib.rs` before writing cross-module tests.
