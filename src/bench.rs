//! `akler bench`: repeatable optimizer measurements without the TUI.
//!
//! Runs the optimizer from a layout over several seeds and reports score
//! quality and throughput. Ordinary layouts use the ordinary optimizer and add
//! a short per-trial profile. Action layouts use the action optimizer, or the
//! magic optimizer when exactly one key is bound to a rule action; that key's
//! rules are refitted. Settings come from `akler.conf`; `--set key=value`
//! overrides search settings for this run only and never writes the
//! configuration.

use super::*;
use crate::action_keys as ak;
use crate::action_ngrams as ng;

const USAGE: &str = "bench LAYOUT CORPUS [--seeds N] [--set key=value]... [--full-ngrams] [--rescore-full] [--unlocked] [--magic-at SLOT] [--show] [--swaps N] [--chain K [--move A=B]] [--hand left|right] [--keep-wred] [--fit-combos K] [--combos K] [--fit-swaps K] [--adaptive K] [--config FILE] [--save DIR [--tag NAME]]";

struct Options {
    layout: PathBuf,
    corpus: PathBuf,
    seeds: usize,
    overrides: String,
    full_ngrams: bool,
    rescore_full: bool,
    unlocked: bool,
    /// Move the existing dedicated magic key to this zero-based physical
    /// slot and lock it; caps remain relative to the unmodified input.
    magic_at: Option<usize>,
    show: bool,
    /// List the best N one-swap and two-swap changes instead of searching.
    swaps: Option<usize>,
    /// With `swaps`, list rotations of exactly this many keys instead.
    chain: Option<usize>,
    /// With `chain`, only rotations that move key A into key B's place.
    required: Option<(u8, u8)>,
    /// With `swaps` or `chain`, move only this hand's keys (0 left, 1 right).
    hand: Option<i8>,
    /// With `chain`, drop rotations that raise either hand's weak redirects.
    keep_wred: bool,
    /// Fit up to this many combos to the layout as it stands; no search.
    fit_combos: Option<usize>,
    /// Override `[combos] max` for this run.
    combos: Option<usize>,
    /// Read this file instead of `akler.conf`.
    config: Option<PathBuf>,
    /// Fit up to this many adaptive swaps to the layout as it stands.
    fit_swaps: Option<usize>,
    /// Override `[adaptive] max` for this run.
    adaptive: Option<usize>,
    /// Write each seed's action or magic result here as
    /// `gen-LAYOUT[-TAG]-sN.jsonc`.
    save: Option<PathBuf>,
    tag: String,
}

fn parse(args: &[String]) -> AppResult<Options> {
    let mut positional = Vec::new();
    let mut options = Options {
        layout: PathBuf::new(),
        corpus: PathBuf::new(),
        seeds: 6,
        overrides: String::new(),
        full_ngrams: false,
        rescore_full: false,
        save: None,
        tag: String::new(),
        show: false,
        unlocked: false,
        magic_at: None,
        swaps: None,
        chain: None,
        required: None,
        hand: None,
        keep_wred: false,
        fit_combos: None,
        combos: None,
        config: None,
        fit_swaps: None,
        adaptive: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seeds" => {
                let value = args.next().ok_or("--seeds needs a count")?;
                options.seeds = bounded_usize(value, 1000)?;
            }
            "--set" => {
                let value = args.next().ok_or("--set needs key=value")?;
                let (key, value) = value.split_once('=').ok_or("--set needs key=value")?;
                options
                    .overrides
                    .push_str(&format!("{} = {}\n", key.trim(), value.trim()));
            }
            "--full-ngrams" => options.full_ngrams = true,
            "--rescore-full" => options.rescore_full = true,
            "--show" => options.show = true,
            "--unlocked" => options.unlocked = true,
            "--magic-at" => {
                let value = args
                    .next()
                    .ok_or("--magic-at needs a 1-based slot number")?;
                options.magic_at = Some(bounded_usize(value, 256)? - 1);
            }
            "--chain" => {
                let value = args.next().ok_or("--chain needs a key count")?;
                options.chain = Some(bounded_usize(value, 12)?.max(2));
            }
            "--move" => {
                let value = args.next().ok_or("--move needs A=B")?;
                let pair = value
                    .split_once('=')
                    .and_then(|(a, b)| Some((single_byte(a)?, single_byte(b)?)))
                    .ok_or("--move needs A=B with one character each")?;
                options.required = Some(pair);
            }
            "--keep-wred" => options.keep_wred = true,
            "--adaptive" => {
                let value = args.next().ok_or("--adaptive needs a count")?;
                options.adaptive = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|count| *count <= crate::swap_fit::MAX_SWAPS)
                        .ok_or_else(|| {
                            format!("--adaptive must be 0-{}", crate::swap_fit::MAX_SWAPS)
                        })?,
                );
            }
            "--fit-swaps" => {
                let value = args.next().ok_or("--fit-swaps needs a count")?;
                options.fit_swaps = Some(bounded_usize(value, crate::swap_fit::MAX_SWAPS)?);
            }
            "--config" => {
                let value = args.next().ok_or("--config needs a file")?;
                options.config = Some(PathBuf::from(value));
            }
            "--save" => {
                let value = args.next().ok_or("--save needs a directory")?;
                options.save = Some(PathBuf::from(value));
            }
            "--tag" => {
                let value = args.next().ok_or("--tag needs a name")?;
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return Err("--tag uses letters, digits, - and _".into());
                }
                options.tag = value.clone();
            }
            "--combos" => {
                let value = args.next().ok_or("--combos needs a count")?;
                options.combos = Some(
                    value
                        .parse()
                        .ok()
                        .filter(|count| *count <= crate::combo_fit::MAX_COMBOS)
                        .ok_or_else(|| {
                            format!("--combos must be 0-{}", crate::combo_fit::MAX_COMBOS)
                        })?,
                );
            }
            "--fit-combos" => {
                let value = args.next().ok_or("--fit-combos needs a count")?;
                options.fit_combos = Some(bounded_usize(value, 32)?);
            }
            "--hand" => {
                options.hand = Some(match args.next().map(String::as_str) {
                    Some("left") => 0,
                    Some("right") => 1,
                    _ => return Err("--hand needs left or right".into()),
                });
            }
            "--swaps" => {
                let value = args.next().ok_or("--swaps needs a count")?;
                options.swaps = Some(bounded_usize(value, 1000)?);
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}\n{USAGE}").into())
            }
            _ => positional.push(arg),
        }
    }
    let [layout, corpus] = positional[..] else {
        return Err(USAGE.into());
    };
    options.layout = PathBuf::from(layout);
    options.corpus = corpus_by_name(corpus)?;
    Ok(options)
}

fn load_source(path: &Path, limits: NgramLimits) -> AppResult<Source> {
    let resolved = if corpus_is_raw(path)? {
        ensure_corpus(path, false, None)?
    } else {
        path.to_owned()
    };
    Source::from_text_with_limits(&fs::read_to_string(&resolved)?, &resolved, limits)
}

/// Mean time of `trial_into` over random moves from the starting arrangement.
/// Copying, scoring and cap checks are included; they were measured below 1%.
fn trial_profile(p: &Problem, free: &[usize]) -> (f64, f64) {
    let start = State::new(p.model.original.clone(), p);
    let mut scratch = start.clone();
    let mut rng = Rng::new(0x5EED);
    let moves: Vec<_> = (0..2000)
        .map(|_| rng.movement(free, p.settings.cycle_probability))
        .collect();
    let affected: usize = moves
        .iter()
        .map(|mv| {
            p.caches[0]
                .pair(start.arr[mv.slots[0]], start.arr[mv.slots[1]])
                .len()
        })
        .sum();
    let started = Instant::now();
    for &mv in &moves {
        trial_into(&mut scratch, &start, mv, p);
    }
    (
        started.elapsed().as_secs_f64() / moves.len() as f64,
        affected as f64 / moves.len() as f64,
    )
}

/// Print the best `count` single swaps and swap pairs of the free letters,
/// scored exactly like the optimizer's trials and respecting its caps. A
/// pair may share a key, which makes a three-key rotation.
fn swap_report(p: &Problem, locked: &[bool], count: usize) {
    let start = State::new(p.model.original.clone(), p);
    let free = movable_slots(p, &start, locked);
    let mut singles = Vec::new();
    for (index, &a) in free.iter().enumerate() {
        for &b in &free[index + 1..] {
            singles.push((a, b));
        }
    }
    let name = |arr: &[usize], (a, b): (usize, usize)| {
        let label = |slot: usize| display_symbol(p.model.canonical[arr[slot]]).to_string();
        format!("{}↔{}", label(a), label(b))
    };
    let mut scratch = start.clone();
    let mut single_scores = std::collections::HashMap::new();
    let mut ranked_singles = Vec::new();
    for &swap in &singles {
        trial_into(&mut scratch, &start, Move::pair(swap.0, swap.1), p);
        single_scores.insert(swap, scratch.score);
        if p.feasible(&scratch.raws) {
            ranked_singles.push((scratch.score, swap));
        }
    }
    ranked_singles.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut first = start.clone();
    let mut seen = std::collections::HashSet::new();
    let mut ranked_pairs = Vec::new();
    for &one in &singles {
        trial_into(&mut first, &start, Move::pair(one.0, one.1), p);
        for &two in &singles {
            if two == one {
                continue;
            }
            trial_into(&mut scratch, &first, Move::pair(two.0, two.1), p);
            if p.feasible(&scratch.raws) && seen.insert(scratch.arr.clone()) {
                ranked_pairs.push((scratch.score, one, two));
            }
        }
    }
    ranked_pairs.sort_by(|a, b| a.0.total_cmp(&b.0));

    let summary = |arr: &[usize]| change_summary(p, &start, arr);
    println!("start            {}", summary(&start.arr));
    println!("\nbest single swaps of {} free keys:", free.len());
    for (rank, &(_, swap)) in ranked_singles.iter().take(count).enumerate() {
        let mut arr = start.arr.clone();
        arr.swap(swap.0, swap.1);
        println!(
            "{:>3}. {:<12} {}",
            rank + 1,
            name(&start.arr, swap),
            summary(&arr)
        );
    }
    println!("\nbest two-swap changes (each swap alone in brackets):");
    for (rank, &(_, one, two)) in ranked_pairs.iter().take(count).enumerate() {
        let mut arr = start.arr.clone();
        let label = format!("{} {}", name(&arr, one), {
            arr.swap(one.0, one.1);
            name(&arr, two)
        });
        arr.swap(two.0, two.1);
        let alone = |swap| {
            let score = single_scores.get(&swap).copied().unwrap_or(f64::NAN);
            100.0 * (score - start.score) / start.score.abs()
        };
        let second = if two.0 == one.0 || two.0 == one.1 || two.1 == one.0 || two.1 == one.1 {
            "rotation".to_string()
        } else {
            format!("{:+.2}%", alone(two))
        };
        println!(
            "{:>3}. {:<18} {}  [{:+.2}%, {second}]",
            rank + 1,
            label,
            summary(&arr),
            alone(one),
        );
    }
}

/// Free main keys holding a symbol.
fn movable_slots(p: &Problem, start: &State, locked: &[bool]) -> Vec<usize> {
    (0..start.arr.len())
        .filter(|&slot| {
            !locked[slot]
                && p.model.board.keys[slot].main
                && !blank(p.model.canonical[start.arr[slot]])
        })
        .collect()
}

/// Weak-redirect share of each hand, in the WRED metric's units: the two
/// hands sum to the overall WRED percentage.
fn weak_redirects_by_hand(p: &Problem, state: &State) -> [f64; 2] {
    let geometry = &p.model.geometry;
    let corpus = &p.corpora[0];
    let pos = positions(&state.arr);
    let mut mass = [0.0; 2];
    for gram in corpus.grams.iter().filter(|gram| gram.kind() == 3) {
        let [a, b, c] = gram.ids().map(|id| pos[id]);
        if geometry.tri.raw((a * geometry.n + b) * geometry.n + c) & bit(WRED) != 0 {
            mass[geometry.keys[a].hand.clamp(0, 1) as usize] += gram.f;
        }
    }
    let denominator = denominator_totals(WRED, &state.raws[0], &corpus.totals);
    mass.map(|value| pct(value, denominator))
}

/// Score, SFB/SFS and the busiest finger loads of `arr`, relative to `start`.
fn change_summary(p: &Problem, start: &State, arr: &[usize]) -> String {
    let state = State::new(arr.to_vec(), p);
    let wred = weak_redirects_by_hand(p, &state);
    let measured = metrics(&state.raws[0], &p.corpora[0]);
    let base = metrics(&start.raws[0], &p.corpora[0]);
    let names = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP"];
    let changed: Vec<_> = names
        .iter()
        .enumerate()
        .filter(|&(finger, _)| (measured.usage[finger] - base.usage[finger]).abs() >= 0.05)
        .map(|(finger, name)| format!("{name} {:.1}", measured.usage[finger]))
        .collect();
    format!(
        "score {:8.4} ({:+.2}%)  SFB {:.2}%  SFS {:.2}%  WRED L {:.2}% R {:.2}%  usage {}",
        state.score,
        100.0 * (state.score - start.score) / start.score.abs(),
        measured.v[SFB],
        measured.v[SFS],
        wred[0],
        wred[1],
        changed.join(" "),
    )
}

/// Print the best `count` rotations of exactly `keys` free keys: each key
/// moves to the next one's place and the last to the first's. Chains grow
/// one key at a time from the best `BEAM` closed rotations of each length;
/// closing a chain p1..pk and then swapping p1 with q gives p1..pk q.
fn single_byte(text: &str) -> Option<u8> {
    let bytes = text.trim().as_bytes();
    (bytes.len() == 1).then(|| bytes[0])
}

fn chain_report(
    p: &Problem,
    locked: &[bool],
    keys: usize,
    count: usize,
    required: Option<(u8, u8)>,
    keep_wred: bool,
) -> AppResult<()> {
    const BEAM: usize = 20000;
    let start = State::new(p.model.original.clone(), p);
    let free = movable_slots(p, &start, locked);
    let base = metrics(&start.raws[0], &p.corpora[0]);
    let names = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP"];
    let loads: Vec<_> = names
        .iter()
        .zip(base.usage)
        .map(|(name, share)| format!("{name} {share:.1}"))
        .collect();
    let wred = weak_redirects_by_hand(p, &start);
    // Allow rounding-level differences only.
    let wred_ok = |state: &State| {
        !keep_wred
            || weak_redirects_by_hand(p, state)
                .iter()
                .zip(wred)
                .all(|(value, limit)| *value <= limit + 1e-9)
    };
    println!(
        "start  score {:.4}  SFB {:.2}%  SFS {:.2}%  WRED L {:.2}% R {:.2}%  usage {}",
        start.score,
        base.v[SFB],
        base.v[SFS],
        wred[0],
        wred[1],
        loads.join(" ")
    );
    let slot_of = |symbol: u8| {
        free.iter()
            .copied()
            .find(|&slot| p.model.canonical[start.arr[slot]].eq_ignore_ascii_case(&symbol))
            .ok_or_else(|| format!("{} is not a free key", symbol as char))
    };
    // A chain's first key moves into its second key's place, so a required
    // move fixes the first two links.
    let seeds: Vec<(usize, usize)> = match required {
        Some((from, to)) => vec![(slot_of(from)?, slot_of(to)?)],
        None => free
            .iter()
            .enumerate()
            .flat_map(|(index, &a)| free[index + 1..].iter().map(move |&b| (a, b)))
            .collect(),
    };
    let mut beam: Vec<(f64, Vec<usize>, State)> = Vec::new();
    for (a, b) in seeds {
        let mut state = start.clone();
        trial_into(&mut state, &start, Move::pair(a, b), p);
        beam.push((state.score, vec![a, b], state));
    }
    let mut seen = std::collections::HashSet::new();
    for _ in 2..keys {
        let mut next = Vec::new();
        for (_, chain, closed) in &beam {
            for &q in &free {
                if chain.contains(&q) {
                    continue;
                }
                let mut state = closed.clone();
                trial_into(&mut state, closed, Move::pair(chain[0], q), p);
                if seen.insert(state.arr.clone()) {
                    let mut longer = chain.clone();
                    longer.push(q);
                    next.push((state.score, longer, state));
                }
            }
        }
        next.sort_by(|a, b| a.0.total_cmp(&b.0));
        if keep_wred {
            // Half the beam keeps the best chains within the weak-redirect
            // limit; the rest may pass through it on the way to longer ones.
            let (safe, other): (Vec<_>, Vec<_>) =
                next.into_iter().partition(|(_, _, state)| wred_ok(state));
            next = safe.into_iter().take(BEAM / 2).collect();
            next.extend(other.into_iter().take(BEAM / 2));
            next.sort_by(|a, b| a.0.total_cmp(&b.0));
        } else {
            next.truncate(BEAM);
        }
        beam = next;
    }
    beam.retain(|(_, _, state)| p.feasible(&state.raws) && wred_ok(state));
    println!(
        "\nbest {keys}-key rotations of {} free keys (a→b: a moves to b's place):",
        free.len()
    );
    let label = |slot: usize| display_symbol(p.model.canonical[start.arr[slot]]).to_string();
    for (rank, (_, chain, state)) in beam.iter().take(count).enumerate() {
        let path: Vec<_> = chain.iter().map(|&slot| label(slot)).collect();
        println!(
            "{:>3}. {}→{}  {}",
            rank + 1,
            path.join("→"),
            path[0],
            change_summary(p, &start, &state.arr)
        );
    }
    Ok(())
}

/// Print an ordinary result row by row with each finger's share of presses.
fn show_layout(problem: &Problem, arr: &[usize]) {
    let symbols = problem.model.symbols(arr);
    let keys = &problem.model.board.keys;
    let rows = keys.iter().map(|key| key.row).max().unwrap_or(0);
    for row in 0..=rows {
        let mut line: Vec<_> = (0..keys.len())
            .filter(|&slot| keys[slot].row == row)
            .map(|slot| (keys[slot].main, keys[slot].col, symbols[slot]))
            .collect();
        line.sort();
        let text: String = line
            .iter()
            .map(|&(_, _, byte)| if byte == b' ' { '_' } else { byte as char })
            .collect();
        println!("         {text}");
    }
    let state = State::new(arr.to_vec(), problem);
    let measured = metrics(&state.raws[0], &problem.corpora[0]);
    let usage = measured.usage;
    let names = ["LP", "LR", "LM", "LI", "RI", "RM", "RR", "RP", "LT", "RT"];
    let shares: Vec<_> = names
        .iter()
        .zip(usage)
        .map(|(name, share)| format!("{name} {share:.1}"))
        .collect();
    println!(
        "         usage % {}  ·  SFB {:.2}%  SFS {:.2}%",
        shares.join("  "),
        measured.v[SFB],
        measured.v[SFS]
    );
}

/// One seed's outcome: score, wall seconds, restarts, trials.
type SeedResult = (f64, f64, usize, u64);

fn print_seed(seed: usize, (score, wall, restarts, trials): SeedResult) {
    println!(
        "seed {:2}  score {score:9.4}  wall {wall:6.2} s  {restarts} restarts  {trials} trials  {:.0} trials/s",
        seed + 1,
        trials as f64 / wall.max(1e-9),
    );
}

fn print_summary(results: &[SeedResult]) {
    let n = results.len() as f64;
    let mut scores: Vec<f64> = results.iter().map(|result| result.0).collect();
    let mean = scores.iter().sum::<f64>() / n;
    let deviation = (scores.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
    scores.sort_by(f64::total_cmp);
    let wall: f64 = results.iter().map(|result| result.1).sum();
    let trials: u64 = results.iter().map(|result| result.3).sum();
    println!(
        "summary  mean {mean:.4}  sd {deviation:.4}  best {:.4}  median {:.4}  worst {:.4}  wall {:.2} s/seed  {:.0} trials/s",
        scores[0],
        scores[scores.len() / 2],
        scores[scores.len() - 1],
        wall / n,
        trials as f64 / wall.max(1e-9),
    );
}

fn load_action_corpus(options: &Options, limits: NgramLimits) -> AppResult<ng::NgramCorpus> {
    let resolved = if corpus_is_raw(&options.corpus)? {
        ensure_corpus(&options.corpus, false, None)?
    } else {
        options.corpus.clone()
    };
    Ok(ng::NgramCorpus::from_text_progress_with_limits(
        &fs::read_to_string(&resolved)?,
        &resolved,
        limits,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
    )?)
}

/// Fit up to `max_combos` combos to the layout's current bindings and print
/// the chosen chords with the score before and after.
fn combo_command(
    options: &Options,
    config: &AppConfig,
    settings: &SearchSettings,
    limits: NgramLimits,
    max_combos: usize,
) -> AppResult<()> {
    let layout = ak::Layout::load(&options.layout)?;
    let corpus = load_action_corpus(options, limits)?;
    let stop = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let weights = &config.weights;
    let before = crate::action_summary::evaluate_with_mode(
        &layout,
        &corpus,
        weights,
        &stop,
        &progress,
        &settings.mode,
    )?;
    let started = Instant::now();
    let fitted = crate::combo_fit::fit(
        &layout,
        &corpus,
        weights,
        &crate::action_summary::PhysicalTables::with_mode(&layout, weights, &settings.mode),
        &crate::combo_fit::Settings {
            max_combos,
            ..Default::default()
        },
        None,
        &stop,
        &progress,
        crate::search_worker_count(settings),
        None,
    )?;
    let names: Vec<_> = fitted
        .chords
        .iter()
        .map(|chord| {
            format!(
                "{}+{}={}",
                fitted.layout.slots[chord.first].label,
                fitted.layout.slots[chord.second].label,
                String::from_utf8_lossy(&chord.output)
            )
        })
        .collect();
    println!(
        "{} on {} · mode {} · up to {max_combos} combos",
        layout.name, corpus.name, settings.mode
    );
    println!(
        "before   score {:9.4}  SFB {:.2}%  SFS {:.2}%",
        before.score, before.metrics.v[SFB], before.metrics.v[SFS]
    );
    println!(
        "after    score {:9.4}  SFB {:.2}%  SFS {:.2}%  combos {}",
        fitted.summary.score,
        fitted.summary.metrics.v[SFB],
        fitted.summary.metrics.v[SFS],
        names.join(" ")
    );
    println!(
        "{} evaluations in {:.2} s",
        fitted.evaluations,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Fit up to `max_swaps` adaptive swaps to the layout's current bindings and
/// print them with the score before and after.
fn swap_command(
    options: &Options,
    config: &AppConfig,
    settings: &SearchSettings,
    limits: NgramLimits,
    max_swaps: usize,
) -> AppResult<()> {
    let layout = ak::Layout::load(&options.layout)?;
    let corpus = load_action_corpus(options, limits)?;
    let stop = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let weights = &config.weights;
    let before = crate::action_summary::evaluate_with_mode(
        &layout,
        &corpus,
        weights,
        &stop,
        &progress,
        &settings.mode,
    )?;
    let started = Instant::now();
    let fitted = crate::swap_fit::fit(
        &layout,
        &corpus,
        weights,
        &crate::action_summary::PhysicalTables::with_mode(&layout, weights, &settings.mode),
        &crate::swap_fit::Settings {
            max_swaps,
            ..Default::default()
        },
        None,
        &stop,
        &progress,
        crate::search_worker_count(settings),
        None,
    )?;
    let names: Vec<_> = fitted
        .swaps
        .iter()
        .map(|swap| {
            format!(
                "{}:{}{}",
                swap.context as char, swap.a as char, swap.b as char
            )
        })
        .collect();
    println!(
        "{} on {} · mode {} · up to {max_swaps} swaps",
        layout.name, corpus.name, settings.mode
    );
    println!(
        "before   score {:9.4}  SFB {:.2}%  SFS {:.2}%",
        before.score, before.metrics.v[SFB], before.metrics.v[SFS]
    );
    println!(
        "after    score {:9.4}  SFB {:.2}%  SFS {:.2}%  swaps {}",
        fitted.summary.score,
        fitted.summary.metrics.v[SFB],
        fitted.summary.metrics.v[SFS],
        names.join(" ")
    );
    println!(
        "{} evaluations in {:.2} s",
        fitted.evaluations,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn position_magic(
    layout: &mut ak::Layout,
    target: Option<usize>,
    unlocked: bool,
) -> AppResult<(Vec<bool>, Option<usize>)> {
    let mut root = crate::magic_optimizer::existing_root(layout);
    if let Some(target) = target {
        let from = root.ok_or("--magic-at requires one existing dedicated magic key")?;
        if target >= layout.slots.len() {
            return Err(format!("--magic-at must be in 1..={}", layout.slots.len()).into());
        }
        layout.swap(from, target);
        root = Some(target);
    }
    let mut locks = if unlocked {
        vec![false; layout.slots.len()]
    } else {
        crate::action_ui::action_default_locks(layout)
    };
    if let Some(target) = target {
        // Moving magic onto a thumb or an empty key can displace Space or
        // leave an empty main key behind. Preserve the normal setup locks.
        if !unlocked {
            for (slot, spare) in layout.spare_slots().into_iter().enumerate() {
                locks[slot] |= spare || layout.space(slot);
            }
        }
        locks[target] = true;
    }
    Ok((locks, root))
}

fn action_command(
    options: &Options,
    config: &AppConfig,
    settings: SearchSettings,
    limits: NgramLimits,
    mut layout: ak::Layout,
) -> AppResult<()> {
    // As the optimizer screen does: own alternate fingers stay on their keys.
    crate::alt_fingers::pin(&mut layout);
    let original = layout.clone();
    let (locks, root) = position_magic(&mut layout, options.magic_at, options.unlocked)?;
    let corpus = load_action_corpus(options, limits)?;
    let corpora = vec![(corpus, 1.0)];
    let combos = config.combos.active_max(&config.weights);
    let swaps = config.adaptive.max_swaps;
    println!(
        "{} on {} · {}{} · mode {} · {} free keys · {} workers · {} s · {} restarts · {} anneal steps · T {} → {}",
        layout.name,
        corpora[0].0.name,
        if root.is_some() { "magic optimizer" } else { "action optimizer" },
        if combos > 0 {
            let bound = if config.combos.exact { "exactly" } else { "up to" };
            format!(" · {bound} {combos} combos")
        } else {
            String::new()
        } + &if swaps > 0 {
            let bound = if config.adaptive.exact { "exactly" } else { "up to" };
            format!(" · {bound} {swaps} swaps")
        } else {
            String::new()
        },
        settings.mode,
        locks.iter().filter(|&&locked| !locked).count(),
        search_worker_count(&settings),
        settings.seconds,
        settings.restarts,
        settings.for_actions().anneal_steps,
        temperature_label(&settings),
        settings.cooling_end,
    );
    if let Some(target) = options.magic_at {
        let slot = &layout.slots[target];
        println!(
            "magic locked at slot {} · row {} column {} · caps relative to original input",
            target + 1,
            slot.row + 1,
            slot.col + 1,
        );
    }
    let mut results = Vec::with_capacity(options.seeds);
    for seed in 0..options.seeds {
        let mut seeded = settings.clone();
        seeded.seed = worker_seed(settings.seed, 1000 + seed);
        let started = Instant::now();
        let snapshot = match root {
            _ if root.is_some() || combos > 0 || swaps > 0 => {
                let snapshot = crate::magic_optimizer::run_controlled(
                    &original,
                    &layout,
                    root,
                    &locks,
                    &corpora,
                    &config.weights,
                    &seeded,
                    &config.magic,
                    &config.combos,
                    &config.adaptive,
                    &Control::new(),
                    &mut |_, _, _, _, _| {},
                )?;
                let mut result = snapshot.0.clone();
                crate::alt_fingers::unpin(&mut result);
                let snapshot = (result, snapshot.1);
                if let Some(directory) = &options.save {
                    let tag = if options.tag.is_empty() {
                        String::new()
                    } else {
                        format!("-{}", options.tag)
                    };
                    let path =
                        directory.join(format!("gen-{}{tag}-s{}.jsonc", layout.name, seed + 1));
                    if path.exists() {
                        return Err(format!("{} already exists", path.display()).into());
                    }
                    std::fs::write(&path, crate::layout_export::jsonc_text(&snapshot.0)?)?;
                    println!("         saved {}", path.display());
                }
                if options.show {
                    if let Some(line) = crate::action_ui::rules_and_combos(&snapshot.0) {
                        println!("         {line}");
                    }
                    let summary = crate::action_summary::evaluate_with_mode(
                        &snapshot.0,
                        &corpora[0].0,
                        &config.weights,
                        &AtomicBool::new(false),
                        &AtomicU64::new(0),
                        &settings.mode,
                    )?;
                    println!(
                        "         SFB {:.6}%  SFS {:.6}%  WRED {:.6}%  rescored {:.6}",
                        summary.metrics.v[SFB],
                        summary.metrics.v[SFS],
                        summary.metrics.v[WRED],
                        summary.score
                    );
                }
                snapshot.1
            }
            _ => crate::action_search::run_parallel(
                &layout,
                &layout,
                &corpora,
                &config.weights,
                &seeded,
                &locks,
                &Control::new(),
                &mut |_, _, _, _| {},
            )?,
        };
        let score = if snapshot.has_best {
            snapshot.best.score
        } else {
            f64::NAN
        };
        let result = (
            score,
            started.elapsed().as_secs_f64(),
            snapshot.progress.restart,
            snapshot.progress.evaluations,
        );
        print_seed(seed, result);
        results.push(result);
    }
    print_summary(&results);
    Ok(())
}

/// `bench --ranker CORPUS`: time a complete ranker load (parse every layout,
/// prepare the corpus, evaluate each layout once) with `akler.conf` caps.
fn ranker_command(args: &[String]) -> AppResult<()> {
    let [corpus] = args else {
        return Err("bench --ranker CORPUS".into());
    };
    let corpus = corpus_by_name(corpus)?;
    // A name resolves to the raw text when both exist; the ranker reads JSON.
    let corpus = if corpus_is_raw(&corpus)? {
        ensure_corpus(&corpus, false, None)?
    } else {
        corpus
    };
    let limits = load_app_config()?.ngrams;
    let cap = |limit: Option<usize>| limit.map_or("all".into(), |n| n.to_string());
    println!(
        "ranker on {} · trigrams {} · tetragrams {} · pentagrams {}",
        corpus_name(&corpus),
        cap(limits.trigrams),
        cap(limits.tetragrams),
        cap(limits.pentagrams),
    );
    for run in 1..=3 {
        let started = Instant::now();
        let data = load_ranking_progress_for_mode(
            &corpus,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            None,
            None,
            None,
        )?;
        println!(
            "run {run}  {} layouts  {:.2} s",
            data.rows.len(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

pub(crate) fn command(args: &[String]) -> AppResult<()> {
    if args.first().is_some_and(|arg| arg == "--ranker") {
        return ranker_command(&args[1..]);
    }
    let options = parse(args)?;
    if let Some(path) = &options.config {
        use_config_file(path)?;
    }
    let config = load_app_config()?;
    let settings = apply_search_text(config.search.clone(), &options.overrides)?;
    let mut limits = config.ngrams;
    let mut full_limits = config.ngrams;
    full_limits.trigrams = None;
    full_limits.tetragrams = None;
    full_limits.pentagrams = None;
    if options.full_ngrams {
        limits = full_limits;
    }
    if let Some(max_swaps) = options.fit_swaps {
        return swap_command(&options, &config, &settings, limits, max_swaps);
    }
    if let Some(max_combos) = options.fit_combos {
        return combo_command(&options, &config, &settings, limits, max_combos);
    }
    let mut config = config;
    if let Some(max) = options.combos {
        config.combos.max_combos = max;
    }
    if let Some(max) = options.adaptive {
        config.adaptive.max_swaps = max;
    }
    // Combo and swap fitting need the action optimizer, plain layouts included.
    if let Ok(layout) = ak::Layout::load(&options.layout) {
        if layout.extended()
            || options.magic_at.is_some()
            || config.combos.active_max(&config.weights) > 0
            || config.adaptive.max_swaps > 0
        {
            return action_command(&options, &config, settings, limits, layout);
        }
    }
    let source = load_source(&options.corpus, limits)?;
    let board = load_board(&options.layout)?;
    let model = Model::with_rolls(board, config.weights.rolls());
    let locked = if options.unlocked {
        vec![false; model.original.len()]
    } else {
        default_locks(&model.board)
    };
    // Optional second problem scoring each result on every retained n-gram.
    let full = if options.rescore_full {
        let source = load_source(&options.corpus, full_limits)?;
        Some(Problem::new(
            model.clone(),
            &[source],
            0,
            config.weights.clone(),
            settings.clone(),
        )?)
    } else {
        None
    };
    let problem = Problem::new(model, &[source], 0, config.weights.clone(), settings)?;
    let mut locked = locked;
    if let Some(hand) = options.hand {
        for (slot, key) in problem.model.board.keys.iter().enumerate() {
            locked[slot] |= key.hand != hand;
        }
    }
    if let Some(keys) = options.chain {
        chain_report(
            &problem,
            &locked,
            keys,
            options.swaps.unwrap_or(15),
            options.required,
            options.keep_wred,
        )?;
        return Ok(());
    }
    if let Some(count) = options.swaps {
        swap_report(&problem, &locked, count);
        return Ok(());
    }
    let s = &problem.settings;
    let free: Vec<_> = (0..locked.len()).filter(|&i| !locked[i]).collect();
    let corpus = &problem.corpora[0];
    let kinds = [1, 2, 3].map(|kind| corpus.grams.iter().filter(|g| g.kind() == kind).count());
    println!(
        "{} on {} · mode {} · {} free keys · {} workers · {} s · {} restarts · {} anneal steps · T {} → {}",
        problem.model.board.name,
        corpus.name,
        s.mode,
        free.len(),
        search_worker_count(s),
        s.seconds,
        s.restarts,
        s.anneal_steps,
        temperature_label(s),
        s.cooling_end,
    );
    let (trial, affected) = trial_profile(&problem, &free);
    println!(
        "grams: {} bigrams, {} skipgrams, {} trigrams · {affected:.0} affected per move · {:.1} µs per trial",
        kinds[0],
        kinds[1],
        kinds[2],
        trial * 1e6,
    );

    let mut results = Vec::with_capacity(options.seeds);
    let mut rescored = Vec::new();
    for seed in 0..options.seeds {
        let mut seeded = problem.clone();
        seeded.settings.seed = worker_seed(s.seed, 1000 + seed);
        let started = Instant::now();
        let snapshot = run_search_parallel(
            &seeded,
            &seeded.model.original,
            &locked,
            &Control::new(),
            &mut |_| {},
        )?;
        let score = if snapshot.has_best {
            snapshot.best.score
        } else {
            f64::NAN
        };
        let result = (
            score,
            started.elapsed().as_secs_f64(),
            snapshot.progress.restart,
            snapshot.progress.evaluations,
        );
        print_seed(seed, result);
        results.push(result);
        if options.show {
            show_layout(&problem, &snapshot.best.arr);
        }
        if let Some(full) = &full {
            let score = State::new(snapshot.best.arr.clone(), full).score;
            println!("         all n-grams {score:9.4}");
            rescored.push((score, result.1, result.2, 0));
        }
    }
    print_summary(&results);
    if !rescored.is_empty() {
        print!("all n-grams ");
        print_summary(&rescored);
    }
    Ok(())
}

#[cfg(test)]
mod magic_position_tests {
    use super::*;

    #[test]
    fn moving_magic_preserves_bindings_and_locks_every_physical_destination() {
        let original = ak::Layout::parse(
            "q w e r t y u i o p\na s d f g h j k l *\nz x c v b n m , . ~\nthumbs: e space\n* eo\n",
            Path::new("positions.dat"),
        ).unwrap();
        let from = crate::magic_optimizer::existing_root(&original).unwrap();
        for target in 0..original.slots.len() {
            for unlocked in [false, true] {
                let mut layout = original.clone();
                let (locks, root) = position_magic(&mut layout, Some(target), unlocked).unwrap();
                assert_eq!(root, Some(target));
                assert!(locks[target]);
                assert_eq!(layout.slots[target].binding, original.slots[from].binding);
                assert_eq!(layout.slots[from].binding, original.slots[target].binding);
                for slot in 0..layout.slots.len() {
                    assert_eq!(layout.slots[slot].finger, original.slots[slot].finger);
                    if !unlocked && (layout.space(slot) || !layout.slots[slot].main) {
                        assert!(locks[slot]);
                    }
                    if slot != from && slot != target {
                        assert_eq!(layout.slots[slot], original.slots[slot]);
                    }
                }
                assert_eq!(layout.actions, original.actions);
            }
        }
        assert!(position_magic(&mut original.clone(), Some(original.slots.len()), false).is_err());
    }
}
