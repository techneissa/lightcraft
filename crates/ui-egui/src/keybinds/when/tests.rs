use super::expr::{self, Chip};
use super::keys::{self, Context};
use super::suggest::{self, ItemKind};
use super::*;

fn p(src: &str) -> Expr {
    parse(src).unwrap_or_else(|e| panic!("{src:?}: {e}")).unwrap_or_else(|| panic!("{src:?} is blank"))
}

fn e(src: &str) -> ParseError {
    match parse(src) {
        Err(e) => e,
        Ok(x) => panic!("{src:?} parsed: {x:?}"),
    }
}

fn holds(src: &str, cx: &Context) -> bool {
    p(src).eval(cx)
}

fn detail() -> Context {
    Context::default().with("view", "detail")
}

#[test]
fn blank_is_no_condition() {
    assert_eq!(parse("").unwrap(), None);
    assert_eq!(parse("  \t ").unwrap(), None);
}

#[test]
fn parses_every_operator() {
    assert_eq!(p("view == detail").text(), "view == detail");
    assert_eq!(p("view != grid").text(), "view != grid");
    assert_eq!(p("textFocus").text(), "textFocus");
    assert_eq!(p("!textFocus").text(), "!textFocus");
    assert_eq!(p("view==detail&&tool==crop").text(), "view == detail && tool == crop");
    assert_eq!(p("view == detail || view == compare").text(), "view == detail || view == compare");
    // && binds tighter than ||, parentheses override it
    assert_eq!(p("hasPhoto || brush && textFocus").text(), "hasPhoto || brush && textFocus");
    assert_eq!(p("(hasPhoto || brush) && textFocus").text(), "(hasPhoto || brush) && textFocus");
    assert_eq!(p("((view == detail))").text(), "view == detail");
    assert_eq!(p("textFocus == false").text(), "!textFocus");
    assert_eq!(p("textFocus != false").text(), "textFocus");
    assert_eq!(p("view == \"detail\"").text(), "view == detail");
}

#[test]
fn negation_is_first_class() {
    assert_eq!(p("!(view == detail)").text(), "!(view == detail)");
    assert_eq!(p("!(view == detail && tool == crop)").text(), "!(view == detail && tool == crop)");
    assert_eq!(p("!!textFocus").text(), "!!textFocus");
    assert_eq!(p("!textFocus && !brush").text(), "!textFocus && !brush");
    assert_eq!(p("!(textFocus || brush)").text(), "!(textFocus || brush)");
    // the canonical text parses back to the same condition
    for src in ["!(view == detail)", "!!textFocus", "!(brush)", "!(hasPhoto || brush) && view != grid"] {
        assert_eq!(p(&p(src).text()), p(src), "{src}");
    }
}

#[test]
fn evaluation_of_negations() {
    let mut cx = detail();
    assert!(holds("view == detail", &cx));
    assert!(!holds("view != detail", &cx));
    assert!(!holds("!(view == detail)", &cx));
    assert!(holds("!(view == grid)", &cx));
    assert!(holds("!!(view == detail)", &cx));
    assert!(holds("!textFocus", &cx));
    assert!(!holds("textFocus", &cx));
    cx.set_flag(keys::TEXT_FOCUS, true);
    assert!(!holds("!textFocus", &cx));
    assert!(holds("!textFocus || view == detail", &cx));
    assert!(!holds("!(textFocus || view == detail)", &cx));
    assert!(holds("!(textFocus && view == grid)", &cx));
}

#[test]
fn evaluation_of_and_or() {
    let cx = detail().with("tool", "crop");
    assert!(holds("view == detail && tool == crop", &cx));
    assert!(!holds("view == detail && tool == masking", &cx));
    assert!(holds("view == grid || tool == crop", &cx));
    assert!(holds("(view == grid || view == detail) && tool != masking", &cx));
    assert!(!holds("view == grid || view == compare", &cx));
}

#[test]
fn bad_input_reports_where_and_why() {
    assert!(e("viw == detail").message.contains("Unknown key “viw”, did you mean “view”"));
    assert_eq!(e("viw == detail").at, 0..3);
    assert!(e("view == deatil").message.contains("did you mean “detail”"));
    assert!(e("view == nonsense").message.contains("one of: grid, square, detail"));
    assert!(e("view = detail").message.contains("=="));
    assert!(e("a & b").message.contains("&&"));
    assert!(e("view | detail").message.contains("||"));
    assert!(e("view").message.contains("needs a value"));
    assert!(e("textFocus == maybe").message.contains("true or false"));
    assert!(e("view == detail and tool == crop").message.contains("Unexpected “and”"));
    assert!(e("and").message.contains("&&"));
    assert!(e("view == detail )").message.contains("Unexpected “)”"));
    assert!(e(") view").message.contains("no ("));
    assert!(e("(view == detail").message.contains("missing"));
    assert!(e("view == detail $").message.contains("Unexpected character “$”"));
    assert!(e("view == \"detail").message.contains("quote"));
    assert!(e("view == detail &&").incomplete);
    assert!(e("view ==").incomplete);
    assert!(e("!").incomplete);
    assert!(!e("viw").incomplete);
    assert!(e(&"x".repeat(parse::MAX_LEN + 1)).message.contains("too long"));
}

#[test]
fn bad_input_never_panics() {
    let nested = format!("{}textFocus{}", "(".repeat(150), ")".repeat(150));
    assert!(e(&nested).message.contains("Too many"));
    assert!(e(&"(".repeat(5000)).message.contains("too long"));
    assert!(e(&format!("{}textFocus", "!".repeat(100))).message.contains("Too many"));
    for junk in ["é", "日本語 == 語", "view ==\u{0}", "&&", "||", "==", "!=", "!=!=", "()", "(!)", "!(", "'", "\"", "view == \u{1F600}", "view\u{301} == detail"] {
        let _ = parse(junk);
        let _ = suggest::suggest(junk, junk.len());
        for i in 0..=junk.len() {
            let _ = suggest::suggest(junk, i);
        }
    }
}

#[test]
fn chips_say_negation_clearly() {
    let atoms = |src: &str| -> Vec<String> {
        p(src)
            .chips()
            .into_iter()
            .map(|c| match c {
                Chip::Atom(s) => s,
                Chip::Word(w) => w.to_string(),
                Chip::Open => "(".into(),
                Chip::Close => ")".into(),
            })
            .collect()
    };
    assert_eq!(atoms("view == detail"), ["Loupe View"]);
    assert_eq!(atoms("view != detail"), ["Not Loupe View"]);
    assert_eq!(atoms("!(view == detail)"), ["Not Loupe View"]);
    assert_eq!(atoms("!(view != detail)"), ["Loupe View"]);
    assert_eq!(atoms("textFocus"), ["Typing"]);
    assert_eq!(atoms("!textFocus"), ["Not Typing"]);
    assert_eq!(atoms("!!textFocus"), ["Typing"]);
    assert_eq!(atoms("view == detail && tool == crop"), ["Loupe View", "and", "Crop Tool"]);
    assert_eq!(atoms("view == detail || !textFocus"), ["Loupe View", "or", "Not Typing"]);
    assert_eq!(atoms("(view == detail || view == compare) && !textFocus"), ["(", "Loupe View", "or", "Compare View", ")", "and", "Not Typing"]);
    assert_eq!(atoms("!(view == detail && tool == crop)"), ["Not", "(", "Loupe View", "and", "Crop Tool", ")"]);
}

#[test]
fn two_conditions_overlap_unless_they_exclude_each_other() {
    let o = |a: &str, b: &str| expr::overlap(parse(a).unwrap().as_ref(), parse(b).unwrap().as_ref()).is_some();
    // no condition always applies
    assert!(o("", "view == detail"));
    assert!(o("", ""));
    assert!(o("view == detail", "tool == crop"));
    assert!(!o("view == detail", "view == grid"));
    assert!(!o("view == detail", "view != detail"));
    assert!(!o("tool == crop", "tool != crop"));
    assert!(!o("textFocus", "!textFocus"));
    // negations: not detail still includes the grid
    assert!(o("view != detail", "view == grid"));
    assert!(o("!(view == detail)", "view != compare"));
    assert!(!o("!(view == detail)", "view == detail"));
    assert!(!o("!(view == detail || view == grid)", "view == grid"));
    assert!(o("!(view == detail && tool == crop)", "view == detail"), "detail without crop is outside the negated group");
    assert!(!o("!(view == detail && tool == crop)", "view == detail && tool == crop"));
    // gridView follows view
    assert!(!o("gridView", "view == detail"));
    assert!(o("gridView", "view == square"));
    assert!(!o("!gridView", "view == grid"));
    assert!(!o("multiSelection", "!hasSelection"));
}

#[test]
fn a_condition_that_can_never_hold_is_noticed() {
    assert!(!expr::satisfiable(&p("view == detail && view == grid")));
    assert!(!expr::satisfiable(&p("textFocus && !textFocus")));
    assert!(expr::satisfiable(&p("view == detail || view == grid")));
    assert!(expr::satisfiable(&p("!(view == detail)")));
}

#[test]
fn the_witness_names_the_shared_context() {
    let a = p("view == detail");
    let b = p("!textFocus");
    let cx = expr::overlap(Some(&a), Some(&b)).unwrap();
    assert_eq!(expr::describe(&cx, Some(&a), Some(&b)), "Loupe View, Not Typing");
}

#[test]
fn every_key_and_value_is_typable() {
    for (i, k) in keys::KEYS.iter().enumerate() {
        assert_eq!(keys::key_index(k.name), Some(i));
        assert!(!k.desc.is_empty());
        if k.is_bool() {
            p(k.name);
            p(&format!("!{}", k.name));
        }
        for v in k.values() {
            p(&format!("{} == {}", k.name, v.name));
            p(&format!("{} != {}", k.name, v.name));
        }
    }
    assert_eq!((keys::VIEW, keys::SLIDESHOW), (keys::key_index("view").unwrap(), keys::key_index("slideshow").unwrap()));
}

fn labels(text: &str) -> Vec<String> {
    suggest::suggest(text, text.len()).items.into_iter().map(|i| i.label).collect()
}

#[test]
fn autocomplete_offers_keys_and_negated_forms_first() {
    let all = labels("");
    assert_eq!(all.first().map(String::as_str), Some("view"));
    for want in ["tool", "textFocus", "!textFocus", "!hasSelection", "!", "!(", "("] {
        assert!(all.iter().any(|l| l == want), "{want} missing from {all:?}");
    }
    assert!(!all.iter().any(|l| l == "!view"), "an enumeration key is negated with !=, not with !");
    assert!(all.iter().all(|l| !l.is_empty()));
    assert!(suggest::suggest("", 0).items.iter().all(|i| !i.detail.is_empty()), "each suggestion has a description");
}

#[test]
fn autocomplete_filters_as_you_type() {
    assert_eq!(labels("vi").first().map(String::as_str), Some("view"), "names that start with the word come first");
    assert_eq!(&labels("te")[..2], ["textFocus", "!textFocus"]);
    // a description word finds the key: healing is the Remove tool's
    assert!(labels("view == hea").is_empty());
    assert_eq!(labels("tool == heal"), ["remove"]);
    assert!(labels("nonsense").is_empty());
    // after && and ||, after ( and after !, a key is expected
    for t in ["view == detail && ", "view == detail || ", "(", "!", "(!", "!("] {
        assert!(labels(t).contains(&"view".to_string()), "{t}: {:?}", labels(t));
    }
    assert!(labels("!").contains(&"hasPhoto".to_string()));
}

#[test]
fn autocomplete_follows_the_grammar() {
    // after an enumeration key: the comparison, both polarities
    assert_eq!(labels("view "), ["==", "!="]);
    assert_eq!(labels("view !"), ["!="]);
    assert_eq!(labels("view ="), ["=="]);
    // after the operator: that key's values
    assert_eq!(labels("view == "), ["grid", "square", "detail", "compare", "survey", "reference", "people"]);
    assert_eq!(labels("view != co"), ["compare"]);
    assert!(labels("tool != ").contains(&"crop".to_string()));
    // after a value or an on / off key: and, or
    assert_eq!(labels("view == detail "), ["&&", "||"]);
    assert_eq!(labels("textFocus "), ["&&", "||"]);
    assert_eq!(labels("!textFocus &"), ["&&"]);
    // a close parenthesis only while one is open
    assert_eq!(labels("(view == detail "), ["&&", "||", ")"]);
    assert_eq!(labels("!(view == detail "), ["&&", "||", ")"]);
    // nonsense gets nothing
    assert!(labels("view == detail view").is_empty());
    assert!(labels("view @").is_empty());
    assert!(labels("foo ").is_empty());
}

#[test]
fn accepting_a_suggestion_replaces_the_word_and_moves_on() {
    let take = |text: &str, label: &str| {
        let s = suggest::suggest(text, text.len());
        let item = s.items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("{label} not offered at {text:?}"));
        suggest::apply(text, &s, item)
    };
    assert_eq!(take("vi", "view"), ("view ".to_string(), 5));
    assert_eq!(take("view ", "!="), ("view != ".to_string(), 8));
    assert_eq!(take("view != d", "detail").0, "view != detail ");
    assert_eq!(take("view == detail && te", "!textFocus").0, "view == detail && !textFocus ");
    assert_eq!(take("!", "textFocus").0, "!textFocus ");
    assert_eq!(take("", "!(").0, "!(");
    // in the middle of a word the whole word is replaced
    let s = suggest::suggest("view == dxx && brush", 9);
    let item = s.items.iter().find(|i| i.label == "detail").unwrap();
    assert_eq!(suggest::apply("view == dxx && brush", &s, item).0, "view == detail  && brush");
    // a whole chain, one Tab at a time, is a valid condition
    let mut text = String::new();
    for label in ["tool", "!=", "crop", "&&", "!textFocus"] {
        text = take(&text, label).0;
    }
    assert_eq!(p(&text).text(), "tool != crop && !textFocus");
    assert!(labels("view ").iter().all(|l| l != "x"));
    assert!(suggest::suggest("vi", 2).items.iter().all(|i| i.kind == ItemKind::Key));
}

#[test]
fn the_context_keeps_grid_view_in_step() {
    let cx = Context::default().with("view", "square");
    assert!(cx.flag(keys::GRID_VIEW) && cx.consistent());
    let cx = Context::default().with("view", "detail");
    assert!(!cx.flag(keys::GRID_VIEW) && cx.consistent());
}
