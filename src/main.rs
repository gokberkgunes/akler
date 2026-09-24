#![allow(dead_code)]
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

include!("model.rs");

include!("ngrams.rs");

include!("search.rs");

include!("config.rs");

include!("ui.rs");

include!("ranking.rs");

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

mod action_summary;

mod session;

mod layout_io;

mod layout_export;
