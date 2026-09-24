//! Numeric implementation of WindowMapper's existing bounded-context policy.
//! Compiled once per search. Successful mapping builds no diagnostics and allocates no heap storage.
use crate::action_keys::{self as ak, Action, Basis, Binding, Emission, Layout};

use std::collections::{BTreeMap, BTreeSet};

use crate::action_profile::MappingCounts;

const MAX_KEYS: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    None,
    Byte(u8),
    RepeatOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emit {
    None,
    Byte(u8),
    Call(usize),
    Terminal { hops: u8, op: Terminal },
}

#[derive(Clone, Copy)]
enum Key {
    Empty,
    Byte(u8),
    Action(usize),
}

struct PackedRule {
    bits: u32,
    mask: u32,
    len: usize,
    emit: Emit,
}

struct SuffixIndex {
    short: Box<[Emit; 257]>,
    // final byte, plus empty-prefix entry
    ranges: Box<[(usize, usize); 256]>,
    longer: Vec<PackedRule>,
}

fn packed_suffix(text: &[u8]) -> u32 {
    let mut bits = 0;
    for (i, &byte) in text.iter().rev().take(4).enumerate() {
        bits |= u32::from(byte) << (8 * i);
    }
    bits
}

impl SuffixIndex {
    fn lookup<const PROFILE: bool>(&self, prefix: &[u8], ops: &mut MappingCounts) -> Emit {
        if PROFILE {
            ops.suffix_index_lookups += 1;
        }
        let Some(&last) = prefix.last() else {
            return self.short[256];
        };
        if prefix.len() > 1 {
            let (start, end) = self.ranges[last as usize];
            if start != end {
                let bits = packed_suffix(prefix);
                // Only rules ending in this byte; longest lengths come first.
                for rule in &self.longer[start..end] {
                    if rule.len > prefix.len() {
                        continue;
                    }
                    if PROFILE {
                        ops.suffix_checks += 1;
                    }
                    if bits & rule.mask == rule.bits {
                        return rule.emit;
                    }
                }
            }
        }
        self.short[last as usize]
    }
}

enum Code {
    Inactive,
    Byte(u8),
    RepeatOutput,
    RepeatAction,
    TextOne {
        table: Box<[Emit; 257]>,
    },
    TextRules(SuffixIndex),
    OutputRules {
        basis: Basis,
        table: Box<[Emit; 257]>,
    },
    PressRules {
        basis: Basis,
        table: Box<[Emit]>,
    },
}

pub(crate) struct Program {
    keys: Vec<Key>,
    actions: Vec<Code>,
}

// Prove an unconditional chain reaches a non-recursive terminal. Branching
// rules and RepeatAction remain dynamic. Cycles/long chains retain Call.
fn terminal_call<'a>(
    mut name: &'a str,
    defs: &'a BTreeMap<String, Action>,
) -> Option<(u8, Terminal)> {
    let mut seen = Vec::new();
    loop {
        if seen.len() >= 32 || seen.contains(&name) {
            return None;
        }
        seen.push(name);
        let hops = seen.len() as u8;
        match &defs[name] {
            Action::Inactive => return Some((hops, Terminal::None)),
            Action::Text(v) => return Some((hops, Terminal::Byte(v[0]))),
            Action::RepeatOutput => return Some((hops, Terminal::RepeatOutput)),
            Action::Rules {
                rules, fallback, ..
            } if rules.is_empty() => match fallback {
                Emission::None => return Some((hops, Terminal::None)),
                Emission::Text(v) => return Some((hops, Terminal::Byte(v[0]))),
                Emission::Call(next) => name = next,
            },
            _ => return None,
        }
    }
}

fn emission(e: &Emission, ids: &BTreeMap<String, usize>, defs: &BTreeMap<String, Action>) -> Emit {
    match e {
        Emission::None => Emit::None,
        Emission::Text(v) => Emit::Byte(v[0]),
        Emission::Call(n) => match terminal_call(n, defs) {
            Some((hops, op)) => Emit::Terminal { hops, op },
            None => Emit::Call(ids[n]),
        },
    }
}

fn text_rules(
    rules: &BTreeMap<Vec<u8>, Emission>,
    fallback: Emit,
    ids: &BTreeMap<String, usize>,
    defs: &BTreeMap<String, Action>,
) -> Code {
    // Empty rules are rejected by the file parser, but retain the original
    // mapper's behavior for programmatically built layouts too.
    let default = rules
        .get(b"".as_slice())
        .map_or(fallback, |e| emission(e, ids, defs));
    let mut short = Box::new([default; 257]);
    let mut longer = Vec::new();
    for (context, e) in rules {
        let emit = emission(e, ids, defs);
        match context.len() {
            0 => {}
            1 => short[context[0] as usize] = emit,
            len => {
                // Initial validation guarantees text rules are shorter than
                // the corpus order (at most 4 bytes for a 5-gram corpus).
                let mask = u32::MAX >> (8 * (4 - len));
                longer.push(PackedRule {
                    bits: packed_suffix(context),
                    mask,
                    len,
                    emit,
                });
            }
        }
    }
    if longer.is_empty() {
        return Code::TextOne { table: short };
    }
    longer.sort_by_key(|r| (r.bits & 255, std::cmp::Reverse(r.len)));
    let mut ranges = Box::new([(0, 0); 256]);
    for (i, rule) in longer.iter().enumerate() {
        let range = &mut ranges[(rule.bits & 255) as usize];
        if range.0 == range.1 {
            range.0 = i;
        }
        range.1 = i + 1;
    }
    Code::TextRules(SuffixIndex {
        short,
        ranges,
        longer,
    })
}

impl Program {
    pub(crate) fn new(layout: &Layout, order: usize) -> ak::Result<Self> {
        if layout.slots.len() > MAX_KEYS || !(1..=5).contains(&order) {
            return Err("numeric mapper capacity exceeded".into());
        }
        ak::WindowMapper::validate(layout, order)?;
        // Only reachable definitions were validated. Unused macro definitions
        // must not make a previously valid n-gram layout fail compilation.
        let mut names = BTreeSet::new();
        let mut pending = Vec::new();
        for slot in &layout.slots {
            if let Binding::Named(n) = &slot.binding {
                pending.push(n.clone());
            }
        }
        while let Some(n) = pending.pop() {
            if !names.insert(n.clone()) {
                continue;
            }
            if let Action::Rules {
                rules, fallback, ..
            } = &layout.actions[&n]
            {
                for e in rules.values().chain(std::iter::once(fallback)) {
                    if let Emission::Call(target) = e {
                        pending.push(target.clone());
                    }
                }
            }
        }
        let ids: BTreeMap<String, usize> = names
            .into_iter()
            .enumerate()
            .map(|(id, n)| (n, id))
            .collect();
        let mut actions = Vec::with_capacity(ids.len());
        // BTreeMap order is the same order used to assign IDs above.
        for name in ids.keys() {
            let code = match &layout.actions[name] {
                Action::Inactive => Code::Inactive,
                Action::Text(v) => Code::Byte(v[0]),
                Action::RepeatOutput => Code::RepeatOutput,
                Action::RepeatAction => Code::RepeatAction,
                Action::Rules {
                    basis,
                    rules,
                    fallback,
                } => {
                    let fallback = emission(fallback, &ids, &layout.actions);
                    match basis {
                        Basis::Text => text_rules(rules, fallback, &ids, &layout.actions),
                        Basis::Press | Basis::SkipPress => {
                            let mut table = Vec::with_capacity(layout.slots.len() + 1);
                            for context in layout
                                .slots
                                .iter()
                                .map(|s| match &s.binding {
                                    Binding::Named(n) => n.as_bytes(),
                                    _ => s.label.as_bytes(),
                                })
                                .chain(std::iter::once(b"".as_slice()))
                            {
                                let mut prefixed = Vec::with_capacity(context.len() + 1);
                                prefixed.push(b'@');
                                prefixed.extend_from_slice(context);
                                let selected = rules
                                    .get(context)
                                    .or_else(|| rules.get(prefixed.as_slice()));
                                table.push(
                                    selected
                                        .map_or(fallback, |e| emission(e, &ids, &layout.actions)),
                                );
                            }
                            Code::PressRules {
                                basis: *basis,
                                table: table.into_boxed_slice(),
                            }
                        }
                        _ => {
                            let mut table = Box::new([fallback; 257]);
                            if let Some(e) = rules.get(b"".as_slice()) {
                                table[256] = emission(e, &ids, &layout.actions);
                            }
                            for byte in 0..256 {
                                if let Some(e) = rules.get([byte as u8].as_slice()) {
                                    table[byte] = emission(e, &ids, &layout.actions);
                                }
                            }
                            Code::OutputRules {
                                basis: *basis,
                                table,
                            }
                        }
                    }
                }
            };
            actions.push(code);
        }
        let keys = layout
            .slots
            .iter()
            .map(|s| match &s.binding {
                Binding::Empty => Key::Empty,
                Binding::Text(v) => Key::Byte(v[0]),
                Binding::Named(n) => Key::Action(ids[n]),
            })
            .collect();
        Ok(Self { keys, actions })
    }

    pub(crate) fn affected_bytes(
        &self,
        state: &KeyState,
        a: usize,
        b: usize,
    ) -> Option<[Option<u8>; 2]> {
        let mut bytes = [None; 2];
        for (i, key) in [a, b].into_iter().enumerate() {
            match self.keys[state.ids[key]] {
                Key::Action(_) => return None,
                Key::Byte(byte) => bytes[i] = Some(byte),
                Key::Empty => {}
            }
        }
        Some(bytes)
    }

    fn resolve_key<const PROFILE: bool>(
        &self,
        key: usize,
        state: &KeyState,
        m: Memory,
        prefix: &[u8],
        stack: &mut [usize; 32],
        depth: usize,
        ops: &mut MappingCounts,
    ) -> Option<Output> {
        match self.keys[state.ids[key]] {
            Key::Empty => None,
            Key::Byte(byte) => Some(Output {
                byte,
                remember: true,
            }),
            Key::Action(id) => self.resolve::<PROFILE>(id, state, m, prefix, stack, depth, ops),
        }
    }

    fn emit<const PROFILE: bool>(
        &self,
        e: Emit,
        state: &KeyState,
        m: Memory,
        prefix: &[u8],
        stack: &mut [usize; 32],
        depth: usize,
        ops: &mut MappingCounts,
    ) -> Option<Output> {
        match e {
            Emit::None => None,
            Emit::Byte(byte) => Some(Output {
                byte,
                remember: true,
            }),
            Emit::Call(id) => self.resolve::<PROFILE>(id, state, m, prefix, stack, depth, ops),
            Emit::Terminal { hops, op } => {
                if PROFILE {
                    ops.terminal_shortcuts += 1;
                    match op {
                        Terminal::None => ops.terminal_none += 1,
                        Terminal::Byte(_) => ops.terminal_byte += 1,
                        Terminal::RepeatOutput => ops.terminal_repeat += 1,
                    }
                }
                // Each elided named action still consumes its original depth.
                // An acyclic unconditional terminal chain cannot be an active
                // ancestor: it never delegates back to a dynamic caller.
                if depth + usize::from(hops) > 32 {
                    if PROFILE {
                        ops.terminal_depth_rejected += 1;
                    }
                    return None;
                }
                match op {
                    Terminal::None => None,
                    Terminal::Byte(byte) => Some(Output {
                        byte,
                        remember: true,
                    }),
                    Terminal::RepeatOutput => m.remembered_output.map(|byte| Output {
                        byte,
                        remember: false,
                    }),
                }
            }
        }
    }

    fn resolve<const PROFILE: bool>(
        &self,
        id: usize,
        state: &KeyState,
        m: Memory,
        prefix: &[u8],
        stack: &mut [usize; 32],
        depth: usize,
        ops: &mut MappingCounts,
    ) -> Option<Output> {
        if PROFILE {
            ops.resolutions += 1;
            if depth > 0 {
                ops.recursive += 1;
            }
        }
        if depth >= 32 || stack[..depth].contains(&id) {
            return None;
        }
        stack[depth] = id;
        let next = depth + 1;
        match &self.actions[id] {
            Code::Inactive => None,
            Code::Byte(byte) => Some(Output {
                byte: *byte,
                remember: true,
            }),
            Code::RepeatOutput => m.remembered_output.map(|byte| Output {
                byte,
                remember: false,
            }),
            Code::RepeatAction => m
                .remembered_key
                .and_then(|key| {
                    self.resolve_key::<PROFILE>(key, state, m, prefix, stack, next, ops)
                })
                .map(|v| Output {
                    byte: v.byte,
                    remember: false,
                }),
            Code::TextOne { table } => {
                if PROFILE {
                    ops.suffix_index_lookups += 1;
                    ops.text_one += 1;
                }
                let selected = table[prefix.last().map_or(256, |&byte| byte as usize)];
                if PROFILE && depth == 0 {
                    ops.text_one_selection = if matches!(
                        selected,
                        Emit::Terminal {
                            op: Terminal::RepeatOutput,
                            ..
                        }
                    ) {
                        1
                    } else {
                        2
                    };
                }
                self.emit::<PROFILE>(selected, state, m, prefix, stack, next, ops)
            }
            Code::TextRules(index) => {
                if PROFILE {
                    ops.text_longer += 1;
                }
                let selected = index.lookup::<PROFILE>(prefix, ops);
                self.emit::<PROFILE>(selected, state, m, prefix, stack, next, ops)
            }
            Code::PressRules { basis, table } => {
                let key = if *basis == Basis::Press {
                    m.last
                } else {
                    m.previous
                };
                let binding = key.map_or(self.keys.len(), |key| state.ids[key]);
                self.emit::<PROFILE>(table[binding], state, m, prefix, stack, next, ops)
            }
            Code::OutputRules { basis, table } => {
                let byte = match basis {
                    Basis::Remembered => m.remembered_output,
                    Basis::Output => m.last_output,
                    Basis::SkipOutput => m.previous_output,
                    _ => unreachable!(),
                };
                self.emit::<PROFILE>(
                    table[byte.map_or(256, usize::from)],
                    state,
                    m,
                    prefix,
                    stack,
                    next,
                    ops,
                )
            }
        }
    }
}

// Slots have fixed geometry. IDs refer to the initial bindings, including labels
// used by press rules. Maintain candidate masks in O(1) per swap, not per context.
pub(crate) struct KeyState {
    ids: [usize; MAX_KEYS],
    literals: [u64; 256],
    actions: u64,
}

impl KeyState {
    pub(crate) fn new(program: &Program) -> Self {
        let mut state = Self {
            ids: [0; MAX_KEYS],
            literals: [0; 256],
            actions: 0,
        };
        for (key, &binding) in program.keys.iter().enumerate() {
            state.ids[key] = key;
            state.insert(key, binding);
        }
        state
    }

    fn insert(&mut self, key: usize, binding: Key) {
        match binding {
            Key::Byte(b) => self.literals[b as usize] |= 1u64 << key,
            Key::Action(_) => self.actions |= 1u64 << key,
            Key::Empty => {}
        }
    }

    pub(crate) fn swap(&mut self, a: usize, b: usize, program: &Program) {
        assert!(a < program.keys.len() && b < program.keys.len());
        let (x, y) = (program.keys[self.ids[a]], program.keys[self.ids[b]]);
        let clear = !((1u64 << a) | (1u64 << b));
        // Clear both first: duplicate literal bindings must retain both bits.
        for binding in [x, y] {
            if let Key::Byte(byte) = binding {
                self.literals[byte as usize] &= clear;
            }
        }
        self.actions &= clear;
        self.ids.swap(a, b);
        self.insert(a, y);
        self.insert(b, x);
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Memory {
    remembered_key: Option<usize>,
    remembered_output: Option<u8>,
    last: Option<usize>,
    previous: Option<usize>,
    last_output: Option<u8>,
    previous_output: Option<u8>,
}

#[derive(Clone, Copy)]
struct Output {
    byte: u8,
    remember: bool,
}

pub(crate) struct Mapper<'a> {
    program: &'a Program,
    state: &'a KeyState,
    previous: [u8; 5],
    previous_len: usize,
    memories: [Memory; 6],
    starts: [usize; 6],
    keys: [Option<usize>; 5],
    stack: [usize; 32],
}

impl<'a> Mapper<'a> {
    pub(crate) fn new(program: &'a Program, state: &'a KeyState) -> Self {
        Self {
            program,
            state,
            previous: [0; 5],
            previous_len: 0,
            memories: [Memory::default(); 6],
            starts: [0; 6],
            keys: [None; 5],
            stack: [0; 32],
        }
    }

    pub(crate) fn map<F>(&mut self, text: &[u8], effort: &F) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_profiled::<false, F>(text, effort, &mut MappingCounts::default())
    }

    pub(crate) fn map_profiled<const PROFILE: bool, F>(
        &mut self,
        text: &[u8],
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_impl::<PROFILE, true, F>(text, effort, ops)
    }
    // FAST=false retains the original generic root resolver for differential
    // tests. Production always specializes FAST=true; no runtime mode flag.
    fn map_impl<const PROFILE: bool, const FAST: bool, F>(
        &mut self,
        text: &[u8],
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if text.is_empty() || text.len() > 5 {
            return Err("cached context must contain 1 to 5 characters".into());
        }
        let common = text
            .iter()
            .zip(&self.previous[..self.previous_len])
            .take_while(|(a, b)| a == b)
            .count();
        if PROFILE {
            ops.positions_reused += common as u64;
        }
        for at in common..text.len() {
            if PROFILE {
                ops.positions_processed += 1;
            }
            let m = self.memories[at];
            let start = self.starts[at];
            let mut best: Option<(usize, Output, f64)> = None;
            // Literal-first and ascending physical-key order exactly match WindowMapper.
            for mut mask in [self.state.literals[text[at] as usize], self.state.actions] {
                while mask != 0 {
                    let key = mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    // Outcomes classify physical action-key attempts, not nested calls.
                    let action = PROFILE && (self.state.actions & (1u64 << key) != 0);
                    if action {
                        ops.action_attempts += 1;
                        ops.text_one_selection = 0;
                    }
                    let resolved = if FAST {
                        // The same binding dispatch as resolve_key. Literals do
                        // not inspect action tables or pay an extra mask test.
                        match self.program.keys[self.state.ids[key]] {
                            Key::Empty => None,
                            Key::Byte(byte) => Some(Output {
                                byte,
                                remember: true,
                            }),
                            Key::Action(id) => {
                                // Select the ORIGINAL indexed entry first. Only a
                                // proven repeat terminal bypasses generic resolve.
                                let repeat = match &self.program.actions[id] {
                                    Code::TextOne { table } => match table[if at == start {
                                        256
                                    } else {
                                        text[at - 1] as usize
                                    }] {
                                        Emit::Terminal {
                                            hops,
                                            op: Terminal::RepeatOutput,
                                        } => Some(hops),
                                        _ => None,
                                    },
                                    _ => None,
                                };
                                if let Some(hops) = repeat {
                                    if PROFILE {
                                        // Preserve logical operation counts even
                                        // though generic calls are bypassed.
                                        ops.resolutions += 1;
                                        ops.text_one += 1;
                                        ops.suffix_index_lookups += 1;
                                        ops.terminal_shortcuts += 1;
                                        ops.terminal_repeat += 1;
                                    }
                                    // Root resolve consumes one depth slot. The
                                    // compiler proved this terminal chain acyclic.
                                    if 1 + usize::from(hops) > 32 {
                                        if PROFILE {
                                            ops.terminal_depth_rejected += 1;
                                            ops.action_no_output += 1;
                                            ops.text_one_repeat_outcomes[0] += 1;
                                        }
                                        continue;
                                    }
                                    let Some(byte) = m.remembered_output else {
                                        if PROFILE {
                                            ops.action_no_output += 1;
                                            ops.text_one_repeat_outcomes[0] += 1;
                                        }
                                        continue;
                                    };
                                    if byte != text[at] {
                                        if PROFILE {
                                            ops.action_mismatch += 1;
                                            ops.text_one_repeat_outcomes[1] += 1;
                                        }
                                        continue;
                                    }
                                    if PROFILE {
                                        ops.text_one_selection = 1;
                                    }
                                    Some(Output {
                                        byte,
                                        remember: false,
                                    })
                                } else {
                                    self.program.resolve::<PROFILE>(
                                        id,
                                        self.state,
                                        m,
                                        &text[start..at],
                                        &mut self.stack,
                                        0,
                                        ops,
                                    )
                                }
                            }
                        }
                    } else {
                        self.program.resolve_key::<PROFILE>(
                            key,
                            self.state,
                            m,
                            &text[start..at],
                            &mut self.stack,
                            0,
                            ops,
                        )
                    };
                    if action {
                        let outcome = match resolved {
                            None => 0,
                            Some(output) if output.byte != text[at] => 1,
                            Some(_) => 2,
                        };
                        match ops.text_one_selection {
                            1 => ops.text_one_repeat_outcomes[outcome] += 1,
                            2 => ops.text_one_other_outcomes[outcome] += 1,
                            _ => {}
                        }
                        ops.text_one_selection = 0;
                    }
                    let Some(output) = resolved else {
                        if action {
                            ops.action_no_output += 1;
                        }
                        continue;
                    };
                    if output.byte != text[at] {
                        if action {
                            ops.action_mismatch += 1;
                        }
                        continue;
                    }
                    if action {
                        ops.action_matches += 1;
                    }
                    if PROFILE {
                        ops.effort_lookups += 1;
                    }
                    let cost = effort(m.previous, m.last, key);
                    if !cost.is_finite() {
                        if action {
                            ops.action_nonfinite_effort += 1;
                        }
                        return Err("non-finite typing effort".into());
                    }
                    if PROFILE && best.is_some() {
                        ops.effort_comparisons += 1;
                    }
                    if best.as_ref().is_some_and(|(_, _, old)| cost >= *old) {
                        if action {
                            ops.action_effort_losses += 1;
                        }
                        continue;
                    }
                    if PROFILE {
                        if let Some((old, _, _)) = best {
                            if self.state.actions & (1u64 << old) != 0 {
                                ops.action_effort_losses += 1;
                            }
                        }
                    }
                    best = Some((key, output, cost));
                }
            }
            if let Some((key, output, _)) = best {
                if PROFILE && self.state.actions & (1u64 << key) != 0 {
                    ops.action_winners += 1;
                }
                let mut next = m;
                next.previous = m.last;
                next.last = Some(key);
                next.previous_output = m.last_output;
                next.last_output = Some(output.byte);
                if output.remember {
                    next.remembered_key = Some(key);
                    next.remembered_output = Some(output.byte);
                }
                self.keys[at] = Some(key);
                self.memories[at + 1] = next;
                self.starts[at + 1] = start;
            } else {
                self.keys[at] = None;
                self.memories[at + 1] = Memory::default();
                self.starts[at + 1] = at + 1;
            }
        }
        self.previous[..text.len()].copy_from_slice(text);
        self.previous_len = text.len();
        Ok(self.keys)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;
    pub(crate) fn fixtures() -> Vec<String> {
        let base =
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
        let mut fixtures = vec![
            base.to_string(),
            crate::action_keys::test_layouts::SHORTHAND.to_string(),
        ];
        for basis in [
            "magic",
            "press-magic",
            "skip-magic",
            "output-magic",
            "skip-output-magic",
            "alternate",
        ] {
            let aliases = if matches!(basis, "press-magic" | "skip-magic") {
                "map m \"@rep\" = \"u\"\nmap m \"rep\" = none\nmap m \"␠\" = \"a\"\n"
            } else {
                ""
            };
            fixtures.push(format!("{base}outer-left: @m @rep @again\nouter-right: @sk ~ ~\naction m = {basis}\nmap m \"i\" = \"'\"\nmap m \"q\" = \"u\"\nmap m \"qu\" = \"e\"\n{aliases}fallback m = @sk\naction sk = skip-magic\nmap sk \"q\" = \"a\"\naction rep = repeat-output\naction again = repeat-action\n"));
        }
        // The bound root uses output history so its dynamic repeat-action
        // fallback remains intact under the text-magic repeat policy.
        fixtures.push(format!("{}outer-left: @m @again @literal\naction m = output-magic\nmap m \"q\" = \"u\"\nfallback m = repeat-action\naction again = repeat-action\naction literal = text \"q\"\naction unused = text \"multiple\"\n", base.replacen("w ", "q ", 1)));
        fixtures.push(crate::action_keys::test_layouts::COMPACT.to_string());
        fixtures.push(format!("{base}outer-left: ~ ◇ ~\n◇ hr u'\n"));
        fixtures.push(format!(
            "{base}outer-left: ~ ◇ ~\n◇ hr u'\nfallback ◇ = repeat-output\n"
        ));
        fixtures.push(format!("{base}adaptive n hr ay\nswap th qe\n"));
        for mode in ["standard", "anglemod", "nokwts", "meteorite"] {
            fixtures.push(format!("{base}swap h nr y ,u\nrow-stagger: {mode}\n"));
        }
        fixtures
    }
    #[test]
    fn numeric_mapper_matches_reference_keys_across_swaps_and_contexts() {
        let mut contexts = vec![
            b"i'i'a".to_vec(),
            b"qqqqu".to_vec(),
            b"qxuqu".to_vec(),
            b"qu!qu".to_vec(),
            b"q qu".to_vec(),
            b"'q".to_vec(),
            b"hrhnr".to_vec(),
            b"thqeq".to_vec(),
            b"h!nr".to_vec(),
        ];
        let mut rng = 19u64;
        let alphabet = b"quaei' !\0";
        for _ in 0..160 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = 1 + (rng >> 32) as usize % 5;
            let mut text = Vec::new();
            for _ in 0..len {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(alphabet[(rng >> 32) as usize % alphabet.len()]);
            }
            contexts.push(text);
        }
        // Deliberately not sorted: prefix reuse must work in either traversal order.
        for source in fixtures() {
            for order in 3..=5 {
                let mut layout = Layout::parse(&source, Path::new("numeric.dat")).unwrap();
                let program = Program::new(&layout, order).unwrap();
                let mut state = KeyState::new(&program);
                for trial in 0..90 {
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let a = (rng >> 32) as usize % layout.slots.len();
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let b = (rng >> 32) as usize % layout.slots.len();
                    layout.swap(a, b);
                    state.swap(a, b, &program);
                    for tied in [true, false] {
                        let mut reference = ak::WindowMapper::new(&layout, order).unwrap();
                        let mut fast = Mapper::new(&program, &state);
                        let effort = |previous: Option<usize>, last: Option<usize>, key: usize| {
                            if tied {
                                0.0
                            } else {
                                key as f64 * 0.01
                                    + last.map_or(0.0, |k| if k == key { 2.0 } else { 0.0 })
                                    + previous.map_or(0.0, |k| if k == key { 1.0 } else { 0.0 })
                            }
                        };
                        for text in &contexts {
                            let text = &text[..text.len().min(order)];
                            let expected = reference.map(text, &effort).unwrap();
                            let actual = fast.map(text, &effort).unwrap();
                            assert_eq!(
                                &actual[..text.len()],
                                &expected[..text.len()],
                                "order {order} trial {trial} text {text:?}"
                            );
                        }
                    }
                    if trial % 3 == 0 {
                        layout.swap(a, b);
                        state.swap(a, b, &program);
                    }
                }
            }
        }
    }
    #[test]
    fn numeric_mapper_matches_reference_depth_limit_and_validation() {
        let base =
            "q w e r t | y u i o p\na s d f g | h j k l ;\nz x c v b | n m , . /\nthumbs: space\n";
        for depth in [31, 32, 33] {
            let mut source = format!("{base}outer-left: ~ @m0 ~\n");
            for id in 0..depth {
                if id + 1 == depth {
                    source.push_str(&format!("action m{id} = text \"'\"\n"));
                } else {
                    // Only m0 is physically bound; keep its delegate fallback
                    // while exercising the unbound text-magic helper chain.
                    let basis = if id == 0 { "output-magic" } else { "magic" };
                    source.push_str(&format!(
                        "action m{id} = {basis}\nfallback m{id} = @m{}\n",
                        id + 1
                    ));
                }
            }
            let layout = Layout::parse(&source, Path::new("depth.dat")).unwrap();
            let program = Program::new(&layout, 5).unwrap();
            let state = KeyState::new(&program);
            assert_eq!(
                Mapper::new(&program, &state)
                    .map(b"q'", &|_, _, _| 0.0)
                    .unwrap(),
                ak::WindowMapper::new(&layout, 5)
                    .unwrap()
                    .map(b"q'", &|_, _, _| 0.0)
                    .unwrap()
            );
        }
        for suffix in [
            "outer-left: ~ @m ~\naction m = text \"ab\"\n",
            "outer-left: ~ @m ~\naction m = magic\nmap m \"abc\" = \"d\"\n",
        ] {
            let layout =
                Layout::parse(&format!("{base}{suffix}"), Path::new("invalid.dat")).unwrap();
            assert_eq!(
                Program::new(&layout, 3).err(),
                ak::WindowMapper::new(&layout, 3).err()
            );
        }
    }
    #[test]
    fn numeric_masks_cover_maximum_key_count_and_duplicate_literals() {
        let mut layout = Layout::parse(&fixtures()[0], Path::new("capacity.dat")).unwrap();
        while layout.slots.len() < MAX_KEYS {
            layout.slots.push(layout.slots[0].clone());
        }
        let program = Program::new(&layout, 5).unwrap();
        let mut state = KeyState::new(&program);
        for (a, b) in [(0, 59), (0, 1), (59, 1), (59, 59)] {
            layout.swap(a, b);
            state.swap(a, b, &program);
            let effort = |_: Option<usize>, last: Option<usize>, key: usize| {
                -(key as f64) + last.map_or(0.0, |old| if old == key { 10.0 } else { 0.0 })
            };
            assert_eq!(
                Mapper::new(&program, &state)
                    .map(b"qwqqq", &effort)
                    .unwrap(),
                ak::WindowMapper::new(&layout, 5)
                    .unwrap()
                    .map(b"qwqqq", &effort)
                    .unwrap()
            );
        }
        layout.slots.push(layout.slots[0].clone());
        assert!(Program::new(&layout, 5).is_err());
    }
    #[test]
    fn profiling_counts_do_not_change_mapping() {
        for source in fixtures() {
            let layout = Layout::parse(&source, Path::new("profile.dat")).unwrap();
            let program = Program::new(&layout, 5).unwrap();
            let state = KeyState::new(&program);
            let mut off = Mapper::new(&program, &state);
            let mut on = Mapper::new(&program, &state);
            let mut disabled = MappingCounts::default();
            let mut enabled = MappingCounts::default();
            for text in [b"qqqqu".as_slice(), b"i'i'a", b"qu!qu", b"q qu", b"qu"] {
                let effort = |_: Option<usize>, _: Option<usize>, key: usize| key as f64 * 0.01;
                assert_eq!(
                    off.map_profiled::<false, _>(text, &effort, &mut disabled)
                        .unwrap(),
                    on.map_profiled::<true, _>(text, &effort, &mut enabled)
                        .unwrap()
                );
            }
            assert_eq!(disabled, MappingCounts::default());
            assert_eq!(
                enabled.action_attempts,
                enabled.action_no_output + enabled.action_mismatch + enabled.action_matches
            );
            assert_eq!(
                enabled.action_matches,
                enabled.action_effort_losses + enabled.action_winners
            );
            assert_eq!(
                enabled.suffix_index_lookups,
                enabled.text_one + enabled.text_longer
            );
            assert_eq!(
                enabled.terminal_shortcuts,
                enabled.terminal_none + enabled.terminal_byte + enabled.terminal_repeat
            );
            assert!(enabled.effort_lookups > 0);
            assert!(enabled.recursive <= enabled.resolutions);
        }
    }
    // Exact generic-root comparison: keys, all live prefix memory, effort-call
    // sequence/values (including NaN bits), errors, and logical profile counters.
    fn compare_root_paths(program: &Program, state: &KeyState, contexts: &[Vec<u8>], mode: usize) {
        use std::cell::RefCell;
        let mut fast = Mapper::new(program, state);
        let mut generic = Mapper::new(program, state);
        let mut fast_ops = MappingCounts::default();
        let mut generic_ops = MappingCounts::default();
        for text in contexts {
            let fast_calls = RefCell::new(Vec::new());
            let generic_calls = RefCell::new(Vec::new());
            let cost = |a: Option<usize>, b: Option<usize>, key: usize| -> f64 {
                match mode {
                    0 => 0.0,
                    // Literal wins ties; first action wins action ties.
                    1 => -(key as f64),
                    2 => {
                        key as f64
                            + a.map_or(0.0, |v| if v == key { 0.25 } else { 0.0 })
                            + b.map_or(0.0, |v| if v == key { 0.5 } else { 0.0 })
                    }
                    3 => {
                        if state.actions & (1u64 << key) != 0 {
                            f64::NAN
                        } else {
                            0.0
                        }
                    }
                    _ => f64::INFINITY,
                }
            };
            let a = fast.map_impl::<true, true, _>(
                text,
                &|p, q, k| {
                    let v = cost(p, q, k);
                    fast_calls.borrow_mut().push((p, q, k, v.to_bits()));
                    v
                },
                &mut fast_ops,
            );
            let b = generic.map_impl::<true, false, _>(
                text,
                &|p, q, k| {
                    let v = cost(p, q, k);
                    generic_calls.borrow_mut().push((p, q, k, v.to_bits()));
                    v
                },
                &mut generic_ops,
            );
            assert_eq!(a, b, "mode {mode}, context {text:?}");
            assert_eq!(
                *fast_calls.borrow(),
                *generic_calls.borrow(),
                "effort calls for {text:?}"
            );
            assert_eq!(fast.memories, generic.memories, "memory for {text:?}");
            assert_eq!(fast.starts, generic.starts);
            assert_eq!(fast.keys, generic.keys);
            assert_eq!(fast.previous, generic.previous);
            assert_eq!(fast.previous_len, generic.previous_len);
            assert_eq!(fast_ops, generic_ops, "counts for {text:?}");
            // Stale recursion-stack cells are deliberately not compared. Every
            // generic invocation checks only its live [..depth] ancestor slice.
            if a.is_err() {
                fast = Mapper::new(program, state);
                generic = Mapper::new(program, state);
            }
        }
    }
    #[test]
    fn root_repeat_fast_path_matches_generic_across_swaps() {
        let contexts: Vec<Vec<u8>> = [
            b"".as_slice(),
            b"q",
            b"qq",
            b"qqqqq",
            b"q!qqq",
            b"i'i'a",
            b"qu!qu",
            b"qu",
            b"q qu",
            b"rkrkr",
            b"aaaaa",
            b"'qq",
            b"!!!!!!",
        ]
        .iter()
        .map(|t| t.to_vec())
        .collect();
        let mut rng = 71u64;
        for source in fixtures() {
            for order in 3..=5 {
                let layout = Layout::parse(&source, Path::new("root-repeat.dat")).unwrap();
                let program = Program::new(&layout, order).unwrap();
                let mut state = KeyState::new(&program);
                for _ in 0..24 {
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let a = (rng >> 32) as usize % layout.slots.len();
                    rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let b = (rng >> 32) as usize % layout.slots.len();
                    state.swap(a, b, &program);
                    for mode in 0..5 {
                        compare_root_paths(&program, &state, &contexts, mode);
                    }
                }
            }
        }
    }
    #[test]
    fn root_repeat_fast_path_preserves_selected_entries_depth_and_cycles() {
        for hops in [1, 30, 31, 32] {
            let repeat = Emit::Terminal {
                hops,
                op: Terminal::RepeatOutput,
            };
            let mut table = Box::new([repeat; 257]);
            // fallback repeat, including empty prefix
            table[b'i' as usize] = Emit::Byte(b'x');
            // explicit byte overrides repeat
            table[b'n' as usize] = Emit::None;
            // explicit none overrides repeat
            table[b'r' as usize] = repeat;
            // explicit repeat is equally eligible
            table[b'd' as usize] = Emit::Call(1);
            // dynamic RepeatAction
            table[b'c' as usize] = Emit::Call(0);
            // cycle: must remain generic
            table[b't' as usize] = Emit::Call(2);
            // nested TextOne repeat
            let program = Program {
                keys: vec![
                    Key::Byte(b'q'),
                    Key::Byte(b'i'),
                    Key::Byte(b'x'),
                    Key::Byte(b'n'),
                    Key::Byte(b'r'),
                    Key::Byte(b'd'),
                    Key::Byte(b'c'),
                    Key::Byte(b't'),
                    Key::Action(0),
                    Key::Action(0),
                ],
                actions: vec![
                    Code::TextOne { table },
                    Code::RepeatAction,
                    Code::TextOne {
                        table: Box::new([repeat; 257]),
                    },
                ],
            };
            let state = KeyState::new(&program);
            let contexts: Vec<Vec<u8>> = [
                b"q".as_slice(),
                b"qq",
                b"qqqqq",
                b"qqx",
                b"q!qqq",
                b"ix",
                b"ii",
                b"nn",
                b"nq",
                b"rr",
                b"rrrrr",
                b"ddddd",
                b"ccccc",
                b"ttttt",
                b"!qqqq",
            ]
            .iter()
            .map(|t| t.to_vec())
            .collect();
            for mode in 0..5 {
                compare_root_paths(&program, &state, &contexts, mode);
            }
        }
    }
    #[test]
    fn action_outcomes_count_final_winners_and_prefix_reuse() {
        let program = Program {
            keys: vec![
                Key::Byte(b'q'),
                Key::Action(0),
                Key::Action(1),
                Key::Action(2),
                Key::Action(3),
            ],
            actions: vec![
                Code::Inactive,
                Code::Byte(b'x'),
                Code::Byte(b'q'),
                Code::Byte(b'q'),
            ],
        };
        let state = KeyState::new(&program);
        for tied in [false, true] {
            let mut mapper = Mapper::new(&program, &state);
            let mut ops = MappingCounts::default();
            let effort = |_: Option<usize>, _: Option<usize>, key: usize| {
                if tied {
                    0.0
                } else {
                    -(key as f64)
                }
            };
            let first = mapper
                .map_profiled::<true, _>(b"q", &effort, &mut ops)
                .unwrap();
            assert_eq!(first[0], Some(if tied { 0 } else { 4 }));
            // The repeated call reuses one position and adds no action attempts.
            assert_eq!(
                mapper
                    .map_profiled::<true, _>(b"q", &effort, &mut ops)
                    .unwrap(),
                first
            );
            assert_eq!(ops.positions_processed, 1);
            assert_eq!(ops.positions_reused, 1);
            assert_eq!(ops.action_attempts, 4);
            assert_eq!(ops.action_no_output, 1);
            assert_eq!(ops.action_mismatch, 1);
            assert_eq!(ops.action_matches, 2);
            assert_eq!(ops.action_effort_losses, if tied { 2 } else { 1 });
            assert_eq!(ops.action_winners, if tied { 0 } else { 1 });
            assert_eq!(ops.action_nonfinite_effort, 0);
        }
    }
    #[test]
    fn text_one_cross_counts_follow_selected_emission_and_final_output() {
        for repeat in [true, false] {
            let mut table = Box::new([Emit::None; 257]);
            if repeat {
                table.fill(Emit::Terminal {
                    hops: 1,
                    op: Terminal::RepeatOutput,
                });
            } else {
                table[b'q' as usize] = Emit::Byte(b'x');
            }
            let program = Program {
                keys: vec![Key::Byte(b'q'), Key::Byte(b'x'), Key::Action(0)],
                actions: vec![Code::TextOne { table }],
            };
            let state = KeyState::new(&program);
            let mut mapper = Mapper::new(&program, &state);
            let mut off = Mapper::new(&program, &state);
            let mut ops = MappingCounts::default();
            // At empty prefix: no output. After q: repeat matches q and
            // mismatches x; an explicit x emission has the opposite outcomes.
            for text in [b"q".as_slice(), b"qq", b"qx"] {
                let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
                assert_eq!(
                    mapper
                        .map_profiled::<true, _>(text, &effort, &mut ops)
                        .unwrap(),
                    off.map(text, &effort).unwrap()
                );
            }
            assert_eq!(ops.action_attempts, 3);
            assert_eq!(ops.text_one_selection, 0);
            assert_eq!(
                ops.text_one_repeat_outcomes,
                if repeat { [1, 1, 1] } else { [0; 3] }
            );
            assert_eq!(
                ops.text_one_other_outcomes,
                if repeat { [0; 3] } else { [1, 1, 1] }
            );
        }
    }
    #[test]
    fn terminal_kind_counters_include_depth_rejected_shortcuts() {
        let program = Program {
            keys: vec![],
            actions: vec![],
        };
        let state = KeyState::new(&program);
        let mut stack = [0; 32];
        let mut ops = MappingCounts::default();
        for op in [Terminal::None, Terminal::Byte(b'q'), Terminal::RepeatOutput] {
            for depth in [0, 32] {
                let _ = program.emit::<true>(
                    Emit::Terminal { hops: 1, op },
                    &state,
                    Memory::default(),
                    b"",
                    &mut stack,
                    depth,
                    &mut ops,
                );
            }
        }
        assert_eq!(ops.terminal_shortcuts, 6);
        assert_eq!(ops.terminal_depth_rejected, 3);
        assert_eq!(ops.terminal_none, 2);
        assert_eq!(ops.terminal_byte, 2);
        assert_eq!(ops.terminal_repeat, 2);
    }
    #[test]
    fn indexed_suffix_selection_matches_longest_suffix_reference() {
        let mut rules = BTreeMap::new();
        rules.insert(Vec::new(), Emission::Text(vec![b'e']));
        for len in 1..=4 {
            for bits in 0..(1usize << len) {
                let context = (0..len)
                    .map(|i| if bits & (1 << i) == 0 { b'a' } else { b'b' })
                    .collect::<Vec<_>>();
                let emit = if bits % 3 == 0 {
                    Emission::None
                } else {
                    Emission::Text(vec![b'0' + len as u8])
                };
                rules.insert(context, emit);
            }
        }
        rules.insert(b"xa".to_vec(), Emission::Text(vec![b'x']));
        let ids = BTreeMap::new();
        let defs = BTreeMap::new();
        let code = text_rules(&rules, Emit::Byte(b'z'), &ids, &defs);
        let Code::TextRules(index) = code else {
            panic!("expected longer suffix index");
        };
        let alphabet = b"abxy";
        let mut ops = MappingCounts::default();
        for len in 0..=4 {
            for mut value in 0..4usize.pow(len as u32) {
                let prefix = (0..len)
                    .map(|_| {
                        let b = alphabet[value % 4];
                        value /= 4;
                        b
                    })
                    .collect::<Vec<_>>();
                let expected = rules
                    .iter()
                    .filter(|(key, _)| prefix.ends_with(key))
                    .max_by_key(|(key, _)| key.len())
                    .map_or(Emit::Byte(b'z'), |(_, e)| emission(e, &ids, &defs));
                assert_eq!(
                    index.lookup::<true>(&prefix, &mut ops),
                    expected,
                    "{prefix:?}"
                );
            }
        }
        // A final byte without longer rules does not compare unrelated entries.
        let before = ops.suffix_checks;
        assert_eq!(index.lookup::<true>(b"aay", &mut ops), Emit::Byte(b'e'));
        assert_eq!(ops.suffix_checks, before);
    }
    #[test]
    fn shorthand_uses_direct_suffix_table_and_terminal_repeat_instruction() {
        let layout = Layout::parse(
            crate::action_keys::test_layouts::SHORTHAND,
            Path::new("direct.dat"),
        )
        .unwrap();
        let program = Program::new(&layout, 5).unwrap();
        let state = KeyState::new(&program);
        let mut ops = MappingCounts::default();
        let fast = Mapper::new(&program, &state)
            .map_profiled::<true, _>(b"qqqqq", &|_, _, _| 0.0, &mut ops)
            .unwrap();
        let reference = ak::WindowMapper::new(&layout, 5)
            .unwrap()
            .map(b"qqqqq", &|_, _, _| 0.0)
            .unwrap();
        assert_eq!(fast, reference);
        assert_eq!(ops.suffix_checks, 0);
        assert!(ops.suffix_index_lookups > 0);
        assert!(ops.terminal_shortcuts > 0);
        assert_eq!(ops.recursive, 0);
    }
    #[test]
    fn terminal_chains_preserve_depth_limits_and_memory() {
        let base = fixtures()[0].clone();
        for depth in [1, 2, 30, 31, 32, 33, 34] {
            for terminal in ["text \"'\"", "repeat-output", "inactive", "magic"] {
                let mut source = format!("{base}outer-left: ~ @m0 ~\n");
                for id in 0..depth {
                    if id + 1 == depth {
                        let kind = if id == 0 && terminal == "magic" {
                            "output-magic"
                        } else {
                            terminal
                        };
                        source.push_str(&format!("action m{id} = {kind}\n"));
                        if terminal == "magic" {
                            source.push_str(&format!("fallback m{id} = \"'\"\n"));
                        }
                    } else {
                        let basis = if id == 0 { "output-magic" } else { "magic" };
                        source.push_str(&format!(
                            "action m{id} = {basis}\nfallback m{id} = @m{}\n",
                            id + 1
                        ));
                    }
                }
                let layout = Layout::parse(&source, Path::new("terminal-depth.dat")).unwrap();
                let program = Program::new(&layout, 5).unwrap();
                let state = KeyState::new(&program);
                let mut fast = Mapper::new(&program, &state);
                let mut reference = ak::WindowMapper::new(&layout, 5).unwrap();
                for text in [b"q'qqq".as_slice(), b"qqqqq", b"q!q'q", b"'qq"] {
                    let effort = |_: Option<usize>, _: Option<usize>, key: usize| -(key as f64);
                    assert_eq!(
                        fast.map(text, &effort).unwrap(),
                        reference.map(text, &effort).unwrap(),
                        "depth {depth} terminal {terminal}"
                    );
                }
            }
        }
    }
    #[test]
    fn cyclic_and_history_dependent_delegates_are_not_flattened() {
        let mut defs = BTreeMap::new();
        defs.insert(
            "a".into(),
            Action::Rules {
                basis: Basis::Text,
                rules: BTreeMap::new(),
                fallback: Emission::Call("b".into()),
            },
        );
        defs.insert(
            "b".into(),
            Action::Rules {
                basis: Basis::Text,
                rules: BTreeMap::new(),
                fallback: Emission::Call("a".into()),
            },
        );
        defs.insert("again".into(), Action::RepeatAction);
        let ids = BTreeMap::from([("a".into(), 0), ("b".into(), 1), ("again".into(), 2)]);
        assert_eq!(
            emission(&Emission::Call("a".into()), &ids, &defs),
            Emit::Call(0)
        );
        assert_eq!(
            emission(&Emission::Call("again".into()), &ids, &defs),
            Emit::Call(2)
        );
        let source = format!("{}outer-left: ~ @a ~\naction a = inactive\n", fixtures()[0]);
        let mut layout = Layout::parse(&source, Path::new("cycle.dat")).unwrap();
        // Programmatic cycle: the .dat parser rejects static cycles earlier.
        layout.actions.extend(defs);
        let program = Program::new(&layout, 5).unwrap();
        let state = KeyState::new(&program);
        assert_eq!(
            Mapper::new(&program, &state)
                .map(b"q'q", &|_, _, _| 0.0)
                .unwrap(),
            ak::WindowMapper::new(&layout, 5)
                .unwrap()
                .map(b"q'q", &|_, _, _| 0.0)
                .unwrap()
        );
    }
}
