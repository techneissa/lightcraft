//! A parsed condition: evaluation, canonical text, readable chips, and whether two conditions can
//! hold at the same time (what makes two commands on one key a conflict).

use super::keys::{self, Context, KEYS, Kind};

/// A condition over the context keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// An on / off key is on (`textFocus`).
    Key(usize),
    /// An enumeration key equals (`==`) or differs from (`!=`) a value.
    Is {
        key: usize,
        value: usize,
        equal: bool,
    },
    /// `!x`: negation of anything, a key, a comparison or a whole group.
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

/// One piece of the readable form of a condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    /// A context (`Loupe View`, `Not Typing`).
    Atom(String),
    /// A joining word (`and`, `or`) or `Not` before a group.
    Word(&'static str),
    Open,
    Close,
}

impl Expr {
    pub fn eval(&self, cx: &Context) -> bool {
        match self {
            Expr::Key(k) => cx.flag(*k),
            Expr::Is { key, value, equal } => (cx.get(*key) == *value) == *equal,
            Expr::Not(e) => !e.eval(cx),
            Expr::And(es) => es.iter().all(|e| e.eval(cx)),
            Expr::Or(es) => es.iter().any(|e| e.eval(cx)),
        }
    }

    /// The keys the condition reads.
    pub fn keys(&self, out: &mut Vec<usize>) {
        match self {
            Expr::Key(k) | Expr::Is { key: k, .. } => out.push(*k),
            Expr::Not(e) => e.keys(out),
            Expr::And(es) | Expr::Or(es) => es.iter().for_each(|e| e.keys(out)),
        }
    }

    /// Whether the condition reads `key`.
    pub fn mentions(&self, key: usize) -> bool {
        let mut v = Vec::new();
        self.keys(&mut v);
        v.contains(&key)
    }

    /// `self && other`, flattened.
    pub fn and(self, other: Expr) -> Expr {
        let mut parts = match self {
            Expr::And(es) => es,
            e => vec![e],
        };
        match other {
            Expr::And(es) => parts.extend(es),
            e => parts.push(e),
        }
        Expr::And(parts)
    }

    /// The canonical text (what is saved): single spaces, parentheses only where needed.
    pub fn text(&self) -> String {
        fn group(e: &Expr) -> String {
            format!("({})", e.text())
        }
        match self {
            Expr::Key(k) => KEYS.get(*k).map_or("", |d| d.name).to_string(),
            Expr::Is { key, value, equal } => {
                let name = KEYS.get(*key).map_or("", |d| d.name);
                let v = KEYS.get(*key).and_then(|d| d.values().get(*value)).map_or("", |v| v.name);
                format!("{name} {} {v}", if *equal { "==" } else { "!=" })
            }
            Expr::Not(e) => match &**e {
                Expr::Key(_) | Expr::Not(_) => format!("!{}", e.text()),
                _ => format!("!{}", group(e)),
            },
            Expr::And(es) => es.iter().map(|e| if matches!(e, Expr::Or(_)) { group(e) } else { e.text() }).collect::<Vec<_>>().join(" && "),
            Expr::Or(es) => es.iter().map(Expr::text).collect::<Vec<_>>().join(" || "),
        }
    }

    /// The readable form: one chip per context, `and` / `or` between them, `Not` in front of what
    /// is negated, groups in parentheses.
    pub fn chips(&self) -> Vec<Chip> {
        let mut out = Vec::new();
        self.push_chips(&mut out, false);
        out
    }

    fn push_chips(&self, out: &mut Vec<Chip>, in_and: bool) {
        // collapse a run of `!`: an even number cancels
        let mut negated = false;
        let mut inner = self;
        while let Expr::Not(e) = inner {
            negated = !negated;
            inner = e;
        }
        match inner {
            Expr::Key(k) => out.push(Chip::Atom(atom(*k, None, !negated))),
            Expr::Is { key, value, equal } => out.push(Chip::Atom(atom(*key, Some(*value), *equal != negated))),
            Expr::And(es) | Expr::Or(es) => {
                let is_and = matches!(inner, Expr::And(_));
                let wrap = negated || (in_and && !is_and);
                if negated {
                    out.push(Chip::Word("Not"));
                }
                if wrap {
                    out.push(Chip::Open);
                }
                for (i, e) in es.iter().enumerate() {
                    if i > 0 {
                        out.push(Chip::Word(if is_and { "and" } else { "or" }));
                    }
                    e.push_chips(out, is_and);
                }
                if wrap {
                    out.push(Chip::Close);
                }
            }
            Expr::Not(_) => {}
        }
    }
}

/// The chip text of a key (and value): `Loupe View`, or `Not Loupe View` when `positive` is false.
fn atom(key: usize, value: Option<usize>, positive: bool) -> String {
    let base = match KEYS.get(key).map(|d| &d.kind) {
        Some(Kind::Enum(values)) => value.and_then(|v| values.get(v)).map_or("", |v| v.chip),
        Some(Kind::Bool { chip }) => chip,
        None => "",
    };
    if positive { base.to_string() } else { format!("Not {base}") }
}

/// A context where both conditions hold (`None` = no condition = always), or `None` when they
/// can't both hold. Only the keys the two read are enumerated, with the keys that follow from them.
pub fn overlap(a: Option<&Expr>, b: Option<&Expr>) -> Option<Context> {
    let mut used = Vec::new();
    a.iter().chain(b.iter()).for_each(|e| e.keys(&mut used));
    let used = keys::closure(&used);
    let mut cx = Context::default();
    // every combination of the used keys' values, like an odometer
    let mut counter = vec![0usize; used.len()];
    loop {
        for (k, v) in used.iter().zip(&counter) {
            cx.set(*k, *v);
        }
        if cx.consistent() && a.is_none_or(|e| e.eval(&cx)) && b.is_none_or(|e| e.eval(&cx)) {
            return Some(cx);
        }
        let mut i = 0;
        loop {
            let Some(slot) = counter.get_mut(i) else { return None };
            let size = used.get(i).and_then(|k| KEYS.get(*k)).map_or(1, |d| d.size());
            *slot += 1;
            if *slot < size {
                break;
            }
            *slot = 0;
            i += 1;
        }
    }
}

/// Whether the condition can ever hold.
pub fn satisfiable(e: &Expr) -> bool {
    overlap(Some(e), None).is_some()
}

/// The chips of `cx` for the keys the two conditions read: "when both work" in words.
pub fn describe(cx: &Context, a: Option<&Expr>, b: Option<&Expr>) -> String {
    let mut used = Vec::new();
    a.iter().chain(b.iter()).for_each(|e| e.keys(&mut used));
    used.sort_unstable();
    used.dedup();
    let used: Vec<usize> = used.into_iter().filter(|k| *k != keys::GRID_VIEW || !used_has_view(a, b)).collect();
    used.iter().map(|k| cx.chip(*k)).collect::<Vec<_>>().join(", ")
}

fn used_has_view(a: Option<&Expr>, b: Option<&Expr>) -> bool {
    a.is_some_and(|e| e.mentions(keys::VIEW)) || b.is_some_and(|e| e.mentions(keys::VIEW))
}
