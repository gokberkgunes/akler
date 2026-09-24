use super::*;

use std::io::Cursor;

const PACKET: &str =
    "v d l w x  z p o u ,\ns t h c g  q n a e i\nf k m y j  ; b ' / .\nthumbs: r space\n";

const RACKET: &str =
    "f d l w j  ; b o u ,\ns t h c y  q n a e i\nx k m g v  z p ' / .\nthumbs: r space\n";

fn model() -> Model {
    Model::new(board_from_text(PACKET, Path::new("packet.dat")).unwrap())
}

fn key(r: usize, c: usize) -> Key {
    main_key(r, c)
}

fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() < 1e-7 * (1.0 + a.abs().max(b.abs())),
        "{a} != {b}"
    );
}

fn counts_text(c: &Counts) -> String {
    let mut b = Vec::new();
    b.push(b'{');
    for (i, (name, v, n)) in [
        ("letters", &c.uni, 1),
        ("bigrams", &c.bi, 2),
        ("skipgrams", &c.skip, 2),
        ("trigrams", &c.tri, 3),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            b.push(b',');
        }
        write_dense(&mut b, name, v, n).unwrap();
    }
    b.push(b',');
    write_sparse(&mut b, "fourgrams", &c.quad, 4).unwrap();
    b.push(b',');
    write_sparse(&mut b, "fivegrams", &c.five, 5).unwrap();
    b.push(b'}');
    String::from_utf8(b).unwrap()
}

fn small_source() -> Source {
    let text = "the quick brown fox jumps over the lazy dog. my packet layout; she sells seashells. hello world, native words and writing. \n'racket' and packet are two different designs. keyboards need useful tests!\n";
    let mut c = CorpusConfig::default();
    c.jobs = 1;
    c.order = 5;
    let (n, _, _) = count_reader(Cursor::new(text), &c, text.len() as u64, &mut |_, _| {}).unwrap();
    Source::from_text(&counts_text(&n), Path::new("corpus-test.json")).unwrap()
}

#[test]
fn ordinary_skips_keep_endpoints_across_an_unavailable_middle() {
    let layout =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
    let model = Model::new(board_from_text(layout, Path::new("inline.dat")).unwrap());
    let source = Source::from_text(
        r#"{"letters":{"a":4,"q":3,"w":2,"r":1,"!":1},
        "bigrams":{"a!":1,"!q":1,"aw":2,"wq":1,"wr":1,"qa":1},
        "skipgrams":{"aq":2,"ar":1},
        "trigrams":{"a!q":1,"awq":1,"awr":1}}"#,
        Path::new("inline.json"),
    )
    .unwrap();
    let corpus = model.corpus(&source).unwrap();
    let raw = full_raw(&model.original, &corpus, &model.geometry);

    assert_eq!(corpus.totals[2], 3.0);
    assert_eq!(raw.0[SFS], 2.0);
    assert_eq!(metrics(&raw, &corpus).v[SFS], pct(2.0, 3.0));
    // Keeping a…q as a skip does not invent the adjacent bigram aq.
    assert!(!corpus
        .grams
        .iter()
        .any(|g| g.kind == 1 && gram_name(g, &model.canonical) == "aq"));
}

#[test]
fn ordinary_skip_only_imports_need_no_middle_reconstruction() {
    let source = Source::from_text(
        r#"{"letters":{"a":2,"b":1,"!":1},"bigrams":{"ab":1},"skipgrams":{"ab":7}}"#,
        Path::new("inline.json"),
    )
    .unwrap();
    assert_eq!(model().corpus(&source).unwrap().totals[2], 7.0);
}

#[test]
fn ordinary_trigram_limits_do_not_drop_endpoint_skips() {
    let text = r#"{"letters":{"a":2,"b":2,"!":1},"bigrams":{"ab":1},
        "trigrams":{"a!b":1,"aba":2}}"#;
    for limit in [None, Some(1)] {
        let source = Source::from_text_with_limits(
            text,
            Path::new("inline.json"),
            NgramLimits {
                trigrams: limit,
                ..NgramLimits::default()
            },
        )
        .unwrap();
        let corpus = model().corpus(&source).unwrap();
        assert_eq!(corpus.totals[2], 3.0);
    }
}

fn source_problem() -> Problem {
    let mut s = SearchSettings::default();
    s.sfb_limit = None;
    s.sfs_limit = None;
    s.travel_limit = None;
    s.sftravel_limit = None;
    Problem::new(model(), &[small_source()], 0, Weights::default(), s).unwrap()
}

#[test]
fn paired_formats_preserve_ordinary_geometry_and_exact_metrics() {
    let text = format!(
        "{PACKET}row-offsets: 0 0.25 0.75\n\
         column-offsets: 0 -0.3 -0.4 -0.3 -0.2 -0.2 -0.3 -0.4 -0.3 0\n"
    );
    let board = board_from_text(&text, Path::new("ordinary")).unwrap();
    let layout = board_as_action_layout(&board, &board.symbols).unwrap();
    let jsonc = layout_export::jsonc_text(&layout).unwrap();
    let source = small_source();
    let baseline = Model::new(board.clone());
    let baseline_corpus = baseline.corpus(&source).unwrap();
    let baseline_raw = full_raw(&baseline.original, &baseline_corpus, &baseline.geometry);

    for saved in [layout.text(), jsonc] {
        // The deliberately misleading suffix must have no effect on loading.
        let round = board_from_text(&saved, Path::new("ordinary.unrelated")).unwrap();
        assert_eq!(round.keys, board.keys);
        assert_eq!(round.symbols, board.symbols);

        let model = Model::new(round);
        let corpus = model.corpus(&source).unwrap();
        let raw = full_raw(&model.original, &corpus, &model.geometry);
        for field in 0..N_RAW {
            assert_eq!(raw.0[field].to_bits(), baseline_raw.0[field].to_bits());
        }
        assert_eq!(
            corpus.totals.map(f64::to_bits),
            baseline_corpus.totals.map(f64::to_bits)
        );
    }
}

#[test]
fn paired_formats_preserve_action_mapping_metrics_and_moved_bindings() {
    let text = "q w e r t | y u i o p\n\
                a s d f g | h j k l ;\n\
                z x c v b | n m , . /\n\
                thumbs: none @★\n\
                action ★ = magic\n\
                map ★ \"h\" = \"r\"\n\
                map ★ \"q\" = none\n\
                fallback ★ = none\n\
                swap h nr\n\
                row-offsets: 0 0.25 0.75\n\
                column-offsets: 0 -0.3 -0.4 -0.3 -0.2 -0.2 -0.3 -0.4 -0.3 0\n";
    let mut layout = action_keys::Layout::parse(text, Path::new("magic")).unwrap();
    let corpus_text = "aan aap hrn hrr qqa aa!an";
    let mut config = CorpusConfig::default();
    config.jobs = 1;
    config.order = 5;
    let (counts, _, _) = count_reader(
        Cursor::new(corpus_text),
        &config,
        corpus_text.len() as u64,
        &mut |_, _| {},
    )
    .unwrap();
    let corpus = action_ngrams::NgramCorpus::from_text(
        &counts_text(&counts),
        Path::new("inline-corpus.json"),
    )
    .unwrap();
    let weights = Weights::new([0.0; N_WEIGHTS]);
    let stop = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let magic = layout
        .slots
        .iter()
        .position(|slot| slot.label == "★")
        .unwrap();

    for moved in [false, true] {
        if moved {
            layout.swap(magic, 0);
        }
        let baseline =
            action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
        for saved in [layout.text(), layout_export::jsonc_text(&layout).unwrap()] {
            let round = action_keys::Layout::parse(&saved, Path::new("extensionless")).unwrap();
            assert_eq!(round.slots, layout.slots);
            assert_eq!(round.actions, layout.actions);

            let evaluated =
                action_ui::evaluate_progress(&round, &corpus, &weights, &stop, &progress).unwrap();
            for field in 0..N_RAW {
                assert_eq!(
                    evaluated.raw.0[field].to_bits(),
                    baseline.raw.0[field].to_bits()
                );
            }
            for metric in 0..N_METRICS {
                assert_eq!(
                    evaluated.metrics.v[metric].to_bits(),
                    baseline.metrics.v[metric].to_bits()
                );
            }
            assert_eq!(evaluated.score.to_bits(), baseline.score.to_bits());
            assert_eq!(
                evaluated.corpus.totals.map(f64::to_bits),
                baseline.corpus.totals.map(f64::to_bits)
            );
        }
    }
}

#[test]
fn exclusive_pair_categories() {
    let keys: Vec<_> = (0..30)
        .map(|i| key(i / 10, i % 10))
        .chain([thumb_key(0), thumb_key(1)])
        .collect();
    for (i, &a) in keys.iter().enumerate() {
        for (j, &b) in keys.iter().enumerate() {
            let f = pair_flags(a, b, i == j);
            assert_eq!(f.bi & bit(FSB) != 0, f.bi & (bit(DFSB) | bit(CFSB)) != 0);
            assert_eq!(f.sk & bit(FSS) != 0, f.sk & (bit(DFSS) | bit(CFSS)) != 0);
            assert_eq!(
                (f.bi & bit(DFSB) != 0) as u8 + (f.bi & bit(CFSB) != 0) as u8,
                (f.bi & bit(FSB) != 0) as u8
            );
            assert_eq!(
                f.bi & bit(FSB) != 0 && (f.bi & (bit(DSB) | bit(CSB)) != 0),
                false
            );
            assert_eq!(
                f.sk & bit(FSS) != 0 && (f.sk & (bit(DSS) | bit(CSS)) != 0),
                false
            );
            if same_hand(a, b) && a.finger != b.finger && (a.row - b.row).abs() == 2 {
                assert_eq!(
                    f.bi & (bit(FSB) | bit(DSB) | bit(CSB)),
                    if (a.rank - b.rank).abs() == 1 {
                        bit(FSB)
                    } else if row_motion(a, b) == RowMotion::Discordant {
                        bit(DSB)
                    } else {
                        bit(CSB)
                    }
                );
            }
            if f.bi & bit(SRAF) != 0 {
                assert_eq!(f.bi & BAD_BI, 0);
                assert_ne!(f.bi & bit(RAW_SRAF), 0);
            }
            assert_eq!(f.bi, pair_flags(b, a, i == j).bi);
        }
    }
}

#[test]
fn concordant_scissors_and_nonadjacent_stretches() {
    let ld = pair_flags(key(0, 2), key(2, 1), false);
    assert_ne!(ld.bi & bit(CFSB), 0);
    assert_eq!(ld.bi & bit(CSB), 0);
    let up = pair_flags(key(0, 8), key(2, 7), false);
    assert_ne!(up.bi & bit(DFSB), 0);
    assert_eq!(up.bi & bit(DSB), 0);
    assert!(is_lateral_stretch(key(1, 4), key(2, 0)));
    // inner index to pinky
    assert!(!is_lateral_stretch(key(1, 3), key(2, 0)));
    assert_eq!(scissor_kind(key(0, 2), key(1, 3)), 0);
    // concordant one-row is not HSB
    assert!(pair_flags(key(0, 2), key(1, 3), false).bi & bit(ROW1_BI) != 0);
}

#[test]
fn alternation_vetoes_and_unfiltered_roll_credit() {
    let keys: Vec<_> = (0..30)
        .map(|i| key(i / 10, i % 10))
        .chain([thumb_key(0), thumb_key(1)])
        .collect();
    for &a in &keys {
        for &b in &keys {
            for &c in &keys {
                let t = tri_flags(a, b, c);
                if !(a.main && b.main && c.main) {
                    assert_eq!(t.bits, 0);
                    assert!(!t.main);
                    continue;
                }
                let ab = pair_flags(a, b, a == b);
                let bc = pair_flags(b, c, b == c);
                let ac = pair_flags(a, c, a == c);
                let blockers = triple_blockers(a, b, c, ab, bc, ac);
                if t.bits & bit(ALT) != 0 {
                    assert_eq!(blockers, 0);
                }
                if t.bits & bit(ROLL) != 0 {
                    assert_ne!(t.bits & bit(RAW_ROLL), 0);
                }
                if t.bits & bit(ALT) != 0 {
                    assert_ne!(t.bits & bit(RAW_ALT), 0);
                }
                if t.bits & bit(SIMPLE_ROLL) != 0 {
                    assert_ne!(t.bits & bit(ROLL), 0);
                }
            }
        }
    }
}

#[test]
fn familiar_positive_examples() {
    let q = |b: u8| {
        let index = b"qwertyuiopasdfghjkl;zxcvbnm,./"
            .iter()
            .position(|&x| x == b)
            .unwrap();
        key(index / 10, index % 10)
    };
    assert!(is_roll(q(b'a'), q(b's'), q(b'j')));
    assert!(is_roll(q(b'e'), q(b'v'), q(b'j')));
    assert!(is_roll(q(b'd'), q(b'g'), q(b'k')));
    assert!(!is_sraf(q(b'd'), q(b'g')));
    assert!(is_alternation(q(b'a'), q(b'j'), q(b's')));
    assert!(!is_alternation(q(b'a'), q(b'j'), q(b'q')));
    assert!(!is_alternation(q(b'e'), q(b'j'), q(b'v')));
    assert!(!is_alternation(q(b'a'), q(b'j'), q(b'a')));
}

#[test]
fn plain_layouts_round_trip() {
    for text in [
        PACKET,
        RACKET,
        "  l p d f  ' w o u\nt s n h m  g c a i e\nv z b k q  x y , . j\nr\n",
    ] {
        let b = board_from_text(text, Path::new("layout.dat")).unwrap();
        let round = board_from_text(&board_text(&b, &b.symbols), Path::new("round.dat")).unwrap();
        assert_eq!(b.symbols, round.symbols);
        assert_eq!(b.keys, round.keys);
    }
    let b = board_from_text(
        PACKET.replace("thumbs: r space", "r").as_str(),
        Path::new("legacy.dat"),
    )
    .unwrap();
    assert_eq!(b.symbols[30..], [b'r', b' ']);
    assert_ne!(b.keys[30].finger, b.keys[31].finger);
    assert!(board_from_text(
        PACKET.replace("v d l", "v d v").as_str(),
        Path::new("dup.dat")
    )
    .is_err());
    let comma = PACKET
        .replace("o u ,", "o u r")
        .replace("thumbs: r space", "THUMBS: , space");
    let b = board_from_text(&comma, Path::new("comma.dat")).unwrap();
    assert_eq!(&b.symbols[30..], &[b',', b' ']);
    assert_eq!(b.symbols.len(), 32);
}

#[test]
fn flexible_rows_and_thumb_alignment() {
    for (text, count, range) in[
        ("q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n", 30,(0, 9)),
        ("~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , . /\nthumbs: space\n", 33,(-1, 9)),
        ("q w e r t | y u i o p ~\na s d f g | h j k l ; ~\nz x c v b | n m , . / ~\nthumbs: space\n", 33,(0, 10)),
        ("~ q w e r t | y u i o p [\n~ a s d f g | h j k l ; ]\n~ z x c v b | n m , . / \\\nthumbs: space\n", 36,(-1, 10)),
    ] {
        let b = board_from_text(text, Path::new("outer.dat")).unwrap();
        assert_eq!(b.keys.iter().filter(|k|k.main).count(), count);
        let cols = (b.keys.iter().filter(|k|k.main).map(|k|k.col).min().unwrap(), b.keys.iter().filter(|k|k.main).map(|k|k.col).max().unwrap());
        assert_eq!(cols, range);
        let round = board_from_text(&board_text(&b, &b.symbols), Path::new("round.dat")).unwrap();
        assert_eq!(b.symbols, round.symbols);
        assert_eq!(b.keys, round.keys);
    }
    let b = board_from_text("~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , . /\nthumbs: space\n", Path::new("outer.dat")).unwrap();
    assert_eq!(b.keys[0].col, -1);
    assert_eq!(b.keys[0].finger, b.keys[1].finger);
    let wide = board_from_text("~ q w e r t | y u i o p [\n~ a s d f g | h j k l ; ]\n~ z x c v b | n m , . / \\\nthumbs: space\n", Path::new("wide.dat")).unwrap();
    let model = Model::new(wide);
    let mut canvas = Canvas::new(64, 20);
    keyboard(
        &mut canvas,
        0,
        &model,
        &model.original,
        &model.original,
        None,
        None,
        &[],
    );
    for &(r, a) in &canvas.hits {
        if matches!(a, Action::Key(_)) {
            assert!(r.x + r.w <= canvas.w);
        }
    }
    let base = PACKET
        .replace("v d l w x", "v d l w [")
        .replace("thumbs: r space", "");
    for indent in [12, 13] {
        let b = board_from_text(
            &format!("{base}{}r\n", " ".repeat(indent)),
            Path::new("right.dat"),
        )
        .unwrap();
        let i = b.symbols.iter().position(|&c| c == b'r').unwrap();
        assert_eq!(b.keys[i].hand, 1);
        let s = b.symbols.iter().position(|&c| c == b' ').unwrap();
        assert_eq!(b.keys[s].hand, 0);
        assert!(board_text(&b, &b.symbols)
            .lines()
            .last()
            .unwrap()
            .starts_with("            r"));
    }
    let b = board_from_text(&format!("{base}          r\n"), Path::new("left.dat")).unwrap();
    let i = b.symbols.iter().position(|&c| c == b'r').unwrap();
    assert_eq!(b.keys[i].hand, 0);
    assert!(board_from_text(
        &PACKET.replace("s t h c g", "capslock t h c g"),
        Path::new("caps.dat")
    )
    .is_err());
}

#[test]
fn checked_json_and_legacy_frequency_import() {
    assert!(parse_json("{\"x\":1,\"x\":2}").is_err());
    assert!(parse_json("{\"x\":1e999}").is_err());
    assert!(parse_json("{\"x\":\"\\uD83D\\uDE00\"}").is_ok());
    assert!(parse_json("{\"x\":\"\\uD800\"}").is_err());
    let s = Source::from_text("{\"monograms\":{\"a\":2,\"b\":1},\"bigrams\":{\"ab\":1,\"ba\":1},\"trigrams\":{\"aba\":1},\"fourgrams\":{\"abab\":1}}", Path::new("corpus-a.json")).unwrap();
    assert_eq!(s.tables[2], vec![("aa".into(), 1.0)]);
    assert_eq!(s.masses, [3.0, 2.0, 1.0, 1.0]);
}

fn normalize_chunks(input: &[u8], cfg: CorpusConfig, width: usize) -> Vec<u8> {
    let mut n = Normalizer::new(cfg);
    let mut out = Vec::new();
    let mut put = |b| {
        out.push(b);
        Ok(())
    };
    for chunk in input.chunks(width.max(1)) {
        n.read(chunk, &mut put).unwrap();
    }
    n.finish(&mut put).unwrap();
    out
}

#[test]
fn normalization_and_utf8_refills() {
    let input = " ABC!   xyz \néf G\r\na\tb😀c   ";
    let cfg = CorpusConfig::default();
    let expected = b"abc! xyz\0f g\0a\0b\0c\0";
    for size in 1..30 {
        assert_eq!(
            normalize_chunks(input.as_bytes(), cfg.clone(), size),
            expected
        );
    }
    let mut tr = cfg.clone();
    for (ch, b) in [
        ('ç', b'c'),
        ('ğ', b'g'),
        ('ı', b'i'),
        ('İ', b'i'),
        ('ö', b'o'),
        ('ş', b's'),
        ('ü', b'u'),
    ] {
        tr.unicode.insert(ch, b);
    }
    assert_eq!(
        normalize_chunks("İç ışığı".as_bytes(), tr, 1),
        b"ic isigi\0"
    );
    let mut short = cfg;
    short.min_length = 4;
    assert_eq!(normalize_chunks(b" ab\n abc \n ab c ", short, 1), b"ab c\0");
}

fn slow_counts(tokens: &[u8], order: usize) -> Vec<BTreeMap<String, u64>> {
    let mut tables = vec![BTreeMap::new(); 6];
    let mut sequence = Vec::new();
    for &b in tokens {
        if b == 0 {
            sequence.clear();
            continue;
        }
        sequence.push(b);
        for n in 1..=order {
            if sequence.len() >= n {
                let key = String::from_utf8(sequence[sequence.len() - n..].to_vec()).unwrap();
                *tables[n - 1].entry(key).or_default() += 1;
            }
        }
        if sequence.len() >= 3 {
            let key = String::from_utf8(vec![sequence[sequence.len() - 3], b]).unwrap();
            *tables[5].entry(key).or_default() += 1;
        }
    }
    tables
}

fn verify_counts(c: &Counts, tables: &[BTreeMap<String, u64>]) {
    for (values, index, n) in [
        (&c.uni, 0, 1),
        (&c.bi, 1, 2),
        (&c.tri, 2, 3),
        (&c.skip, 5, 2),
    ] {
        assert_eq!(
            values.iter().filter(|&&v| v > 0).count(),
            tables[index].len()
        );
        for (i, &v) in values.iter().enumerate() {
            if v > 0 {
                assert_eq!(v, *tables[index].get(&unpack_gram(i as u64, n)).unwrap());
            }
        }
    }
    for (map, index, n) in [(&c.quad, 3, 4), (&c.five, 4, 5)] {
        assert_eq!(map.len(), tables[index].len());
        for (&key, &v) in map {
            assert_eq!(v, *tables[index].get(&unpack_gram(key, n)).unwrap());
        }
    }
}

#[test]
fn packed_ngrams_match_reference() {
    let mut rng = Rng::new(456);
    let alphabet = b"ABCDE abcd .,'\n\t";
    let input: Vec<u8> = (0..40000)
        .map(|_| alphabet[rng.index(alphabet.len())])
        .collect();
    let mut cfg = CorpusConfig::default();
    cfg.jobs = 1;
    cfg.order = 5;
    let tokens = normalize_chunks(&input, cfg.clone(), 37);
    let (c, hash, _) = count_reader(
        Cursor::new(&input),
        &cfg,
        input.len() as u64,
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(hash, fingerprint_bytes(&input));
    verify_counts(&c, &slow_counts(&tokens, 5));
}

#[test]
fn block_prefixes_preserve_every_width() {
    let tokens = b"this sentence does not end at a buffer boundary.\0another one!\0";
    let mut whole = Counts::new(5);
    for &b in tokens {
        whole.push(b);
    }
    for chunk_size in 1..13 {
        let mut merged = Counts::new(5);
        for (start, body) in tokens.chunks(chunk_size).enumerate() {
            let pos = start * chunk_size;
            merged.window = 0;
            merged.len = 0;
            for &b in &tokens[pos.saturating_sub(4)..pos] {
                merged.warm(b);
            }
            for &b in body {
                merged.push(b);
            }
        }
        assert_eq!(whole.uni, merged.uni);
        assert_eq!(whole.bi, merged.bi);
        assert_eq!(whole.tri, merged.tri);
        assert_eq!(whole.skip, merged.skip);
        assert_eq!(whole.quad, merged.quad);
        assert_eq!(whole.five, merged.five);
    }
}

#[test]
fn parallel_reader_matches_single() {
    let input = b"a surprisingly long sequence without newline boundaries ".repeat(160000);
    // crosses 4 MiB jobs and 1 MiB reads
    let mut cfg = CorpusConfig::default();
    cfg.jobs = 1;
    cfg.order = 5;
    let (a, h1, _) = count_reader(
        Cursor::new(&input),
        &cfg,
        input.len() as u64,
        &mut |_, _| {},
    )
    .unwrap();
    cfg.jobs = 2;
    let (b, h2, _) = count_reader(
        Cursor::new(&input),
        &cfg,
        input.len() as u64,
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(h1, h2);
    assert_eq!(a.uni, b.uni);
    assert_eq!(a.bi, b.bi);
    assert_eq!(a.skip, b.skip);
    assert_eq!(a.tri, b.tri);
    assert_eq!(a.quad, b.quad);
    assert_eq!(a.five, b.five);
}

#[test]
fn same_key_repeats_and_thumb_denominators() {
    let p = source_problem();
    let mut s = State::new(p.model.original.clone(), &p);
    let exact = s.raws[0].clone();
    let mut t = s.clone();
    trial_into(&mut t, &s, Move::pair(3, 30), &p);
    assert_ne!(exact.0[SRAF_DEN], t.raws[0].0[SRAF_DEN]);
    assert_ne!(exact.0[RHYTHM_DEN], t.raws[0].0[RHYTHM_DEN]);
    checked_rescore(&mut t, &p).unwrap();
    s = t;
    checked_rescore(&mut s, &p).unwrap();
    let thumb = pair_flags(thumb_key(0), thumb_key(1), false);
    assert_eq!(thumb.bi & bit(SFB), 0);
    assert_eq!(pair_flags(key(0, 0), key(0, 0), true).bi & bit(SFB), 0);
}

#[test]
fn incremental_swaps_and_cycles_in_both_objectives() {
    for mode in ["simple", "detailed"] {
        let mut p = source_problem();
        p.settings.mode = mode.into();
        let mut s = State::new(p.model.original.clone(), &p);
        let mut t = s.clone();
        let mut rng = Rng::new(718);
        let free: Vec<_> = (0..s.arr.len())
            .filter(|&i| p.model.canonical[s.arr[i]] != b' ')
            .collect();
        for _ in 0..400 {
            let mv = rng.movement(&free, 0.5);
            trial_into(&mut t, &s, mv, &p);
            checked_rescore(&mut t, &p).unwrap();
            std::mem::swap(&mut s, &mut t);
        }
    }
}

#[test]
fn contributors_reconcile_all_metrics() {
    let p = source_problem();
    let mut after = p.model.original.clone();
    after.swap(0, 17);
    after.swap(3, 30);
    let before = &p.model.original;
    let c = &p.corpora[0];
    let r0 = full_raw(before, c, &p.model.geometry);
    let r1 = full_raw(&after, c, &p.model.geometry);
    for m in 0..N_METRICS {
        for view in [CreditView::Clean, CreditView::Raw, CreditView::Rejected] {
            let rows = contributor_data_mode(m, before, &after, c, &p.model, true, view);
            close(
                rows.iter().map(|v| v.before).sum(),
                pct(contribution_mass(m, &r0, view), denominator(m, &r0, c)),
            );
            close(
                rows.iter().map(|v| v.after).sum(),
                pct(contribution_mass(m, &r1, view), denominator(m, &r1, c)),
            );
        }
    }
    for i in 0..7 {
        let rows = simple_contributors(i, &p.model, before, &after, c);
        close(
            rows.iter().map(|x| x.before).sum(),
            metrics(&r0, c).simple[i],
        );
        close(
            rows.iter().map(|x| x.after).sum(),
            metrics(&r1, c).simple[i],
        );
    }
}

#[test]
fn relative_guardrails_in_all_design_modes() {
    let mut p = source_problem();
    p.settings.sfb_limit = Some(0.05);
    p.baseline[0].v[SFB] = 0.30;
    let mut r = Raw::default();
    r.0[SFB] = p.corpora[0].totals[1] * 0.35 / 100.0;
    for design in ["refine", "random", "evolve"] {
        p.settings.design = design.into();
        assert!(p.feasible(&[r.clone()]));
        let mut bad = r.clone();
        bad.0[SFB] += p.corpora[0].totals[1] * 0.001 / 100.0;
        assert!(!p.feasible(&[bad]));
    }
    p.settings.sfb_limit = None;
    p.settings.travel_limit = Some(0.0);
    p.baseline[0].v[TRAVEL] = 20.0;
    r.0[TRAVEL] = 0.20 * p.corpora[0].totals[0];
    assert!(p.feasible(&[r.clone()]));
    r.0[TRAVEL] += 1.0;
    assert!(!p.feasible(&[r]));
}

#[test]
fn config_round_trips_and_presets() {
    let weights = weights_from_text("sfb = 17\nfsb = 6\ntravel = 0.05\n").unwrap();
    assert_eq!(weights.0[DFSB], 6.0);
    assert_eq!(weights.0[CFSB], 3.0);
    assert_eq!(weights.0[FSB], 0.0);
    let w = weights_from_text(&weights_text(&weights)).unwrap();
    assert_eq!(w.0, weights.0);
    let mut s = SearchSettings::default();
    s.design = "evolve".into();
    s.mix.insert("reddit".into(), 0.7);
    apply_preset(&mut s, "strict");
    let r = search_from_text(&search_settings_text(&s)).unwrap();
    assert_eq!(r.mode, "simple");
    assert_eq!(r.travel_limit, Some(0.0));
    assert_eq!(r.design, "evolve");
    assert_eq!(r.simple, s.simple);
    assert_eq!(r.mix, s.mix);
    assert!(search_from_text("seconds = NaN").is_err());
    assert!(search_from_text("max_sfb_increase = -1").is_err());
}

#[test]
fn crossover_keeps_permutation_and_locks() {
    let m = model();
    let base = m.original.clone();
    let free: Vec<_> = (0..base.len())
        .filter(|&i| !m.board.keys[i].home() && m.canonical[base[i]] != b' ')
        .collect();
    let mut rng = Rng::new(11);
    for _ in 0..200 {
        let mut a = base.clone();
        let mut b = base.clone();
        shuffle(&mut a, &free, &mut rng);
        shuffle(&mut b, &free, &mut rng);
        let c = crossover(&a, &b, &free, &mut rng);
        assert!(valid_arrangement(&c, base.len()));
        for (i, &v) in base.iter().enumerate() {
            if !free.contains(&i) {
                assert_eq!(c[i], v);
            }
        }
    }
}

#[test]
fn diversity_ignores_punctuation_only_moves() {
    let m = model();
    let a = m.original.clone();
    let mut b = a.clone();
    b.swap(9, 29);
    assert_eq!(letter_distance(&m, &a, &b), 0);
    b.swap(0, 1);
    assert_eq!(letter_distance(&m, &a, &b), 2);
    let pool = vec![
        Candidate { arr: a, score: 1.0 },
        Candidate { arr: b, score: 2.0 },
    ];
    assert_eq!(diverse_candidates(&m, &pool, 12, 6, true).len(), 1);
}

#[test]
fn search_respects_locks_and_relative_limits() {
    let mut p = source_problem();
    p.settings.restarts = 2;
    p.settings.passes = 2;
    p.settings.anneal_steps = 60;
    p.settings.seconds = 0.0;
    p.settings.sfb_limit = Some(0.0);
    p.settings.sfs_limit = Some(0.0);
    let locks = default_locks(&p.model.board);
    let result = run_search(&p, &p.model.original, &locks, &Control::new(), &mut |_| {}).unwrap();
    assert!(result.has_best);
    assert!(!result.archive.is_empty());
    for c in result.archive {
        let mut state = State::new(c.arr.clone(), &p);
        assert!(p.feasible(&state.raws));
        checked_rescore(&mut state, &p).unwrap();
        for (i, &locked) in locks.iter().enumerate() {
            if locked {
                assert_eq!(c.arr[i], p.model.original[i]);
            }
        }
    }
}

#[test]
fn rank_gradient_has_no_white_ties() {
    let r = RankRange { lo: 0.2, hi: 0.8 };
    assert_eq!(rank_color(SFB, 0.2, r), gradient(1.0));
    assert_eq!(rank_color(SFB, 0.8, r), gradient(0.0));
    assert_eq!(rank_color(ROLL, 0.2, r), gradient(0.0));
    assert_eq!(rank_color(ROLL, 0.8, r), gradient(1.0));
    assert_eq!(
        rank_color(SFB, 0.2, RankRange { lo: 0.2, hi: 0.2 }),
        gradient(0.5)
    );
    let left = rank_color(SFB, 0.21, r);
    let right = rank_color(SFB, 0.22, r);
    assert!((left as i32 - right as i32).abs() <= 2);
}

#[test]
fn rank_hide_history_and_hitboxes() {
    let mut columns = RankColumns::new();
    let g = RankGrid::new(120, 24, 10, &mut columns);
    assert_eq!(g.layout_at(4, RANK_DATA + 2, 10), Some(2));
    assert_eq!(
        g.metric_at(g.content_x(0) + 2, RANK_DATA, 10),
        Some(RANK_SCORE)
    );
    columns.hide(SFB);
    columns.hide_row(PathBuf::from("packet.dat"));
    columns.hide(SFS);
    columns.undo(6);
    assert!(!columns.hidden[SFS]);
    assert!(columns.hidden[SFB]);
    columns.undo(6);
    assert!(columns.rows.is_empty());
    columns.undo(6);
    assert!(!columns.hidden[SFB]);
    for width in [64, 80, 108, 240] {
        let g = RankGrid::new(width, 24, 10, &mut columns);
        assert!(g.width <= width);
        for (j, &m) in g.metrics.iter().enumerate() {
            assert_eq!(g.metric_at(g.content_x(j), RANK_Y + 1, 4), Some(m));
        }
    }
}

#[test]
fn metric_grid_has_room_for_values_and_deltas() {
    let p = source_problem();
    let raw = full_raw(&p.model.original, &p.corpora[0], &p.model.geometry);
    let m = metrics(&raw, &p.corpora[0]);
    for width in [64, 80, 108] {
        for dp in [2, 4] {
            let mut c = Canvas::new(width, 1);
            let end = grouped_metric_cards(&mut c, 0, &m, &m, &raw, &p.corpora[0], dp);
            assert!(end <= c.h);
            for &(r, a) in &c.hits {
                if let Action::Metric(_) = a {
                    assert!(r.x + r.w <= width);
                    assert!(r.y + r.h <= c.h);
                }
            }
            assert!(!c.cells.iter().any(|cell| cell.ch == '…'));
            let text = c
                .cells
                .chunks(c.w)
                .map(|row| row.iter().map(|cell| cell.ch).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            for label in ["INROLL", "OUTROLL", "IN2", "OUT2", "IN3", "OUT3"] {
                assert!(text.contains(label), "missing {label} at width {width}");
            }
        }
    }
    assert_eq!(number(0.0001, 2), "<0.01");
    assert_eq!(delta_text(1.0, 1.0, 2), "—");
    assert_eq!(number(3.1, 2), "3.10");
}

#[test]
fn numeric_metric_panel_matches_corpus_panel() {
    let problem = source_problem();
    let corpus = &problem.corpora[0];
    let raw = full_raw(&problem.model.original, corpus, &problem.model.geometry);
    let values = metrics(&raw, corpus);

    for width in [64, 80, 108] {
        for decimals in [2, 4] {
            let mut detailed = Canvas::new(width, 1);
            let mut numeric = Canvas::new(width, 1);
            let detailed_end =
                grouped_metric_cards(&mut detailed, 0, &values, &values, &raw, corpus, decimals);
            let numeric_end = grouped_metric_totals(
                &mut numeric,
                0,
                &values,
                &values,
                &raw,
                &corpus.totals,
                decimals,
            );

            assert_eq!(detailed_end, numeric_end);
            assert!(detailed.cells == numeric.cells);
            assert_eq!(detailed.hits.len(), numeric.hits.len());
            for ((a, action_a), (b, action_b)) in detailed.hits.iter().zip(&numeric.hits) {
                assert_eq!((a.x, a.y, a.w, a.h), (b.x, b.y, b.w, b.h));
                assert!(matches!(
                    (action_a, action_b),
                    (Action::Metric(a), Action::Metric(b)) if a == b
                ));
            }
        }
    }
}

#[test]
fn mouse_decoder_distinguishes_middle_press_release() {
    let mut d = Decoder::default();
    d.push(b"\x1b[<1;12;9M\x1b[<1;12;9m");
    assert_eq!(
        d.next(false),
        Some(Event::Mouse {
            x: 11,
            y: 8,
            button: 1,
            release: false,
            motion: false
        })
    );
    assert_eq!(
        d.next(false),
        Some(Event::Mouse {
            x: 11,
            y: 8,
            button: 1,
            release: true,
            motion: false
        })
    );
    d.push(b"\x1b[200~q\x1b[201~");
    assert_eq!(d.next(false), Some(Event::Paste("q".into())));
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "layouter-test-{}-{}",
            std::process::id(),
            timestamp()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cache_metadata_and_higher_orders() {
    let dir = TestDir::new();
    let raw = dir.0.join("raw.txt");
    let cache = dir.0.join("cache.json");
    fs::write(&raw, b"abcd\n").unwrap();
    let meta = fs::metadata(&raw).unwrap();
    let cfg = CorpusConfig::default();
    let header = format!("{{\"source\":{{\"engine\":\"layouter-rust\",\"cache_version\":{},\"raw_size\":{},\"raw_mtime_ns\":{},\"config_fingerprint\":\"{:016x}\",\"max_order\":5}},\n\"letters\":{{}}}}\n", CORPUS_VERSION, meta.len(), json_quote(&mtime_ns(&meta)), 7u64);
    fs::write(&cache, header).unwrap();
    assert!(cache_current(&raw, &cache, &cfg, 7).unwrap());
    assert!(cache_current_at_least(&raw, &cache, 7, 3).unwrap());
    assert!(cache_current_at_least(&raw, &cache, 7, 4).unwrap());
    assert!(!cache_current(&raw, &cache, &cfg, 8).unwrap());
    fs::write(&raw, b"abcdefg\n").unwrap();
    assert!(!cache_current(&raw, &cache, &cfg, 7).unwrap());
}

#[test]
fn config_and_native_free_cancellation() {
    let dir = TestDir::new();
    let conf = dir.0.join("a.config.json");
    fs::write(&conf, r#"{"input_graphemes":["a","i","c"," "],"normalization":{"I":"i","İ":"i","ç":"c"},"min_sequence_length":1,"max_order":5,"jobs":2}"#).unwrap();
    let (cfg, _) = corpus_config(&conf).unwrap();
    assert_eq!(normalize_chunks("Iİ ça".as_bytes(), cfg, 1), b"ii ca\0");
    let raw = dir.0.join("text.txt");
    fs::write(&raw, "content").unwrap();
    let cancel = AtomicBool::new(true);
    let mut reader = CorpusReader {
        file: File::open(raw).unwrap(),
        cancel: Some(&cancel),
    };
    let mut buf = [0u8; 8];
    assert_eq!(
        reader.read(&mut buf).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn generated_layouts_are_new_and_keep_space() {
    let mut p = source_problem();
    p.settings.seconds = 0.0;
    p.settings.restarts = 3;
    p.settings.anneal_steps = 30;
    p.settings.passes = 1;
    p.settings.diversity = 2;
    p.settings.archive = 4;
    let locks: Vec<_> = p
        .model
        .original
        .iter()
        .map(|&i| p.model.canonical[i] == b' ')
        .collect();
    for design in ["random", "evolve"] {
        p.settings.design = design.into();
        let result =
            run_search(&p, &p.model.original, &locks, &Control::new(), &mut |_| {}).unwrap();
        assert!(result.has_best);
        assert!(!result.archive.is_empty());
        for (i, c) in result.archive.iter().enumerate() {
            assert!(letter_distance(&p.model, &c.arr, &p.model.original) >= 2);
            assert!(valid_arrangement(&c.arr, p.model.original.len()));
            for (pos, &locked) in locks.iter().enumerate() {
                if locked {
                    assert_eq!(c.arr[pos], p.model.original[pos]);
                }
            }
            for d in &result.archive[..i] {
                assert!(letter_distance(&p.model, &c.arr, &d.arr) >= 2);
            }
        }
    }
}

#[test]
fn hidden_rows_leave_the_color_range() {
    let p = source_problem();
    let raw = full_raw(&p.model.original, &p.corpora[0], &p.model.geometry);
    let metric = metrics(&raw, &p.corpora[0]);
    let mut rows = Vec::new();
    for (i, v) in [0.20, 0.21, 0.80].iter().enumerate() {
        let mut model = p.model.clone();
        model.board.path = PathBuf::from(format!("{i}.dat"));
        let mut m = metric.clone();
        m.v[SFB] = *v;
        rows.push(RankRow::Plain {
            model,
            corpus: p.corpora[0].clone(),
            raw: raw.clone(),
            metrics: m,
            score: *v,
        });
    }
    let hidden = BTreeSet::new();
    let range = rank_ranges(&rows, &hidden);
    close(range[SFB].hi, 0.8);
    let mut hidden = hidden;
    hidden.insert(PathBuf::from("2.dat"));
    let range = rank_ranges(&rows, &hidden);
    close(range[SFB].lo, 0.2);
    close(range[SFB].hi, 0.21);
    assert_eq!(rank_order(&rows, "", Some(SFB), true, &hidden).len(), 2);
}

#[test]
fn stagger_geometry_round_trip_and_zero_offset_control() {
    let plain = board_from_text(PACKET, Path::new("plain.dat")).unwrap();
    let zero = board_from_text(
        &format!("{PACKET}row-stagger: off\n"),
        Path::new("zero.dat"),
    )
    .unwrap();
    assert_eq!(plain.keys, zero.keys);
    assert_eq!(
        board_text(&plain, &plain.symbols),
        board_text(&zero, &zero.symbols)
    );

    let stagger = board_from_text(
        &format!("{PACKET}row-stagger: standard\n"),
        Path::new("stagger.dat"),
    )
    .unwrap();
    let round = board_from_text(
        &board_text(&stagger, &stagger.symbols),
        Path::new("round.dat"),
    )
    .unwrap();
    assert_eq!(stagger.keys, round.keys);
    let action = crate::action_keys::Layout::parse(
        &board_text(&stagger, &stagger.symbols),
        Path::new("action.dat"),
    )
    .unwrap();
    for (key, slot) in stagger.keys.iter().zip(&action.slots) {
        assert_eq!(key.row_offset, slot.row_offset);
    }
    let geometry = Geometry::new(stagger.keys.clone());
    assert_eq!(geometry.home[0][0].to_bits(), 0.25f64.hypot(1.0).to_bits());
    assert_eq!(
        geometry.sf_distance[10].to_bits(),
        1.0f64.hypot(0.25).to_bits()
    );
    let mut a = main_key(0, 0);
    let b = main_key(1, 1);
    assert!(!is_lateral_stretch(a, b));
    a.row_offset = -1000;
    assert!(is_lateral_stretch(a, b));
    assert_eq!(pair_flags(a, b, false).bi & bit(SFB), 0);
}

#[test]
fn finger_tints_are_grayscale_and_follow_physical_fingers() {
    assert_eq!(finger_tint(0), finger_tint(2));
    assert_ne!(finger_tint(0), finger_tint(1));
    assert_eq!(ansi_color(finger_tint(0)), "\x1b[38;2;155;155;155m");
    assert_eq!(ansi_color(finger_tint(1)), "\x1b[38;2;230;230;230m");
    assert_eq!(stagger_cells(-250, -250, 8), 0);
    assert_eq!(stagger_cells(500, -250, 8), 6);
}

#[test]
fn named_stagger_modes_use_physical_slots_in_both_parsers() {
    let qwerty =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
    let plain = board_from_text(qwerty, Path::new("inline")).unwrap();
    let cases = [
        ("standard", [0, 1, 2, 3, 3, 0, 1, 2, 3, 3, 0, 1, 2, 3, 3]),
        ("standart", [0, 1, 2, 3, 3, 0, 1, 2, 3, 3, 0, 1, 2, 3, 3]),
        ("anglemod", [0, 1, 2, 3, 3, 0, 1, 2, 3, 3, 1, 2, 3, 3, 3]),
        ("nokwts", [0, 1, 2, 2, 3, 0, 1, 2, 3, 3, 0, 1, 3, 3, 3]),
        ("meteorite", [0, 1, 2, 3, 3, 0, 1, 2, 3, 3, 1, 2, 2, 3, 3]),
    ];
    for (name, expected) in cases {
        let source = format!("{qwerty}row-stagger: {name}\n");
        let board = board_from_text(&source, Path::new("inline")).unwrap();
        let mut action = crate::action_keys::Layout::parse(&source, Path::new("inline")).unwrap();
        for (i, key) in board.keys.iter().enumerate() {
            if key.main && key.hand == 0 {
                assert_eq!(
                    key.finger,
                    expected[key.row as usize * 5 + key.col as usize],
                    "{name}, slot {i}"
                );
                assert_eq!(key.rank, key.finger as i8);
            } else {
                assert_eq!(key.finger, plain.keys[i].finger);
            }
            assert_eq!(key.finger, action.slots[i].finger);
            assert_eq!(key.rank, action.slots[i].rank);
            assert_eq!(key.row_offset, action.slots[i].row_offset);
        }
        let saved =
            board_from_text(&board_text(&board, &board.symbols), Path::new("inline")).unwrap();
        assert_eq!(board.row_stagger, saved.row_stagger);
        assert_eq!(board.keys, saved.keys);
        let before: Vec<_> = action
            .slots
            .iter()
            .map(|s| (s.finger, s.rank, s.row_offset))
            .collect();
        action.swap(0, 22);
        assert_eq!(
            before,
            action
                .slots
                .iter()
                .map(|s| (s.finger, s.rank, s.row_offset))
                .collect::<Vec<_>>()
        );
        let saved = crate::action_keys::Layout::parse(&action.text(), Path::new("inline")).unwrap();
        assert_eq!(action.row_stagger, saved.row_stagger);
        assert_eq!(action.slots, saved.slots);

        // QWERTY S/Z is a same-finger pair only with the angle/meteorite maps.
        let sf = pair_flags(board.keys[11], board.keys[20], false);
        assert_eq!(
            sf.bi & bit(SFB) != 0,
            matches!(name, "anglemod" | "meteorite")
        );
        assert_eq!(
            sf.sk & bit(SFS) != 0,
            matches!(name, "anglemod" | "meteorite")
        );
    }
    for directive in [
        "row-stagger:",
        "row-stagger: standard",
        "row-stagger: standart",
    ] {
        assert_eq!(
            crate::action_keys::parse_row_stagger(directive).unwrap(),
            Some(crate::action_keys::RowStagger::Standard)
        );
    }
    for value in ["-0.25 0 0.5", "unknown", "nokwts,"] {
        assert!(crate::action_keys::parse_row_stagger(&format!("row-stagger: {value}")).is_err());
    }
    assert!(board_from_text(
        &format!("{qwerty}row-stagger: standard\nrow-stagger: anglemod\n"),
        Path::new("inline")
    )
    .is_err());
}

#[test]
fn directional_rolls_match_mana_finger_rules_without_thumbs() {
    // Independent translation of the supplied Mana2 basic roll definitions.
    // Mana numbers LP..LT as 0..4 and RT..RP as 5..9.
    fn reference(a: Key, b: Key, c: Key) -> Option<usize> {
        if !(a.main && b.main && c.main) {
            return None;
        }

        let finger = |key: Key| {
            if key.finger < 4 {
                key.finger
            } else {
                key.finger + 2
            }
        };
        let mirror = |finger: usize| if finger >= 5 { 9 - finger } else { finger };
        let (x, y, z) = (finger(a), finger(b), finger(c));
        let (left_a, left_b, left_c) = (x < 5, y < 5, z < 5);
        if left_a != left_c && x != y && y != z {
            let direction = (left_a == left_b && x > y) || (left_a != left_b && z < y);
            return Some(if direction != left_b { IN2 } else { OUT2 });
        }
        if left_a == left_b && left_b == left_c && x != y && y != z && x != z {
            if mirror(x) < mirror(y) && mirror(y) < mirror(z) {
                return Some(IN3);
            }
            if mirror(x) > mirror(y) && mirror(y) > mirror(z) {
                return Some(OUT3);
            }
        }

        None
    }

    let keys: Vec<_> = (0..30)
        .map(|i| main_key(i / 10, i % 10))
        .chain([thumb_key(0), thumb_key(1)])
        .collect();
    let kinds = [IN2, OUT2, IN3, OUT3];
    for &a in &keys {
        for &b in &keys {
            for &c in &keys {
                let expected = reference(a, b, c);
                let flags = tri_flags(a, b, c);
                assert_eq!(roll_kind(a, b, c), expected);
                for kind in kinds {
                    assert_eq!(flags.bits & bit(kind) != 0, expected == Some(kind));
                }
                assert_eq!(flags.bits & bit(ROLL) != 0, expected.is_some());
                assert_eq!(flags.bits & bit(SIMPLE_ROLL) != 0, expected.is_some());
                assert_eq!(
                    flags.bits & bit(INROLL) != 0,
                    matches!(expected, Some(IN2 | IN3))
                );
                assert_eq!(
                    flags.bits & bit(OUTROLL) != 0,
                    matches!(expected, Some(OUT2 | OUT3))
                );
                if expected.is_some() {
                    assert_eq!(flags.bits & (bit(REDIR) | bit(OSF)), 0);
                }
            }
        }
    }
}

#[test]
fn roll_percentages_exclude_thumbs_and_reward_the_total_once() {
    let geometry = Geometry::new(vec![
        main_key(1, 0),
        main_key(1, 1),
        main_key(1, 2),
        main_key(1, 3),
        main_key(1, 6),
        thumb_key(0),
    ]);
    let positions = [0, 1, 2, 3, 4, 5];
    let mut raw = Raw::default();
    for (ids, frequency) in [
        ([0, 1, 4], 2.0),   // IN2
        ([1, 0, 4], 3.0),   // OUT2
        ([0, 1, 2], 5.0),   // IN3
        ([2, 1, 0], 7.0),   // OUT3
        ([0, 4, 1], 11.0),  // Alternation; still in the roll denominator.
        ([5, 0, 1], 100.0), // Thumb; excluded entirely, not spliced out.
    ] {
        let gram = Gram {
            ids,
            len: 3,
            kind: 3,
            f: frequency,
        };
        add_gram(&mut raw, &gram, &positions, &geometry, 1.0);
    }

    assert_eq!(raw.0[RHYTHM_DEN], 28.0);
    assert_eq!(raw.0[ROLL], 17.0);
    let metrics = metrics_totals(&raw, &[0.0, 0.0, 0.0, 128.0]);
    for (metric, frequency) in [
        (IN2, 2.0),
        (OUT2, 3.0),
        (IN3, 5.0),
        (OUT3, 7.0),
        (INROLL, 7.0),
        (OUTROLL, 10.0),
        (ROLL, 17.0),
    ] {
        assert_eq!(metrics.v[metric], pct(frequency, 28.0));
        assert!(is_rhythm(metric));
        assert!(higher_better(metric));
    }
    close(metrics.v[INROLL] + metrics.v[OUTROLL], metrics.v[ROLL]);
    assert_eq!(metrics.simple[6].to_bits(), metrics.v[ROLL].to_bits());

    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[ROLL] = 0.25;
    for metric in [IN2, OUT2, IN3, OUT3, INROLL, OUTROLL] {
        weights.0[metric] = 1000.0; // Display-only even if internal storage is set.
    }
    let score = breakdown(&metrics, &weights);
    assert_eq!(score.net, -metrics.v[ROLL] * 0.25);
    for metric in [IN2, OUT2, IN3, OUT3, INROLL, OUTROLL] {
        assert_eq!(score.contributions[metric], 0.0);
        assert!(raw_positive(metric).is_none());
    }
    assert!(raw_positive(ROLL).is_none());
    assert!(N_RAW <= 64);
}

#[test]
fn roll_columns_and_detail_groups_cover_each_metric_once() {
    for metric in [ROLL, INROLL, OUTROLL, IN2, OUT2, IN3, OUT3] {
        assert_eq!(RANK_ORDER.iter().filter(|&&m| m == metric).count(), 1);
        assert_eq!(
            TABLE_GROUPS
                .iter()
                .flat_map(|(_, group)| group.iter())
                .filter(|&&m| m == metric)
                .count(),
            1
        );
    }
    let columns = parse_rank_columns("ROLL INROLL OUTROLL IN2 OUT2 IN3 OUT3").unwrap();
    for metric in [ROLL, INROLL, OUTROLL, IN2, OUT2, IN3, OUT3] {
        assert!(!columns[metric]);
    }
    assert_eq!(
        parse_rank_columns(&rank_columns_text(&columns)).unwrap(),
        columns
    );
}

#[test]
fn roll_filters_are_independent_and_only_check_consecutive_pairs() {
    let right = main_key(1, 6);
    let scissor = [main_key(0, 0), main_key(2, 1), right];
    let stretch = [main_key(1, 0), main_key(1, 4), right];
    let mut both = scissor;
    both[1].row_offset = 1000;
    let clean = [main_key(1, 0), main_key(1, 1), right];

    for include_scissors in [false, true] {
        for include_stretches in [false, true] {
            let settings = RollSettings {
                include_thumbs: false,
                include_scissors,
                include_stretches,
            };
            for (keys, allowed) in [
                (scissor, include_scissors),
                (stretch, include_stretches),
                (both, include_scissors && include_stretches),
                (clean, true),
            ] {
                let [a, b, c] = keys;
                let flags = tri_flags_with_settings(a, b, c, settings);
                assert_eq!(flags.bits & bit(IN2) != 0, allowed);
                assert_eq!(flags.bits & bit(INROLL) != 0, allowed);
                assert_eq!(flags.bits & bit(ROLL) != 0, allowed);
                assert_eq!(flags.bits & bit(SIMPLE_ROLL) != 0, allowed);
                // Filtering the numerator must never shrink its denominator.
                assert_ne!(flags.bits & bit(ROLL_DEN), 0);
            }
        }
    }

    // Consecutive stretches are below threshold; skip AC exceeds it.
    let a = main_key(1, 0);
    let mut b = main_key(1, 1);
    let mut c = main_key(1, 2);
    b.row_offset = 750;
    c.row_offset = 1500;
    assert_ne!(pair_flags(a, c, false).sk & bit(LSS), 0);
    let settings = RollSettings {
        include_thumbs: false,
        include_scissors: false,
        include_stretches: false,
    };
    assert_ne!(
        tri_flags_with_settings(a, b, c, settings).bits & bit(IN3),
        0
    );

    // A row change that is neither a scissor nor a stretch remains a roll.
    assert_ne!(
        tri_flags_with_settings(main_key(0, 2), main_key(1, 3), right, settings).bits & bit(IN2),
        0
    );
}

#[test]
fn thumb_rolls_have_their_own_denominator_and_inward_finger_rank() {
    let with_thumbs = RollSettings {
        include_thumbs: true,
        ..RollSettings::default()
    };
    let left = thumb_key(0);
    let right = thumb_key(1);
    let pinky = main_key(1, 0);
    let ring = main_key(1, 1);
    let right_index = main_key(1, 6);
    for (keys, kind) in [
        ([pinky, left, right_index], IN2),
        ([left, pinky, right_index], OUT2),
        ([pinky, ring, left], IN3),
        ([left, ring, pinky], OUT3),
        ([right, right_index, ring], OUT2),
        ([main_key(1, 9), right_index, right], IN3),
    ] {
        let [a, b, c] = keys;
        assert_eq!(roll_kind_with_settings(a, b, c, with_thumbs), Some(kind));
        assert_eq!(roll_kind(a, b, c), None);
        let flags = tri_flags_with_settings(a, b, c, with_thumbs);
        assert!(!flags.main); // Other rhythm metrics still exclude this triple.
        assert_ne!(flags.bits & bit(kind), 0);
        assert_eq!(flags.bits & (bit(ALT) | bit(REDIR) | bit(OSF)), 0);
    }
    assert_eq!(
        roll_kind_with_settings(left, pinky, left, with_thumbs),
        None
    );

    let keys = vec![pinky, ring, right_index, left];
    let positions = [0, 1, 2, 3];
    let mut totals = Vec::new();
    for include_thumbs in [false, true] {
        let settings = RollSettings {
            include_thumbs,
            ..RollSettings::default()
        };
        let geometry = Geometry::with_rolls(keys.clone(), settings);
        let mut raw = Raw::default();
        for (ids, f) in [([0, 1, 2], 2.0), ([0, 3, 2], 3.0), ([3, 0, 3], 5.0)] {
            add_gram(
                &mut raw,
                &Gram {
                    ids,
                    len: 3,
                    kind: 3,
                    f,
                },
                &positions,
                &geometry,
                1.0,
            );
        }
        assert_eq!(raw.0[RHYTHM_DEN], 2.0);
        let metrics = metrics_totals(&raw, &[0.0, 0.0, 0.0, 10.0]);
        assert_eq!(metrics.simple[6], metrics.v[ROLL]);
        totals.push((raw.0[ROLL_DEN], metrics.v[IN2]));
    }
    assert_eq!(totals, [(2.0, 100.0), (10.0, 50.0)]);
}

#[test]
fn ordinary_search_refreshes_roll_geometry_after_policy_changes() {
    let source = small_source();
    let base = model();
    let old = full_raw(
        &base.original,
        &base.corpus(&source).unwrap(),
        &base.geometry,
    );
    for include_thumbs in [false, true] {
        for include_scissors in [false, true] {
            for include_stretches in [false, true] {
                let rolls = RollSettings {
                    include_thumbs,
                    include_scissors,
                    include_stretches,
                };
                let weights = Weights::default().with_rolls(rolls);
                let problem = Problem::new(
                    base.clone(),
                    &[source.clone()],
                    0,
                    weights,
                    SearchSettings::default(),
                )
                .unwrap();
                assert_eq!(problem.model.geometry.rolls, rolls);
                let raw = full_raw(
                    &problem.model.original,
                    &problem.corpora[0],
                    &problem.model.geometry,
                );
                for metric in 0..N_METRICS {
                    if !roll_metric(metric) {
                        assert_eq!(raw.0[metric].to_bits(), old.0[metric].to_bits());
                    }
                }
                assert_eq!(raw.0[RHYTHM_DEN], old.0[RHYTHM_DEN]);
                let mut state = State::new(problem.model.original.clone(), &problem);
                let mut next = state.clone();
                trial_into(&mut next, &state, Move::pair(0, 30), &problem);
                checked_rescore(&mut next, &problem).unwrap();
                state = next;
                checked_rescore(&mut state, &problem).unwrap();
            }
        }
    }
}
