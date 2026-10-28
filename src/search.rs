#[derive(Clone, Debug)]
struct SearchSettings {
    hybrid: bool,
    workers: usize,
    restarts: usize,
    passes: usize,
    anneal_steps: usize,
    /// Upper limit on `anneal_steps` for the action and magic optimizers,
    /// whose results stop improving long before the ordinary optimizer's.
    magic_anneal_steps: usize,
    temperature: f64,
    cooling_end: f64,
    /// Calibrate both annealing temperatures from sampled move costs. The
    /// numeric temperatures remain the fallback and drive other searches.
    cycle_probability: f64,
    seconds: f64,
    seed: u64,
    archive: usize,
    sfb_limit: Option<f64>,
    sfs_limit: Option<f64>,
    travel_limit: Option<f64>,
    sftravel_limit: Option<f64>,
    mix: BTreeMap<String, f64>,
    design: String,
    mode: String,
    preset: String,
    diversity: usize,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            hybrid: true,
            workers: 0,
            restarts: 12,
            passes: 80,
            anneal_steps: 30000,
            magic_anneal_steps: 12000,
            temperature: 0.30,
            cooling_end: 0.003,
            cycle_probability: 0.20,
            seconds: 30.0,
            seed: 0x8A5C_3D91_27E4_B6F1,
            archive: 12,
            sfb_limit: Some(0.0),
            sfs_limit: Some(0.0),
            travel_limit: None,
            sftravel_limit: None,
            mix: BTreeMap::new(),
            design: "refine".into(),
            mode: "normal".into(),
            preset: "custom".into(),
            diversity: 6,
        }
    }
}

impl SearchSettings {
    /// Settings for the action and magic optimizers: `magic_anneal_steps`
    /// annealing steps per restart; `anneal_steps` is the ordinary
    /// optimizer's.
    pub(crate) fn for_actions(&self) -> Self {
        Self {
            anneal_steps: self.magic_anneal_steps,
            ..self.clone()
        }
    }

    fn uses_mana2_stats(&self) -> bool {
        self.mode.as_str() == "aklgg"
    }
}

fn search_worker_count(settings: &SearchSettings) -> usize {
    if std::env::var("AKLER_PROFILE")
        .or_else(|_| std::env::var("LAYOUTER_PROFILE"))
        .is_ok_and(|level| matches!(level.as_str(), "1" | "2"))
    {
        return 1;
    }
    let requested = if settings.workers == 0 {
        available_threads()
    } else {
        settings.workers
    };
    requested.max(1).min(256).min(settings.restarts.max(1))
}

fn worker_seed(seed: u64, worker: usize) -> u64 {
    seed.wrapping_add((worker as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn bounded_usize(s: &str, max: usize) -> AppResult<usize> {
    let n: usize = s.parse()?;
    if n == 0 || n > max {
        return Err(format!("expected 1..{max}").into());
    }
    Ok(n)
}

fn bounded_workers(s: &str) -> AppResult<usize> {
    let n: usize = s.parse()?;
    if n > 256 {
        return Err("expected 0..256".into());
    }
    Ok(n)
}

fn optional_limit(s: &str) -> AppResult<Option<f64>> {
    if s.eq_ignore_ascii_case("none") {
        Ok(None)
    } else {
        Ok(Some(finite_nonnegative(s)?))
    }
}

fn apply_preset(s: &mut SearchSettings, name: &str) {
    s.preset = name.into();
    if name == "custom" {
        return;
    }
    s.sfb_limit = Some(0.05);
    s.sfs_limit = Some(0.25);
    s.travel_limit = None;
    s.sftravel_limit = None;
    match name {
        "strict" => {
            s.sfb_limit = Some(0.0);
            s.sfs_limit = Some(0.0);
            s.travel_limit = Some(0.0);
            s.sftravel_limit = Some(0.0);
        }
        "low-travel" => {
            s.travel_limit = Some(0.0);
            s.sftravel_limit = Some(0.0);
        }
        "explore" => {
            s.sfb_limit = None;
            s.sfs_limit = None;
        }
        _ => {}
    }
}

fn validate_search_settings(s: &SearchSettings) -> AppResult<()> {
    if !["refine", "random", "evolve"].contains(&s.design.as_str()) {
        return Err("design must be refine, random or evolve".into());
    }
    if !["normal", "aklgg"].contains(&s.mode.as_str()) {
        return Err("stat set must be normal or aklgg".into());
    }
    if !["custom", "balanced", "strict", "low-travel", "explore"].contains(&s.preset.as_str()) {
        return Err("unknown preset".into());
    }
    if s.diversity > 32 {
        return Err("min_distance must be 0..32".into());
    }
    for v in [s.sfb_limit, s.sfs_limit, s.travel_limit, s.sftravel_limit]
        .into_iter()
        .flatten()
        .chain([s.seconds, s.temperature, s.cooling_end, s.cycle_probability])
    {
        if !v.is_finite() || v < 0.0 || v > 1e6 {
            return Err("search values must be finite, nonnegative and <= 1000000".into());
        }
    }
    if s.cycle_probability > 1.0 {
        return Err("3-key share must be <= 1".into());
    }
    if s.restarts == 0
        || s.restarts > 100000
        || s.workers > 256
        || s.passes == 0
        || s.passes > 100000
        || s.anneal_steps == 0
        || s.anneal_steps > 10000000
        || s.magic_anneal_steps == 0
        || s.magic_anneal_steps > 10000000
        || s.archive == 0
        || s.archive > 100
    {
        return Err("invalid search iteration count".into());
    }
    if s.hybrid && (s.temperature <= 0.0 || s.cooling_end <= 0.0 || s.cooling_end > s.temperature) {
        return Err("require 0 < end temperature <= start temperature".into());
    }
    Ok(())
}

fn search_from_text(text: &str) -> AppResult<SearchSettings> {
    apply_search_text(SearchSettings::default(), text)
}

/// Apply `key = value` search settings over existing ones, then validate.
fn apply_search_text(mut s: SearchSettings, text: &str) -> AppResult<SearchSettings> {
    for (k, v) in config_lines(text)? {
        match k.as_str() {
            "method" => {
                s.hybrid = match v.as_str() {
                    "hybrid" => true,
                    "sweep" => false,
                    _ => return Err("method must be hybrid or sweep".into()),
                }
            }
            "workers" => s.workers = bounded_workers(&v)?,
            "restarts" => s.restarts = bounded_usize(&v, 100000)?,
            "passes" => s.passes = bounded_usize(&v, 100000)?,
            "anneal_steps" => s.anneal_steps = bounded_usize(&v, 10000000)?,
            // The old name, from when it capped `anneal_steps`.
            "magic_anneal_steps" | "max_action_anneal_steps" => {
                s.magic_anneal_steps = bounded_usize(&v, 10000000)?
            }
            "archive" => s.archive = bounded_usize(&v, 100)?,
            "temperature_start" if v.eq_ignore_ascii_case("auto") => {
                return Err("temperature_start = auto was removed; use a number such as 3".into())
            }
            "temperature_start" => s.temperature = finite_nonnegative(&v)?,
            "temperature_end" => s.cooling_end = finite_nonnegative(&v)?,
            "cycle_probability" => s.cycle_probability = finite_nonnegative(&v)?,
            "seconds" => s.seconds = finite_nonnegative(&v)?,
            "seed" => {
                s.seed = if let Some(h) = v.strip_prefix("0x") {
                    u64::from_str_radix(h, 16)?
                } else {
                    v.parse()?
                }
            }
            "max_sfb_increase" | "max_sfb" => s.sfb_limit = optional_limit(&v)?,
            "max_sfs_increase" | "max_sfs" => s.sfs_limit = optional_limit(&v)?,
            "max_travel_increase" => s.travel_limit = optional_limit(&v)?,
            "max_sftravel_increase" => s.sftravel_limit = optional_limit(&v)?,
            "design" => s.design = v.to_ascii_lowercase(),
            "mode" | "metrics" => {
                s.mode = match v.to_ascii_lowercase().as_str() {
                    "normal" | "detailed" => "normal",
                    // Mana mode was merged into aklgg.
                    "aklgg" | "mana" | "mana2" => "aklgg",
                    _ => return Err("stat set must be normal or aklgg".into()),
                }.into();
            }
            "preset" => s.preset = v.to_ascii_lowercase(),
            "min_distance" => s.diversity = v.parse()?,
            _ if k.starts_with("corpus.") => {
                s.mix
                    .insert(k[7..].to_ascii_lowercase(), finite_nonnegative(&v)?);
            }
            _ => return Err(format!("unknown search setting {k}").into()),
        }
    }
    validate_search_settings(&s)?;
    Ok(s)
}

fn method_label(s: &SearchSettings) -> &'static str {
    if s.hybrid {
        "hybrid"
    } else {
        "sweep"
    }
}

fn temperature_label(s: &SearchSettings) -> String {
    config_number(s.temperature)
}

fn search_settings_text(s: &SearchSettings) -> String {
    let lim = |n: Option<f64>| n.map(|v| v.to_string()).unwrap_or("none".into());
    let mut text = format!("method = {}\nworkers = {}\nrestarts = {}\npasses = {}\nanneal_steps = {}\nmagic_anneal_steps = {}\ntemperature_start = {}\ntemperature_end = {}\ncycle_probability = {}\nseconds = {}\nseed = 0x{:x}\narchive = {}\nmax_sfb_increase = {}\nmax_sfs_increase = {}\nmax_travel_increase = {}\nmax_sftravel_increase = {}\ndesign = {}\nmode = {}\npreset = {}\nmin_distance = {}\n", method_label(s), s.workers, s.restarts, s.passes, s.anneal_steps, s.magic_anneal_steps, temperature_label(s), s.cooling_end, s.cycle_probability, s.seconds, s.seed, s.archive, lim(s.sfb_limit), lim(s.sfs_limit), lim(s.travel_limit), lim(s.sftravel_limit), s.design, s.mode, s.preset, s.diversity);
    for (k, v) in &s.mix {
        text.push_str(&format!("corpus.{k} = {v}\n"));
    }
    text
}

fn new_seed() -> u64 {
    let mut b = [0u8; 8];
    if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
    {
        u64::from_le_bytes(b)
    } else {
        timestamp() as u64
    }
}

#[derive(Clone)]
struct AffectedCache {
    pairs: Vec<Vec<u32>>,
    n: usize,
}

fn merge_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len() + b.len());
    while i < a.len() || j < b.len() {
        if j == b.len() || (i < a.len() && a[i] < b[j]) {
            out.push(a[i]);
            i += 1;
        } else if i == a.len() || b[j] < a[i] {
            out.push(b[j]);
            j += 1;
        } else {
            out.push(a[i]);
            i += 1;
            j += 1;
        }
    }
    out
}

impl AffectedCache {
    fn new(c: &Corpus) -> Self {
        let n = c.by_symbol.len();
        let mut pairs = vec![Vec::new(); n * n];
        for a in 0..n {
            for b in a + 1..n {
                pairs[a * n + b] = merge_sorted(&c.by_symbol[a], &c.by_symbol[b]);
            }
        }
        Self { pairs, n }
    }

    fn pair(&self, a: usize, b: usize) -> &[u32] {
        &self.pairs[a.min(b) * self.n + a.max(b)]
    }
}

#[derive(Clone)]
struct Problem {
    model: Model,
    corpora: Vec<Corpus>,
    shares: Vec<f64>,
    weights: Weights,
    settings: SearchSettings,
    baseline: Vec<Metrics>,
    caches: Vec<AffectedCache>,
}

impl Problem {
    fn new(
        mut model: Model,
        sources: &[Source],
        selected: usize,
        weights: Weights,
        settings: SearchSettings,
    ) -> AppResult<Self> {
        validate_search_settings(&settings)?;
        if model.geometry.rolls != weights.rolls() {
            model.geometry =
                Arc::new(Geometry::with_alts(model.board.keys.clone(), weights.rolls()));
        }
        if selected >= sources.len() {
            return Err("no selected corpus".into());
        }
        let use_mix = settings.mix.values().any(|v| *v > 0.0);
        let mut order = vec![selected];
        if use_mix {
            for (name, share) in &settings.mix {
                if *share > 0.0 && !sources.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
                    return Err(format!("training corpus {name:?} not found").into());
                }
            }
            for (i, s) in sources.iter().enumerate() {
                if i != selected
                    && settings
                        .mix
                        .get(&s.name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0.0)
                        > 0.0
                {
                    order.push(i);
                }
            }
        }
        let corpora = order
            .iter()
            .map(|&i| model.corpus(&sources[i]))
            .collect::<AppResult<Vec<_>>>()?;
        let mut shares: Vec<_> = order
            .iter()
            .map(|&i| {
                if use_mix {
                    settings
                        .mix
                        .get(&sources[i].name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0.0)
                } else {
                    1.0
                }
            })
            .collect();
        let sum: f64 = shares.iter().sum();
        if sum <= 0.0 || !sum.is_finite() {
            return Err("training shares must have a finite positive sum".into());
        }
        for v in &mut shares {
            *v /= sum;
        }
        let baseline = corpora
            .iter()
            .map(|c| metrics(&full_raw(&model.original, c, &model.geometry), c))
            .collect();
        let caches = corpora.iter().map(AffectedCache::new).collect();
        Ok(Self {
            model,
            corpora,
            shares,
            weights,
            settings,
            baseline,
            caches,
        })
    }

    fn breakdown(&self, raw: &Raw, c: &Corpus) -> Breakdown {
        if self.settings.uses_mana2_stats() {
            return mana_breakdown(&mana_stats_for_corpus(raw, c), &self.weights);
        }
        let m = metrics(raw, c);
        breakdown(&m, &self.weights)
    }

    fn score(&self, raws: &[Raw]) -> f64 {
        raws.iter()
            .enumerate()
            .map(|(i, r)| self.shares[i] * self.breakdown(r, &self.corpora[i]).net)
            .sum()
    }

    fn limits(&self) -> [Option<f64>; 4] {
        [
            self.settings.sfb_limit,
            self.settings.sfs_limit,
            self.settings.travel_limit,
            self.settings.sftravel_limit,
        ]
    }

    /// AKLER counters feed only normal scoring and the caps. Mana and AKL.GG
    /// searches without caps skip them in trials; full rescoring restores them.
    fn normal_counters_unused(&self) -> bool {
        self.settings.uses_mana2_stats() && self.limits().iter().all(Option::is_none)
    }

    fn violation(&self, raws: &[Raw]) -> f64 {
        if self.limits().iter().all(Option::is_none) {
            return 0.0;
        }
        let mut v = 0.0;
        for (i, r) in raws.iter().enumerate() {
            if self.shares[i] == 0.0 {
                continue;
            }
            let m = metrics(r, &self.corpora[i]);
            for (metric, limit) in [
                (SFB, self.settings.sfb_limit),
                (SFS, self.settings.sfs_limit),
                (TRAVEL, self.settings.travel_limit),
                (SFTRAVEL, self.settings.sftravel_limit),
            ] {
                if let Some(d) = limit {
                    let cap = self.baseline[i].v[metric] + d;
                    let excess = m.v[metric] - cap;
                    if excess > 1e-10 {
                        v += (excess / cap.abs().max(1.0)).powi(2);
                    }
                }
            }
        }
        v
    }

    fn feasible(&self, raws: &[Raw]) -> bool {
        self.violation(raws) == 0.0
    }
}

#[derive(Clone)]
struct State {
    arr: Vec<usize>,
    pos: Vec<usize>,
    raws: Vec<Raw>,
    score: f64,
}

impl State {
    fn new(arr: Vec<usize>, p: &Problem) -> Self {
        let pos = positions(&arr);
        let raws = p
            .corpora
            .iter()
            .map(|c| full_raw_with_mana2(&arr, c, &p.model.geometry, p.settings.uses_mana2_stats()))
            .collect::<Vec<_>>();
        let score = p.score(&raws);
        Self {
            arr,
            pos,
            raws,
            score,
        }
    }

    fn copy_from(&mut self, s: &Self) {
        self.arr.clone_from(&s.arr);
        self.pos.clone_from(&s.pos);
        self.raws.clone_from(&s.raws);
        self.score = s.score;
    }
}

#[derive(Clone, Copy, Debug)]
struct Move {
    slots: [usize; 3],
    len: usize,
}

impl Move {
    fn pair(a: usize, b: usize) -> Self {
        Self {
            slots: [a, b, 0],
            len: 2,
        }
    }
}

#[inline(always)]
fn replace_gram<const PHYSICAL_ONLY: bool>(
    raw: &mut Raw,
    g: &Gram,
    old: &[usize],
    new: &[usize],
    geometry: &Geometry,
) {
    if PHYSICAL_ONLY {
        move_gram_physical(raw, g, old, new, geometry);
    } else {
        move_gram(raw, g, old, new, geometry);
    }
}

// Reuse scratch vectors and merge affected lists in place: no heap allocation
// per pair/cycle evaluation after the caller constructs its two States.
fn trial_into(dst: &mut State, old: &State, mv: Move, p: &Problem) {
    // Choose the counter set once per trial so the gram loop has no branch.
    if p.normal_counters_unused() {
        trial_counters::<true>(dst, old, mv, p);
    } else {
        trial_counters::<false>(dst, old, mv, p);
    }
}

#[inline(always)]
fn trial_counters<const PHYSICAL_ONLY: bool>(dst: &mut State, old: &State, mv: Move, p: &Problem) {
    dst.copy_from(old);
    dst.arr.swap(mv.slots[0], mv.slots[1]);
    if mv.len == 3 {
        dst.arr.swap(mv.slots[1], mv.slots[2]);
    }
    for &slot in &mv.slots[..mv.len] {
        dst.pos[dst.arr[slot]] = slot;
    }
    let x = old.arr[mv.slots[0]];
    let y = old.arr[mv.slots[1]];
    for (i, c) in p.corpora.iter().enumerate() {
        let pair = p.caches[i].pair(x, y);
        if mv.len == 2 {
            for &index in pair {
                replace_gram::<PHYSICAL_ONLY>(
                    &mut dst.raws[i],
                    &c.grams[index as usize],
                    &old.pos,
                    &dst.pos,
                    &p.model.geometry,
                );
            }
            continue;
        }
        let third = if mv.len == 3 {
            &c.by_symbol[old.arr[mv.slots[2]]][..]
        } else {
            &[]
        };
        let (mut a, mut b) = (0, 0);
        while a < pair.len() || b < third.len() {
            let index = if b == third.len() || (a < pair.len() && pair[a] < third[b]) {
                let j = pair[a];
                a += 1;
                j
            } else if a == pair.len() || third[b] < pair[a] {
                let j = third[b];
                b += 1;
                j
            } else {
                let j = pair[a];
                a += 1;
                b += 1;
                j
            };
            replace_gram::<PHYSICAL_ONLY>(
                &mut dst.raws[i],
                &c.grams[index as usize],
                &old.pos,
                &dst.pos,
                &p.model.geometry,
            );
        }
    }
    dst.score = p.score(&dst.raws);
}

fn checked_rescore(state: &mut State, p: &Problem) -> AppResult<()> {
    let exact = State::new(state.arr.clone(), p);
    let close = |a: f64, b: f64| b.is_finite() && (a - b).abs() <= 1e-7 + 1e-9 * a.abs();
    for (a, b) in exact.raws.iter().zip(&state.raws) {
        if p.normal_counters_unused() {
            // Trials maintained only the Mana2 counters; check those instead.
            let (a, b) = (a.1.as_ref().unwrap(), b.1.as_ref().unwrap());
            let speed = |raw: &mana2_metrics::RawStats| {
                raw.1.bigrams.into_iter().chain(raw.1.skipgrams)
            };
            if !a.0.iter().zip(&b.0).all(|(a, b)| close(*a, *b))
                || !speed(a).zip(speed(b)).all(|(a, b)| close(a, b))
            {
                return Err("incremental Mana2 counter mismatch".into());
            }
            continue;
        }
        for field in 0..N_RAW {
            if !close(a.0[field], b.0[field]) {
                return Err(format!("incremental counter mismatch, field {field}").into());
            }
        }
    }
    if !state.score.is_finite()
        || (state.score - exact.score).abs() > 1e-7 * (1.0 + exact.score.abs())
    {
        return Err("incremental objective mismatch".into());
    }
    state.copy_from(&exact);
    Ok(())
}

fn valid_arrangement(a: &[usize], n: usize) -> bool {
    if a.len() != n {
        return false;
    }
    let mut seen = vec![false; n];
    for &id in a {
        if id >= n || seen[id] {
            return false;
        }
        seen[id] = true;
    }
    true
}

#[derive(Clone)]
struct Candidate {
    arr: Vec<usize>,
    score: f64,
}

fn same_visible(a: &[usize], b: &[usize], canonical: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(&x, &y)| x == y || (blank(canonical[x]) && blank(canonical[y])))
}

fn arrangement_id(m: &Model, a: &[usize]) -> Vec<u8> {
    m.symbols(a)
        .into_iter()
        .map(|b| if blank(b) { 0 } else { b })
        .collect()
}

fn letter_distance(m: &Model, a: &[usize], b: &[usize]) -> usize {
    let pa = positions(a);
    let pb = positions(b);
    m.canonical
        .iter()
        .enumerate()
        .filter(|(i, ch)| ch.is_ascii_lowercase() && pa[*i] != pb[*i])
        .count()
}

fn known_layouts(m: &Model) -> BTreeSet<Vec<u8>> {
    let mut out = BTreeSet::new();
    out.insert(arrangement_id(m, &m.original));
    if let Ok(paths) = discover_layouts(Path::new(LAYOUT_DIR)) {
        for p in paths {
            if let Ok(b) = load_board(&p) {
                if b.symbols.len() == m.canonical.len() {
                    out.insert(
                        b.symbols
                            .into_iter()
                            .map(|b| if blank(b) { 0 } else { b })
                            .collect(),
                    );
                }
            }
        }
    }
    out
}

fn diverse_candidates(
    m: &Model,
    pool: &[Candidate],
    limit: usize,
    distance: usize,
    generation: bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for c in pool {
        if out.iter().any(|x| {
            if generation {
                letter_distance(m, &x.arr, &c.arr) < distance.max(1)
            } else {
                same_visible(&x.arr, &c.arr, &m.canonical)
            }
        }) {
            continue;
        }
        out.push(c.clone());
        if out.len() == limit {
            break;
        }
    }
    out
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64) * (1.0 / 9007199254740992.0)
    }

    fn index(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn movement(&mut self, free: &[usize], cycles: f64) -> Move {
        let a = self.index(free.len());
        let mut b = self.index(free.len() - 1);
        if b >= a {
            b += 1;
        }
        if free.len() >= 3 && self.unit() < cycles {
            let mut c = self.index(free.len());
            while c == a || c == b {
                c = self.index(free.len());
            }
            Move {
                slots: [free[a], free[b], free[c]],
                len: 3,
            }
        } else {
            Move::pair(free[a], free[b])
        }
    }
}

fn shuffle(a: &mut [usize], free: &[usize], rng: &mut Rng) {
    for i in (1..free.len()).rev() {
        let j = rng.index(i + 1);
        a.swap(free[i], free[j]);
    }
}

fn crossover(a: &[usize], b: &[usize], free: &[usize], rng: &mut Rng) -> Vec<usize> {
    let mut out = a.to_vec();
    if free.len() < 2 {
        return out;
    }
    let lo = rng.index(free.len());
    let hi = lo + 1 + rng.index(free.len() - lo);
    let mut used = vec![false; a.len()];
    for &slot in &free[lo..hi] {
        used[a[slot]] = true;
    }
    let mut next = hi % free.len();
    for k in 0..free.len() {
        let v = b[free[(hi + k) % free.len()]];
        if used[v] {
            continue;
        }
        if next >= lo && next < hi {
            next = hi % free.len();
        }
        out[free[next]] = v;
        used[v] = true;
        next = (next + 1) % free.len();
    }
    out
}

#[derive(Clone, Debug, Default)]
struct Progress {
    phase: String,
    restart: usize,
    pass: usize,
    evaluations: u64,
    accepted: u64,
    elapsed: f64,
    seed: u64,
}

#[derive(Clone)]
struct Snapshot {
    best: Candidate,
    progress: Progress,
    archive: Vec<Candidate>,
    has_best: bool,
}

enum SearchMessage {
    Update(usize, Snapshot),
    Done(usize, Result<Snapshot, String>),
}

struct Control {
    cancel: AtomicBool,
    pause: AtomicBool,
}

impl Control {
    fn new() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
        }
    }
}

struct SearchRuntime<'a> {
    started: Instant,
    deadline: Option<Instant>,
    last_frame: Instant,
    progress: Progress,
    control: &'a Control,
    callback: &'a mut dyn FnMut(Snapshot),
    archive: Vec<Candidate>,
    pool: Vec<Candidate>,
    best: State,
    has_best: bool,
    known: Option<BTreeSet<Vec<u8>>>,
    template: Vec<usize>,
}

impl<'a> SearchRuntime<'a> {
    fn stop(&self, _p: &Problem) -> bool {
        self.control.cancel.load(Ordering::Relaxed)
            || self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn checkpoint(&self, p: &Problem) -> bool {
        while self.control.pause.load(Ordering::Relaxed) && !self.stop(p) {
            thread::sleep(Duration::from_millis(15));
        }
        self.stop(p)
    }

    fn frame(&mut self, force: bool) {
        if force || self.last_frame.elapsed() >= Duration::from_millis(100) {
            self.progress.elapsed = self.started.elapsed().as_secs_f64();
            self.last_frame = Instant::now();
            let snapshot = self.snapshot();
            (self.callback)(snapshot);
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            best: Candidate {
                arr: self.best.arr.clone(),
                score: self.best.score,
            },
            progress: self.progress.clone(),
            archive: self.archive.clone(),
            has_best: self.has_best,
        }
    }

    fn keep(&mut self, s: &State, p: &Problem) {
        if !s.score.is_finite() || !p.feasible(&s.raws) {
            return;
        }
        if let Some(known) = &self.known {
            if letter_distance(&p.model, &self.template, &s.arr) < p.settings.diversity
                || known.contains(&arrangement_id(&p.model, &s.arr))
            {
                return;
            }
        }
        if !self.has_best || s.score < self.best.score - 1e-10 {
            self.best.copy_from(s);
            self.has_best = true;
        }
        if self
            .pool
            .iter()
            .any(|c| same_visible(&c.arr, &s.arr, &p.model.canonical))
        {
            return;
        }
        self.pool.push(Candidate {
            arr: s.arr.clone(),
            score: s.score,
        });
        self.pool
            .sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
        self.pool.truncate(256usize.max(p.settings.archive * 16));
        self.archive = diverse_candidates(
            &p.model,
            &self.pool,
            p.settings.archive,
            p.settings.diversity,
            self.known.is_some(),
        );
    }

    fn accept(&mut self, s: &State, p: &Problem) {
        self.progress.accepted += 1;
        if !self.has_best || s.score < self.best.score - 1e-10 {
            self.keep(s, p);
        }
    }
}

/// Uphill acceptance probabilities targeted by automatic temperatures: the
/// median sampled uphill move at the start, the 10th percentile at the end.
fn sweep(
    cur: &mut State,
    tmp: &mut State,
    free: &[usize],
    p: &Problem,
    rt: &mut SearchRuntime<'_>,
) -> AppResult<()> {
    let mut good = Vec::with_capacity(free.len() * free.len() / 2);
    for pass in 0..p.settings.passes {
        if rt.checkpoint(p) {
            break;
        }
        rt.progress.pass = pass + 1;
        rt.progress.phase = "sweep".into();
        good.clear();
        for i in 0..free.len() {
            for j in i + 1..free.len() {
                if rt.checkpoint(p) {
                    return Ok(());
                }
                let mv = Move::pair(free[i], free[j]);
                trial_into(tmp, cur, mv, p);
                rt.progress.evaluations += 1;
                if tmp.score < cur.score - 1e-10 && p.feasible(&tmp.raws) {
                    good.push((tmp.score - cur.score, mv));
                }
                rt.frame(false);
            }
        }
        if good.is_empty() {
            break;
        }
        good.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut changed = false;
        for (_, mv) in &good {
            if rt.checkpoint(p) {
                return Ok(());
            }
            trial_into(tmp, cur, *mv, p);
            rt.progress.evaluations += 1;
            if tmp.score < cur.score - 1e-10 && p.feasible(&tmp.raws) {
                cur.copy_from(tmp);
                changed = true;
                rt.accept(cur, p);
            }
        }
        checked_rescore(cur, p)?;
        rt.frame(false);
        if !changed {
            break;
        }
    }
    Ok(())
}

fn repair(
    cur: &mut State,
    tmp: &mut State,
    free: &[usize],
    p: &Problem,
    rt: &mut SearchRuntime<'_>,
) -> AppResult<bool> {
    if p.feasible(&cur.raws) {
        return Ok(true);
    }
    rt.progress.phase = "repair".into();
    for _ in 0..p.settings.passes.min(32) {
        let mut best = p.violation(&cur.raws);
        let mut movement = None;
        for i in 0..free.len() {
            for j in i + 1..free.len() {
                if rt.checkpoint(p) {
                    return Ok(false);
                }
                let mv = Move::pair(free[i], free[j]);
                trial_into(tmp, cur, mv, p);
                rt.progress.evaluations += 1;
                let v = p.violation(&tmp.raws);
                if v < best {
                    best = v;
                    movement = Some(mv);
                }
                rt.frame(false);
            }
        }
        if let Some(mv) = movement {
            trial_into(tmp, cur, mv, p);
            cur.copy_from(tmp);
            rt.progress.accepted += 1;
        } else {
            break;
        }
        if p.feasible(&cur.raws) {
            checked_rescore(cur, p)?;
            return Ok(true);
        }
    }
    checked_rescore(cur, p)?;
    Ok(false)
}

fn run_search_plan(
    p: &Problem,
    seed_arr: &[usize],
    locked: &[bool],
    control: &Control,
    search_seed: u64,
    restarts: usize,
    restart_offset: usize,
    started: Instant,
    deadline: Option<Instant>,
    callback: &mut dyn FnMut(Snapshot),
) -> AppResult<Snapshot> {
    if !valid_arrangement(seed_arr, p.model.canonical.len()) || locked.len() != seed_arr.len() {
        return Err("invalid search arrangement or lock mask".into());
    }
    let initial = State::new(seed_arr.to_vec(), p);
    let generation = p.settings.design != "refine";
    if !generation && !p.feasible(&initial.raws) {
        return Err("starting arrangement violates active limits".into());
    }
    let free: Vec<_> = (0..locked.len()).filter(|&i| !locked[i]).collect();
    let mut rng = Rng::new(search_seed);
    let mut rt = SearchRuntime {
        started,
        deadline,
        last_frame: started,
        progress: Progress {
            seed: search_seed,
            ..Progress::default()
        },
        control,
        callback,
        archive: Vec::new(),
        pool: Vec::new(),
        best: initial.clone(),
        has_best: false,
        known: if generation {
            Some(known_layouts(&p.model))
        } else {
            None
        },
        template: seed_arr.to_vec(),
    };
    if !generation {
        rt.keep(&initial, p);
    }
    let letter_count = free
        .iter()
        .filter(|&&pos| p.model.canonical[seed_arr[pos]].is_ascii_lowercase())
        .count();
    if free.len() < 2 || generation && p.settings.diversity > letter_count {
        return Ok(rt.snapshot());
    }
    let mut cur = initial.clone();
    let mut tmp = initial.clone();
    let mut local = initial.clone();
    for restart in 0..restarts {
        if rt.checkpoint(p) {
            break;
        }
        rt.progress.restart = restart + 1;
        rt.progress.pass = 0;
        if generation {
            let parents =
                diverse_candidates(&p.model, &rt.pool, 64, p.settings.diversity.max(2), true);
            let mut a = seed_arr.to_vec();
            if p.settings.design == "evolve" && parents.len() >= 2 && rng.unit() >= 0.30 {
                let x = rng.index(parents.len());
                let mut y = rng.index(parents.len() - 1);
                if y >= x {
                    y += 1;
                }
                a = crossover(&parents[x].arr, &parents[y].arr, &free, &mut rng);
                let kicks = 2 + rng.index((free.len() / 5).max(1));
                for _ in 0..kicks {
                    let mv = rng.movement(&free, 0.20);
                    a.swap(mv.slots[0], mv.slots[1]);
                    if mv.len == 3 {
                        a.swap(mv.slots[1], mv.slots[2]);
                    }
                }
            } else {
                shuffle(&mut a, &free, &mut rng);
            }
            cur = State::new(a, p);
            if !repair(&mut cur, &mut tmp, &free, p, &mut rt)? {
                continue;
            }
        } else {
            let global_restart = restart_offset + restart;
            cur.copy_from(if global_restart == 0 { &initial } else { &rt.best });
            if global_restart > 0 {
                for _ in 0..48 {
                    if rt.checkpoint(p) {
                        break;
                    }
                    trial_into(&mut tmp, &cur, rng.movement(&free, 0.25), p);
                    rt.progress.evaluations += 1;
                    if p.feasible(&tmp.raws) {
                        cur.copy_from(&tmp);
                    }
                }
            }
        }
        if p.settings.hybrid {
            rt.progress.phase = "anneal".into();
            let (start_temperature, end_temperature) =
                (p.settings.temperature, p.settings.cooling_end);
            local.copy_from(&cur);
            for step in 0..p.settings.anneal_steps {
                if rt.checkpoint(p) {
                    break;
                }
                let fraction =
                    step as f64 / p.settings.anneal_steps.saturating_sub(1).max(1) as f64;
                let temp =
                    start_temperature * (end_temperature / start_temperature).powf(fraction);
                trial_into(
                    &mut tmp,
                    &cur,
                    rng.movement(&free, p.settings.cycle_probability),
                    p,
                );
                rt.progress.evaluations += 1;
                let d = tmp.score - cur.score;
                if p.feasible(&tmp.raws) && (d < 0.0 || rng.unit() < (-d / temp).exp()) {
                    cur.copy_from(&tmp);
                    rt.accept(&cur, p);
                    if cur.score < local.score {
                        local.copy_from(&cur);
                    }
                }
                if step % 256 == 0 {
                    checked_rescore(&mut cur, p)?;
                }
                rt.frame(false);
            }
            cur.copy_from(&local);
        }
        sweep(&mut cur, &mut tmp, &free, p, &mut rt)?;
        checked_rescore(&mut cur, p)?;
        rt.keep(&cur, p);
        rt.frame(true);
    }
    if rt.has_best {
        checked_rescore(&mut rt.best, p)?;
    }
    for c in &rt.archive {
        if !valid_arrangement(&c.arr, seed_arr.len()) {
            return Err("generated permutation is invalid".into());
        }
        for (i, &lock) in locked.iter().enumerate() {
            if lock && c.arr[i] != seed_arr[i] {
                return Err("locked key moved".into());
            }
        }
    }
    rt.progress.phase = if control.cancel.load(Ordering::Relaxed) {
        "stopped"
    } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        "time limit"
    } else {
        "finished"
    }
    .into();
    rt.progress.elapsed = started.elapsed().as_secs_f64();
    Ok(rt.snapshot())
}

fn search_deadline(started: Instant, seconds: f64) -> Option<Instant> {
    (seconds > 0.0).then(|| started + Duration::from_secs_f64(seconds))
}

fn run_search(
    p: &Problem,
    seed_arr: &[usize],
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot),
) -> AppResult<Snapshot> {
    let started = Instant::now();
    run_search_plan(
        p,
        seed_arr,
        locked,
        control,
        p.settings.seed,
        p.settings.restarts,
        0,
        started,
        search_deadline(started, p.settings.seconds),
        callback,
    )
}

fn merge_search_snapshots(
    p: &Problem,
    snapshots: &[Option<Snapshot>],
    newest: usize,
    started: Instant,
    final_phase: Option<&str>,
) -> Option<Snapshot> {
    let fallback = snapshots.iter().flatten().next()?;
    let mut best = None::<Candidate>;
    let mut pool = Vec::new();
    let mut progress = Progress {
        phase: final_phase
            .map(str::to_owned)
            .unwrap_or_else(|| snapshots[newest].as_ref().unwrap_or(fallback).progress.phase.clone()),
        elapsed: started.elapsed().as_secs_f64(),
        seed: p.settings.seed,
        ..Progress::default()
    };
    for snapshot in snapshots.iter().flatten() {
        progress.restart += snapshot.progress.restart;
        progress.pass = progress.pass.max(snapshot.progress.pass);
        progress.evaluations += snapshot.progress.evaluations;
        progress.accepted += snapshot.progress.accepted;
        pool.extend(snapshot.archive.iter().cloned());
        if snapshot.has_best {
            let candidate = snapshot.best.clone();
            pool.push(candidate.clone());
            if best.as_ref().is_none_or(|current| {
                candidate
                    .score
                    .total_cmp(&current.score)
                    .then_with(|| candidate.arr.cmp(&current.arr))
                    .is_lt()
            }) {
                best = Some(candidate);
            }
        }
    }
    pool.sort_by(|a, b| a.score.total_cmp(&b.score).then_with(|| a.arr.cmp(&b.arr)));
    let archive = diverse_candidates(
        &p.model,
        &pool,
        p.settings.archive,
        p.settings.diversity,
        p.settings.design != "refine",
    );
    let has_best = best.is_some();
    Some(Snapshot {
        best: best.unwrap_or_else(|| fallback.best.clone()),
        progress,
        archive,
        has_best,
    })
}

fn run_search_parallel(
    p: &Problem,
    seed_arr: &[usize],
    locked: &[bool],
    control: &Control,
    callback: &mut dyn FnMut(Snapshot),
) -> AppResult<Snapshot> {
    let workers = search_worker_count(&p.settings);
    if workers <= 1 {
        return run_search(p, seed_arr, locked, control, callback);
    }

    let started = Instant::now();
    let deadline = search_deadline(started, p.settings.seconds);
    let mut snapshots = vec![None; workers];
    let mut errors = Vec::<(usize, String)>::new();
    let (tx, rx) = mpsc::channel();
    let scoped = thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::with_capacity(workers);
        for worker in 0..workers {
            let tx = tx.clone();
            let restarts = p.settings.restarts / workers
                + usize::from(worker < p.settings.restarts % workers);
            let restart_offset = worker * (p.settings.restarts / workers)
                + worker.min(p.settings.restarts % workers);
            handles.push((
                worker,
                scope.spawn(move || {
                    let mut worker_callback = |snapshot| {
                        let _ = tx.send(SearchMessage::Update(worker, snapshot));
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_search_plan(
                            p,
                            seed_arr,
                            locked,
                            control,
                            worker_seed(p.settings.seed, worker),
                            restarts,
                            restart_offset,
                            started,
                            deadline,
                            &mut worker_callback,
                        )
                    }))
                    .map_err(|_| "search worker panicked".to_owned())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                    let _ = tx.send(SearchMessage::Done(worker, result));
                }),
            ));
        }
        drop(tx);

        for message in rx {
            let worker = match message {
                SearchMessage::Update(worker, snapshot) => {
                    snapshots[worker] = Some(snapshot);
                    worker
                }
                SearchMessage::Done(worker, Ok(snapshot)) => {
                    snapshots[worker] = Some(snapshot);
                    continue;
                }
                SearchMessage::Done(worker, Err(error)) => {
                    control.cancel.store(true, Ordering::Relaxed);
                    errors.push((worker, error));
                    continue;
                }
            };
            if let Some(snapshot) =
                merge_search_snapshots(p, &snapshots, worker, started, None)
            {
                callback(snapshot);
            }
        }
        for (worker, handle) in handles {
            if handle.join().is_err() {
                errors.push((worker, "search worker panicked".into()));
            }
        }
        Ok(())
    });
    if let Err(error) = scoped {
        return Err(error.into());
    }
    if !errors.is_empty() {
        errors.sort_by_key(|(worker, _)| *worker);
        return Err(errors.remove(0).1.into());
    }

    let phase = if control.cancel.load(Ordering::Relaxed) {
        "stopped"
    } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        "time limit"
    } else {
        "finished"
    };
    merge_search_snapshots(p, &snapshots, 0, started, Some(phase))
        .ok_or_else(|| "search workers exited without a result".into())
}
