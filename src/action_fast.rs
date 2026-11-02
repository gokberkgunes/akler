//! Numeric implementation of WindowMapper's existing bounded-context policy.
//! Compiled once per search. Successful mapping builds no diagnostics and allocates no heap storage.
use crate::action_keys::{self as ak, Action, Basis, Binding, Emission, Layout};

use std::collections::{BTreeMap, BTreeSet};

use crate::action_profile::MappingCounts;

const MAX_KEYS: usize = 60;
const NO_KEY: u8 = u8::MAX;

#[inline(always)]
fn stored_key(key: usize) -> u8 {
    debug_assert!(key < MAX_KEYS);
    key as u8
}

#[inline(always)]
fn optional_key(key: u8) -> Option<usize> {
    (key != NO_KEY).then_some(usize::from(key))
}

// Source byte zero occupies the least significant byte, so a trailing-zero
// count on two words' XOR gives their common byte prefix. Padding is excluded
// by the explicit context lengths, including when source bytes themselves are
// zero. Read only bytes belonging to the validated bounded context.
#[inline(always)]
fn packed_context(text: &[u8]) -> u64 {
    match text.len() {
        1 => u64::from(text[0]),
        2 => u64::from(u16::from_le_bytes([text[0], text[1]])),
        3 => u64::from(u32::from_le_bytes([text[0], text[1], text[2], 0])),
        4 => u64::from(u32::from_le_bytes([text[0], text[1], text[2], text[3]])),
        5 => {
            u64::from(u32::from_le_bytes([text[0], text[1], text[2], text[3]]))
                | (u64::from(text[4]) << 32)
        }
        _ => unreachable!("context length was validated"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Terminal {
    None,
    Byte(u8),
    RepeatOutput,
    RepeatPreviousOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emit {
    None,
    Byte(u8),
    Call(usize),
    Terminal(Terminal),
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
    RepeatOutput,
    RepeatPreviousOutput,
    RepeatAction,
    TextOne {
        table: Box<[Emit; 257]>,
    },
    TextRules(SuffixIndex),
    PressRules {
        basis: Basis,
        table: Box<[Emit]>,
    },
}

pub(crate) struct Program {
    keys: Vec<Key>,
    actions: Vec<Code>,
    action_outputs: Vec<Option<[u64; 4]>>,
    fingers: Vec<usize>,
}

// Resolve an unconditional chain to its terminal. Layouts reject call cycles,
// so the chain ends. Branching rules and RepeatAction remain dynamic calls.
fn terminal_call<'a>(mut name: &'a str, defs: &'a BTreeMap<String, Action>) -> Option<Terminal> {
    loop {
        match &defs[name] {
            Action::RepeatOutput => return Some(Terminal::RepeatOutput),
            Action::RepeatPreviousOutput => return Some(Terminal::RepeatPreviousOutput),
            Action::Rules {
                rules, fallback, ..
            } if rules.is_empty() => match fallback {
                Emission::None => return Some(Terminal::None),
                Emission::Text(v) => return Some(Terminal::Byte(v[0])),
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
        Emission::Call(n) => terminal_call(n, defs).map_or(Emit::Call(ids[n]), Emit::Terminal),
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
                Action::RepeatOutput => Code::RepeatOutput,
                Action::RepeatPreviousOutput => Code::RepeatPreviousOutput,
                Action::RepeatAction => Code::RepeatAction,
                Action::Rules {
                    basis,
                    rules,
                    fallback,
                } => {
                    // Adaptive keys read the key pressed before them.
                    let by_key = ak::key_context(name, *basis, rules, fallback);
                    let fallback = emission(fallback, &ids, &layout.actions);
                    match basis {
                        Basis::Text if !by_key => {
                            text_rules(rules, fallback, &ids, &layout.actions)
                        }
                        Basis::Text | Basis::Press | Basis::SkipPress => {
                            let mut table = Vec::with_capacity(layout.slots.len() + 1);
                            let mut prefixed = Vec::new();
                            for context in layout
                                .slots
                                .iter()
                                .map(|s| match &s.binding {
                                    Binding::Named(n) => n.as_bytes(),
                                    _ => s.label.as_bytes(),
                                })
                                .chain(std::iter::once(b"".as_slice()))
                            {
                                prefixed.clear();
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
                                basis: if by_key { Basis::Press } else { *basis },
                                table: table.into_boxed_slice(),
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
        let fingers = layout.slots.iter().map(|slot| slot.finger).collect();
        let mut program = Self {
            keys,
            actions,
            action_outputs: Vec::new(),
            fingers,
        };
        let mut outputs = vec![None; program.actions.len()];
        for id in 0..program.actions.len() {
            let mut bytes = [0; 4];
            program.collect_action_bytes(id, &mut outputs, &mut bytes);
        }
        program.action_outputs = outputs
            .into_iter()
            .map(|output| output.unwrap_or(None))
            .collect();
        Ok(program)
    }

    fn collect_emit_bytes(
        &self,
        emit: Emit,
        outputs: &mut [Option<Option<[u64; 4]>>],
        bytes: &mut [u64; 4],
    ) -> bool {
        match emit {
            Emit::None | Emit::Terminal(Terminal::None) => true,
            Emit::Byte(byte) | Emit::Terminal(Terminal::Byte(byte)) => {
                bytes[usize::from(byte) / 64] |= 1 << (usize::from(byte) % 64);
                true
            }
            Emit::Call(id) => self.collect_action_bytes(id, outputs, bytes),
            Emit::Terminal(Terminal::RepeatOutput | Terminal::RepeatPreviousOutput) => false,
        }
    }

    /// Collect every byte a named action can emit without depending on typing
    /// history. Returning false means the action can repeat a previous output
    /// or repeat a physical binding; callers must then examine every cached
    /// context. Layouts reject call cycles, so the recursion ends.
    fn collect_action_bytes(
        &self,
        id: usize,
        outputs: &mut [Option<Option<[u64; 4]>>],
        bytes: &mut [u64; 4],
    ) -> bool {
        let output = match outputs[id] {
            Some(output) => output,
            None => {
                let mut local = [0; 4];
                let mut collect = |emit| self.collect_emit_bytes(emit, outputs, &mut local);
                let known = match &self.actions[id] {
                    Code::RepeatOutput | Code::RepeatPreviousOutput | Code::RepeatAction => false,
                    Code::TextOne { table } => table.iter().copied().all(&mut collect),
                    Code::TextRules(index) => {
                        index.short.iter().copied().all(&mut collect)
                            && index.longer.iter().map(|rule| rule.emit).all(&mut collect)
                    }
                    Code::PressRules { table, .. } => table.iter().copied().all(&mut collect),
                };
                let output = known.then_some(local);
                outputs[id] = Some(output);
                output
            }
        };
        if let Some(output) = output {
            for (target, source) in bytes.iter_mut().zip(output) {
                *target |= source;
            }
            true
        } else {
            false
        }
    }

    /// Return the union of source bytes whose cached contexts can change when
    /// these physical slots move. Literal bindings contribute one byte. A
    /// static named action contributes all outputs from all rule branches and
    /// nested calls. History-dependent or recursive actions return None so the
    /// incremental evaluator falls back to a full scan.
    pub(crate) fn affected_bytes(&self, state: &KeyState, slots: &[usize]) -> Option<[u64; 4]> {
        let mut bytes = [0; 4];
        for &slot in slots {
            match self.keys[state.ids[slot]] {
                Key::Empty => {}
                Key::Byte(byte) => {
                    bytes[usize::from(byte) / 64] |= 1 << (usize::from(byte) % 64);
                }
                Key::Action(id) => match self.action_outputs[id] {
                    Some(outputs) => {
                        for (target, source) in bytes.iter_mut().zip(outputs) {
                            *target |= source;
                        }
                    }
                    None => return None,
                },
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
            Emit::Terminal(op) => {
                if PROFILE {
                    ops.terminal_shortcuts += 1;
                    match op {
                        Terminal::None => ops.terminal_none += 1,
                        Terminal::Byte(_) => ops.terminal_byte += 1,
                        Terminal::RepeatOutput | Terminal::RepeatPreviousOutput => {
                            ops.terminal_repeat += 1
                        }
                    }
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
                    Terminal::RepeatPreviousOutput => m.previous_output.map(|byte| Output {
                        byte,
                        remember: true,
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
        // Calls are one level deep, so only repeat-action can revisit an
        // action. The depth test never fails; it spares the bounds checks.
        if depth >= stack.len() || stack[..depth].contains(&id) {
            return None;
        }
        stack[depth] = id;
        let next = depth + 1;
        match &self.actions[id] {
            Code::RepeatOutput => m.remembered_output.map(|byte| Output {
                byte,
                remember: false,
            }),
            Code::RepeatPreviousOutput => m.previous_output.map(|byte| Output {
                byte,
                remember: true,
            }),
            Code::RepeatAction => optional_key(m.remembered_key)
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
                        Emit::Terminal(Terminal::RepeatOutput)
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
                let binding = optional_key(key).map_or(self.keys.len(), |key| state.ids[key]);
                self.emit::<PROFILE>(table[binding], state, m, prefix, stack, next, ops)
            }
        }
    }
}

// Slots have fixed geometry. IDs refer to the initial bindings, including labels
// used by press rules. Maintain candidate masks in O(1) per swap, not per context.
#[derive(Clone)]
pub(crate) struct KeyState {
    ids: [usize; MAX_KEYS],
    literals: [u64; 256],
    actions: u64,
    action_targets: [u64; 256],
    dynamic_actions: u64,
    // The set of bound actions is unchanged by physical-slot permutations.
    terminal_dependencies: Option<(bool, bool)>,
}

impl KeyState {
    /// The slot holding each binding, indexed by binding id.
    pub(crate) fn slots(&self, bindings: usize) -> [u8; MAX_KEYS] {
        let mut slots = [NO_KEY; MAX_KEYS];
        for (slot, &id) in self.ids[..bindings].iter().enumerate() {
            slots[id] = slot as u8;
        }
        slots
    }

    /// The binding id in `slot`.
    pub(crate) fn binding(&self, slot: usize) -> usize {
        self.ids[slot]
    }

    pub(crate) fn new(program: &Program) -> Self {
        let mut state = Self {
            ids: [0; MAX_KEYS],
            literals: [0; 256],
            actions: 0,
            action_targets: [0; 256],
            dynamic_actions: 0,
            terminal_dependencies: None,
        };
        for (key, &binding) in program.keys.iter().enumerate() {
            state.ids[key] = key;
            state.insert(key, binding, program);
        }
        state.terminal_dependencies = bounded_terminal_dependencies(program, &state);
        state
    }

    fn update_action_targets(&mut self, key: usize, id: usize, program: &Program, add: bool) {
        let bit = 1u64 << key;
        let Some(outputs) = program.action_outputs.get(id).copied().flatten() else {
            if add {
                self.dynamic_actions |= bit;
            } else {
                self.dynamic_actions &= !bit;
            }
            return;
        };
        for (word, mut outputs) in outputs.into_iter().enumerate() {
            while outputs != 0 {
                let offset = outputs.trailing_zeros() as usize;
                outputs &= outputs - 1;
                let target = word * 64 + offset;
                if add {
                    self.action_targets[target] |= bit;
                } else {
                    self.action_targets[target] &= !bit;
                }
            }
        }
    }

    fn insert(&mut self, key: usize, binding: Key, program: &Program) {
        match binding {
            Key::Byte(b) => self.literals[b as usize] |= 1u64 << key,
            Key::Action(id) => {
                self.actions |= 1u64 << key;
                self.update_action_targets(key, id, program, true);
            }
            Key::Empty => {}
        }
    }

    fn remove(&mut self, key: usize, binding: Key, program: &Program) {
        let bit = !(1u64 << key);
        match binding {
            Key::Byte(byte) => self.literals[byte as usize] &= bit,
            Key::Action(id) => {
                self.actions &= bit;
                self.update_action_targets(key, id, program, false);
            }
            Key::Empty => {}
        }
    }

    fn actions_for(&self, byte: u8) -> u64 {
        self.action_targets[byte as usize] | self.dynamic_actions
    }

    /// Earliest source position needed by a finite-effort terminal tail.
    /// Two consecutive unique literals replace every history field before
    /// the first ambiguous tail byte. Eligible actions inspect at most the
    /// final source byte, so the prefix before those literals is irrelevant.
    /// This boundary is unchanged when bindings move between physical slots.
    /// Whether `byte` has one literal root and no action emits it: it is
    /// then typed on that key whatever surrounds it, without look-ahead.
    /// Slot permutations keep this.
    pub(crate) fn unique(&self, byte: u8) -> bool {
        self.actions_for(byte) == 0 && self.literals[byte as usize].is_power_of_two()
    }

    /// The slots whose key can type `byte`, as a bit mask: literally, or as
    /// some action's output.
    pub(crate) fn typing_slots(&self, byte: u8) -> u64 {
        self.literals[byte as usize] | self.actions_for(byte)
    }

    pub(crate) fn terminal_dependency_start(
        &self,
        _program: &Program,
        text: &[u8],
        end: usize,
    ) -> usize {
        if self.terminal_dependencies.is_none() || end > text.len() {
            return 0;
        }
        let unique = |byte: u8| self.unique(byte);
        let tail_start = end.saturating_sub(3);
        let Some(first) = (tail_start..end).find(|&at| !unique(text[at])) else {
            return tail_start;
        };
        if first >= 2 && unique(text[first - 2]) && unique(text[first - 1]) {
            first - 2
        } else {
            0
        }
    }

    /// Return a terminal tail whose roots are independent of action history
    /// and typing effort. Every represented byte must have exactly one literal
    /// root and no action capable of emitting it.
    pub(crate) fn direct_terminal(&self, text: &[u8], end: usize) -> Option<[u8; 3]> {
        if end > text.len() {
            return None;
        }
        let start = end.saturating_sub(3);
        let mut tail = [NO_KEY; 3];
        for (offset, &byte) in text[start..end].iter().enumerate() {
            let literals = self.literals[byte as usize];
            if self.actions_for(byte) != 0 || !literals.is_power_of_two() {
                return None;
            }
            tail[3 - (end - start) + offset] = literals.trailing_zeros() as u8;
        }
        Some(tail)
    }

    pub(crate) fn swap(&mut self, a: usize, b: usize, program: &Program) {
        assert!(a < program.keys.len() && b < program.keys.len());
        let (x, y) = (program.keys[self.ids[a]], program.keys[self.ids[b]]);
        self.remove(a, x, program);
        self.remove(b, y, program);
        self.ids.swap(a, b);
        self.insert(a, y, program);
        self.insert(b, x, program);
    }
}

/// The binding typing each character of a context when no effort choice can
/// arise: every action emits a fixed byte and reads only the previous
/// character, or only the previous key (adaptive keys), and in each such
/// context at most one binding types each character. The mapper then has a
/// single candidate per position, so effort and look-ahead never matter,
/// and a binding keeps typing the same positions wherever a permutation
/// moves it.
pub(crate) struct StaticBindings {
    /// [previous character, 256 at a run start][character]: the binding,
    /// other than adaptive keys, typing it, `UNTYPED` when none can, `MIXED`
    /// when several could.
    text: Vec<[u8; 256]>,
    /// The same for adaptive keys by the previous key's binding, `keys.len()
    /// - 1` at a run start; empty without adaptive keys.
    keys: Vec<[u8; 256]>,
}

pub(crate) const UNTYPED: u8 = u8::MAX;
const MIXED: u8 = u8::MAX - 1;

impl StaticBindings {
    /// `None` when some action reads more than the previous character or
    /// key, or any other history (calls, longer rules, repeats of anything
    /// but the previous character).
    ///
    /// A repeat types the previous character: it repeats the remembered
    /// output, every output but a repeat's is remembered, and a repeat's
    /// equals the remembered one, so within a run the remembered output is
    /// the previous character.
    pub(crate) fn new(program: &Program) -> Option<Self> {
        let simple = |emit: &Emit| {
            matches!(
                emit,
                Emit::None
                    | Emit::Byte(_)
                    | Emit::Terminal(Terminal::None | Terminal::Byte(_))
            )
        };
        let repeat = |emit: &Emit| {
            matches!(
                emit,
                Emit::Terminal(Terminal::RepeatOutput)
            )
        };
        let mut adaptive = vec![false; program.actions.len()];
        for (id, code) in program.actions.iter().enumerate() {
            match code {
                Code::RepeatOutput => {}
                Code::TextOne { table }
                    if table.iter().all(|emit| simple(emit) || repeat(emit)) => {}
                Code::PressRules {
                    basis: Basis::Press,
                    table,
                } if table.iter().all(simple) => adaptive[id] = true,
                _ => return None,
            }
        }
        let is_adaptive =
            |binding: usize| matches!(program.keys[binding], Key::Action(id) if adaptive[id]);
        let state = KeyState::new(program);
        let mut stack = [0; 32];
        let mut ops = MappingCounts::default();
        let mut fill = |row: &mut [u8; 256], memory: Memory, prefix: &[u8], adaptive: bool| {
            for binding in
                (0..program.keys.len()).filter(|&binding| is_adaptive(binding) == adaptive)
            {
                let output = program
                    .resolve_key::<false>(binding, &state, memory, prefix, &mut stack, 0, &mut ops);
                if let Some(output) = output {
                    let entry = &mut row[usize::from(output.byte)];
                    *entry = if *entry == UNTYPED {
                        binding as u8
                    } else {
                        MIXED
                    };
                }
            }
        };
        let mut text = vec![[UNTYPED; 256]; 257];
        for (previous, row) in text.iter_mut().enumerate() {
            let mut memory = Memory::default();
            let prefix = [previous as u8];
            let prefix: &[u8] = if previous == 256 {
                &[]
            } else {
                memory.remembered_output = Some(previous as u8);
                memory.last_output = Some(previous as u8);
                &prefix
            };
            fill(row, memory, prefix, false);
        }
        let mut keys = Vec::new();
        if adaptive.contains(&true) {
            keys = vec![[UNTYPED; 256]; program.keys.len() + 1];
            for (previous, row) in keys.iter_mut().enumerate() {
                let mut memory = Memory::default();
                if previous < program.keys.len() {
                    memory.last = stored_key(previous);
                }
                fill(row, memory, &[], true);
            }
        }
        Some(Self { text, keys })
    }

    /// The binding typing each position of `text`, `UNTYPED` where nothing
    /// does (the next position then starts a new run, as in the mapper), or
    /// `None` when some position has several candidates.
    pub(crate) fn bindings(&self, text: &[u8]) -> Option<[u8; 5]> {
        let mut bindings = [UNTYPED; 5];
        let mut start = 0;
        for (at, &byte) in text.iter().enumerate() {
            let mut entry = if at == start {
                self.text[256][usize::from(byte)]
            } else {
                self.text[usize::from(text[at - 1])][usize::from(byte)]
            };
            if !self.keys.is_empty() {
                let previous = if at == start {
                    self.keys.len() - 1
                } else {
                    usize::from(bindings[at - 1])
                };
                entry = match (entry, self.keys[previous][usize::from(byte)]) {
                    (entry, UNTYPED) => entry,
                    (UNTYPED, keyed) => keyed,
                    _ => MIXED,
                };
            }
            match entry {
                MIXED => return None,
                UNTYPED => start = at + 1,
                binding => bindings[at] = binding,
            }
        }
        Some(bindings)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Memory {
    // Physical slots fit in u8 because Program rejects more than MAX_KEYS.
    // Keeping these hot history records compact avoids moving three tagged
    // Option<usize> values for every mapped position.
    remembered_key: u8,
    remembered_output: Option<u8>,
    last: u8,
    previous: u8,
    last_output: Option<u8>,
    previous_output: Option<u8>,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            remembered_key: NO_KEY,
            remembered_output: None,
            last: NO_KEY,
            previous: NO_KEY,
            last_output: None,
            previous_output: None,
        }
    }
}

#[derive(Clone, Copy)]
struct Output {
    byte: u8,
    remember: bool,
}

const TRANSITION_SLOTS: usize = 4096;
const PREPARED_TERMINAL_SLOTS: usize = 16384;

/// Validated source bytes and terminal end for an already bounded context.
/// Source bytes occupy the low 40 bits; length and end have separate tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreparedContext(u64);

impl PreparedContext {
    #[cfg(test)]
    pub(crate) fn new(text: &[u8], end: usize) -> ak::Result<Self> {
        if end > text.len() {
            return Err("terminal end exceeds cached context length".into());
        }
        if text.is_empty() || text.len() > 5 {
            return Err("cached context must contain 1 to 5 characters".into());
        }
        Ok(Self(
            packed_context(text) | ((text.len() as u64) << 40) | ((end as u64) << 43),
        ))
    }
}

#[derive(Clone, Copy, Default)]
struct Transition {
    signature: u64,
    /// The owning mapper's `stamp`; any other value reads as empty.
    stamp: u32,
    key: u8,
    remember: bool,
    look_ahead: bool,
}

#[derive(Clone, Copy, Default)]
struct PreparedTerminal {
    // Valid contexts have a nonzero length tag, leaving zero as empty.
    signature: u64,
    stamp: u32,
    tail: [u8; 3],
}

/// Memo tables kept between mappers, so each candidate need not allocate and
/// clear them: entries written under an earlier stamp read as empty.
#[derive(Default)]
pub(crate) struct MemoTables {
    transitions: Option<Box<[Transition]>>,
    prepared_terminals: Option<Box<[PreparedTerminal]>>,
    stamp: u32,
}

#[inline(always)]
fn transition_slot(signature: u64) -> usize {
    (signature.wrapping_mul(0x9e3779b97f4a7c15) >> (64 - 12)) as usize
}

#[inline(always)]
fn prepared_terminal_slot(signature: u64) -> usize {
    (signature.wrapping_mul(0x9e3779b97f4a7c15) >> (64 - 14)) as usize
}

#[inline(always)]
fn transition_signature(memory: Memory, prefix: Option<u8>, byte: u8, next: Option<u8>) -> u64 {
    let output = |byte: Option<u8>| byte.map_or(256, u64::from);
    // Eligible actions cannot inspect remembered_key or last_output. They
    // remain in the live memory used below to apply the cached winning output.
    u64::from(memory.last)
        | (u64::from(memory.previous) << 8)
        | (output(memory.remembered_output) << 16)
        | (output(memory.previous_output) << 25)
        | (output(prefix) << 34)
        | (u64::from(byte) << 43)
        | (output(next) << 51)
}

fn bounded_terminal_dependencies(program: &Program, state: &KeyState) -> Option<(bool, bool)> {
    let mut remembered = false;
    let mut previous = false;
    let mut roots = state.actions;
    while roots != 0 {
        let key = roots.trailing_zeros() as usize;
        roots &= roots - 1;
        let Key::Action(id) = program.keys[state.ids[key]] else {
            unreachable!("action root mask contains only action bindings");
        };
        match &program.actions[id] {
            Code::RepeatOutput => remembered = true,
            Code::RepeatPreviousOutput => previous = true,
            Code::PressRules {
                basis: Basis::Press,
                table,
            } if table
                .iter()
                .all(|emit| matches!(emit, Emit::None | Emit::Byte(_))) => {}
            Code::TextOne { table } => {
                for emit in table.iter() {
                    match emit {
                        Emit::Call(_) => return None,
                        Emit::Terminal(Terminal::RepeatOutput) => remembered = true,
                        Emit::Terminal(Terminal::RepeatPreviousOutput) => previous = true,
                        _ => {}
                    }
                }
            }
            _ => return None,
        }
    }
    Some((remembered, previous))
}

pub(crate) struct Mapper<'a> {
    program: &'a Program,
    state: &'a KeyState,
    previous: u64,
    previous_len: usize,
    memories: [Memory; 6],
    starts: [u8; 6],
    keys: [u8; 5],
    may_look_ahead: [bool; 5],
    stack: [usize; 32],
    transitions: Option<Box<[Transition]>>,
    transition_memory_mask: u64,
    prepared_terminals: Option<Box<[PreparedTerminal]>>,
    /// Tables from `reuse_memos` not yet enabled.
    spare: MemoTables,
    /// Marks this mapper's memo entries; never zero, the empty entry's.
    stamp: u32,
}

impl<'a> Mapper<'a> {
    #[inline(always)]
    fn same_finger_roots(&self, mut roots: u64, key: usize) -> bool {
        let finger = self.program.fingers[key];
        while roots != 0 {
            let root = roots.trailing_zeros() as usize;
            roots &= roots - 1;
            if self.program.fingers[root] != finger {
                return false;
            }
        }
        true
    }

    fn action_priority(&self, key: usize, memory: Memory) -> Option<bool> {
        let Key::Action(id) = self.program.keys[self.state.ids[key]] else {
            return None;
        };
        let skip = match &self.program.actions[id] {
            Code::PressRules {
                basis: Basis::SkipPress,
                ..
            }
            | Code::RepeatPreviousOutput => true,
            Code::TextOne { .. }
            | Code::TextRules(_)
            | Code::PressRules { .. }
            | Code::RepeatOutput
            | Code::RepeatAction => false,
        };
        let finger = self.program.fingers[key];
        let prior = if skip { memory.previous } else { memory.last };
        if optional_key(prior).is_some_and(|old| self.program.fingers[old] == finger) {
            return Some(false);
        }
        let other = if skip { memory.last } else { memory.previous };
        if optional_key(other).is_some_and(|old| self.program.fingers[old] == finger) {
            return None;
        }
        Some(true)
    }

    pub(crate) fn new(program: &'a Program, state: &'a KeyState) -> Self {
        Self {
            program,
            state,
            previous: 0,
            previous_len: 0,
            memories: [Memory::default(); 6],
            starts: [0; 6],
            keys: [NO_KEY; 5],
            may_look_ahead: [false; 5],
            stack: [0; 32],
            transitions: None,
            transition_memory_mask: u64::MAX,
            prepared_terminals: None,
            spare: MemoTables::default(),
            stamp: 1,
        }
    }

    /// Take memo tables an earlier mapper returned with `return_memos`; the
    /// `enable_*_memo` calls then use them instead of fresh tables.
    pub(crate) fn reuse_memos(&mut self, memos: &mut MemoTables) {
        let mut spare = std::mem::take(memos);
        spare.stamp = spare.stamp.wrapping_add(1);
        if spare.stamp == 0 {
            // Entries may carry any earlier stamp; start over from empty.
            spare = MemoTables {
                stamp: 1,
                ..MemoTables::default()
            };
        }
        self.stamp = spare.stamp;
        self.spare = spare;
    }

    /// Give this mapper's memo tables back for a later `reuse_memos`.
    pub(crate) fn return_memos(&mut self, memos: &mut MemoTables) {
        let mut spare = std::mem::take(&mut self.spare);
        spare.transitions = self.transitions.take().or(spare.transitions);
        spare.prepared_terminals = self.prepared_terminals.take().or(spare.prepared_terminals);
        spare.stamp = self.stamp;
        *memos = spare;
    }

    /// Reuse pure finite-effort transitions within one candidate. The caller
    /// must use the same effort function for the remaining lifetime of this
    /// mapper. Only bound actions that inspect at most one source byte
    /// and have no dynamic calls qualify; ordinary mapping bypasses this cache.
    /// Profiling records cache hits/misses and counts action attempts only for
    /// transitions whose resolution actually runs.
    pub(crate) fn enable_transition_memo(&mut self) {
        let Some((remembered, previous)) = self.state.terminal_dependencies else {
            return;
        };
        let ambiguous = self.state.actions != 0
            || self
                .state
                .literals
                .iter()
                .any(|&roots| roots & roots.wrapping_sub(1) != 0);
        if ambiguous {
            // All remaining history used by effort and action priority is
            // retained. Unread outputs cannot affect a cached winner, and the
            // actual memory update below still propagates their live values.
            self.transition_memory_mask = u64::MAX
                ^ (if remembered { 0 } else { 511 << 16 })
                ^ (if previous { 0 } else { 511 << 25 });
            self.transitions = Some(self.spare.transitions.take().unwrap_or_else(|| {
                vec![Transition::default(); TRANSITION_SLOTS].into_boxed_slice()
            }));
        }
    }

    /// Reuse complete prepared terminal results within one candidate. The
    /// effort function must stay pure, finite, bounded, and unchanged for this
    /// mapper's remaining lifetime. Every source byte, including following
    /// lookahead, and the relative terminal end participate in the key.
    pub(crate) fn enable_prepared_terminal_memo(&mut self) {
        self.prepared_terminals = Some(self.spare.prepared_terminals.take().unwrap_or_else(|| {
            vec![PreparedTerminal::default(); PREPARED_TERMINAL_SLOTS].into_boxed_slice()
        }));
    }

    pub(crate) fn map<F>(&mut self, text: &[u8], effort: &F) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_impl::<false, true, false, F>(
            text,
            None,
            None,
            effort,
            &mut MappingCounts::default(),
        )?;
        Ok(self.expanded_keys())
    }

    /// Return every source position with fixed, pure, finite bounded effort.
    /// Unlike terminal mapping, contextual fingers can inspect the full result.
    pub(crate) fn map_finite_profiled<const PROFILE: bool, F>(
        &mut self,
        text: &[u8],
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_impl::<PROFILE, true, true, F>(text, None, None, effort, ops)?;
        Ok(self.expanded_keys())
    }

    /// Map forced combo roots while treating every marked source position as
    /// part of one chord event. The returned roots still identify every key
    /// used by the chord, but chord keys never become physical predecessors.
    pub(crate) fn map_chords<F>(
        &mut self,
        text: &[u8],
        forced: &[Option<usize>],
        chords: &[bool],
        effort: &F,
    ) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        self.map_chords_profiled::<false, F>(
            text,
            forced,
            chords,
            effort,
            &mut MappingCounts::default(),
        )
    }

    pub(crate) fn map_chords_profiled<const PROFILE: bool, F>(
        &mut self,
        text: &[u8],
        forced: &[Option<usize>],
        chords: &[bool],
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[Option<usize>; 5]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if forced.len() != text.len() || chords.len() != text.len() {
            return Err("forced roots and chord mask must match cached context length".into());
        }
        if forced
            .iter()
            .flatten()
            .any(|&key| key >= self.program.keys.len())
        {
            return Err("forced root is not a physical slot".into());
        }
        if chords
            .iter()
            .zip(forced)
            .any(|(&chord, root)| chord && root.is_none())
        {
            return Err("chord position requires a forced physical root".into());
        }
        if forced.iter().all(Option::is_none) && !chords.contains(&true) {
            // Nothing forced: the plain, memoized mapping.
            return self.map_profiled::<PROFILE, F>(text, effort, ops);
        }
        self.map_impl::<PROFILE, true, false, F>(text, Some(forced), Some(chords), effort, ops)?;
        Ok(self.expanded_keys())
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
        self.map_impl::<PROFILE, true, false, F>(text, None, None, effort, ops)?;
        Ok(self.expanded_keys())
    }

    /// Map an ordinary context and return only its terminal physical history.
    /// The action optimizer stores this compact tail, so avoid copying the
    /// full `[Option<usize>; 5]` result through the hot proposal loop.
    pub(crate) fn map_terminal_profiled<const PROFILE: bool, F>(
        &mut self,
        text: &[u8],
        end: usize,
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[u8; 3]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if end > text.len() {
            return Err("terminal end exceeds cached context length".into());
        }
        self.map_impl::<PROFILE, true, false, F>(text, None, None, effort, ops)?;
        Ok(self.terminal(end))
    }

    /// Terminal-only mapping for callers that have proved every effort value
    /// pure, bounded, and finite. With no eligible action and exactly one
    /// literal root, that root wins without consulting effort. Fully literal
    /// terminal tails bypass mapping of history that the caller discards.
    pub(crate) fn map_terminal_finite_profiled<const PROFILE: bool, F>(
        &mut self,
        text: &[u8],
        end: usize,
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[u8; 3]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if text.is_empty() || text.len() > 5 {
            return Err("cached context must contain 1 to 5 characters".into());
        }
        if end > text.len() {
            return Err("terminal end exceeds cached context length".into());
        }
        // The caller has proved effort pure, bounded, and finite.  When every
        // source position represented in the returned tail has one literal
        // root and no action that can emit that byte, those physical roots are
        // independent of all preceding output and press history.  Avoid
        // rebuilding the rest of the bounded context merely to discard it.
        //
        // A direct return deliberately invalidates prefix reuse: the compact
        // terminal result does not reconstruct the internal memory snapshots
        // required by a later general mapping.
        if let Some(tail) = self.state.direct_terminal(text, end) {
            self.previous_len = 0;
            if PROFILE {
                ops.literal_contexts += 1;
            }
            return Ok(tail);
        }
        let start = self
            .state
            .terminal_dependency_start(self.program, text, end);
        self.map_impl::<PROFILE, true, true, F>(&text[start..], None, None, effort, ops)?;
        Ok(self.terminal(end - start))
    }

    /// Map an immutable prepared context, checking the complete-result cache
    /// before decoding its source bytes. The same finite-effort and source
    /// dependency contract applies as to the slice-based prepared method.
    pub(crate) fn map_prepared_context_finite_profiled<const PROFILE: bool, F>(
        &mut self,
        context: PreparedContext,
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<[u8; 3]>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        let signature = context.0;
        if self.prepared_terminals.is_some() {
            let entry =
                self.prepared_terminals.as_ref().unwrap()[prepared_terminal_slot(signature)];
            if entry.signature == signature && entry.stamp == self.stamp {
                if PROFILE {
                    ops.prepared_terminal_hits += 1;
                }
                // The previous fully mapped prefix and its snapshots remain
                // valid. A result hit does not replace that reusable history.
                return Ok(entry.tail);
            }
            if PROFILE {
                ops.prepared_terminal_misses += 1;
            }
        }
        let bytes = signature.to_le_bytes();
        let len = ((signature >> 40) & 7) as usize;
        let end = ((signature >> 43) & 7) as usize;
        self.map_packed_impl::<PROFILE, true, true, F>(
            &bytes[..len],
            signature & 0xff_ffff_ffff,
            None,
            None,
            effort,
            ops,
        )?;
        let tail = self.terminal(end);
        if self.prepared_terminals.is_some() {
            self.prepared_terminals.as_mut().unwrap()[prepared_terminal_slot(signature)] =
                PreparedTerminal {
                    signature,
                    stamp: self.stamp,
                    tail,
                };
        }
        Ok(tail)
    }

    #[inline(always)]
    fn expanded_keys(&self) -> [Option<usize>; 5] {
        self.keys.map(optional_key)
    }

    #[inline(always)]
    fn terminal(&self, end: usize) -> [u8; 3] {
        // The last three keys before `end`, read as one window over the keys
        // padded with three empty positions.
        let k = self.keys;
        let padded = [NO_KEY, NO_KEY, NO_KEY, k[0], k[1], k[2], k[3], k[4]];
        let at = end.min(5);
        [padded[at], padded[at + 1], padded[at + 2]]
    }
    // FAST=false retains the original generic root resolver for differential
    // tests. Production always specializes FAST=true; no runtime mode flag.
    #[inline]
    fn map_impl<const PROFILE: bool, const FAST: bool, const PURE: bool, F>(
        &mut self,
        text: &[u8],
        forced: Option<&[Option<usize>]>,
        chords: Option<&[bool]>,
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        if text.is_empty() || text.len() > 5 {
            return Err("cached context must contain 1 to 5 characters".into());
        }
        let packed = packed_context(text);
        self.map_packed_impl::<PROFILE, FAST, PURE, F>(text, packed, forced, chords, effort, ops)
    }

    // Both callers supply validated contexts. Prepared callers already own
    // the packed source, while the general wrapper packs its source once.
    fn map_packed_impl<const PROFILE: bool, const FAST: bool, const PURE: bool, F>(
        &mut self,
        text: &[u8],
        packed: u64,
        forced: Option<&[Option<usize>]>,
        chords: Option<&[bool]>,
        effort: &F,
        ops: &mut MappingCounts,
    ) -> ak::Result<()>
    where
        F: Fn(Option<usize>, Option<usize>, usize) -> f64,
    {
        debug_assert!((1..=5).contains(&text.len()));
        let common = if forced.is_some() {
            0
        } else {
            (((packed ^ self.previous).trailing_zeros() / 8) as usize)
                .min(text.len())
                .min(self.previous_len)
        };
        let common = if common > 0
            && (common < text.len() || common < self.previous_len)
            && self.may_look_ahead[common - 1]
        {
            common - 1
        } else {
            common
        };
        if PROFILE {
            ops.positions_reused += common as u64;
        }
        for at in common..text.len() {
            if PROFILE {
                ops.positions_processed += 1;
            }
            let m = self.memories[at];
            let start = usize::from(self.starts[at]);
            let mut best: Option<(usize, Output, f64)> = None;
            let mut may_look_ahead = false;
            let literals = self.state.literals[text[at] as usize];
            let actions = self.state.actions_for(text[at]);
            // Under the finite-effort contract this lone literal is already
            // the winner. Update its history directly instead of routing a
            // guaranteed result through candidate and winner bookkeeping.
            if PURE
                && forced.is_none()
                && chords.is_none()
                && actions == 0
                && literals.is_power_of_two()
            {
                let key = literals.trailing_zeros() as u8;
                self.keys[at] = key;
                self.memories[at + 1] = Memory {
                    remembered_key: key,
                    remembered_output: Some(text[at]),
                    last: key,
                    previous: m.last,
                    last_output: Some(text[at]),
                    previous_output: m.last_output,
                };
                self.starts[at + 1] = start as u8;
                self.may_look_ahead[at] = false;
                continue;
            }
            if PURE && forced.is_none() && chords.is_none() && actions == 0 && literals == 0 {
                self.keys[at] = NO_KEY;
                self.memories[at + 1] = Memory::default();
                self.starts[at + 1] = (at + 1) as u8;
                self.may_look_ahead[at] = false;
                continue;
            }
            let memo_signature = if PURE
                && forced.is_none()
                && self.transitions.is_some()
                && (actions != 0 || literals & literals.wrapping_sub(1) != 0)
            {
                Some(
                    transition_signature(
                        m,
                        (at != start).then(|| text[at - 1]),
                        text[at],
                        text.get(at + 1).copied(),
                    ) & self.transition_memory_mask,
                )
            } else {
                None
            };
            let cached = memo_signature.and_then(|signature| {
                let entry = self.transitions.as_ref().unwrap()[transition_slot(signature)];
                (entry.stamp == self.stamp && entry.signature == signature).then_some(entry)
            });
            if PROFILE && memo_signature.is_some() {
                if cached.is_some() {
                    ops.transition_hits += 1;
                } else {
                    ops.transition_misses += 1;
                }
            }
            if let Some(entry) = cached {
                best = optional_key(entry.key).map(|key| {
                    (
                        key,
                        Output {
                            byte: text[at],
                            remember: entry.remember,
                        },
                        0.0,
                    )
                });
                may_look_ahead = entry.look_ahead;
            } else if let Some(key) = forced.and_then(|roots| roots[at]) {
                best = Some((
                    key,
                    Output {
                        byte: text[at],
                        remember: true,
                    },
                    0.0,
                ));
            } else {
                if PURE && actions == 0 && literals.is_power_of_two() {
                    best = Some((
                        literals.trailing_zeros() as usize,
                        Output {
                            byte: text[at],
                            remember: true,
                        },
                        0.0,
                    ));
                }
                // Literal-first and ascending physical-key order exactly match WindowMapper.
                for mut mask in if PURE && best.is_some() {
                    [0, 0]
                } else {
                    [literals, actions]
                } {
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
                                    // Select the original indexed entry once. Calls
                                    // retain the generic resolver and its live stack;
                                    // terminal entries need neither a repeated lookup
                                    // nor root recursion checks.
                                    let selected = match &self.program.actions[id] {
                                        Code::TextOne { table } => Some(
                                            table[if at == start {
                                                256
                                            } else {
                                                text[at - 1] as usize
                                            }],
                                        ),
                                        _ => None,
                                    };
                                    if let Some(Emit::Terminal(Terminal::RepeatOutput)) = selected
                                    {
                                        if PROFILE {
                                            // Preserve logical operation counts even
                                            // though generic calls are bypassed.
                                            ops.resolutions += 1;
                                            ops.text_one += 1;
                                            ops.suffix_index_lookups += 1;
                                            ops.terminal_shortcuts += 1;
                                            ops.terminal_repeat += 1;
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
                                    } else if let Some(
                                        emit @ (Emit::None | Emit::Byte(_) | Emit::Terminal(_)),
                                    ) = selected
                                    {
                                        if PROFILE {
                                            ops.resolutions += 1;
                                            ops.text_one += 1;
                                            ops.suffix_index_lookups += 1;
                                            ops.text_one_selection = 2;
                                        }
                                        self.program.emit::<PROFILE>(
                                            emit,
                                            self.state,
                                            m,
                                            &text[start..at],
                                            &mut self.stack,
                                            1,
                                            ops,
                                        )
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
                        let cost = effort(optional_key(m.previous), optional_key(m.last), key);
                        if !cost.is_finite() {
                            if action {
                                ops.action_nonfinite_effort += 1;
                            }
                            return Err("non-finite typing effort".into());
                        }
                        if PROFILE && best.is_some() {
                            ops.effort_comparisons += 1;
                        }
                        let mut nonfinite_next = false;
                        let loses = best.as_ref().is_some_and(|(old_key, _, old_cost)| {
                            let literal =
                                matches!(self.program.keys[self.state.ids[*old_key]], Key::Byte(_));
                            let priority = literal.then(|| self.action_priority(key, m)).flatten();
                            may_look_ahead |= priority != Some(false);
                            let next_is_chord =
                                chords.is_some_and(|mask| mask.get(at + 1) == Some(&true));
                            let projected =
                                if priority != Some(false) && at + 1 < text.len() && !next_is_chord
                                {
                                    let next = forced.and_then(|roots| roots[at + 1]).map_or(
                                        self.state.literals[text[at + 1] as usize],
                                        |slot| 1u64 << slot,
                                    );
                                    if next != 0 && (!literal || self.same_finger_roots(next, key))
                                    {
                                        let mut next_cost = |current| {
                                            let mut mask = next;
                                            let mut minimum = f64::INFINITY;
                                            while mask != 0 {
                                                let n = mask.trailing_zeros() as usize;
                                                mask &= mask - 1;
                                                let value =
                                                    effort(optional_key(m.last), Some(current), n);
                                                nonfinite_next |= !value.is_finite();
                                                minimum = minimum.min(value);
                                            }
                                            minimum
                                        };
                                        let action_total = cost + next_cost(key);
                                        let literal_total = *old_cost + next_cost(*old_key);
                                        nonfinite_next |=
                                            !action_total.is_finite() || !literal_total.is_finite();
                                        Some(
                                            action_total < literal_total
                                                || (priority == Some(true)
                                                    && action_total == literal_total),
                                        )
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                            priority == Some(false)
                                || match projected {
                                    Some(wins) => !wins,
                                    None => priority != Some(true) && cost >= *old_cost,
                                }
                        });
                        if nonfinite_next {
                            return Err("non-finite typing effort".into());
                        }
                        if loses {
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
            }
            if let Some(signature) = memo_signature {
                if cached.is_none() {
                    self.transitions.as_mut().unwrap()[transition_slot(signature)] = Transition {
                        signature,
                        stamp: self.stamp,
                        key: best.map_or(NO_KEY, |(key, _, _)| stored_key(key)),
                        remember: best.is_some_and(|(_, output, _)| output.remember),
                        look_ahead: may_look_ahead,
                    };
                }
            }
            if let Some((key, output, _)) = best {
                // A combo-forced root is a physical press, even when the
                // binding currently occupying that slot is an action.  It
                // bypassed resolution above, so it must not be profiled as an
                // action winner without a corresponding action attempt.
                if PROFILE
                    && cached.is_none()
                    && forced.and_then(|roots| roots[at]).is_none()
                    && self.state.actions & (1u64 << key) != 0
                {
                    ops.action_winners += 1;
                }
                let mut next = m;
                next.previous = m.last;
                next.last = stored_key(key);
                next.previous_output = m.last_output;
                next.last_output = Some(output.byte);
                if output.remember {
                    next.remembered_key = stored_key(key);
                    next.remembered_output = Some(output.byte);
                }
                if chords.is_some_and(|mask| mask[at]) {
                    next.previous = NO_KEY;
                    next.last = NO_KEY;
                    next.remembered_key = NO_KEY;
                }
                self.keys[at] = stored_key(key);
                self.memories[at + 1] = next;
                self.starts[at + 1] = start as u8;
            } else {
                self.keys[at] = NO_KEY;
                self.memories[at + 1] = Memory::default();
                self.starts[at + 1] = (at + 1) as u8;
            }
            self.may_look_ahead[at] = may_look_ahead;
        }
        self.previous = packed;
        self.previous_len = if forced.is_some() { 0 } else { text.len() };
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;
    /// Layouts covering literals, magic keys with repeat and literal
    /// fallbacks, a skip key that calls a magic key, adaptive keys, swaps
    /// and row stagger.
    pub(crate) fn fixtures() -> Vec<String> {
        use crate::action_keys::test_layouts::{compact, jsonc, magic, qwerty, shorthand, LEFT_OUTER_FINGERS};
        let twelve = r#""fingermap": ["0 0 1 2 3 3 6 6 7 8 9 9", "0 0 1 2 3 3 6 6 7 8 9 9", "0 0 1 2 3 3 6 6 7 8 9 9"]"#;
        let outer = [
            "skip q w e r t y u i o p skip",
            "@ a s d f g h j k l ; $",
            "skip z x c v b n m , . / skip",
        ];
        let diamond = [
            "skip q w e r t y u i o p",
            "◇ a s d f g h j k l ;",
            "skip z x c v b n m , . /",
        ];
        let swaps = [("hn", "hr"), ("hr", "hn"), ("y,", "yu"), ("yu", "y,")];
        vec![
            qwerty(""),
            shorthand(),
            // A magic key and a skip key that calls it.
            jsonc(outer, ["space", ""], &format!(r#"{twelve},
"magic": {{"keys": "@$", "wildcards": "*", "rules": [
  {{"inputs": "*@", "output": "**"}}, {{"inputs": "i@", "output": "i'"}},
  {{"inputs": "q@", "output": "qu"}}, {{"inputs": "qu@", "output": "que"}},
  {{"inputs": "*_$", "output": "*_*"}}, {{"inputs": "q_$", "output": "q_a"}},
  {{"inputs": "@_$", "call": "@"}}]}}"#)),
            // A magic key that repeats the last action where no rule matches.
            jsonc(outer, ["space", ""], &format!(r#"{twelve},
"magic": {{"key": "@", "fallback": "repeat-action", "rules": [{{"inputs": "i@", "output": "i'"}}, {{"inputs": "q@", "output": "qu"}}]}},
"skip": {{"key": "$", "rules": [{{"inputs": "q_$", "output": "q_a"}}]}}"#)),
            compact(),
            jsonc(diamond, ["space", ""], &format!("{LEFT_OUTER_FINGERS},\n{}", magic(&[("h◇", "hr"), ("u◇", "u'")]))),
            qwerty(&magic(&[("an", "ay"), ("hn", "hr"), ("the", "thq"), ("thq", "the")])),
            jsonc(
                ["q w e r t y u i o p", "a s d f g h j k l ;", "z x c v b n m , . /"],
                ["space", ""],
                &format!(r#""board": {{"isRowStaggered": true, "rowOrColumnStagger": [-0.25, 0, 0.5]}},
{}"#, magic(&swaps)),
            ),
            jsonc(diamond, ["space", ""], &format!("{LEFT_OUTER_FINGERS},\n{}", magic(&[[("h◇", "hr"), ("u◇", "u'")].as_slice(), &swaps].concat()))),
        ]
    }
    #[test]
    fn previous_output_fallback_and_following_magic_use_fast_memory() {
        // A repeat key and a key repeating the output two presses back.
        let source = crate::action_keys::test_layouts::jsonc(
            ["skip q w e r t y u i o p skip", "@ a s d f g h j k l ; $", "skip z x c v b n m , . / skip"],
            ["space", ""],
            r#""fingermap": ["0 0 1 2 3 3 6 6 7 8 9 9", "0 0 1 2 3 3 6 6 7 8 9 9", "0 0 1 2 3 3 6 6 7 8 9 9"],
"magic": {"keys": "@$", "wildcards": "*", "rules": [{"inputs": "*@", "output": "**"}, {"inputs": "*_$", "output": "*_*"}]}"#,
        );
        let layout = Layout::parse(&source, Path::new("previous-output.jsonc")).unwrap();
        let program = Program::new(&layout, 5).unwrap();
        let state = KeyState::new(&program);
        let named = |name: &str| {
            layout
                .slots
                .iter()
                .position(|slot| matches!(&slot.binding, Binding::Named(value) if value == name))
                .unwrap()
        };
        let sk = named("skip");
        let magic = named("magic");
        let keys = Mapper::new(&program, &state)
            .map(b"qxqq", &|_, _, next| {
                if next == sk || next == magic {
                    -1.0
                } else {
                    0.0
                }
            })
            .unwrap();
        assert_eq!(keys[2], Some(sk));
        assert_eq!(keys[3], Some(magic));
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
                let mut layout = Layout::parse(&source, Path::new("numeric.jsonc")).unwrap();
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
    fn adaptive_keys_read_the_key_pressed_before() {
        // After s the r key types c; after the r key, h is on l's key, even
        // where that r key typed c (as on keyboards and akl.gg).
        let layout = Layout::parse(
            &crate::action_keys::test_layouts::qwerty(&crate::action_keys::test_layouts::magic(&[
                ("sc", "sr"),
                ("sr", "sc"),
                ("rh", "rl"),
                ("rl", "rh"),
            ])),
            Path::new("key.jsonc"),
        )
        .unwrap();
        let slot = |label: &str| layout.slots.iter().position(|s| s.label == label).unwrap();
        let program = Program::new(&layout, 5).unwrap();
        let state = KeyState::new(&program);
        let effort = |_: Option<usize>, _: Option<usize>, _: usize| 0.0;
        let expected = [Some(slot("s")), Some(slot("r")), Some(slot("l"))];
        let fast = Mapper::new(&program, &state).map(b"sch", &effort).unwrap();
        assert_eq!(fast[..3], expected);
        let mut reference = ak::WindowMapper::new(&layout, 5).unwrap();
        assert_eq!(reference.map(b"sch", &effort).unwrap()[..3], expected);
        let statics = StaticBindings::new(&program).unwrap();
        let bindings = statics.bindings(b"sch").unwrap();
        assert_eq!(
            bindings[..3]
                .iter()
                .map(|&b| Some(usize::from(b)))
                .collect::<Vec<_>>(),
            expected
        );
        // "rh" alone types h on l's key too; "ch" after another key does not.
        assert_eq!(
            Mapper::new(&program, &state).map(b"rh", &effort).unwrap()[1],
            Some(slot("l"))
        );
        assert_eq!(
            Mapper::new(&program, &state).map(b"ach", &effort).unwrap()[2],
            Some(slot("h"))
        );
    }

    #[test]
    fn static_bindings_match_the_mapper_across_swaps() {
        let mut rng = 7u64;
        let mut next = |bound: usize| {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            (rng >> 32) as usize % bound
        };
        let alphabet = b"hnraytqe,uio' !\0";
        let contexts: Vec<Vec<u8>> = (0..400)
            .map(|_| {
                let len = 1 + next(5);
                (0..len).map(|_| alphabet[next(alphabet.len())]).collect()
            })
            .collect();
        let (mut eligible, mut checked, mut repeats, mut mixed) = (0, 0, 0, 0);
        // A repeating magic key with adaptive keys reads both the previous
        // character and the previous key.
        let both = fixtures().last().unwrap().clone();
        for source in fixtures() {
            let repeating = source.contains('◇');
            let mut layout = Layout::parse(&source, Path::new("static.jsonc")).unwrap();
            let program = Program::new(&layout, 5).unwrap();
            let Some(statics) = StaticBindings::new(&program) else {
                continue;
            };
            eligible += 1;
            let mut state = KeyState::new(&program);
            for _ in 0..40 {
                let (a, b) = (next(layout.slots.len()), next(layout.slots.len()));
                layout.swap(a, b);
                state.swap(a, b, &program);
                let slots = state.slots(program.keys.len());
                for tied in [true, false] {
                    let effort = |previous: Option<usize>, last: Option<usize>, key: usize| {
                        if tied {
                            0.0
                        } else {
                            key as f64 * 0.01
                                + last.map_or(0.0, |k| if k == key { 2.0 } else { 0.0 })
                                + previous.map_or(0.0, |k| if k == key { 1.0 } else { 0.0 })
                        }
                    };
                    let mut mapper = Mapper::new(&program, &state);
                    for text in &contexts {
                        let Some(bindings) = statics.bindings(text) else {
                            continue;
                        };
                        checked += 1;
                        repeats += usize::from(repeating);
                        mixed += usize::from(source == both);
                        let expected = mapper.map(text, &effort).unwrap();
                        let actual: Vec<_> = bindings[..text.len()]
                            .iter()
                            .map(|&binding| {
                                (binding != UNTYPED)
                                    .then(|| usize::from(slots[usize::from(binding)]))
                            })
                            .collect();
                        assert_eq!(&actual[..], &expected[..text.len()], "{source:?} {text:?}");
                    }
                }
            }
        }
        // The plain base, the swap fixtures, the magic key repeating the
        // previous character and that key with adaptive keys qualify.
        assert!(
            eligible >= 5 && checked > 5_000 && repeats > 2_000 && mixed > 1_000,
            "{eligible} {checked} {repeats} {mixed}"
        );
    }

    // Exact generic-root comparison: keys, all live prefix memory, effort-call
    // sequence/values (including NaN bits), errors, and logical profile counters.
    fn compare_root_paths(program: &Program, state: &KeyState, contexts: &[Vec<u8>], mode: usize) {
        use std::cell::RefCell;
        let mut fast = Mapper::new(program, state);
        let mut generic = Mapper::new(program, state);
        let new_finite = || {
            let mut mapper = Mapper::new(program, state);
            mapper.enable_transition_memo();
            mapper
        };
        let mut finite = (mode <= 2).then(new_finite);
        let mut fast_ops = MappingCounts::default();
        let mut generic_ops = MappingCounts::default();
        // A second pass also compares memoized transitions after unrelated
        // prefixes have changed the live history and lookahead boundaries.
        for text in contexts.iter().cycle().take(contexts.len() * 2) {
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
            let a = fast.map_impl::<true, true, false, _>(
                text,
                None,
                None,
                &|p, q, k| {
                    let v = cost(p, q, k);
                    fast_calls.borrow_mut().push((p, q, k, v.to_bits()));
                    v
                },
                &mut fast_ops,
            );
            let b = generic.map_impl::<true, false, false, _>(
                text,
                None,
                None,
                &|p, q, k| {
                    let v = cost(p, q, k);
                    generic_calls.borrow_mut().push((p, q, k, v.to_bits()));
                    v
                },
                &mut generic_ops,
            );
            let finite_result = finite.as_mut().map(|mapper| {
                mapper.map_impl::<false, true, true, _>(
                    text,
                    None,
                    None,
                    &cost,
                    &mut MappingCounts::default(),
                )
            });
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
            if let (Some(result), Some(mapper)) = (finite_result, finite.as_ref()) {
                assert_eq!(result, b, "finite mode {mode}, context {text:?}");
                assert_eq!(mapper.memories, generic.memories, "finite memory");
                assert_eq!(mapper.starts, generic.starts, "finite starts");
                assert_eq!(mapper.keys, generic.keys, "finite keys");
                assert_eq!(mapper.previous, generic.previous, "finite prefix");
                assert_eq!(mapper.previous_len, generic.previous_len);
            }
            // Stale recursion-stack cells are deliberately not compared. Every
            // generic invocation checks only its live [..depth] ancestor slice.
            if a.is_err() {
                fast = Mapper::new(program, state);
                generic = Mapper::new(program, state);
                finite = (mode <= 2).then(new_finite);
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
                let layout = Layout::parse(&source, Path::new("root-repeat.jsonc")).unwrap();
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
}
