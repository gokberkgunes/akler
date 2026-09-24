const RAW_CORPUS_DIR: &str = "corpus/raw";

const PROCESSED_CORPUS_DIR: &str = "corpus/processed";

const CORPUS_VERSION: u64 = 1;

const DEFAULT_ORDER: usize = 5;

const TOKEN_CHUNK: usize = 4*1024*1024;

fn fingerprint_bytes(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325, |h, &b|(h^b as u64).wrapping_mul(0x100000001b3))
}

fn corpus_name(path: &Path) -> String {
    label(path, "corpus-")
}

fn corpus_dirs() -> AppResult<()> {
    fs::create_dir_all(RAW_CORPUS_DIR)?;
    fs::create_dir_all(PROCESSED_CORPUS_DIR)?;
    Ok(())
}

fn is_corpus_config_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".config.json"))
}

fn frequency_json_header(path: &Path) -> AppResult<bool> {
    let mut header = Vec::with_capacity(8192);
    File::open(path)?.take(8192).read_to_end(&mut header)?;
    let header = String::from_utf8_lossy(&header);
    let header = header.trim_start_matches('\u{feff}').trim_start();
    let has_frequency_field = [
        "\"letters\"",
        "\"monograms\"",
        "\"unigrams\"",
        "\"bigrams\"",
        "\"skipgrams\"",
        "\"trigrams\"",
        "\"fourgrams\"",
        "\"quadrigrams\"",
        "\"quadgrams\"",
        "\"tetragrams\"",
        "\"fivegrams\"",
    ]
    .iter()
    .any(|field| {
        header.match_indices(*field).any(|(offset, _)| {
            header[offset + field.len()..]
                .trim_start()
                .starts_with(':')
        })
    });

    Ok(header.starts_with('{') && has_frequency_field)
}

// Files kept in corpus/raw are text sources regardless of their suffix. For a
// direct path, preserve the established frequency-JSON routes and use a bounded
// header check for imported frequency tables whose filename is unconventional.
fn corpus_is_raw(path: &Path) -> AppResult<bool> {
    if is_corpus_config_path(path) {
        return Ok(false);
    }
    if path
        .parent()
        .is_some_and(|parent| parent.ends_with(Path::new(RAW_CORPUS_DIR)))
    {
        return Ok(true);
    }
    let extension = path.extension().and_then(|extension| extension.to_str());
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("json")) {
        return Ok(false);
    }
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("txt")) {
        return Ok(true);
    }

    if frequency_json_header(path)? {
        return Ok(false);
    }

    Ok(true)
}

fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c<' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c)
        }
    }
    out.push('"');
    out
}

// Corpus loading retains only tables used by the objective. Higher-order tables
// remain in the JSON cache and are loaded on demand by `corpus top`.
fn read_frequency_table(p: &mut JsonParser<'_>, width: usize, keep: bool) -> AppResult<Vec<(String, f64)>> {
    p.ws();
    p.expect(b'{')?;
    p.ws();
    let mut values = Vec::new();
    let mut seen = BTreeSet::new();
    if p.byte() == Some(b'}') {
        p.p += 1;
        return Ok(values);
    }
    loop {
        p.ws();
        let name = p.string()?;
        p.ws();
        p.expect(b':')?;
        let value = match p.value(0)? {
            Json::Number(v) if v >= 0.0 => v,
            _ => return Err("corpus count must be finite and non-negative".into())
        };
        if name.chars().count() != width {
            return Err(format!("invalid {width}-gram {name:?}").into());
        }
        if keep {
            if !seen.insert(name.clone()) {
                return Err(format!("duplicate n-gram {name:?}").into());
            }
            values.push((name, value));
        }
        p.ws();
        if p.byte() == Some(b'}') {
            p.p += 1;
            break;
        }
        p.expect(b',')?;
    }
    Ok(values)
}

// Selection never changes the order of surviving entries. Ties use the
// n-gram itself, so limits do not depend on JSON field order.
fn top_ngram_keys<K: Ord>(entries: impl IntoIterator<Item = (K, f64)>, limit: usize) -> BTreeSet<K> {
    let mut ranked: Vec<_> = entries.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(limit);
    ranked.into_iter().map(|(key, _)| key).collect()
}

fn ngram_limit_warning(order: usize, retained: usize, original: usize, mass: f64, original_mass: f64) -> String {
    let percent = if original_mass > 0.0 { (mass / original_mass) * 100.0 } else { 100.0 };
    format!("Approximate {order}-grams: {retained}/{original} sequences retained ({percent:.2}% source frequency).")
}

fn load_frequency_source(text: &str, path: &Path) -> AppResult<Source> {
    load_frequency_source_with_limits(text, path, NgramLimits::default())
}

fn load_frequency_source_with_limits(text: &str, path: &Path, limits: NgramLimits) -> AppResult<Source> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = JsonParser {
        text,
        p: 0
    };
    p.ws();
    p.expect(b'{')?;
    let mut tables: [Vec<(String, f64)>; 4] = std::array::from_fn(|_|Vec::new());
    let mut got = [false; 4];
    let mut root_seen = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut uni_name = String::new();
    let mut options = None;
    loop {
        p.ws();
        if p.byte() == Some(b'}') {
            p.p += 1;
            break;
        }
        let key = p.string()?;
        if !root_seen.insert(key.clone()) {
            return Err(format!("duplicate JSON field {key}").into());
        }
        p.ws();
        p.expect(b':')?;
        let kind = match key.as_str() {
            "letters"|"monograms"|"unigrams" => Some(0),
            "bigrams" => Some(1),
            "skipgrams" => Some(2),
            "trigrams" => Some(3),
            _ => None
        };
        if let Some(i) = kind {
            let v = read_frequency_table(&mut p, [1, 2, 2, 3][i], true)?;
            if i == 0 && got[0] {
                if key == "letters" {
                    tables[0] = v;
                    uni_name = key.clone();
                }
                warnings.push(format!("Multiple unigram tables; using {uni_name}."));
            } else {
                tables[i] = v;
                got[i] = true;
                if i == 0 {
                    uni_name = key.clone();
                }
            }
        } else if matches!(key.as_str(), "fourgrams"|"quadrigrams"|"quadgrams"|"tetragrams"|"fivegrams") {
            read_frequency_table(&mut p, if key == "fivegrams" {
                5
            } else {
                4
            }, false)?;
        } else {
            let value = p.value(0)?;
            if key == "options" {
                options = Some(value);
            }
        }
        p.ws();
        if p.byte() == Some(b'}') {
            p.p += 1;
            break;
        }
        p.expect(b',')?;
    }
    p.ws();
    if p.p != text.len() {
        return Err("trailing corpus JSON data".into());
    }
    if !got[0]||!got[1] {
        return Err("corpus requires letters/monograms and bigrams".into());
    }
    if !got[2] {
        if got[3] {
            let mut skip = BTreeMap::new();
            for (s, v) in &tables[3] {
                let cs: Vec<char> = s.chars().collect();
                *skip.entry(format!("{}{}", cs[0], cs[2])).or_insert(0.0) += v;
            }
            tables[2] = skip.into_iter().collect();
            warnings.push("Skipgrams derived from stored trigrams; pruning in the source file limits skip evidence.".into());
        } else {
            warnings.push("No skipgrams or trigrams in source.".into());
        }
    }
    let masses: [f64; 4] = std::array::from_fn(|i|tables[i].iter().map(|x|x.1).sum:: <f64>());
    if masses.iter().any(|n|!n.is_finite()) {
        return Err("corpus frequency sum overflow".into());
    }
    if let Some(Json::Object(o)) = options {
        if let Some(Json::Number(n)) = o.get("trigramsMax") {
            if *n>0.0 && tables[3].len()>=*n as usize {
                warnings.push(format!("Source retains at most {n} trigrams."));
            }
        }
        if let Some(Json::Bool(false)) = o.get("includeSpacegrams") {
            warnings.push("Source excludes spacegrams.".into());
        }
    }

    // Skipgrams, including those derived above, retain all stored evidence.
    // Only the ordinary evaluator's trigram population is approximated.
    if let Some(limit) = limits.trigrams {
        if limit == 0 {
            return Err("trigram limit must be positive or all".into());
        }
        if tables[3].len() > limit {
            let original = tables[3].len();
            let selected = top_ngram_keys(tables[3].iter().cloned(), limit);
            tables[3].retain(|(gram, _)| selected.contains(gram));
            let mass = tables[3].iter().map(|(_, f)| *f).sum();
            warnings.push(ngram_limit_warning(3, tables[3].len(), original, mass, masses[3]));
        }
    }

    Ok(Source {
        name: corpus_name(path),
        path: path.to_owned(),
        tables,
        masses,
        warnings,
        fingerprint: fingerprint_bytes(text.as_bytes())
    })
}

#[derive(Clone)]
struct CorpusConfig {
    ascii: [u8; 256],
    unicode: BTreeMap<char, u8>,
    separator: u8,
    min_length: usize,
    order: usize,
    jobs: usize
}

impl Default for CorpusConfig {
    fn default() -> Self {
        let mut ascii = [0; 256];
        for i in 32u8..=126 {
            ascii[i as usize] = i.to_ascii_lowercase();
        }
        Self {
            ascii,
            unicode: BTreeMap::new(),
            separator: b' ',
            min_length: 1,
            order: DEFAULT_ORDER,
            jobs: thread::available_parallelism().map(|n|n.get()).unwrap_or(1).min(4)
        }
    }
}

fn corpus_config(path: &Path) -> AppResult<(CorpusConfig, u64)> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e)if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into())
    };
    let mut c = CorpusConfig::default();
    let fp = fingerprint_bytes(&bytes);
    if bytes.is_empty() {
        return Ok((c, fp));
    }
    let root = match parse_json(std::str::from_utf8(&bytes)?)? {
        Json::Object(v) => v,
        _ => return Err("corpus config must be an object".into())
    };
    for k in root.keys() {
        if !["input_graphemes", "normalization", "word_separator", "min_sequence_length", "max_order", "jobs"].contains(&k.as_str()) {
            return Err(format!("unsupported corpus option {k}").into());
        }
    }
    if let Some(value) = root.get("word_separator") {
        let s = match value {
            Json::String(s) => s,
            _ => return Err("word_separator must be a string".into())
        };
        if s.len() != 1||!s.as_bytes()[0].is_ascii_graphic() && s != " " {
            return Err("word_separator must be one printable ASCII character".into());
        }
        c.separator = s.as_bytes()[0];
    }
    if let Some(v) = root.get("input_graphemes") {
        let list = match v {
            Json::Array(v) => v,
            _ => return Err("input_graphemes must be an array".into())
        };
        c.ascii = [0; 256];
        for v in list {
            let s = match v {
                Json::String(s) => s,
                _ => return Err("input_graphemes entries must be strings".into())
            };
            if s.len() != 1 ||(!s.as_bytes()[0].is_ascii_graphic() && s != " ") {
                return Err("plain layouts require single printable ASCII output characters".into());
            }
            c.ascii[s.as_bytes()[0] as usize] = s.as_bytes()[0];
        }
        c.ascii[c.separator as usize] = c.separator;
    }
    if let Some(v) = root.get("normalization") {
        let obj = match v {
            Json::Object(o) => o,
            _ => return Err("normalization must be an object".into())
        };
        for (input, output) in obj {
            let mut chars = input.chars();
            let ch = chars.next().ok_or("empty normalization input")?;
            if chars.next().is_some() {
                return Err("normalization input must be one Unicode scalar".into());
            }
            let s = match output {
                Json::String(s) => s,
                _ => return Err("normalization output must be a string".into())
            };
            if s.len() != 1 || s.as_bytes()[0]<32 || s.as_bytes()[0]>126 {
                return Err("normalization output must be one printable ASCII character".into());
            }
            let b = s.as_bytes()[0];
            if c.ascii[b as usize] == 0 {
                return Err("normalization output is not accepted in input_graphemes".into());
            }
            if ch.is_ascii() {
                c.ascii[ch as usize] = b;
            } else {
                c.unicode.insert(ch, b);
            }
        }
    }
    for (name, lo, hi) in [("min_sequence_length", 1, 1_000_000),("max_order", 3, 5),("jobs", 1, 16)] {
        if let Some(v) = root.get(name) {
            let n = match v {
                Json::Number(v)if v.fract() == 0.0&&*v >= lo as f64&&*v <= hi as f64=>*v as usize,
                _ => return Err(format!("{name} must be {lo}..{hi}").into())
            };
            match name {
                "min_sequence_length" => c.min_length = n,
                "max_order" => c.order = n,
                _ => c.jobs = n
            }
        }
    }
    Ok((c, fp))
}

#[derive(Default)]
struct GramHasher(u64);

impl std::hash::Hasher for GramHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = fingerprint_bytes(bytes);
    }

    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }

    fn write_u64(&mut self, mut v: u64) {
        v^=v>>30;
        v = v.wrapping_mul(0xbf58476d1ce4e5b9);
        v^=v>>27;
        v = v.wrapping_mul(0x94d049bb133111eb);
        self.0 = v^(v>>31);
    }
}

type GramMap = HashMap<u64, u64, std::hash::BuildHasherDefault<GramHasher>>;

struct Counts {
    uni: Vec<u64>,
    bi: Vec<u64>,
    skip: Vec<u64>,
    tri: Vec<u64>,
    quad: GramMap,
    five: GramMap,
    order: usize,
    window: u64,
    len: usize
}

impl Counts {
    fn new(order: usize) -> Self {
        Self {
            uni: vec![0; 128],
            bi: vec![0; 1<<14],
            skip: vec![0; 1<<14],
            tri: vec![0; 1<<21],
            quad: GramMap::default(),
            five: GramMap::default(),
            order,
            window: 0,
            len: 0
        }
    }
    #[inline]
    fn warm(&mut self, b: u8) {
        if b == 0 {
            self.window = 0;
            self.len = 0;
        } else {
            self.window = ((self.window<<7)|b as u64)&((1u64<<35) - 1);
            self.len = (self.len + 1).min(5);
        }
    }
    #[inline]
    fn push(&mut self, b: u8) {
        self.warm(b);
        if b == 0 {
            return;
        }
        self.uni[b as usize] += 1;
        if self.len >= 2 {
            self.bi[(self.window&0x3fff) as usize] += 1;
        }
        if self.len >= 3 {
            self.tri[(self.window&0x1fffff) as usize] += 1;
            let k = (((self.window>>14)&127)<<7)|(self.window&127);
            self.skip[k as usize] += 1;
        }
        if self.order >= 4 && self.len >= 4 {
            *self.quad.entry(self.window&0xfffffff).or_insert(0) += 1;
        }
        if self.order >= 5 && self.len >= 5 {
            *self.five.entry(self.window).or_insert(0) += 1;
        }
    }

    fn merge(&mut self, other: Counts) -> AppResult<()> {
        for (dst, src) in [&mut self.uni, &mut self.bi, &mut self.skip, &mut self.tri].into_iter().zip([other.uni, other.bi, other.skip, other.tri]) {
            for (a, b) in dst.iter_mut().zip(src) {
                *a = a.checked_add(b).ok_or("n-gram count overflow")?;
            }
        }
        for (dst, src) in [(&mut self.quad, other.quad),(&mut self.five, other.five)] {
            for (k, v) in src {
                let a = dst.entry(k).or_insert(0);
                *a = a.checked_add(v).ok_or("n-gram count overflow")?;
            }
        }
        Ok(())
    }

    fn rows(&self) -> [usize; 5] {
        [self.uni.iter().filter(|&&v|v != 0).count(), self.bi.iter().filter(|&&v|v != 0).count(), self.tri.iter().filter(|&&v|v != 0).count(), self.quad.len(), self.five.len()]
    }
}

// Spaces are deferred, collapsed and trimmed at sequence boundaries, matching
// the former plain-text path. Refill boundaries do not terminate a sequence.
struct Normalizer {
    cfg: CorpusConfig,
    has_word: bool,
    pending_space: bool,
    pending: Vec<u8>,
    started: bool,
    utf: [u8; 4],
    utf_len: usize,
    utf_need: usize,
    source_bytes: u64,
    hash: u64,
    invalid: u64,
}

impl Normalizer {
    fn new(cfg: CorpusConfig) -> Self {
        Self {
            cfg,
            has_word: false,
            pending_space: false,
            pending: Vec::new(),
            started: false,
            utf: [0; 4],
            utf_len: 0,
            utf_need: 0,
            source_bytes: 0,
            hash: 0xcbf29ce484222325,
            invalid: 0
        }
    }

    fn output(&mut self, b: u8, emit: &mut impl FnMut(u8) -> AppResult<()>) -> AppResult<()> {
        if self.started {
            emit(b)?;
        } else {
            self.pending.push(b);
            if self.pending.len() >= self.cfg.min_length {
                for x in self.pending.drain(..) {
                    emit(x)?;
                }
                self.started = true;
            }
        }
        Ok(())
    }

    fn mapped(&mut self, b: u8, emit: &mut impl FnMut(u8) -> AppResult<()>) -> AppResult<()> {
        if b == 0 {
            if self.started {
                emit(0)?;
            }
            self.pending.clear();
            self.started = false;
            self.has_word = false;
            self.pending_space = false;
        } else if b == self.cfg.separator {
            if self.has_word {
                self.pending_space = true;
            }
        } else {
            if self.pending_space {
                self.output(self.cfg.separator, emit)?;
                self.pending_space = false;
            }
            self.output(b, emit)?;
            self.has_word = true;
        }
        Ok(())
    }

    fn read(&mut self, bytes: &[u8], emit: &mut impl FnMut(u8) -> AppResult<()>) -> AppResult<()> {
        for &b in bytes {
            self.source_bytes += 1;
            self.hash = (self.hash^b as u64).wrapping_mul(0x100000001b3);
            if self.utf_need != 0 {
                if b&0xc0 == 0x80 {
                    self.utf[self.utf_len] = b;
                    self.utf_len += 1;
                    if self.utf_len == self.utf_need {
                        let ch = std::str::from_utf8(&self.utf[..self.utf_len]).ok().and_then(|s|s.chars().next());
                        let v = ch.and_then(|c|self.cfg.unicode.get(&c).copied()).unwrap_or(0);
                        if v == 0 {
                            self.invalid += 1;
                        }
                        self.utf_need = 0;
                        self.utf_len = 0;
                        self.mapped(v, emit)?;
                    }
                    continue;
                }
                self.utf_need = 0;
                self.utf_len = 0;
                self.invalid += 1;
                self.mapped(0, emit)?;
            }
            if b<128 {
                self.mapped(self.cfg.ascii[b as usize], emit)?;
            }
            else {
                let need = match b {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 0
                };
                if need == 0 {
                    self.invalid += 1;
                    self.mapped(0, emit)?;
                } else {
                    self.utf[0] = b;
                    self.utf_len = 1;
                    self.utf_need = need;
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, emit: &mut impl FnMut(u8) -> AppResult<()>) -> AppResult<()> {
        if self.utf_need != 0 {
            self.invalid += 1;
            self.utf_need = 0;
            self.utf_len = 0;
        }
        self.mapped(0, emit)
    }
}

struct TokenBlock {
    prefix: Vec<u8>,
    bytes: Vec<u8>
}

fn count_reader<R: Read>(mut input: R, cfg: &CorpusConfig, total: u64, notify: &mut dyn FnMut(u64, u64)) -> AppResult<(Counts, u64, u64)> {
    let jobs = if total>0 && total<8*1024*1024 {
        1
    } else {
        cfg.jobs.max(1)
    };
    let mut normalizer = Normalizer::new(cfg.clone());
    let mut buf = vec![0; 1<<20];
    if jobs == 1 {
        let mut out = Counts::new(cfg.order);
        let mut emit=|b| {
            out.push(b);
            Ok(())
        };
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            normalizer.read(&buf[..n], &mut emit)?;
            notify(normalizer.source_bytes, total);
        }
        normalizer.finish(&mut emit)?;
        return Ok((out, normalizer.hash, normalizer.invalid));
    }
    let mut txs = Vec::new();
    let mut workers = Vec::new();
    for _ in 0..jobs {
        let (tx, rx) = mpsc::sync_channel:: <TokenBlock>(1);
        let order = cfg.order;
        txs.push(tx);
        workers.push(thread::spawn(move || {
            let mut c = Counts::new(order);
            while let Ok(block) = rx.recv() {
                c.window = 0;
                c.len = 0;
                for b in block.prefix {
                    c.warm(b);
                }
                for b in block.bytes {
                    c.push(b);
                }
            }
            c
        }));
    }
    let mut chunk = Vec::with_capacity(TOKEN_CHUNK);
    let mut prefix = Vec::new();
    let mut tail = Vec::new();
    let mut slot = 0;
    let processing = (|| -> AppResult<()> {
        let mut emit=|b: u8| -> AppResult<()> {
            chunk.push(b);
            if chunk.len() >= TOKEN_CHUNK {
                tail.clear();
                tail.extend_from_slice(&chunk[chunk.len().saturating_sub(4)..]);
                let block = TokenBlock {
                    prefix: std::mem::take(&mut prefix),
                    bytes: std::mem::replace(&mut chunk, Vec::with_capacity(TOKEN_CHUNK))
                };
                txs[slot % jobs].send(block).map_err(|_|"corpus worker stopped")?;
                prefix.extend_from_slice(&tail);
                slot += 1;
            }
            Ok(())
        };
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            normalizer.read(&buf[..n], &mut emit)?;
            notify(normalizer.source_bytes, total);
        }
        normalizer.finish(&mut emit)?;
        drop(emit);
        if !chunk.is_empty() {
            txs[slot % jobs].send(TokenBlock {
                prefix,
                bytes: chunk
            }).map_err(|_|"corpus worker stopped")?;
        }
        Ok(())
    })();
    drop(txs);
    let mut out: Option<Counts> = None;
    for worker in workers {
        let c = worker.join().map_err(|_|"corpus worker panicked")?;
        if let Some(ref mut dst) = out {
            let dst: &mut Counts = dst;
            dst.merge(c)?;
        } else {
            out = Some(c);
        }
    }
    processing?;
    Ok((out.unwrap(), normalizer.hash, normalizer.invalid))
}

fn unpack_gram(mut code: u64, width: usize) -> String {
    let mut b = [0u8; 5];
    for i in(0..width).rev() {
        b[i] = (code&127) as u8;
        code>>=7;
    }
    String::from_utf8(b[..width].to_vec()).expect("ASCII n-gram")
}

fn write_dense(w: &mut impl Write, name: &str, values: &[u64], width: usize) -> AppResult<()> {
    write!(w, "{}:{{", json_quote(name))?;
    let mut comma = false;
    for (i, &n) in values.iter().enumerate() {
        if n == 0 {
            continue;
        }
        if comma {
            w.write_all(b",")?;
        }
        comma = true;
        write!(w, "{}:{}", json_quote(&unpack_gram(i as u64, width)), n)?;
    }
    w.write_all(b"}")?;
    Ok(())
}

fn write_sparse(w: &mut impl Write, name: &str, values: &GramMap, width: usize) -> AppResult<()> {
    let mut keys: Vec<u64> = values.keys().copied().collect();
    keys.sort_unstable();
    write!(w, "{}:{{", json_quote(name))?;
    for (i, k) in keys.iter().enumerate() {
        if i>0 {
            w.write_all(b",")?;
        }
        write!(w, "{}:{}", json_quote(&unpack_gram(*k, width)), values[k])?;
    }
    w.write_all(b"}")?;
    Ok(())
}

fn mtime_ns(meta: &fs::Metadata) -> String {
    meta.modified().ok().and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string()).unwrap_or_default()
}

fn corpus_config_path(raw: &Path) -> PathBuf {
    raw.with_file_name(format!("{}.config.json", raw.file_stem().unwrap_or_default().to_string_lossy()))
}

fn cached_path(raw: &Path) -> PathBuf {
    Path::new(PROCESSED_CORPUS_DIR).join(format!("corpus-{}.json", corpus_name(raw)))
}

fn cache_current_at_least(raw: &Path, cache: &Path, config_hash: u64, min_order: usize) -> AppResult<bool> {
    let f = match File::open(cache) {
        Ok(f) => f,
        Err(e)if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into())
    };
    // Metadata is the bounded first line. Older/imported JSON remains readable,
    // but is not mistaken for a fresh Rust cache of a raw file.
    let mut header = String::new();
    BufReader::new(f).take(8192).read_line(&mut header)?;
    let line = header.trim().trim_end_matches(',');
    if !line.starts_with("{\"source\":") {
        return Ok(false);
    }
    let obj = match parse_json(&format!("{line}}}")) {
        Ok(Json::Object(v)) => v,
        _ => return Ok(false)
    };
    let obj = match obj.get("source") {
        Some(Json::Object(v)) => v,
        _ => return Ok(false)
    };
    let md = fs::metadata(raw)?;
    let string=|key: &str|match obj.get(key) {
        Some(Json::String(s)) => s.as_str(),
        _ => ""
    };
    let num=|key: &str|match obj.get(key) {
        Some(Json::Number(v))=>*v,
        _=>-1.0
    };
    Ok(string("engine") == "layouter-rust" && num("cache_version") == CORPUS_VERSION as f64
        && num("raw_size") == md.len() as f64 && string("raw_mtime_ns") == mtime_ns(&md)
        && string("config_fingerprint") == format!("{config_hash:016x}") && num("max_order") >= min_order as f64)
}

fn cache_current(raw: &Path, cache: &Path, cfg: &CorpusConfig, config_hash: u64) -> AppResult<bool> {
    cache_current_at_least(raw, cache, config_hash, cfg.order)
}

fn rebuild_corpus(raw: &Path, cfg: &CorpusConfig, config_hash: u64, notify: &mut dyn FnMut(u64, u64)) -> AppResult<PathBuf> {
    rebuild_corpus_control(raw, cfg, config_hash, notify, None)
}

struct CorpusReader<'a> {
    file: File,
    cancel: Option<&'a AtomicBool>
}

impl Read for CorpusReader<'_> {
    fn read(&mut self, buf: &mut[u8]) -> io::Result<usize> {
        if self.cancel.map(|c|c.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "corpus build cancelled"));
        }
        self.file.read(buf)
    }
}

fn rebuild_corpus_control(raw: &Path, cfg: &CorpusConfig, config_hash: u64, notify: &mut dyn FnMut(u64, u64), cancel: Option<&AtomicBool>) -> AppResult<PathBuf> {
    corpus_dirs()?;
    let before = fs::metadata(raw)?;
    let f = CorpusReader {
        file: File::open(raw)?,
        cancel
    };
    let (counts, raw_hash, unsupported) = count_reader(f, cfg, before.len(), notify)?;
    let after = fs::metadata(raw)?;
    if before.len() != after.len() || mtime_ns(&before) != mtime_ns(&after) {
        return Err("raw corpus changed during generation; cache was not replaced".into());
    }
    let total: u64 = counts.uni.iter().sum();
    if total == 0 {
        return Err("corpus contains no accepted text".into());
    }
    let path = cached_path(raw);
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), timestamp()));
    let result = (|| -> AppResult<()> {
        let f = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        let mut w = BufWriter::with_capacity(1<<20, f);
        writeln!(w, "{{\"source\":{{\"engine\":\"layouter-rust\",\"cache_version\":{},\"raw_size\":{},\"raw_mtime_ns\":{},\"raw_fingerprint\":\"{:016x}\",\"config_fingerprint\":\"{:016x}\",\"max_order\":{},\"characters\":{},\"unsupported_scalars\":{},\"rows\":{:?}}},", CORPUS_VERSION, before.len(), json_quote(&mtime_ns(&before)), raw_hash, config_hash, cfg.order, total, unsupported, counts.rows())?;
        for (i,(name, v, width)) in [("letters", &counts.uni, 1),("bigrams", &counts.bi, 2),("skipgrams", &counts.skip, 2),("trigrams", &counts.tri, 3)].into_iter().enumerate() {
            if i>0 {
                w.write_all(b",\n")?;
            }
            write_dense(&mut w, name, v, width)?;
        }
        if cfg.order >= 4 {
            w.write_all(b",\n")?;
            write_sparse(&mut w, "fourgrams", &counts.quad, 4)?;
        }
        if cfg.order >= 5 {
            w.write_all(b",\n")?;
            write_sparse(&mut w, "fivegrams", &counts.five, 5)?;
        }
        write!(w, ",\n\"options\":{{\"includeSpacegrams\":true,\"keepCapitalisation\":{}}}\n}}\n", cfg.ascii.iter().chain(cfg.unicode.values()).any(|b|b.is_ascii_uppercase()))?;
        w.flush()?;
        w.get_ref().sync_all()?;
        if cancel.map(|c|c.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err("corpus build cancelled".into());
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    Ok(path)
}

fn ensure_corpus(raw: &Path, force: bool, override_order: Option<usize>) -> AppResult<PathBuf> {
    let (mut cfg, hash) = corpus_config(&corpus_config_path(raw))?;
    if let Some(n) = override_order {
        if !(3..=5).contains(&n) {
            return Err("n-gram order must be 3..5".into());
        }
        cfg.order = cfg.order.max(n);
    }
    let cache = cached_path(raw);
    if !force && cache_current(raw, &cache, &cfg, hash)? {
        return Ok(cache);
    }
    let mut last = Instant::now() - Duration::from_secs(1);
    let name = corpus_name(raw);
    let mut progress=|n,
    total| {
        if last.elapsed()>Duration::from_millis(250) {
            eprint!("\r{name} {:3.0}%", pct(n as f64, total as f64));
            last = Instant::now();
        }
    };
    let result = rebuild_corpus(raw, &cfg, hash, &mut progress);
    eprint!("\r\x1b[2K");
    result
}

fn corpus_paths() -> AppResult<Vec<PathBuf>> {
    corpus_dirs()?;
    let mut by_name = BTreeMap::new();
    for dir in [CORPUS_DIR, PROCESSED_CORPUS_DIR, "."] {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries {
                let p = e?.path();
                if p.is_file() && p.extension().and_then(|s|s.to_str()) == Some("json") &&(dir != "." || p.file_name().unwrap_or_default().to_string_lossy().starts_with("corpus-")) {
                    by_name.insert(corpus_name(&p).to_ascii_lowercase(), p);
                }
            }
        }
    }
    for path in raw_corpus_paths()? {
        by_name.insert(corpus_name(&path).to_ascii_lowercase(), path);
    }
    Ok(by_name.into_values().collect())
}

fn raw_corpus_paths_in(directory: &Path) -> AppResult<Vec<PathBuf>> {
    let mut by_name = BTreeMap::<String, PathBuf>::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.is_file() || is_corpus_config_path(&path) {
            continue;
        }
        let name = corpus_name(&path).to_ascii_lowercase();
        if let Some(existing) = by_name.insert(name.clone(), path.clone()) {
            return Err(format!(
                "raw corpora {} and {} both use the name {name:?}",
                existing.display(),
                path.display()
            )
            .into());
        }
    }

    let mut paths: Vec<_> = by_name.into_values().collect();
    paths.sort();
    Ok(paths)
}

fn raw_corpus_paths() -> AppResult<Vec<PathBuf>> {
    corpus_dirs()?;
    raw_corpus_paths_in(Path::new(RAW_CORPUS_DIR))
}

fn corpus_by_name(name: &str) -> AppResult<PathBuf> {
    let p = Path::new(name);
    if p.is_file() {
        return Ok(p.to_owned());
    }
    let name = corpus_name(p).to_ascii_lowercase();
    corpus_paths()?.into_iter().find(|p|corpus_name(p).eq_ignore_ascii_case(&name)).ok_or_else(|| format!("corpus {name:?} not found").into())
}

fn default_corpus_path() -> AppResult<Option<PathBuf>> {
    if Path::new(DEFAULT_CORPUS).is_file() {
        return Ok(Some(PathBuf::from(DEFAULT_CORPUS)));
    }
    Ok(corpus_paths()?.into_iter().find(|p|corpus_name(p).eq_ignore_ascii_case("reddit")))
}

fn valid_corpus_stem(name: &str) -> bool {
    !name.is_empty()&&!matches!(name, "."|"..")&&!name.contains('/')&&!name.contains('\\') && name.chars().all(|c|!c.is_control())
}

fn import_corpus_text(name: &str, input: &Path, config: Option<&Path>) -> AppResult<PathBuf> {
    if !valid_corpus_stem(name) {
        return Err("corpus name must be a filename stem".into());
    }
    if !fs::metadata(input)?.is_file() {
        return Err("corpus input must be a regular file".into());
    }
    corpus_dirs()?;
    let path = Path::new(RAW_CORPUS_DIR).join(format!("{name}.txt"));
    let logical_name = corpus_name(&path);
    if raw_corpus_paths()?
        .iter()
        .any(|path| corpus_name(path).eq_ignore_ascii_case(&logical_name))
    {
        return Err(format!("raw corpus {logical_name:?} already exists").into());
    }
    let config_bytes = config.map(fs::read).transpose()?;
    let mut dst = OpenOptions::new().create_new(true).write(true).open(&path)?;
    let result = (|| -> AppResult<()> {
        io::copy(&mut File::open(input)?, &mut dst)?;
        dst.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result?;
    if let Some(bytes) = config_bytes {
        let config_path = corpus_config_path(&path);
        let mut f = match OpenOptions::new().create_new(true).write(true).open(&config_path) {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
        };
        let result = (|| -> AppResult<()> {
            f.write_all(&bytes)?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_file(&config_path);
            return Err(error);
        }
    }

    Ok(path)
}

fn add_corpus(name: &str, input: &Path, config: Option<&Path>) -> AppResult<PathBuf> {
    let path = import_corpus_text(name, input, config)?;
    ensure_corpus(&path, true, None)
}

#[cfg(test)]
mod ngram_limit_tests {
    use super::*;

    const SOURCE: &str = r#"{
        "letters":{"a":4,"b":5,"c":3},
        "bigrams":{"ab":3,"bb":1,"bc":3},
        "trigrams":{"abc":2,"abb":2,"bbc":1}
    }"#;

    #[test]
    fn ordinary_limits_keep_ties_deterministic_and_preserve_skip_evidence() {
        let path = Path::new("inline.json");
        let original = Source::from_text(SOURCE, path).unwrap();
        let limits = NgramLimits { trigrams: Some(1), ..NgramLimits::default() };
        let limited = Source::from_text_with_limits(SOURCE, path, limits).unwrap();

        assert_eq!(limited.tables[3], vec![("abb".to_owned(), 2.0)]);
        assert_eq!(limited.tables[..3], original.tables[..3]);
        assert_eq!(limited.masses, original.masses);
        assert_eq!(limited.fingerprint, original.fingerprint);
        assert!(limited.warnings.iter().any(|warning| warning.contains("1/3") && warning.contains("40.00%")));

        let reordered = SOURCE.replace("\"abc\":2,\"abb\":2,\"bbc\":1", "\"bbc\":1,\"abb\":2,\"abc\":2");
        let other = Source::from_text_with_limits(&reordered, path, limits).unwrap();
        assert_eq!(limited.tables[3], other.tables[3]);
    }

    #[test]
    fn ordinary_all_and_unreached_limits_preserve_tables_and_warnings() {
        let original = Source::from_text(SOURCE, Path::new("inline.json")).unwrap();
        for limits in [NgramLimits::default(), NgramLimits { trigrams: Some(20), ..NgramLimits::default() }] {
            let actual = Source::from_text_with_limits(SOURCE, Path::new("inline.json"), limits).unwrap();
            assert_eq!(actual.tables, original.tables);
            assert_eq!(actual.masses, original.masses);
            assert_eq!(actual.warnings, original.warnings);
        }
    }

    #[test]
    fn ordinary_limits_preserve_explicit_skipgrams_and_validate_before_pruning() {
        let explicit = SOURCE.replace("\"trigrams\":", "\"skipgrams\":{\"ac\":100},\"trigrams\":");
        let limits = NgramLimits { trigrams: Some(1), ..NgramLimits::default() };
        let source = Source::from_text_with_limits(&explicit, Path::new("inline.json"), limits).unwrap();
        assert_eq!(source.tables[2], vec![("ac".to_owned(), 100.0)]);

        let malformed = SOURCE.replace("\"bbc\":1", "\"wrong-width\":1");
        assert!(Source::from_text_with_limits(&malformed, Path::new("inline.json"), limits).is_err());
    }
}

#[cfg(test)]
mod corpus_path_tests {
    use super::*;

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "layouter-{name}-{}-{}",
            std::process::id(),
            timestamp()
        ))
    }

    #[test]
    fn raw_detection_accepts_arbitrary_names_and_preserves_frequency_json() {
        let directory = temporary_directory("corpus-detection");
        fs::create_dir_all(&directory).unwrap();
        let prose = directory.join("prose.text");
        let extensionless_prose = directory.join("book");
        let extensionless = directory.join("counts");
        let legacy = directory.join("legacy.txt");
        let prose_json = directory.join("prose-object");
        let json = directory.join("broken.json");
        fs::write(&prose, "ordinary corpus text").unwrap();
        fs::write(&extensionless_prose, "extensionless corpus text").unwrap();
        fs::write(&extensionless, r#"{"letters":{"a":1}}"#).unwrap();
        fs::write(&legacy, r#"{"letters":{"a":1}}"#).unwrap();
        fs::write(&prose_json, r#"{"source":"a quoted passage"}"#).unwrap();
        fs::write(&json, "{ malformed frequency JSON").unwrap();

        assert!(corpus_is_raw(&prose).unwrap());
        assert!(corpus_is_raw(&extensionless_prose).unwrap());
        assert!(!corpus_is_raw(&extensionless).unwrap());
        assert!(corpus_is_raw(&legacy).unwrap());
        assert!(corpus_is_raw(&prose_json).unwrap());
        assert!(!corpus_is_raw(&json).unwrap());
        assert!(corpus_is_raw(Path::new("/tmp/example/corpus/raw/book.json")).unwrap());
        assert!(!corpus_is_raw(Path::new("/tmp/example/corpus/raw/book.config.json")).unwrap());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn raw_discovery_ignores_sidecars_and_rejects_cache_name_collisions() {
        let directory = temporary_directory("raw-discovery");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("novel"), "alpha").unwrap();
        fs::write(directory.join("speech.md"), "beta").unwrap();
        fs::write(directory.join("speech.config.json"), "{}").unwrap();

        let paths = raw_corpus_paths_in(&directory).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.file_name().and_then(|name| name.to_str()) == Some("novel")));
        assert!(paths.iter().any(|path| path.file_name().and_then(|name| name.to_str()) == Some("speech.md")));

        fs::write(directory.join("speech.txt"), "gamma").unwrap();
        let error = raw_corpus_paths_in(&directory).unwrap_err().to_string();
        assert!(error.contains("both use the name"));

        fs::remove_dir_all(directory).unwrap();
    }
}
