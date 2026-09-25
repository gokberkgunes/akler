# akler

TUI layout ranker, designer, optimizer with magic support.
For now uses only n-grams, no corpus traversal.

## Build and run


```sh
cargo build --release
./target/release/akler
```

This program comes with a corpus. You may generate ngrams from a corpus you
want located at `corpus/raw/` with **Corpora -> Import text corpus** if you
need.


Layout files go to `layouts/`. File types are detected from
their contents. Check examples.

Edit [akler.conf](akler.conf) for weights, search settings, ranker
columns,

## Documentation

- [DAT/JSONC layouts, magic/adaptive rules, and geometry](doc/LAYOUTS.md)
- [Every metric, weight, unit, and overlap](doc/METRICS.md)
- [Corpora, ranker controls, search settings, and profiling](doc/USAGE.md)

## Development

```sh
cargo fmt
rustfmt --edition 2021 src/*.rs
cargo check --release
cargo test --release -- --test-threads=1
cargo build --release
```

The explicit `rustfmt` command also covers files loaded with `include!`.
Add `--offline` to Cargo check/test/build commands when dependencies are cached.
