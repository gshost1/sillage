//! Paper Fig. 4 / Listing 4.

mod common;

use sillage_core::Lang;
use sillage_parse::parse_snippet;

const LISTING4: &str = "if (view instanceof ViewGroup) {
    for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
        View innerView = ((ViewGroup) view).getChildAt(i);
    }
}";

#[test]
fn listing4_tree_matches_fig4() {
    let t = parse_snippet(Lang::Java, LISTING4).unwrap();
    t.validate().unwrap();
    common::check_spans(&t);
    println!("{}", common::dump(&t));

    assert_eq!(t.label(0), "if##");
    let labels = common::labels(&t);
    for want in ["if##", "(#)", "#instanceof#", "{#}", "#.#", "(#)#", "#()", "#(#)", "#<#", "#++", "#=#", "int#", "#;#;#", "for(#)#"] {
        assert!(labels.iter().any(|l| l == want), "missing label {want:?} in {labels:?}");
    }

    let toks = common::tokens(&t);
    let var_of = |name: &str| -> Vec<Option<u32>> {
        toks.iter().filter(|(t, _)| t == name).map(|(_, v)| *v).collect()
    };
    let view = var_of("view");
    assert_eq!(view.len(), 3);
    assert!(view[0].is_some());
    assert!(view.iter().all(|v| *v == view[0]));
    for name in ["ViewGroup", "getChildCount", "getChildAt", "View"] {
        let vs = var_of(name);
        assert!(!vs.is_empty(), "{name} missing");
        assert!(vs.iter().all(Option::is_none), "{name} must not be a variable");
    }
    let i = var_of("i");
    assert_eq!(i.len(), 4);
    assert!(i[0].is_some() && i.iter().all(|v| *v == i[0]) && i[0] != view[0]);
    let inner = var_of("innerView");
    assert!(inner[0].is_some() && inner[0] != i[0] && inner[0] != view[0]);
    assert_eq!(var_of("0"), vec![None]);
}
