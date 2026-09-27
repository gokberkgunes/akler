//! Atomic pattern editor. An edit owns its layout and prepared report snapshot.
use crate::action_keys;
use crate::action_ngrams::NgramCorpus;
use crate::atomic_report::{self, AtomicReport, Population, PreparedAtomic};
use crate::*;

enum Drawing {
    Plain {
        model: Model,
        slots: Vec<usize>,
    },
    Action {
        current: action_keys::Layout,
        original: action_keys::Layout,
    },
}

enum CorpusData {
    Plain(Arc<Source>),
    Action(NgramCorpus),
}

struct UndoEntry {
    drawing: Drawing,
    prepared: PreparedAtomic,
}

struct GroupDetail {
    group_index: usize,
    title: String,
    header: String,
    rows: Vec<String>,
}

struct AtomicView {
    prepared: PreparedAtomic,
    drawing: Drawing,
    corpus: CorpusData,
    report: AtomicReport,
    selected: Population,
    query: Option<String>,
    grouping: Option<atomic_report::Grouping>,
    table_header: String,
    row_lines: Vec<String>,
    detail: Option<GroupDetail>,
    table_cursor: usize,
    scroll: usize,
    error: Option<String>,
    status: Option<String>,
    undo: Vec<UndoEntry>,
    cursor: usize,
    keyboard_mode: bool,
    selected_key: Option<usize>,
    drag: Option<usize>,
}

impl AtomicView {
    fn new(prepared: PreparedAtomic, drawing: Drawing, corpus: CorpusData) -> AppResult<Self> {
        let population = [Population::Bigrams, Population::Trigrams, Population::Skip1]
            .into_iter()
            .find(|&population| prepared.available(population))
            .ok_or("no atomic pattern population is available in this corpus")?;
        let report = prepared.report(population, None)?;
        let selected = report.population;
        let (table_header, row_lines) = atomic_report::table_lines(&report);
        Ok(Self {
            prepared,
            drawing,
            corpus,
            report,
            selected,
            query: None,
            grouping: None,
            table_header,
            row_lines,
            detail: None,
            table_cursor: 0,
            scroll: 0,
            error: None,
            status: None,
            undo: Vec::new(),
            cursor: 0,
            keyboard_mode: false,
            selected_key: None,
            drag: None,
        })
    }

    fn prepare(&self, drawing: &Drawing) -> AppResult<PreparedAtomic> {
        match (drawing, &self.corpus) {
            (Drawing::Plain { model, slots }, CorpusData::Plain(source)) => {
                let mut board = model.board.clone();
                board.symbols = model.symbols(slots);
                atomic_report::prepare_ordinary(board, (**source).clone())
            }
            (Drawing::Action { current, .. }, CorpusData::Action(corpus)) => {
                atomic_report::prepare_action(current.clone(), corpus.clone())
            }
            _ => Err("atomic layout and corpus engines do not match".into()),
        }
    }

    fn presentation(
        report: &AtomicReport,
        grouping: Option<&atomic_report::Grouping>,
    ) -> AppResult<(String, Vec<String>)> {
        match grouping {
            Some(grouping) => atomic_report::grouped_lines(report, grouping),
            None => Ok(atomic_report::table_lines(report)),
        }
    }

    fn grouping_text(&self) -> &str {
        self.grouping
            .as_ref()
            .map_or("none", atomic_report::Grouping::text)
    }

    fn visible_table(&self) -> (&str, &[String]) {
        match &self.detail {
            Some(detail) => (&detail.header, &detail.rows),
            None => (&self.table_header, &self.row_lines),
        }
    }

    fn enter_group(&mut self) -> AppResult<()> {
        if self.detail.is_some() {
            return Ok(());
        }
        let Some(grouping) = &self.grouping else {
            return Ok(());
        };
        let groups = atomic_report::group_buckets(&self.report, grouping)?;
        let Some(group) = groups.get(self.table_cursor) else {
            return Ok(());
        };
        let mut selected = self.report.clone();
        selected.rows = group
            .row_indices
            .iter()
            .map(|&index| self.report.rows[index].clone())
            .collect();
        let (header, rows) = atomic_report::table_lines(&selected);
        self.detail = Some(GroupDetail {
            group_index: self.table_cursor,
            title: format!(
                "{} ({} patterns, {} of population)",
                group.values.join(" → "),
                group.row_indices.len(),
                atomic_report::percent(group.frequency, self.report.population_frequency)
            ),
            header,
            rows,
        });
        self.table_cursor = 0;
        self.scroll = 0;
        Ok(())
    }

    fn leave_group(&mut self) -> bool {
        if let Some(detail) = self.detail.take() {
            self.table_cursor = detail.group_index;
            self.scroll = 0;
            true
        } else {
            false
        }
    }

    fn move_table_cursor(&mut self, delta: isize, canvas: &Canvas, height: usize) {
        let row_count = self.visible_table().1.len();
        if row_count == 0 {
            return;
        }
        let current_visible = canvas.hits.iter().any(|(rect, action)| {
            matches!(action, Action::Item(index) if *index == self.table_cursor)
                && rect.y >= self.scroll
                && rect.y < self.scroll + height
        });
        if !current_visible {
            if let Some(index) = visible_item_cursor(canvas, self.scroll, height, delta > 0) {
                self.table_cursor = index;
            }
        }
        let previous = self.table_cursor;
        self.table_cursor = self
            .table_cursor
            .saturating_add_signed(delta)
            .min(row_count - 1);
        if self.table_cursor == previous && delta != 0 {
            self.scroll_by(delta.signum(), canvas.h, height);
            return;
        }
        if let Some((rect, _)) = canvas.hits.iter().find(
            |(_, action)| matches!(action, Action::Item(index) if *index == self.table_cursor),
        ) {
            if rect.y < self.scroll {
                self.scroll = rect.y;
            } else if rect.y >= self.scroll + height.max(1) {
                self.scroll = rect.y + 1 - height.max(1);
            }
        }
    }

    fn swapped_drawing(&self, a: usize, b: usize) -> AppResult<Drawing> {
        match &self.drawing {
            Drawing::Plain { model, slots } => {
                if a >= slots.len() || b >= slots.len() {
                    return Err("physical slot is out of range".into());
                }
                if model.canonical[slots[a]] == b' ' || model.canonical[slots[b]] == b' ' {
                    return Err("Space stays fixed".into());
                }
                let mut next = slots.clone();
                next.swap(a, b);
                Ok(Drawing::Plain {
                    model: model.clone(),
                    slots: next,
                })
            }
            Drawing::Action { current, original } => {
                if a >= current.slots.len() || b >= current.slots.len() {
                    return Err("physical slot is out of range".into());
                }
                if current.space(a) || current.space(b) {
                    return Err("Space stays fixed".into());
                }
                let mut next = current.clone();
                next.swap(a, b);
                Ok(Drawing::Action {
                    current: next,
                    original: original.clone(),
                })
            }
        }
    }

    fn swap(&mut self, a: usize, b: usize) -> AppResult<()> {
        if a == b {
            return Ok(());
        }
        let next_drawing = self.swapped_drawing(a, b)?;
        let next_prepared = self.prepare(&next_drawing)?;
        let next_report = next_prepared.report(self.selected, self.query.as_deref())?;
        let presentation = Self::presentation(&next_report, self.grouping.as_ref())?;
        let old = UndoEntry {
            drawing: std::mem::replace(&mut self.drawing, next_drawing),
            prepared: std::mem::replace(&mut self.prepared, next_prepared),
        };
        self.undo.push(old);
        (self.table_header, self.row_lines) = presentation;
        self.report = next_report;
        self.detail = None;
        self.table_cursor = 0;
        self.scroll = 0;
        self.error = None;
        self.selected_key = None;
        self.drag = None;
        self.status = Some(format!("Swapped physical slots {a} and {b}"));
        Ok(())
    }

    fn undo(&mut self) -> AppResult<()> {
        let Some(old) = self.undo.last() else {
            self.status = Some("Nothing to undo".into());
            return Ok(());
        };
        let report = old.prepared.report(self.selected, self.query.as_deref())?;
        let presentation = Self::presentation(&report, self.grouping.as_ref())?;
        let old = self.undo.pop().unwrap();
        self.drawing = old.drawing;
        self.prepared = old.prepared;
        (self.table_header, self.row_lines) = presentation;
        self.report = report;
        self.detail = None;
        self.table_cursor = 0;
        self.scroll = 0;
        self.error = None;
        self.selected_key = None;
        self.drag = None;
        self.status = Some("Undo".into());
        Ok(())
    }

    fn save_copy(&mut self) {
        let result: AppResult<PathBuf> = match &self.drawing {
            Drawing::Plain { model, slots } => save_new_layout(&model.board, &model.symbols(slots)),
            Drawing::Action { current, .. } => current.save_new().map_err(|error| error.into()),
        };
        self.status = Some(match result {
            Ok(path) => saved_layout_message(&path),
            Err(error) => format!("Save failed: {error}"),
        });
    }

    fn physical_keys(&self) -> Vec<Key> {
        match &self.drawing {
            Drawing::Plain { model, .. } => model.board.keys.clone(),
            Drawing::Action { current, .. } => action_ui::physical_keys(current),
        }
    }

    fn changed(&self) -> bool {
        match &self.drawing {
            Drawing::Plain { model, slots } => *slots != model.original,
            Drawing::Action { current, original } => current
                .slots
                .iter()
                .zip(&original.slots)
                .any(|(a, b)| a.binding != b.binding),
        }
    }

    fn apply(&mut self, population: Population, query: Option<String>) {
        match self.prepared.report(population, query.as_deref()) {
            Ok(report) => {
                let (table_header, row_lines) =
                    match Self::presentation(&report, self.grouping.as_ref()) {
                        Ok(presentation) => presentation,
                        Err(error) => {
                            self.error = Some(format!(
                                "{} query {:?}, grouping {}: {error}",
                                population.name(),
                                query.as_deref().unwrap_or("all patterns"),
                                self.grouping_text()
                            ));
                            self.status = None;
                            return;
                        }
                    };
                self.selected = population;
                self.table_header = table_header;
                self.row_lines = row_lines;
                self.report = report;
                self.query = query;
                self.detail = None;
                self.table_cursor = 0;
                self.scroll = 0;
                self.error = None;
                self.status = None;
            }
            Err(error) => {
                self.error = Some(format!(
                    "{} query {:?}: {error}",
                    population.name(),
                    query.as_deref().unwrap_or("all patterns")
                ));
                self.status = None;
            }
        }
    }

    fn submit_query(&mut self, text: &str) {
        let trimmed = text.trim();
        let query = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        self.apply(self.selected, query);
    }

    fn cycle_population(&mut self) {
        self.apply(self.selected.next(), self.query.clone());
    }

    fn submit_grouping(&mut self, text: &str) {
        let trimmed = text.trim();
        let grouping = if trimmed.is_empty() {
            None
        } else {
            match atomic_report::Grouping::parse(trimmed) {
                Ok(grouping) => Some(grouping),
                Err(error) => {
                    self.error = Some(format!("grouping {trimmed:?}: {error}"));
                    self.status = None;
                    return;
                }
            }
        };
        match Self::presentation(&self.report, grouping.as_ref()) {
            Ok((table_header, row_lines)) => {
                self.grouping = grouping;
                self.table_header = table_header;
                self.row_lines = row_lines;
                self.detail = None;
                self.table_cursor = 0;
                self.scroll = 0;
                self.error = None;
                self.status = None;
            }
            Err(error) => {
                self.error = Some(format!("grouping {trimmed:?}: {error}"));
                self.status = None;
            }
        }
    }

    fn export(&mut self, path: &Path) {
        if path.as_os_str().is_empty() {
            self.status = Some("Export path is empty".into());
            return;
        }
        let text = match &self.grouping {
            Some(grouping) => match atomic_report::render_grouped_text(&self.report, grouping) {
                Ok(text) => text,
                Err(error) => {
                    self.status = Some(format!("Export failed: {error}"));
                    return;
                }
            },
            None => atomic_report::render_text(&self.report),
        };
        self.status = Some(match atomic_report::write_new_report(path, &text) {
            Ok(()) => format!(
                "Exported {} rows to {}",
                self.report.rows.len(),
                path.display()
            ),
            Err(error) => format!("Export failed: {error}"),
        });
    }

    fn clamp_scroll(&mut self, content_height: usize, viewport_height: usize) {
        self.scroll = self
            .scroll
            .min(content_height.saturating_sub(viewport_height));
    }

    fn scroll_by(&mut self, delta: isize, content_height: usize, viewport_height: usize) {
        let maximum = content_height.saturating_sub(viewport_height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(maximum);
    }

    fn scroll_table(&mut self, event: &Event, canvas: &Canvas, height: usize) -> bool {
        let previous = self.scroll;
        if !scroll_event(event, &mut self.scroll, canvas.h, height) {
            return false;
        }
        if let Some(index) =
            visible_item_cursor(canvas, self.scroll, height, self.scroll < previous)
        {
            self.table_cursor = index;
        }
        true
    }

    fn render(&self, width: usize) -> Canvas {
        let mut canvas = Canvas::new(width, 5);
        canvas.text(
            0,
            0,
            &short(
                if self.changed() {
                    "Atomic editor *"
                } else {
                    "Atomic editor"
                },
                canvas.w,
            ),
            FG,
        );
        canvas.text(
            0,
            1,
            &short(
                &format!(
                    "Layout: {}   Corpus: {}",
                    self.report.layout, self.report.corpus
                ),
                canvas.w,
            ),
            FG,
        );
        canvas.text(
            0,
            2,
            &short(
                &format!(
                    "Population: {}   Query: {}",
                    self.selected.name(),
                    self.query.as_deref().unwrap_or("all patterns")
                ),
                canvas.w,
            ),
            BLUE,
        );
        canvas.text(
            0,
            3,
            &short(&format!("Grouping: {}", self.grouping_text()), canvas.w),
            BLUE,
        );
        canvas.text(
            0,
            4,
            &short(
                "Arrows/Space keys   j/k table   Enter/l open group   h back   u undo   g grouping   / query   ? help   q quit",
                canvas.w,
            ),
            MUTED,
        );
        let highlights: Vec<_> = self.selected_key.into_iter().chain(self.drag).collect();
        let keyboard_end = match &self.drawing {
            Drawing::Plain { model, slots } => keyboard(
                &mut canvas,
                5,
                model,
                slots,
                &model.original,
                None,
                self.keyboard_mode.then_some(self.cursor),
                &highlights,
            ),
            Drawing::Action { current, original } => action_ui::action_keyboard(
                &mut canvas,
                5,
                current,
                original,
                None,
                self.selected_key
                    .or(self.drag)
                    .or(self.keyboard_mode.then_some(self.cursor)),
            ),
        };
        let mut y = keyboard_end.max(6);
        let pct = atomic_report::percent(
            self.report.matching_frequency,
            self.report.population_frequency,
        );
        canvas.text(
            0,
            y,
            &short(
                &format!(
                    "Matching: {:.6} / {:.6} ({pct})   {} rows",
                    self.report.matching_frequency,
                    self.report.population_frequency,
                    self.report.rows.len()
                ),
                canvas.w,
            ),
            CYAN,
        );
        y += 1;
        if let Some(error) = &self.error {
            canvas.text(0, y, &short(&format!("ERROR: {error}"), canvas.w), RED);
            y += 1;
            canvas.text(
                0,
                y,
                &short(
                    &format!(
                        "Showing previous valid results: {} / {} / grouping {}",
                        self.report.population.name(),
                        self.report.query,
                        self.grouping_text()
                    ),
                    canvas.w,
                ),
                YELLOW,
            );
            y += 1;
        }
        if let Some(status) = &self.status {
            canvas.text(
                0,
                y,
                &short(status, canvas.w),
                if status.starts_with("Export failed")
                    || status.starts_with("Export path")
                    || status.starts_with("Save failed")
                    || status.starts_with("Swap failed")
                    || status.starts_with("Undo failed")
                    || status.starts_with("Group failed")
                {
                    RED
                } else {
                    CYAN
                },
            );
            y += 1;
        }
        for warning in &self.report.warnings {
            canvas.text(
                0,
                y,
                &short(&format!("Warning: {warning}"), canvas.w),
                YELLOW,
            );
            y += 1;
        }
        canvas.text(
            0,
            y,
            &short(&format!("Source: {}", self.report.source), canvas.w),
            MUTED,
        );
        y += 2;
        if let Some(detail) = &self.detail {
            canvas.text(
                0,
                y,
                &short(&format!("Group: {}   (h/Esc back)", detail.title), canvas.w),
                CYAN,
            );
            y += 1;
        }
        let (table_header, row_lines) = self.visible_table();
        canvas.text(0, y, &short(table_header, canvas.w), FG);
        y += 1;
        if row_lines.is_empty() {
            canvas.text(
                0,
                y,
                if self.report.population_frequency == 0.0 {
                    "Empty population"
                } else {
                    "No matching patterns"
                },
                MUTED,
            );
            y += 1;
        } else {
            for (index, line) in row_lines.iter().enumerate() {
                canvas.text(
                    0,
                    y,
                    &short(line, canvas.w),
                    if index == self.table_cursor {
                        YELLOW
                    } else {
                        FG
                    },
                );
                canvas.hit(
                    Rect {
                        x: 0,
                        y,
                        w: canvas.w,
                        h: 1,
                    },
                    Action::Item(index),
                );
                y += 1;
            }
        }
        y += 1;
        canvas.text(
            0,
            y,
            &short(
                "j/k table   Enter/l open   h/Esc group back   Arrows/Space keys   g group   s save   x report   Pg scroll   q quit",
                canvas.w,
            ),
            MUTED,
        );
        canvas
    }
}

fn visible_item_cursor(canvas: &Canvas, scroll: usize, height: usize, last: bool) -> Option<usize> {
    let visible = canvas.hits.iter().filter_map(|(rect, action)| {
        if rect.y >= scroll && rect.y < scroll + height {
            if let Action::Item(index) = action {
                return Some(*index);
            }
        }
        None
    });
    if last {
        visible.last()
    } else {
        visible.into_iter().next()
    }
}

fn help(term: &mut Terminal) -> AppResult<()> {
    info_page(term, "Atomic editor help", &[
        "The editor uses weighted physical slots and the same report as `akler atomic`. Tab cycles bigrams, trigrams, and skip1. A blank query selects all patterns.".into(),
        "Click two keys or drag one onto another to swap. Arrows move the physical cursor; Space selects and swaps. j/k move through table rows; Enter/l opens a selected group, and h/Esc returns. Click a group row to open it. u undoes the latest swap. Space stays fixed.".into(),
        "g opens a grouping checklist. Up/Down moves; Space or click toggles fields; Enter adds the highlighted field and applies all selected fields; c clears grouping immediately; q cancels. Grouping does not change the population denominator.".into(),
        "s saves a new DAT and JSONC layout copy without overwriting; x exports the grouped summary and all matching detail rows to a separate new text file. Page Up/Down and the wheel scroll results.".into(),
        "length = 2 and roll.direction = inward and row.direction = descending".into(),
        "length = 3 and redirect = true and endpoints.same_key = false".into(),
        "length = 2 and gap[0] = 1 and endpoints.same_finger = true".into(),
        "Triple-only fields on pairs return false, including !=. An unavailable geometry attribute returns an error. Invalid queries keep the previous valid results visible.".into(),
        "Press / to edit the query, g to edit grouping, Tab to switch population, and q to return. A failed swap keeps the current layout and report.".into(),
    ])
}

fn group_choices() -> Vec<String> {
    let mut fields = vec![
        "row.direction",
        "start.finger_type",
        "end.finger_type",
        "start.row",
        "end.row",
        "roll.direction",
        "first.direction",
        "last.direction",
        "endpoints.direction",
        "hand.pattern",
        "contains.thumb",
        "row.transitions",
        "row.total_steps",
        "row.net_delta",
        "length",
        "redirect",
        "hand.same",
        "hand.alternating",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    for place in ["start", "end", "position[0]", "position[1]", "position[2]"] {
        for property in [
            "key",
            "finger",
            "finger_type",
            "hand",
            "row",
            "column",
            "original",
        ] {
            let field = format!("{place}.{property}");
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    for place in ["first", "last", "endpoints"] {
        for property in [
            "same_key",
            "same_finger",
            "direction",
            "scissor",
            "lateral_stretch",
            "diagonal_stretch",
        ] {
            let field = format!("{place}.{property}");
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    fields.extend(["gap[0]", "gap[1]", "row.delta[0]", "row.delta[1]"].map(str::to_owned));
    fields
}

fn chosen_group_text(choices: &[String], selected: &[String]) -> String {
    choices
        .iter()
        .filter(|field| selected.contains(field))
        .chain(selected.iter().filter(|field| !choices.contains(field)))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn applied_group_text(choices: &[String], selected: &[String], cursor: usize) -> String {
    let mut applied = selected.to_vec();
    if !applied.contains(&choices[cursor]) {
        applied.push(choices[cursor].clone());
    }
    chosen_group_text(choices, &applied)
}

fn toggle_group_field(selected: &mut Vec<String>, field: &str) {
    if let Some(index) = selected.iter().position(|item| item == field) {
        selected.remove(index);
    } else {
        selected.push(field.to_owned());
    }
}

/// A draft checklist. Cancel leaves the active grouping untouched.
fn choose_grouping(
    term: &mut Terminal,
    current: Option<&atomic_report::Grouping>,
) -> AppResult<Option<String>> {
    let choices = group_choices();
    let mut selected = current
        .map(|grouping| {
            grouping
                .text()
                .split(',')
                .map(str::trim)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(Vec::new);
    let mut cursor = 0usize;
    let mut scroll = 0usize;
    let mut status = String::new();
    loop {
        let mut canvas = Canvas::new(term.width(), choices.len() + 5);
        canvas.text(0, 0, "Atomic grouping", FG);
        canvas.text(
            0,
            1,
            "Space/click toggle   Enter add + apply   c clear   q cancel",
            MUTED,
        );
        canvas.text(
            0,
            2,
            &short(
                &format!(
                    "Selected ({}): {}",
                    selected.len(),
                    chosen_group_text(&choices, &selected)
                ),
                canvas.w,
            ),
            BLUE,
        );
        for (index, field) in choices.iter().enumerate() {
            let y = index + 3;
            let mark = if selected.contains(field) { 'x' } else { ' ' };
            let label = format!(
                "{} [{mark}] {field}",
                if index == cursor { '›' } else { ' ' }
            );
            canvas.text(
                1,
                y,
                &short(&label, canvas.w.saturating_sub(1)),
                if index == cursor { YELLOW } else { FG },
            );
            canvas.hit(
                Rect {
                    x: 0,
                    y,
                    w: canvas.w,
                    h: 1,
                },
                Action::Item(index),
            );
        }
        canvas.text(0, choices.len() + 3, &short(&status, canvas.w), CYAN);
        term.present(&canvas, scroll)?;
        let event = term.event()?;
        let previous_scroll = scroll;
        if scroll_event(&event, &mut scroll, canvas.h, term.size.1) {
            if let Some(index) =
                visible_item_cursor(&canvas, scroll, term.size.1, scroll < previous_scroll)
            {
                cursor = index;
            }
            continue;
        }
        if let Some(Action::Item(index)) = action_press(term, &canvas, &event, scroll) {
            cursor = index;
            toggle_group_field(&mut selected, &choices[index]);
        } else {
            match event {
                Event::Escape | Event::Quit | Event::Char('q') => return Ok(None),
                Event::Enter => return Ok(Some(applied_group_text(&choices, &selected, cursor))),
                Event::Up | Event::Char('k') => cursor = cursor.saturating_sub(1),
                Event::Down | Event::Char('j') => cursor = (cursor + 1).min(choices.len() - 1),
                Event::Char(' ') => toggle_group_field(&mut selected, &choices[cursor]),
                Event::Char('c') => return Ok(Some(String::new())),
                Event::Char('/') => {
                    if let Some(field) =
                        input_box(term, "Add grouping field", "advanced field name", "")?
                    {
                        let field = field.trim();
                        if !field.is_empty() {
                            match atomic_report::Grouping::parse(field) {
                                Ok(_) => {
                                    if !selected.iter().any(|item| item == field) {
                                        selected.push(field.to_owned());
                                    }
                                    status.clear();
                                }
                                Err(error) => status = error.to_string(),
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let y = cursor + 3;
        if y < scroll {
            scroll = y;
        } else if y >= scroll + term.size.1.max(1) {
            scroll = y + 1 - term.size.1.max(1);
        }
    }
}

fn open_selected_group(term: &Terminal, view: &mut AtomicView) {
    if let Err(error) = view.enter_group() {
        view.status = Some(format!("Group failed: {error}"));
    } else if view.detail.is_some() {
        let canvas = view.render(term.width());
        view.move_table_cursor(0, &canvas, term.size.1);
    }
}

fn leave_selected_group(term: &Terminal, view: &mut AtomicView) -> bool {
    if !view.leave_group() {
        return false;
    }
    let canvas = view.render(term.width());
    view.move_table_cursor(0, &canvas, term.size.1);
    true
}

fn display(term: &mut Terminal, mut view: AtomicView) -> AppResult<()> {
    loop {
        let canvas = view.render(term.width());
        view.clamp_scroll(canvas.h, term.size.1);
        term.present(&canvas, view.scroll)?;
        let event = term.event()?;
        if view.scroll_table(&event, &canvas, term.size.1) {
            continue;
        }
        if let Some(Action::Item(index)) = action_press(term, &canvas, &event, view.scroll) {
            view.table_cursor = index;
            if view.grouping.is_some() && view.detail.is_none() {
                open_selected_group(term, &mut view);
            }
            continue;
        }
        match event {
            Event::Char('j') => {
                view.move_table_cursor(1, &canvas, term.size.1);
                continue;
            }
            Event::Char('k') => {
                view.move_table_cursor(-1, &canvas, term.size.1);
                continue;
            }
            Event::Char('h') => {
                leave_selected_group(term, &mut view);
                continue;
            }
            Event::Char('l') => {
                open_selected_group(term, &mut view);
                continue;
            }
            Event::Escape if leave_selected_group(term, &mut view) => continue,
            _ => {}
        }
        if matches!(event, Event::Up | Event::Down | Event::Left | Event::Right) {
            let (dr, dc) = direction(&event).expect("arrow has a direction");
            view.cursor = move_cursor(view.cursor, dr, dc, &view.physical_keys());
            view.keyboard_mode = true;
            continue;
        }
        let mut swap = None;
        match event {
            Event::Escape if view.selected_key.is_some() || view.drag.is_some() => {
                view.selected_key = None;
                view.drag = None;
            }
            Event::Escape | Event::Quit | Event::Char('q') => return Ok(()),
            Event::Enter if view.grouping.is_some() && view.detail.is_none() => {
                open_selected_group(term, &mut view);
            }
            Event::Char(' ') => {
                view.keyboard_mode = true;
                if let Some(first) = view.selected_key.take() {
                    if first != view.cursor {
                        swap = Some((first, view.cursor));
                    }
                } else {
                    view.selected_key = Some(view.cursor);
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: false,
            } => {
                view.keyboard_mode = false;
                if let Some(Action::Key(slot)) = term.hit(&canvas, x, y, view.scroll) {
                    view.drag = Some(slot);
                    view.cursor = slot;
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: false,
                motion: true,
            } => {
                if view.drag.is_some() {
                    if let Some(Action::Key(slot)) = term.hit(&canvas, x, y, view.scroll) {
                        view.cursor = slot;
                    }
                }
            }
            Event::Mouse {
                x,
                y,
                button: 0,
                release: true,
                ..
            } => {
                if let Some(first) = view.drag.take() {
                    if let Some(Action::Key(slot)) = term.hit(&canvas, x, y, view.scroll) {
                        view.cursor = slot;
                        if first != slot {
                            swap = Some((first, slot));
                            view.selected_key = None;
                        } else if let Some(selected) = view.selected_key.take() {
                            if selected != slot {
                                swap = Some((selected, slot));
                            }
                        } else {
                            view.selected_key = Some(slot);
                        }
                    }
                }
            }
            Event::Home => view.scroll = 0,
            Event::End => view.scroll = canvas.h.saturating_sub(term.size.1),
            Event::Char('u') => {
                if let Err(error) = view.undo() {
                    view.status = Some(format!("Undo failed: {error}"));
                }
            }
            Event::Char('s') => view.save_copy(),
            Event::Tab => view.cycle_population(),
            Event::Char('g') => {
                if let Some(text) = choose_grouping(term, view.grouping.as_ref())? {
                    view.submit_grouping(&text);
                }
            }
            Event::Char('/') => {
                if let Some(text) = input_box(
                    term,
                    "Atomic query",
                    "",
                    view.query.as_deref().unwrap_or(""),
                )? {
                    view.submit_query(&text);
                }
            }
            Event::Char('?') => help(term)?,
            Event::Char('x') => {
                if let Some(path) = input_box(term, "Export atomic report to a new file", "", "")? {
                    view.export(Path::new(&path));
                }
            }
            _ => {}
        }
        if let Some((a, b)) = swap {
            if let Err(error) = view.swap(a, b) {
                view.status = Some(format!("Swap failed: {error}"));
            }
        }
    }
}

pub(crate) fn open(term: &mut Terminal) -> AppResult<()> {
    let Some(mut corpus_path) = select_source_path(term, true)? else {
        return Ok(());
    };
    let mut session = crate::session::CorpusSession::new();
    loop {
        if term.quitting {
            return Ok(());
        }
        let paths = discover_layouts(Path::new(LAYOUT_DIR))?;
        let mut names: Vec<_> = paths.iter().map(|path| layout_file_label(path)).collect();
        names.push("Change corpus...".into());
        names.push("Reload corpus snapshot".into());
        let Some(choice) = menu(
            term,
            &format!("Atomic layouts — {}", corpus_name(&corpus_path)),
            &names,
        )?
        else {
            return Ok(());
        };
        if choice == paths.len() {
            if let Some(path) = select_source_path(term, false)? {
                corpus_path = path;
                session.invalidate();
            }
            continue;
        }
        if choice == paths.len() + 1 {
            session.invalidate();
            continue;
        }
        let path = &paths[choice];
        let result = (|| -> AppResult<()> {
            let text = fs::read_to_string(path)?;
            let layout = parse_open_layout(&text, path)?;
            let view = match layout {
                OpenLayout::Plain(board) => {
                    let Some(source) = session.plain_tui(term, &corpus_path)? else {
                        return Ok(());
                    };
                    let model = Model::new(board.clone());
                    let drawing = Drawing::Plain {
                        slots: model.original.clone(),
                        model,
                    };
                    AtomicView::new(
                        atomic_report::prepare_ordinary(board, (*source).clone())?,
                        drawing,
                        CorpusData::Plain(source),
                    )?
                }
                OpenLayout::Action(layout) => {
                    let Some(corpus) = session.action_tui(term, &corpus_path)? else {
                        return Ok(());
                    };
                    let drawing = Drawing::Action {
                        current: layout.clone(),
                        original: layout.clone(),
                    };
                    AtomicView::new(
                        atomic_report::prepare_action(layout, corpus.clone())?,
                        drawing,
                        CorpusData::Action(corpus),
                    )?
                }
            };
            display(term, view)
        })();
        if let Err(error) = result {
            if !term.quitting {
                info_page(term, "Atomic editor error", &[error.to_string()])?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_ngrams::NgramCorpus;

    const ROWS: &str =
        "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
    const CORPUS: &str = r#"{"letters":{"q":4,"s":2,"w":2},"bigrams":{"qs":1.5,"sw":0.5,"wq":1.0},"trigrams":{"qsw":0.5,"swq":0.25}}"#;

    #[test]
    fn grouping_checklist_offers_valid_fields_and_builds_a_deterministic_selection() {
        let choices = group_choices();
        assert_eq!(
            &choices[..5],
            [
                "row.direction",
                "start.finger_type",
                "end.finger_type",
                "start.row",
                "end.row"
            ]
        );
        let mut unique = std::collections::BTreeSet::new();
        for field in &choices {
            assert!(unique.insert(field));
            atomic_report::Grouping::parse(field).unwrap();
        }
        let mut selected = Vec::new();
        toggle_group_field(&mut selected, "end.finger_type");
        toggle_group_field(&mut selected, "row.direction");
        toggle_group_field(&mut selected, "start.finger_type");
        assert_eq!(
            chosen_group_text(&choices, &selected),
            "row.direction, start.finger_type, end.finger_type"
        );
        toggle_group_field(&mut selected, "row.direction");
        assert_eq!(
            chosen_group_text(&choices, &selected),
            "start.finger_type, end.finger_type"
        );
        selected.clear();
        assert!(chosen_group_text(&choices, &selected).is_empty());
        assert_eq!(applied_group_text(&choices, &selected, 3), "start.row");
        selected.push("row.direction".into());
        assert_eq!(
            applied_group_text(&choices, &selected, 3),
            "row.direction, start.row"
        );
        assert_eq!(applied_group_text(&choices, &selected, 0), "row.direction");
    }

    fn plain_view() -> AtomicView {
        let board = board_from_text(ROWS, Path::new("inline.dat")).unwrap();
        let source = Source::from_text(CORPUS, Path::new("inline.json")).unwrap();
        let model = Model::new(board.clone());
        let drawing = Drawing::Plain {
            slots: model.original.clone(),
            model,
        };
        AtomicView::new(
            atomic_report::prepare_ordinary(board, source.clone()).unwrap(),
            drawing,
            CorpusData::Plain(Arc::new(source)),
        )
        .unwrap()
    }

    #[test]
    fn grouped_table_opens_exact_members_and_reverts_to_current_results() {
        let mut view = plain_view();
        view.submit_grouping("start.row");
        let groups =
            atomic_report::group_buckets(&view.report, view.grouping.as_ref().unwrap()).unwrap();
        assert_eq!(groups.len(), view.row_lines.len());
        assert_eq!(groups[0].row_indices.len(), 2);
        let group_frequency: f64 = groups[0]
            .row_indices
            .iter()
            .map(|&index| view.report.rows[index].frequency)
            .sum();
        assert_eq!(group_frequency, groups[0].frequency);
        let canvas = view.render(80);
        assert_eq!(
            canvas
                .hits
                .iter()
                .filter(|(_, action)| matches!(action, Action::Item(_)))
                .count(),
            groups.len()
        );
        view.move_table_cursor(1, &canvas, 12);
        assert_eq!(view.table_cursor, 1);
        view.move_table_cursor(100, &canvas, 12);
        assert_eq!(view.table_cursor, groups.len() - 1);
        view.table_cursor = 0;
        view.enter_group().unwrap();
        assert_eq!(view.detail.as_ref().unwrap().rows.len(), 2);
        assert!(view.detail.as_ref().unwrap().title.contains("83.33%"));
        assert_eq!(view.report.rows.len(), 3);
        assert_eq!(view.report.population_frequency, 3.0);
        assert!(view.leave_group());
        assert!(view.detail.is_none());
        assert!(!view.leave_group());

        view.table_cursor = 1;
        view.enter_group().unwrap();
        assert_eq!(view.detail.as_ref().unwrap().rows.len(), 1);
        assert!(view.leave_group());
        assert_eq!(view.table_cursor, 1);

        view.table_cursor = 0;
        view.enter_group().unwrap();
        view.swap(0, 1).unwrap();
        assert!(view.detail.is_none());
        assert_eq!(view.grouping_text(), "start.row");
        view.undo().unwrap();
        assert!(view.detail.is_none());
        assert_eq!(view.grouping_text(), "start.row");
        assert_eq!(view.report.population_frequency, 3.0);
        view.submit_query("start.key = 99");
        assert!(view.row_lines.is_empty());
        view.enter_group().unwrap();
        assert!(view.detail.is_none());
    }

    #[test]
    fn table_cursor_follows_page_scroll_and_can_move_up_again() {
        let mut view = plain_view();
        view.row_lines = (0..50).map(|index| format!("row {index}")).collect();
        let canvas = view.render(80);
        for _ in 0..3 {
            assert!(view.scroll_table(&Event::PageDown, &canvas, 12));
        }
        assert!(view.scroll > 0);
        assert!(view.table_cursor > 0);
        let previous_cursor = view.table_cursor;
        view.move_table_cursor(-1, &canvas, 12);
        assert_eq!(view.table_cursor, previous_cursor - 1);
        let previous_scroll = view.scroll;
        assert!(view.scroll_table(&Event::PageUp, &canvas, 12));
        assert!(view.scroll < previous_scroll);
        let selected_y = canvas
            .hits
            .iter()
            .find_map(|(rect, action)| {
                matches!(action, Action::Item(index) if *index == view.table_cursor)
                    .then_some(rect.y)
            })
            .unwrap();
        assert!(selected_y >= view.scroll && selected_y < view.scroll + 12);
        let previous_scroll = view.scroll;
        assert!(view.scroll_table(&Event::Wheel(-3), &canvas, 12));
        assert!(view.scroll < previous_scroll);
        view.scroll = canvas.h.saturating_sub(12);
        view.table_cursor = 0;
        view.move_table_cursor(-1, &canvas, 12);
        assert!(view.table_cursor > 0);
    }

    fn magic_view() -> AtomicView {
        let rows = ROWS.replacen("w e", "@magic e", 1);
        let layout = action_keys::Layout::parse(
            &format!("{rows}action magic = magic\nfallback magic = repeat-output\n"),
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = NgramCorpus::from_text(
            r#"{"letters":{"a":3},"bigrams":{"aa":2},"trigrams":{"aaa":1}}"#,
            Path::new("inline.json"),
        )
        .unwrap();
        let drawing = Drawing::Action {
            current: layout.clone(),
            original: layout.clone(),
        };
        AtomicView::new(
            atomic_report::prepare_action(layout, corpus.clone()).unwrap(),
            drawing,
            CorpusData::Action(corpus),
        )
        .unwrap()
    }

    #[test]
    fn ordinary_query_population_and_errors_keep_valid_report() {
        let mut view = plain_view();
        let total = view.report.population_frequency;
        assert_eq!(view.report.population, Population::Bigrams);
        view.submit_query("start.finger = left_pinky");
        assert!(view.report.matching_frequency < total);
        assert_eq!(view.report.population_frequency, total);
        let prior = atomic_report::render_text(&view.report);
        view.submit_query("start.finger =");
        assert!(view.error.is_some());
        assert_eq!(atomic_report::render_text(&view.report), prior);
        assert!(view.render(80).cells.iter().any(|cell| cell.ch == 'E'));
        view.submit_query(" ");
        assert!(view.error.is_none());
        assert_eq!(view.report.matching_frequency, total);
        view.scroll = 99;
        view.submit_grouping(
            "row.direction, start.finger_type, end.finger_type, start.row, end.row",
        );
        assert_eq!(
            view.grouping_text(),
            "row.direction, start.finger_type, end.finger_type, start.row, end.row"
        );
        assert_eq!(view.scroll, 0);
        assert!(view.table_header.contains("row.direction"));
        assert!(view
            .row_lines
            .iter()
            .any(|line| line.contains("descending")));
        assert!(view.row_lines.iter().any(|line| line.contains("pinky")));
        let prior_group = (view.table_header.clone(), view.row_lines.clone());
        view.submit_grouping("row.direction, unknown");
        assert!(view.error.is_some());
        assert_eq!(
            (view.table_header.clone(), view.row_lines.clone()),
            prior_group
        );
        assert_eq!(
            view.grouping_text(),
            "row.direction, start.finger_type, end.finger_type, start.row, end.row"
        );
        view.cycle_population();
        assert_eq!(view.report.population, Population::Trigrams);
        assert_eq!(view.report.population_frequency, 0.75);
        assert!(view.grouping.is_some());
        view.cycle_population();
        assert_eq!(view.report.population, Population::Skip1);
        assert_eq!(view.report.population_frequency, 0.75);
        view.submit_query("position[2].row = home");
        assert_eq!(view.report.matching_frequency, 0.0);
        view.submit_query("first.unknown = true");
        assert!(view.error.is_some());
        assert_eq!(view.report.matching_frequency, 0.0);
        view.submit_grouping(" ");
        assert!(view.grouping.is_none());
        assert_eq!(
            (view.table_header.clone(), view.row_lines.clone()),
            atomic_report::table_lines(&view.report)
        );
    }

    #[test]
    fn unsupported_geometry_keeps_previous_valid_results() {
        let board = board_from_text(ROWS, Path::new("inline.dat")).unwrap();
        let source = Source::from_text(CORPUS, Path::new("inline.json")).unwrap();
        let model = Model::new(board.clone());
        let drawing = Drawing::Plain {
            slots: model.original.clone(),
            model,
        };
        let mut prepared = atomic_report::prepare_ordinary(board, source.clone()).unwrap();
        prepared.clear_geometry_for_test();
        let mut view =
            AtomicView::new(prepared, drawing, CorpusData::Plain(Arc::new(source))).unwrap();
        let previous = atomic_report::render_text(&view.report);
        view.submit_query("first.scissor = false");
        assert!(view
            .error
            .as_deref()
            .unwrap()
            .contains("UnsupportedAttribute"));
        assert_eq!(atomic_report::render_text(&view.report), previous);
    }

    #[test]
    fn viewer_report_matches_cli_report_for_both_engines() {
        let ordinary = plain_view();
        let board = board_from_text(ROWS, Path::new("inline.dat")).unwrap();
        let source = Source::from_text(CORPUS, Path::new("inline.json")).unwrap();
        let cli = atomic_report::ordinary_counts(board, source, Population::Bigrams, None).unwrap();
        assert_eq!(
            atomic_report::render_text(&ordinary.report),
            atomic_report::render_text(&cli)
        );

        let mut magic = magic_view();
        assert!(matches!(magic.drawing, Drawing::Action { .. }));
        assert!(atomic_report::render_text(&magic.report).contains("@magic"));
        magic.cycle_population();
        magic.submit_query("endpoints.same_key = true");
        assert_eq!(magic.report.population, Population::Trigrams);
        assert_eq!(magic.report.query, "endpoints.same_key = true");
        let rows = ROWS.replacen("w e", "@magic e", 1);
        let layout = action_keys::Layout::parse(
            &format!("{rows}action magic = magic\nfallback magic = repeat-output\n"),
            Path::new("magic.dat"),
        )
        .unwrap();
        let corpus = NgramCorpus::from_text(
            r#"{"letters":{"a":3},"bigrams":{"aa":2},"trigrams":{"aaa":1}}"#,
            Path::new("inline.json"),
        )
        .unwrap();
        let cli = atomic_report::action_counts(
            layout,
            corpus,
            Population::Trigrams,
            Some("endpoints.same_key = true"),
        )
        .unwrap();
        assert_eq!(
            atomic_report::render_text(&magic.report),
            atomic_report::render_text(&cli)
        );
    }

    #[test]
    fn replacing_layout_or_corpus_creates_fresh_view_state() {
        let mut old = plain_view();
        old.submit_query("start.key = 0");
        old.scroll = 9;
        let board = board_from_text(ROWS, Path::new("replacement.dat")).unwrap();
        let source = Source::from_text(
            r#"{"letters":{"q":1,"s":1},"bigrams":{"qs":1},"trigrams":{}}"#,
            Path::new("new.json"),
        )
        .unwrap();
        let model = Model::new(board.clone());
        let drawing = Drawing::Plain {
            slots: model.original.clone(),
            model,
        };
        let next = AtomicView::new(
            atomic_report::prepare_ordinary(board, source.clone()).unwrap(),
            drawing,
            CorpusData::Plain(Arc::new(source)),
        )
        .unwrap();
        assert_eq!(next.scroll, 0);
        assert!(next.query.is_none());
        assert_eq!(next.report.matching_frequency, 1.0);
        assert_ne!(old.report.corpus, next.report.corpus);

        let changed = ROWS.replacen("q w", "w q", 1);
        let board = board_from_text(&changed, Path::new("changed.dat")).unwrap();
        let source = Source::from_text(CORPUS, Path::new("inline.json")).unwrap();
        let model = Model::new(board.clone());
        let drawing = Drawing::Plain {
            slots: model.original.clone(),
            model,
        };
        let changed = AtomicView::new(
            atomic_report::prepare_ordinary(board, source.clone()).unwrap(),
            drawing,
            CorpusData::Plain(Arc::new(source)),
        )
        .unwrap();
        assert_ne!(changed.report.keys[0].label, old.report.keys[0].label);
        assert_eq!(changed.report.keys[0].label.as_deref(), Some("w"));
        assert!(changed.undo.is_empty());
    }

    #[test]
    fn export_uses_all_rows_and_never_uses_screen_truncation() {
        let mut view = plain_view();
        view.submit_grouping("row.direction, start.finger_type, end.finger_type");
        let full =
            atomic_report::render_grouped_text(&view.report, view.grouping.as_ref().unwrap())
                .unwrap();
        assert!(full.contains("Grouped by: row.direction, start.finger_type, end.finger_type"));
        let (_, detail_rows) = atomic_report::table_lines(&view.report);
        assert!(detail_rows.iter().all(|line| full.contains(line)));
        assert!(!full.contains('\u{1b}'));
        let path = std::env::temp_dir().join(format!(
            "akler-atomic-ui-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        view.export(&path);
        assert_eq!(fs::read_to_string(&path).unwrap(), full);
        view.export(&path);
        assert!(view.status.as_deref().unwrap().starts_with("Export failed"));
        fs::remove_file(path).unwrap();
        view.scroll = 1000;
        let canvas = view.render(64);
        assert!(canvas.h > 0);
        view.clamp_scroll(canvas.h, 12);
        assert_eq!(view.scroll, canvas.h.saturating_sub(12));
        view.scroll_by(-1000, canvas.h, 12);
        assert_eq!(view.scroll, 0);
        view.scroll_by(1000, canvas.h, 12);
        assert_eq!(view.scroll, canvas.h.saturating_sub(12));
        assert!(view.row_lines.iter().all(|line| full.contains(line)));
    }

    #[test]
    fn ordinary_swap_preserves_geometry_query_and_exact_undo_report() {
        let mut view = plain_view();
        view.cycle_population();
        view.cycle_population();
        view.submit_query("start.key = 0");
        view.submit_grouping("row.direction, start.row, end.row");
        let before = atomic_report::render_text(&view.report);
        let before_grouped = (view.table_header.clone(), view.row_lines.clone());
        let keys = view.report.keys.clone();
        view.scroll = 100;
        view.swap(0, 1).unwrap();
        assert!(view.changed());
        assert_eq!(view.selected, Population::Skip1);
        assert_eq!(view.query.as_deref(), Some("start.key = 0"));
        assert_eq!(view.grouping_text(), "row.direction, start.row, end.row");
        assert_eq!(view.scroll, 0);
        assert_eq!(view.report.keys[0].label.as_deref(), Some("w"));
        assert_eq!(view.report.keys[1].label.as_deref(), Some("q"));
        assert_eq!(view.report.keys[0].x, keys[0].x);
        assert_eq!(view.report.keys[0].finger, keys[0].finger);
        assert_ne!(atomic_report::render_text(&view.report), before);
        let Drawing::Plain { model, slots } = &view.drawing else {
            panic!("plain layout expected")
        };
        let mut board = model.board.clone();
        board.symbols = model.symbols(slots);
        let source = Source::from_text(CORPUS, Path::new("inline.json")).unwrap();
        let direct =
            atomic_report::ordinary_counts(board, source, Population::Skip1, Some("start.key = 0"))
                .unwrap();
        assert_eq!(
            atomic_report::render_text(&view.report),
            atomic_report::render_text(&direct)
        );
        view.undo().unwrap();
        assert!(!view.changed());
        assert_eq!(atomic_report::render_text(&view.report), before);
        assert_eq!(
            (view.table_header.clone(), view.row_lines.clone()),
            before_grouped
        );
        assert_eq!(view.selected, Population::Skip1);
        assert_eq!(view.query.as_deref(), Some("start.key = 0"));
        assert_eq!(view.grouping_text(), "row.direction, start.row, end.row");
    }

    #[test]
    fn action_swap_moves_winning_root_slot_and_undo_restores_report() {
        let mut view = magic_view();
        view.submit_grouping("start.finger_type, end.finger_type");
        let before = atomic_report::render_text(&view.report);
        let action = view
            .report
            .keys
            .iter()
            .position(|key| key.label.as_deref() == Some("@magic"))
            .unwrap();
        assert!(view
            .report
            .rows
            .iter()
            .any(|row| row.slots.contains(&(action as u32))));
        let target = 9;
        let original_geometry = view.report.keys[target].clone();
        view.swap(action, target).unwrap();
        assert_eq!(view.report.keys[target].label.as_deref(), Some("@magic"));
        assert_eq!(view.grouping_text(), "start.finger_type, end.finger_type");
        assert_eq!(view.report.keys[target].finger, original_geometry.finger);
        assert_eq!(view.report.keys[target].x, original_geometry.x);
        assert!(view
            .report
            .rows
            .iter()
            .any(|row| row.slots.contains(&(target as u32))));
        assert_ne!(atomic_report::render_text(&view.report), before);
        view.undo().unwrap();
        assert_eq!(atomic_report::render_text(&view.report), before);

        let rows = ROWS.replacen("w e", "@magic e", 1);
        let mut layout = action_keys::Layout::parse(
            &format!("{rows}action magic = magic\nfallback magic = repeat-output\n"),
            Path::new("duplicate.dat"),
        )
        .unwrap();
        layout.slots[0].label = "@magic".into();
        let corpus = NgramCorpus::from_text(
            r#"{"letters":{"a":3},"bigrams":{"aa":2},"trigrams":{"aaa":1}}"#,
            Path::new("duplicate.json"),
        )
        .unwrap();
        let drawing = Drawing::Action {
            current: layout.clone(),
            original: layout.clone(),
        };
        let mut view = AtomicView::new(
            atomic_report::prepare_action(layout, corpus.clone()).unwrap(),
            drawing,
            CorpusData::Action(corpus),
        )
        .unwrap();
        assert_eq!(view.report.keys[0].label, view.report.keys[1].label);
        view.swap(1, 9).unwrap();
        let text = atomic_report::render_text(&view.report);
        assert!(text.contains("@magic[#9]"));
        assert_eq!(view.report.keys[0].label, view.report.keys[9].label);
    }

    #[test]
    fn failed_space_swap_keeps_layout_and_report_together() {
        let mut ordinary = plain_view();
        let prior = atomic_report::render_text(&ordinary.report);
        let Drawing::Plain { model, slots } = &ordinary.drawing else {
            unreachable!()
        };
        let space = (0..slots.len())
            .find(|&slot| model.canonical[slots[slot]] == b' ')
            .unwrap();
        assert!(ordinary.swap(0, space).is_err());
        assert_eq!(atomic_report::render_text(&ordinary.report), prior);
        assert!(ordinary.undo.is_empty());

        let mut magic = magic_view();
        let prior = atomic_report::render_text(&magic.report);
        let Drawing::Action { current, .. } = &magic.drawing else {
            panic!("action layout expected")
        };
        let space = (0..current.slots.len())
            .find(|&slot| current.space(slot))
            .unwrap();
        assert!(magic.swap(0, space).is_err());
        assert_eq!(atomic_report::render_text(&magic.report), prior);
    }

    #[test]
    fn saved_action_copy_reloads_with_current_binding() {
        let token = format!(
            "atomic-save-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut magic = magic_view();
        let action = magic
            .report
            .keys
            .iter()
            .position(|key| key.label.as_deref() == Some("@magic"))
            .unwrap();
        magic.swap(action, 9).unwrap();
        let Drawing::Action { current, .. } = &magic.drawing else {
            unreachable!()
        };
        let mut current = current.clone();
        current.path = std::env::temp_dir().join(format!("{token}-magic.dat"));
        current.name = format!("{token}-magic");
        current.slots[9].label = "@".into();
        let path = current.save_new().unwrap();
        let reloaded = parse_open_layout(&fs::read_to_string(&path).unwrap(), &path).unwrap();
        let OpenLayout::Action(reloaded) = reloaded else {
            panic!("action save changed layout type")
        };
        assert_eq!(reloaded.slots[9].label, "@");
        assert_eq!(reloaded.slots[9].finger, current.slots[9].finger);
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("jsonc")).unwrap();
    }

    #[test]
    fn editor_save_copy_uses_paired_new_files_without_overwrite() {
        let token = format!(
            "atomic-editor-copy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut view = plain_view();
        view.swap(0, 1).unwrap();
        let Drawing::Plain { model, slots } = &mut view.drawing else {
            unreachable!()
        };
        model.board.path = std::env::temp_dir().join(format!("{token}.dat"));
        let expected_symbols = model.symbols(slots);
        let expected_keys = model.board.keys.clone();
        let path = std::env::temp_dir().join(format!("{token}-optimized-001.dat"));
        view.save_copy();
        assert!(view.status.as_deref().unwrap().starts_with("saved "));
        assert!(path.is_file());
        assert!(path.with_extension("jsonc").is_file());
        let loaded = parse_open_layout(&fs::read_to_string(&path).unwrap(), &path).unwrap();
        let OpenLayout::Plain(board) = loaded else {
            panic!("copy changed layout type")
        };
        assert_eq!(board.symbols, expected_symbols);
        assert_eq!(board.keys, expected_keys);
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("jsonc")).unwrap();
    }
}
