//! Recoverable magic searches. Layout files are authoritative; saved scores are historical.
use crate::action_keys::{Layout, Result};
use crate::magic_design::DesignArchive;
use crate::*;
use std::io::Write;

fn numbers(values: &[f64]) -> Json {
    Json::Array(
        values
            .iter()
            .map(|&v| {
                if v.is_finite() {
                    Json::Number(v)
                } else {
                    Json::Null
                }
            })
            .collect(),
    )
}

fn write_new(path: &Path, text: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn save(archive: &DesignArchive, corpus: &str, weights: &Weights) -> Result<PathBuf> {
    save_in(archive, corpus, weights, Path::new("layouts/magic"))
}

fn save_in(
    archive: &DesignArchive,
    corpus: &str,
    weights: &Weights,
    parent: &Path,
) -> Result<PathBuf> {
    // Validate every export before creating any artifacts.
    let original = crate::layout_export::jsonc_text(&archive.baseline)?;
    let layouts = archive
        .candidates
        .iter()
        .map(|c| crate::layout_export::jsonc_text(&c.layout))
        .collect::<Result<Vec<_>>>()?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stem: String = archive
        .baseline
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("layout")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut selected = None;
    for n in 1..10000 {
        let path = parent.join(format!("{stem}-{n:03}"));
        match std::fs::create_dir(&path) {
            Ok(()) => {
                selected = Some(path);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let path = selected.ok_or("No unused magic archive directory is available.")?;
    let result = (|| {
        write_new(&path.join("original.jsonc"), &original)?;
        let mut table = String::from(
            "row\tfile\tpass\troot\tscore\tdelta\trules\trules_off_score\trules_gain\trules_note",
        );
        for name in METRIC_NAMES {
            table.push('\t');
            table.push_str(name);
        }
        table.push('\n');
        let mut candidates = Vec::new();
        for (index, (candidate, text)) in archive.candidates.iter().zip(&layouts).enumerate() {
            let file = format!("candidate-{:03}.jsonc", index + 1);
            write_new(&path.join(&file), text)?;
            table.push_str(&format!(
                "{}\t{}\t{}\t{}\t{:.12}\t{:.12}\t{}",
                index + 1,
                file,
                candidate.pass,
                candidate.root,
                candidate.score,
                candidate.score - archive.before.score,
                candidate.rules.len()
            ));
            let off = candidate
                .rules_off_score
                .map(|v| format!("{v:.12}"))
                .unwrap_or_else(|| "n/a".into());
            let gain = candidate
                .rules_off_score
                .map(|v| format!("{:.12}", v - candidate.score))
                .unwrap_or_else(|| "n/a".into());
            let note: String = candidate
                .rules_off_note
                .as_deref()
                .unwrap_or("")
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            table.push_str(&format!("\t{off}\t{gain}\t{note}"));
            for v in candidate.metrics.v {
                table.push_str(&format!("\t{v:.12}"));
            }
            table.push('\n');
            let rules = candidate
                .rules
                .iter()
                .map(|r| {
                    Json::Object(BTreeMap::from([
                        ("skip".into(), Json::Bool(r.skip)),
                        (
                            "context".into(),
                            Json::String((r.context as char).to_string()),
                        ),
                        (
                            "output".into(),
                            Json::String((r.output as char).to_string()),
                        ),
                        ("frequency".into(), Json::Number(r.frequency)),
                    ]))
                })
                .collect();
            candidates.push(Json::Object(BTreeMap::from([
                ("file".into(), Json::String(file)),
                ("pass".into(), Json::Number(candidate.pass as f64)),
                ("root".into(), Json::Number(candidate.root as f64)),
                ("score".into(), Json::Number(candidate.score)),
                (
                    "rules_off_score".into(),
                    candidate
                        .rules_off_score
                        .map(Json::Number)
                        .unwrap_or(Json::Null),
                ),
                (
                    "rules_gain".into(),
                    candidate
                        .rules_off_score
                        .map(|v| Json::Number(v - candidate.score))
                        .unwrap_or(Json::Null),
                ),
                (
                    "rules_comparison_note".into(),
                    candidate
                        .rules_off_note
                        .clone()
                        .map(Json::String)
                        .unwrap_or(Json::Null),
                ),
                ("metrics".into(), numbers(&candidate.metrics.v)),
                ("usage".into(), numbers(&candidate.metrics.usage)),
                ("off".into(), numbers(&candidate.metrics.off)),
                ("rules".into(), Json::Array(rules)),
            ])));
        }
        write_new(&path.join("table.tsv"), &table)?;
        let manifest = Json::Object(BTreeMap::from([
            ("schema".into(), Json::String("akler-magic-archive-1".into())),
            ("corpus".into(), Json::String(corpus.into())),
            ("source_layout".into(), Json::String(archive.baseline.path.to_string_lossy().into())),
            ("source_name".into(), Json::String(archive.baseline.name.clone())),
            ("source_order".into(), Json::Number(archive.source_order as f64)),
            ("settings".into(), Json::String(format!("{:?}", archive.settings))),
            ("allowed_roots".into(), Json::Array(archive.settings.rule_settings.roots.iter().map(|&root| Json::Number(root as f64)).collect())),
            ("objective".into(), Json::String(format!("{weights:?}"))),
            ("weights_text".into(), Json::String(weights_text(weights))),
            ("weights".into(), numbers(&weights.0)),
            ("baseline_score".into(), Json::Number(archive.before.score)),
            ("baseline_metrics".into(), numbers(&archive.before.metrics.v)),
            ("evaluations".into(), Json::Number(archive.candidate_count as f64)),
            ("warnings".into(), Json::Array(archive.warnings.iter().cloned().map(Json::String).collect())),
            ("score_note".into(), Json::String("Historical scores, including rules-off comparisons; recalculated on reopening against the current corpus and objective. Rules-off preserves the fallback.".into())),
            ("candidates".into(), Json::Array(candidates)),
        ]));
        // Manifest appears last, so its presence identifies a complete archive.
        write_new(
            &path.join("manifest.jsonc"),
            &crate::alt_fingers::compact_json(&manifest),
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        return match std::fs::remove_dir_all(&path) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!(
                "{error}; incomplete archive remains at {}: {cleanup}",
                path.display()
            )),
        };
    }
    Ok(path)
}

fn load_layout_file(file: &Path) -> Result<Layout> {
    if std::fs::symlink_metadata(file)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Magic archive layout cannot be a symbolic link.".into());
    }
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    crate::layout_io::parse_json_layout(&text, file)
}

pub(crate) fn load_baseline(path: &Path) -> Result<Layout> {
    let mut layout = load_layout_file(&path.join("original.jsonc"))?;
    let text =
        std::fs::read_to_string(path.join("manifest.jsonc")).map_err(|error| error.to_string())?;
    let Json::Object(manifest) = crate::layout_io::parse_jsonc(&text)? else {
        return Err("Invalid magic archive manifest.".into());
    };
    if let Some(Json::String(name)) = manifest.get("source_name") {
        layout.name = name.clone();
    } else if let Some(Json::String(source)) = manifest.get("source_layout") {
        if let Some(name) = Path::new(source).file_stem().and_then(|name| name.to_str()) {
            layout.name = name.to_owned();
        }
    }
    Ok(layout)
}

/// Physical slot restrictions saved with new archives; older archives use current defaults.
pub(crate) fn load_roots(path: &Path) -> Result<Option<Vec<usize>>> {
    let text = std::fs::read_to_string(path.join("manifest.jsonc")).map_err(|e| e.to_string())?;
    let Json::Object(manifest) = crate::layout_io::parse_jsonc(&text)? else {
        return Err("Invalid magic archive manifest.".into());
    };
    let Some(value) = manifest.get("allowed_roots") else {
        return Ok(None);
    };
    let Json::Array(values) = value else {
        return Err("Invalid magic archive allowed keys.".into());
    };
    let mut roots = Vec::new();
    for value in values {
        let Json::Number(n) = value else {
            return Err("Invalid magic archive allowed key.".into());
        };
        if !n.is_finite() || *n < 0.0 || n.fract() != 0.0 || *n >= usize::MAX as f64 {
            return Err("Invalid magic archive physical slot.".into());
        }
        let root = *n as usize;
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(Some(roots))
}

pub(crate) fn load_candidates(path: &Path) -> Result<Vec<Layout>> {
    let text = std::fs::read_to_string(path.join("manifest.jsonc")).map_err(|e| e.to_string())?;
    let Json::Object(manifest) = crate::layout_io::parse_jsonc(&text)? else {
        return Err("Invalid magic archive manifest.".into());
    };
    if !matches!(manifest.get("schema"), Some(Json::String(s)) if s == "akler-magic-archive-1") {
        return Err("Unsupported magic archive schema.".into());
    }
    let Some(Json::Array(rows)) = manifest.get("candidates") else {
        return Err("Missing magic archive candidates.".into());
    };
    rows.iter().enumerate().map(|(index, row)| {
        let expected = format!("candidate-{:03}.jsonc", index + 1);
        if !matches!(row, Json::Object(fields) if matches!(fields.get("file"), Some(Json::String(file)) if file == &expected)) {
            return Err("Invalid magic archive candidate filename.".into());
        }
        let file = path.join(expected);
        load_layout_file(&file)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_magic_archive_round_trip_is_self_contained_and_rejects_traversal() {
        let layout = Layout::parse(
            r#"{
                "layout": {
                    "fingers": ["q w e r t y u i o p", "a s d f g h j k l ;", "z x c v b n m , . /"],
                    "thumbs": ["space", ""]
                },
                "fingermap": ["0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9", "0 1 2 3 3 6 6 7 8 9"],
                "magic": {"magicKeys": [], "rules": [{"inputs": "qa", "output": "qq"}]}
            }"#,
            Path::new("source.dat"),
        )
        .unwrap();
        let corpus = crate::action_ngrams::NgramCorpus::from_text(
            r#"{"letters":{"i":2,"a":2,"r":2,"k":1},"bigrams":{"ii":1,"aa":1,"rk":1},"trigrams":{"aaa":1}}"#,
            Path::new("archive-test.json")).unwrap();
        let weights = Weights::default();
        let before = crate::action_summary::evaluate(
            &layout,
            &corpus,
            &weights,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap();
        let candidate = crate::magic_design::Candidate {
            layout: layout.clone(),
            score: before.score,
            rules_off_score: None,
            rules_off_note: None,
            raw: before.raw.clone(),
            totals: before.totals,
            metrics: before.metrics.clone(),
            root: 0,
            rules: Vec::new(),
            pass: 1,
        };
        let archive = DesignArchive {
            baseline: layout.clone(),
            before,
            candidates: vec![candidate],
            candidate_count: 1,
            warnings: vec!["test warning".into()],
            settings: crate::magic_design::IterativeSettings::default(),
            source_order: 3,
        };
        let parent = std::env::temp_dir().join(format!(
            "akler-magic-archive-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = save_in(&archive, "test-corpus", &weights, &parent).unwrap();
        let second = save_in(&archive, "test-corpus", &weights, &parent).unwrap();
        assert_ne!(first, second);
        let restored = load_candidates(&first).unwrap().remove(0);
        let baseline = load_baseline(&first).unwrap();
        assert_eq!(baseline.name, layout.name);
        assert_eq!(baseline.slots, layout.slots);
        assert_eq!(baseline.actions, layout.actions);
        assert_eq!(restored.slots, layout.slots);
        assert_eq!(restored.actions, layout.actions);
        assert_eq!(restored.left_outer, layout.left_outer);
        assert_eq!(restored.right_outer, layout.right_outer);
        let file = first.join("manifest.jsonc");
        let metadata = std::fs::read_to_string(&file).unwrap();
        assert!(metadata.contains("test-corpus"));
        assert!(metadata.contains("test warning"));
        std::fs::write(
            file,
            metadata.replace("candidate-001.jsonc", "../source.jsonc"),
        )
        .unwrap();
        assert!(load_candidates(&first).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }
}
