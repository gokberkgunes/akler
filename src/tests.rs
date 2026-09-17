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
    let text="the quick brown fox jumps over the lazy dog. my packet layout; she sells seashells. hello world, native words and writing. \n'racket' and packet are two different designs. keyboards need useful tests!\n";
    let mut c = CorpusConfig::default();
    c.jobs = 1;
    c.order = 5;
    let (n, _, _) = count_reader(Cursor::new(text), &c, text.len() as u64, &mut |_, _| {}).unwrap();
    Source::from_text(&counts_text(&n), Path::new("corpus-test.json")).unwrap()
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
    assert!(is_lateral_stretch(key(1, 4), key(2, 0))); // inner index to pinky
    assert!(!is_lateral_stretch(key(1, 3), key(2, 0)));
    assert_eq!(scissor_kind(key(0, 2), key(1, 3)), 0); // concordant one-row is not HSB
    assert!(pair_flags(key(0, 2), key(1, 3), false).bi & bit(ROW1_BI) != 0);
}
#[test]
fn all_trigram_reward_vetoes() {
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
                if t.bits & (bit(ROLL) | bit(ALT)) != 0 {
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
                    assert_eq!(ab.bi & (bit(ROW1_BI) | bit(ROW2_BI)), 0);
                    assert_eq!(bc.bi & (bit(ROW1_BI) | bit(ROW2_BI)), 0);
                    assert_eq!(ac.sk & (bit(ROW1_SK) | bit(ROW2_SK)), 0);
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
    assert!(!is_roll(q(b'e'), q(b'v'), q(b'j')));
    assert!(!is_roll(q(b'd'), q(b'g'), q(b'k')));
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
    for(text,count,range)in[
        ("q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n",30,(0,9)),
        ("~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , . /\nthumbs: space\n",33,(-1,9)),
        ("q w e r t | y u i o p ~\na s d f g | h j k l ; ~\nz x c v b | n m , . / ~\nthumbs: space\n",33,(0,10)),
        ("~ q w e r t | y u i o p [\n~ a s d f g | h j k l ; ]\n~ z x c v b | n m , . / \\\nthumbs: space\n",36,(-1,10)),
    ]{
        let b=board_from_text(text,Path::new("outer.dat")).unwrap();assert_eq!(b.keys.iter().filter(|k|k.main).count(),count);
        let cols=(b.keys.iter().filter(|k|k.main).map(|k|k.col).min().unwrap(),b.keys.iter().filter(|k|k.main).map(|k|k.col).max().unwrap());assert_eq!(cols,range);
        let round=board_from_text(&board_text(&b,&b.symbols),Path::new("round.dat")).unwrap();assert_eq!(b.symbols,round.symbols);assert_eq!(b.keys,round.keys);
    }
    let b=board_from_text("~ q w e r t | y u i o p\n~ a s d f g | h j k l ;\n~ z x c v b | n m , . /\nthumbs: space\n",Path::new("outer.dat")).unwrap();
    assert_eq!(b.keys[0].col, -1);
    assert_eq!(b.keys[0].finger, b.keys[1].finger);
    let wide=board_from_text("~ q w e r t | y u i o p [\n~ a s d f g | h j k l ; ]\n~ z x c v b | n m , . / \\\nthumbs: space\n",Path::new("wide.dat")).unwrap();
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
    let s=Source::from_text("{\"monograms\":{\"a\":2,\"b\":1},\"bigrams\":{\"ab\":1,\"ba\":1},\"trigrams\":{\"aba\":1},\"fourgrams\":{\"abab\":1}}",Path::new("corpus-a.json")).unwrap();
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
    let input = b"a surprisingly long sequence without newline boundaries ".repeat(160000); // crosses 4 MiB jobs and 1 MiB reads
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
        }
    }
    assert_eq!(number(0.0001, 2), "<0.01");
    assert_eq!(delta_text(1.0, 1.0, 2), "—");
    assert_eq!(number(3.1, 2), "3.10");
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
    let header=format!("{{\"source\":{{\"engine\":\"layouter-rust\",\"cache_version\":{},\"raw_size\":{},\"raw_mtime_ns\":{},\"config_fingerprint\":\"{:016x}\",\"max_order\":5}},\n\"letters\":{{}}}}\n",CORPUS_VERSION,meta.len(),json_quote(&mtime_ns(&meta)),7u64);
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
    fs::write(&conf,r#"{"input_graphemes":["a","i","c"," "],"normalization":{"I":"i","İ":"i","ç":"c"},"min_sequence_length":1,"max_order":5,"jobs":2}"#).unwrap();
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
        rows.push(RankRow {
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
