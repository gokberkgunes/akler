# layouter

TUI layout ranker, designer, optimizer with magic support.

For now uses only n-grams, no corpus traversal.

## Build and run


```sh
cargo build --release
./target/release/layouter
```

## Layouts and corpora

Store layouts in `layouts/`.

Import a text corpus and build its n-gram cache:

```sh
./target/release/layouter corpus add mycorpus input.txt
./target/release/layouter corpus build corpus/raw/mycorpus.txt --order 5
```

The generated cache is `corpus/processed/corpus-mycorpus.json`. Use `--order 3` or `--order 4` for a lower-order cache. Corpus text and generated caches are excluded from Git.

## Development

```sh
cargo check --release
cargo test --release -- --test-threads=1
```

Optional diagnostics: set `LAYOUTER_PROFILE=1` for optimizer profiling or `LAYOUTER_PROFILE_LOAD=1` for loading timings.
