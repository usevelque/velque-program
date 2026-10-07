# Contributing

Issues and pull requests are welcome. A few things keep this repository honest, so please follow them.

## Build and test

```bash
cargo build-sbf --tools-version v1.54
cd tests && cargo run --release --locked
```

The toolchain is Agave 4.1.2 with platform-tools v1.54. The same commands run in CI on every pull request, and every check must print `PASS`.

## Rules for changes

- **The binary is checked against devnet every day.** A change to `src/` changes the binary, so the `on-chain binary` workflow stays red until the program is upgraded from that commit. Say in the pull request that an upgrade is needed.
- **Account layouts are fixed offsets.** If you move or add a field in `src/state.rs`, bump the tag of that account so old accounts are rejected instead of misread, and update the tables at the top of the file.
- **The clearing rule lives in two places.** `src/clearing.rs` and `src/clearing.mjs` in [velque-sdk](https://github.com/usevelque/velque-sdk) must give the same price and the same fills. Change them together.
- **Every behaviour has a check.** Add a case to `tests/src/main.rs` for anything new, including the rejection paths.
- **No new dependencies without a reason.** The program is `no_std` with no allocator, and its size is part of the status table in the README.
- **Stay inside the compute budget.** The tests print compute units for the heavy paths: a full window, a full day book sweep. Do not make them worse without saying so.

## Commits

One change per commit, a short imperative subject with a prefix: `feat:`, `fix:`, `test:`, `docs:`, `chore:`, `perf:`.

## Security

Do not open a public issue for a vulnerability. See [SECURITY.md](SECURITY.md).
