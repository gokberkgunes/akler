//! A corpus snapshot shared by successive layout opens in one TUI mode.
//!
//! Only the requested evaluator is prepared. Files are checked for metadata
//! changes on each open; leaving the layout chooser discards the snapshot.
use crate::action_ngrams::NgramCorpus;
use crate::*;

#[derive(Clone)]
struct CorpusSnapshot {
    path: PathBuf,
    text: Arc<str>,
    limits: NgramLimits,
    plain: Option<Arc<Source>>,
    action: Option<NgramCorpus>,
}

impl CorpusSnapshot {
    fn from_text(path: PathBuf, text: String, limits: NgramLimits) -> Self {
        Self {
            path,
            text: Arc::from(text),
            limits,
            plain: None,
            action: None,
        }
    }

    fn set_limits(&mut self, limits: NgramLimits) {
        if self.limits != limits {
            self.limits = limits;
            self.plain = None;
            self.action = None;
        }
    }

    fn plain(&mut self) -> action_keys::Result<Arc<Source>> {
        if let Some(source) = &self.plain {
            return Ok(Arc::clone(source));
        }

        let source = Arc::new(
            Source::from_text_with_limits(&self.text, &self.path, self.limits)
                .map_err(|error| format!("{}: {error}", self.path.display()))?,
        );
        self.plain = Some(Arc::clone(&source));
        Ok(source)
    }

    fn action(
        &mut self,
        stop: &AtomicBool,
        progress: &AtomicU64,
    ) -> action_keys::Result<NgramCorpus> {
        if let Some(corpus) = &self.action {
            return Ok(corpus.clone());
        }

        let corpus = NgramCorpus::from_text_progress_with_limits(
            &self.text,
            &self.path,
            self.limits,
            stop,
            progress,
        )?;
        self.action = Some(corpus.clone());
        Ok(corpus)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CorpusStamp {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl CorpusStamp {
    fn read(path: &Path) -> AppResult<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            path: path.to_owned(),
            bytes: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        })
    }
}

pub(crate) struct CorpusSession {
    stamp: Option<CorpusStamp>,
    snapshot: Option<CorpusSnapshot>,
}

impl CorpusSession {
    pub(crate) fn new() -> Self {
        Self {
            stamp: None,
            snapshot: None,
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.stamp = None;
        self.snapshot = None;
    }

    fn prepare_tui(&mut self, term: &mut Terminal, path: &Path, action: bool) -> AppResult<bool> {
        let mut timing = crate::load_profile::LoadProfile::new("session corpus snapshot");
        let path = prepare_corpus_path_tui(term, path)?;
        let limits = load_app_config()?.ngrams;
        let stamp = CorpusStamp::read(&path)?;
        timing.mark("Resolve cache and check metadata/configuration");

        let reusable = self.stamp.as_ref() == Some(&stamp);
        if reusable {
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.set_limits(limits);
                let ready = if action {
                    snapshot.action.is_some()
                } else {
                    snapshot.plain.is_some()
                };
                if ready {
                    timing.mark("Reuse prepared corpus");
                    return Ok(true);
                }
            }
        }

        let old_snapshot = if reusable {
            self.snapshot.clone()
        } else {
            None
        };
        let name = corpus_name(&path);
        let Some(snapshot) = crate::action_ui::ngram_job(
            term,
            if action {
                "Preparing action n-grams"
            } else {
                "Preparing ordinary n-grams"
            },
            &name,
            "",
            move |stop, progress| {
                let mut timing =
                    crate::load_profile::LoadProfile::new("prepare requested corpus engine");
                let mut snapshot = match old_snapshot {
                    Some(snapshot) => snapshot,
                    None => {
                        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                        CorpusSnapshot::from_text(path, text, limits)
                    }
                };
                timing.mark("Read corpus once or reuse text snapshot");
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }

                if action {
                    snapshot.action(stop, progress)?;
                } else {
                    snapshot.plain()?;
                }
                timing.mark("Prepare requested evaluator (nested reports may overlap)");
                if stop.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                progress.store(100, Ordering::Relaxed);
                Ok(snapshot)
            },
        )?
        else {
            return Ok(false);
        };
        timing.mark("Read and prepare requested corpus engine");

        // If the file changed during the read, the old stamp causes the next
        // open to reload it. Metadata is an invalidation hint, not a content hash.
        self.stamp = Some(stamp);
        self.snapshot = Some(snapshot);
        Ok(true)
    }

    pub(crate) fn plain_tui(
        &mut self,
        term: &mut Terminal,
        path: &Path,
    ) -> AppResult<Option<Arc<Source>>> {
        if !self.prepare_tui(term, path, false)? {
            return Ok(None);
        }
        Ok(self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.plain.clone()))
    }

    pub(crate) fn action_tui(
        &mut self,
        term: &mut Terminal,
        path: &Path,
    ) -> AppResult<Option<NgramCorpus>> {
        if !self.prepare_tui(term, path, true)? {
            return Ok(None);
        }
        Ok(self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.action.clone()))
    }
}

pub(crate) fn load_action_training(
    term: &mut Terminal,
    selected: &NgramCorpus,
    settings: &SearchSettings,
) -> AppResult<Option<Vec<(NgramCorpus, f64)>>> {
    let use_mix = settings.mix.values().any(|share| *share > 0.0);
    let share = if use_mix {
        settings
            .mix
            .get(&selected.name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0.0)
    } else {
        1.0
    };
    let mut result = vec![(selected.clone(), share)];
    if !use_mix {
        return Ok(Some(result));
    }

    let paths = corpus_paths()?;
    for (name, share) in &settings.mix {
        if *share > 0.0
            && !name.eq_ignore_ascii_case(&selected.name)
            && !paths
                .iter()
                .any(|path| corpus_name(path).eq_ignore_ascii_case(name))
        {
            return Err(format!("training corpus {name:?} not found").into());
        }
    }

    // Preserve the ordinary optimizer's discovered-corpus order and retain
    // the selected corpus first, even when its objective share is zero.
    let mut session = CorpusSession::new();
    for path in paths {
        let name = corpus_name(&path).to_ascii_lowercase();
        let share = settings.mix.get(&name).copied().unwrap_or(0.0);
        if share > 0.0 && !name.eq_ignore_ascii_case(&selected.name) {
            let Some(corpus) = session.action_tui(term, &path)? else {
                return Ok(None);
            };
            result.push((corpus, share));
        }
    }
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str =
        r#"{"letters":{"a":4,"b":3},"bigrams":{"ab":2,"ba":2},"trigrams":{"aba":1,"bab":1}}"#;

    fn snapshot() -> CorpusSnapshot {
        CorpusSnapshot::from_text(
            PathBuf::from("inline.json"),
            TEXT.into(),
            NgramLimits::default(),
        )
    }

    #[test]
    fn requested_engine_is_lazy_and_plain_storage_is_reused() {
        let mut snapshot = snapshot();
        assert!(snapshot.plain.is_none());
        assert!(snapshot.action.is_none());

        let first = snapshot.plain().unwrap();
        let second = snapshot.plain().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(snapshot.action.is_none());
        assert_eq!(
            first.tables,
            Source::from_text(TEXT, Path::new("inline.json"))
                .unwrap()
                .tables
        );
    }

    #[test]
    fn action_preparation_does_not_initialize_ordinary_tables() {
        let mut snapshot = snapshot();
        let corpus = snapshot
            .action(&AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap();
        assert_eq!(corpus.order, 3);
        assert!(snapshot.plain.is_none());
        assert!(snapshot.action.is_some());

        // A cached result requires no parser, so a cancellation flag cannot
        // make this in-memory lookup fail as a new parse would.
        assert!(snapshot
            .action(&AtomicBool::new(true), &AtomicU64::new(0))
            .is_ok());
    }

    #[test]
    fn limits_invalidate_both_engines_but_retain_the_text_snapshot() {
        let mut snapshot = snapshot();
        let text = Arc::clone(&snapshot.text);
        let original = snapshot.plain().unwrap();
        snapshot
            .action(&AtomicBool::new(false), &AtomicU64::new(0))
            .unwrap();
        snapshot.set_limits(NgramLimits::default());
        assert!(Arc::ptr_eq(&original, &snapshot.plain().unwrap()));
        assert!(snapshot.action.is_some());

        snapshot.set_limits(NgramLimits {
            trigrams: Some(1),
            ..NgramLimits::default()
        });
        assert!(snapshot.plain.is_none());
        assert!(snapshot.action.is_none());
        assert!(Arc::ptr_eq(&text, &snapshot.text));
        let limited = snapshot.plain().unwrap();
        assert_eq!(limited.tables[3].len(), 1);
        assert!(!Arc::ptr_eq(&original, &limited));
        assert_eq!(original.tables[3].len(), 2);
    }

    #[test]
    fn replaced_text_and_explicit_invalidation_do_not_reuse_old_tables() {
        let mut old = snapshot();
        let original = old.plain().unwrap();
        let mut new = CorpusSnapshot::from_text(
            PathBuf::from("inline.json"),
            TEXT.replace("\"a\":4", "\"a\":5"),
            NgramLimits::default(),
        );
        let replacement = new.plain().unwrap();
        assert_ne!(original.fingerprint, replacement.fingerprint);
        assert!(!Arc::ptr_eq(&original, &replacement));

        let mut session = CorpusSession::new();
        session.snapshot = Some(old);
        session.invalidate();
        assert!(session.stamp.is_none());
        assert!(session.snapshot.is_none());
    }
}
