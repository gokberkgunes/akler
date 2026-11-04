use super::*;

use std::io::Cursor;

const PACKET: &str = r#"{"layout": {"fingers": ["v d l w x z p o u ,", "s t h c g q n a e i", "f k m y j ; b ' / ."], "thumbs": ["r", "space"]}}"#;

fn model() -> Model {
    Model::new(board_from_text(PACKET, Path::new("packet.jsonc")).unwrap())
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
        crate::action_keys::test_layouts::QWERTY_JSONC;
    let model = Model::new(board_from_text(layout, Path::new("inline.jsonc")).unwrap());
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
        .any(|g| g.kind() == 1 && gram_name(g, &model.canonical) == "aq"));

    // An explicitly supplied skip population also needs no middle press or
    // trigram table to retain its frequency.
    let source = Source::from_text(
        r#"{"letters":{"a":2,"b":1,"!":1},"bigrams":{"ab":1},"skipgrams":{"ab":7}}"#,
        Path::new("inline.json"),
    )
    .unwrap();
    assert_eq!(model.corpus(&source).unwrap().totals[2], 7.0);
}

#[test]
fn same_key_bigrams_and_skipgrams_are_separate_metrics() {
    let layout =
        crate::action_keys::test_layouts::QWERTY_JSONC;
    let model = Model::new(board_from_text(layout, Path::new("inline.jsonc")).unwrap());
    let source = Source::from_text(
        r#"{"letters":{"a":4,"p":2},"bigrams":{"aa":2,"ap":1},"skipgrams":{"aa":3,"ap":1}}"#,
        Path::new("inline.json"),
    )
    .unwrap();
    let corpus = model.corpus(&source).unwrap();
    let raw = full_raw(&model.original, &corpus, &model.geometry);
    let values = metrics(&raw, &corpus);

    assert_eq!(raw.0[SKB], 2.0);
    assert_eq!(raw.0[SKS], 3.0);
    assert_eq!(raw.0[SFB], 0.0);
    assert_eq!(raw.0[SFS], 0.0);
    assert_eq!(values.v[SKB], pct(2.0, 3.0));
    assert_eq!(values.v[SKS], pct(3.0, 4.0));
    assert_eq!(Weights::default().0[SKB], 0.0);
    assert_eq!(Weights::default().0[SKS], 0.0);

    let same = pair_flags(key(0, 0), key(0, 0), true);
    assert_ne!(same.bi & bit(SKB), 0);
    assert_ne!(same.sk & bit(SKS), 0);
    assert_eq!(same.bi & bit(SFB), 0);
    assert_eq!(same.sk & bit(SFS), 0);

    let different = pair_flags(key(0, 0), key(1, 0), false);
    assert_ne!(different.bi & bit(SFB), 0);
    assert_ne!(different.sk & bit(SFS), 0);
    assert_eq!(different.bi & bit(SKB), 0);
    assert_eq!(different.sk & bit(SKS), 0);
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
fn action_fspeed_populations_match_ordinary_source_masses() {
    // Magic/action layouts normalize Fspeed by the same whole-corpus bigram
    // and skipgram masses as ordinary layouts (and akl.gg), unmapped
    // characters included, with spacegrams on or off and unaffected by caps.
    let text = "the quick brown fox! jumps over 42 lazy dogs. ~tests~ \n";
    let mut c = CorpusConfig::default();
    c.jobs = 1;
    c.order = 5;
    let (n, _, _) = count_reader(Cursor::new(text), &c, text.len() as u64, &mut |_, _| {}).unwrap();
    let json = counts_text(&n);
    let path = Path::new("corpus-test.json");
    for include_spacegrams in [true, false] {
        for trigrams in [None, Some(3)] {
            let limits = NgramLimits {
                include_spacegrams,
                trigrams,
                ..NgramLimits::default()
            };
            let source = Source::from_text_with_limits(&json, path, limits).unwrap();
            let action =
                crate::action_ngrams::NgramCorpus::from_text_with_limits(&json, path, limits)
                    .unwrap();
            assert!(source.masses[1] > 0.0 && source.masses[2] > 0.0);
            close(action.speed_populations[0], source.masses[1]);
            close(action.speed_populations[1], source.masses[2]);
        }
    }
}

#[test]
fn own_alt_rules_name_the_keys_their_letters_are_on() {
    // After w the adaptive swap types c on x's key. The c and g rules stay on
    // c's own key, exactly as the optimizers pin them, so evaluating the
    // layout as saved and pinned gives the same result.
    let text = "gwc wcg gwcg cg gc cwc wcwg ";
    let mut corpus_config = CorpusConfig::default();
    corpus_config.jobs = 1;
    corpus_config.order = 5;
    let (counts, _, _) = count_reader(
        Cursor::new(text),
        &corpus_config,
        text.len() as u64,
        &mut |_, _| {},
    )
    .unwrap();
    let action =
        action_ngrams::NgramCorpus::from_text(&counts_text(&counts), Path::new("inline.json"))
            .unwrap();
    let mut layout =
        crate::action_keys::Layout::parse(
            &crate::action_keys::test_layouts::jsonc(
                ["v d l w x z p o u ,", "s t h c g q n a e i", "f k m y j ; b ' / ."],
                ["r", "space"],
                &crate::action_keys::test_layouts::magic(&[("wc", "wx"), ("wx", "wc")]),
            ),
            Path::new("swap.jsonc"),
        )
            .unwrap();
    layout.alt_fingers = crate::alt_fingers::parse_json(
        &parse_json(
            r#"[{"ngram":["c","g"],"fingers":["LM","LI"]},
            {"ngram":["g",null,"c"],"fingers":["LI",null,"LM"]}]"#,
        )
        .unwrap(),
    )
    .unwrap();
    let mut pinned = layout.clone();
    crate::alt_fingers::pin(&mut pinned);
    let mut weights = Weights::default();
    weights.3 = true;
    let stop = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let saved = action_ui::evaluate_progress(&layout, &action, &weights, &stop, &progress).unwrap();
    let at_keys =
        action_ui::evaluate_progress(&pinned, &action, &weights, &stop, &progress).unwrap();
    assert_eq!(
        saved.raw.0.map(f64::to_bits),
        at_keys.raw.0.map(f64::to_bits)
    );
    assert_eq!(saved.score.to_bits(), at_keys.score.to_bits());
    // The rules still apply on c's own key.
    weights.3 = false;
    let off = action_ui::evaluate_progress(&layout, &action, &weights, &stop, &progress).unwrap();
    assert_ne!(off.score.to_bits(), saved.score.to_bits());
}

#[test]
fn jump_categories_and_adjacent_discordant_subsets() {
    let keys: Vec<_> = (0..30)
        .map(|i| key(i / 10, i % 10))
        .chain([thumb_key(0), thumb_key(1)])
        .collect();
    for (i, &a) in keys.iter().enumerate() {
        for (j, &b) in keys.iter().enumerate() {
            let f = pair_flags(a, b, i == j);
            let adjacent = (a.rank - b.rank).abs() == 1;
            let row_gap = (a.row - b.row).abs();
            let motion = row_motion(a, b);
            for (metric, expected) in [
                (
                    DAFJB,
                    adjacent && row_gap == 2 && motion == RowMotion::Discordant,
                ),
                (
                    CAFJB,
                    adjacent && row_gap == 2 && motion == RowMotion::Concordant,
                ),
                (
                    DAHJB,
                    adjacent && row_gap == 1 && motion == RowMotion::Discordant,
                ),
                (
                    CAHJB,
                    adjacent && row_gap == 1 && motion == RowMotion::Concordant,
                ),
                (
                    DNHJB,
                    !adjacent && row_gap == 1 && motion == RowMotion::Discordant,
                ),
                (
                    CNHJB,
                    !adjacent && row_gap == 1 && motion == RowMotion::Concordant,
                ),
                (
                    FSB,
                    adjacent && row_gap == 2 && motion == RowMotion::Discordant,
                ),
                (
                    HSB,
                    adjacent && row_gap == 1 && motion == RowMotion::Discordant,
                ),
                (
                    DNFJB,
                    !adjacent && row_gap == 2 && motion == RowMotion::Discordant,
                ),
                (
                    CNFJB,
                    !adjacent && row_gap == 2 && motion == RowMotion::Concordant,
                ),
            ] {
                assert_eq!(f.bi & bit(metric) != 0, expected, "{metric}: {i} -> {j}");
            }
            for (bigram, skipgram) in [
                (DAFJB, DAFJS),
                (CAFJB, CAFJS),
                (DAHJB, DAHJS),
                (CAHJB, CAHJS),
                (FSB, FSS),
                (HSB, HSS),
                (DNFJB, DNFJS),
                (CNFJB, CNFJS),
                (DNHJB, DNHJS),
                (CNHJB, CNHJS),
            ] {
                assert_eq!(f.bi & bit(bigram) != 0, f.sk & bit(skipgram) != 0);
            }
            if f.bi & bit(SRAF) != 0 {
                assert_eq!(f.bi & BAD_BI, 0);
                assert_ne!(f.bi & bit(RAW_SRAF), 0);
            }
            let reverse = pair_flags(b, a, i == j).bi;
            let directions = bit(INSRAF) | bit(OUTSRAF);
            assert_eq!(f.bi & !directions, reverse & !directions);
            assert_eq!(f.bi & bit(INSRAF) != 0, reverse & bit(OUTSRAF) != 0);
            assert_eq!(f.bi & bit(OUTSRAF) != 0, reverse & bit(INSRAF) != 0);
        }
    }
}

#[test]
fn adjacent_and_nonadjacent_full_jump_weights_score_separately() {
    let mut raw = Raw::default();
    for (adjacent, nonadjacent, adjacent_count, nonadjacent_count) in [
        (DAFJB, DNFJB, 2.0, 1.0),
        (CAFJB, CNFJB, 2.0, 2.0),
        (DAFJS, DNFJS, 2.0, 3.0),
        (CAFJS, CNFJS, 2.0, 4.0),
    ] {
        raw.0[adjacent] = adjacent_count;
        raw.0[nonadjacent] = nonadjacent_count;
    }
    let metrics = metrics_totals(&raw, &[100.0, 10.0, 10.0, 0.0]);
    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    for (broad, subset, broad_weight, subset_weight) in [
        (DAFJB, DNFJB, 4.0, 2.0),
        (CAFJB, CNFJB, 3.0, 1.0),
        (DAFJS, DNFJS, 5.0, 2.0),
        (CAFJS, CNFJS, 4.0, 1.0),
    ] {
        weights.0[broad] = broad_weight;
        weights.0[subset] = subset_weight;
    }
    let score = breakdown(&metrics, &weights);
    for (metric, expected) in [
        (DAFJB, 80.0),
        (DNFJB, 20.0),
        (CAFJB, 60.0),
        (CNFJB, 20.0),
        (DAFJS, 100.0),
        (DNFJS, 60.0),
        (CAFJS, 80.0),
        (CNFJS, 40.0),
    ] {
        close(score.contributions[metric], expected);
    }

    let layout = action_keys::Layout::parse(PACKET, Path::new("inline.jsonc")).unwrap();
    let keys = action_ui::physical_keys(&layout);
    let adjacent = (0..keys.len())
        .flat_map(|a| (0..keys.len()).map(move |b| (a, b)))
        .find(|&(a, b)| pair_flags(keys[a], keys[b], a == b).bi & bit(DAFJB) != 0)
        .unwrap();
    let nonadjacent = (0..keys.len())
        .flat_map(|a| (0..keys.len()).map(move |b| (a, b)))
        .find(|&(a, b)| pair_flags(keys[a], keys[b], a == b).bi & bit(DNFJB) != 0)
        .unwrap();
    let effort = action_ui::LocalEffort::new(&layout, &weights);
    let neutral = action_ui::LocalEffort::new(&layout, &Weights::new([0.0; N_WEIGHTS]));
    for ((a, b), bigram, skipgram) in [(adjacent, 4.0, 5.0), (nonadjacent, 2.0, 2.0)] {
        close(
            effort.get(None, Some(a), b) - neutral.get(None, Some(a), b),
            bigram,
        );
        close(
            effort.get(Some(a), None, b) - neutral.get(Some(a), None, b),
            skipgram,
        );
    }
}

#[test]
fn concordant_jumps_and_nonadjacent_subsets() {
    let ld = pair_flags(key(0, 2), key(2, 1), false);
    assert_ne!(ld.bi & bit(CAFJB), 0);
    assert_eq!(ld.bi & bit(FSB), 0);
    assert_eq!(ld.bi & bit(CNFJB), 0);
    let up = pair_flags(key(0, 8), key(2, 7), false);
    assert_ne!(up.bi & bit(DAFJB), 0);
    assert_ne!(up.bi & bit(FSB), 0);
    assert_eq!(up.bi & bit(DNFJB), 0);
    assert!(is_lateral_stretch(key(1, 4), key(2, 0)));
    // inner index to pinky
    assert!(!is_lateral_stretch(key(1, 3), key(2, 0)));
    assert_eq!(scissor_kind(key(0, 2), key(1, 3)), 0);
    // A concordant one-row jump is CAHJB but not the discordant HSB subset.
    assert_ne!(pair_flags(key(0, 2), key(1, 3), false).bi & bit(CAHJB), 0);
    assert_eq!(pair_flags(key(0, 2), key(1, 3), false).bi & bit(HSB), 0);
}

#[test]
fn alternation_hand_order_and_unfiltered_roll_credit() {
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
                assert_eq!(t.bits & bit(ALT) != 0, a.hand == c.hand && a.hand != b.hand);
                if t.bits & bit(ROLL) != 0 {
                    assert_ne!(t.bits & (bit(INROLL) | bit(OUTROLL)), 0);
                }
                if t.bits & bit(ALT) != 0 {
                    assert_ne!(t.bits & bit(RAW_ALT), 0);
                }
            }
        }
    }
}

#[test]
fn clean_sraf_direction_and_legacy_rewards_are_independent() {
    for (pinky, ring) in [(key(0, 0), key(0, 1)), (key(0, 9), key(0, 8))] {
        let inward = pair_flags(pinky, ring, false).bi;
        let outward = pair_flags(ring, pinky, false).bi;
        assert_ne!(inward & bit(SRAF), 0);
        assert_ne!(inward & bit(INSRAF), 0);
        assert_eq!(inward & bit(OUTSRAF), 0);
        assert_ne!(outward & bit(SRAF), 0);
        assert_ne!(outward & bit(OUTSRAF), 0);
        assert_eq!(outward & bit(INSRAF), 0);
    }
    let mut stretched = key(0, 0);
    stretched.row_offset = -2000;
    let blocked = pair_flags(stretched, key(0, 1), false).bi;
    assert_ne!(blocked & bit(RAW_SRAF), 0);
    assert_eq!(blocked & (bit(SRAF) | bit(INSRAF) | bit(OUTSRAF)), 0);

    let legacy = weights_from_text("sraf_reward = 0.3\nroll_reward = 0.1\n").unwrap();
    assert_eq!((legacy.0[INSRAF], legacy.0[OUTSRAF]), (0.3, 0.3));
    assert_eq!((legacy.0[INROLL], legacy.0[OUTROLL]), (0.1, 0.1));
    let split = weights_from_text(
        "outroll = 0.4\nroll_reward = 0.1\noutsraf_reward = 0.7\nsraf_reward = 0.3\n",
    )
    .unwrap();
    assert_eq!((split.0[INROLL], split.0[OUTROLL]), (0.1, 0.4));
    assert_eq!((split.0[INSRAF], split.0[OUTSRAF]), (0.3, 0.7));
    let serialized = weights_text(&split);
    assert!(!serialized.contains("\nroll_reward ="));
    assert!(!serialized.contains("\nsraf_reward ="));
    assert_eq!(weights_from_text(&serialized).unwrap().0, split.0);

    let board = board_from_text(
        crate::action_keys::test_layouts::QWERTY_JSONC,
        Path::new("sraf.jsonc"),
    )
    .unwrap();
    let model = Model::new(board);
    let source = Source::from_text(
        r#"{"letters":{"q":3,"w":3},"bigrams":{"qw":2,"wq":1}}"#,
        Path::new("sraf.json"),
    )
    .unwrap();
    let corpus = model.corpus(&source).unwrap();
    let raw = full_raw(&model.original, &corpus, &model.geometry);
    assert_eq!(
        (raw.0[SRAF], raw.0[INSRAF], raw.0[OUTSRAF]),
        (3.0, 2.0, 1.0)
    );
    let metrics = metrics(&raw, &corpus);
    close(metrics.v[INSRAF] + metrics.v[OUTSRAF], metrics.v[SRAF]);
    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[INSRAF] = 0.5;
    weights.0[OUTSRAF] = 1.0;
    weights.0[SRAF] = 100.0; // Combined display total never scores twice.
    let score = breakdown(&metrics, &weights);
    assert_eq!(score.contributions[SRAF], 0.0);
    close(score.net, -metrics.v[INSRAF] * 0.5 - metrics.v[OUTSRAF]);
}

#[test]
fn action_sraf_uses_current_root_slot_and_directional_objective() {
    // A magic key without rules, typing q.
    let mut layout = action_keys::Layout::parse(
        &crate::action_keys::test_layouts::jsonc(
            ["@ w e r t y u i o p", "a s d f g h j k l ;", "z x c v b n m , . /"],
            ["space", ""],
            r#""magic": {"key": "@", "fallback": "q"}"#,
        ),
        Path::new("action-sraf.jsonc"),
    )
    .unwrap();
    let corpus = action_ngrams::NgramCorpus::from_text(
        r#"{"letters":{"q":3,"w":3},"bigrams":{"qw":2,"wq":1},"trigrams":{"qwq":1}}"#,
        Path::new("action-sraf.json"),
    )
    .unwrap();
    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[INSRAF] = 0.5;
    weights.0[OUTSRAF] = 1.0;
    let stop = AtomicBool::new(false);
    let progress = AtomicU64::new(0);
    let first = action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
    assert_eq!((first.raw.0[INSRAF], first.raw.0[OUTSRAF]), (2.0, 1.0));
    close(
        first.metrics.v[INSRAF] + first.metrics.v[OUTSRAF],
        first.metrics.v[SRAF],
    );
    close(first.score, breakdown(&first.metrics, &weights).net);

    layout.swap(0, 9);
    let moved = action_ui::evaluate_progress(&layout, &corpus, &weights, &stop, &progress).unwrap();
    assert_eq!((moved.raw.0[INSRAF], moved.raw.0[OUTSRAF]), (0.0, 0.0));
    assert_ne!(first.score, moved.score);
}

#[test]
fn checked_json_and_legacy_frequency_import() {
    for text in [
        r#""""#,
        r#""a q""#,
        r#""é😀""#,
        r#""a\"q""#,
        r#""\u0061\uD83D\uDE00""#,
        r#""\uD800""#,
        r#""\x""#,
        "\"a\nq\"",
        "\"unfinished",
        "not-a-string",
    ] {
        let mut fast = JsonParser { text, p: 0 };
        let mut decoded = JsonParser { text, p: 0 };
        assert_eq!(
            fast.string_cow()
                .map(|value| value.into_owned())
                .map_err(|error| error.to_string()),
            decoded.string_decoded().map_err(|error| error.to_string()),
            "JSON string {text:?}",
        );
        assert_eq!(fast.p, decoded.p);
    }
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
    // A repeated returning key remains an alternation and a same-key skip.
    let a = key(1, 0);
    let p_key = key(0, 9);
    let flags = tri_flags(a, p_key, a);
    assert_ne!(flags.bits & bit(RAW_ALT), 0);
    assert_ne!(flags.bits & bit(ALT), 0);
    assert_ne!(pair_flags(a, a, true).sk & bit(SKS), 0);
    assert!(raw_positive(ALT).is_none());
}

#[test]
fn incremental_swaps_and_cycles_in_both_objectives() {
    // Exercise both native halves against the original u128 bit traversal.
    for mask in [
        0,
        bit(0),
        bit(63),
        bit(64),
        bit(N_RAW - 1),
        (1u128 << N_RAW) - 1,
    ] {
        let mut actual = Raw::default();
        let mut expected = Raw::default();
        for (i, value) in actual.0.iter_mut().enumerate() {
            *value = (i as f64 + 1.0) / 7.0;
        }
        expected.0 = actual.0;
        for delta in [0.1, -0.1, 1e-20] {
            add_bits(&mut actual, mask, delta);
            let mut bits = mask;
            while bits != 0 {
                expected.0[bits.trailing_zeros() as usize] += delta;
                bits &= bits - 1;
            }
            for (left, right) in actual.0.iter().zip(expected.0) {
                assert_eq!(left.to_bits(), right.to_bits());
            }
        }
    }
    // Uncapped Mana2 trials maintain only Mana2 counters; a cap keeps both.
    for (mode, cap) in [("aklgg", None), ("aklgg", Some(100.0)), ("normal", None)] {
        let mut p = source_problem();
        p.settings.mode = mode.into();
        p.settings.sfb_limit = cap;
        assert_eq!(p.normal_counters_unused(), mode == "aklgg" && cap.is_none());
        let mut s = State::new(p.model.original.clone(), &p);
        let mut t = s.clone();
        let mut rng = Rng::new(718);
        let free: Vec<_> = (0..s.arr.len())
            .filter(|&i| p.model.canonical[s.arr[i]] != b' ')
            .collect();
        for _ in 0..400 {
            let mv = rng.movement(&free, 0.5);
            trial_into(&mut t, &s, mv, &p);
            if mv.len == 2 {
                // Retain the original generic affected-list merge as a reference.
                let mut reference = s.clone();
                reference.arr.swap(mv.slots[0], mv.slots[1]);
                for &slot in &mv.slots[..mv.len] {
                    reference.pos[reference.arr[slot]] = slot;
                }
                let x = s.arr[mv.slots[0]];
                let y = s.arr[mv.slots[1]];
                for (i, corpus) in p.corpora.iter().enumerate() {
                    let pair = p.caches[i].pair(x, y);
                    let third: &[u32] = &[];
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
                        add_gram(
                            &mut reference.raws[i],
                            &corpus.grams[index as usize],
                            &s.pos,
                            &p.model.geometry,
                            -1.0,
                        );
                        add_gram(
                            &mut reference.raws[i],
                            &corpus.grams[index as usize],
                            &reference.pos,
                            &p.model.geometry,
                            1.0,
                        );
                    }
                }
                reference.score = p.score(&reference.raws);
                assert_eq!(t.arr, reference.arr);
                assert_eq!(t.pos, reference.pos);
                assert_eq!(t.score.to_bits(), reference.score.to_bits());
                for (actual, expected) in t.raws.iter().zip(&reference.raws) {
                    if !p.normal_counters_unused() {
                        for (left, right) in actual.0.iter().zip(expected.0) {
                            assert_eq!(left.to_bits(), right.to_bits());
                        }
                    }
                    match (&actual.1, &expected.1) {
                        (Some(left), Some(right)) => {
                            for (a, b) in left.0.iter().zip(right.0) {
                                assert_eq!(a.to_bits(), b.to_bits());
                            }
                            for (a, b) in left
                                .1
                                .bigrams
                                .iter()
                                .chain(&left.1.skipgrams)
                                .zip(right.1.bigrams.iter().chain(&right.1.skipgrams))
                            {
                                assert_eq!(a.to_bits(), b.to_bits());
                            }
                        }
                        (None, None) => {}
                        _ => panic!("Mana2 raw presence differs"),
                    }
                }
            }
            checked_rescore(&mut t, &p).unwrap();
            std::mem::swap(&mut s, &mut t);
        }
    }
}

#[test]
fn contributors_reconcile_all_metrics() {
    // Swapping unrelated letters between a main key and a thumb changes the
    // SRAF denominator, while the physical t-h pair and its mass stay fixed.
    let fixture = model();
    let source = Source::from_text(
        r#"{"letters":{"t":10,"h":10,"s":10,"r":10},"bigrams":{"th":2,"st":1,"rt":3}}"#,
        Path::new("inline.json"),
    )
    .unwrap();
    let corpus = fixture.corpus(&source).unwrap();
    let mut moved = fixture.original.clone();
    moved.swap(10, 30);
    let before_raw = full_raw(&fixture.original, &corpus, &fixture.geometry);
    let after_raw = full_raw(&moved, &corpus, &fixture.geometry);
    let d0 = denominator(SRAF, &before_raw, &corpus);
    let d1 = denominator(SRAF, &after_raw, &corpus);
    let rows = contributor_data_mode(
        SRAF,
        &fixture.original,
        &moved,
        &corpus,
        &fixture,
        false,
        CreditView::Clean,
    );
    let th = rows.iter().find(|row| row.gram == "th").unwrap();
    assert!(th.after < th.before);
    let (before, after) = contributor_comparison(th.before, th.after, d0, d1);
    close(before, after);
    assert_eq!(change_color(SRAF, before, after), FG);
    assert_eq!(delta_text(before, after, DETAIL_DECIMALS), "—");
    for (gram, color) in [("st", RED), ("rt", GREEN)] {
        let row = rows.iter().find(|row| row.gram == gram).unwrap();
        let (before, after) = contributor_comparison(row.before, row.after, d0, d1);
        assert_eq!(change_color(SRAF, before, after), color);
    }
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
fn corner_weight_counts_only_physical_inner_corners_and_updates_swaps() {
    let text = crate::action_keys::test_layouts::QWERTY_JSONC;
    let board = board_from_text(text, Path::new("corner.jsonc")).unwrap();
    let model = Model::new(with_spare_keys(board));
    let selected: Vec<_> = model
        .geometry
        .corner
        .iter()
        .enumerate()
        .filter(|(_, corner)| **corner)
        .map(|(slot, _)| model.canonical[model.original[slot]])
        .collect();
    assert_eq!(selected, b"tybn");
    let source = Source::from_text_with_limits(
        r#"{"monograms":{"t":10,"q":1,"y":1,"b":1,"n":1," ":6},"bigrams":{"tq":1}}"#,
        Path::new("corner.json"),
        NgramLimits::default(),
    )
    .unwrap();
    let corpus = model.corpus(&source).unwrap();
    let mut raw = full_raw(&model.original, &corpus, &model.geometry);
    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[CORNER_WEIGHT] = 2.0;
    close(metrics(&raw, &corpus).corner, 65.0);
    close(breakdown(&metrics(&raw, &corpus), &weights).net, 130.0);
    let slot = |byte| {
        model
            .original
            .iter()
            .position(|&id| model.canonical[id] == byte)
            .unwrap()
    };
    let mut moved = model.original.clone();
    moved.swap(slot(b't'), slot(b'q'));
    for gram in &corpus.grams {
        move_gram(
            &mut raw,
            gram,
            &positions(&model.original),
            &positions(&moved),
            &model.geometry,
        );
    }
    close(raw.0[CORNER_RAW], 4.0);
    close(
        raw.0[CORNER_RAW],
        full_raw(&moved, &corpus, &model.geometry).0[CORNER_RAW],
    );
    let values = metrics(&raw, &corpus);
    close(values.corner, 20.0);
    close(breakdown(&values, &weights).net, 40.0);
    assert_eq!(
        weights_from_text("corner = 2\n").unwrap().0[CORNER_WEIGHT],
        2.0
    );
    assert_eq!(
        weights_from_text(&weights_text(&weights)).unwrap().0,
        weights.0
    );
    let rows = metric_score_rows(&values, &values, &weights);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "corner");
    close(rows[0].points.1, 40.0);
}

#[test]
fn usage_penalty_pairs_hands_ignores_thumbs_and_round_trips() {
    let mut raw = Raw::default();
    raw.0[USAGE] = 20.0;
    raw.0[USAGE + 7] = 10.0;
    raw.0[USAGE + 8] = 60.0;
    raw.0[OFF] = 5.0;
    raw.0[OFF + 7] = 4.0;
    let metrics = metrics_totals(&raw, &[100.0, 0.0, 0.0, 0.0]);
    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[N_METRICS] = 0.6;
    weights.0[USAGE_WEIGHT] = 0.06;
    let score = breakdown(&metrics, &weights);
    assert!((score.contributions[N_METRICS] - 5.4).abs() < 1e-12);
    assert!((score.contributions[USAGE_WEIGHT] - 1.8).abs() < 1e-12);
    assert!((score.net - 7.2).abs() < 1e-12);

    let parsed = weights_from_text("usage_pinky = 0.06\nusage_index = 0.015\n").unwrap();
    assert_eq!(parsed.0[USAGE_WEIGHT], 0.06);
    assert_eq!(parsed.0[USAGE_WEIGHT + 3], 0.015);
    assert_eq!(
        weights_from_text(&weights_text(&parsed)).unwrap().0,
        parsed.0
    );
    let defaults = Weights::default();
    for i in 0..4 {
        assert!(defaults.0[USAGE_WEIGHT + i] <= defaults.0[N_METRICS + i]);
    }
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
    p.settings.travel_limit = Some(0.0);
    p.settings.sftravel_limit = Some(0.0);
    let locks = default_locks(&p.model.board);

    p.settings.workers = 1;
    let serial = run_search(&p, &p.model.original, &locks, &Control::new(), &mut |_| {}).unwrap();
    let one_worker =
        run_search_parallel(&p, &p.model.original, &locks, &Control::new(), &mut |_| {}).unwrap();
    assert_eq!(one_worker.best.arr, serial.best.arr);
    assert_eq!(one_worker.best.score.to_bits(), serial.best.score.to_bits());
    assert_eq!(one_worker.progress.evaluations, serial.progress.evaluations);
    assert_eq!(one_worker.progress.accepted, serial.progress.accepted);

    p.settings.workers = 2;
    let groups: Vec<_> = (0..2)
        .map(|worker| {
            let started = Instant::now();
            run_search_plan(
                &p,
                &p.model.original,
                &locks,
                &Control::new(),
                worker_seed(p.settings.seed, worker),
                1,
                worker,
                started,
                None,
                &mut |_| {},
            )
            .unwrap()
        })
        .collect();
    let expected = merge_search_snapshots(
        &p,
        &[Some(groups[0].clone()), Some(groups[1].clone())],
        1,
        Instant::now(),
        Some("finished"),
    )
    .unwrap();
    let result =
        run_search_parallel(&p, &p.model.original, &locks, &Control::new(), &mut |_| {}).unwrap();
    assert_eq!(result.best.arr, expected.best.arr);
    assert_eq!(result.best.score.to_bits(), expected.best.score.to_bits());
    assert_eq!(result.progress.restart, 2);
    assert_eq!(result.progress.evaluations, expected.progress.evaluations);
    assert_eq!(result.progress.accepted, expected.progress.accepted);
    assert_eq!(result.archive.len(), expected.archive.len());
    for (candidate, expected) in result.archive.iter().zip(&expected.archive) {
        assert_eq!(candidate.arr, expected.arr);
        assert_eq!(candidate.score.to_bits(), expected.score.to_bits());
    }

    assert!(result.has_best);
    assert!(result.best.score <= State::new(p.model.original.clone(), &p).score + 1e-10);
    assert!(!result.archive.is_empty());
    for candidate in result.archive {
        let mut state = State::new(candidate.arr.clone(), &p);
        assert!(p.feasible(&state.raws));
        checked_rescore(&mut state, &p).unwrap();
        assert!((candidate.score - state.score).abs() < 1e-8);
        for (i, &locked) in locks.iter().enumerate() {
            if locked {
                assert_eq!(candidate.arr[i], p.model.original[i]);
            }
        }
    }
}

#[test]
fn physical_objectives_swap_matches_full_physical_recalculation() {
    {
        let mut problem = source_problem();
        problem.settings.mode = "aklgg".into();
        problem.weights.2 = mana2_metrics::Weights::from_text("sfb = -29\n").unwrap();
        let before = State::new(problem.model.original.clone(), &problem);
        let stats = mana_stats_for_corpus(&before.raws[0], &problem.corpora[0]);
        close(before.score, problem.weights.physical_score(&stats));
        let mut trial = before.clone();
        trial_into(&mut trial, &before, Move::pair(0, 1), &problem);
        let rebuilt = State::new(trial.arr.clone(), &problem);
        assert!((trial.score - rebuilt.score).abs() < 1e-9);
        for (incremental, full) in trial.raws.iter().zip(&rebuilt.raws) {
            let actual_speed = incremental.1.unwrap().1;
            let full_speed = full.1.unwrap().1;
            for (left, right) in actual_speed
                .bigrams
                .iter()
                .chain(&actual_speed.skipgrams)
                .zip(full_speed.bigrams.iter().chain(&full_speed.skipgrams))
            {
                assert!((left - right).abs() < 1e-9);
            }
            for (left, right) in incremental.1.unwrap().0.iter().zip(full.1.unwrap().0) {
                assert!((left - right).abs() < 1e-9);
            }
        }
    }
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("akler-test-{}-{}", std::process::id(), timestamp()));
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
fn moving_a_wildcard_key_updates_incremental_alt_finger_metrics() {
    let text =
        crate::action_keys::test_layouts::QWERTY_JSONC;
    let mut layout = action_keys::Layout::parse(text, Path::new("plain.jsonc")).unwrap();
    let counts = count_reader(
        Cursor::new(b"fsg fsg fsg"),
        &CorpusConfig::default(),
        11,
        &mut |_, _| {},
    )
    .unwrap()
    .0;
    let corpus =
        action_ngrams::NgramCorpus::from_text(&counts_text(&counts), Path::new("probe.json"))
            .unwrap();
    let rules = alt_fingers::AltRules::parse("f_g (LM LI)").unwrap();
    let alts = alt_fingers::AltPolicy {
        enabled: true,
        positions: alt_fingers::AltRules {
            rowstag: rules.clone(),
            colstag: rules,
        }
        .intern(),
    };
    let geometry = Geometry::new(action_ui::physical_keys(&layout));
    let stop = AtomicBool::new(false);
    let mut cache = action_ngrams::Incremental::new_with_policy(
        &corpus,
        &layout,
        geometry.clone(),
        &stop,
        alts,
        false,
        |_, _, _| 0.0,
    )
    .unwrap();
    cache.enable_finite_effort();
    let s = layout.slots.iter().position(|slot| slot.label == "s").unwrap();
    let d = layout.slots.iter().position(|slot| slot.label == "d").unwrap();
    let mut proposal = action_ngrams::Proposal::new();
    cache
        .propose_swap(s, d, &stop, |_, _, _| 0.0, &mut proposal)
        .unwrap();
    layout.swap(s, d);
    let fresh = action_ngrams::Incremental::new_with_policy(
        &corpus,
        &layout,
        geometry,
        &stop,
        alts,
        false,
        |_, _, _| 0.0,
    )
    .unwrap();
    close(fresh.metrics().v[SFS], 55.55555555555556);
    for (actual, expected) in proposal.metrics().v.into_iter().zip(fresh.metrics().v) {
        close(actual, expected);
    }
}

#[test]
fn alternate_fingers_preserve_unigrams_at_boundaries_and_gaps() {
    let layout = action_keys::Layout::parse(
        crate::action_keys::test_layouts::QWERTY_JSONC,
        Path::new("plain.jsonc"),
    )
    .unwrap();
    // No input below contains q or w, so the rule activates the contextual
    // path without changing any key's finger.
    let rules = alt_fingers::AltRules::parse("q_w (LM LI)").unwrap();
    let alts = alt_fingers::AltPolicy {
        enabled: true,
        positions: alt_fingers::AltRules {
            rowstag: rules.clone(),
            colstag: rules,
        }
        .intern(),
    };
    let inputs: &[&[u8]] = &[
        b"a",
        b"as",
        b"asd",
        b"asdf",
        b"asdf asdf",
        b"as\ndf",
        b"asd!fg",
        b"asdf!",
        b"as!d",
        b"aaaa",
        b"asdf asdf jkl;",
    ];
    let capped = NgramLimits {
        trigrams: Some(1),
        tetragrams: Some(1),
        pentagrams: Some(1),
        ..NgramLimits::default()
    };
    for order in [4, 5] {
        for &input in inputs {
            let mut config = CorpusConfig::default();
            config.order = order;
            let (source, _, _) = count_reader(
                Cursor::new(input),
                &config,
                input.len() as u64,
                &mut |_, _| {},
            )
            .unwrap();
            let mut text = counts_text(&source);
            if order == 4 {
                text.truncate(text.rfind(",\"fivegrams\":").unwrap());
                text.push('}');
            }
            for limits in [NgramLimits::default(), capped] {
                let corpus = action_ngrams::NgramCorpus::from_text_with_limits(
                    &text,
                    Path::new("inline.json"),
                    limits,
                )
                .unwrap();
                let result = corpus
                    .evaluate_with_policy(
                        &layout,
                        &AtomicBool::new(false),
                        &AtomicU64::new(0),
                        alts,
                        false,
                        |_, _, _| 0.0,
                    )
                    .unwrap();
                for (slot, key) in layout.slots.iter().enumerate() {
                    let action_keys::Binding::Text(bytes) = &key.binding else {
                        continue;
                    };
                    if bytes.len() != 1 {
                        continue;
                    }
                    let expected = source.uni[usize::from(bytes[0])] as f64;
                    let physical = result.tables[0].get(&[slot][..]).copied().unwrap_or(0.0);
                    let finger: f64 = result.finger_tables.as_ref().unwrap()[0]
                        .iter()
                        .filter(|(pattern, _)| pattern.slots == [slot])
                        .map(|(_, &frequency)| frequency)
                        .sum();
                    assert_eq!(
                        physical, expected,
                        "order {order}, input {input:?}, {limits:?}, key {}",
                        key.label
                    );
                    assert_eq!(
                        finger, expected,
                        "order {order}, input {input:?}, {limits:?}, key {}",
                        key.label
                    );
                }
                for (first, a) in layout.slots.iter().enumerate() {
                    let action_keys::Binding::Text(a) = &a.binding else {
                        continue;
                    };
                    if a.len() != 1 {
                        continue;
                    }
                    for (second, b) in layout.slots.iter().enumerate() {
                        let action_keys::Binding::Text(b) = &b.binding else {
                            continue;
                        };
                        if b.len() != 1 {
                            continue;
                        }
                        let expected =
                            source.bi[(usize::from(a[0]) << 7) | usize::from(b[0])] as f64;
                        let physical = result.tables[1]
                            .get(&[first, second][..])
                            .copied()
                            .unwrap_or(0.0);
                        let finger: f64 = result.finger_tables.as_ref().unwrap()[1]
                            .iter()
                            .filter(|(pattern, _)| pattern.slots == [first, second])
                            .map(|(_, &frequency)| frequency)
                            .sum();
                        assert_eq!(physical, expected, "physical bigram {a:?}/{b:?}: order {order}, input {input:?}, {limits:?}");
                        assert_eq!(
                            finger, expected,
                            "finger bigram {a:?}/{b:?}: order {order}, input {input:?}, {limits:?}"
                        );
                    }
                }
                for (pattern, &frequency) in &result.finger_tables.as_ref().unwrap()[1] {
                    assert!(frequency >= 0.0);
                    assert_eq!(
                        pattern.fingers,
                        pattern
                            .slots
                            .iter()
                            .map(|&slot| layout.slots[slot].finger)
                            .collect::<Vec<_>>()
                    );
                }
                let total: f64 = result.tables[0]
                    .entries()
                    .map(|(_, frequency)| frequency)
                    .sum();
                assert_eq!(
                    total, result.presses,
                    "order {order}, input {input:?}, {limits:?}"
                );
            }
        }
    }
}

#[test]
fn capped_alternate_finger_contexts_keep_the_keys_rules_read() {
    let layout = action_keys::Layout::parse(
        crate::action_keys::test_layouts::QWERTY_JSONC,
        Path::new("capped-alternates.jsonc"),
    )
    .unwrap();
    let slot = |label: &str| layout.slots.iter().position(|s| s.label == label).unwrap();
    let input = b"qwer qwer qwer qwer asfg asfg";
    let source = count_reader(
        Cursor::new(input),
        &CorpusConfig::default(),
        input.len() as u64,
        &mut |_, _| {},
    )
    .unwrap()
    .0;
    let text = counts_text(&source);
    // f moves to LM two keys after a, which a capped "asfg" context drops.
    let rules = alt_fingers::AltRules::parse("a_f (LP LM)").unwrap();
    let alts = alt_fingers::AltPolicy {
        enabled: true,
        positions: alt_fingers::AltRules {
            rowstag: rules.clone(),
            colstag: rules,
        }
        .intern(),
    };
    let pattern = action_ngrams::FingerPattern {
        slots: vec![slot("s"), slot("f")],
        fingers: vec![1, 2],
    };
    let capped = NgramLimits {
        tetragrams: Some(1),
        pentagrams: Some(1),
        ..NgramLimits::default()
    };
    let mut with_combo = layout.clone();
    with_combo.combos = vec![crate::parse_json(r#"{"keys":["q","w"],"output":"qw"}"#).unwrap()];
    for layout in [&layout, &with_combo] {
        for limits in [NgramLimits::default(), capped] {
            let corpus = action_ngrams::NgramCorpus::from_text_with_limits(
                &text,
                Path::new("inline.json"),
                limits,
            )
            .unwrap();
            let result = corpus
                .evaluate_with_policy(
                    layout,
                    &AtomicBool::new(false),
                    &AtomicU64::new(0),
                    alts,
                    true,
                    |_, _, _| 0.0,
                )
                .unwrap();
            assert_eq!(
                result.finger_tables.as_ref().unwrap()[1].get(&pattern),
                Some(&2.0),
                "{limits:?}, {} combos",
                layout.combos.len()
            );
        }
    }
    // `[ngrams] alt_contexts = 0` keeps no extra contexts: the capped one
    // falls back and loses the rule's first key.
    let corpus = action_ngrams::NgramCorpus::from_text_with_limits(
        &text,
        Path::new("inline.json"),
        NgramLimits {
            alt_contexts: 0,
            ..capped
        },
    )
    .unwrap();
    let result = corpus
        .evaluate_with_policy(
            &layout,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
            alts,
            true,
            |_, _, _| 0.0,
        )
        .unwrap();
    assert_ne!(
        result.finger_tables.as_ref().unwrap()[1].get(&pattern),
        Some(&2.0)
    );
}

#[test]
fn alternate_bigram_corrections_match_five_key_reference() {
    let layout = action_keys::Layout::parse(
        crate::action_keys::test_layouts::QWERTY_JSONC,
        Path::new("alternate-reference.jsonc"),
    )
    .unwrap();
    let defaults: Vec<_> = layout.slots.iter().map(|slot| slot.finger).collect();
    let cases = [
        ("g_x (LM LR)", true),
        ("asf (. . LM) g_x (LM LR)", true),
        ("fg (LR LI) g_x (LM LR)", true),
        ("afg (. . LR) g_x (LM LR)", true),
        ("afg (LR . LR) g_x (LM LR)", false),
        ("g_x (LM LM)", false),
    ];
    for (rule_text, expected_safe) in cases {
        let input = b"afgvx";
        let rules = alt_fingers::AltRules::parse(rule_text).unwrap();
        let alts = alt_fingers::AltPolicy {
            enabled: true,
            positions: alt_fingers::AltRules {
                rowstag: rules.clone(),
                colstag: rules,
            }
            .intern(),
        };
        let mapped: Vec<_> = input
            .iter()
            .map(|&byte| {
                layout.slots.iter().position(|slot| {
            matches!(&slot.binding, action_keys::Binding::Text(text) if text == &[byte])
        })
            })
            .collect();
        let compiled = alts.compile(&layout).unwrap();
        let safe = compiled.bigram_can_use_lookahead(input, &mapped, 2, &defaults);
        assert_eq!(safe, expected_safe, "{rule_text}");
        if !safe {
            continue;
        }
        let fingers = compiled.resolve(input, &mapped, &defaults);
        let expected = action_ngrams::FingerPattern {
            slots: vec![mapped[1].unwrap(), mapped[2].unwrap()],
            fingers: vec![fingers[1].unwrap(), fingers[2].unwrap()],
        };
        let source = count_reader(
            Cursor::new(input),
            &CorpusConfig::default(),
            input.len() as u64,
            &mut |_, _| {},
        )
        .unwrap()
        .0;
        let corpus =
            action_ngrams::NgramCorpus::from_text(&counts_text(&source), Path::new("inline.json"))
                .unwrap();
        let actual = corpus
            .evaluate_with_policy(
                &layout,
                &AtomicBool::new(false),
                &AtomicU64::new(0),
                alts,
                false,
                |_, _, _| 0.0,
            )
            .unwrap();
        assert_eq!(
            actual.finger_tables.unwrap()[1].get(&expected),
            Some(&1.0),
            "{rule_text}"
        );
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
    let header = format!("{{\"source\":{{\"engine\":\"akler-rust\",\"cache_version\":{},\"raw_size\":{},\"raw_mtime_ns\":{},\"config_fingerprint\":\"{:016x}\",\"max_order\":5}},\n\"letters\":{{}}}}\n", CORPUS_VERSION, meta.len(), json_quote(&mtime_ns(&meta)), 7u64);
    fs::write(&cache, header).unwrap();
    assert!(cache_current(&raw, &cache, &cfg, 7).unwrap());
    let legacy = fs::read_to_string(&cache)
        .unwrap()
        .replace("akler-rust", "layouter-rust");
    fs::write(&cache, legacy).unwrap();
    assert!(cache_current(&raw, &cache, &cfg, 7).unwrap());
    assert!(cache_current_at_least(&raw, &cache, 7, 3).unwrap());
    assert!(cache_current_at_least(&raw, &cache, 7, 4).unwrap());
    assert!(!cache_current(&raw, &cache, &cfg, 8).unwrap());
    fs::write(&raw, b"abcdefg\n").unwrap();
    assert!(!cache_current(&raw, &cache, &cfg, 7).unwrap());
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
                assert_eq!(
                    flags.bits & bit(INROLL) != 0,
                    matches!(expected, Some(IN2 | IN3))
                );
                assert_eq!(
                    flags.bits & bit(OUTROLL) != 0,
                    matches!(expected, Some(OUT2 | OUT3))
                );
                if expected.is_some() {
                    assert_eq!(flags.bits & bit(REDIR), 0);
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
        let gram = Gram::new(ids, 3, 3, frequency);
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

    let mut weights = Weights::new([0.0; N_WEIGHTS]);
    weights.0[ROLL] = 1000.0; // Combined total is display-only.
    weights.0[INROLL] = 0.25;
    weights.0[OUTROLL] = 0.5;
    for metric in [IN2, OUT2, IN3, OUT3] {
        weights.0[metric] = 1000.0; // Length breakdowns are display-only.
    }
    let score = breakdown(&metrics, &weights);
    assert_eq!(
        score.net,
        -metrics.v[INROLL] * 0.25 - metrics.v[OUTROLL] * 0.5
    );
    assert_eq!(score.contributions[ROLL], 0.0);
    for metric in [IN2, OUT2, IN3, OUT3] {
        assert_eq!(score.contributions[metric], 0.0);
        assert!(raw_positive(metric).is_none());
    }
    assert!(raw_positive(ROLL).is_none());
    assert!(N_RAW <= 128);
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
                alts: None,
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
        alts: None,
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
        assert_eq!(flags.bits & (bit(ALT) | bit(REDIR)), 0);
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
                &Gram::new(ids, 3, 3, f),
                &positions,
                &geometry,
                1.0,
            );
        }
        assert_eq!(raw.0[RHYTHM_DEN], 2.0);
        let metrics = metrics_totals(&raw, &[0.0, 0.0, 0.0, 10.0]);
        totals.push((raw.0[ROLL_DEN], metrics.v[IN2]));
    }
    assert_eq!(totals, [(2.0, 100.0), (10.0, 50.0)]);
}

#[test]
fn score_tab_rows_sum_to_the_objective_in_both_modes() {
    let mut problem = source_problem();
    let state = State::new(problem.model.original.clone(), &problem);
    let mut moved = state.clone();
    trial_into(&mut moved, &state, Move::pair(0, 1), &problem);
    let metrics = |s: &State| metrics(&s.raws[0], &problem.corpora[0]);
    let rows = metric_score_rows(&metrics(&state), &metrics(&moved), &problem.weights);
    close(
        rows.iter().map(|r| r.points.0).sum(),
        breakdown(&metrics(&state), &problem.weights).net,
    );
    close(
        rows.iter().map(|r| r.points.1).sum(),
        breakdown(&metrics(&moved), &problem.weights).net,
    );

    problem.settings.mode = "aklgg".into();
    let state = State::new(problem.model.original.clone(), &problem);
    let mut moved = state.clone();
    trial_into(&mut moved, &state, Move::pair(0, 1), &problem);
    let stats = |s: &State| mana_stats_for_corpus(&s.raws[0], &problem.corpora[0]);
    let rows = aklgg_score_rows(&stats(&state), &stats(&moved), &problem.weights);
    close(rows.iter().map(|r| r.points.0).sum(), state.score);
    close(rows.iter().map(|r| r.points.1).sum(), moved.score);
    assert!(rows.iter().any(|r| r.points.0 != r.points.1));
}

#[test]
fn simple_stats_show_jump_totals_in_the_alias_slots() {
    let model = model();
    let corpus = model.corpus(&small_source()).unwrap();
    let raw = full_raw(&model.original, &corpus, &model.geometry);
    let mut shown = metrics(&raw, &corpus);
    for (k, &m) in [DAFJB, CAFJB, DNFJB, CNFJB].iter().enumerate() {
        shown.v[m] = 1.0 + k as f64;
    }
    for (k, &m) in [DAHJB, CAHJB, DNHJB, CNHJB].iter().enumerate() {
        shown.v[m] = 0.5 * (1.0 + k as f64);
    }
    let mut c = Canvas::new(120, 40);
    editor_metric_cards(&mut c, 0, &shown, &shown, &raw, &corpus, 2, true);
    let text: String = c.cells.iter().map(|cell| cell.ch).collect();
    for label in ["FJB", "FJS", "HJB", "HJS", "10.00", "5.00"] {
        assert!(text.contains(label), "simple stats are missing {label}");
    }
    for label in ["FSB", "FSS", "HSB", "HSS"] {
        assert!(!text.contains(label), "simple stats still show {label}");
    }
}
