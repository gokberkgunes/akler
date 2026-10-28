fn magic_designer_main(term: &mut Terminal) -> AppResult<()> {
    let Some(path) = select_source_path(term, true)? else {
        return Ok(());
    };
    let Some(data) = load_ranking_tui(term, &path)? else {
        return Ok(());
    };
    let order = rank_layout_choices(&data.rows, "");
    let names: Vec<_> = order
        .iter()
        .map(|&i| data.rows[i].name().to_owned())
        .collect();
    if let Some(index) = search_menu(term, "Magic designer: layout", &names)? {
        magic_design_open(term, &data, order[index])?;
    }
    Ok(())
}

fn magic_root_label(layout: &action_keys::Layout, index: usize) -> String {
    let slot = &layout.slots[index];
    format!(
        "#{} {} · {} · row {} col {}",
        index + 1,
        slot.label,
        FINGER_NAMES.get(slot.finger).copied().unwrap_or("?"),
        slot.row + 1,
        slot.col + 1
    )
}

fn magic_design_open(term: &mut Terminal, data: &RankingData, selected: usize) -> AppResult<()> {
    use crate::magic_design::{IterativeSettings, PlacementMode, Settings};
    if data.mana2 {
        return info_page(
            term,
            "Magic design",
            &["Open an ordinary detailed ranking to design magic rules.".into()],
        );
    }
    let Some(row) = data.rows.get(selected) else {
        return Ok(());
    };
    let (mut original, existing_corpus) = match row {
        RankRow::Plain { model, .. } => (
            board_as_action_layout(&model.board, &model.board.symbols)?,
            None,
        ),
        RankRow::Action { layout, corpus, .. } => (layout.clone(), Some(Arc::clone(corpus))),
    };
    let corpus = match existing_corpus {
        Some(corpus) => corpus,
        None => {
            let path = data.path.clone();
            let limits = data.ngrams;
            let Some(corpus) = action_ui::ngram_job(
                term,
                move |stop, progress| {
                    let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                    action_ngrams::NgramCorpus::from_text_progress_with_limits(
                        &raw, &path, limits, stop, progress,
                    )
                },
            )?
            else {
                return Ok(());
            };
            Arc::new(corpus)
        }
    };
    let config = load_app_config()?;
    let search = config.search;
    let mut settings = IterativeSettings {
        rule_settings: Settings {
            roots: Vec::new(),
            locked_slots: Vec::new(),
            max_rules: 5,
            shortlist: 24,
            sfb_limit: search.sfb_limit,
            sfs_limit: search.sfs_limit,
            travel_limit: search.travel_limit,
            sftravel_limit: search.sftravel_limit,
        },
        passes: 3,
        workers: search.workers,
        archive: 20,
        placement: PlacementMode::ExtraBothPinkies,
    };
    let mut status = String::new();
    let mut preview = match crate::magic_design::prepare(&original, settings.placement) {
        Ok(layout) => layout,
        Err(error) => {
            status = error;
            settings.placement = PlacementMode::Existing;
            original.clone()
        }
    };
    let mut allowed: Vec<usize> = crate::magic_design::placement_roots(&preview)
        .into_iter()
        .filter(|&i| {
            let slot = &preview.slots[i];
            config
                .magic
                .allowed_keys
                .as_ref()
                .map_or(config.magic.allowed_fingers[slot.finger], |keys| {
                    keys.contains(&(slot.row as usize, slot.col as usize))
                })
        })
        .collect();
    let mut reference = preview.clone();
    let mut root = allowed
        .first()
        .copied()
        .unwrap_or_else(|| magic_preview_root(&preview));
    magic_move_preview(&mut preview, &reference, root);
    let mut archive: Option<crate::magic_design::DesignArchive> = None;
    let mut candidate = 0usize;
    let mut candidates_page = false;
    let mut selected_setting = 0usize;
    let mut candidate_offset = 0usize;
    let mut rule_report: Option<(String, Vec<String>)> = None;
    loop {
        term.refresh_size()?;
        let viewport_size = term.size;
        let chosen = archive.as_ref().and_then(|a| a.candidates.get(candidate));
        let shown = if candidates_page {
            chosen.map(|c| &c.layout).unwrap_or(&preview)
        } else {
            &preview
        };
        let selected_root = if candidates_page {
            chosen.map(|c| c.root)
        } else {
            Some(root)
        };
        let height = term.size.1.max(1);
        let footer = height.saturating_sub(2);
        let mut candidate_rows = 1usize;
        let mut c = Canvas::new(term.width(), height);
        let tabs = if c.w >= 30 {
            [" [1] Design ", " [2] Candidates "]
        } else {
            [" [1] ", " [2] "]
        };
        workspace_tabs(&mut c, usize::from(candidates_page), &tabs);
        c.text(
            0,
            1,
            &short(&format!("{} · {}", original.name, data.name), c.w),
            MUTED,
        );
        let mut y = action_ui::action_keyboard(
            &mut c,
            2,
            shown,
            if candidates_page && shown.slots.len() == reference.slots.len() {
                &reference
            } else {
                shown
            },
            None,
            selected_root,
        ) + 1;
        if candidates_page {
            if let Some(a) = &archive {
                if let Some(entry) = chosen {
                    y = grouped_metric_table(
                        &mut c,
                        y,
                        &a.before.metrics,
                        &entry.metrics,
                        &entry.raw,
                        &entry.totals,
                        term.decimals(),
                        if term.simple_stats {
                            &SIMPLE_TABLE_GROUPS
                        } else {
                            &EDITOR_TABLE_GROUPS
                        },
                        true,
                        false,
                    );
                    y = finger_table(
                        &mut c,
                        y + 1,
                        &a.before.metrics,
                        &entry.metrics,
                        term.decimals(),
                    );
                    y = score_panel(
                        &mut c,
                        y + 1,
                        &breakdown(&a.before.metrics, &data.weights),
                        &breakdown(&entry.metrics, &data.weights),
                        None,
                        term.decimals(),
                    );
                    y += 1;
                    if let Some(off) = entry.rules_off_score {
                        c.text(
                            0,
                            y,
                            &format!(
                                "Rules off {:.4} · Rules on {:.4} · Rules gain ",
                                off, entry.score
                            ),
                            MUTED,
                        );
                        let x = format!(
                            "Rules off {:.4} · Rules on {:.4} · Rules gain ",
                            off, entry.score
                        )
                        .chars()
                        .count();
                        c.text(
                            x,
                            y,
                            &format!("{:+.4}", off - entry.score),
                            if off > entry.score + 1e-9 {
                                GREEN
                            } else if off < entry.score - 1e-9 {
                                RED
                            } else {
                                FG
                            },
                        );
                    } else {
                        c.text(
                            0,
                            y,
                            &short(
                                &format!(
                                    "Rules comparison: n/a · {}",
                                    entry.rules_off_note.as_deref().unwrap_or("unavailable")
                                ),
                                c.w,
                            ),
                            MUTED,
                        );
                    }
                    y += 2;
                }
                // Keep three result rows available when the terminal can hold them.
                let list_reserve = 5usize;
                if y + list_reserve > footer {
                    c = Canvas::new(term.width(), height);
                    workspace_tabs(&mut c, 1, &tabs);
                    c.text(
                        0,
                        1,
                        &short(&format!("{} · {}", original.name, data.name), c.w),
                        MUTED,
                    );
                    y = 2;
                    let mut keyboard = Canvas::new(c.w, 40);
                    let keyboard_end = action_ui::action_keyboard(
                        &mut keyboard,
                        0,
                        shown,
                        &reference,
                        None,
                        selected_root,
                    );
                    if y + keyboard_end + 3 + list_reserve <= footer {
                        for row in 0..keyboard_end {
                            c.cells[(y + row) * c.w..(y + row + 1) * c.w]
                                .clone_from_slice(&keyboard.cells[row * c.w..(row + 1) * c.w]);
                        }
                        for (mut rect, action) in keyboard.hits {
                            rect.y += y;
                            c.hit(rect, action);
                        }
                        y += keyboard_end + 1;
                    } else if y + 2 + list_reserve <= footer {
                        c.text(
                            0,
                            y,
                            &short(
                                &format!(
                                    "Keys: {}",
                                    shown
                                        .slots
                                        .iter()
                                        .filter(|slot| slot.main)
                                        .map(|slot| slot.label.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                ),
                                c.w,
                            ),
                            MUTED,
                        );
                        y += 1;
                    }
                    if let Some(entry) = chosen {
                        let m = &entry.metrics.v;
                        c.text(
                            0,
                            y,
                            &short(
                                &format!(
                                    "Score {:.5} · SFB {:.4} · SFS {:.4} · LSB {:.4}",
                                    entry.score, m[SFB], m[SFS], m[LSB]
                                ),
                                c.w,
                            ),
                            FG,
                        );
                        y += 1;
                    }
                }
                // Very short terminals prioritize the candidate list.
                y = y.min(footer.saturating_sub(list_reserve));
                let results_start = y;
                c.text(
                    0,
                    y,
                    "     Score       SFB      SFS      LSB      FJB      RED  Rules gain",
                    FG,
                );
                y += 1;
                let m = &a.before.metrics.v;
                c.text(
                    0,
                    y,
                    &format!(
                        "B {:>10.5} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>8.4}",
                        a.before.score,
                        m[SFB],
                        m[SFS],
                        m[LSB],
                        m[DAFJB] + m[CAFJB] + m[DNFJB] + m[CNFJB],
                        m[REDIR]
                    ),
                    MUTED,
                );
                y += 1;
                candidate_rows = footer.saturating_sub(y).max(1);
                candidate_offset =
                    candidate_offset.min(a.candidates.len().saturating_sub(candidate_rows));
                if candidate < candidate_offset {
                    candidate_offset = candidate;
                } else if candidate >= candidate_offset + candidate_rows {
                    candidate_offset = candidate + 1 - candidate_rows;
                }
                for (i, entry) in a
                    .candidates
                    .iter()
                    .enumerate()
                    .skip(candidate_offset)
                    .take(candidate_rows)
                {
                    let m = &entry.metrics.v;
                    c.text(
                        0,
                        y,
                        &format!(
                            "{} {:>10.5} {:>8.4} {:>8.4} {:>8.4} {:>8.4} {:>8.4}",
                            if i == candidate { '●' } else { ' ' },
                            entry.score,
                            m[SFB],
                            m[SFS],
                            m[LSB],
                            m[DAFJB] + m[CAFJB] + m[DNFJB] + m[CNFJB],
                            m[REDIR]
                        ),
                        if i == candidate { CYAN } else { FG },
                    );
                    let before = &a.before.metrics.v;
                    for (x, value, baseline, width, precision) in [
                        (2, entry.score, a.before.score, 10, 5),
                        (13, m[SFB], before[SFB], 8, 4),
                        (22, m[SFS], before[SFS], 8, 4),
                        (31, m[LSB], before[LSB], 8, 4),
                        (
                            40,
                            m[DAFJB] + m[CAFJB] + m[DNFJB] + m[CNFJB],
                            before[DAFJB] + before[CAFJB] + before[DNFJB] + before[CNFJB],
                            8,
                            4,
                        ),
                        (49, m[REDIR], before[REDIR], 8, 4),
                    ] {
                        let color = if (value - baseline).abs() < 1e-9 {
                            FG
                        } else if value < baseline {
                            GREEN
                        } else {
                            RED
                        };
                        c.text(x, y, &format!("{value:>width$.precision$}"), color);
                    }
                    if let Some(off) = entry.rules_off_score {
                        c.text(
                            58,
                            y,
                            &format!("{:+10.4}", off - entry.score),
                            if off > entry.score + 1e-9 {
                                GREEN
                            } else if off < entry.score - 1e-9 {
                                RED
                            } else {
                                FG
                            },
                        );
                    } else {
                        c.text(58, y, "       n/a", MUTED);
                    }
                    let row = Rect {
                        x: 0,
                        y,
                        w: c.w.min(69),
                        h: 1,
                    };
                    if i == candidate {
                        c.row_style(row, Some((42, 42, 42)), false);
                    }
                    c.hit(row, Action::Item(i));
                    y += 1;
                }
                if c.w >= 120 && footer.saturating_sub(results_start) >= 8 {
                    if let Some(entry) = chosen {
                        magic_score_changes(
                            &mut c,
                            74,
                            results_start,
                            &a.before.metrics,
                            &entry.metrics,
                            &data.weights,
                        );
                    }
                }
            } else {
                c.text(
                    0,
                    y.min(footer.saturating_sub(1)),
                    "No candidates yet.",
                    MUTED,
                );
            }
        } else {
            if y + 6 > footer {
                c = Canvas::new(term.width(), height);
                workspace_tabs(&mut c, 0, &tabs);
                c.text(
                    0,
                    1,
                    &short(&format!("{} · {}", original.name, data.name), c.w),
                    MUTED,
                );
                y = 2.min(footer.saturating_sub(5));
                if y + 6 <= footer {
                    c.text(
                        0,
                        y,
                        &short(
                            &format!(
                                "Keys: {}",
                                shown
                                    .slots
                                    .iter()
                                    .filter(|slot| slot.main)
                                    .map(|slot| slot.label.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            ),
                            c.w,
                        ),
                        MUTED,
                    );
                    y += 1;
                }
            }
            let setting_rows = footer.saturating_sub(y).max(1);
            let setting_offset = selected_setting.saturating_sub(setting_rows.saturating_sub(1));
            for (index, (key, text)) in [
                (
                    'p',
                    format!(
                        "Blocked fingers: {}",
                        magic_blocked_fingers(&reference, &allowed)
                    ),
                ),
                ('3', format!("Passes: {}", settings.passes)),
                (
                    '4',
                    format!("Rules per key: {}", settings.rule_settings.max_rules),
                ),
                (
                    '5',
                    format!("Rule shortlist: {}", settings.rule_settings.shortlist),
                ),
                ('6', format!("Retain candidates: {}", settings.archive)),
            ]
            .into_iter()
            .enumerate()
            .skip(setting_offset)
            .take(setting_rows)
            {
                c.text(1, y, &text, FG);
                if index == selected_setting {
                    c.row_style(
                        Rect {
                            x: 0,
                            y,
                            w: c.w,
                            h: 1,
                        },
                        Some((42, 42, 42)),
                        false,
                    );
                }
                c.hit(
                    Rect {
                        x: 0,
                        y,
                        w: c.w,
                        h: 1,
                    },
                    Action::Command(key),
                );
                y += 1;
            }
        }
        // Rendering helpers may grow their canvas; the workspace always stays in the viewport.
        c.h = height;
        c.cells.truncate(c.w * height);
        c.hits.retain(|(rect, _)| rect.y < footer);
        c.text(0, footer, "? Help", MUTED);
        c.text(0, height.saturating_sub(1), &short(&status, c.w), FG);
        term.present(&c, 0)?;
        let mut event = term.event()?;
        if term.size != viewport_size {
            continue;
        }
        if let Event::Mouse {
            x,
            y,
            button: 0,
            release: false,
            motion: false,
            ..
        } = event
        {
            match term.hit(&c, x, y, 0) {
                Some(Action::Key(i)) if candidates_page && Some(i) == selected_root => {
                    event = Event::Enter
                }
                Some(Action::Item(i)) => candidate = i,
                Some(Action::Command(ch)) => {
                    if !candidates_page {
                        if let Some(index) =
                            ['p', '3', '4', '5', '6'].iter().position(|&key| key == ch)
                        {
                            selected_setting = index;
                        }
                    }
                    event = Event::Char(ch);
                }
                _ => {}
            }
        }
        if !candidates_page && matches!(event, Event::Enter) {
            event = Event::Char(['p', '3', '4', '5', '6'][selected_setting]);
        }
        match event {
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Char('j') | Event::Down if !candidates_page => {
                selected_setting = (selected_setting + 1).min(4);
            }
            Event::Char('k') | Event::Up if !candidates_page => {
                selected_setting = selected_setting.saturating_sub(1);
            }
            Event::Char('m') if candidates_page => term.toggle_stats_view(),
            Event::Char('1') => {
                candidates_page = false;
                candidate_offset = 0;
            }
            Event::Char('2') => {
                candidates_page = true;
                candidate_offset = 0;
            }
            Event::Char('p') if !candidates_page => {
                magic_finger_picker(term, &reference, &mut allowed)?;
                if !allowed.contains(&root) {
                    root = allowed
                        .first()
                        .copied()
                        .unwrap_or_else(|| magic_preview_root(&reference));
                }
                magic_move_preview(&mut preview, &reference, root);
                status.clear();
            }
            Event::Char(ch @ ('3'..='6')) if !candidates_page => {
                let (title, value, max) = match ch {
                    '3' => ("Passes", &mut settings.passes, 100),
                    '4' => (
                        "Rules per key",
                        &mut settings.rule_settings.max_rules,
                        crate::magic_design::MAX_RULES,
                    ),
                    '5' => ("Rule shortlist", &mut settings.rule_settings.shortlist, 256),
                    _ => ("Retain candidates", &mut settings.archive, 200),
                };
                if let Some(text) = input_box(
                    term,
                    title,
                    &format!("Integer from 1 to {max}"),
                    &value.to_string(),
                )? {
                    match text.parse::<usize>() {
                        Ok(n) if n > 0 && n <= max => *value = n,
                        _ => status = format!("{title}: enter an integer from 1 to {max}."),
                    }
                }
            }
            Event::Char('j') | Event::Down if candidates_page => {
                if let Some(a) = &archive {
                    candidate = (candidate + 1).min(a.candidates.len().saturating_sub(1));
                }
            }
            Event::Char('k') | Event::Up if candidates_page => {
                candidate = candidate.saturating_sub(1);
            }
            Event::Wheel(_) | Event::PageUp | Event::PageDown if candidates_page => {
                let delta = match event {
                    Event::Wheel(n) => n as i64,
                    Event::PageUp => -(candidate_rows as i64),
                    _ => candidate_rows as i64,
                };
                let last = archive
                    .as_ref()
                    .map_or(0, |a| a.candidates.len().saturating_sub(1));
                candidate = (candidate as i64 + delta).clamp(0, last as i64) as usize;
            }
            Event::Char('c') if candidates_page => {
                if let (Some(a), Some(entry)) = (&archive, chosen) {
                    let mut detail = Canvas::new(term.width(), 100);
                    let mut end = grouped_metric_table(
                        &mut detail,
                        0,
                        &a.before.metrics,
                        &entry.metrics,
                        &entry.raw,
                        &entry.totals,
                        term.decimals(),
                        &EDITOR_TABLE_GROUPS,
                        true,
                        false,
                    );
                    end = finger_table(
                        &mut detail,
                        end + 1,
                        &a.before.metrics,
                        &entry.metrics,
                        term.decimals(),
                    );
                    end = magic_score_changes(
                        &mut detail,
                        0,
                        end + 1,
                        &a.before.metrics,
                        &entry.metrics,
                        &data.weights,
                    );
                    let mut lines: Vec<String> = (0..end)
                        .map(|row| {
                            detail.cells[row * detail.w..(row + 1) * detail.w]
                                .iter()
                                .map(|cell| cell.ch)
                                .collect::<String>()
                                .trim_end()
                                .to_owned()
                        })
                        .collect();
                    lines.push(String::new());
                    lines.push(match entry.rules_off_score {
                        Some(off) => format!(
                            "Rules off {off:.4} · Rules on {:.4} · Rules gain {:+.4}",
                            entry.score,
                            off - entry.score
                        ),
                        None => format!(
                            "Rules comparison: n/a · {}",
                            entry.rules_off_note.as_deref().unwrap_or("unavailable")
                        ),
                    });
                    help_popup(term, &c, "Candidate details", &lines)?;
                }
            }
            Event::Enter if candidates_page => {
                if let Some(entry) = archive.as_ref().and_then(|a| a.candidates.get(candidate)) {
                    let key = format!(
                        "{}\nroot={}",
                        crate::layout_export::jsonc_text(&entry.layout)?,
                        entry.root
                    );
                    let lines = if let Some((_, lines)) =
                        rule_report.as_ref().filter(|(cached, _)| cached == &key)
                    {
                        lines.clone()
                    } else {
                        let layout = entry.layout.clone();
                        let root = entry.root;
                        let label = magic_root_label(&layout, root);
                        let weights = data.weights.clone();
                        let corpus = Arc::clone(&corpus);
                        let Some(effects) = action_ui::ngram_job(
                            term,
                            move |stop, progress| {
                                crate::magic_design::rule_effects(
                                    &layout, root, &corpus, &weights, stop, progress,
                                )
                            },
                        )?
                        else {
                            continue;
                        };
                        let mut lines = vec![label, String::new()];
                        for effect in effects {
                            lines.push(format!(
                                "After {:?}: types {:?}",
                                char::from(effect.rule.context),
                                char::from(effect.rule.output)
                            ));
                            if effect.direct_pair_saved > 1e-9 {
                                let repaired: Vec<_> = (0..N_METRICS)
                                    .filter(|&i| effect.direct_contribution_saved[i] > 1e-9)
                                    .map(|i| METRIC_NAMES[i])
                                    .collect();
                                lines.push(format!(
                                    "  Direct pair repair: {}.",
                                    repaired.join(", ")
                                ));
                            } else {
                                lines.push(
                                    "  No direct pair repair: excluded from new designs.".into(),
                                );
                            }
                            let penalty = if effect.penalty_saved > 1e-9 {
                                format!("Penalty reduced by {:.4} points", effect.penalty_saved)
                            } else if effect.penalty_saved < -1e-9 {
                                format!("Penalty increased by {:.4} points", -effect.penalty_saved)
                            } else {
                                "No penalty reduction".into()
                            };
                            let score = if effect.score_saved > 1e-9 {
                                format!("score improves by {:.4} points", effect.score_saved)
                            } else if effect.score_saved < -1e-9 {
                                format!("score worsens by {:.4} points", -effect.score_saved)
                            } else {
                                "no score change".into()
                            };
                            lines.push(format!("  {penalty}; {score}."));
                            let mut fixes: Vec<_> = (0..N_WEIGHTS)
                                .filter(|&i| {
                                    (i >= N_METRICS || !higher_better(i))
                                        && effect.contribution_saved[i] > 1e-9
                                })
                                .collect();
                            fixes.sort_by(|&a, &b| {
                                effect.contribution_saved[b]
                                    .total_cmp(&effect.contribution_saved[a])
                                    .then(a.cmp(&b))
                            });
                            for i in fixes.into_iter().take(3) {
                                if i < N_METRICS {
                                    lines.push(format!("  {}: {} without rule, {} with rule; {:.4} score points saved.", METRIC_NAMES[i], metric_value_text(i, effect.metrics_without[i], 4), metric_value_text(i, effect.metrics_with[i], 4), effect.contribution_saved[i]));
                                } else {
                                    lines.push(format!(
                                        "  {}: {:.4} score points saved.",
                                        weight_display_name(i),
                                        effect.contribution_saved[i]
                                    ));
                                }
                            }
                            lines.push(String::new());
                        }
                        if lines.len() == 2 {
                            lines.push("No explicit rules; this key uses its fallback.".into());
                        }
                        lines.push(
                            "Benefits compare this layout with that one rule removed.".into(),
                        );
                        rule_report = Some((key, lines.clone()));
                        lines
                    };
                    help_popup(term, &c, "Magic rules", &lines)?;
                }
            }
            Event::Char(' ') | Event::Char('r') => {
                if !candidates_page {
                    match crate::magic_design::prepare(&reference, PlacementMode::Existing) {
                        Ok(seed) if seed.slots == reference.slots => {}
                        Ok(_) => {
                            status = "Placement preview changed; select a placement again before starting.".into();
                            continue;
                        }
                        Err(error) => {
                            status = format!("Cannot start this placement: {error}");
                            continue;
                        }
                    }
                }
                if allowed.is_empty() {
                    status = "Unblock at least one finger (p) before searching.".into();
                    continue;
                }
                settings.rule_settings.roots = allowed.clone();
                let seed = if candidates_page {
                    let Some(entry) = archive.as_ref().and_then(|a| a.candidates.get(candidate))
                    else {
                        status = "Select a candidate to refine.".into();
                        continue;
                    };
                    entry.layout.clone()
                } else {
                    reference.clone()
                };
                let baseline = original.clone();
                let weights = data.weights.clone();
                let corpus = Arc::clone(&corpus);
                let mut run_settings = settings.clone();
                run_settings.placement = PlacementMode::Existing;

                let Some(mut next) = action_ui::ngram_job(
                    term,
                    move |stop, progress| {
                        crate::magic_design::iterative_refine(
                            &seed,
                            &baseline,
                            &corpus,
                            &weights,
                            &run_settings,
                            stop,
                            progress,
                        )
                    },
                )?
                else {
                    status = "Design cancelled; retained candidates are available.".into();
                    continue;
                };
                next.settings = settings.clone();
                if candidates_page {
                    if let Some(mut prior) = archive.take() {
                        next.candidate_count += prior.candidate_count;
                        next.candidates.append(&mut prior.candidates);
                        next.candidates.sort_by(|a, b| a.score.total_cmp(&b.score));
                        let mut seen = BTreeSet::new();
                        next.candidates.retain(|entry| {
                            let rule_count = match &entry.layout.slots[entry.root].binding {
                                action_keys::Binding::Named(name) => {
                                    match entry.layout.actions.get(name) {
                                        Some(action_keys::Action::Rules { rules, .. }) => {
                                            rules.len()
                                        }
                                        _ => usize::MAX,
                                    }
                                }
                                _ => usize::MAX,
                            };
                            allowed.contains(&entry.root)
                                && rule_count <= settings.rule_settings.max_rules
                                && layout_export::jsonc_text(&entry.layout)
                                    .is_ok_and(|text| seen.insert(text))
                        });
                        next.candidates.truncate(settings.archive);
                        next.warnings.extend(prior.warnings);
                    }
                }
                archive = Some(next);
                candidate = 0;
                candidates_page = true;
                candidate_offset = 0;
                status.clear();
            }
            Event::Char('S') => {
                if let Some(a) = &archive {
                    status = match crate::magic_archive::save(a, &data.name, &data.weights) {
                        Ok(path) => format!("Saved archive: {}", path.display()),
                        Err(error) => format!("Save failed: {error}"),
                    };
                }
            }
            Event::Char('s') => {
                if let Some(entry) = archive.as_ref().and_then(|a| a.candidates.get(candidate)) {
                    let mut standalone = entry.layout.clone();
                    let stem: String = original
                        .name
                        .chars()
                        .map(|ch| {
                            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                                ch
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    standalone.path = Path::new(LAYOUT_DIR).join(format!("{stem}-magic.jsonc"));
                    status = match crate::layout_export::save_jsonc_copy(&standalone) {
                        Ok(path) => saved_layout_message(&path),
                        Err(error) => format!("Save failed: {error}"),
                    };
                }
            }
            Event::Char('o') => {
                if let Some(path) = input_box(
                    term,
                    "Open magic archive",
                    "Saved archive folder",
                    "layouts/magic/",
                )? {
                    let folder = std::path::Path::new(&path);
                    let recovered = crate::magic_archive::load_baseline(folder).and_then(|base| {
                        crate::magic_archive::load_candidates(folder).and_then(|layouts| {
                            crate::magic_archive::load_roots(folder)
                                .map(|roots| (base, layouts, roots))
                        })
                    });
                    match recovered {
                        Ok((base, layouts, saved_roots)) => {
                            let recovery_preview = if let Some(first) = layouts.first() {
                                first.clone()
                            } else {
                                crate::magic_design::prepare(
                                    &base,
                                    PlacementMode::ExtraBothPinkies,
                                )?
                            };
                            let eligible = crate::magic_design::placement_roots(&recovery_preview);
                            let recovered_allowed = match saved_roots {
                                Some(roots) => {
                                    if roots.iter().any(|root| !eligible.contains(root)) {
                                        status = "Open failed: archive contains an unsupported allowed key.".into();
                                        continue;
                                    }
                                    roots
                                }
                                None => eligible
                                    .into_iter()
                                    .filter(|&i| {
                                        config
                                            .magic
                                            .allows(action_ui::physical_keys(&recovery_preview)[i])
                                    })
                                    .collect(),
                            };
                            let mut recovery_settings = settings.clone();
                            recovery_settings.rule_settings.roots = recovered_allowed;
                            let weights = data.weights.clone();
                            let corpus = Arc::clone(&corpus);
                            if let Some(loaded) = action_ui::ngram_job(
                                term,
                                move |stop, progress| {
                                    let before = action_summary::evaluate(
                                        &base, &corpus, &weights, stop, progress,
                                    )?;
                                    let mut entries = Vec::new();
                                    for layout in layouts {
                                        let root = magic_recovery_root(&layout)?;
                                        if !recovery_settings.rule_settings.roots.contains(&root) {
                                            continue;
                                        }
                                        let summary = action_summary::evaluate(
                                            &layout, &corpus, &weights, stop, progress,
                                        )?;
                                        let rules = magic_recovered_rules(&layout, root);
                                        entries.push(crate::magic_design::Candidate {
                                            layout,
                                            score: summary.score,
                                            rules_off_score: None,
                                            rules_off_note: None,
                                            raw: summary.raw,
                                            totals: summary.totals,
                                            metrics: summary.metrics,
                                            root,
                                            rules,
                                            pass: 0,
                                        });
                                    }
                                    entries.sort_by(|a, b| a.score.total_cmp(&b.score));
                                    crate::magic_design::fill_rules_off_comparisons(
                                        &mut entries,
                                        &corpus,
                                        &weights,
                                        stop,
                                        progress,
                                    )?;
                                    Ok(crate::magic_design::DesignArchive { baseline: base, before, candidate_count: entries.len(), candidates: entries,
                                        warnings: vec!["Recovered candidates rescored with the current corpus and weights; pass history and rule frequencies are unavailable.".into()], settings: recovery_settings, source_order: corpus.order })
                                },
                            )? {
                                original = loaded.baseline.clone();
                                settings.placement = PlacementMode::Existing;
                                preview = recovery_preview;
                                allowed = loaded.settings.rule_settings.roots.clone();
                                allowed.retain(|&i| i < preview.slots.len());
                                reference = preview.clone();
                                root = allowed
                                    .first()
                                    .copied()
                                    .unwrap_or_else(|| magic_preview_root(&preview));
                                magic_move_preview(&mut preview, &reference, root);
                                archive = Some(loaded);
                                candidate = 0;
                                candidates_page = true;
                                candidate_offset = 0;
                                status = "Recovered layouts rescored with the current corpus and weights.".into();
                            }
                        }
                        Err(error) => status = format!("Open failed: {error}"),
                    }
                }
            }
            Event::Char('?') => {
                let mut lines = vec![
                    "Workspace".into(),
                    "1/2\tDesign / Candidates".into(),
                    "o\tOpen an archive".into(),
                    "q\tBack".into(),
                    "".into(),
                    "Design".into(),
                    "j/k ↑/↓\tSelect setting".into(),
                    "Enter\tEdit selected setting".into(),
                    "p\tToggle finger blocks (both hands)".into(),
                    "3\tPasses".into(),
                    "4\tRules per key".into(),
                    "5\tRule shortlist".into(),
                    "6\tRetained candidates".into(),
                    "Space/r\tStart search".into(),
                    "".into(),
                    "Candidates".into(),
                    "j/k, wheel, PgUp/PgDn\tSelect candidate".into(),
                    "c\tAll cached metrics and score contributions".into(),
                    "Enter\tRules and their measured benefits".into(),
                    "m\tSimple / Detailed stats".into(),
                    "Click magic key\tShow selected magic rules".into(),
                    "r\tRefine candidate".into(),
                    "S\tSave all candidates".into(),
                    "s\tSave selected layout".into(),
                    "".into(),
                    "Search tries every location permitted by the finger blocks.".into(),
                    "Automatic searches only allowed physical keys; binding swaps do not change them.".into(),
                    "Rules must repair their own pair, then improve the finished layout.".into(),
                    "Rules are fitted from both the current dictionary and an empty dictionary.".into(),
                    "Search carries eight diverse designs between passes.".into(),
                    "At the rule limit, stalled fits try replacing one context with another.".into(),
                    "Stalled fitting refreshes its shortlist and tries bounded two-rule changes.".into(),
                    "Two refined designs feed one extra placement-search cycle.".into(),
                    "Joint rounds try ten single swaps, four two-swap designs, and six SFB-repair swaps.".into(),
                    "Joint refinement is followed by another allowed-location check.".into(),
                    "Two relocation finalists get a letter/rule round before comparison.".into(),
                    "Rules gain compares identical bindings with this dictionary cleared; the fallback stays active.".into(),
                    "Rules comparison is n/a if coverage or populations change.".into(),
                    "Search uses [search] workers; 0 uses available CPUs.".into(),
                    "Search uses the current corpus and Detailed weights; caps use the original layout.".into(),
                    "SFB: same-finger bigrams. SFS: same-finger skipgrams.".into(),
                    "LSB: lateral stretch bigrams. FJB: all full-jump bigrams. RED: redirects.".into(),
                    "Baseline is the original layout; colors compare against it.".into(),
                    "Score uses all Detailed weights, including stretches, travel, and finger usage.".into(),
                    "Score changes lists changed weighted terms; negative changes improve the score.".into(),
                ];
                if let Some(a) = &archive {
                    lines.push(format!(
                        "{} evaluations; {} retained candidates.",
                        a.candidate_count,
                        a.candidates.len()
                    ));
                    lines.extend(a.warnings.iter().cloned());
                }
                help_popup(term, &c, "Magic designer", &lines)?;
            }
            _ => {}
        }
    }
}

const MAGIC_FINGER_GROUPS: [(&str, [usize; 2]); 5] = [
    ("Pinky", [0, 7]),
    ("Ring", [1, 6]),
    ("Middle", [2, 5]),
    ("Index", [3, 4]),
    ("Thumb", [8, 9]),
];

fn magic_blocked_fingers(layout: &action_keys::Layout, allowed: &[usize]) -> String {
    let names: Vec<_> = MAGIC_FINGER_GROUPS
        .iter()
        .filter(|(_, fingers)| {
            !allowed
                .iter()
                .any(|&i| fingers.contains(&layout.slots[i].finger))
        })
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

fn magic_finger_picker(
    term: &mut Terminal,
    layout: &action_keys::Layout,
    allowed: &mut Vec<usize>,
) -> AppResult<()> {
    let mut enabled: [bool; 5] = std::array::from_fn(|group| {
        allowed.iter().any(|&i| {
            MAGIC_FINGER_GROUPS[group]
                .1
                .contains(&layout.slots[i].finger)
        })
    });
    let mut selected = 0;
    let mut changed = false;
    loop {
        let mut c = Canvas::new(term.width(), 10);
        c.text(0, 0, "Magic finger blocks · both hands", FG);
        for (i, (name, _)) in MAGIC_FINGER_GROUPS.iter().enumerate() {
            c.text(
                1,
                i + 2,
                &format!(
                    "{name:<8}  Block: {}",
                    if enabled[i] { "off" } else { "on" }
                ),
                FG,
            );
            if i == selected {
                c.row_style(
                    Rect {
                        x: 0,
                        y: i + 2,
                        w: c.w,
                        h: 1,
                    },
                    Some((42, 42, 42)),
                    false,
                );
            }
            c.hit(
                Rect {
                    x: 0,
                    y: i + 2,
                    w: c.w,
                    h: 1,
                },
                Action::Item(i),
            );
        }
        c.text(0, 8, "? Help", MUTED);
        term.present(&c, 0)?;
        match term.event()? {
            Event::Escape | Event::Quit | Event::Char('q') => break,
            Event::Char('j') | Event::Down => selected = (selected + 1).min(4),
            Event::Char('k') | Event::Up => selected = selected.saturating_sub(1),
            Event::Char(' ')
            | Event::Enter
            | Event::Char('h')
            | Event::Char('l')
            | Event::Left
            | Event::Right => {
                enabled[selected] = !enabled[selected];
                changed = true;
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false,
                ..
            } => {
                if let Some(Action::Item(i)) = term.hit(&c, x, y, 0) {
                    selected = i;
                    enabled[i] = !enabled[i];
                    changed = true;
                }
            }
            Event::Char('?') => help_popup(
                term,
                &c,
                "Magic finger blocks",
                &[
                    "j/k or arrows: select a finger.".into(),
                    "h/l, Space, Enter, or click: toggle its block.".into(),
                    "Block on excludes that finger on both hands.".into(),
                    "q or Esc: return. Changes apply to this workspace.".into(),
                ],
            )?,
            _ => {}
        }
    }
    if changed {
        *allowed = crate::magic_design::placement_roots(layout)
            .into_iter()
            .filter(|&i| {
                MAGIC_FINGER_GROUPS
                    .iter()
                    .enumerate()
                    .any(|(group, (_, fingers))| {
                        enabled[group] && fingers.contains(&layout.slots[i].finger)
                    })
            })
            .collect();
    }
    Ok(())
}

fn magic_recovery_root(layout: &action_keys::Layout) -> action_keys::Result<usize> {
    layout
        .slots
        .iter()
        .position(|slot| {
            slot.label == "*" && matches!(slot.binding, action_keys::Binding::Named(_))
        })
        .or_else(|| {
            crate::magic_design::eligible_roots(layout)
                .into_iter()
                .find(|&i| matches!(layout.slots[i].binding, action_keys::Binding::Named(_)))
        })
        .ok_or_else(|| "Recovered candidate has no supported magic key.".into())
}

fn magic_recovered_rules(
    layout: &action_keys::Layout,
    root: usize,
) -> Vec<crate::magic_design::Rule> {
    let mut result = Vec::new();
    let action_keys::Binding::Named(name) = &layout.slots[root].binding else {
        return result;
    };
    if let Some(action) = layout.actions.get(name) {
        if let action_keys::Action::Rules { basis, rules, .. } = action {
            let skip = matches!(
                basis,
                action_keys::Basis::SkipOutput | action_keys::Basis::SkipPress
            );
            for (context, emission) in rules {
                if let action_keys::Emission::Text(output) = emission {
                    if context.len() == 1 && output.len() == 1 {
                        result.push(crate::magic_design::Rule {
                            skip,
                            context: context[0],
                            output: output[0],
                            frequency: 0.0,
                        });
                    }
                }
            }
        }
    }
    result
}

fn magic_preview_root(layout: &action_keys::Layout) -> usize {
    layout
        .slots
        .iter()
        .position(|slot| {
            matches!(slot.binding, action_keys::Binding::Named(_))
                && crate::layout_io::dedicated_magic_label(&slot.label)
        })
        .unwrap_or_else(|| {
            crate::magic_design::eligible_roots(layout)
                .first()
                .copied()
                .unwrap_or(0)
        })
}

fn magic_move_preview(
    preview: &mut action_keys::Layout,
    reference: &action_keys::Layout,
    target: usize,
) {
    *preview = reference.clone();
    if let Some(root) = reference.slots.iter().position(|slot| {
        matches!(slot.binding, action_keys::Binding::Named(_))
            && crate::layout_io::dedicated_magic_label(&slot.label)
    }) {
        if root != target {
            preview.swap(root, target);
        }
    }
}

fn magic_score_changes(
    c: &mut Canvas,
    x: usize,
    mut y: usize,
    before: &Metrics,
    after: &Metrics,
    weights: &Weights,
) -> usize {
    let before = breakdown(before, weights);
    let after = breakdown(after, weights);
    c.text(x, y, "Score changes", FG);
    y += 1;
    c.text(x, y, "               Before     After     Change", MUTED);
    y += 1;
    for (name, old, new) in [
        ("Penalties", before.penalty, after.penalty),
        ("− Rewards", before.bonus, after.bonus),
        ("= Score", before.net, after.net),
    ] {
        c.text(
            x,
            y,
            &format!("{name:<13} {old:>9.4} {new:>9.4} {:+10.4}", new - old),
            FG,
        );
        y += 1;
    }
    y += 1;
    let mut changes: Vec<_> = (0..N_WEIGHTS)
        .filter(|&i| (after.contributions[i] - before.contributions[i]).abs() > 1e-9)
        .collect();
    changes.sort_by(|&a, &b| {
        (after.contributions[b] - before.contributions[b])
            .abs()
            .total_cmp(&(after.contributions[a] - before.contributions[a]).abs())
            .then(a.cmp(&b))
    });
    for i in changes {
        let old = before.contributions[i];
        let new = after.contributions[i];
        let delta = new - old;
        c.text(
            x,
            y,
            &format!(
                "{:<13} {old:>9.4} {new:>9.4} {delta:+10.4}",
                weight_display_name(i)
            ),
            if delta < 0.0 { GREEN } else { RED },
        );
        y += 1;
    }
    y
}
