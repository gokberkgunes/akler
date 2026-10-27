use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// WASI hosts have no stty (`HostTerminal`).
#[cfg(not(target_os = "wasi"))]
use std::process::Command;

include!("model.rs");

include!("ngrams.rs");

include!("search.rs");

include!("config.rs");

include!("ui.rs");

include!("ranking.rs");

include!("weakness.rs");

include!("app.rs");

#[cfg(test)]
mod tests;

mod action_keys;

mod action_ui;

mod action_ngrams;

mod action_fast;

mod action_profile;

mod load_profile;

mod profile_output;

mod action_search;

mod action_rule_delta;
mod action_rule_eval;
mod action_summary;

mod session;

mod layout_io;

mod layout_export;

mod alt_fingers;
mod combo_fit;
mod combos;
mod layout_rules;
mod swap_fit;

mod mana2_metrics;

mod finger_speed;

mod bench;

mod magic_design;
mod magic_optimizer;
mod magic_rule_fit;

include!("magic_ui.rs");

include!("scissor_test.rs");

// Threaded WebAssembly builds recycle small blocks per thread (`wasm_alloc`).
#[cfg(all(target_family = "wasm", target_feature = "atomics"))]
mod wasm_alloc;

#[cfg(all(target_family = "wasm", target_feature = "atomics"))]
#[global_allocator]
static ALLOCATOR: wasm_alloc::Allocator = wasm_alloc::Allocator;

mod magic_archive;
